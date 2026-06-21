# Arbiter — Coding Agent Policy Engine

An MCP server that decides which coding agent (Claude Code, Codex CLI, Aider) should handle a given task, based on Decision Tree inference, safety invariants, and historical performance data.

## Quick Start

### Prerequisites

- Rust 1.75+ (with `cargo`)
- Python 3.11+ (with [`uv`](https://github.com/astral-sh/uv))

### Build

```bash
# Build entire workspace (release mode)
cargo build --release

# Verify tests pass
cargo test
```

### Generate Decision Tree

```bash
uv run python scripts/bootstrap_agent_tree.py
```

This creates `models/agent_policy_tree.json` from expert rules.

### Run the MCP Server

```bash
cargo run --release --bin arbiter-mcp
```

The server reads JSON-RPC 2.0 from stdin and writes responses to stdout. All logs go to stderr.

**Options:**

| Flag | Default | Description |
|------|---------|-------------|
| `--tree <PATH>` | `models/agent_policy_tree.json` | Decision tree JSON file |
| `--config <DIR>` | `config/` | Config directory (agents.toml, invariants.toml) |
| `--db <PATH>` | `arbiter.db` | SQLite database path |
| `--log-level <LEVEL>` | `info` | Log level: trace, debug, info, warn, error |

If the decision tree file is missing or invalid, the server starts in **degraded round-robin mode** and still accepts requests.

## Architecture

```
┌─────────────────────────────────────────────────────┐
│              Agent Orchestrator (Python)             │
│         Task Queue → Dependency Resolver → Spawner  │
│                        │                            │
│                   MCP Client                        │
└────────────────────────┬────────────────────────────┘
                         │ JSON-RPC 2.0 (stdio)
┌────────────────────────▼────────────────────────────┐
│              Arbiter (Rust MCP Server)               │
│                                                      │
│  route_task → Feature Builder → DT Inference         │
│                                  → Invariant Check   │
│                                  → Agent Selection   │
│                                                      │
│  report_outcome → Stats Update → Feedback Store      │
│  get_agent_status → Registry Query                   │
│  get_metrics → Decision counters + latency stats     │
│  get_budget_status → Spend tracking + per-agent cost │
│  report_benchmark → Benchmark Store (R-06b M4)       │
└──────────────────────────────────────────────────────┘
```

**Decision flow:** `task JSON → 22-dim feature vector → Decision Tree → ranked agents → 10 invariant checks → assign / fallback / reject`

### Project Structure

```
arbiter/
├── arbiter-core/        # Library: types, DT inference, invariants, policy engine
├── arbiter-mcp/         # Binary: MCP server (stdio JSON-RPC 2.0)
├── arbiter-cli/         # CLI: benchmarks and smoke tests
├── config/
│   ├── agents.toml      # Agent definitions
│   └── invariants.toml  # Safety rule thresholds
├── models/
│   └── agent_policy_tree.json  # Bootstrap decision tree
├── scripts/             # Python: tree generation and evaluation utilities
└── orchestrator/        # Python MCP client library
```

**arbiter-core** is a pure logic library — no database, no network. It owns types, decision tree inference, invariant checking, and feature vector construction. The one exception is the `obs` observability emitter: it lives in `arbiter-core` as shared infrastructure (so any binary can use it) and writes file-per-pid JSONL log sinks. It is currently initialized only by the `arbiter-mcp` server.

**arbiter-mcp** is the main binary — it owns the MCP protocol, stdio I/O, SQLite persistence, config loading, and tool dispatch.

## MCP Tool Usage

Arbiter exposes six tools over the MCP protocol:

### route_task

Routes a coding task to the best available agent.

**Request:**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "route_task",
    "arguments": {
      "task_id": "task-42",
      "task": {
        "type": "bugfix",
        "language": "python",
        "complexity": "simple",
        "priority": "normal",
        "scope": ["src/auth.py"],
        "branch": "fix/login-bug",
        "estimated_tokens": 2000,
        "description": "Fix null pointer in login handler"
      },
      "constraints": {
        "budget_remaining_usd": 5.0,
        "preferred_agent": "claude_code@claude-sonnet-4-6"
      }
    }
  }
}
```

**Response:**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "content": [{
      "type": "text",
      "text": "{\"task_id\":\"task-42\",\"action\":\"assign\",\"chosen_agent\":\"claude_code@claude-sonnet-4-6\",\"confidence\":0.85,\"reasoning\":\"Best match for python bugfix\",\"invariant_checks\":[{\"rule\":\"agent_available\",\"passed\":true,\"severity\":\"critical\"}, ...],\"inference_us\":42}"
    }]
  }
}
```

### report_outcome

Reports task execution results to update agent statistics.

