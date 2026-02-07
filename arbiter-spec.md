# Technical Specification: Arbiter — Policy Engine MCP Server (MVP)

**Version:** 1.1
**Date:** 2026-02-07
**Based on:** mcp-policy-engine-design.md v1.0
**Scope:** Phase 1 (MVP) — minimum viable product
**Repository:** `arbiter`

---

## 1. Goal and Scope

### 1.1 Goal

Create a Rust MCP server (`arbiter`) that receives a coding task description from the
Agent Orchestrator and returns a decision: which agent (Claude Code, Codex CLI,
Aider) should handle the task, with what parameters, and why.

### 1.2 What is included in the MVP

- MCP server with stdio transport (JSON-RPC 2.0)
- 2 required tools: `route_task`, `report_outcome`
- 1 informational tool: `get_agent_status`
- Decision Tree inference (reused from `arbiter-core`)
- 10 invariant rules with cascade fallback
- Agent registry with 3 agents
- SQLite persistence for stats and decision log
- Expert-policy bootstrap tree
- Python MCP client for the Orchestrator
- Smoke / integration / benchmark tests

### 1.3 What is NOT included in the MVP

- HTTP SSE transport (Phase 2)
- `evaluate_strategy` tool (Phase 3)
- Hot reload of the tree (Phase 2)
- Retraining pipeline (Phase 2, only logging in the MVP)
- Dashboard / TUI (Phase 4)
- Docker Compose deployment (Phase 4)
- ONNX backend (already exists in the PoC, but not critical for the MVP)

### 1.4 Definitions

| Term | Meaning |
|---|---|
| Arbiter | Rust MCP server that makes task routing decisions |
| Orchestrator | Python daemon that manages tasks, dependencies, git, and agent launching |
| Agent | External coding tool (Claude Code, Codex CLI, Aider) |
| Decision Tree | A trained sklearn/XGBoost model exported to JSON |
| Invariant | A safety rule checked before executing a decision |
| Expert Policy | An initial set of rules encoding expert knowledge |

---

## 2. Contract with the Existing AI-OS PoC

The AI-OS PoC (`ai-os-poc/`) contains a working Decision Tree inference, Invariant Layer,
and Model Registry in Rust. Arbiter reuses the core logic, adapting it for the context
of coding agents instead of ML models.

### 2.1 What is reused from `arbiter-core` directly

| Module | File | What we take | Adaptation |
|---|---|---|---|
| Decision Tree inference | `policy/decision_tree.rs` | sklearn JSON parsing, traversal, decision path | No changes — connected as a dependency |
| Policy Engine wrapper | `policy/engine.rs` | DT + ONNX fallback | Extended: adding multi-agent evaluation |
| Metrics | `metrics.rs` | Atomic counters, Prometheus | No changes |

### 2.2 What is adapted

| Module | File | Current state (AI-OS PoC) | What we change (Arbiter) |
|---|---|---|---|
| FeatureVector | `types.rs` | 26-dim for ML models (GPU, VRAM, queue) | New 22-dim vector for coding agents (task_type, language, complexity, agent_stats) |
| PolicyAction | `types.rs` | Enum: RouteToModel, ScaleUp, ScaleDown, Reject, Fallback | New enum: Assign(agent_id), Reject(reason), Fallback(agent_id, reason) |
| Invariant rules | `invariant/rules.rs` | 7 rules for ML infrastructure (GPU capacity, VRAM) | 10 new rules for agent orchestration (scope isolation, branch lock, concurrency) |
| Registry | `registry/lifecycle.rs` | 8-state FSM for ML models, in-memory | 4-state FSM for agents (inactive, active, busy, failed) + SQLite persistence |

### 2.3 What is written from scratch

| Component | Description |
|---|---|
| `arbiter-mcp` crate | MCP server binary, stdio transport, JSON-RPC handler |
| MCP protocol layer | `initialize`, `tools/list`, `tools/call` handlers |
| `route_task` tool | Feature extraction → DT inference → invariant check → response |
| `report_outcome` tool | Outcome recording, stats update |
| `get_agent_status` tool | Agent registry query |
| SQLite layer | Schema, migrations, CRUD for outcomes/stats/decisions |
| Feature builder | Raw task JSON → 22-dim numeric vector |
| Agent config loader | TOML parser for agents.toml, invariants.toml |
| Bootstrap tree trainer | Python script: expert rules → sklearn tree → JSON export |
| Python MCP client | `ArbiterClient` class for the Orchestrator |

### 2.4 Cargo workspace layout

