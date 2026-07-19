"""Ingest ATP benchmark payloads into arbiter.db via the report_benchmark tool.

One-off operational script for R-07 (sweep-2026-06-21). Reads
`report_benchmark_*.json` payloads from a sweep directory and feeds each one
through the contractual MCP path (ATP -> arbiter `report_benchmark`), so the
`benchmark_runs` table is populated exactly as a live ingest would.

Usage::

    uv run python scripts/ingest_benchmark_payloads.py \
        --payloads ../atp-platform/_cowork_output/r07-pipecheck/sweep-2026-06-21  (gov:allow-cowork) \
        --db arbiter.db

Idempotent: re-running reports each row as "duplicate" (run_id PRIMARY KEY).
"""

from __future__ import annotations

import argparse
import asyncio
import json
import sys
from pathlib import Path

# Allow running as a plain script from the repo root.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from orchestrator.arbiter_client import ArbiterClient, ArbiterClientConfig


async def ingest(payload_dir: Path, db_path: Path) -> int:
    """Ingest every report_benchmark_*.json under payload_dir into db_path.

    Returns the process exit code (0 on success, 1 on any failure).
    """
    payloads = sorted(payload_dir.glob("report_benchmark_*.json"))
    if not payloads:
        print(f"No report_benchmark_*.json found in {payload_dir}", file=sys.stderr)
        return 1

    client = ArbiterClient(ArbiterClientConfig(db_path=db_path, log_level="warn"))
    await client.start()
    created = 0
    duplicate = 0
    unexpected = 0
    try:
        for path in payloads:
            args = json.loads(path.read_text())
            agent_id = args.get("agent_id", "?") if isinstance(args, dict) else "?"
            result = await client.report_benchmark(args)
            status = result.get("status", "?")
            run_id = result.get("run_id", "?")
            print(f"{path.name}: {status} ({agent_id}, run_id={run_id})")
            if status == "created":
                created += 1
            elif status == "duplicate":
                duplicate += 1
            else:
                # Unknown status (e.g. tool/schema change) — do not silently pass.
                unexpected += 1
                print(
                    f"  ! unexpected status {status!r} for {path.name}", file=sys.stderr
                )
    finally:
        await client.stop()

    print(
        f"\nDone: {len(payloads)} payloads -> {created} created, "
        f"{duplicate} duplicate, {unexpected} unexpected"
    )
    return 1 if unexpected else 0


def main() -> int:
    """Parse args and run the async ingest."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--payloads",
        type=Path,
        required=True,
        help="Directory containing report_benchmark_*.json files",
    )
    parser.add_argument(
        "--db",
        type=Path,
        default=Path("arbiter.db"),
        help="Path to arbiter.db (default: arbiter.db)",
    )
    args = parser.parse_args()
    return asyncio.run(ingest(args.payloads, args.db))


if __name__ == "__main__":
    raise SystemExit(main())
