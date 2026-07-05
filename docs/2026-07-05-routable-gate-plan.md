# Routable-Flip Benchmark-Evidence Gate — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Гейт ADR-ECO-003a D4: PR, вводящий пару в роутинг (`routable=true` в `config/agents-catalog.toml`), обязан нести валидный `bench`-эвиденс; CI проверяет декларацию (правила A/B), локальный `verify` сверяет её с `benchmark_runs` в `arbiter.db`.

**Architecture:** Один stdlib-only Python-скрипт `scripts/check_routable_gate.py` с сабкомандами `gate` (диффовый, CI) и `verify` (data-gate, локальный). CI-job на `pull_request` гоняет `gate` против `base.sha`-версии каталога. Тесты — pytest в `tests/`, который добавляется в CI (закрывает pre-existing дыру).

**Tech Stack:** Python 3.12 stdlib (`tomllib`, `sqlite3`, `argparse`, `json`, `math`, `re`, `datetime`). Никаких новых зависимостей. Тесты: pytest (уже в dev-deps).

**Spec:** `docs/2026-07-05-routable-gate-design.md` (Draft v3) — источник требований. Semantика скора зеркалит `get_benchmark_score` (`arbiter-mcp/src/db.rs:817-841`); схема таблицы — `db.rs:936-951` (`score_components TEXT NOT NULL`).

## Global Constraints

- Скрипт — stdlib-only: никаких импортов вне стандартной библиотеки Python 3.12.
- Type hints обязательны для всех функций; line length 88; `uv run ruff format --check .`, `uv run ruff check .` и `uv run pyrefly check` должны быть зелёными (все три гоняются в CI python-job).
- Exit-коды: `0` — нарушений нет (warnings допустимы); `1` — policy-нарушения; `2` — невалидный вход/окружение (битый TOML, отсутствующий файл/db, дубликаты `agent_id`, отсутствие таблицы `benchmark_runs`, невалидный `--eps`, не-RFC3339 `ts`).
- Диагностика режимов в stdout (`GATE FAIL …`/`VERIFY FAIL …`/`WARN …`); ошибки класса exit 2 — в stderr (`ERROR: …`).
- Числовые проверки: `bool` не является числом (в Python `bool` — подкласс `int`); `rank_score` — `math.isfinite` и `[0,1]`.
- Тесты импортируют скрипт как модуль: `from scripts.check_routable_gate import …` (паттерн `tests/test_gen_agents_scaffold.py`).
- Коммиты на ветке `feat/routable-gate`, футер `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

---

### Task 1: Загрузка каталога + валидация bench-схемы

**Files:**
- Create: `scripts/check_routable_gate.py`
- Create: `tests/test_routable_gate.py`

**Interfaces:**
- Produces (потребляются Task 2/3):
  - `class GateInputError(Exception)` — класс exit-2-ошибок
  - `load_catalog(path: Path) -> dict[str, Any]`
  - `agents_map(catalog: dict[str, Any], label: str) -> dict[str, dict[str, Any]]` — по `agent_id`, `GateInputError` на дубликат
  - `validate_bench(entry: dict[str, Any]) -> list[str]` — список нарушений (пустой = валидно); проверяет и `tested is True`
  - константы `SUITE_RE`, `DATE_FRESHNESS_DAYS = 7`, `DEFAULT_EPS = 0.05`

- [ ] **Step 1: Написать падающие тесты валидации**

Создать `tests/test_routable_gate.py`:

```python
"""Tests for scripts/check_routable_gate.py (ADR-ECO-003a D4).

The gate has two layers: `gate` validates evidence *declarations* on
routable flips (CI, no DB), `verify` checks declarations against real
`benchmark_runs` rows (local). Design:
docs/2026-07-05-routable-gate-design.md.
"""

from __future__ import annotations

import copy
from pathlib import Path
from typing import Any

import pytest

from scripts.check_routable_gate import (
    GateInputError,
    agents_map,
    load_catalog,
    validate_bench,
)

VALID_BENCH: dict[str, Any] = {
    "benchmark": "code-review",
    "suite": "0123abc",
    "rank_score": 0.9,
    "date": "2026-07-03",
    "run_ids": ["r1", "r2"],
}


def make_entry(**overrides: Any) -> dict[str, Any]:
    entry: dict[str, Any] = {
        "harness": "h1",
        "model": "m1",
        "tested": True,
        "routable": True,
        "bench": copy.deepcopy(VALID_BENCH),
    }
    entry.update(overrides)
    return entry


def bench_with(**overrides: Any) -> dict[str, Any]:
    bench = copy.deepcopy(VALID_BENCH)
    bench.update(overrides)
    return bench


def bench_without(key: str) -> dict[str, Any]:
    bench = copy.deepcopy(VALID_BENCH)
    del bench[key]
    return bench


