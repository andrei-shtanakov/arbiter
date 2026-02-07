# Design Specification

> Architecture, API, data schemas, and key decisions for Arbiter

## 1. Architecture Overview

### 1.1 Principles

| Principle | Description |
|---------|----------|
| Core is pure logic | arbiter-core has no I/O, no SQLite, no network — only types, inference, invariants |
| MCP owns I/O | arbiter-mcp handles stdio, SQLite, config loading, tool dispatch |
| Hand-rolled protocol | No MCP SDK — simple JSON-RPC 2.0, full control, zero bloat |
| Fail-safe routing | Critical invariant violation triggers cascade fallback, never silent failure |
| Full audit trail | Every decision logged to SQLite with feature vector, decision path, invariant results |

### 1.2 High-Level Diagram

```
┌─────────────────────────────────────────────────────┐
│              Agent Orchestrator (Python)             │
│         Task Queue -> Dependency Resolver -> Spawner │
│                        |                            │
│                   MCP Client                        │
└────────────────────────┬────────────────────────────┘
                         | JSON-RPC 2.0 (stdio)
                         | One JSON object per line
┌────────────────────────┴────────────────────────────┐
│              Arbiter (Rust MCP Server)               │
│                                                      │
│  route_task -> Feature Builder -> DT Inference       │
│                                  -> Invariant Check  │
│                                  -> Agent Selection  │
│                                                      │
│  report_outcome -> Stats Update -> Feedback Store    │
│  get_agent_status -> Registry Query                  │
│                                                      │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐          │
│  │ arbiter  │  │  SQLite  │  │  Config  │          │
│  │  -core   │  │   (db)   │  │  (TOML)  │          │
│  └──────────┘  └──────────┘  └──────────┘          │
└──────────────────────────────────────────────────────┘
```

**Traces to:** [REQ-001], [REQ-010], [REQ-020], [REQ-030]

---

## 2. Components

### DESIGN-001: MCP Server (arbiter-mcp)

#### Description
Binary crate that implements the MCP server over stdio. Handles JSON-RPC 2.0
dispatch, lifecycle management, and tool execution. All I/O is owned here.

#### Interface
```rust
// arbiter-mcp/src/server.rs
pub struct McpServer {
    tree: DecisionTree,
    db: Database,
    registry: AgentRegistry,
    config: ArbiterConfig,
}

impl McpServer {
    pub async fn new(args: CliArgs) -> Result<Self>;
    pub async fn run(&mut self) -> Result<()>;  // Main stdio loop
}
```

#### MCP Methods

| Method | Handler | Description |
|--------|---------|-------------|
| `initialize` | `handle_initialize` | Handshake, return capabilities |
| `initialized` | `handle_initialized` | Ack notification |
| `tools/list` | `handle_tools_list` | Return 3 tool schemas |
| `tools/call` | `handle_tools_call` | Dispatch to route/report/status |

#### Capabilities Response
```json
{
  "capabilities": { "tools": {} },
  "serverInfo": { "name": "arbiter", "version": "0.1.0" }
}
```

**Traces to:** [REQ-001], [REQ-002]

---

### DESIGN-002: route_task Tool

#### Description
Primary routing tool. Takes task description, builds feature vectors for each
candidate agent, runs Decision Tree inference, applies invariant checks with
cascade fallback, and returns agent assignment.

#### Input Schema
```json
{
  "task_id": "string (required)",
  "task": {
    "type": "string: feature|bugfix|refactor|test|docs|review|research",
    "language": "string: python|rust|typescript|go|mixed|other",
    "complexity": "string: trivial|simple|moderate|complex|critical",
    "priority": "string: low|normal|high|urgent",
    "scope": ["string: file/directory paths affected"],
    "branch": "string: git branch name",
    "estimated_tokens": "integer",
    "has_dependencies": "boolean",
    "requires_internet": "boolean",
    "sla_minutes": "integer",
    "description": "string"
  },
  "constraints": {
    "preferred_agent": "string (optional)",
    "excluded_agents": ["string (optional)"],
    "budget_remaining_usd": "float (optional)",
    "total_pending_tasks": "integer (optional)",
    "running_tasks": [
      {
        "task_id": "string",
        "agent_id": "string",
        "scope": ["string"],
        "branch": "string"
      }
    ]
  }
}
```

