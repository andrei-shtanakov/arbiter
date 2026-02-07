# Tasks

> Tasks with priorities, dependencies, and traceability to requirements

## Legend

**Priority:**
- 🔴 P0 — Critical, blocks release
- 🟠 P1 — High, needed for full functionality
- 🟡 P2 — Medium, experience improvement
- 🟢 P3 — Low, nice to have

**Status:**
- ⬜ TODO
- 🔄 IN PROGRESS
- ✅ DONE
- ⏸️ BLOCKED

**Estimate:**
- Specified in days (d) or hours (h)
- Prefer ranges: 3-5d

---

## Definition of Done (for EVERY task)

> A task is NOT considered complete without fulfilling these items:

- [ ] **Unit tests** — coverage for new code
- [ ] **Tests pass** — `cargo test` / `pytest` pass
- [ ] **No warnings** — `cargo clippy -- -D warnings` clean
- [ ] **Formatted** — `cargo fmt --all`
- [ ] **No unsafe/unwrap** — in production paths

---

## Testing Tasks (required)

### TASK-100: Test Infrastructure Setup
🔴 P0 | 🔄 IN_PROGRESS | Est: 1d

**Description:**
Set up Cargo workspace, crate structure, and test infrastructure.
Create empty crates with correct dependencies.

**Checklist:**
- [ ] Cargo.toml workspace with members: arbiter-core, arbiter-mcp, arbiter-cli
- [ ] arbiter-core/Cargo.toml with workspace dependencies
- [ ] arbiter-mcp/Cargo.toml with workspace dependencies + rusqlite, toml
- [ ] arbiter-cli/Cargo.toml
- [ ] Minimal lib.rs / main.rs for compilation
- [ ] `cargo build` passes
- [ ] `cargo test` passes (empty tests)
- [ ] `cargo clippy -- -D warnings` clean

**Traces to:** [NFR-000]
**Depends on:** —
**Blocks:** [TASK-001], [TASK-002], [TASK-003], [TASK-004], [TASK-005], [TASK-006], [TASK-007], [TASK-008], [TASK-009]

---

## Milestone 1: MVP

### TASK-001: Core Types and Data Structures
🔴 P0 | ⬜ TODO | Est: 1d

**Description:**
Define all core data types in arbiter-core: AgentFeatureVector,
AgentAction, AgentState, InvariantResult, Severity, PredictionResult.

**Checklist:**
- [ ] `types.rs`: AgentAction enum (Assign, Reject, Fallback)
- [ ] `types.rs`: AgentState enum (Active, Inactive, Busy, Failed)
- [ ] `types.rs`: Severity enum (Critical, Warning)
- [ ] `types.rs`: InvariantResult struct
- [ ] `types.rs`: TaskInput, Constraints, RunningTask structs
- [ ] `types.rs`: PredictionResult struct (class, confidence, path)
- [ ] Serde Serialize/Deserialize for all types
- [ ] Unit tests for serialization round-trip

**Tests (Definition of Done):**
- [ ] Unit tests: all types serialize/deserialize correctly
- [ ] Unit tests: enum variants match expected strings

**Traces to:** [REQ-010], [REQ-050]
**Depends on:** [TASK-100]
**Blocks:** [TASK-004], [TASK-005], [TASK-006]

---

### TASK-002: MCP Server Shell and Config Loader
🔴 P0 | ⬜ TODO | Est: 2d

**Description:**
Implement MCP server skeleton: main.rs with CLI args, JSON-RPC 2.0 stdio loop,
initialize/initialized/tools-list handlers. Config loader for agents.toml and invariants.toml.

**Checklist:**
- [ ] `main.rs`: CLI argument parsing (--tree, --config, --db, --log-level)
- [ ] `main.rs`: tracing-subscriber init (stderr only)
- [ ] `server.rs`: JSON-RPC 2.0 message parsing (stdin line-by-line)
- [ ] `server.rs`: handle_initialize -> capabilities response
- [ ] `server.rs`: handle_initialized notification
- [ ] `server.rs`: handle_tools_list -> 3 tool schemas
- [ ] `server.rs`: handle_tools_call dispatch
- [ ] `server.rs`: error -32601 for unknown methods
- [ ] `server.rs`: error -32602 for invalid params
- [ ] `server.rs`: graceful shutdown on stdin EOF
- [ ] `config.rs`: parse agents.toml -> AgentConfig structs
- [ ] `config.rs`: parse invariants.toml -> InvariantConfig struct
- [ ] `config.rs`: error on missing config file
- [ ] `config.rs`: error on missing required fields