class TestValidateBench:
    def test_valid_entry_has_no_problems(self) -> None:
        assert validate_bench(make_entry()) == []

    def test_missing_bench_block(self) -> None:
        problems = validate_bench(make_entry(bench=None))
        assert any("bench" in p for p in problems)

    def test_tested_false_is_a_problem(self) -> None:
        problems = validate_bench(make_entry(tested=False))
        assert any("tested" in p for p in problems)

    @pytest.mark.parametrize("key", sorted(VALID_BENCH))
    def test_each_required_key_is_required(self, key: str) -> None:
        problems = validate_bench(make_entry(bench=bench_without(key)))
        assert any(key in p for p in problems)

    @pytest.mark.parametrize(
        "score", [1.5, -0.1, float("nan"), float("inf"), True, "0.9"]
    )
    def test_bad_rank_score(self, score: Any) -> None:
        problems = validate_bench(make_entry(bench=bench_with(rank_score=score)))
        assert any("rank_score" in p for p in problems)

    @pytest.mark.parametrize("suite", ["foo", "", "ABCDEF0", "0123abc!", "012"])
    def test_bad_suite_digest(self, suite: str) -> None:
        problems = validate_bench(make_entry(bench=bench_with(suite=suite)))
        assert any("suite" in p for p in problems)

    @pytest.mark.parametrize("d", ["03.07.2026", "2026-13-01", "2099-01-01", 20260703])
    def test_bad_or_future_date(self, d: Any) -> None:
        problems = validate_bench(make_entry(bench=bench_with(date=d)))
        assert any("date" in p for p in problems)

    @pytest.mark.parametrize("run_ids", [[], ["r1", "r1"], ["r1", ""], "r1", [1]])
    def test_bad_run_ids(self, run_ids: Any) -> None:
        problems = validate_bench(make_entry(bench=bench_with(run_ids=run_ids)))
        assert any("run_ids" in p for p in problems)

    def test_runs_must_match_run_ids_length(self) -> None:
        problems = validate_bench(make_entry(bench=bench_with(runs=3)))
        assert any("runs" in p for p in problems)

    def test_runs_matching_length_is_ok(self) -> None:
        assert validate_bench(make_entry(bench=bench_with(runs=2))) == []

    def test_runs_bool_rejected(self) -> None:
        problems = validate_bench(make_entry(bench=bench_with(runs=True)))
        assert any("runs" in p for p in problems)


class TestCatalogLoading:
    def test_load_missing_file_raises(self, tmp_path: Path) -> None:
        with pytest.raises(GateInputError, match="not found"):
            load_catalog(tmp_path / "nope.toml")

    def test_load_broken_toml_raises(self, tmp_path: Path) -> None:
        p = tmp_path / "broken.toml"
        p.write_text("[agents\nboom")
        with pytest.raises(GateInputError, match="TOML"):
            load_catalog(p)

    def test_duplicate_agent_id_raises(self) -> None:
        catalog = {
            "agents": [
                {"harness": "h1", "model": "m1"},
                {"harness": "h1", "model": "m1"},
            ]
        }
        with pytest.raises(GateInputError, match="duplicate"):
            agents_map(catalog, "head")

    def test_agents_map_keys_by_agent_id(self) -> None:
        catalog = {"agents": [{"harness": "h1", "model": "m1"}]}
        assert list(agents_map(catalog, "base")) == ["h1@m1"]

    def test_agents_map_empty_catalog(self) -> None:
        assert agents_map({}, "base") == {}
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `uv run python -m pytest tests/test_routable_gate.py -v`
Expected: FAIL — `ModuleNotFoundError: No module named 'scripts.check_routable_gate'`.

- [ ] **Step 3: Реализация загрузки и валидации**

Создать `scripts/check_routable_gate.py`:

```python
#!/usr/bin/env python3
"""Routable-flip benchmark-evidence gate (ADR-ECO-003a D4).

Two modes:
- ``gate``:   diff base vs head catalog; enforce evidence rules A/B
  (CI-level, declarative — no DB access).
- ``verify``: check declared evidence against ``benchmark_runs`` rows in
  ``arbiter.db`` (local data-gate).

Stdlib only. Design: docs/2026-07-05-routable-gate-design.md.
Score semantics mirror ``get_benchmark_score``
(arbiter-mcp/src/db.rs:817-841).
"""

from __future__ import annotations

import math
import re
import tomllib
from datetime import date, datetime, timezone
from pathlib import Path
from typing import Any

SUITE_RE = re.compile(r"^[0-9a-f]{7,64}$")
REQUIRED_BENCH_KEYS = ("benchmark", "suite", "rank_score", "date", "run_ids")
DATE_FRESHNESS_DAYS = 7
DEFAULT_EPS = 0.05


class GateInputError(Exception):
    """Invalid input or environment — maps to exit code 2."""


def load_catalog(path: Path) -> dict[str, Any]:
    """Parse a catalog TOML file; raise GateInputError on missing/broken input."""
    try:
        with open(path, "rb") as f:
            return tomllib.load(f)
    except FileNotFoundError as exc:
        raise GateInputError(f"catalog not found: {path}") from exc
    except tomllib.TOMLDecodeError as exc:
        raise GateInputError(f"invalid TOML in {path}: {exc}") from exc


def agent_id(entry: dict[str, Any]) -> str:
    """Canonical join key '{harness}@{model}' (matches benchmark_runs.agent_id)."""
    return f"{entry.get('harness', '')}@{entry.get('model', '')}"


def agents_map(catalog: dict[str, Any], label: str) -> dict[str, dict[str, Any]]:
    """[[agents]] list -> map by agent_id. Duplicates are exit-2 input errors:
    a silent dict collapse would hide an entry from the gate (mirror of V4)."""
    result: dict[str, dict[str, Any]] = {}
    for entry in catalog.get("agents", []):
        aid = agent_id(entry)
        if aid in result:
            raise GateInputError(f"duplicate agent_id {aid!r} in {label} catalog")
        result[aid] = entry
    return result


def _is_number(value: Any) -> bool:
    # bool is an int subclass in Python; a boolean is not a score.
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def _parse_iso_date(value: Any) -> date | None:
    """Strict ISO YYYY-MM-DD string -> date. TOML bare dates (tomllib returns
    datetime.date) are rejected: the schema requires a quoted string."""
    if not isinstance(value, str):
        return None
    try:
        return date.fromisoformat(value)
    except ValueError:
        return None


def validate_bench(entry: dict[str, Any]) -> list[str]:
    """Full schema validation of a routable entry's evidence (design §3).

    Returns a list of human-readable problems; empty list = valid.
    Checks `tested = true` too — a routable pair must be in the ATP sweep.
    """
    problems: list[str] = []
    if entry.get("tested") is not True:
        problems.append("tested must be true for a routable pair")

    bench = entry.get("bench")
    if not isinstance(bench, dict):
        problems.append("missing bench evidence block (inline table)")
        return problems

    for key in REQUIRED_BENCH_KEYS:
        if key not in bench:
            problems.append(f"bench.{key} is required")

    if "benchmark" in bench:
        benchmark = bench["benchmark"]
        if not isinstance(benchmark, str) or not benchmark:
            problems.append("bench.benchmark must be a non-empty string")

    if "suite" in bench:
        suite = bench["suite"]
        if not isinstance(suite, str) or not SUITE_RE.fullmatch(suite):
            problems.append(
                "bench.suite must be a lowercase hex digest (^[0-9a-f]{7,64}$)"
            )

    if "rank_score" in bench:
        score = bench["rank_score"]
        if not _is_number(score) or not math.isfinite(score) or not 0.0 <= score <= 1.0:
            problems.append(
                "bench.rank_score must be a finite number in [0, 1] (bool rejected)"
            )

    if "date" in bench:
        declared = _parse_iso_date(bench["date"])
        if declared is None:
            problems.append("bench.date must be an ISO YYYY-MM-DD string")
        elif declared > datetime.now(timezone.utc).date():
            problems.append("bench.date must not be in the future (UTC)")

    run_ids = bench.get("run_ids")
    if "run_ids" in bench:
        if (
            not isinstance(run_ids, list)
            or not run_ids
            or not all(isinstance(r, str) and r for r in run_ids)
        ):
            problems.append(
                "bench.run_ids must be a non-empty array of non-empty strings"
            )
        elif len(set(run_ids)) != len(run_ids):
            problems.append("bench.run_ids must not contain duplicates")

    if "runs" in bench:
        runs = bench["runs"]
        if not isinstance(runs, int) or isinstance(runs, bool) or runs < 1:
            problems.append("bench.runs must be an integer >= 1")
        elif isinstance(run_ids, list) and runs != len(run_ids):
            problems.append("bench.runs must equal len(bench.run_ids)")

    return problems
```

