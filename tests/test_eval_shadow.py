"""Tests for scripts/eval_shadow.py — shadow/live agreement report (P1)."""

from __future__ import annotations

import json
import sqlite3
from pathlib import Path
from typing import Any

import pytest

from scripts.eval_shadow import EvalInputError, report

# Minimal v2 slice of the arbiter schema: only the columns eval_shadow reads
# (decisions incl. shadow_json; outcomes for the live-outcome join).
SCHEMA = """
CREATE TABLE decisions (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id           TEXT NOT NULL,
    timestamp         TEXT NOT NULL DEFAULT (datetime('now')),
    task_json         TEXT NOT NULL,
    feature_vector    TEXT NOT NULL,
    constraints_json  TEXT,
    chosen_agent      TEXT NOT NULL,
    action            TEXT NOT NULL,
    confidence        REAL NOT NULL,
    decision_path     TEXT NOT NULL,
    fallback_agent    TEXT,
    fallback_reason   TEXT,
    invariants_json   TEXT NOT NULL,
    invariants_passed INTEGER NOT NULL,
    invariants_failed INTEGER NOT NULL,
    inference_us      INTEGER NOT NULL,
    shadow_json       TEXT
);
CREATE TABLE outcomes (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id     TEXT NOT NULL,
    decision_id INTEGER,
    agent_id    TEXT NOT NULL,
    timestamp   TEXT NOT NULL DEFAULT (datetime('now')),
    status      TEXT NOT NULL,
    retry_count INTEGER NOT NULL DEFAULT 0
);
"""


def shadow_blob(
    agent: str, live_top1: str, *, tree: str = "live", weight: float = 0.15
) -> str:
    """A shadow_json payload mirroring the Rust ShadowDecision keys."""
    return json.dumps(
        {
            "agent": agent,
            "confidence": 0.6,
            "tree": tree,
            "bench_weight": weight,
            "live_top1": live_top1,
            "agrees_with_live": agent == live_top1,
        }
    )


def insert_decision(
    conn: sqlite3.Connection,
    task_id: str,
    *,
    task_type: str = "review",
    agent: str = "aider",
    action: str = "assign",
    shadow: str | None = None,
    ts: str = "2026-07-14 00:00:00",
) -> None:
    conn.execute(
        """INSERT INTO decisions (
               task_id, timestamp, task_json, feature_vector, chosen_agent,
               action, confidence, decision_path, invariants_json,
               invariants_passed, invariants_failed, inference_us, shadow_json
           ) VALUES (?, ?, ?, '[]', ?, ?, 0.9, '[]', '[]', 10, 0, 42, ?)""",
        (
            task_id,
            ts,
            json.dumps({"type": task_type, "language": "python"}),
            agent,
            action,
            shadow,
        ),
    )


def insert_outcome(
    conn: sqlite3.Connection, task_id: str, agent: str, status: str
) -> None:
    conn.execute(
        "INSERT INTO outcomes (task_id, agent_id, status) VALUES (?, ?, ?)",
        (task_id, agent, status),
    )


@pytest.fixture
def db_path(tmp_path: Path) -> Path:
    p = tmp_path / "eval.db"
    conn = sqlite3.connect(p)
    conn.executescript(SCHEMA)
    # 4 decisions with shadow: 3 agree, 1 disagree; 2 without shadow.
    insert_decision(conn, "t1", agent="aider", shadow=shadow_blob("aider", "aider"))
    insert_decision(conn, "t2", agent="aider", shadow=shadow_blob("aider", "aider"))
    insert_decision(
        conn,
        "t3",
        task_type="bugfix",
        agent="claude_code",
        shadow=shadow_blob("claude_code", "claude_code"),
    )
    insert_decision(
        conn,
        "t4",
        agent="aider",
        shadow=shadow_blob("claude_code", "aider"),
    )
    insert_decision(conn, "t5", agent="aider")
    insert_decision(conn, "t6", task_type="feature", agent="claude_code")
    # Outcome for the disagreement: the live agent failed.
    insert_outcome(conn, "t4", "aider", "failure")
    conn.commit()
    conn.close()
    return p


def test_coverage_and_agreement(db_path: Path) -> None:
    r = report(db_path)
    assert r["total_decisions"] == 6
    assert r["with_shadow"] == 4
    assert r["coverage"] == pytest.approx(4 / 6)
    assert r["assign"]["agreement_rate"] == pytest.approx(0.75)


def test_disagreement_row_carries_context(db_path: Path) -> None:
    r = report(db_path)
    rows: list[dict[str, Any]] = r["disagreements"]
    assert len(rows) == 1
    row = rows[0]
    assert row["task_id"] == "t4"
    assert row["task_type"] == "review"
    assert row["live_agent"] == "aider"
    assert row["shadow_agent"] == "claude_code"
    assert row["live_outcome"] == "failure"


def test_per_task_type_breakdown(db_path: Path) -> None:
    per_tt = report(db_path)["assign"]["per_task_type"]
    # review: t1, t2 agree; t4 disagrees -> 2/3. bugfix: t3 -> 1/1.
    assert per_tt["review"]["agreement_rate"] == pytest.approx(2 / 3)
    assert per_tt["bugfix"]["agreement_rate"] == pytest.approx(1.0)


def test_since_window_filters(db_path: Path) -> None:
    r = report(db_path, since="2027-01-01")
    assert r["total_decisions"] == 0
    assert r["coverage"] == 0.0


def test_v1_schema_raises_clear_input_error(tmp_path: Path) -> None:
    """A schema v1 DB (no shadow_json column) must raise EvalInputError,
    not a raw sqlite3.OperationalError traceback (CLI exit 2)."""
    p = tmp_path / "v1.db"
    conn = sqlite3.connect(p)
    conn.executescript(SCHEMA.replace(",\n    shadow_json       TEXT", ""))
    conn.commit()
    conn.close()
    with pytest.raises(EvalInputError, match="schema v2"):
        report(p)


def test_missing_db_raises_input_error(tmp_path: Path) -> None:
    with pytest.raises(EvalInputError, match="not found"):
        report(tmp_path / "nope.db")


def test_non_assign_rows_reported_separately(tmp_path: Path) -> None:
    p = tmp_path / "eval2.db"
    conn = sqlite3.connect(p)
    conn.executescript(SCHEMA)
    insert_decision(
        conn,
        "f1",
        agent="codex",
        action="fallback",
        shadow=shadow_blob("aider", "claude_code"),
    )
    insert_decision(conn, "a1", agent="aider", shadow=shadow_blob("aider", "aider"))
    conn.commit()
    conn.close()

    r = report(p)
    # Fallback rows never pollute the assign agreement metric.
    assert r["assign"]["count"] == 1
    assert r["assign"]["agreement_rate"] == pytest.approx(1.0)
    assert len(r["non_assign"]) == 1
    assert r["non_assign"][0]["action"] == "fallback"
    # The stored live_top1 keeps the row analyzable.
    assert r["non_assign"][0]["shadow"]["live_top1"] == "claude_code"
