# R8 — Maestro Integration + Production Readiness

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Formalize the Maestro-Arbiter contract with typed DTOs, add an end-to-end smoke test, and create ops documentation. (R8.2 running_tasks reconciliation already done in R7.)

**Architecture:** Python DTOs use dataclasses for type-safe request/response objects. The E2E test builds the Rust binary, spawns it as a subprocess, runs a full route→report→status cycle, and verifies stats. Ops docs cover startup, monitoring, troubleshooting, retrain, and backup.

**Note on R8.3 (Provider-aware routing):** This task requires changing the feature vector dimensions (22→24+), which would ripple through the decision tree format, bootstrap script, all tests, and the eval framework. This is deferred to a separate release to avoid destabilizing the codebase in Phase C. The current 22-dim vector handles the target use case well.

**Tech Stack:** Python (dataclasses, asyncio, pytest), Markdown (ops docs)

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Modify | `orchestrator/arbiter_client.py` | Add typed DTOs for responses |
| Create | `orchestrator/types.py` | Dataclass DTOs: RouteDecision, OutcomeResult, AgentStatus |
| Create | `orchestrator/tests/test_e2e_smoke.py` | End-to-end smoke test |
| Create | `docs/ops-runbook.md` | Operations runbook |

---

### Task 1: Typed DTOs

**Files:**
- Create: `orchestrator/types.py`
- Modify: `orchestrator/arbiter_client.py`

Add dataclass-based response types so Maestro gets typed objects instead of raw dicts.

- [ ] **Step 1: Create `orchestrator/types.py`**

```python
"""Typed data transfer objects for Arbiter MCP responses.

These DTOs provide type-safe access to Arbiter routing decisions,
outcome reports, and agent status queries. They are parsed from
the raw JSON-RPC tool results.
"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass(frozen=True)
class InvariantCheck:
    """Result of a single invariant rule check."""

    rule: str
    severity: str
    passed: bool
    detail: str


@dataclass(frozen=True)
class RouteDecision:
    """Result of a route_task call."""

    task_id: str
    action: str
    chosen_agent: str
    confidence: float
    reasoning: str
    decision_path: list[str] = field(default_factory=list)
    fallback_agent: str | None = None
    fallback_reason: str | None = None
    invariant_checks: list[InvariantCheck] = field(default_factory=list)
    inference_us: int = 0
    candidates_evaluated: int = 0
    warnings: list[str] = field(default_factory=list)

    @classmethod
    def from_dict(cls, data: dict) -> RouteDecision:
        """Parse from raw JSON dict."""
        invariants = [
            InvariantCheck(**inv)
            for inv in data.get("invariant_checks", [])
        ]
        return cls(
            task_id=data.get("task_id", ""),
            action=data.get("action", ""),
            chosen_agent=data.get("chosen_agent", ""),
            confidence=data.get("confidence", 0.0),
            reasoning=data.get("reasoning", ""),
            decision_path=data.get("decision_path", []),
            fallback_agent=data.get("fallback_agent"),
            fallback_reason=data.get("fallback_reason"),
            invariant_checks=invariants,
            inference_us=data.get("inference_us", 0),
            candidates_evaluated=data.get("candidates_evaluated", 0),
            warnings=data.get("warnings", []),
        )


@dataclass(frozen=True)
class UpdatedStats:
    """Agent stats returned with outcome reports."""

    agent_id: str
    total_tasks: int
    success_rate: float
    avg_duration_min: float
    avg_cost_usd: float


@dataclass(frozen=True)
class OutcomeResult:
    """Result of a report_outcome call."""

    task_id: str
    recorded: bool
    updated_stats: UpdatedStats
    retrain_suggested: bool
    warnings: list[str] = field(default_factory=list)

    @classmethod
    def from_dict(cls, data: dict) -> OutcomeResult:
        """Parse from raw JSON dict."""
        stats_data = data.get("updated_stats", {})
        stats = UpdatedStats(
            agent_id=stats_data.get("agent_id", ""),
            total_tasks=stats_data.get("total_tasks", 0),
            success_rate=stats_data.get("success_rate", 0.0),
            avg_duration_min=stats_data.get("avg_duration_min", 0.0),
            avg_cost_usd=stats_data.get("avg_cost_usd", 0.0),
        )
        return cls(
            task_id=data.get("task_id", ""),
            recorded=data.get("recorded", False),
            updated_stats=stats,
            retrain_suggested=data.get("retrain_suggested", False),
            warnings=data.get("warnings", []),
        )


@dataclass(frozen=True)
class AgentCapabilities:
    """Static agent capabilities."""

    languages: list[str] = field(default_factory=list)
    task_types: list[str] = field(default_factory=list)
    max_concurrent: int = 0
    cost_per_hour: float = 0.0


@dataclass(frozen=True)
class AgentStatusInfo:
    """Status information for a single agent."""

    id: str
    display_name: str
    state: str
    capabilities: AgentCapabilities
    total_tasks: int = 0
    success_rate: float = 0.0
    running_tasks: int = 0

    @classmethod
    def from_dict(cls, data: dict) -> AgentStatusInfo:
        """Parse from raw JSON dict."""
        caps_data = data.get("capabilities", {})
        caps = AgentCapabilities(
            languages=caps_data.get("languages", []),
            task_types=caps_data.get("task_types", []),
            max_concurrent=caps_data.get("max_concurrent", 0),
            cost_per_hour=caps_data.get("cost_per_hour", 0.0),
        )
        perf = data.get("performance", {})
        load = data.get("current_load", {})
        return cls(
            id=data.get("id", ""),
            display_name=data.get("display_name", ""),
            state=data.get("state", ""),
            capabilities=caps,
            total_tasks=perf.get("total_tasks", 0),
            success_rate=perf.get("success_rate", 0.0),
            running_tasks=load.get("running_tasks", 0),
        )
```