- [ ] **Step 4: Прогнать тесты**

Run: `uv run python -m pytest tests/test_routable_gate.py -v`
Expected: PASS (все тесты Task 1).

- [ ] **Step 5: Линт/типы + коммит**

```bash
uv run ruff format . && uv run ruff check . && uv run pyrefly check
git add scripts/check_routable_gate.py tests/test_routable_gate.py
git commit -m "feat(gate): catalog loading + bench evidence schema validation (ADR-003a D4)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: Режим `gate` (правила A/B) + CLI

**Files:**
- Modify: `scripts/check_routable_gate.py` (добавить `run_gate`, `main`)
- Modify: `tests/test_routable_gate.py` (добавить gate-тесты)

**Interfaces:**
- Consumes: `load_catalog`, `agents_map`, `validate_bench`, `GateInputError` из Task 1.
- Produces:
  - `run_gate(base_path: Path, head_path: Path) -> int` — 0/1 (2 через исключение)
  - `main(argv: list[str] | None = None) -> int` — CLI-диспетч, ловит `GateInputError` → печать `ERROR: …` в stderr, возврат 2. Task 3 расширит `main` веткой `verify`.

- [ ] **Step 1: Написать падающие gate-тесты**

Добавить в `tests/test_routable_gate.py` (импорт дополнить `main`, `run_gate`):

```python
from scripts.check_routable_gate import main, run_gate  # noqa: E402  (append to imports)

CATALOG_HEADER = """
[models."m1"]
vendor = "v"
status = "active"

[harnesses.h1]
kind = "cli"
shim = "s.py"
routable = true
"""

AGENT_NOT_ROUTABLE = """
[[agents]]
harness = "h1"
model = "m1"
tested = true
routable = false
"""

AGENT_ROUTABLE_NO_BENCH = """
[[agents]]
harness = "h1"
model = "m1"
tested = true
routable = true
"""

BENCH_LINE = (
    'bench = { benchmark = "code-review", suite = "0123abc", '
    'rank_score = 0.9, date = "2026-07-03", run_ids = ["r1", "r2"] }'
)

AGENT_ROUTABLE_WITH_BENCH = AGENT_ROUTABLE_NO_BENCH + BENCH_LINE + "\n"


def write_pair(tmp_path: Path, base_agents: str, head_agents: str) -> tuple[Path, Path]:
    base = tmp_path / "base.toml"
    head = tmp_path / "head.toml"
    base.write_text(CATALOG_HEADER + base_agents)
    head.write_text(CATALOG_HEADER + head_agents)
    return base, head


def gate(tmp_path: Path, base_agents: str, head_agents: str) -> int:
    base, head = write_pair(tmp_path, base_agents, head_agents)
    return main(["gate", "--base-file", str(base), "--head-file", str(head)])