#### Output Schema
```json
{
  "task_id": "string",
  "action": "assign|reject|fallback",
  "chosen_agent": "string",
  "confidence": "float [0, 1]",
  "reasoning": "string",
  "decision_path": ["string"],
  "fallback_agent": "string|null",
  "fallback_reason": "string|null",
  "invariant_checks": [
    {
      "rule": "string",
      "severity": "Critical|Warning",
      "passed": "boolean",
      "detail": "string"
    }
  ],
  "metadata": {
    "inference_us": "integer",
    "feature_vector": ["float x22"],
    "candidates_evaluated": "integer"
  }
}
```

#### Algorithm
```
1. Validate input JSON against schema
2. Load current agent states from registry
3. Filter agents by hard constraints:
   a. agent supports task_type
   b. agent supports language
   c. agent has available slots
   d. agent not in excluded_agents
4. For each candidate agent:
   a. Build 22-dim feature vector
   b. Run Decision Tree inference -> score
5. Rank agents by score, select top
6. If preferred_agent is top candidate, boost confidence +0.1
7. Run 10 invariant checks against selected agent
8. If critical invariant fails:
   a. Try next-best agent (up to 2 fallback attempts)
   b. If all fail -> action="reject"
9. Log decision to SQLite (decisions table)
10. Increment running_tasks for chosen agent
11. Return decision with full audit trail
```

**Traces to:** [REQ-010], [REQ-011], [REQ-012], [REQ-013]

---

### DESIGN-003: Feature Vector Builder

#### Description
Converts raw task JSON + agent stats + system state into a 22-dimensional
float vector suitable for Decision Tree inference.

#### 22-Dimensional Feature Vector

| Index | Feature | Encoding | Range | Default |
|-------|---------|----------|-------|---------|
| 0 | task_type | ordinal: feature=0, bugfix=1, refactor=2, test=3, docs=4, review=5, research=6 | [0, 6] | 0 |
| 1 | language | ordinal: python=0, rust=1, typescript=2, go=3, mixed=4, other=5 | [0, 5] | 5 |
| 2 | complexity | ordinal: trivial=0, simple=1, moderate=2, complex=3, critical=4 | [0, 4] | 1 |
| 3 | priority | ordinal: low=0, normal=1, high=2, urgent=3 | [0, 3] | 1 |
| 4 | scope_size | raw count, capped 100 | [0, 100] | 1 |
| 5 | estimated_tokens | raw / 1000, capped 200 | [0, 200] | 50 |
| 6 | has_dependencies | boolean | {0, 1} | 0 |
| 7 | requires_internet | boolean | {0, 1} | 0 |
| 8 | sla_minutes | raw, capped 480 | [0, 480] | 120 |
| 9 | agent_success_rate | float from agent_stats | [0, 1] | 0.5 |
| 10 | agent_available_slots | max - running | [0, 10] | 2 |
| 11 | agent_running_tasks | raw | [0, 10] | 0 |
| 12 | agent_avg_duration_min | from agent_stats | [0, 480] | 15.0 |
| 13 | agent_avg_cost_usd | from agent_stats | [0, 100] | 0.10 |
| 14 | agent_recent_failures | count, last 24h | [0, 50] | 0 |
| 15 | agent_supports_task_type | boolean | {0, 1} | 1 |
| 16 | agent_supports_language | boolean | {0, 1} | 1 |
| 17 | total_running_tasks | sum across all agents | [0, 20] | 0 |
| 18 | total_pending_tasks | from constraints | [0, 100] | 0 |
| 19 | budget_remaining_usd | from constraints or config | [0, 1000] | 10.0 |
| 20 | time_of_day_hour | current hour UTC | [0, 23] | current |
| 21 | concurrent_scope_conflicts | count overlapping scope | [0, 10] | 0 |

#### Interface
```rust
// arbiter-mcp/src/features.rs
pub fn build_feature_vector(
    task: &TaskInput,
    agent: &AgentInfo,
    system_state: &SystemState,
) -> [f64; 22];
```

**Traces to:** [REQ-040]

---

### DESIGN-004: Invariant Layer

#### Description
10 safety rules evaluated before every agent assignment. Critical violations
trigger cascade fallback. Warning violations are logged but don't block.

#### Interface
```rust
// arbiter-core/src/invariant/rules.rs
pub struct InvariantResult {
    pub rule: String,
    pub severity: Severity,
    pub passed: bool,
    pub detail: String,
}

pub enum Severity {
    Critical,
    Warning,
}

pub fn check_all_invariants(
    action: &AgentAction,
    state: &SystemState,
    config: &InvariantConfig,
) -> Vec<InvariantResult>;  // Always returns 10 results
```

