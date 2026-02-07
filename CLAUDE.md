# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with the Arbiter project.

## Project Overview

**Name:** Arbiter — Coding Agent Policy Engine
**Goal:** MCP server that decides which coding agent (Claude Code, Codex CLI, Aider) should handle a given task, based on Decision Tree inference, safety invariants, and historical performance data.
**Language:** Rust (core + MCP server), Python (orchestrator client, bootstrap scripts)
**Spec:** See `arbiter-spec.md` for the full technical specification.

---

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

Decision flow: `task JSON → 22-dim feature vector → Decision Tree → ranked agents → 10 invariant checks → assign/fallback/reject`

---

## Project Structure

```
arbiter/
├── Cargo.toml                    # Workspace: arbiter-core, arbiter-mcp, arbiter-server, arbiter-cli
├── arbiter-core/                 # Shared library (DT inference, invariants, types, metrics)
│   └── src/
│       ├── types.rs              # AgentFeatureVector (22-dim), AgentAction, AgentState
│       ├── policy/
│       │   ├── decision_tree.rs  # Native DT inference (sklearn JSON → tree traversal)
│       │   ├── onnx.rs           # ONNX backend (feature-gated)
│       │   └── engine.rs         # Policy Engine: evaluate_for_agents()
│       ├── invariant/
│       │   └── rules.rs          # 10 safety rules, Critical/Warning severity
│       ├── registry/
│       │   └── lifecycle.rs      # AgentState FSM (active/inactive/busy/failed)
│       └── metrics.rs            # Atomic counters, Prometheus
├── arbiter-mcp/                  # MCP Server binary (the main deliverable)
│   └── src/
│       ├── main.rs               # Entry: args, init tree + SQLite + config, stdio loop
│       ├── server.rs             # JSON-RPC 2.0 dispatch (initialize, tools/list, tools/call)
│       ├── tools/
│       │   ├── route_task.rs     # Primary: task → agent decision
│       │   ├── report_outcome.rs # Feedback: task result → stats update
│       │   └── agent_status.rs   # Query: agent capabilities + performance
│       ├── features.rs           # Task JSON + agent stats → 22-dim float vector
│       ├── agents.rs             # Agent registry (TOML config + SQLite stats)
│       ├── db.rs                 # SQLite schema, migrations, queries
│       └── config.rs             # TOML config loader (agents.toml, invariants.toml)
├── arbiter-server/               # HTTP API (Axum) — legacy from AI-OS PoC, not MVP priority
├── arbiter-cli/                  # CLI for smoke tests and benchmarks
├── config/
│   ├── agents.toml               # Agent definitions (capabilities, costs, concurrency)
│   └── invariants.toml           # Rule thresholds (budget, retries, rate limits)
├── models/
│   └── agent_policy_tree.json    # Bootstrap decision tree (sklearn export)
├── scripts/
│   ├── export_sklearn_tree.py    # Generic sklearn → Arbiter JSON converter
│   └── bootstrap_agent_tree.py   # Expert rules → training data → tree → JSON
└── orchestrator/                 # Python MCP client
    ├── arbiter_client.py         # ArbiterClient class (subprocess + JSON-RPC)
    └── tests/
        └── test_arbiter_integration.py
```

---

## Rust Development

### Build & Test Commands

```bash
# Build entire workspace
cargo build --release

# Run all unit tests
cargo test

# Run integration tests only
cargo test --test integration

# Run specific crate tests
cargo test -p arbiter-core
cargo test -p arbiter-mcp

# Check without building
cargo check --workspace

# Lint
cargo clippy --workspace -- -D warnings

# Format
cargo fmt --all

# Run MCP server
cargo run --release --bin arbiter-mcp -- --help

# Run benchmarks via CLI
cargo run --release --bin arbiter-cli -- bench
```

### Coding Standards

- **No `unsafe`** in production code
- **No `unwrap()`** in production paths — use `?` operator, `anyhow::Result`, or explicit error handling
- **All public functions** must have doc comments
- **Error types:** use `thiserror` for library errors, `anyhow` for binary entry points
- **Logging:** `tracing` crate, all output to stderr (stdout is MCP protocol only)
- **Async runtime:** `tokio` (for stdio I/O in arbiter-mcp)
- **Serialization:** `serde` + `serde_json` everywhere
- **SQLite:** `rusqlite` with `bundled` feature (no system dependency)
- **Config:** `toml` crate for agents.toml / invariants.toml
- **Tests:** `#[cfg(test)]` modules in each file + integration tests in `tests/`

### Workspace Dependencies

Shared dependencies should be declared in the workspace `Cargo.toml` and inherited by crates:

```toml
[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
anyhow = "1"
```

### Key Architectural Rules