```
arbiter/                             # Repository root
├── Cargo.toml                       # Workspace members: arbiter-core, arbiter-mcp,
│                                    #   arbiter-server, arbiter-cli
├── arbiter-core/                    # Shared library (evolved from aios-core)
│   └── src/
│       ├── types.rs                 # MODIFY: add AgentFeatureVector, AgentAction
│       ├── policy/
│       │   ├── decision_tree.rs     # REUSE as-is
│       │   ├── onnx.rs             # REUSE as-is
│       │   └── engine.rs           # MODIFY: add evaluate_for_agents()
│       ├── invariant/
│       │   └── rules.rs            # MODIFY: add 10 agent-specific rules
│       ├── registry/
│       │   └── lifecycle.rs        # MODIFY: add AgentState FSM
│       └── metrics.rs              # REUSE as-is
│
├── arbiter-mcp/                     # NEW — MCP Server binary
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs                  # Entry: parse args, init, run stdio loop
│       ├── server.rs                # MCP protocol: JSON-RPC dispatch
│       ├── tools/
│       │   ├── mod.rs
│       │   ├── route_task.rs
│       │   ├── report_outcome.rs
│       │   └── agent_status.rs
│       ├── features.rs              # Task JSON → FeatureVector
│       ├── agents.rs                # Agent registry + stats (backed by SQLite)
│       ├── db.rs                    # SQLite schema, migrations, queries
│       └── config.rs                # TOML config loader
│
├── arbiter-server/                  # HTTP API (Axum) — from AI-OS PoC, untouched in MVP
├── arbiter-cli/                     # CLI testing — from AI-OS PoC
├── config/                          # NEW
│   ├── agents.toml
│   └── invariants.toml
├── models/
│   ├── demo_tree.json              # Existing (AI-OS PoC demo)
│   └── agent_policy_tree.json      # NEW — bootstrap tree for agents
├── scripts/
│   ├── export_sklearn_tree.py      # Existing
│   └── bootstrap_agent_tree.py     # NEW — expert policy → tree
└── orchestrator/                    # NEW — Python client + integration
    ├── arbiter_client.py            # MCP client wrapper
    └── tests/
        └── test_arbiter_integration.py
```

---

## 3. Data Schema (SQLite)

### 3.1 Database File

Path: specified via `--db <path>` (default: `./arbiter.db`).

### 3.2 Tables

```sql
-- Schema versioning
CREATE TABLE IF NOT EXISTS schema_version (
    version     INTEGER PRIMARY KEY,
    applied_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Registered agents and their current state
CREATE TABLE IF NOT EXISTS agents (
    id                TEXT PRIMARY KEY,           -- "claude_code", "codex_cli", "aider"
    display_name      TEXT NOT NULL,
    state             TEXT NOT NULL DEFAULT 'active'
                      CHECK (state IN ('active', 'inactive', 'busy', 'failed')),
    max_concurrent    INTEGER NOT NULL DEFAULT 2,
    running_tasks     INTEGER NOT NULL DEFAULT 0,
    config_json       TEXT NOT NULL,              -- serialized agent config from TOML
    updated_at        TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Aggregated statistics (updated on each report_outcome)
CREATE TABLE IF NOT EXISTS agent_stats (
    agent_id          TEXT NOT NULL,
    task_type         TEXT NOT NULL,              -- "feature", "bugfix", etc.
    language          TEXT NOT NULL,              -- "python", "rust", etc.
    total_tasks       INTEGER NOT NULL DEFAULT 0,
    successful_tasks  INTEGER NOT NULL DEFAULT 0,
    failed_tasks      INTEGER NOT NULL DEFAULT 0,
    total_duration_min REAL NOT NULL DEFAULT 0.0,
    total_cost_usd    REAL NOT NULL DEFAULT 0.0,
    total_tokens      INTEGER NOT NULL DEFAULT 0,
    last_failure_at   TEXT,
    updated_at        TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (agent_id, task_type, language),
    FOREIGN KEY (agent_id) REFERENCES agents(id)
);

-- Each decision made (audit + data for retraining)
CREATE TABLE IF NOT EXISTS decisions (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id           TEXT NOT NULL,
    timestamp         TEXT NOT NULL DEFAULT (datetime('now')),
    -- Input
    task_json         TEXT NOT NULL,               -- full task JSON from the Orchestrator
    feature_vector    TEXT NOT NULL,               -- JSON array of 22 floats
    constraints_json  TEXT,                        -- constraints from the Orchestrator
    -- Decision
    chosen_agent      TEXT NOT NULL,
    action            TEXT NOT NULL CHECK (action IN ('assign', 'reject', 'fallback')),
    confidence        REAL NOT NULL,
    decision_path     TEXT NOT NULL,               -- JSON array of strings
    fallback_agent    TEXT,
    fallback_reason   TEXT,
    -- Invariant results
    invariants_json   TEXT NOT NULL,               -- JSON array of check results
    invariants_passed INTEGER NOT NULL,            -- count of passed
    invariants_failed INTEGER NOT NULL,            -- count of failed
    -- Timing
    inference_us      INTEGER NOT NULL             -- tree inference time in microseconds
);

-- Task execution results (feedback loop)
CREATE TABLE IF NOT EXISTS outcomes (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id           TEXT NOT NULL,
    decision_id       INTEGER NOT NULL,
    agent_id          TEXT NOT NULL,
    timestamp         TEXT NOT NULL DEFAULT (datetime('now')),
    -- Result
    status            TEXT NOT NULL CHECK (status IN ('success', 'failure', 'timeout', 'cancelled')),
    duration_min      REAL,
    tokens_used       INTEGER,
    cost_usd          REAL,
    exit_code         INTEGER,
    files_changed     INTEGER,
    tests_passed      INTEGER,                     -- 0 or 1
    validation_passed INTEGER,                     -- 0 or 1
    error_summary     TEXT,
    retry_count       INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (decision_id) REFERENCES decisions(id),
    FOREIGN KEY (agent_id) REFERENCES agents(id)
);

-- Indexes for fast queries
CREATE INDEX IF NOT EXISTS idx_decisions_task ON decisions(task_id);
CREATE INDEX IF NOT EXISTS idx_decisions_agent ON decisions(chosen_agent);
CREATE INDEX IF NOT EXISTS idx_decisions_ts ON decisions(timestamp);
CREATE INDEX IF NOT EXISTS idx_outcomes_task ON outcomes(task_id);
CREATE INDEX IF NOT EXISTS idx_outcomes_agent ON outcomes(agent_id);
CREATE INDEX IF NOT EXISTS idx_outcomes_status ON outcomes(status);
CREATE INDEX IF NOT EXISTS idx_outcomes_ts ON outcomes(timestamp);
```