class TestGateRuleA:
    def test_flip_with_valid_bench_passes(self, tmp_path: Path, capsys: Any) -> None:
        assert gate(tmp_path, AGENT_NOT_ROUTABLE, AGENT_ROUTABLE_WITH_BENCH) == 0
        assert "GATE OK: 1 gated change(s)" in capsys.readouterr().out

    def test_flip_without_bench_fails(self, tmp_path: Path, capsys: Any) -> None:
        assert gate(tmp_path, AGENT_NOT_ROUTABLE, AGENT_ROUTABLE_NO_BENCH) == 1
        assert "GATE FAIL h1@m1" in capsys.readouterr().out

    def test_new_routable_entry_without_bench_fails(self, tmp_path: Path) -> None:
        assert gate(tmp_path, "", AGENT_ROUTABLE_NO_BENCH) == 1

    def test_new_routable_entry_with_bench_passes(self, tmp_path: Path) -> None:
        assert gate(tmp_path, "", AGENT_ROUTABLE_WITH_BENCH) == 0

    def test_flip_with_tested_false_fails(self, tmp_path: Path, capsys: Any) -> None:
        head = AGENT_ROUTABLE_WITH_BENCH.replace("tested = true", "tested = false")
        assert gate(tmp_path, AGENT_NOT_ROUTABLE, head) == 1
        assert "tested" in capsys.readouterr().out

    def test_missing_routable_in_base_is_false(self, tmp_path: Path) -> None:
        base = AGENT_NOT_ROUTABLE.replace("routable = false\n", "")
        assert gate(tmp_path, base, AGENT_ROUTABLE_NO_BENCH) == 1

    def test_no_flip_is_ok(self, tmp_path: Path, capsys: Any) -> None:
        assert gate(tmp_path, AGENT_NOT_ROUTABLE, AGENT_NOT_ROUTABLE) == 0
        assert "no gated changes" in capsys.readouterr().out

    def test_non_routable_addition_is_ok(self, tmp_path: Path) -> None:
        assert gate(tmp_path, "", AGENT_NOT_ROUTABLE) == 0

    def test_demote_and_delete_are_ok(self, tmp_path: Path) -> None:
        assert gate(tmp_path, AGENT_ROUTABLE_WITH_BENCH, AGENT_NOT_ROUTABLE) == 0
        assert gate(tmp_path, AGENT_ROUTABLE_WITH_BENCH, "") == 0

    def test_grandfathered_untouched_routable_is_ok(self, tmp_path: Path) -> None:
        # routable=true в base и head, bench нет нигде — молча проходит.
        assert gate(tmp_path, AGENT_ROUTABLE_NO_BENCH, AGENT_ROUTABLE_NO_BENCH) == 0


class TestGateRuleB:
    def test_removing_bench_from_routable_fails(
        self, tmp_path: Path, capsys: Any
    ) -> None:
        assert gate(tmp_path, AGENT_ROUTABLE_WITH_BENCH, AGENT_ROUTABLE_NO_BENCH) == 1
        assert "evidence removed" in capsys.readouterr().out

    def test_valid_bench_change_passes(self, tmp_path: Path) -> None:
        head = AGENT_ROUTABLE_WITH_BENCH.replace(
            'run_ids = ["r1", "r2"]', 'run_ids = ["r3", "r4"]'
        )
        assert gate(tmp_path, AGENT_ROUTABLE_WITH_BENCH, head) == 0

    def test_invalid_bench_change_fails(self, tmp_path: Path) -> None:
        head = AGENT_ROUTABLE_WITH_BENCH.replace('suite = "0123abc"', 'suite = "foo"')
        assert gate(tmp_path, AGENT_ROUTABLE_WITH_BENCH, head) == 1

    def test_backfill_valid_bench_on_routable_passes(self, tmp_path: Path) -> None:
        assert gate(tmp_path, AGENT_ROUTABLE_NO_BENCH, AGENT_ROUTABLE_WITH_BENCH) == 0

    def test_backfill_invalid_bench_fails(self, tmp_path: Path) -> None:
        head = AGENT_ROUTABLE_WITH_BENCH.replace("rank_score = 0.9", "rank_score = 1.5")
        assert gate(tmp_path, AGENT_ROUTABLE_NO_BENCH, head) == 1

    def test_unchanged_bench_not_revalidated(self, tmp_path: Path) -> None:
        # bench одинаков в base и head — правило B не трогает запись, даже
        # если бы валидатор к ней придрался (например tested=false исторически).
        stale = AGENT_ROUTABLE_WITH_BENCH.replace("tested = true", "tested = false")
        assert gate(tmp_path, stale, stale) == 0


class TestGateExitCodes:
    def test_duplicate_agent_id_in_head_is_exit_2(
        self, tmp_path: Path, capsys: Any
    ) -> None:
        head = AGENT_ROUTABLE_NO_BENCH + AGENT_ROUTABLE_NO_BENCH
        assert gate(tmp_path, "", head) == 2
        assert "duplicate" in capsys.readouterr().err

    def test_duplicate_agent_id_in_base_is_exit_2(self, tmp_path: Path) -> None:
        base = AGENT_NOT_ROUTABLE + AGENT_NOT_ROUTABLE
        assert gate(tmp_path, base, "") == 2

    def test_broken_toml_is_exit_2(self, tmp_path: Path) -> None:
        base = tmp_path / "base.toml"
        head = tmp_path / "head.toml"
        base.write_text("[agents\nboom")
        head.write_text(CATALOG_HEADER)
        assert main(["gate", "--base-file", str(base), "--head-file", str(head)]) == 2

    def test_missing_file_is_exit_2(self, tmp_path: Path) -> None:
        head = tmp_path / "head.toml"
        head.write_text(CATALOG_HEADER)
        missing = tmp_path / "nope.toml"
        assert (
            main(["gate", "--base-file", str(missing), "--head-file", str(head)]) == 2
        )
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `uv run python -m pytest tests/test_routable_gate.py -v`
Expected: FAIL — `ImportError: cannot import name 'main'`.

- [ ] **Step 3: Реализовать `run_gate` и `main`**

Добавить в `scripts/check_routable_gate.py` (в шапку импортов: `import argparse`, `import sys`):