1. **arbiter-core is a library** — no I/O, no SQLite, no network. Pure logic: types, DT inference, invariant checks, feature vector construction
2. **arbiter-mcp is a binary** — owns I/O (stdio, SQLite), config loading, MCP protocol, tool dispatch. Depends on arbiter-core
3. **MCP protocol is hand-rolled** — no MCP SDK dependency. Simple JSON-RPC 2.0 over stdin/stdout, one JSON object per line
4. **Feature vector is 22 floats** — see `arbiter-spec.md` section 4.5 for exact encoding
5. **Invariant rules return all 10 results always** — even when all pass
6. **Critical invariant failure → cascade fallback** — try next agent, up to 2 attempts, then reject
7. **SQLite stores everything** — decisions, outcomes, agent stats. Schema in `arbiter-spec.md` section 3

---

## Python Development

### Package Management

**CRITICAL: Use `uv` only, never pip**

```bash
# Install dependency
uv add package-name

# Dev dependency
uv add --dev package-name

# Run script
uv run python scripts/bootstrap_agent_tree.py

# Run tests
uv run pytest orchestrator/tests/

# FORBIDDEN: pip install, uv pip install
```

### Python Components

1. **`scripts/bootstrap_agent_tree.py`** — generates bootstrap decision tree from expert rules
   - Dependencies: scikit-learn, numpy
   - Output: `models/agent_policy_tree.json`

2. **`scripts/export_sklearn_tree.py`** — generic sklearn tree → Arbiter JSON converter
   - Already exists from AI-OS PoC

3. **`orchestrator/arbiter_client.py`** — MCP client for Python Orchestrator
   - Pure stdlib: asyncio, json, subprocess
   - No external dependencies

4. **`orchestrator/tests/test_arbiter_integration.py`** — end-to-end MCP protocol tests
   - Dependencies: pytest, pytest-asyncio

### Python Coding Standards

- Type hints required for all functions
- Docstrings for all public classes and methods
- Line length: 88 chars (black default)
- Formatting: `ruff format`
- Linting: `ruff check`

---

## MCP Protocol Reference

The server implements JSON-RPC 2.0 over stdio. Three tools are exposed:

### route_task
Primary tool. Takes a task description, returns agent assignment with confidence, decision path, and invariant check results.

### report_outcome
Feedback loop. Takes task execution results, updates agent stats, returns updated statistics.

### get_agent_status
Query tool. Returns agent capabilities, current load, and performance history.

See `arbiter-spec.md` sections 4.2–4.4 for full input/output schemas.

---

## Testing Strategy

| Layer | Tool | Location | Count |
|---|---|---|---|
| Unit (Rust) | `cargo test` | `arbiter-core/src/`, `arbiter-mcp/src/` | 22 tests |
| Integration (Rust) | `cargo test --test integration` | `arbiter-mcp/tests/` | 7 tests |
| MCP Protocol (Python) | `pytest` | `orchestrator/tests/` | 7 tests |
| Benchmarks (Rust) | `cargo run --bin arbiter-cli` | `arbiter-cli/src/` | 5 benchmarks |

### Performance Targets

- route_task throughput: > 10,000 decisions/sec (in-process)
- route_task e2e latency: < 5ms p99 (over MCP stdio)
- report_outcome latency: < 10ms p99 (including SQLite write)
- Memory usage: < 50MB RSS
- SQLite size after 10K decisions: < 10MB

---

## Common Tasks

### Add a new coding agent

1. Add agent definition to `config/agents.toml`
2. Add expert rules to `scripts/bootstrap_agent_tree.py`
3. Regenerate tree: `uv run python scripts/bootstrap_agent_tree.py`
4. Update capability mappings in `arbiter-mcp/src/features.rs` (task_type/language ordinals)
5. Run tests: `cargo test && uv run pytest orchestrator/tests/`

### Add a new invariant rule

1. Implement rule function in `arbiter-core/src/invariant/rules.rs`
2. Add rule to the invariant checker pipeline
3. Add threshold config to `config/invariants.toml`
4. Add unit tests (at least: pass case + fail case)
5. Update `arbiter-spec.md` invariant table

### Retrain / update the decision tree

1. Ensure outcomes are logged in `arbiter.db`
2. Run: `uv run python scripts/bootstrap_agent_tree.py --from-db arbiter.db --output models/agent_policy_tree.json`
3. Restart arbiter (or send SIGHUP when hot-reload is implemented)

### Debug a routing decision

1. Check `arbiter.db` → `decisions` table for the task_id
2. Look at `feature_vector` (22 floats) and `decision_path` (tree traversal)
3. Check `invariants_json` for any failed rules
4. Use `arbiter-cli tree` to visualize the decision tree structure

---

## Important Files to Read First

When onboarding or starting a new task, read these in order:

1. `CLAUDE.md` (this file) — project overview and conventions
2. `arbiter-spec.md` — full technical specification with acceptance criteria
3. `config/agents.toml` — agent definitions
4. `config/invariants.toml` — safety rule thresholds
5. `arbiter-core/src/types.rs` — core data types
6. `arbiter-mcp/src/server.rs` — MCP protocol handler
