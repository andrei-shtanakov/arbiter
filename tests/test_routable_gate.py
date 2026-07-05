"""Tests for scripts/check_routable_gate.py (ADR-ECO-003a D4).

The gate has two layers: `gate` validates evidence *declarations* on
routable flips (CI, no DB), `verify` checks declarations against real
`benchmark_runs` rows (local). Design:
docs/2026-07-05-routable-gate-design.md.
"""

from __future__ import annotations

import copy
import sqlite3
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

    def test_agents_wrong_shape_raises(self) -> None:
        # `agents = {}` instead of [[agents]] — malformed shape is exit-2 class.
        with pytest.raises(GateInputError, match="array of tables"):
            agents_map({"agents": {"harness": "h1"}}, "head")

    def test_agents_entry_not_a_table_raises(self) -> None:
        with pytest.raises(GateInputError, match="not a table"):
            agents_map({"agents": ["h1@m1"]}, "base")


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


# ============================================================================
# Verify tests (Task 3)
# ============================================================================

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

    def test_naive_ts_without_offset_is_exit_2(self, tmp_path: Path) -> None:
        # Ingest (require_rfc3339, chrono) rejects naive timestamps, so a
        # tz-less ts in the db is corrupted data — no silent assume-UTC.
        rows = [{"run_id": "r1", "ts": "2026-07-03T10:00:00"}, {"run_id": "r2"}]
        assert verify(tmp_path, AGENT_ROUTABLE_WITH_BENCH, rows) == 2

    @pytest.mark.parametrize("eps", ["-1", "nan", "inf"])
    def test_invalid_eps_is_exit_2(self, tmp_path: Path, eps: str) -> None:
        assert verify(tmp_path, AGENT_ROUTABLE_WITH_BENCH, R1_R2, "--eps", eps) == 2

    def test_garbage_db_file_is_exit_2(self, tmp_path: Path) -> None:
        db = tmp_path / "garbage.db"
        db.write_text("this is not a sqlite database at all")
        catalog = write_catalog(tmp_path, AGENT_ROUTABLE_WITH_BENCH)
        assert main(["verify", "--db", str(db), "--catalog", str(catalog)]) == 2

    def test_incompatible_schema_is_exit_2(self, tmp_path: Path) -> None:
        db = tmp_path / "old.db"
        conn = sqlite3.connect(db)
        conn.execute("CREATE TABLE benchmark_runs (run_id TEXT PRIMARY KEY)")
        conn.commit()
        conn.close()
        catalog = write_catalog(tmp_path, AGENT_ROUTABLE_WITH_BENCH)
        assert main(["verify", "--db", str(db), "--catalog", str(catalog)]) == 2