```python
def run_gate(base_path: Path, head_path: Path) -> int:
    """Diff-based declaration gate (design §2, rules A and B). Returns 0/1."""
    base = agents_map(load_catalog(base_path), "base")
    head = agents_map(load_catalog(head_path), "head")

    failures: list[tuple[str, str]] = []
    gated = 0
    for aid, entry in head.items():
        if entry.get("routable") is not True:
            continue
        base_entry = base.get(aid)
        promoted = base_entry is None or base_entry.get("routable") is not True

        if promoted:
            # Rule A: flip or new routable pair requires full valid evidence.
            gated += 1
            failures.extend((aid, p) for p in validate_bench(entry))
            continue

        # Rule B: bench on a routable entry is an audit record.
        base_bench = base_entry.get("bench")
        head_bench = entry.get("bench")
        if base_bench is not None and head_bench is None:
            gated += 1
            failures.append(
                (aid, "evidence removed: bench deleted from a routable entry")
            )
        elif head_bench is not None and head_bench != base_bench:
            # Changed or backfilled bench revalidates in full.
            gated += 1
            failures.extend((aid, p) for p in validate_bench(entry))

    for aid, problem in failures:
        print(f"GATE FAIL {aid}: {problem}")
    if failures:
        return 1
    if gated:
        print(f"GATE OK: {gated} gated change(s) with valid evidence")
    else:
        print("GATE OK: no gated changes")
    return 0


def main(argv: list[str] | None = None) -> int:
    """CLI entry point; returns the process exit code (0/1/2)."""
    parser = argparse.ArgumentParser(
        prog="check_routable_gate",
        description="Routable-flip benchmark-evidence gate (ADR-ECO-003a D4)",
    )
    sub = parser.add_subparsers(dest="mode", required=True)

    gate_parser = sub.add_parser("gate", help="diff-based declaration gate (CI)")
    gate_parser.add_argument("--base-file", required=True, type=Path)
    gate_parser.add_argument("--head-file", required=True, type=Path)

    args = parser.parse_args(argv)
    try:
        return run_gate(args.base_file, args.head_file)
    except GateInputError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
```

- [ ] **Step 4: Прогнать тесты**

Run: `uv run python -m pytest tests/test_routable_gate.py -v`
Expected: PASS.

- [ ] **Step 5: Линт/типы + коммит**

```bash
uv run ruff format . && uv run ruff check . && uv run pyrefly check
git add scripts/check_routable_gate.py tests/test_routable_gate.py
git commit -m "feat(gate): diff gate mode — promotion rule A + bench-tamper rule B

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: Режим `verify` (data-gate против arbiter.db)

**Files:**
- Modify: `scripts/check_routable_gate.py` (добавить `run_verify`, ветку CLI)
- Modify: `tests/test_routable_gate.py` (добавить verify-тесты)

**Interfaces:**
- Consumes: всё из Task 1–2; `main` расширяется веткой `verify`.
- Produces: `run_verify(db_path: Path, catalog_path: Path, eps: float) -> int` — 0/1 (2 через `GateInputError`).

- [ ] **Step 1: Написать падающие verify-тесты**

Добавить в `tests/test_routable_gate.py` (в шапку: `import sqlite3`):

```python
# Реальная схема benchmark_runs (arbiter-mcp/src/db.rs:936-951).
# score_components — TEXT NOT NULL: NULL-кейса на реальных данных не существует.
BENCHMARK_RUNS_SCHEMA = """
CREATE TABLE benchmark_runs (
    run_id                TEXT PRIMARY KEY,
    payload_version       TEXT NOT NULL,
    benchmark_id          TEXT NOT NULL,
    agent_id              TEXT NOT NULL,
    ts                    TEXT NOT NULL,
    score                 REAL NOT NULL,
    score_components      TEXT NOT NULL,
    total_tokens          INTEGER,
    total_cost_usd        REAL,
    duration_seconds      REAL NOT NULL,
    per_task              TEXT NOT NULL,
    per_task_total_count  INTEGER NOT NULL,
    per_task_truncated    INTEGER NOT NULL,
    inserted_at           TEXT NOT NULL DEFAULT (datetime('now'))
)
"""


def make_db(tmp_path: Path, rows: list[dict[str, Any]]) -> Path:
    db_path = tmp_path / "arbiter.db"
    conn = sqlite3.connect(db_path)
    conn.execute(BENCHMARK_RUNS_SCHEMA)
    for row in rows:
        conn.execute(
            "INSERT INTO benchmark_runs (run_id, payload_version, benchmark_id,"
            " agent_id, ts, score, score_components, duration_seconds, per_task,"
            " per_task_total_count, per_task_truncated)"
            " VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                row["run_id"],
                "1.0",
                row.get("benchmark_id", "code-review"),
                row.get("agent_id", "h1@m1"),
                row.get("ts", "2026-07-03T10:00:00Z"),
                row.get("score", 0.9),
                row.get("score_components", '{"rank_score": 0.9}'),
                60.0,
                "[]",
                0,
                0,
            ),
        )
    conn.commit()
    conn.close()
    return db_path


def write_catalog(tmp_path: Path, agents: str) -> Path:
    path = tmp_path / "catalog.toml"
    path.write_text(CATALOG_HEADER + agents)
    return path


def verify(
    tmp_path: Path,
    agents: str,
    rows: list[dict[str, Any]],
    *extra: str,
) -> int:
    db = make_db(tmp_path, rows)
    catalog = write_catalog(tmp_path, agents)
    argv = ["verify", "--db", str(db), "--catalog", str(catalog), *extra]
    return main(argv)


R1_R2 = [{"run_id": "r1"}, {"run_id": "r2"}]