**Tests (Definition of Done):**
- [ ] Unit tests: config parsing valid TOML (UT-19)
- [ ] Unit tests: config parsing missing field error (UT-20)

**Traces to:** [REQ-001], [REQ-002], [REQ-070], [REQ-071]
**Depends on:** [TASK-100]
**Blocks:** [TASK-004], [TASK-007], [TASK-008], [TASK-010]

---

### TASK-003: Decision Tree Loader and Bootstrap Script
🔴 P0 | ⬜ TODO | Est: 2d

**Description:**
Implement Decision Tree loading from JSON in arbiter-core (adapted from AI-OS PoC).
Write Python script for bootstrap tree generation from expert rules.

**Checklist:**
- [ ] `policy/decision_tree.rs`: DecisionTree::from_json() sklearn JSON parsing
- [ ] `policy/decision_tree.rs`: DecisionTree::predict() tree traversal
- [ ] `policy/decision_tree.rs`: PredictionResult with decision_path
- [ ] `policy/engine.rs`: evaluate_for_agents() multi-agent evaluation
- [ ] `scripts/bootstrap_agent_tree.py`: 10 expert rules -> ~500 training examples
- [ ] `scripts/bootstrap_agent_tree.py`: noise injection for robustness
- [ ] `scripts/bootstrap_agent_tree.py`: DecisionTreeClassifier(max_depth=7)
- [ ] `scripts/bootstrap_agent_tree.py`: export to Arbiter JSON format
- [ ] `scripts/bootstrap_agent_tree.py`: accuracy report + confusion matrix
- [ ] `models/agent_policy_tree.json`: generated bootstrap tree

**Tests (Definition of Done):**
- [ ] Unit tests: bootstrap tree determinism (UT-21)
- [ ] Unit tests: all agents filtered -> reject (UT-22)
- [ ] Bootstrap tree accuracy > 95%
- [ ] Tree depth <= 7, nodes <= 127

**Traces to:** [REQ-060], [REQ-061]
**Depends on:** [TASK-100]
**Blocks:** [TASK-004]

---

### TASK-004: route_task Tool Implementation
🔴 P0 | ⬜ TODO | Est: 3d

**Description:**
Implement full route_task pipeline: validate input, build feature vectors,
run DT inference, check invariants, cascade fallback, log to SQLite.

**Checklist:**
- [ ] `tools/route_task.rs`: input validation against schema
- [ ] `tools/route_task.rs`: load agent states from registry
- [ ] `tools/route_task.rs`: filter by hard constraints (type, lang, slots, exclude)
- [ ] `tools/route_task.rs`: build feature vectors per candidate
- [ ] `tools/route_task.rs`: run DT inference and rank
- [ ] `tools/route_task.rs`: preferred_agent confidence boost (+0.1)
- [ ] `tools/route_task.rs`: run invariant checks
- [ ] `tools/route_task.rs`: cascade fallback on critical failure (max 2)
- [ ] `tools/route_task.rs`: log decision to SQLite
- [ ] `tools/route_task.rs`: increment running_tasks
- [ ] `tools/route_task.rs`: return full decision with audit trail

**Tests (Definition of Done):**
- [ ] Integration test: happy path route -> assign (IT-01)
- [ ] Integration test: fallback on scope conflict (IT-02)
- [ ] Integration test: all rejected (IT-03)
- [ ] Integration test: cold start (IT-04)

**Traces to:** [REQ-010], [REQ-011], [REQ-012], [REQ-013]
**Depends on:** [TASK-001], [TASK-002], [TASK-003], [TASK-005], [TASK-006], [TASK-009]
**Blocks:** [TASK-011]

---

### TASK-005: Feature Vector Builder
🔴 P0 | ⬜ TODO | Est: 1d

**Description:**
Implement conversion of task JSON + agent stats + system state into 22-dim float vector.
All encoding, capping, defaults per spec.

**Checklist:**
- [ ] `features.rs`: task_type ordinal encoding (7 values)
- [ ] `features.rs`: language ordinal encoding (6 values)
- [ ] `features.rs`: complexity, priority ordinal encoding
- [ ] `features.rs`: scope_size, estimated_tokens capping
- [ ] `features.rs`: boolean features (has_dependencies, requires_internet)
- [ ] `features.rs`: agent stats features (success_rate, duration, cost, failures)
- [ ] `features.rs`: agent capability features (supports_type, supports_lang)
- [ ] `features.rs`: system features (total_running, pending, budget, time, conflicts)
- [ ] `features.rs`: default values for missing optional fields
- [ ] `features.rs`: build_feature_vector() function