- [ ] **Step 2: Add typed methods to ArbiterClient**

In `orchestrator/arbiter_client.py`, add import at top:

```python
from orchestrator.types import AgentStatusInfo, OutcomeResult, RouteDecision
```

Add typed convenience methods after the existing `get_agent_status`:

```python
    async def route_task_typed(
        self,
        task_id: str,
        task: dict[str, Any],
        constraints: dict[str, Any] | None = None,
    ) -> RouteDecision:
        """Route a task and return a typed RouteDecision."""
        raw = await self.route_task(task_id, task, constraints)
        return RouteDecision.from_dict(raw)

    async def report_outcome_typed(
        self,
        task_id: str,
        agent_id: str,
        status: str,
        **kwargs: Any,
    ) -> OutcomeResult:
        """Report outcome and return a typed OutcomeResult."""
        raw = await self.report_outcome(task_id, agent_id, status, **kwargs)
        return OutcomeResult.from_dict(raw)

    async def get_agent_status_typed(
        self,
        agent_id: str | None = None,
    ) -> list[AgentStatusInfo]:
        """Query agent status and return typed AgentStatusInfo list."""
        raw = await self.get_agent_status(agent_id)
        return [AgentStatusInfo.from_dict(a) for a in raw.get("agents", [])]
```

- [ ] **Step 3: Run tests**

Run: `cd /Users/Andrei_Shtanakov/labs/all_ai_orchestrators/arbiter && uv run python -c "from orchestrator.types import RouteDecision, OutcomeResult, AgentStatusInfo; print('imports ok')"`
Expected: "imports ok"

- [ ] **Step 4: Commit**

```bash
git add orchestrator/types.py orchestrator/arbiter_client.py
git commit -m "feat: add typed DTOs for Arbiter route/outcome/status responses"
```

---

### Task 2: End-to-End Smoke Test

**Files:**
- Create: `orchestrator/tests/test_e2e_smoke.py`

Full cycle test: build binary → spawn → route → report → status → verify.

- [ ] **Step 1: Create the E2E test**

