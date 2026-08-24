"""Tests for scripts/eval_agents_toml.py (inbox #84).

Deterministic, LLM-free routing eval of the agents.toml description
surface: TF-IDF ranking of test queries, trigger rank-1 rate with a
ratchet, and a description-collision detector. Negative cases declare
an owner agent that must outrank the tested one (pairwise routing
check, agent-skills Tier-2 style).
"""

from __future__ import annotations

from pathlib import Path

import pytest

from scripts.eval_agents_toml import (
    CI_MIN_RANK1,
    COLLISION_ERROR,
    COLLISION_WARN,
    EvalInputError,
    TfIdfIndex,
    detect_collisions,
    evaluate,
    load_agents,
    load_cases,
    main,
    tokenize,
)

REPO_ROOT = Path(__file__).resolve().parent.parent
AGENTS_TOML = REPO_ROOT / "config" / "agents.toml"
CASES_TOML = REPO_ROOT / "tests" / "fixtures" / "routing-eval" / "cases.toml"


# ---------------------------------------------------------------------------
# tokenize
# ---------------------------------------------------------------------------


def test_tokenize_lowercases_splits_and_drops_stopwords() -> None:
    assert tokenize("Review the Python module!") == ["review", "python", "module"]


def test_tokenize_normalizes_domain_aliases() -> None:
    # "fixing" stems to "fix", both "fix" and "bug" alias to the catalog
    # vocabulary token "bugfix".
    assert tokenize("fixing a bug") == ["bugfix", "bugfix"]


def test_tokenize_drops_pure_numbers() -> None:
    assert tokenize("sonnet 4 6") == ["sonnet"]


# ---------------------------------------------------------------------------
# ranking
# ---------------------------------------------------------------------------


def make_index(docs: dict[str, list[str]]) -> TfIdfIndex:
    return TfIdfIndex(docs)


def test_rank_prefers_lexical_match() -> None:
    index = make_index(
        {
            "a": ["review", "python", "review"],
            "b": ["test", "go", "feature"],
        }
    )
    ranking = index.rank("review some python")
    assert [agent_id for agent_id, _ in ranking] == ["a", "b"]
    assert ranking[0][1] > ranking[1][1]


def test_rank_tie_breaks_by_agent_id() -> None:
    index = make_index(
        {
            "beta": ["review", "python"],
            "alpha": ["review", "python"],
        }
    )
    ranking = index.rank("review python")
    assert [agent_id for agent_id, _ in ranking] == ["alpha", "beta"]


def test_rank_unmatched_query_scores_zero() -> None:
    index = make_index({"a": ["review"]})
    ranking = index.rank("kubernetes cluster autoscaler")
    assert ranking == [("a", 0.0)]


# ---------------------------------------------------------------------------
# collisions
# ---------------------------------------------------------------------------


def test_detect_collisions_identical_docs_is_error() -> None:
    index = make_index(
        {
            "a": ["review", "python"],
            "b": ["review", "python"],
        }
    )
    collisions = detect_collisions(index)
    assert len(collisions) == 1
    collision = collisions[0]
    assert {collision.agent_a, collision.agent_b} == {"a", "b"}
    assert collision.similarity == pytest.approx(1.0)
    assert collision.level == "error"


def test_detect_collisions_disjoint_docs_is_clean() -> None:
    index = make_index(
        {
            "a": ["review", "python"],
            "b": ["test", "go"],
        }
    )
    assert detect_collisions(index) == []


def test_collision_thresholds_are_ordered() -> None:
    assert 0.0 < COLLISION_WARN < COLLISION_ERROR <= 1.0


# ---------------------------------------------------------------------------
# load_agents (real catalog)
# ---------------------------------------------------------------------------


def test_load_agents_builds_docs_from_real_config() -> None:
    docs = load_agents(AGENTS_TOML)
    assert "claude_code@claude-sonnet-4-6" in docs
    assert "aider" in docs
    # The description surface includes the routing-relevant capability
    # vocabulary: supported task types and languages.
    doc = docs["claude_code@claude-sonnet-4-6"]
    assert "review" in doc
    assert "python" in doc


# ---------------------------------------------------------------------------
# load_cases
# ---------------------------------------------------------------------------


def write_cases(tmp_path: Path, body: str) -> Path:
    path = tmp_path / "cases.toml"
    path.write_text(body, encoding="utf-8")
    return path


def test_load_cases_rejects_unknown_agent(tmp_path: Path) -> None:
    path = write_cases(
        tmp_path,
        """
        schema_version = 1

        [[case]]
        id = "c1"
        kind = "positive"
        query = "review python"
        expect = "ghost@nowhere"
        """,
    )
    with pytest.raises(EvalInputError, match="ghost@nowhere"):
        load_cases(path, {"a"})


def test_load_cases_rejects_negative_without_owner(tmp_path: Path) -> None:
    path = write_cases(
        tmp_path,
        """
        schema_version = 1

        [[case]]
        id = "c1"
        kind = "negative"
        query = "review python"
        agent = "a"
        """,
    )
    with pytest.raises(EvalInputError, match="owner"):
        load_cases(path, {"a"})


def test_load_cases_rejects_duplicate_ids(tmp_path: Path) -> None:
    path = write_cases(
        tmp_path,
        """
        schema_version = 1

        [[case]]
        id = "c1"
        kind = "positive"
        query = "review python"
        expect = "a"

        [[case]]
        id = "c1"
        kind = "positive"
        query = "review rust"
        expect = "a"
        """,
    )
    with pytest.raises(EvalInputError, match="duplicate"):
        load_cases(path, {"a"})


