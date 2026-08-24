"""Deterministic, LLM-free routing eval of config/agents.toml (inbox #84).

Second, independent and free signal next to the R-07 benchmark loop: a
TF-IDF match of test queries against each agent's *description surface*
(display_name + supports_types + supports_languages + agent_id tokens),
run on every PR that touches the catalog. It catches two regression
classes before they reach route_task:

  - description regressions — a capability word disappears from an
    agent's section and its queries stop ranking it first (metric:
    trigger rank-1 rate, gated by a ratchet ``--min-rank1 N``);
  - description collisions — two agents whose description surfaces are
    lexically indistinguishable (pairwise cosine: warn >= 0.5,
    error >= 0.75), i.e. the catalog itself cannot tell them apart.

Case fixtures live in ``tests/fixtures/routing-eval/cases.toml`` as a
pinned ``[[case]]`` set. A ``positive`` case declares the agent that must
be rank-1 with a positive score. A ``negative`` case declares the agent
that must NOT win and an ``owner`` agent that must strictly outrank it —
a real pairwise routing check, not just "absent from top-1".

This is a lexical model of the catalog surface, deliberately simpler
than route_task (no DT, no invariants, no stats). It does not replace
the benchmark signal; it guards the words the benchmark never sees.
Sample this port follows: agent-skills ``scripts/run-evals.js`` (TF-IDF,
rank-1 ratchet, collision detector).

Usage:
    uv run python scripts/eval_agents_toml.py
    uv run python scripts/eval_agents_toml.py --min-rank1 80

Exit codes: 0 = pass; 1 = eval failure (ratchet miss, failed negative
case, or error-level collision); 2 = input error.
"""

from __future__ import annotations

import argparse
import math
import re
import sys
import tomllib
from collections import Counter
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_AGENTS_TOML = REPO_ROOT / "config" / "agents.toml"
DEFAULT_CASES = REPO_ROOT / "tests" / "fixtures" / "routing-eval" / "cases.toml"

# The [[case]] fixture contract this loader understands. A bump means
# the semantics changed; a file declaring anything else is rejected
# rather than silently mis-evaluated.
CASES_SCHEMA_VERSION = 1

# Ratchet floor used by CI ("the floor may rise, never fall for a green
# build"). The shipped fixture set currently scores 100%; the gap is
# headroom for benign catalog edits, mirroring agent-skills' 86% -> 80.
CI_MIN_RANK1 = 80.0

# Pairwise cosine thresholds for the description-collision detector
# (same levels as agent-skills run-evals.js).
COLLISION_WARN = 0.5
COLLISION_ERROR = 0.75

# Tokens carrying no routing signal. Kept small and generic on purpose:
# capability vocabulary must never land here.
STOPWORDS = frozenset(
    """
    a an and are as at be been being by can could did do does for from
    has have how i if in into is it its me my no not of on or our over
    please should so some that the them then these this those to under
    up was we what when where which will with would you your
    add change create edit help make need needs new use using via want
    wants write
    """.split()
)

# Domain-vocabulary normalization: natural-query words -> the canonical
# tokens used by agents.toml (supports_types / supports_languages).
ALIASES = {
    "audit": "review",
    "bug": "bugfix",
    "cleanup": "refactor",
    "debug": "bugfix",
    "doc": "docs",
    "document": "docs",
    "documentation": "docs",
    "explore": "research",
    "fix": "bugfix",
    "golang": "go",
    "hotfix": "bugfix",
    "investigate": "research",
    "js": "javascript",
    "py": "python",
    "readme": "docs",
    "restructure": "refactor",
    "rs": "rust",
    "simplify": "refactor",
    "ts": "typescript",
}

_WORD_RE = re.compile(r"[a-z0-9]+")


class EvalInputError(Exception):
    """A fixture or catalog input is missing or malformed."""