### 3.3 Migrations

On startup, `arbiter` checks `schema_version`. If the table does not exist or the
version is less than the current one, it applies migrations sequentially. The MVP uses
a single migration (v1 — creation of all tables).

---

## 4. Component Specifications

### 4.1 MCP Server (`arbiter-mcp/src/server.rs`)

**Protocol:** JSON-RPC 2.0 over stdio (stdin/stdout), one JSON object per line.

**Lifecycle:**

1. The Orchestrator launches `arbiter` as a subprocess
2. The Orchestrator sends `initialize` → the server responds with capabilities
3. The Orchestrator sends `initialized` notification
4. Then — `tools/list` and `tools/call` as needed
5. On shutdown, the Orchestrator closes stdin → the server terminates gracefully

**Required MCP methods:**

| Method | Direction | Description |
|---|---|---|
| `initialize` | client → server | Handshake, exchange capabilities |
| `initialized` | client → server | Notification: handshake complete |
| `tools/list` | client → server | Return list of 3 tools with schemas |
| `tools/call` | client → server | Execute a tool |

**Server capabilities response:**

```json
{
  "capabilities": {
    "tools": {}
  },
  "serverInfo": {
    "name": "arbiter",
    "version": "0.1.0"
  }
}
```

**Acceptance criteria:**

- AC-4.1.1: The server starts in < 500ms, including tree loading and SQLite init
- AC-4.1.2: The server correctly handles `initialize` → `initialized` → `tools/list`
- AC-4.1.3: The server returns a JSON-RPC error (-32601) for unknown methods
- AC-4.1.4: The server returns a JSON-RPC error (-32602) for invalid params
- AC-4.1.5: The server terminates correctly on EOF on stdin (exit code 0)
- AC-4.1.6: All messages go to stderr (logs), never to stdout (protocol only)

### 4.2 Tool: `route_task` (`arbiter-mcp/src/tools/route_task.rs`)

**Input/Output:** see design document, section 2.1.

**Algorithm:**

```
1. Validate input JSON against schema
2. Load current agent states from registry
3. Filter agents by hard constraints:
   a. agent supports task_type
   b. agent supports language
   c. agent has available slots (running_tasks < max_concurrent)
   d. agent not in excluded_agents
4. For each candidate agent:
   a. Build 22-dim feature vector (task features + agent stats + system state)
   b. Run Decision Tree inference → score
5. Rank agents by score, select top
6. Run 10 invariant checks against selected agent
7. If critical invariant fails:
   a. Try next-best agent (up to max_fallback_attempts)
   b. If all fail → return action="reject"
8. Log decision to SQLite (decisions table)
9. Increment running_tasks for chosen agent
10. Return decision with full audit trail
```