class TestVerify:
    def test_valid_evidence_passes(self, tmp_path: Path, capsys: Any) -> None:
        assert verify(tmp_path, AGENT_ROUTABLE_WITH_BENCH, R1_R2) == 0
        assert "VERIFY OK h1@m1" in capsys.readouterr().out

    def test_fallback_to_score_when_no_rank_score_key(self, tmp_path: Path) -> None:
        rows = [
            {"run_id": "r1", "score": 0.9, "score_components": "{}"},
            {"run_id": "r2", "score": 0.9, "score_components": "not json"},
        ]
        assert verify(tmp_path, AGENT_ROUTABLE_WITH_BENCH, rows) == 0

    def test_fallback_when_rank_score_not_numeric(self, tmp_path: Path) -> None:
        # Строка и JSON-boolean — не числа (зеркало serde_json as_f64 -> None).
        rows = [
            {"run_id": "r1", "score": 0.9, "score_components": '{"rank_score": "x"}'},
            {"run_id": "r2", "score": 0.9, "score_components": '{"rank_score": true}'},
        ]
        assert verify(tmp_path, AGENT_ROUTABLE_WITH_BENCH, rows) == 0

    def test_missing_run_id_fails(self, tmp_path: Path, capsys: Any) -> None:
        assert verify(tmp_path, AGENT_ROUTABLE_WITH_BENCH, [{"run_id": "r1"}]) == 1
        assert "r2" in capsys.readouterr().out

    def test_run_id_with_wrong_agent_fails(self, tmp_path: Path) -> None:
        rows = [{"run_id": "r1"}, {"run_id": "r2", "agent_id": "other@m"}]
        assert verify(tmp_path, AGENT_ROUTABLE_WITH_BENCH, rows) == 1

    def test_run_id_with_wrong_benchmark_fails(self, tmp_path: Path) -> None:
        rows = [{"run_id": "r1"}, {"run_id": "r2", "benchmark_id": "other-bench"}]
        assert verify(tmp_path, AGENT_ROUTABLE_WITH_BENCH, rows) == 1

    def test_mean_outside_eps_fails(self, tmp_path: Path, capsys: Any) -> None:
        rows = [
            {"run_id": "r1", "score_components": '{"rank_score": 0.5}'},
            {"run_id": "r2", "score_components": '{"rank_score": 0.5}'},
        ]
        assert verify(tmp_path, AGENT_ROUTABLE_WITH_BENCH, rows) == 1
        assert "0.500" in capsys.readouterr().out  # фактическое среднее в выводе

    def test_custom_eps_allows_wider_gap(self, tmp_path: Path) -> None:
        rows = [
            {"run_id": "r1", "score_components": '{"rank_score": 0.8}'},
            {"run_id": "r2", "score_components": '{"rank_score": 0.8}'},
        ]
        assert verify(tmp_path, AGENT_ROUTABLE_WITH_BENCH, rows, "--eps", "0.2") == 0

    def test_stale_date_fails(self, tmp_path: Path, capsys: Any) -> None:
        rows = [
            {"run_id": "r1", "ts": "2026-05-01T10:00:00Z"},
            {"run_id": "r2", "ts": "2026-05-01T11:00:00Z"},
        ]
        assert verify(tmp_path, AGENT_ROUTABLE_WITH_BENCH, rows) == 1
        assert "date" in capsys.readouterr().out.lower()

    def test_runtime_effective_score_reported(
        self, tmp_path: Path, capsys: Any
    ) -> None:
        # Равные ts: детерминизм через второй ключ run_id DESC -> строка r2.
        rows = [
            {"run_id": "r1", "score_components": '{"rank_score": 0.9}'},
            {"run_id": "r2", "score_components": '{"rank_score": 0.9}'},
        ]
        assert verify(tmp_path, AGENT_ROUTABLE_WITH_BENCH, rows) == 0
        assert "runtime-effective" in capsys.readouterr().out

    def test_grandfathered_pair_warns_but_passes(
        self, tmp_path: Path, capsys: Any
    ) -> None:
        assert verify(tmp_path, AGENT_ROUTABLE_NO_BENCH, []) == 0
        assert "WARN h1@m1" in capsys.readouterr().out

    def test_invalid_declaration_fails(self, tmp_path: Path) -> None:
        agents = AGENT_ROUTABLE_WITH_BENCH.replace('suite = "0123abc"', 'suite = "zz"')
        assert verify(tmp_path, agents, R1_R2) == 1


class TestVerifyExitCodes:
    def test_missing_db_is_exit_2(self, tmp_path: Path) -> None:
        catalog = write_catalog(tmp_path, AGENT_ROUTABLE_WITH_BENCH)
        argv = ["verify", "--db", str(tmp_path / "no.db"), "--catalog", str(catalog)]
        assert main(argv) == 2

    def test_db_without_table_is_exit_2(self, tmp_path: Path) -> None:
        db = tmp_path / "empty.db"
        sqlite3.connect(db).close()
        catalog = write_catalog(tmp_path, AGENT_ROUTABLE_WITH_BENCH)
        assert main(["verify", "--db", str(db), "--catalog", str(catalog)]) == 2

    def test_corrupted_ts_is_exit_2(self, tmp_path: Path) -> None:
        rows = [{"run_id": "r1", "ts": "not-a-timestamp"}, {"run_id": "r2"}]
        assert verify(tmp_path, AGENT_ROUTABLE_WITH_BENCH, rows) == 2

    @pytest.mark.parametrize("eps", ["-1", "nan", "inf"])
    def test_invalid_eps_is_exit_2(self, tmp_path: Path, eps: str) -> None:
        assert verify(tmp_path, AGENT_ROUTABLE_WITH_BENCH, R1_R2, "--eps", eps) == 2
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `uv run python -m pytest tests/test_routable_gate.py -v`
Expected: FAIL — argparse не знает сабкоманду `verify` (exit 2 от SystemExit argparse → тесты падают на assert).

Примечание: argparse на неизвестной сабкоманде вызывает `SystemExit`; тесты
Task 3 упадут с ошибкой, это и есть «красный».

- [ ] **Step 3: Реализовать `run_verify` и ветку CLI**

Добавить в `scripts/check_routable_gate.py` (в шапку: `import json`, `import sqlite3`):