def _stem(token: str) -> str:
    """Naive suffix stripper — just enough for the catalog vocabulary."""
    for suffix in ("ing", "ed"):
        if token.endswith(suffix) and len(token) - len(suffix) >= 3:
            return token[: -len(suffix)]
    if token.endswith("s") and not token.endswith("ss") and len(token) >= 4:
        return token[:-1]
    return token


def tokenize(text: str) -> list[str]:
    """Lowercase, split, drop stopwords/numbers, normalize to catalog vocab."""
    tokens: list[str] = []
    for raw in _WORD_RE.findall(text.lower()):
        if raw.isdigit() or raw in STOPWORDS:
            continue
        token = ALIASES.get(raw)
        if token is None:
            token = _stem(raw)
            token = ALIASES.get(token, token)
        if len(token) < 2 or token in STOPWORDS:
            continue
        tokens.append(token)
    return tokens


def agent_doc(agent_id: str, cfg: dict[str, object]) -> list[str]:
    """Build the description-surface token document for one agent.

    Capability vocabulary (supports_types / supports_languages) is
    weighted 2x — it is the surface route_task actually filters on;
    agent_id and display_name tokens count once.
    """
    tokens = tokenize(agent_id)
    display_name = cfg.get("display_name", "")
    if isinstance(display_name, str):
        tokens += tokenize(display_name)
    for key in ("supports_types", "supports_languages"):
        values = cfg.get(key, [])
        if isinstance(values, list):
            for value in values:
                if isinstance(value, str):
                    tokens += tokenize(value) * 2
    return tokens


def load_agents(path: Path) -> dict[str, list[str]]:
    """Load agents.toml into {agent_id: description tokens}."""
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except OSError as exc:
        raise EvalInputError(f"cannot read agents toml {path}: {exc}") from exc
    except tomllib.TOMLDecodeError as exc:
        raise EvalInputError(f"invalid TOML in {path}: {exc}") from exc
    docs = {
        agent_id: agent_doc(agent_id, cfg)
        for agent_id, cfg in data.items()
        if isinstance(cfg, dict)
    }
    if not docs:
        raise EvalInputError(f"no agent sections found in {path}")
    return docs


class TfIdfIndex:
    """TF-IDF vectors over agent description documents, cosine ranking."""

    def __init__(self, docs: dict[str, list[str]]) -> None:
        self._doc_count = len(docs)
        df: Counter[str] = Counter()
        for tokens in docs.values():
            df.update(set(tokens))
        self._df = df
        self._vectors = {
            agent_id: self._vectorize(Counter(tokens))
            for agent_id, tokens in docs.items()
        }

    @property
    def agent_ids(self) -> list[str]:
        """All indexed agent ids, sorted."""
        return sorted(self._vectors)

    def _idf(self, token: str) -> float:
        return math.log((self._doc_count + 1) / (self._df.get(token, 0) + 1)) + 1.0

    def _vectorize(self, counts: Counter[str]) -> dict[str, float]:
        vector = {t: c * self._idf(t) for t, c in counts.items()}
        norm = math.sqrt(sum(w * w for w in vector.values()))
        if norm > 0.0:
            vector = {t: w / norm for t, w in vector.items()}
        return vector

    def rank(self, query: str) -> list[tuple[str, float]]:
        """Rank all agents against a query; ties break by agent_id."""
        query_vector = self._vectorize(Counter(tokenize(query)))
        scored = [
            (agent_id, sum(w * doc.get(t, 0.0) for t, w in query_vector.items()))
            for agent_id, doc in self._vectors.items()
        ]
        return sorted(scored, key=lambda pair: (-pair[1], pair[0]))

    def similarity(self, agent_a: str, agent_b: str) -> float:
        """Cosine similarity between two agents' description vectors."""
        doc_a = self._vectors[agent_a]
        doc_b = self._vectors[agent_b]
        return sum(w * doc_b.get(t, 0.0) for t, w in doc_a.items())


