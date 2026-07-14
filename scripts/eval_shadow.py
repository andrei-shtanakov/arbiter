#!/usr/bin/env python3
"""Shadow routing P1 offline eval — shadow/live agreement report.

Reads ``decisions.shadow_json`` (schema v2) and reports:
- coverage: decisions carrying a shadow snapshot / total (``--since`` window);
- agreement rate overall and per task_type, over ``action = 'assign'`` rows;
- a disagreement table joined with the live agent's latest ``outcomes.status``;
- ``action != 'assign'`` rows separately (fallback distorts the live-top1
  comparison; the stored ``live_top1`` key keeps them analyzable).

CAVEAT (one-sided counterfactual): this measures the *blast radius* of
switching policies, not shadow quality — the shadow agent's outcome is never
observed. Directional claims need benchmark evidence or an interleave.

Stdlib only. Conventions mirror ``check_routable_gate.py``.
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import sys
from pathlib import Path
from typing import Any

CAVEAT = (
    "CAVEAT: one-sided counterfactual — the shadow agent's outcome is never\n"
    "observed. This report measures blast radius, not shadow quality."
)


class EvalInputError(Exception):
    """Invalid input or environment — maps to exit code 2."""


def _rate(agree: int, count: int) -> float:
    return agree / count if count else 0.0


def _parse_shadow(raw: str, task_id: str) -> dict[str, Any] | None:
    """Best-effort parse of a shadow_json blob; malformed rows are skipped."""
    try:
        shadow = json.loads(raw)
    except json.JSONDecodeError:
        print(f"WARNING: malformed shadow_json for {task_id}", file=sys.stderr)
        return None
    return shadow if isinstance(shadow, dict) else None


def _task_type(task_json: str) -> str:
    try:
        return str(json.loads(task_json).get("type", "unknown"))
    except json.JSONDecodeError:
        return "unknown"


def _live_outcome(conn: sqlite3.Connection, task_id: str, agent: str) -> str | None:
    row = conn.execute(
        "SELECT status FROM outcomes WHERE task_id = ?1 AND agent_id = ?2 "
        "ORDER BY id DESC LIMIT 1",
        (task_id, agent),
    ).fetchone()
    return row["status"] if row else None


def report(db_path: Path | str, since: str | None = None) -> dict[str, Any]:
    """Compute the shadow/live agreement report from an arbiter.db."""
    path = Path(db_path)
    if not path.exists():
        raise EvalInputError(f"database not found: {path}")
    conn = sqlite3.connect(path)
    conn.row_factory = sqlite3.Row
    try:
        where = "WHERE timestamp >= ?1" if since else ""
        params = (since,) if since else ()
        rows = conn.execute(
            "SELECT task_id, timestamp, task_json, chosen_agent, action, "
            f"shadow_json FROM decisions {where} ORDER BY id",
            params,
        ).fetchall()

        total = len(rows)
        shadow_rows: list[tuple[sqlite3.Row, dict[str, Any]]] = []
        for row in rows:
            if row["shadow_json"] is None:
                continue
            shadow = _parse_shadow(row["shadow_json"], row["task_id"])
            if shadow is not None:
                shadow_rows.append((row, shadow))

        assign = [(r, s) for r, s in shadow_rows if r["action"] == "assign"]
        non_assign = [
            {
                "task_id": r["task_id"],
                "task_type": _task_type(r["task_json"]),
                "live_agent": r["chosen_agent"],
                "action": r["action"],
                "shadow": s,
            }
            for r, s in shadow_rows
            if r["action"] != "assign"
        ]

        agree_count = sum(1 for _, s in assign if s.get("agrees_with_live"))
        per_tt: dict[str, dict[str, Any]] = {}
        for row, shadow in assign:
            tt = per_tt.setdefault(
                _task_type(row["task_json"]), {"count": 0, "agree": 0}
            )
            tt["count"] += 1
            tt["agree"] += 1 if shadow.get("agrees_with_live") else 0
        for tt in per_tt.values():
            tt["agreement_rate"] = _rate(tt["agree"], tt["count"])

        disagreements = [
            {
                "task_id": row["task_id"],
                "task_type": _task_type(row["task_json"]),
                "live_agent": row["chosen_agent"],
                "action": row["action"],
                "shadow_agent": shadow.get("agent"),
                "shadow_tree": shadow.get("tree"),
                "bench_weight": shadow.get("bench_weight"),
                "live_outcome": _live_outcome(
                    conn, row["task_id"], row["chosen_agent"]
                ),
            }
            for row, shadow in assign
            if not shadow.get("agrees_with_live")
        ]

        return {
            "db": str(path),
            "since": since,
            "total_decisions": total,
            "with_shadow": len(shadow_rows),
            "coverage": _rate(len(shadow_rows), total),
            "assign": {
                "count": len(assign),
                "agree": agree_count,
                "agreement_rate": _rate(agree_count, len(assign)),
                "per_task_type": per_tt,
            },
            "non_assign": non_assign,
            "disagreements": disagreements,
        }
    finally:
        conn.close()


def print_text(r: dict[str, Any]) -> None:
    """Human-readable report to stdout."""
    window = f" since {r['since']}" if r["since"] else ""
    print(f"Shadow routing report — {r['db']}{window}")
    print(
        f"coverage: {r['with_shadow']}/{r['total_decisions']} decisions "
        f"carry a shadow snapshot ({r['coverage']:.1%})"
    )
    a = r["assign"]
    print(
        f"agreement (assign rows): {a['agree']}/{a['count']} "
        f"({a['agreement_rate']:.1%})"
    )
    for tt, stats in sorted(a["per_task_type"].items()):
        print(
            f"  {tt}: {stats['agree']}/{stats['count']} ({stats['agreement_rate']:.1%})"
        )
    if r["disagreements"]:
        print("\ndisagreements (live agent's outcome, one-sided):")
        header = ("task_id", "task_type", "live", "shadow", "live_outcome")
        print("  " + " | ".join(header))
        for d in r["disagreements"]:
            print(
                f"  {d['task_id']} | {d['task_type']} | {d['live_agent']} | "
                f"{d['shadow_agent']} | {d['live_outcome'] or '-'}"
            )
    if r["non_assign"]:
        print(
            f"\nnon-assign rows with shadow (reported separately, "
            f"not in the agreement metric): {len(r['non_assign'])}"
        )
        for n in r["non_assign"]:
            print(
                f"  {n['task_id']} | {n['task_type']} | action={n['action']} "
                f"| live={n['live_agent']} | shadow={n['shadow'].get('agent')} "
                f"| live_top1={n['shadow'].get('live_top1')}"
            )
    print(f"\n{CAVEAT}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Shadow/live agreement report over decisions.shadow_json"
    )
    parser.add_argument(
        "--db", type=Path, default=Path("arbiter.db"), help="path to arbiter.db"
    )
    parser.add_argument(
        "--since",
        help="only decisions with timestamp >= SINCE (e.g. 2026-07-01)",
    )
    parser.add_argument(
        "--json", action="store_true", help="emit the report as JSON to stdout"
    )
    args = parser.parse_args(argv)

    try:
        r = report(args.db, since=args.since)
    except EvalInputError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    if args.json:
        print(json.dumps(r, indent=2))
    else:
        print_text(r)
    return 0


if __name__ == "__main__":
    sys.exit(main())