#### Rules

| # | Rule ID | Severity | Logic |
|---|---------|----------|-------|
| 1 | `agent_available` | Critical | state==active AND running < max_concurrent |
| 2 | `scope_isolation` | Critical | intersection(task.scope, running_scopes) == empty |
| 3 | `branch_not_locked` | Critical | task.branch not in running_branches |
| 4 | `concurrency_limit` | Critical | total_running < max_total_concurrent (5) |
| 5 | `budget_remaining` | Warning | estimated_cost <= budget_remaining |
| 6 | `retry_limit` | Warning | retry_count < max_retries (3) |
| 7 | `rate_limit` | Warning | calls_per_minute < limit (60) |
| 8 | `agent_health` | Warning | failures_24h < threshold (5) |
| 9 | `task_compatible` | Warning | supports language AND task_type |
| 10 | `sla_feasible` | Warning | est_duration * buffer <= sla_minutes |

#### Cascade Fallback
```
1. Top agent fails critical check
2. Take next-best by DT score
3. Run invariants on next-best
4. If passed -> action="fallback" with fallback_reason
5. If failed -> repeat (up to max_fallback_attempts=2)
6. All failed -> action="reject"
```

**Traces to:** [REQ-050], [REQ-013]

---

### DESIGN-005: report_outcome Tool

#### Description
Feedback loop tool. Records task execution results, updates agent statistics,
checks for health issues.

#### Input Schema
```json
{
  "task_id": "string (required)",
  "agent_id": "string (required)",
  "status": "string: success|failure|timeout|cancelled (required)",
  "duration_min": "float (optional)",
  "tokens_used": "integer (optional)",
  "cost_usd": "float (optional)",
  "exit_code": "integer (optional)",
  "files_changed": "integer (optional)",
  "tests_passed": "boolean (optional)",
  "validation_passed": "boolean (optional)",
  "error_summary": "string (optional)",
  "retry_count": "integer (optional, default 0)"
}
```

#### Output Schema
```json
{
  "task_id": "string",
  "recorded": true,
  "updated_stats": {
    "agent_id": "string",
    "total_tasks": "integer",
    "success_rate": "float",
    "avg_duration_min": "float",
    "avg_cost_usd": "float"
  },
  "retrain_suggested": "boolean",
  "warnings": ["string"]
}
```

**Traces to:** [REQ-020], [REQ-021], [REQ-022]

---

### DESIGN-006: get_agent_status Tool

#### Description
Query tool for agent capabilities and performance metrics.

#### Input Schema
```json
{
  "agent_id": "string (optional, omit for all agents)"
}
```

#### Output Schema
```json
{
  "agents": [
    {
      "id": "string",
      "display_name": "string",
      "state": "active|inactive|busy|failed",
      "capabilities": {
        "languages": ["string"],
        "task_types": ["string"],
        "max_concurrent": "integer",
        "cost_per_hour": "float"
      },
      "current_load": {
        "running_tasks": "integer",
        "available_slots": "integer"
      },
      "performance": {
        "total_tasks": "integer",
        "success_rate": "float",
        "avg_duration_min": "float",
        "avg_cost_usd": "float",
        "by_language": { "python": { "tasks": 10, "success_rate": 0.9 } },
        "by_type": { "bugfix": { "tasks": 5, "success_rate": 1.0 } }
      }
    }
  ]
}
```

**Traces to:** [REQ-030]

---

### DESIGN-007: Decision Tree Engine

#### Description
Loads sklearn-exported Decision Tree from JSON and performs inference.
Reused from arbiter-core (AI-OS PoC) without modification.

#### Interface
```rust
// arbiter-core/src/policy/decision_tree.rs (reuse)
pub struct DecisionTree { ... }

impl DecisionTree {
    pub fn from_json(json: &str) -> Result<Self>;
    pub fn predict(&self, features: &[f64]) -> PredictionResult;
}

pub struct PredictionResult {
    pub class: usize,       // agent index
    pub confidence: f64,    // [0, 1]
    pub path: Vec<String>,  // decision path for audit
}

// arbiter-core/src/policy/engine.rs (extended)
pub fn evaluate_for_agents(
    tree: &DecisionTree,
    feature_vectors: &[(String, [f64; 22])],  // (agent_id, features)
) -> Vec<(String, PredictionResult)>;  // ranked by confidence
```

#### Bootstrap Tree Expert Rules

