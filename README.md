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
├── scripts/             # Python: tree generation utilities
└── orchestrator/        # Python MCP client library
```

**arbiter-core** is a pure logic library — no I/O, no database, no network. It owns types, decision tree inference, invariant checking, and feature vector construction.

**arbiter-mcp** is the main binary — it owns the MCP protocol, stdio I/O, SQLite persistence, config loading, and tool dispatch.

## MCP Tool Usage

Arbiter exposes three tools over the MCP protocol:

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
        "preferred_agent": "claude_code"
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
      "text": "{\"task_id\":\"task-42\",\"action\":\"assign\",\"chosen_agent\":\"claude_code\",\"confidence\":0.85,\"reasoning\":\"Best match for python bugfix\",\"invariant_checks\":[{\"rule\":\"agent_available\",\"passed\":true,\"severity\":\"critical\"}, ...],\"inference_us\":42}"
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
      "agent_id": "claude_code",
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
      "agent_id": "claude_code"
    }
  }
}
```

Omit `agent_id` to get status for all agents.

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

Once configured, Claude Desktop can call `route_task` to determine which agent should handle a coding task, `report_outcome` to feed back results, and `get_agent_status` to inspect agent health.

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
    status = await client.get_agent_status("claude_code")
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

Defines the available coding agents and their capabilities.

```toml
[claude_code]
display_name = "Claude Code"
supports_languages = ["python", "rust", "typescript"]
supports_types = ["feature", "bugfix", "refactor", "docs", "review"]
max_concurrent = 2
cost_per_hour = 0.30
avg_duration_min = 18.0

[codex_cli]
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