**Acceptance criteria:**

- AC-4.2.1: With 3 active agents and empty history, `route_task` returns a decision in < 5ms
- AC-4.2.2: If preferred_agent is specified and available, it is selected (with a +0.1 confidence boost, all else being equal)
- AC-4.2.3: If all agents are excluded → action="reject", reasoning contains the reason
- AC-4.2.4: If the selected agent has a scope conflict → fallback to the next agent
- AC-4.2.5: The decision is written to SQLite with the full feature vector and decision path
- AC-4.2.6: The response always contains invariant_checks with results for all 10 rules
- AC-4.2.7: On invalid input JSON → JSON-RPC error with a description of the problem
- AC-4.2.8: Confidence is in [0.0, 1.0], decision_path is a non-empty array of strings
- AC-4.2.9: running_tasks is incremented on action="assign" or action="fallback"

### 4.3 Tool: `report_outcome` (`arbiter-mcp/src/tools/report_outcome.rs`)

**Input/Output:** see design document, section 2.2.

**Algorithm:**

```
1. Validate input JSON
2. Find corresponding decision in decisions table by task_id
3. Insert outcome into outcomes table
4. Update agent_stats:
   a. Increment total_tasks
   b. Increment successful_tasks or failed_tasks
   c. Add duration, cost, tokens to running totals
   d. If failure → update last_failure_at
5. Update agents.running_tasks (decrement by 1)
6. Check if agent should transition to 'failed' state:
   a. If failures_last_24h > threshold → state='failed'
7. Return updated stats + retrain_suggested flag
```

**Acceptance criteria:**

- AC-4.3.1: After report_outcome, agent_stats reflects the new data
- AC-4.3.2: If task_id is not found in decisions → warning in the response, but the outcome is still recorded
- AC-4.3.3: running_tasks is correctly decremented (never < 0)
- AC-4.3.4: With > 5 failures in 24h → retrain_suggested=true
- AC-4.3.5: All outcome fields are optional except status
- AC-4.3.6: A duplicate report_outcome for the same task_id → is recorded (idempotency via overwrite)

### 4.4 Tool: `get_agent_status` (`arbiter-mcp/src/tools/agent_status.rs`)

**Input/Output:** see design document, section 2.3.

**Acceptance criteria:**

- AC-4.4.1: Without parameters → returns all 3 agents
- AC-4.4.2: With agent_id → returns a single agent or error "agent not found"
- AC-4.4.3: Performance stats are calculated from agent_stats (not hardcoded)
- AC-4.4.4: by_language and by_type groupings are correct with empty history (empty objects)

### 4.5 Feature Builder (`arbiter-mcp/src/features.rs`)

Transformation of raw task JSON + agent stats → 22-dim float vector.

**Encoding:**

| Feature | Encoding | Range |
|---|---|---|
| task_type | ordinal: feature=0, bugfix=1, refactor=2, test=3, docs=4, review=5, research=6 | [0, 6] |
| language | ordinal: python=0, rust=1, typescript=2, go=3, mixed=4, other=5 | [0, 5] |
| complexity | ordinal: trivial=0, simple=1, moderate=2, complex=3, critical=4 | [0, 4] |
| priority | ordinal: low=0, normal=1, high=2, urgent=3 | [0, 3] |
| scope_size | raw count, capped at 100 | [0, 100] |
| estimated_tokens | raw / 1000, capped at 200 | [0.0, 200.0] |
| has_dependencies | boolean | {0, 1} |
| requires_internet | boolean | {0, 1} |
| sla_minutes | raw, capped at 480 (8h) | [0, 480] |
| agent_success_rate | float from agent_stats (for this type+lang combo) | [0.0, 1.0] |
| agent_available_slots | max_concurrent - running_tasks | [0, 10] |
| agent_running_tasks | raw | [0, 10] |
| agent_avg_duration_min | from agent_stats | [0.0, 480.0] |
| agent_avg_cost_usd | from agent_stats | [0.0, 100.0] |
| agent_recent_failures | count from outcomes, last 24h | [0, 50] |
| agent_supports_task_type | boolean | {0, 1} |
| agent_supports_language | boolean | {0, 1} |
| total_running_tasks | sum across all agents | [0, 20] |
| total_pending_tasks | from Orchestrator (passed in context) | [0, 100] |
| budget_remaining_usd | from constraints or default | [0.0, 1000.0] |
| time_of_day_hour | current hour UTC | [0, 23] |
| concurrent_scope_conflicts | count of running tasks with overlapping scope | [0, 10] |

