"""End-to-end smoke tests for the Arbiter MCP server.

Exercises the full route -> report -> status cycle through the
real binary, validating typed DTO parsing along the way.
"""

from __future__ import annotations

from pathlib import Path

import pytest
import pytest_asyncio

from orchestrator.arbiter_client import ArbiterClient, ArbiterClientConfig
from orchestrator.types import (
    AgentStatusInfo,
    OutcomeResult,
    RouteDecision,
)

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

PROJECT_ROOT = Path(__file__).resolve().parent.parent.parent

_BINARY_CANDIDATES = [
    PROJECT_ROOT / "target" / "release" / "arbiter-mcp",
    PROJECT_ROOT / "target" / "debug" / "arbiter-mcp",
]
TREE_PATH = PROJECT_ROOT / "models" / "agent_policy_tree.json"
CONFIG_DIR = PROJECT_ROOT / "config"


def _find_binary() -> Path:
    for candidate in _BINARY_CANDIDATES:
        if candidate.exists():
            return candidate
    pytest.skip("Arbiter binary not found. Run: cargo build --bin arbiter-mcp")
    raise AssertionError("unreachable")  # for type checker


@pytest_asyncio.fixture
async def client() -> ArbiterClient:
    """Start an ArbiterClient for each test, stop it afterwards."""
    binary = _find_binary()
    config = ArbiterClientConfig(
        binary_path=binary,
        tree_path=TREE_PATH,
        config_dir=CONFIG_DIR,
        log_level="warn",
    )
    c = ArbiterClient(config)
    await c.start()
    try:
        yield c  # type: ignore[misc]
    finally:
        await c.stop()


# ---------------------------------------------------------------------------
# E2E-01: Full route -> report -> status cycle
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_full_route_report_status_cycle(
    client: ArbiterClient,
) -> None:
    """Route a bugfix, report success, query status — all typed."""
    # 1. Route
    decision = await client.route_task_typed(
        "e2e-01",
        {
            "type": "bugfix",
            "language": "python",
            "complexity": "simple",
            "priority": "normal",
        },
    )
    assert isinstance(decision, RouteDecision)
    assert decision.task_id == "e2e-01"
    assert decision.action in ("assign", "fallback")
    assert 0.0 <= decision.confidence <= 1.0
    assert len(decision.invariant_checks) == 10
    for check in decision.invariant_checks:
        assert check.rule
        assert check.severity.lower() in ("critical", "warning")

    agent = decision.chosen_agent
    assert agent  # non-empty

    # 2. Report outcome
    outcome = await client.report_outcome_typed(
        task_id="e2e-01",
        agent_id=agent,
        status="success",
        duration_min=3.5,
        tokens_used=800,
        cost_usd=0.04,
    )
    assert isinstance(outcome, OutcomeResult)
    assert outcome.task_id == "e2e-01"
    assert outcome.recorded is True
    assert outcome.updated_stats.agent_id == agent
    assert outcome.updated_stats.total_tasks >= 1

    # 3. Query agent status
    agents = await client.get_agent_status_typed()
    assert len(agents) > 0
    assert all(isinstance(a, AgentStatusInfo) for a in agents)

    # The agent that handled our task should appear in the list
    agent_ids = [a.id for a in agents]
    assert agent in agent_ids


# ---------------------------------------------------------------------------
# E2E-02: Two sequential tasks
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_route_two_tasks_sequentially(
    client: ArbiterClient,
) -> None:
    """Route a Python bugfix, report it, then route a Rust feature."""
    # Task 1: Python bugfix
    d1 = await client.route_task_typed(
        "e2e-02a",
        {
            "type": "bugfix",
            "language": "python",
            "complexity": "simple",
            "priority": "normal",
        },
    )
    assert d1.task_id == "e2e-02a"
    assert d1.action in ("assign", "fallback")

    await client.report_outcome_typed(
        task_id="e2e-02a",
        agent_id=d1.chosen_agent,
        status="success",
        duration_min=2.0,
        tokens_used=500,
        cost_usd=0.02,
    )

    # Task 2: Rust feature
    d2 = await client.route_task_typed(
        "e2e-02b",
        {
            "type": "feature",
            "language": "rust",
            "complexity": "complex",
            "priority": "high",
        },
    )
    assert d2.task_id == "e2e-02b"
    assert d2.action in ("assign", "fallback")

    outcome = await client.report_outcome_typed(
        task_id="e2e-02b",
        agent_id=d2.chosen_agent,
        status="success",
        duration_min=15.0,
        tokens_used=3000,
        cost_usd=0.25,
    )
    assert outcome.recorded is True
    assert outcome.updated_stats.total_tasks >= 1