```python
def _clamp01(value: float) -> float:
    return min(1.0, max(0.0, value))


def _effective_score(score: float, components: str) -> float:
    """Mirror of get_benchmark_score (arbiter-mcp/src/db.rs:817-841): prefer
    score_components.rank_score when it is a JSON number (bool is not — same
    as serde_json Value::as_f64), else fall back to the scalar score."""
    try:
        parsed = json.loads(components)
    except (json.JSONDecodeError, TypeError):
        return _clamp01(score)
    if isinstance(parsed, dict):
        rank = parsed.get("rank_score")
        if _is_number(rank):
            return _clamp01(float(rank))
    return _clamp01(score)


def _parse_rfc3339(value: str) -> datetime | None:
    try:
        parsed = datetime.fromisoformat(value)
    except ValueError:
        return None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return parsed


def run_verify(db_path: Path, catalog_path: Path, eps: float) -> int:
    """Local data-gate (design §4): check declared evidence against
    benchmark_runs rows. Returns 0/1; input problems raise GateInputError."""
    if not (math.isfinite(eps) and eps >= 0):
        raise GateInputError(f"--eps must be a finite number >= 0, got {eps}")
    catalog = agents_map(load_catalog(catalog_path), "catalog")
    if not db_path.exists():
        raise GateInputError(f"db not found: {db_path}")
    conn = sqlite3.connect(db_path)
    try:
        has_table = conn.execute(
            "SELECT name FROM sqlite_master WHERE type='table'"
            " AND name='benchmark_runs'"
        ).fetchone()
        if has_table is None:
            raise GateInputError(f"db {db_path} has no benchmark_runs table")
        return _verify_agents(conn, catalog, eps)
    finally:
        conn.close()


def _verify_agents(
    conn: sqlite3.Connection, catalog: dict[str, dict[str, Any]], eps: float
) -> int:
    failures = 0
    for aid, entry in catalog.items():
        if entry.get("routable") is not True:
            continue
        if entry.get("bench") is None:
            print(f"WARN {aid}: routable pair without bench evidence (grandfathered)")
            continue
        problems = validate_bench(entry)
        if problems:
            for p in problems:
                print(f"VERIFY FAIL {aid}: invalid declaration: {p}")
            failures += 1
            continue
        if not _verify_one(conn, aid, entry["bench"], eps):
            failures += 1
    return 1 if failures else 0


def _verify_one(
    conn: sqlite3.Connection, aid: str, bench: dict[str, Any], eps: float
) -> bool:
    benchmark = bench["benchmark"]
    scores: list[float] = []
    timestamps: list[datetime] = []
    ok = True
    for rid in bench["run_ids"]:
        row = conn.execute(
            "SELECT agent_id, benchmark_id, ts, score, score_components"
            " FROM benchmark_runs WHERE run_id = ?",
            (rid,),
        ).fetchone()
        if row is None:
            print(f"VERIFY FAIL {aid}: run_id {rid!r} not found in benchmark_runs")
            ok = False
            continue
        row_aid, row_bench, ts, score, components = row
        if row_aid != aid or row_bench != benchmark:
            print(
                f"VERIFY FAIL {aid}: run_id {rid!r} belongs to"
                f" ({row_aid}, {row_bench}), expected ({aid}, {benchmark})"
            )
            ok = False
            continue
        parsed_ts = _parse_rfc3339(ts)
        if parsed_ts is None:
            # Ingest guarantees RFC3339 — a bad ts is corrupted data, not a
            # policy mismatch (design §4).
            raise GateInputError(f"corrupted data: run {rid!r} has invalid ts {ts!r}")
        timestamps.append(parsed_ts)
        scores.append(_effective_score(score, components))
    if not ok:
        return False

    claimed = bench["rank_score"]
    mean = sum(scores) / len(scores)
    if abs(mean - claimed) > eps:
        per_run = ", ".join(f"{s:.3f}" for s in scores)
        print(
            f"VERIFY FAIL {aid}: claimed rank_score {claimed} vs actual mean"
            f" {mean:.3f} (eps {eps}; per-run: {per_run})"
        )
        return False

    declared_date = _parse_iso_date(bench["date"])
    assert declared_date is not None  # validate_bench уже проверил
    latest_ts = max(timestamps)
    if abs((declared_date - latest_ts.date()).days) > DATE_FRESHNESS_DAYS:
        print(
            f"VERIFY FAIL {aid}: bench.date {bench['date']} is more than"
            f" {DATE_FRESHNESS_DAYS} days from latest run ts {latest_ts.date()}"
        )
        return False

    runtime_row = conn.execute(
        "SELECT score, score_components FROM benchmark_runs"
        " WHERE agent_id = ? AND benchmark_id = ?"
        " ORDER BY ts DESC, run_id DESC LIMIT 1",
        (aid, benchmark),
    ).fetchone()
    runtime_score = _effective_score(runtime_row[0], runtime_row[1])
    print(
        f"VERIFY OK {aid}: mean {mean:.3f} matches claimed {claimed} (eps {eps});"
        f" runtime-effective (latest row): {runtime_score:.3f}"
    )
    return True
```

В `main` добавить парсер и ветку (после `gate_parser`):

```python
    verify_parser = sub.add_parser("verify", help="local data-gate against arbiter.db")
    verify_parser.add_argument("--db", required=True, type=Path)
    verify_parser.add_argument("--eps", type=float, default=DEFAULT_EPS)
    verify_parser.add_argument(
        "--catalog", type=Path, default=Path("config/agents-catalog.toml")
    )
```

и в `try`-блоке:

```python
        if args.mode == "gate":
            return run_gate(args.base_file, args.head_file)
        return run_verify(args.db, args.catalog, args.eps)
```

- [ ] **Step 4: Прогнать тесты**