**Default values** (when data is unavailable):

| Feature | Default | Reason |
|---|---|---|
| agent_success_rate | 0.5 | Neutral prior for new agents |
| agent_avg_duration_min | 15.0 | Conservative estimate |
| agent_avg_cost_usd | 0.10 | Mid-range estimate |
| estimated_tokens | 50.0 (= 50K tokens) | Typical task |
| budget_remaining_usd | config default (10.0) | From invariants.toml |
| scope_size | 1 | Minimum |

**Acceptance criteria:**

- AC-4.5.1: For one task + 3 agents → exactly 3 vectors of 22 elements are built
- AC-4.5.2: All values are within the specified ranges (capping is correct)
- AC-4.5.3: When optional fields are absent → default values are used
- AC-4.5.4: The feature builder works without SQLite (for unit tests, with mock stats)

### 4.6 Invariant Layer (`arbiter-core/src/invariant/rules.rs` — extension)

10 rules. Each rule is a function `(action, system_state) → InvariantResult`.

```rust
pub struct InvariantResult {
    pub rule: String,          // rule name
    pub severity: Severity,    // Critical | Warning
    pub passed: bool,
    pub detail: String,        // human-readable explanation
}

pub enum Severity {
    Critical,  // blocks action, triggers fallback
    Warning,   // logged, action proceeds
}
```

**Rules:**

| # | Rule ID | Severity | Input | Logic | Failure message |
|---|---|---|---|---|---|
| 1 | `agent_available` | Critical | agent state + slots | agent.state == "active" AND running < max_concurrent | "Agent {id} unavailable: state={state}, slots={available}" |
| 2 | `scope_isolation` | Critical | task scope + running tasks' scopes | intersection(task.scope, running_scopes) == ∅ | "Scope conflict: {files} shared with task {other_id}" |
| 3 | `branch_not_locked` | Critical | task branch + running tasks' branches | task.branch ∉ running_branches | "Branch {branch} locked by task {other_id}" |
| 4 | `concurrency_limit` | Critical | total running | total_running < max_total_concurrent | "Concurrency limit: {running}/{max}" |
| 5 | `budget_remaining` | Warning | estimated cost + remaining | estimated_cost ≤ budget_remaining | "Budget: need ${cost}, have ${remaining}" |
| 6 | `retry_limit` | Warning | task retry count | retry_count < max_retries | "Retry limit: attempt {n}/{max}" |
| 7 | `rate_limit` | Warning | API calls this minute | calls < rate_limit_per_minute | "Rate limit: {calls}/{limit} calls/min" |
| 8 | `agent_health` | Warning | recent failures | failures_24h < max_failures_per_agent_24h | "Agent {id}: {n} failures in 24h (limit: {max})" |
| 9 | `task_compatible` | Warning | agent capabilities | agent supports language AND task_type | "Agent {id} doesn't support {lang}/{type}" |
| 10 | `sla_feasible` | Warning | estimated duration x buffer | estimated_duration x sla_buffer ≤ sla_minutes | "SLA risk: est {est}min x {buf} > {sla}min" |

**Cascade fallback on Critical violation:**

```
1. The selected agent failed a critical check
2. Take the next agent by score from the DT ranking
3. Run invariants
4. If passed → assign with fallback_reason
5. If failed → repeat (up to max_fallback_attempts=2)
6. If all failed → action="reject"
```

**Acceptance criteria:**

- AC-4.6.1: Critical violation → action="fallback" or "reject", never "assign"
- AC-4.6.2: Warning violation → action="assign", warning in invariant_checks
- AC-4.6.3: Scope isolation checks intersection at the file/directory level
- AC-4.6.4: Branch lock checks exact match of the branch name
- AC-4.6.5: Invariant check completes in < 1ms (all rules combined)
- AC-4.6.6: All 10 rules are always executed and returned in the response (even when passed=true)

### 4.7 Agent Registry (`arbiter-mcp/src/agents.rs`)

**State FSM:**

```
  ┌──────────┐    register     ┌──────────┐
  │          │ ──────────────► │          │
  │ (absent) │                 │  active  │ ◄─── recover
  │          │                 │          │ ────►─┐
  └──────────┘                 └────┬─────┘       │
                                    │             │
                              spawn │        fail │
                                    ▼             │
                               ┌──────────┐       │
                               │   busy   │ ──────┘
                               │          │   (running=max OR
                               └────┬─────┘    health check fail)
                                    │
                                    │ all tasks complete
                                    ▼
                               back to active
```