| # | Conditions | Agent | Rationale |
|---|-----------|-------|-----------|
| 1 | complex/critical + Rust | claude_code | Best Rust |
| 2 | complex/critical + Python | claude_code | Best complex |
| 3 | docs/review/research | claude_code | Needs internet |
| 4 | trivial/simple + bugfix | aider | Fast & cheap |
| 5 | trivial/simple + refactor | aider | Fast & cheap |
| 6 | TypeScript + feature | codex_cli | Strong TS |
| 7 | Go language | codex_cli | Better Go |
| 8 | moderate + Python | codex_cli | Cost/quality balance |
| 9 | test + simple/moderate | aider | Routine |
| 10 | DEFAULT | claude_code | Safe fallback |

**Traces to:** [REQ-060], [REQ-061]

---

### DESIGN-008: Configuration

#### Description
TOML-based configuration for agents and invariant thresholds.

#### agents.toml
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

#### invariants.toml
```toml
[budget]
threshold_usd = 10.0

[retries]
max_retries = 3

[rate_limit]
calls_per_minute = 60

[agent_health]
max_failures_24h = 5

[concurrency]
max_total_concurrent = 5

[sla]
buffer_multiplier = 1.5
```

**Traces to:** [REQ-070], [REQ-071]

---

### DESIGN-009: SQLite Persistence Layer

#### Description
SQLite database for audit trail, stats, and agent state persistence.

#### Schema (v1)

```sql
CREATE TABLE IF NOT EXISTS schema_version (
    version     INTEGER PRIMARY KEY,
    applied_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS agents (
    id                TEXT PRIMARY KEY,
    display_name      TEXT NOT NULL,
    state             TEXT NOT NULL DEFAULT 'active'
                      CHECK (state IN ('active','inactive','busy','failed')),
    max_concurrent    INTEGER NOT NULL DEFAULT 2,
    running_tasks     INTEGER NOT NULL DEFAULT 0,
    config_json       TEXT NOT NULL,
    updated_at        TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS agent_stats (
    agent_id          TEXT NOT NULL,
    task_type         TEXT NOT NULL,
    language          TEXT NOT NULL,
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

CREATE TABLE IF NOT EXISTS decisions (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id           TEXT NOT NULL,
    timestamp         TEXT NOT NULL DEFAULT (datetime('now')),
    task_json         TEXT NOT NULL,
    feature_vector    TEXT NOT NULL,
    constraints_json  TEXT,
    chosen_agent      TEXT NOT NULL,
    action            TEXT NOT NULL CHECK (action IN ('assign','reject','fallback')),
    confidence        REAL NOT NULL,
    decision_path     TEXT NOT NULL,
    fallback_agent    TEXT,
    fallback_reason   TEXT,
    invariants_json   TEXT NOT NULL,
    invariants_passed INTEGER NOT NULL,
    invariants_failed INTEGER NOT NULL,
    inference_us      INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS outcomes (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id           TEXT NOT NULL,
    decision_id       INTEGER NOT NULL,
    agent_id          TEXT NOT NULL,
    timestamp         TEXT NOT NULL DEFAULT (datetime('now')),
    status            TEXT NOT NULL CHECK (status IN ('success','failure','timeout','cancelled')),
    duration_min      REAL,
    tokens_used       INTEGER,
    cost_usd          REAL,
    exit_code         INTEGER,
    files_changed     INTEGER,
    tests_passed      INTEGER,
    validation_passed INTEGER,
    error_summary     TEXT,
    retry_count       INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (decision_id) REFERENCES decisions(id),
    FOREIGN KEY (agent_id) REFERENCES agents(id)
);

-- Indices
CREATE INDEX IF NOT EXISTS idx_decisions_task ON decisions(task_id);
CREATE INDEX IF NOT EXISTS idx_decisions_agent ON decisions(chosen_agent);
CREATE INDEX IF NOT EXISTS idx_decisions_ts ON decisions(timestamp);
CREATE INDEX IF NOT EXISTS idx_outcomes_task ON outcomes(task_id);
CREATE INDEX IF NOT EXISTS idx_outcomes_agent ON outcomes(agent_id);
CREATE INDEX IF NOT EXISTS idx_outcomes_status ON outcomes(status);
CREATE INDEX IF NOT EXISTS idx_outcomes_ts ON outcomes(timestamp);
```

