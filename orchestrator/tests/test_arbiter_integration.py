"""Protocol tests for the Arbiter MCP client.

Tests PT-01 through PT-07: handshake, routing, report cycle,
error handling, crash recovery, and batch operations.
"""

from __future__ import annotations

from pathlib import Path

import pytest
import pytest_asyncio

from orchestrator.arbiter_client import (
    ArbiterClient,
    ArbiterClientConfig,
    ArbiterProtocolError,
    FallbackScheduler,
)

# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

PROJECT_ROOT = Path(__file__).resolve().parent.parent.parent
BINARY_PATH = PROJECT_ROOT / "target" / "release" / "arbiter-mcp"
TREE_PATH = PROJECT_ROOT / "models" / "agent_policy_tree.json"
CONFIG_DIR = PROJECT_ROOT / "config"


def _skip_if_no_binary() -> None:
    if not BINARY_PATH.exists():
        pytest.skip(
            f"Arbiter binary not found at {BINARY_PATH}. "
            f"Run: cargo build --release --bin arbiter-mcp"
        )


@pytest_asyncio.fixture
async def client() -> ArbiterClient:
    """Create a started ArbiterClient, stop it after the test."""
    _skip_if_no_binary()
    config = ArbiterClientConfig(
        binary_path=BINARY_PATH,
        tree_path=TREE_PATH,
        config_dir=CONFIG_DIR,
        log_level="warn",
        reconnect_delay=0.1,
    )
    c = ArbiterClient(config)
    await c.start()
    yield c  # type: ignore[misc]
    await c.stop()


def _simple_task() -> dict:
    """Return a minimal valid task dict."""
    return {
        "type": "bugfix",
        "language": "python",
        "complexity": "simple",
        "priority": "normal",
    }


# ---------------------------------------------------------------------------
# PT-01: Handshake
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_pt01_handshake() -> None:
    """PT-01: ArbiterClient.start() performs MCP handshake."""
    _skip_if_no_binary()
    config = ArbiterClientConfig(
        binary_path=BINARY_PATH,
        tree_path=TREE_PATH,
        config_dir=CONFIG_DIR,
        log_level="warn",
    )
    c = ArbiterClient(config)
    result = await c.start()

    assert result["protocolVersion"] == "2024-11-05"
    assert "capabilities" in result
    assert result["serverInfo"]["name"] == "arbiter"
    assert c.is_running

    await c.stop()
    assert not c.is_running


# ---------------------------------------------------------------------------
# PT-02: Route simple task
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_pt02_route_simple(client: ArbiterClient) -> None:
    """PT-02: Route a simple bugfix task and get a decision."""
    decision = await client.route_task("pt02-task", _simple_task())

    assert decision["task_id"] == "pt02-task"
    assert decision["action"] in ("assign", "fallback")
    assert "chosen_agent" in decision
    assert 0.0 <= decision["confidence"] <= 1.0
    assert "reasoning" in decision
    assert "decision_path" in decision
    assert "invariant_checks" in decision
    checks = decision["invariant_checks"]
    assert len(checks) == 10
    for check in checks:
        assert "rule" in check
        assert "severity" in check
        assert "passed" in check
    assert "metadata" in decision
    meta = decision["metadata"]
    assert "inference_us" in meta
    assert "feature_vector" in meta
    assert len(meta["feature_vector"]) == 22


# ---------------------------------------------------------------------------
# PT-03: Route + report cycle
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_pt03_route_report_cycle(client: ArbiterClient) -> None:
    """PT-03: Route a task, then report its outcome."""
    decision = await client.route_task("pt03-task", _simple_task())
    agent = decision["chosen_agent"]

    outcome = await client.report_outcome(
        task_id="pt03-task",
        agent_id=agent,
        status="success",
        duration_min=5.0,
        tokens_used=1000,
        cost_usd=0.05,
    )

    assert outcome["task_id"] == "pt03-task"
    assert outcome["recorded"] is True
    assert "updated_stats" in outcome
    stats = outcome["updated_stats"]
    assert stats["agent_id"] == agent
    assert stats["total_tasks"] >= 1