**Request:**

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": {
    "name": "report_outcome",
    "arguments": {
      "task_id": "task-42",
      "agent_id": "claude_code@claude-sonnet-4-6",
      "status": "success",
      "duration_min": 12.5,
      "tokens_used": 1850,
      "cost_usd": 0.06,
      "files_changed": 2,
      "tests_passed": true
    }
  }
}
```

### get_agent_status

Queries agent capabilities, current load, and performance history.

**Request:**

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "tools/call",
  "params": {
    "name": "get_agent_status",
    "arguments": {
      "agent_id": "claude_code@claude-sonnet-4-6"
    }
  }
}
```

Omit `agent_id` to get status for all agents.

### get_metrics

Returns server-wide metrics: decision counts, fallback/reject rates, and latency statistics.

**Request:**

```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "method": "tools/call",
  "params": {
    "name": "get_metrics",
    "arguments": {}
  }
}
```

### get_budget_status

Returns budget overview: total spent, budget limit, remaining amount, and per-agent cost breakdown.

**Request:**

```json
{
  "jsonrpc": "2.0",
  "id": 5,
  "method": "tools/call",
  "params": {
    "name": "get_budget_status",
    "arguments": {}
  }
}
```

### report_benchmark

**`report_benchmark`** (R-06b M4, since v0.2.0): persists per-agent per-benchmark scores from Maestro's external benchmark runs into the `benchmark_runs` table for use by R-07 (eval-driven routing). Idempotent via `run_id` PRIMARY KEY. See the Maestro-side design doc for the cross-repo contract: https://github.com/andrei-shtanakov/Maestro/blob/master/docs/superpowers/specs/2026-05-23-r06b-m4-arbiter-wiring-design.md

**Request:**

```json
{
  "jsonrpc": "2.0",
  "id": 6,
  "method": "tools/call",
  "params": {
    "name": "report_benchmark",
    "arguments": {
      "payload_version": "1.0.0",
      "run_id": "run-abc123",
      "benchmark_id": "atp-python-bugfix-v1",
      "agent_id": "claude_code@claude-sonnet-4-6",
      "ts": "2026-05-23T10:00:00Z",
      "score": 0.87,
      "score_components": {"correctness": 0.9, "speed": 0.8},
      "duration_seconds": 42.5,
      "per_task": [],
      "per_task_total_count": 10,
      "per_task_truncated": false
    }
  }
}
```

**Response:** `{"status": "created" | "duplicate", "run_id": "<id>"}`. A duplicate `run_id` returns `status: "duplicate"` without inserting a second row (idempotent).

**Error classification:**
- Validation errors (bad fields, empty IDs, non-RFC3339 `ts`, unsupported `payload_version`) → JSON-RPC `-32602`
- DB I/O errors → JSON-RPC `-32000` (transient, Maestro will retry)

## Claude Desktop Integration

Add Arbiter to your Claude Desktop MCP configuration (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "arbiter": {
      "command": "/path/to/arbiter/target/release/arbiter-mcp",
      "args": [
        "--tree", "/path/to/arbiter/models/agent_policy_tree.json",
        "--config", "/path/to/arbiter/config/",
        "--db", "/path/to/arbiter/arbiter.db",
        "--log-level", "warn"
      ]
    }
  }
}
```

Once configured, Claude Desktop can call `route_task` to determine which agent should handle a coding task, `report_outcome` to feed back results, `get_agent_status` to inspect agent health, `get_metrics` for server-wide decision statistics, `get_budget_status` for cost tracking, and `report_benchmark` (R-06b M4, v0.2.0+) to persist benchmark scores from Maestro's ATP runs into the `benchmark_runs` table.

The server negotiates MCP `protocolVersion` `"1.1.0"` (updated from `"2024-11-05"` in v0.2.0).

## Orchestrator Integration

Use the Python client to integrate Arbiter into an agent orchestrator:

```python
import asyncio
from orchestrator.arbiter_client import ArbiterClient, ArbiterClientConfig

async def main():
    client = ArbiterClient(ArbiterClientConfig(
        binary_path="target/release/arbiter-mcp",
        tree_path="models/agent_policy_tree.json",
        config_dir="config/",
        log_level="warn",
    ))

    await client.start()

    # Route a task
    decision = await client.route_task("task-1", {
        "type": "feature",
        "language": "rust",
        "complexity": "moderate",
        "priority": "high",
        "scope": ["src/api/"],
        "description": "Add pagination to list endpoint",
    })

    print(f"Agent: {decision['chosen_agent']}")
    print(f"Confidence: {decision['confidence']}")
    print(f"Action: {decision['action']}")

    # Report outcome after execution
    await client.report_outcome(
        task_id="task-1",
        agent_id=decision["chosen_agent"],
        status="success",
        duration_min=15.0,
        cost_usd=0.08,
    )

    # Check agent status
    status = await client.get_agent_status("claude_code@claude-sonnet-4-6")
    print(f"Success rate: {status['success_rate']}")

    await client.stop()