@dataclass(frozen=True)
class Collision:
    """Two agents whose description surfaces are lexically too close."""

    agent_a: str
    agent_b: str
    similarity: float
    level: str  # "warn" | "error"


def detect_collisions(
    index: TfIdfIndex,
    warn: float = COLLISION_WARN,
    error: float = COLLISION_ERROR,
) -> list[Collision]:
    """Pairwise description-collision scan, most similar first."""
    collisions = []
    agent_ids = index.agent_ids
    for i, agent_a in enumerate(agent_ids):
        for agent_b in agent_ids[i + 1 :]:
            similarity = index.similarity(agent_a, agent_b)
            if similarity >= warn:
                level = "error" if similarity >= error else "warn"
                collisions.append(Collision(agent_a, agent_b, similarity, level))
    return sorted(collisions, key=lambda c: (-c.similarity, c.agent_a, c.agent_b))


@dataclass(frozen=True)
class Case:
    """One fixture case; ``expect`` for positive, ``agent``+``owner`` else."""

    case_id: str
    kind: str  # "positive" | "negative"
    query: str
    expect: str = ""
    agent: str = ""
    owner: str = ""


def _require_known(case_id: str, field: str, value: str, known: set[str]) -> None:
    if not value:
        raise EvalInputError(f"case {case_id!r}: missing required field {field!r}")
    if value not in known:
        raise EvalInputError(
            f"case {case_id!r}: unknown agent id {value!r} in {field!r}"
        )


def load_cases(path: Path, agent_ids: set[str]) -> list[Case]:
    """Load and validate the pinned [[case]] fixture set."""
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except OSError as exc:
        raise EvalInputError(f"cannot read cases file {path}: {exc}") from exc
    except tomllib.TOMLDecodeError as exc:
        raise EvalInputError(f"invalid TOML in {path}: {exc}") from exc

    schema_version = data.get("schema_version")
    if schema_version != CASES_SCHEMA_VERSION:
        raise EvalInputError(
            f"{path}: schema_version must be {CASES_SCHEMA_VERSION}, "
            f"got {schema_version!r}"
        )

    raw_cases = data.get("case", [])
    if not isinstance(raw_cases, list) or not raw_cases:
        raise EvalInputError(f"{path}: no [[case]] entries found")

    cases: list[Case] = []
    seen: set[str] = set()
    for raw in raw_cases:
        case_id = str(raw.get("id", ""))
        if not case_id:
            raise EvalInputError(f"{path}: a [[case]] entry is missing 'id'")
        if case_id in seen:
            raise EvalInputError(f"{path}: duplicate case id {case_id!r}")
        seen.add(case_id)
        kind = str(raw.get("kind", ""))
        query = str(raw.get("query", ""))
        if not query:
            raise EvalInputError(f"case {case_id!r}: missing required field 'query'")
        case = Case(
            case_id=case_id,
            kind=kind,
            query=query,
            expect=str(raw.get("expect", "")),
            agent=str(raw.get("agent", "")),
            owner=str(raw.get("owner", "")),
        )
        if kind == "positive":
            _require_known(case_id, "expect", case.expect, agent_ids)
        elif kind == "negative":
            _require_known(case_id, "agent", case.agent, agent_ids)
            _require_known(case_id, "owner", case.owner, agent_ids)
            if case.agent == case.owner:
                raise EvalInputError(
                    f"case {case_id!r}: 'agent' and 'owner' must differ"
                )
        else:
            raise EvalInputError(f"case {case_id!r}: unknown kind {kind!r}")
        cases.append(case)
    return cases


@dataclass(frozen=True)
class CaseResult:
    """Outcome of one case against the index."""

    case: Case
    passed: bool
    detail: str


@dataclass(frozen=True)
class EvalReport:
    """Aggregated eval outcome over the whole case set."""

    results: list[CaseResult]
    positives_total: int
    rank1_hits: int
    rank1_rate: float

    @property
    def failures(self) -> list[CaseResult]:
        """All failed cases, fixture order preserved."""
        return [result for result in self.results if not result.passed]


