"""Tests for the `ab` mode of scripts/check_routable_gate.py.

A/B view over `benchmark_runs`: "model A vs B on benchmark T" as input for
the human routable-flip gate (ADR-ECO-003a, TODO @id:benchmark-ab-view).
The view is NOT a gate: success is always exit 0; input problems exit 2.
"""

from __future__ import annotations

import json
import sqlite3
from pathlib import Path
from typing import Any

from scripts.check_routable_gate import main
from tests.test_routable_gate import BENCHMARK_RUNS_SCHEMA


def task(index: int, passed: int, graded: int) -> dict[str, Any]:
    """per_task entry: contract-v1 required fields (task_index,
    duration_seconds) + the pass-count extension newer payloads carry."""
    return {
        "task_index": index,
        "run_pass_count": passed,
        "runs_graded": graded,
        "score": 0.0,
        "duration_seconds": 1.0,
    }


def make_db(tmp_path: Path, rows: list[dict[str, Any]]) -> Path:
    db_path = tmp_path / "arbiter.db"
    conn = sqlite3.connect(db_path)
    conn.execute(BENCHMARK_RUNS_SCHEMA)
    for row in rows:
        per_task = row.get("per_task", [])
        per_task_json = per_task if isinstance(per_task, str) else json.dumps(per_task)
        conn.execute(
            "INSERT INTO benchmark_runs (run_id, payload_version, benchmark_id,"
            " agent_id, ts, score, score_components, duration_seconds, per_task,"
            " per_task_total_count, per_task_truncated, score_semantics)"
            " VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                row["run_id"],
                "1.0",
                row.get("benchmark_id", "code-review"),
                row.get("agent_id", "a@m"),
                row.get("ts", "2026-07-03T10:00:00Z"),
                row.get("score", 0.5),
                row.get("score_components", "{}"),
                60.0,
                per_task_json,
                row.get("per_task_total_count", len(per_task)),
                row.get("per_task_truncated", 0),
                row.get("score_semantics"),
            ),
        )
    conn.commit()
    conn.close()
    return db_path


def ab(
    tmp_path: Path,
    rows: list[dict[str, Any]],
    agent_a: str = "a@m",
    agent_b: str = "b@m",
    benchmark: str = "code-review",
) -> int:
    db = make_db(tmp_path, rows)
    return main(["ab", "--db", str(db), "--benchmark", benchmark, agent_a, agent_b])


def two_agents(
    a_components: str = '{"rank_score": 0.9}',
    b_components: str = '{"rank_score": 0.7}',
    a_tasks: list[dict[str, Any]] | None = None,
    b_tasks: list[dict[str, Any]] | None = None,
) -> list[dict[str, Any]]:
    return [
        {
            "run_id": "ra",
            "agent_id": "a@m",
            "score_components": a_components,
            "per_task": a_tasks or [],
        },
        {
            "run_id": "rb",
            "agent_id": "b@m",
            "score_components": b_components,
            "per_task": b_tasks or [],
        },
    ]


class TestAbRunLevel:
    def test_effective_scores_and_delta(self, tmp_path: Path, capsys: Any) -> None:
        assert ab(tmp_path, two_agents()) == 0
        out = capsys.readouterr().out
        assert "a@m effective (latest usable): 0.900" in out
        assert "b@m effective (latest usable): 0.700" in out
        assert "delta (A - B): +0.200" in out

    def test_latest_run_tie_broken_by_run_id_desc(
        self, tmp_path: Path, capsys: Any
    ) -> None:
        # Равные ts у r1/r2 -> детерминизм через второй ключ run_id DESC (r2).
        rows = [
            {
                "run_id": "r1",
                "agent_id": "a@m",
                "score_components": '{"rank_score": 0.4}',
            },
            {
                "run_id": "r2",
                "agent_id": "a@m",
                "score_components": '{"rank_score": 0.8}',
            },
            {
                "run_id": "rb",
                "agent_id": "b@m",
                "score_components": '{"rank_score": 0.7}',
            },
        ]
        assert ab(tmp_path, rows) == 0
        assert "a@m effective (latest usable): 0.800" in capsys.readouterr().out

    def test_latest_run_by_ts_not_insertion_order(
        self, tmp_path: Path, capsys: Any
    ) -> None:
        rows = [
            {
                "run_id": "r-new",
                "agent_id": "a@m",
                "ts": "2026-07-04T10:00:00Z",
                "score_components": '{"rank_score": 0.9}',
            },
            {
                "run_id": "r-old",
                "agent_id": "a@m",
                "ts": "2026-07-01T10:00:00Z",
                "score_components": '{"rank_score": 0.2}',
            },
            {
                "run_id": "rb",
                "agent_id": "b@m",
                "score_components": '{"rank_score": 0.7}',
            },
        ]
        assert ab(tmp_path, rows) == 0
        assert "a@m effective (latest usable): 0.900" in capsys.readouterr().out

    def test_rank_score_preferred_over_scalar(
        self, tmp_path: Path, capsys: Any
    ) -> None:
        rows = two_agents(a_components='{"rank_score": 0.9}')
        rows[0]["score"] = 0.1  # scalar must NOT win over rank_score
        assert ab(tmp_path, rows) == 0
        assert "a@m effective (latest usable): 0.900" in capsys.readouterr().out

    def test_run_history_lists_run_ids(self, tmp_path: Path, capsys: Any) -> None:
        assert ab(tmp_path, two_agents()) == 0
        out = capsys.readouterr().out
        assert "ra" in out
        assert "rb" in out


