"""A/B check for the R-07 benchmark re-rank on a code-review task.

Spawns arbiter-mcp twice (the ARBITER_BENCH_WEIGHT env var is read per-process
inside route_task), routes one `review` task each time, and prints the ranked
candidates + decision_path so the `bench_adjust[...]` audit line is visible.

Usage::

    ARBITER_BENCH_WEIGHT is set internally; just run:
    uv run python scripts/ab_bench_rerank.py --db arbiter.db
"""

from __future__ import annotations

import argparse
import asyncio
import os
from pathlib import Path

import sys

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from orchestrator.arbiter_client import ArbiterClient, ArbiterClientConfig

REVIEW_TASK = {
    "type": "review",
    "language": "python",
    "complexity": "moderate",
    "priority": "normal",
}


async def run_once(db_path: Path, weight: str) -> None:
    """Route one review task with a given ARBITER_BENCH_WEIGHT and print result."""
    os.environ["ARBITER_BENCH_WEIGHT"] = weight
    client = ArbiterClient(ArbiterClientConfig(db_path=db_path, log_level="warn"))
    await client.start()
    try:
        decision = await client.route_task("ab-review-task", REVIEW_TASK)
    finally:
        await client.stop()

    print(f"\n{'=' * 60}\nARBITER_BENCH_WEIGHT={weight}\n{'=' * 60}")
    print(f"action:       {decision.get('action')}")
    print(f"chosen_agent: {decision.get('chosen_agent')}")
    print(f"confidence:   {decision.get('confidence')}")
    print(f"candidates_evaluated: {decision.get('candidates_evaluated')}")
    print("decision_path:")
    for step in decision.get("decision_path", []):
        print(f"   - {step}")


async def main_async(db_path: Path) -> int:
    """Run the WEIGHT=0 then WEIGHT=0.15 A/B."""
    await run_once(db_path, "0")
    await run_once(db_path, "0.15")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", type=Path, default=Path("arbiter.db"))
    args = parser.parse_args()
    return asyncio.run(main_async(args.db))


if __name__ == "__main__":
    raise SystemExit(main())