In the MVP: state is managed via the running_tasks count:
- `running_tasks == 0` → active
- `0 < running_tasks < max_concurrent` → active (has capacity)
- `running_tasks == max_concurrent` → busy (no capacity)
- `failures_24h > threshold` → failed (manual recovery)

**Acceptance criteria:**

- AC-4.7.1: On startup — agents are loaded from agents.toml and written to SQLite if they do not already exist
- AC-4.7.2: Stats are queried by aggregation from the agent_stats table
- AC-4.7.3: running_tasks is incremented on route_task(assign) and decremented on report_outcome
- AC-4.7.4: running_tasks cannot go below 0

### 4.8 Config Loader (`arbiter-mcp/src/config.rs`)

**Files:**

- `config/agents.toml` — agent definitions (see design document, section 6.2)
- `config/invariants.toml` — rule thresholds (see design document, section 6.3)

**Acceptance criteria:**

- AC-4.8.1: Missing config file → the server does not start, stderr: "Config not found: {path}"
- AC-4.8.2: Invalid TOML → the server does not start, stderr: parse error with line number
- AC-4.8.3: Unknown fields in TOML → ignored with a warning in stderr
- AC-4.8.4: Missing required fields → error with a description of which field is missing

---

## 5. Error Handling

### 5.1 Error Categories

| Category | Examples | Behavior |
|---|---|---|
| **Startup fatal** | Tree fails to load, SQLite cannot open, config is invalid | Server does not start, exit code 1, stderr describes the problem |
| **Protocol error** | Invalid JSON-RPC, unknown method | JSON-RPC error response, server continues running |
| **Tool input error** | Invalid tool parameters | JSON-RPC error -32602 with a description of the problem |
| **Runtime recoverable** | SQLite write fails (disk full), agent stats inconsistent | Tool returns a result with a warning, stderr log |
| **Runtime degraded** | Tree cannot make a decision (all features are defaults) | Fallback to hardcoded round-robin, warning in the response |

### 5.2 Specific Scenarios

| Scenario | Behavior |
|---|---|
| Arbiter failed to start (crash on startup) | The Orchestrator catches the subprocess exit, logs stderr, switches to fallback: round-robin assignment without policy |
| Arbiter crashed during operation | The Orchestrator catches the broken pipe, restarts the server, pending route_task → retry after 1s |
| SQLite locked (concurrent access) | Retry with backoff (50ms, 100ms, 200ms), max 3 attempts. If all failed → the tool returns a result without writing to the DB, warning in the response |
| Tree failed to load, but config is OK | The server starts in degraded mode, route_task uses hardcoded rules (round-robin among capable agents), warning in every response |
| All agents state=failed | route_task returns action="reject", reasoning="All agents unhealthy" |
| Unknown task_type or language | Defaults are used (task_type=0, language=5="other"), warning in the response |
| report_outcome for an unknown task_id | The outcome is recorded, decision_id=NULL, warning "No matching decision found" |
| stdin EOF (Orchestrator shutdown) | The server flushes the SQLite WAL, closes cleanly, exit code 0 |

### 5.3 Orchestrator fallback mode

If Arbiter is unavailable, the Orchestrator switches to a built-in round-robin:

```python
class FallbackScheduler:
    """Used when Arbiter is unavailable"""
    AGENT_ORDER = ["claude_code", "codex_cli", "aider"]

    def __init__(self):
        self._index = 0

    def route(self, task: dict) -> str:
        agent = self.AGENT_ORDER[self._index % len(self.AGENT_ORDER)]
        self._index += 1
        return agent
```

---

## 6. Test Scenarios

### 6.1 Unit Tests (Rust, `cargo test`)