#### Interface
```rust
// arbiter-mcp/src/db.rs
pub struct Database { ... }

impl Database {
    pub fn open(path: &Path) -> Result<Self>;
    pub fn migrate(&self) -> Result<()>;
    pub fn insert_decision(&self, decision: &Decision) -> Result<i64>;
    pub fn insert_outcome(&self, outcome: &Outcome) -> Result<()>;
    pub fn get_agent_stats(&self, agent_id: &str) -> Result<AgentStats>;
    pub fn update_agent_stats(&self, agent_id: &str, outcome: &Outcome) -> Result<()>;
    pub fn find_decision_by_task(&self, task_id: &str) -> Result<Option<Decision>>;
    pub fn get_recent_failures(&self, agent_id: &str, hours: u32) -> Result<u32>;
}
```

**Traces to:** [REQ-080]

---

### DESIGN-010: Python MCP Client

#### Description
ArbiterClient class for Python Orchestrator. Manages subprocess lifecycle
and JSON-RPC communication. Pure stdlib (asyncio, json, subprocess).

#### Interface
```python
# orchestrator/arbiter_client.py
class ArbiterClient:
    def __init__(
        self,
        binary_path: str,
        tree_path: str = "models/agent_policy_tree.json",
        config_dir: str = "config/",
        db_path: str = "arbiter.db",
    ): ...

    async def start(self) -> None: ...
    async def stop(self) -> None: ...

    async def route_task(
        self, task_id: str, task: dict, constraints: dict | None = None
    ) -> dict: ...

    async def report_outcome(
        self, task_id: str, agent_id: str, status: str, **kwargs
    ) -> dict: ...

    async def get_agent_status(
        self, agent_id: str | None = None
    ) -> dict: ...
```

**Traces to:** [REQ-090]

---

## 3. Data Schemas

### 3.1 Agent State FSM

```
  (absent) --register--> active <--recover-- failed
                           |                   ^
                           |--spawn-->  busy   |
                           |<--done---  |      |
                           |            |      |
                           +--health-fail------+
```

In MVP, state is derived from running_tasks count:
- `running_tasks == 0` -> active
- `0 < running_tasks < max_concurrent` -> active (has capacity)
- `running_tasks == max_concurrent` -> busy (no capacity)
- `failures_24h > threshold` -> failed (manual recovery)

---

## 4. Data Flow

### 4.1 Route Task Flow

```
Task JSON (from Orchestrator)
    |
    v
┌────────────────┐     ┌────────────────┐
│ Validate Input │────>│ Load Agent     │
│ (JSON schema)  │     │ States + Stats │
└────────────────┘     └───────┬────────┘
                               |
                               v
                    ┌──────────────────┐
                    │ Filter by Hard   │
                    │ Constraints      │
                    │ (type, lang,     │
                    │  slots, exclude) │
                    └────────┬─────────┘
                             |
                             v
                    ┌──────────────────┐
                    │ Build Feature    │
                    │ Vectors (22-dim) │
                    │ per candidate    │
                    └────────┬─────────┘
                             |
                             v
                    ┌──────────────────┐
                    │ DT Inference     │
                    │ -> Score + Rank  │
                    └────────┬─────────┘
                             |
                             v
                    ┌──────────────────┐
                    │ Invariant Checks │
                    │ (10 rules)       │
                    └────────┬─────────┘
                             |
                   ┌─────────┴─────────┐
                   |                   |
                   v                   v
            ┌──────────┐        ┌──────────┐
            │ All Pass │        │ Critical │
            │ -> Assign│        │ Failure  │
            └────┬─────┘        └────┬─────┘
                 |                   |
                 |                   v
                 |            ┌──────────┐
                 |            │ Cascade  │
                 |            │ Fallback │
                 |            │ (max 2)  │
                 |            └────┬─────┘
                 |                 |
                 v                 v
            ┌──────────────────────────┐
            │ Log to SQLite            │
            │ Update running_tasks     │
            │ Return Decision          │
            └──────────────────────────┘
```

---

## 5. Key Decisions (ADR)

### ADR-001: Hand-rolled MCP Protocol
**Status:** Accepted
**Date:** 2026-02-07

**Context:**
MCP protocol is JSON-RPC 2.0 over stdio. Available Rust MCP SDKs add significant
dependency weight and may not match our exact needs.

**Decision:**
Implement JSON-RPC 2.0 manually. The protocol surface is small: 4 methods
(initialize, initialized, tools/list, tools/call).

**Rationale:**
- Small protocol surface (4 methods, 3 tools)
- Full control over serialization and error handling
- Zero external dependencies for protocol layer
- Easy to test and debug

**Consequences:**
- (+) No SDK dependency, smaller binary
- (+) Full control over protocol behavior
- (+) Easy to add custom extensions
- (-) Must handle JSON-RPC edge cases manually