```python
"""End-to-end smoke test for Arbiter MCP server.

Builds the Rust binary, spawns it as subprocess, runs a full
route → report_outcome → get_agent_status cycle, verifies results.

Requires: cargo build to have been run (uses release binary if available,
otherwise debug).
"""

from __future__ import annotations

import asyncio
import subprocess
import sys
from pathlib import Path

import pytest

# Add parent to path for imports
sys.path.insert(0, str(Path(__file__).parent.parent.parent))

from orchestrator.arbiter_client import ArbiterClient, ArbiterClientConfig
from orchestrator.types import OutcomeResult, RouteDecision


def find_binary() -> Path:
    """Find the arbiter-mcp binary (release or debug)."""
    project_root = Path(__file__).parent.parent.parent
    release = project_root / "target" / "release" / "arbiter-mcp"
    debug = project_root / "target" / "debug" / "arbiter-mcp"

    if release.exists():
        return release
    if debug.exists():
        return debug

    # Try building
    result = subprocess.run(
        ["cargo", "build", "--bin", "arbiter-mcp"],
        cwd=str(project_root),
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        pytest.skip(f"Cannot build arbiter-mcp: {result.stderr[:200]}")

    if debug.exists():
        return debug
    pytest.skip("arbiter-mcp binary not found after build")


@pytest.fixture
def project_root() -> Path:
    return Path(__file__).parent.parent.parent


@pytest.fixture
def client_config(project_root: Path) -> ArbiterClientConfig:
    binary = find_binary()
    return ArbiterClientConfig(
        binary_path=str(binary),
        tree_path=str(project_root / "models" / "agent_policy_tree.json"),
        config_dir=str(project_root / "config"),
        log_level="warn",
    )


@pytest.mark.anyio
async def test_full_route_report_status_cycle(
    client_config: ArbiterClientConfig,
) -> None:
    """Full E2E: route → report → status → verify stats updated."""
    client = ArbiterClient(client_config)

    try:
        # Start and handshake
        caps = await client.start()
        assert "capabilities" in caps
        assert client.is_running

        # Route a task
        decision_raw = await client.route_task(
            "e2e-task-1",
            {
                "type": "bugfix",
                "language": "python",
                "complexity": "simple",
                "priority": "normal",
                "description": "E2E smoke test task",
            },
        )
        decision = RouteDecision.from_dict(decision_raw)
        assert decision.action in ("assign", "fallback")
        assert decision.chosen_agent != ""
        assert decision.confidence > 0.0
        chosen_agent = decision.chosen_agent

        # Report outcome
        outcome_raw = await client.report_outcome(
            "e2e-task-1",
            chosen_agent,
            "success",
            duration_min=5.0,
            cost_usd=0.15,
            tokens_used=10000,
        )
        outcome = OutcomeResult.from_dict(outcome_raw)
        assert outcome.recorded is True
        assert outcome.updated_stats.agent_id == chosen_agent
        assert outcome.updated_stats.total_tasks >= 1

        # Get agent status
        status_raw = await client.get_agent_status()
        agents = status_raw.get("agents", [])
        assert len(agents) >= 1

        # Verify the chosen agent appears in status
        agent_ids = [a["id"] for a in agents]
        assert chosen_agent in agent_ids

    finally:
        await client.stop()


@pytest.mark.anyio
async def test_route_two_tasks_sequentially(
    client_config: ArbiterClientConfig,
) -> None:
    """Route two different tasks to verify consistent behavior."""
    client = ArbiterClient(client_config)

    try:
        await client.start()

        # Task 1: Python bugfix
        d1 = await client.route_task(
            "e2e-seq-1",
            {
                "type": "bugfix",
                "language": "python",
                "complexity": "simple",
                "priority": "normal",
            },
        )
        decision1 = RouteDecision.from_dict(d1)
        assert decision1.action in ("assign", "fallback")

        # Report outcome for task 1
        await client.report_outcome(
            "e2e-seq-1", decision1.chosen_agent, "success"
        )

        # Task 2: Rust feature (more complex)
        d2 = await client.route_task(
            "e2e-seq-2",
            {
                "type": "feature",
                "language": "rust",
                "complexity": "complex",
                "priority": "high",
            },
        )
        decision2 = RouteDecision.from_dict(d2)
        assert decision2.action in ("assign", "fallback")

        await client.report_outcome(
            "e2e-seq-2", decision2.chosen_agent, "success"
        )

    finally:
        await client.stop()
```

- [ ] **Step 2: Run the E2E test**

Run: `cd /Users/Andrei_Shtanakov/labs/all_ai_orchestrators/arbiter && cargo build --bin arbiter-mcp && uv run pytest orchestrator/tests/test_e2e_smoke.py -v`
Expected: 2 tests pass.

- [ ] **Step 3: Commit**

```bash
git add orchestrator/tests/test_e2e_smoke.py
git commit -m "feat: add end-to-end smoke test for full route-report-status cycle"
```

---

### Task 3: Ops Runbook

**Files:**
- Create: `docs/ops-runbook.md`

- [ ] **Step 1: Create the runbook**

```markdown
# Arbiter Operations Runbook

## Startup

```bash
# Build
cargo build --release

# Run with defaults
cargo run --release --bin arbiter-mcp