**Tests (Definition of Done):**
- [ ] Unit tests: full task -> 22-dim vector (UT-01)
- [ ] Unit tests: minimal task -> defaults (UT-02)
- [ ] Unit tests: unknown type encoding (UT-03)
- [ ] All values in documented ranges

**Traces to:** [REQ-040]
**Depends on:** [TASK-001]
**Blocks:** [TASK-004]

---

### TASK-006: Invariant Rules Implementation
🔴 P0 | ⬜ TODO | Est: 2d

**Description:**
Implement 10 invariant rules in arbiter-core. 4 Critical (block + fallback)
and 6 Warning (log + allow). Full scope, branch, concurrency, budget checks.

**Checklist:**
- [ ] `invariant/rules.rs`: agent_available (Critical)
- [ ] `invariant/rules.rs`: scope_isolation (Critical) — file/directory overlap check
- [ ] `invariant/rules.rs`: branch_not_locked (Critical) — exact match
- [ ] `invariant/rules.rs`: concurrency_limit (Critical) — total < max
- [ ] `invariant/rules.rs`: budget_remaining (Warning)
- [ ] `invariant/rules.rs`: retry_limit (Warning)
- [ ] `invariant/rules.rs`: rate_limit (Warning)
- [ ] `invariant/rules.rs`: agent_health (Warning)
- [ ] `invariant/rules.rs`: task_compatible (Warning)
- [ ] `invariant/rules.rs`: sla_feasible (Warning)
- [ ] `invariant/rules.rs`: check_all_invariants() returns all 10 results

**Tests (Definition of Done):**
- [ ] Unit tests: scope_isolation overlap (UT-04)
- [ ] Unit tests: scope_isolation no overlap (UT-05)
- [ ] Unit tests: scope_isolation directory contains file (UT-06)
- [ ] Unit tests: concurrency at limit (UT-07)
- [ ] Unit tests: concurrency below limit (UT-08)
- [ ] Unit tests: budget exceeded (UT-09)
- [ ] Unit tests: budget ok (UT-10)
- [ ] Unit tests: branch locked (UT-11)
- [ ] Unit tests: agent health failures (UT-12)
- [ ] Total check time < 1ms

**Traces to:** [REQ-050]
**Depends on:** [TASK-001]
**Blocks:** [TASK-004]

---

### TASK-007: report_outcome Tool Implementation
🔴 P0 | ⬜ TODO | Est: 1-2d

**Description:**
Implement report_outcome tool: validate input, find decision, insert outcome,
update agent_stats, decrement running_tasks, check health.

**Checklist:**
- [ ] `tools/report_outcome.rs`: input validation
- [ ] `tools/report_outcome.rs`: find decision by task_id
- [ ] `tools/report_outcome.rs`: insert outcome into outcomes table
- [ ] `tools/report_outcome.rs`: update agent_stats aggregates
- [ ] `tools/report_outcome.rs`: decrement running_tasks (clamp >= 0)
- [ ] `tools/report_outcome.rs`: check failures_24h > threshold -> retrain_suggested
- [ ] `tools/report_outcome.rs`: handle unknown task_id (decision_id=NULL, warning)
- [ ] `tools/report_outcome.rs`: return updated_stats + retrain_suggested

**Tests (Definition of Done):**
- [ ] Integration test: stats accumulation 10x (IT-05)
- [ ] Integration test: agent failure detection (IT-06)
- [ ] Unit tests: running_tasks clamp to 0 (UT-15)

**Traces to:** [REQ-020], [REQ-021], [REQ-022]
**Depends on:** [TASK-002], [TASK-009]
**Blocks:** [TASK-011]

---

### TASK-008: get_agent_status Tool Implementation
🟠 P1 | ⬜ TODO | Est: 1d

**Description:**
Implement get_agent_status tool: query agent registry, aggregate stats
from agent_stats table, return capabilities and performance.

**Checklist:**
- [ ] `tools/agent_status.rs`: handle empty params (return all agents)
- [ ] `tools/agent_status.rs`: handle agent_id param (single agent)
- [ ] `tools/agent_status.rs`: error for unknown agent_id
- [ ] `tools/agent_status.rs`: aggregate stats by_language, by_type
- [ ] `tools/agent_status.rs`: include capabilities from config
- [ ] `tools/agent_status.rs`: include current_load (running_tasks, slots)

**Tests (Definition of Done):**
- [ ] Unit tests: all agents returned
- [ ] Unit tests: single agent returned
- [ ] Unit tests: empty stats (fresh start)

**Traces to:** [REQ-030]
**Depends on:** [TASK-002], [TASK-009]
**Blocks:** —

---

### TASK-009: SQLite Persistence Layer
🔴 P0 | ⬜ TODO | Est: 2d