def _score_of(ranking: list[tuple[str, float]], agent_id: str) -> float:
    for ranked_id, score in ranking:
        if ranked_id == agent_id:
            return score
    return 0.0


def evaluate(index: TfIdfIndex, cases: list[Case]) -> EvalReport:
    """Run every case; rank-1 rate is computed over positive cases only."""
    results: list[CaseResult] = []
    positives_total = 0
    rank1_hits = 0
    for case in cases:
        ranking = index.rank(case.query)
        top_id, top_score = ranking[0]
        if case.kind == "positive":
            positives_total += 1
            passed = top_id == case.expect and top_score > 0.0
            if passed:
                rank1_hits += 1
            detail = f"rank-1 = {top_id} ({top_score:.3f}), expected {case.expect}"
        else:
            owner_score = _score_of(ranking, case.owner)
            agent_score = _score_of(ranking, case.agent)
            passed = owner_score > agent_score
            detail = (
                f"owner {case.owner} ({owner_score:.3f}) vs "
                f"agent {case.agent} ({agent_score:.3f})"
            )
        results.append(CaseResult(case=case, passed=passed, detail=detail))
    rank1_rate = 100.0 * rank1_hits / positives_total if positives_total else 100.0
    return EvalReport(
        results=results,
        positives_total=positives_total,
        rank1_hits=rank1_hits,
        rank1_rate=rank1_rate,
    )


def _print_report(report: EvalReport, collisions: list[Collision]) -> None:
    for result in report.results:
        status = "PASS" if result.passed else "FAIL"
        print(
            f"[{status}] {result.case.kind:<8} {result.case.case_id}: {result.detail}"
        )
    for collision in collisions:
        print(
            f"[{collision.level.upper()}] collision {collision.agent_a} ~ "
            f"{collision.agent_b}: cosine {collision.similarity:.3f}"
        )
    print(
        f"trigger rank-1 rate: {report.rank1_rate:.1f}% "
        f"({report.rank1_hits}/{report.positives_total} positive cases)"
    )


def main(argv: list[str] | None = None) -> int:
    """CLI entry point; see module docstring for exit codes."""
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--agents-toml",
        type=Path,
        default=DEFAULT_AGENTS_TOML,
        help="path to config/agents.toml",
    )
    parser.add_argument(
        "--cases",
        type=Path,
        default=DEFAULT_CASES,
        help="path to the [[case]] fixture set",
    )
    parser.add_argument(
        "--min-rank1",
        type=float,
        default=None,
        help="ratchet floor for the trigger rank-1 rate, in percent",
    )
    args = parser.parse_args(argv)

    try:
        docs = load_agents(args.agents_toml)
        index = TfIdfIndex(docs)
        cases = load_cases(args.cases, set(docs))
    except EvalInputError as exc:
        print(f"input error: {exc}", file=sys.stderr)
        return 2

    collisions = detect_collisions(index)
    report = evaluate(index, cases)
    _print_report(report, collisions)

    failed = False
    error_collisions = [c for c in collisions if c.level == "error"]
    if error_collisions:
        print(f"FAIL: {len(error_collisions)} error-level description collision(s)")
        failed = True
    failed_negatives = [r for r in report.failures if r.case.kind == "negative"]
    if failed_negatives:
        print(f"FAIL: {len(failed_negatives)} negative case(s) not owned")
        failed = True
    if args.min_rank1 is not None:
        # A ratchet over zero positive cases is vacuous: stripping the
        # positives from the fixture set must not turn the gate green.
        if report.positives_total == 0:
            print("FAIL: --min-rank1 given but the case set has no positive cases")
            failed = True
        elif report.rank1_rate < args.min_rank1:
            print(
                f"FAIL: rank-1 rate {report.rank1_rate:.1f}% is below the "
                f"ratchet floor {args.min_rank1:.1f}%"
            )
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