| ID | Component | Scenario | Expected |
|---|---|---|---|
| UT-01 | Feature builder | Full task JSON → 22-dim vector | All 22 values within correct ranges |
| UT-02 | Feature builder | Minimal task (only required fields) → vector | Default values for optional fields |
| UT-03 | Feature builder | task_type="unknown" → vector | task_type encoded as 6 (other), warning logged |
| UT-04 | Invariant: scope_isolation | Task scope=["src/main.rs"], running=["src/main.rs"] | passed=false, detail contains "src/main.rs" |
| UT-05 | Invariant: scope_isolation | Task scope=["src/lib.rs"], running=["src/main.rs"] | passed=true |
| UT-06 | Invariant: scope_isolation | Task scope=["src/"], running=["src/main.rs"] | passed=false (directory contains file) |
| UT-07 | Invariant: concurrency | 5 running, limit 5 | passed=false |
| UT-08 | Invariant: concurrency | 4 running, limit 5 | passed=true |
| UT-09 | Invariant: budget | cost=0.50, remaining=0.30 | passed=false, detail shows amounts |
| UT-10 | Invariant: budget | cost=0.50, remaining=1.00 | passed=true |
| UT-11 | Invariant: branch_lock | branch="feature/x", running has "feature/x" | passed=false |
| UT-12 | Invariant: agent_health | 6 failures in 24h, threshold=5 | passed=false |
| UT-13 | Registry | Increment running_tasks from 0 → 1 | state still "active" |
| UT-14 | Registry | Increment running_tasks to max_concurrent | state "busy" |
| UT-15 | Registry | Decrement running_tasks below 0 → clamped | running_tasks=0, no panic |
| UT-16 | DB | Insert decision + query by task_id | Record found, all fields match |
| UT-17 | DB | Insert outcome + verify agent_stats update | Stats reflect new outcome |
| UT-18 | DB | Concurrent writes (2 threads) | No corruption, WAL handles contention |
| UT-19 | Config | Valid agents.toml → parsed config | 3 agents with all fields |
| UT-20 | Config | Missing required field → error | Error message names the field |
| UT-21 | DT | Bootstrap tree + known input → deterministic output | Same input always produces same agent choice |
| UT-22 | DT | All agents filtered out → empty candidates | Graceful: returns reject action |

### 6.2 Integration Tests (Rust, `cargo test --test integration`)

| ID | Scenario | Description | Expected |
|---|---|---|---|
| IT-01 | Happy path | route_task → assign → report_outcome(success) | Decision logged, stats updated, success_rate reflects |
| IT-02 | Fallback | route_task, primary agent has scope conflict | Fallback agent assigned, fallback_reason populated |
| IT-03 | All rejected | route_task, all agents excluded | action="reject", all invariant_checks present |
| IT-04 | Cold start | route_task with zero history | Decision made using bootstrap tree defaults |
| IT-05 | Stats accumulation | 10x (route_task + report_outcome) | agent_stats correctly accumulated |
| IT-06 | Agent failure | 6x report_outcome(failure) for same agent in 24h | agent_health invariant fails, agent deprioritized |
| IT-07 | Concurrent routing | 3x route_task simultaneously (async) | No race conditions, running_tasks consistent |

### 6.3 MCP Protocol Tests (Python, `pytest`)

| ID | Scenario | Description | Expected |
|---|---|---|---|
| PT-01 | Handshake | initialize → initialized → tools/list | 3 tools returned with correct schemas |
| PT-02 | Route simple | tools/call route_task with minimal task | Valid response with decision |
| PT-03 | Route + Report | Full cycle: route → report success | Stats updated, second route reflects history |
| PT-04 | Invalid params | tools/call with missing required field | JSON-RPC error -32602 |
| PT-05 | Unknown tool | tools/call with name="nonexistent" | JSON-RPC error -32601 |
| PT-06 | Server crash recovery | Kill server mid-operation, restart | Orchestrator reconnects, state preserved in SQLite |
| PT-07 | Large batch | 100x route_task sequentially | All succeed, total time < 2s |

### 6.4 Benchmark Tests (Rust, `cargo run --bin arbiter-cli`)

| ID | Metric | Target | Measurement |
|---|---|---|---|
| BT-01 | route_task throughput | > 10,000 decisions/sec | 10K route_task calls (in-process, no MCP overhead) |
| BT-02 | route_task e2e latency | < 5ms p99 | Over MCP stdio, including serialization |
| BT-03 | report_outcome latency | < 10ms p99 | Including SQLite write |
| BT-04 | Memory usage | < 50MB RSS | With loaded tree + 10K decisions in DB |
| BT-05 | SQLite size after 10K decisions | < 10MB | With full audit trail |

---

## 7. Bootstrap Tree

### 7.1 Expert Rules

Minimal set of rules for cold start (extensible):

| # | Conditions | Agent | Rationale |
|---|---|---|---|
| 1 | complexity ∈ {complex, critical} AND language=rust | claude_code | Best Rust performance |
| 2 | complexity ∈ {complex, critical} AND language=python | claude_code | Best for complex tasks |
| 3 | type=docs OR type=review OR type=research | claude_code | Needs internet + tools |
| 4 | complexity ∈ {trivial, simple} AND type=bugfix | aider | Fast & cheap for simple fixes |
| 5 | complexity ∈ {trivial, simple} AND type=refactor | aider | Fast & cheap for refactors |
| 6 | language=typescript AND type=feature | codex_cli | Strong TS performance |
| 7 | language=go | codex_cli | Better Go support |
| 8 | complexity=moderate AND language=python | codex_cli | Good balance cost/quality |
| 9 | type=test AND complexity ≤ moderate | aider | Test writing is routine |
| 10 | DEFAULT (all others) | claude_code | Safest fallback |