class TestAbPerTask:
    def test_divergent_tasks_listed_and_summary(
        self, tmp_path: Path, capsys: Any
    ) -> None:
        rows = two_agents(
            a_tasks=[task(0, 3, 3), task(1, 1, 3)],
            b_tasks=[task(0, 3, 3), task(1, 3, 3)],
        )
        assert ab(tmp_path, rows) == 0
        out = capsys.readouterr().out
        assert "task 1:" in out
        assert "task 0:" not in out  # ничья не показывается
        assert "A better on 0 task(s), B better on 1, tie 1" in out

    def test_equal_ratio_with_different_denominators_is_tie(
        self, tmp_path: Path, capsys: Any
    ) -> None:
        # 1/2 == 2/4 точно (целочисленное кросс-умножение, без float-допуска).
        rows = two_agents(a_tasks=[task(0, 1, 2)], b_tasks=[task(0, 2, 4)])
        assert ab(tmp_path, rows) == 0
        out = capsys.readouterr().out
        assert "A better on 0 task(s), B better on 0, tie 1" in out
        assert "task 0:" not in out

    def test_missing_pass_count_keys_are_ungraded_not_corrupted(
        self, tmp_path: Path, capsys: Any
    ) -> None:
        # Ранние payload'ы (реальные строки 2026-06/07 в arbiter.db) не несут
        # run_pass_count/runs_graded вовсе — контракт v1 их и не требует;
        # это «не оценено», не порча.
        legacy = [
            {
                "task_index": 0,
                "score": 0.0,
                "task_type": "review",
                "duration_seconds": 1.0,
            }
        ]
        rows = two_agents(a_tasks=legacy, b_tasks=[task(0, 3, 3)])
        assert ab(tmp_path, rows) == 0
        out = capsys.readouterr().out
        assert "A better on 0 task(s), B better on 0, tie 0" in out
        assert "1 ungraded" in out

    def test_ungraded_tasks_excluded_from_counts(
        self, tmp_path: Path, capsys: Any
    ) -> None:
        # runs_graded == 0 хотя бы с одной стороны: pass rate не определён.
        rows = two_agents(
            a_tasks=[task(0, 0, 0), task(1, 3, 3)],
            b_tasks=[task(0, 2, 3), task(1, 1, 3)],
        )
        assert ab(tmp_path, rows) == 0
        out = capsys.readouterr().out
        assert "A better on 1 task(s), B better on 0, tie 0" in out
        assert "1 ungraded" in out


class TestAbIncomplete:
    def test_differing_task_sets_mark_incomplete(
        self, tmp_path: Path, capsys: Any
    ) -> None:
        rows = two_agents(
            a_tasks=[task(0, 3, 3), task(1, 3, 3)],
            b_tasks=[task(1, 1, 3), task(2, 3, 3)],
        )
        assert ab(tmp_path, rows) == 0
        out = capsys.readouterr().out
        assert "INCOMPLETE COMPARISON" in out
        assert "1 task(s) only in A" in out
        assert "1 task(s) only in B" in out
        # Счётчики только по пересечению (task 1).
        assert "A better on 1 task(s), B better on 0, tie 0" in out

    def test_truncated_per_task_marks_incomplete(
        self, tmp_path: Path, capsys: Any
    ) -> None:
        rows = two_agents(a_tasks=[task(0, 3, 3)], b_tasks=[task(0, 3, 3)])
        rows[0]["per_task_truncated"] = 1
        assert ab(tmp_path, rows) == 0
        assert "INCOMPLETE COMPARISON" in capsys.readouterr().out

    def test_complete_comparison_has_no_incomplete_marker(
        self, tmp_path: Path, capsys: Any
    ) -> None:
        rows = two_agents(a_tasks=[task(0, 3, 3)], b_tasks=[task(0, 3, 3)])
        assert ab(tmp_path, rows) == 0
        assert "INCOMPLETE COMPARISON" not in capsys.readouterr().out