**Description:**
Implement SQLite layer: schema creation, migrations, CRUD operations for
decisions, outcomes, agent_stats. Retry with backoff on contention.

**Checklist:**
- [ ] `db.rs`: Database::open() with WAL mode
- [ ] `db.rs`: migrate() -> create schema v1 (5 tables, 8 indices)
- [ ] `db.rs`: insert_decision() -> returns id
- [ ] `db.rs`: insert_outcome()
- [ ] `db.rs`: update_agent_stats() from outcome
- [ ] `db.rs`: get_agent_stats() with aggregation
- [ ] `db.rs`: find_decision_by_task()
- [ ] `db.rs`: get_recent_failures(agent_id, hours)
- [ ] `db.rs`: increment/decrement running_tasks
- [ ] `db.rs`: retry with backoff (50ms, 100ms, 200ms) on lock
- [ ] `agents.rs`: AgentRegistry backed by SQLite
- [ ] `agents.rs`: load agents from config, upsert into SQLite

**Tests (Definition of Done):**
- [ ] Unit tests: insert/query decision (UT-16)
- [ ] Unit tests: insert outcome + stats update (UT-17)
- [ ] Unit tests: concurrent writes (UT-18)
- [ ] Unit tests: running_tasks increment/decrement (UT-13, UT-14)

**Traces to:** [REQ-080]
**Depends on:** [TASK-100]
**Blocks:** [TASK-004], [TASK-007], [TASK-008]

---

### TASK-010: Python MCP Client
🟠 P1 | ⬜ TODO | Est: 1-2d

**Description:**
Implement ArbiterClient for Python Orchestrator. Subprocess management,
JSON-RPC communication, reconnection logic.

**Checklist:**
- [ ] `orchestrator/arbiter_client.py`: ArbiterClient class
- [ ] `orchestrator/arbiter_client.py`: start() -> subprocess + handshake
- [ ] `orchestrator/arbiter_client.py`: stop() -> graceful shutdown
- [ ] `orchestrator/arbiter_client.py`: route_task() -> send + receive
- [ ] `orchestrator/arbiter_client.py`: report_outcome()
- [ ] `orchestrator/arbiter_client.py`: get_agent_status()
- [ ] `orchestrator/arbiter_client.py`: reconnection on broken pipe
- [ ] `orchestrator/arbiter_client.py`: FallbackScheduler class

**Tests (Definition of Done):**
- [ ] Protocol test: handshake (PT-01)
- [ ] Protocol test: route simple (PT-02)
- [ ] Protocol test: route + report cycle (PT-03)
- [ ] Protocol test: invalid params error (PT-04)
- [ ] Protocol test: unknown tool error (PT-05)
- [ ] Protocol test: server crash recovery (PT-06)
- [ ] Protocol test: large batch 100x (PT-07)

**Traces to:** [REQ-090]
**Depends on:** [TASK-002]
**Blocks:** —

---

### TASK-011: Integration Tests and Benchmarks
🟠 P1 | ⬜ TODO | Est: 2d

**Description:**
Write full integration tests (Rust) and benchmarks.
Verify end-to-end pipeline and performance targets.

**Checklist:**
- [ ] Integration test: happy path (IT-01)
- [ ] Integration test: fallback on scope conflict (IT-02)
- [ ] Integration test: all rejected (IT-03)
- [ ] Integration test: cold start (IT-04)
- [ ] Integration test: stats accumulation 10x (IT-05)
- [ ] Integration test: agent failure 6x (IT-06)
- [ ] Integration test: concurrent routing 3x (IT-07)
- [ ] Benchmark: route throughput > 10K/sec (BT-01)
- [ ] Benchmark: route e2e latency < 5ms p99 (BT-02)
- [ ] Benchmark: report latency < 10ms p99 (BT-03)
- [ ] Benchmark: memory < 50MB (BT-04)
- [ ] Benchmark: SQLite size < 10MB after 10K (BT-05)

**Tests (Definition of Done):**
- [ ] All 7 integration tests pass
- [ ] All 5 benchmarks meet targets

**Traces to:** [NFR-000], [NFR-001]
**Depends on:** [TASK-004], [TASK-007]
**Blocks:** —

---

## Milestone 2: Integration & Polish

### TASK-012: Configuration Files
🟠 P1 | ⬜ TODO | Est: 4h

**Description:**
Create config/agents.toml and config/invariants.toml with full definitions
of three agents and thresholds for all invariant rules.