### 7.2 Generation

`scripts/bootstrap_agent_tree.py`:

1. Expands 10 rules into ~500 training examples with variations
2. Adds noise to agent features (success_rate, duration) for robustness
3. Trains `DecisionTreeClassifier(max_depth=7, min_samples_leaf=10)`
4. Exports to the Arbiter JSON format (compatible with `arbiter-core::policy::decision_tree`)
5. Outputs accuracy, tree stats, confusion matrix

**Acceptance criteria:**

- AC-7.1: The bootstrap tree has accuracy > 95% on training data (expert rules)
- AC-7.2: The exported JSON loads in Rust without errors
- AC-7.3: Tree depth ≤ 7, node count ≤ 127

---

## 8. CLI Arguments

```
arbiter — Coding Agent Policy Engine (MCP Server)

USAGE:
    arbiter [OPTIONS]

OPTIONS:
    --tree <PATH>       Path to decision tree JSON [default: models/agent_policy_tree.json]
    --config <DIR>      Path to config directory [default: config/]
    --db <PATH>         Path to SQLite database [default: arbiter.db]
    --log-level <LEVEL> Log level: trace|debug|info|warn|error [default: info]
    --version           Print version
    --help              Print help
```

---

## 9. Dependencies

### 9.1 Rust crates (arbiter-mcp)

| Crate | Version | Purpose |
|---|---|---|
| `serde` + `serde_json` | 1.x | JSON serialization |
| `tokio` | 1.x | Async runtime (for stdin/stdout) |
| `rusqlite` | 0.31+ | SQLite (bundled feature) |
| `toml` | 0.8+ | Config parsing |
| `tracing` + `tracing-subscriber` | 0.1 / 0.3 | Structured logging (stderr) |
| `chrono` | 0.4+ | Timestamps |
| `arbiter-core` | workspace | DT inference, invariants, metrics |

We do not add an MCP SDK — we implement the protocol manually (it is simple: JSON-RPC 2.0
over stdio, 3 methods). This removes a heavy dependency and gives us full control.

### 9.2 Python (orchestrator/)

| Package | Purpose |
|---|---|
| `asyncio` | Subprocess management |
| `json` | MCP protocol |
| `sqlite3` | Orchestrator's own DB (not Arbiter DB) |
| `pytest` + `pytest-asyncio` | MCP protocol tests |

### 9.3 Python (scripts/)

| Package | Purpose |
|---|---|
| `scikit-learn` | Tree training |
| `numpy` | Data generation |
| `json` | Export |

---

## 10. Deployment

### 10.1 Claude Desktop / Claude Code Integration

```json
{
  "mcpServers": {
    "arbiter": {
      "command": "/path/to/arbiter",
      "args": [
        "--tree", "/path/to/models/agent_policy_tree.json",
        "--config", "/path/to/config/",
        "--db", "/path/to/arbiter.db"
      ]
    }
  }
}
```

### 10.2 Orchestrator Daemon Integration

```python
from arbiter_client import ArbiterClient

client = ArbiterClient(
    binary_path="/path/to/arbiter",
    tree_path="models/agent_policy_tree.json",
    config_dir="config/",
    db_path="arbiter.db"
)
await client.start()
decision = await client.route_task(task_id="task-1", task={...})
```

---

## 11. Definition of Done

The MVP is considered complete when:

- [ ] `cargo build --release` compiles without warnings
- [ ] `cargo test` — all 22 unit tests pass
- [ ] `cargo test --test integration` — all 7 integration tests pass
- [ ] `pytest orchestrator/tests/` — all 7 MCP protocol tests pass
- [ ] `arbiter --help` prints usage
- [ ] `arbiter` starts, accepts the MCP handshake, and responds to tools/list
- [ ] `route_task` returns a correct decision for each of the 10 expert rules
- [ ] `report_outcome` writes to SQLite and updates stats
- [ ] `get_agent_status` returns correct statistics after a series of route+report
- [ ] Benchmark: > 10K decisions/sec in-process, < 5ms e2e over stdio
- [ ] Bootstrap tree is generated, exported, and loaded in Rust
- [ ] README.md with quick start, architecture, and usage examples
- [ ] Code review: no unsafe, no unwrap() in production paths, all errors handled
