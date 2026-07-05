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

import argparse
import json
import math
import re
import sqlite3
import sys
import tomllib
from datetime import date, datetime, timezone
from pathlib import Path
from typing import Any

SUITE_RE = re.compile(r"^[0-9a-f]{7,64}$")
ISO_DATE_RE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
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
    datetime.date) are rejected: the schema requires a quoted string.
    fromisoformat alone is too lax (accepts "20260703" and week dates), so a
    regex pre-check pins the exact format."""
    if not isinstance(value, str) or not ISO_DATE_RE.fullmatch(value):
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

    verify_parser = sub.add_parser("verify", help="local data-gate against arbiter.db")
    verify_parser.add_argument("--db", required=True, type=Path)
    verify_parser.add_argument("--eps", type=float, default=DEFAULT_EPS)
    verify_parser.add_argument(
        "--catalog", type=Path, default=Path("config/agents-catalog.toml")
    )

    args = parser.parse_args(argv)
    try:
        if args.mode == "gate":
            return run_gate(args.base_file, args.head_file)
        return run_verify(args.db, args.catalog, args.eps)
    except GateInputError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