# Run with custom paths
cargo run --release --bin arbiter-mcp -- \
  --config config/ \
  --tree models/agent_policy_tree.json \
  --db arbiter.db \
  --log-level info
```

The server starts on stdio (JSON-RPC 2.0). On startup it:
1. Loads config from `config/agents.toml` and `config/invariants.toml`
2. Loads decision tree from `models/agent_policy_tree.json` (runs in round-robin mode if unavailable)
3. Opens SQLite database (creates if missing, runs migrations)
4. Purges records older than 90 days
5. Resets orphaned `running_tasks` counters (crash recovery)
6. Starts file watcher for hot-reloading config and tree

## Claude Desktop Setup

1. Copy `config/claude_desktop_config.json` to Claude Desktop settings
2. Replace `__ARBITER_DIR__` with the absolute path to the Arbiter project
3. Restart Claude Desktop

## Monitoring

### get_metrics
Returns decision counters, fallback rate, and latency statistics.

### get_budget_status
Returns total spend, remaining budget, and per-agent cost breakdown.

### get_agent_status
Returns per-agent state, capabilities, and performance history.

### Logs
All logs go to stderr. Use `--log-level debug` for verbose output.

Key log events:
- `route_task decision` — every routing decision with agent, confidence, latency
- `report_outcome recorded` — every outcome with agent and status
- `config reloaded` — hot reload triggered
- `tree reloaded` — decision tree reloaded
- `purged old records` — retention cleanup

## Troubleshooting

### Server won't start
- Check config syntax: `cat config/agents.toml | toml-test` or look for parse errors in stderr
- Check tree JSON: valid JSON with `n_features`, `n_classes`, `nodes` arrays
- Check DB permissions: Arbiter needs read/write to the DB path

### All tasks rejected
- Check `get_agent_status` — are agents in `failed` state?
- Check `get_metrics` — high `reject_rate`?
- Check invariant thresholds in `config/invariants.toml`
- Running tasks may be stuck: restart resets counters

### Performance degraded
- Check `get_metrics` latency stats
- Check DB size: `ls -la arbiter.db`
- Purge runs on startup; for immediate purge, restart the server

### Hot reload not working
- Check stderr for watcher errors
- Only `.toml` files in config dir and the exact tree JSON path are watched
- Invalid config/tree files are rejected (old state preserved)

## Retraining

```bash
# Retrain from expert rules only
uv run python scripts/bootstrap_agent_tree.py

# Retrain including real outcome data
uv run python scripts/bootstrap_agent_tree.py --from-db arbiter.db

# Evaluate tree quality
uv run python scripts/eval_tree.py
```

The tree file is hot-reloaded — no restart needed after retraining.

## Database Backup

```bash
# SQLite backup (safe while server is running, WAL mode)
sqlite3 arbiter.db ".backup arbiter-backup.db"

# Or simply copy (stop server first for consistency)
cp arbiter.db arbiter-backup.db
```

## Configuration Reference

### agents.toml
| Field | Type | Description |
|-------|------|-------------|
| display_name | string | Human-readable name |
| supports_languages | string[] | Languages the agent handles |
| supports_types | string[] | Task types the agent handles |
| max_concurrent | int | Max parallel tasks |
| cost_per_hour | float | Cost estimate (USD/hour) |
| avg_duration_min | float | Average task duration |

### invariants.toml
| Section | Field | Description |
|---------|-------|-------------|
| budget | threshold_usd | Budget limit for cost estimates |
| retries | max_retries | Max retry attempts per task |
| rate_limit | calls_per_minute | API rate limit |
| agent_health | max_failures_24h | Failure threshold for agent health |
| concurrency | max_total_concurrent | Max total running tasks |
| sla | buffer_multiplier | SLA duration buffer (e.g., 1.5x) |
```

- [ ] **Step 2: Commit**

```bash
git add docs/ops-runbook.md
git commit -m "docs: add operations runbook for startup, monitoring, troubleshooting, retrain"
```

---

## Exit Criteria Checklist

- [ ] `orchestrator/types.py` has RouteDecision, OutcomeResult, AgentStatusInfo dataclasses
- [ ] ArbiterClient has `*_typed()` methods returning DTOs
- [ ] E2E smoke test passes (route → report → status)
- [ ] Ops runbook covers: startup, monitoring, troubleshooting, retrain, backup
- [ ] All existing tests still pass