**Checklist:**
- [ ] `config/agents.toml`: claude_code definition
- [ ] `config/agents.toml`: codex_cli definition
- [ ] `config/agents.toml`: aider definition
- [ ] `config/invariants.toml`: budget threshold
- [ ] `config/invariants.toml`: retries, rate_limit, agent_health
- [ ] `config/invariants.toml`: concurrency, sla

**Traces to:** [REQ-070], [REQ-071]
**Depends on:** [TASK-002]
**Blocks:** —

---

### TASK-013: Error Handling and Degraded Mode
🟡 P2 | ⬜ TODO | Est: 1d

**Description:**
Implement graceful degradation: fallback round-robin when tree is unavailable,
retry with backoff for SQLite, handling of unknown task_type/language.

**Checklist:**
- [ ] Degraded mode: round-robin when tree unavailable
- [ ] SQLite retry with backoff (50ms, 100ms, 200ms)
- [ ] Unknown task_type -> default + warning
- [ ] Unknown language -> default + warning
- [ ] All agents failed -> reject with reasoning

**Traces to:** [REQ-003]
**Depends on:** [TASK-004], [TASK-009]
**Blocks:** —

---

### TASK-014: README and Documentation
🟡 P2 | ⬜ TODO | Est: 4h

**Description:**
Write README.md with quick start, architecture diagram, usage examples,
and integration instructions for Claude Desktop and Orchestrator.

**Checklist:**
- [ ] Quick start guide (build, configure, run)
- [ ] Architecture overview diagram
- [ ] MCP tool usage examples
- [ ] Claude Desktop integration snippet
- [ ] Orchestrator integration snippet
- [ ] Performance characteristics
- [ ] Configuration reference

**Traces to:** [NFR-003]
**Depends on:** [TASK-004], [TASK-007], [TASK-008]
**Blocks:** —

---

## Dependency Graph

```
TASK-100 (Workspace Setup)
    |
    ├──> TASK-001 (Core Types)
    |        |
    |        ├──> TASK-005 (Feature Vector)
    |        |        |
    |        ├──> TASK-006 (Invariants)
    |        |        |
    |        └────────┴──> TASK-004 (route_task) ──> TASK-011 (Tests+Bench)
    |                           ^                         |
    |                           |                         └──> TASK-014 (README)
    ├──> TASK-002 (MCP Server + Config)
    |        |
    |        ├──> TASK-004 (route_task)
    |        ├──> TASK-007 (report_outcome) ──> TASK-011
    |        ├──> TASK-008 (agent_status)
    |        ├──> TASK-010 (Python Client)
    |        └──> TASK-012 (Config Files)
    |
    ├──> TASK-003 (DT Loader + Bootstrap)
    |        |
    |        └──> TASK-004 (route_task)
    |
    └──> TASK-009 (SQLite Layer)
             |
             ├──> TASK-004 (route_task)
             ├──> TASK-007 (report_outcome)
             ├──> TASK-008 (agent_status)
             └──> TASK-013 (Error Handling)
```

---

## Summary by Milestone

### MVP (Milestone 1)
| Priority | Count | Est. Total |
|----------|-------|------------|
| 🔴 P0 | 8 | ~14d |
| 🟠 P1 | 3 | ~5d |
| **Total** | **11** | **~19d** |

### Integration (Milestone 2)
| Priority | Count | Est. Total |
|----------|-------|------------|
| 🟠 P1 | 1 | ~0.5d |
| 🟡 P2 | 2 | ~1.5d |
| **Total** | **3** | **~2d** |

### Grand Total
| Priority | Count | Est. Total |
|----------|-------|------------|
| 🔴 P0 | 8 | ~14d |
| 🟠 P1 | 4 | ~5.5d |
| 🟡 P2 | 2 | ~1.5d |
| **Total** | **14** | **~21d** |

---

## Risk Register

| Risk | Impact | Probability | Mitigation |
|------|--------|-------------|------------|
| AI-OS PoC DT code incompatible | High | Medium | Write DT inference from scratch if needed (1-2d extra) |
| SQLite contention under load | Medium | Low | WAL mode + retry with backoff, tested in UT-18 |
| Bootstrap tree accuracy < 95% | Medium | Low | Tune max_depth, add more training examples |
| MCP protocol edge cases | Low | Medium | Extensive protocol tests (PT-01 to PT-07) |
| Performance targets not met | Medium | Low | Profile early, optimize hot path |

---

## Notes

- TASK-100 is the critical path — all other tasks depend on workspace setup
- Tasks 001, 002, 003, 009 can be worked in parallel after TASK-100
- TASK-004 (route_task) is the most complex task with many dependencies
- Python client (TASK-010) can be developed in parallel once MCP server shell exists
- Config files (TASK-012) can be created early as they define the test data
