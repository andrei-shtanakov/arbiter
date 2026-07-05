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