def test_load_cases_rejects_wrong_schema_version(tmp_path: Path) -> None:
    path = write_cases(
        tmp_path,
        """
        schema_version = 2

        [[case]]
        id = "c1"
        kind = "positive"
        query = "review python"
        expect = "a"
        """,
    )
    with pytest.raises(EvalInputError, match="schema_version"):
        load_cases(path, {"a"})


def test_load_cases_rejects_missing_schema_version(tmp_path: Path) -> None:
    path = write_cases(
        tmp_path,
        """
        [[case]]
        id = "c1"
        kind = "positive"
        query = "review python"
        expect = "a"
        """,
    )
    with pytest.raises(EvalInputError, match="schema_version"):
        load_cases(path, {"a"})


def test_load_cases_parses_positive_and_negative(tmp_path: Path) -> None:
    path = write_cases(
        tmp_path,
        """
        schema_version = 1

        [[case]]
        id = "pos"
        kind = "positive"
        query = "review python"
        expect = "a"

        [[case]]
        id = "neg"
        kind = "negative"
        query = "review python"
        agent = "b"
        owner = "a"
        """,
    )
    cases = load_cases(path, {"a", "b"})
    assert [case.case_id for case in cases] == ["pos", "neg"]
    assert cases[0].kind == "positive"
    assert cases[0].expect == "a"
    assert cases[1].kind == "negative"
    assert cases[1].agent == "b"
    assert cases[1].owner == "a"


# ---------------------------------------------------------------------------
# evaluate
# ---------------------------------------------------------------------------


def test_evaluate_counts_positive_hits_and_misses(tmp_path: Path) -> None:
    index = make_index(
        {
            "a": ["review", "python"],
            "b": ["test", "go"],
        }
    )
    path = write_cases(
        tmp_path,
        """
        schema_version = 1

        [[case]]
        id = "hit"
        kind = "positive"
        query = "review python"
        expect = "a"

        [[case]]
        id = "miss"
        kind = "positive"
        query = "test go"
        expect = "a"
        """,
    )
    cases = load_cases(path, {"a", "b"})
    report = evaluate(index, cases)
    assert report.positives_total == 2
    assert report.rank1_hits == 1
    assert report.rank1_rate == pytest.approx(50.0)
    assert [failure.case.case_id for failure in report.failures] == ["miss"]


def test_evaluate_negative_requires_owner_to_outrank(tmp_path: Path) -> None:
    index = make_index(
        {
            "reviewer": ["review", "python"],
            "tester": ["test", "go", "python"],
        }
    )
    path = write_cases(
        tmp_path,
        """
        schema_version = 1

        [[case]]
        id = "neg-pass"
        kind = "negative"
        query = "review python"
        agent = "tester"
        owner = "reviewer"

        [[case]]
        id = "neg-fail"
        kind = "negative"
        query = "test go"
        agent = "tester"
        owner = "reviewer"
        """,
    )
    cases = load_cases(path, {"reviewer", "tester"})
    report = evaluate(index, cases)
    # Negative cases do not enter the rank-1 rate.
    assert report.positives_total == 0
    assert [failure.case.case_id for failure in report.failures] == ["neg-fail"]


# ---------------------------------------------------------------------------
# main (CLI)
# ---------------------------------------------------------------------------


def test_main_passes_on_shipped_fixtures_at_ci_floor(
    capsys: pytest.CaptureFixture[str],
) -> None:
    # Pins the CI job green: the shipped case set over the shipped
    # agents.toml must clear the ratchet floor with no error collisions.
    exit_code = main(
        [
            "--agents-toml",
            str(AGENTS_TOML),
            "--cases",
            str(CASES_TOML),
            "--min-rank1",
            str(CI_MIN_RANK1),
        ]
    )
    output = capsys.readouterr().out
    assert exit_code == 0, output
    assert "rank-1 rate" in output


def test_main_fails_ratchet_below_floor(tmp_path: Path) -> None:
    # A query with no lexical overlap cannot be rank-1 for its expected
    # agent with a positive score -> rank-1 rate 0% -> ratchet trips.
    path = write_cases(
        tmp_path,
        """
        schema_version = 1

        [[case]]
        id = "doomed"
        kind = "positive"
        query = "kubernetes cluster autoscaler tuning"
        expect = "aider"
        """,
    )
    exit_code = main(
        [
            "--agents-toml",
            str(AGENTS_TOML),
            "--cases",
            str(path),
            "--min-rank1",
            "80",
        ]
    )
    assert exit_code == 1


def test_main_fails_ratchet_with_no_positive_cases(tmp_path: Path) -> None:
    # A ratchet over zero positive cases is vacuous — stripping the
    # positives from the fixture set must not turn the gate green.
    path = write_cases(
        tmp_path,
        """
        schema_version = 1

        [[case]]
        id = "neg-only"
        kind = "negative"
        query = "audit this python module for style issues"
        agent = "aider"
        owner = "opencode@glm-5.1"
        """,
    )
    exit_code = main(
        [
            "--agents-toml",
            str(AGENTS_TOML),
            "--cases",
            str(path),
            "--min-rank1",
            "80",
        ]
    )
    assert exit_code == 1


def test_main_reports_input_error_for_missing_cases_file(tmp_path: Path) -> None:
    exit_code = main(
        [
            "--agents-toml",
            str(AGENTS_TOML),
            "--cases",
            str(tmp_path / "nope.toml"),
        ]
    )
    assert exit_code == 2


def test_shipped_catalog_has_no_error_collisions() -> None:
    # The collision detector is a finding generator (warn) and a gate
    # (error). The shipped catalog must be below the error threshold —
    # otherwise the CI job would be red on day one.
    docs = load_agents(AGENTS_TOML)
    index = TfIdfIndex(docs)
    errors = [c for c in detect_collisions(index) if c.level == "error"]
    assert errors == []