class TestAbSuiteIdentityNote:
    def test_v1_limitation_note_always_printed(
        self, tmp_path: Path, capsys: Any
    ) -> None:
        # benchmark_runs не хранит suite identity — вью обязан говорить об этом.
        assert ab(tmp_path, two_agents()) == 0
        assert "no suite identity" in capsys.readouterr().out


class TestAbExitCodes:
    def test_missing_db_is_exit_2(self, tmp_path: Path) -> None:
        argv = [
            "ab",
            "--db",
            str(tmp_path / "no.db"),
            "--benchmark",
            "code-review",
            "a@m",
            "b@m",
        ]
        assert main(argv) == 2

    def test_db_without_table_is_exit_2(self, tmp_path: Path) -> None:
        db = tmp_path / "empty.db"
        sqlite3.connect(db).close()
        argv = ["ab", "--db", str(db), "--benchmark", "code-review", "a@m", "b@m"]
        assert main(argv) == 2

    def test_agent_without_rows_is_exit_2(self, tmp_path: Path, capsys: Any) -> None:
        rows = [{"run_id": "ra", "agent_id": "a@m"}]
        assert ab(tmp_path, rows) == 2
        assert "b@m" in capsys.readouterr().err

    def test_same_agent_twice_is_exit_2(self, tmp_path: Path) -> None:
        rows = [{"run_id": "ra", "agent_id": "a@m"}]
        assert ab(tmp_path, rows, agent_a="a@m", agent_b="a@m") == 2

    def test_corrupted_per_task_json_is_exit_2(self, tmp_path: Path) -> None:
        rows = two_agents()
        rows[0]["per_task"] = "not json"
        assert ab(tmp_path, rows) == 2


GRADED = (
    '{"schema_version": 1, "kind": "aggregated_evaluation", "quality_signal": true}'
)
UNGRADED = '{"schema_version": 1, "kind": "completion_rate", "quality_signal": false}'


class TestAbScoreUsability:
    """Вью обязано показывать то, что увидит маршрутизация (inbox #81, #82).

    Зеркало `Database::get_benchmark_score`: непригодный прогон помечается
    `withheld`, а сравнение берёт последний ПРИГОДНЫЙ прогон, а не последний.
    """

    def test_percent_score_is_withheld_not_shown_as_perfect(
        self, tmp_path: Path, capsys: Any
    ) -> None:
        rows = two_agents()
        rows[0]["score"] = 66.67
        rows[0]["score_components"] = "{}"
        assert ab(tmp_path, rows) == 0
        out = capsys.readouterr().out
        assert "withheld (not usable for routing)" in out
        assert "INCOMPLETE COMPARISON: no run usable for routing for a@m" in out

    def test_ungraded_run_is_withheld(self, tmp_path: Path, capsys: Any) -> None:
        rows = two_agents()
        rows[0]["score_semantics"] = UNGRADED
        assert ab(tmp_path, rows) == 0
        out = capsys.readouterr().out
        assert "withheld (not usable for routing)" in out
        assert "INCOMPLETE COMPARISON: no run usable for routing for a@m" in out

    def test_graded_run_is_used(self, tmp_path: Path, capsys: Any) -> None:
        rows = two_agents()
        rows[0]["score_semantics"] = GRADED
        rows[1]["score_semantics"] = GRADED
        assert ab(tmp_path, rows) == 0
        out = capsys.readouterr().out
        assert "a@m effective (latest usable): 0.900" in out
        assert "delta (A - B): +0.200" in out

    def test_ungraded_newest_run_does_not_mask_a_graded_older_one(
        self, tmp_path: Path, capsys: Any
    ) -> None:
        rows = two_agents()
        rows[0]["ts"] = "2026-07-01T10:00:00Z"
        rows[0]["score_semantics"] = GRADED
        rows.append(
            {
                "run_id": "ra-new",
                "agent_id": "a@m",
                "ts": "2026-07-05T10:00:00Z",
                "score_components": '{"rank_score": 0.2}',
                "score_semantics": UNGRADED,
            }
        )
        assert ab(tmp_path, rows) == 0
        out = capsys.readouterr().out
        assert "a@m effective (latest usable): 0.900" in out

    def test_absent_semantics_stays_usable_as_legacy(
        self, tmp_path: Path, capsys: Any
    ) -> None:
        # Все строки, записанные до появления контракта, блока не имеют —
        # признать их не-качественными значило бы выключить R-07 целиком.
        assert ab(tmp_path, two_agents()) == 0
        assert "a@m effective (latest usable): 0.900" in capsys.readouterr().out