**Traces to:** [REQ-001], [REQ-002]

---

### ADR-002: SQLite for All Persistence
**Status:** Accepted
**Date:** 2026-02-07

**Context:**
Need to persist decisions, outcomes, agent stats, and schema versions.
Options: SQLite, file-based JSON, PostgreSQL, in-memory only.

**Decision:**
Use SQLite with `rusqlite` (bundled feature, no system dependency).

**Rationale:**
- Zero external setup (embedded)
- ACID transactions for data integrity
- SQL queries for aggregation (agent_stats)
- Full audit trail
- Survives restarts
- < 10MB for 10K decisions

**Consequences:**
- (+) No external database to manage
- (+) Portable, single-file database
- (+) SQL for complex queries
- (-) WAL contention under concurrent writes (mitigated with retry+backoff)

**Traces to:** [REQ-080]

---

### ADR-003: 22-Dimensional Feature Vector
**Status:** Accepted
**Date:** 2026-02-07

**Context:**
Decision Tree needs a fixed-size numeric input. Must encode task properties,
agent capabilities, and system state.

**Decision:**
22 features covering 3 dimensions: task (9), agent (8), system (5).

**Rationale:**
- Comprehensive enough for accurate routing
- Small enough for fast inference (< 1ms)
- Includes both static (capabilities) and dynamic (stats, load) features
- Compatible with sklearn export format

**Consequences:**
- (+) Rich signal for routing decisions
- (+) Fast inference
- (-) Adding new features requires retraining

**Traces to:** [REQ-040]

---

## 6. API Reference

### 6.1 CLI Arguments

```bash
arbiter -- Coding Agent Policy Engine (MCP Server)

USAGE:
    arbiter [OPTIONS]

OPTIONS:
    --tree <PATH>       Path to decision tree JSON
                        [default: models/agent_policy_tree.json]
    --config <DIR>      Path to config directory
                        [default: config/]
    --db <PATH>         Path to SQLite database
                        [default: arbiter.db]
    --log-level <LEVEL> Log level: trace|debug|info|warn|error
                        [default: info]
    --version           Print version
    --help              Print help
```

---

## 7. Directory Structure

```
arbiter/
├── Cargo.toml                    # Workspace root
├── arbiter-core/                 # Shared library (pure logic)
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── types.rs              # AgentFeatureVector, AgentAction, AgentState
│       ├── policy/
│       │   ├── mod.rs
│       │   ├── decision_tree.rs  # DT inference (from AI-OS PoC)
│       │   ├── onnx.rs           # ONNX backend (feature-gated)
│       │   └── engine.rs         # Multi-agent evaluation
│       ├── invariant/
│       │   ├── mod.rs
│       │   └── rules.rs          # 10 safety rules
│       ├── registry/
│       │   ├── mod.rs
│       │   └── lifecycle.rs      # Agent state FSM
│       └── metrics.rs            # Atomic counters
├── arbiter-mcp/                  # MCP Server binary
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs               # Entry point
│       ├── server.rs             # JSON-RPC dispatch
│       ├── tools/
│       │   ├── mod.rs
│       │   ├── route_task.rs
│       │   ├── report_outcome.rs
│       │   └── agent_status.rs
│       ├── features.rs           # Feature vector builder
│       ├── agents.rs             # Agent registry
│       ├── db.rs                 # SQLite layer
│       └── config.rs             # TOML config loader
├── arbiter-server/               # HTTP API (Axum) — not MVP
├── arbiter-cli/                  # CLI for tests & benchmarks
├── config/
│   ├── agents.toml
│   └── invariants.toml
├── models/
│   └── agent_policy_tree.json    # Bootstrap decision tree
├── scripts/
│   ├── export_sklearn_tree.py
│   └── bootstrap_agent_tree.py
└── orchestrator/
    ├── arbiter_client.py
    └── tests/
        └── test_arbiter_integration.py
```

---

## 8. Dependencies

### 8.1 Rust Workspace

```toml
[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
tracing-subscriber = "0.3"
anyhow = "1"
thiserror = "1"
rusqlite = { version = "0.31", features = ["bundled"] }
toml = "0.8"
chrono = "0.4"
```

### 8.2 Python (orchestrator)

- asyncio (stdlib)
- json (stdlib)
- subprocess (stdlib)
- pytest + pytest-asyncio (dev)

### 8.3 Python (scripts)

- scikit-learn
- numpy