Run: `uv run python -m pytest tests/test_routable_gate.py -v`
Expected: PASS (все тесты Task 1–3).

- [ ] **Step 5: Смоук на живом вендорном каталоге + линт + коммит**

```bash
# Смоук: no gated changes на identical base/head
uv run python scripts/check_routable_gate.py gate \
    --base-file config/agents-catalog.toml --head-file config/agents-catalog.toml
# Expected: "GATE OK: no gated changes", exit 0

uv run ruff format . && uv run ruff check . && uv run pyrefly check
git add scripts/check_routable_gate.py tests/test_routable_gate.py
git commit -m "feat(gate): verify mode — data-gate against benchmark_runs (runtime score semantics)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: CI + Makefile + документация + полный прогон

**Files:**
- Modify: `.github/workflows/ci.yml` (job `routable-gate` + шаг `pytest (workspace)`)
- Modify: `Makefile` (цель `test-python`)
- Modify: `CLAUDE.md` (скрипт в Project Structure + описание в Python Components)
- Modify: `TODO.md` (запись о задаче)

**Interfaces:**
- Consumes: `scripts/check_routable_gate.py` из Task 1–3.
- Produces: зелёный полный прогон CI-эквивалента локально.

- [ ] **Step 1: CI — новый job и workspace-pytest**

В `.github/workflows/ci.yml`:

1. После шага `pytest (orchestrator)` (строка ~72) добавить:

```yaml
      - name: pytest (workspace)
        run: uv run python -m pytest tests/ -v
```

2. Новый job после job'а `python` (той же вложенности, что `rust`/`python`):

```yaml
  routable-gate:
    name: Routable-flip gate (ADR-003a D4)
    runs-on: ubuntu-latest
    if: github.event_name == 'pull_request'
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - name: Set up Python 3.12
        uses: actions/setup-python@v5
        with:
          python-version: "3.12"

      - name: Extract base catalog
        run: git show "${{ github.event.pull_request.base.sha }}:config/agents-catalog.toml" > /tmp/base-catalog.toml

      - name: Run gate
        run: python scripts/check_routable_gate.py gate --base-file /tmp/base-catalog.toml --head-file config/agents-catalog.toml
```

- [ ] **Step 2: Makefile**

В цели `test-python` (строка ~49) после существующей строки добавить:

```make
	uv run pytest tests/ -v
```

- [ ] **Step 3: Локальная проверка CI-эквивалента**

```bash
uv run ruff format --check . && uv run ruff check . && uv run pyrefly check
uv run python -m pytest orchestrator/tests/ tests/ -v
# gate-смоук как в CI:
git show origin/master:config/agents-catalog.toml > /tmp/base-catalog.toml
python3 scripts/check_routable_gate.py gate --base-file /tmp/base-catalog.toml --head-file config/agents-catalog.toml
```

Expected: всё зелёное; gate печатает `GATE OK: no gated changes`.

- [ ] **Step 4: CLAUDE.md**

1. В Project Structure под `scripts/` после строки `ab_bench_rerank.py` добавить:

```
│   ├── check_routable_gate.py    # ADR-003a D4: routable-flip evidence gate (CI) + verify vs benchmark_runs (local)
```

2. В разделе «Python Components» добавить пункт (после `ab_bench_rerank.py`/`ingest_benchmark_payloads.py`):

```markdown
5. **`scripts/check_routable_gate.py`** — гейт routable-промоушна (ADR-ECO-003a D4)
   - `gate --base-file A --head-file B` — диффовый evidence-declaration гейт
     (CI-job `routable-gate`, только PR): флип `routable=true` обязан нести
     валидный `bench`-блок; правило B защищает bench от удаления/порчи
   - `verify --db arbiter.db [--eps 0.05]` — локальный data-gate: сверка
     заявленного `rank_score` со средним эффективных скоров по `run_ids`
     (семантика `get_benchmark_score`)
   - Дизайн: `docs/2026-07-05-routable-gate-design.md`
```

(нумерацию последующих пунктов сдвинуть).

- [ ] **Step 5: TODO.md**

В секцию «Активные задачи» добавить:

```markdown
### ADR-ECO-003a: гейт routable-PR на benchmark-эвиденс — ✅ закрыт

- [x] `scripts/check_routable_gate.py`: `gate` (диффовые правила A/B,
  evidence-declaration) + `verify` (data-gate против `benchmark_runs`,
  runtime-семантика скора, mean по `run_ids`, eps 0.05)
- [x] CI-job `routable-gate` (pull_request, base.sha) + `pytest tests/`
  добавлен в CI python-job (закрыта pre-existing дыра: workspace-тесты не гонялись)
- [x] Дизайн: `docs/2026-07-05-routable-gate-design.md` (v3, два раунда ревью)
- Follow-ups (вне репы): конвенция `bench`-полей в SSOT-канон atp-platform;
  бэкфил трёх grandfathered-пар
- Коммит: `<hash последнего коммита Task 4>`
```

- [ ] **Step 6: Финальный прогон + коммит**

```bash
uv run ruff format --check . && uv run ruff check . && uv run pyrefly check && uv run python -m pytest orchestrator/tests/ tests/ -v && cargo test --workspace
git add .github/workflows/ci.yml Makefile CLAUDE.md TODO.md
git commit -m "ci: routable-gate job (ADR-003a D4) + workspace pytest in CI; docs sync

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

(`cargo test --workspace` — убедиться, что Rust не задет; изменений в Rust нет.)

---

## Out of scope (из спеки §1/§7, не делать)

- Бэкфил `bench`-полей и зеркалирование конвенции в SSOT-канон atp-platform.
- A/B-вью над `benchmark_runs` (третий пункт 003a).
- Внешний immutable evidence store.
- Изменения Rust-кода.
