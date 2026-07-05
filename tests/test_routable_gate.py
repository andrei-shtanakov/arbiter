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
    main,
    run_gate,  # noqa: F401
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

    @pytest.mark.parametrize(
        "d",
        ["03.07.2026", "2026-13-01", "2099-01-01", 20260703, "20260703", "2026-W27-5"],
    )
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