asyncio.run(main())
```

The client manages the Arbiter subprocess lifecycle and handles automatic reconnection on broken pipes.

## Performance Characteristics

| Metric | Target | Notes |
|--------|--------|-------|
| Route throughput | > 10,000 decisions/sec | In-process, no I/O overhead |
| Route latency (e2e) | < 5 ms p99 | Over MCP stdio pipe |
| Report outcome latency | < 10 ms p99 | Including SQLite write |
| Memory usage | < 50 MB RSS | Steady state |
| SQLite size | < 10 MB | After 10K decisions |

The decision tree is loaded into memory at startup. Inference is a simple tree traversal over 22 float features — no external calls or heavy computation. SQLite writes use WAL mode for concurrent read performance.

Run benchmarks with the CLI:

```bash
cargo run --release --bin arbiter-cli -- bench
```

## Configuration Reference

### agents.toml

Defines the available coding agents and their capabilities. Each section header
is the agent's `agent_id` — the opaque routing key used everywhere
(`preferred_agent`, `chosen_agent`, `report_outcome.agent_id`, `benchmark_runs`).

Since the 2026-06-19 convention change, an id that carries a model dimension uses
`<harness>@<model>` so the model is a first-class routing key (e.g. one harness
running multiple models no longer collides). A harness-only id is still valid for
agents with no model axis — `aider` below has no suffix. The model id is verbatim
and may contain `.`/`-`/`:`; because `@` is not allowed in a bare TOML key, the
fused section headers are quoted.

```toml
["claude_code@claude-sonnet-4-6"]
display_name = "Claude Code"
supports_languages = ["python", "rust", "typescript"]
supports_types = ["feature", "bugfix", "refactor", "docs", "review", "research"]
max_concurrent = 2
cost_per_hour = 0.30
avg_duration_min = 18.0

["codex_cli@gpt-5.5"]
display_name = "Codex CLI"
supports_languages = ["typescript", "go", "python"]
supports_types = ["feature", "bugfix", "refactor", "test"]
max_concurrent = 3
cost_per_hour = 0.20
avg_duration_min = 12.0

[aider]
display_name = "Aider"
supports_languages = ["python", "javascript"]
supports_types = ["bugfix", "refactor", "test"]
max_concurrent = 5
cost_per_hour = 0.10
avg_duration_min = 8.0
```

| Field | Type | Description |
|-------|------|-------------|
| `display_name` | string | Human-readable name |
| `supports_languages` | string[] | Languages this agent handles |
| `supports_types` | string[] | Task types this agent handles |
| `max_concurrent` | integer | Maximum concurrent tasks |
| `cost_per_hour` | float | Hourly cost in USD |
| `avg_duration_min` | float | Average task duration in minutes |

### invariants.toml

Safety rule thresholds that govern routing decisions.

```toml
[budget]
threshold_usd = 10.0       # Max budget per session

[retries]
max_retries = 3             # Max retry attempts per task

[rate_limit]
calls_per_minute = 60       # Max routing calls per minute

[agent_health]
max_failures_24h = 5        # Failures before agent is flagged unhealthy

[concurrency]
max_total_concurrent = 5    # Max tasks running across all agents

[sla]
buffer_multiplier = 1.5     # SLA headroom multiplier
```

### Invariant Rules

All 10 invariant rules are evaluated on every routing decision. Critical failures trigger cascade fallback (up to 2 attempts, then reject). Warning failures are logged but don't block assignment.

| # | Rule | Severity | Description |
|---|------|----------|-------------|
| 1 | `agent_available` | Critical | Agent is active and accepting tasks |
| 2 | `scope_isolation` | Critical | No file-scope conflicts with running tasks |
| 3 | `branch_not_locked` | Critical | Target branch isn't locked by another agent |
| 4 | `concurrency_limit` | Critical | Agent hasn't hit max concurrent tasks |
| 5 | `budget_remaining` | Warning | Session budget not exhausted |
| 6 | `retry_limit` | Warning | Task hasn't exceeded retry limit |
| 7 | `rate_limit` | Warning | Routing calls within rate limit |
| 8 | `agent_health` | Warning | Agent failure rate within threshold |
| 9 | `task_compatible` | Warning | Agent supports task type and language |
| 10 | `sla_feasible` | Warning | Agent can complete within SLA |

## Development

```bash
# Build
cargo build --release

# Run all tests
cargo test

# Run specific crate tests
cargo test -p arbiter-core
cargo test -p arbiter-mcp

# Lint
cargo clippy --workspace -- -D warnings

# Format
cargo fmt --all

# Python tests (orchestrator)
uv run pytest orchestrator/tests/

# Regenerate decision tree
uv run python scripts/bootstrap_agent_tree.py
```

## License

MIT