# ---------------------------------------------------------------------------
# PT-04: Invalid params error
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_pt04_invalid_params(client: ArbiterClient) -> None:
    """PT-04: Sending invalid params returns a protocol error."""
    with pytest.raises(ArbiterProtocolError) as exc_info:
        # Missing required 'task' field
        await client.route_task("pt04-task", {})  # type: ignore[arg-type]

    assert exc_info.value.code == -32602


# ---------------------------------------------------------------------------
# PT-05: Unknown tool error
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_pt05_unknown_tool(client: ArbiterClient) -> None:
    """PT-05: Calling an unknown tool returns a protocol error."""
    with pytest.raises(ArbiterProtocolError) as exc_info:
        await client._call_tool_once("nonexistent_tool", {})

    assert exc_info.value.code == -32602
    assert "nonexistent_tool" in exc_info.value.rpc_message


# ---------------------------------------------------------------------------
# PT-06: Server crash recovery
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_pt06_server_crash_recovery() -> None:
    """PT-06: Client reconnects after the server crashes."""
    _skip_if_no_binary()
    config = ArbiterClientConfig(
        binary_path=BINARY_PATH,
        tree_path=TREE_PATH,
        config_dir=CONFIG_DIR,
        log_level="warn",
        reconnect_delay=0.1,
        max_reconnect_attempts=3,
    )
    c = ArbiterClient(config)
    await c.start()

    try:
        # Kill the server process
        assert c._process is not None
        c._process.kill()
        await c._process.wait()

        # Next call should trigger reconnect and succeed
        decision = await c.route_task("pt06-task", _simple_task())
        assert decision["task_id"] == "pt06-task"
        assert decision["action"] in ("assign", "fallback")
    finally:
        await c.stop()


# ---------------------------------------------------------------------------
# PT-07: Large batch (100 tasks)
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_pt07_large_batch(client: ArbiterClient) -> None:
    """PT-07: Route 100 tasks sequentially to verify stability."""
    task_types = [
        "feature",
        "bugfix",
        "refactor",
        "test",
        "docs",
        "review",
        "research",
    ]
    languages = [
        "python",
        "rust",
        "typescript",
        "go",
        "mixed",
        "other",
    ]

    for i in range(100):
        task = {
            "type": task_types[i % len(task_types)],
            "language": languages[i % len(languages)],
            "complexity": "simple",
            "priority": "normal",
        }
        decision = await client.route_task(f"batch-{i:03d}", task)
        assert decision["task_id"] == f"batch-{i:03d}"
        assert decision["action"] in ("assign", "fallback", "reject")


# ---------------------------------------------------------------------------
# FallbackScheduler tests
# ---------------------------------------------------------------------------


def test_fallback_scheduler_round_robin() -> None:
    """FallbackScheduler cycles through agents in order."""
    scheduler = FallbackScheduler()
    agents = [scheduler.next_agent(f"t{i}") for i in range(6)]
    assert agents == [
        "claude_code",
        "codex_cli",
        "aider",
        "claude_code",
        "codex_cli",
        "aider",
    ]


def test_fallback_scheduler_reset() -> None:
    """FallbackScheduler.reset() restarts the cycle."""
    scheduler = FallbackScheduler()
    scheduler.next_agent("t0")
    scheduler.next_agent("t1")
    scheduler.reset()
    assert scheduler.next_agent("t2") == "claude_code"


def test_fallback_scheduler_custom_agents() -> None:
    """FallbackScheduler works with custom agent lists."""
    scheduler = FallbackScheduler(agents=["a", "b"])
    assert scheduler.next_agent("t0") == "a"
    assert scheduler.next_agent("t1") == "b"
    assert scheduler.next_agent("t2") == "a"
