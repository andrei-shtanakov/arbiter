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
🔴 P0 | ✅ DONE | Est: 1d

**Description:**
Set up Cargo workspace, crate structure, and test infrastructure.
Create empty crates with correct dependencies.

**Checklist:**
- [x] Cargo.toml workspace with members: arbiter-core, arbiter-mcp, arbiter-cli
- [x] arbiter-core/Cargo.toml with workspace dependencies
- [x] arbiter-mcp/Cargo.toml with workspace dependencies + rusqlite, toml
- [x] arbiter-cli/Cargo.toml
- [x] Minimal lib.rs / main.rs for compilation
- [x] `cargo build` passes
- [x] `cargo test` passes (empty tests)
- [x] `cargo clippy -- -D warnings` clean

**Traces to:** [NFR-000]
**Depends on:** —
**Blocks:** [TASK-001], [TASK-002], [TASK-003], [TASK-004], [TASK-005], [TASK-006], [TASK-007], [TASK-008], [TASK-009]

---

## Milestone 1: MVP

### TASK-001: Core Types and Data Structures
🔴 P0 | ✅ DONE | Est: 1d

**Description:**
Define all core data types in arbiter-core: AgentFeatureVector,
AgentAction, AgentState, InvariantResult, Severity, PredictionResult.

**Checklist:**
- [x] `types.rs`: AgentAction enum (Assign, Reject, Fallback)
- [x] `types.rs`: AgentState enum (Active, Inactive, Busy, Failed)
- [x] `types.rs`: Severity enum (Critical, Warning)
- [x] `types.rs`: InvariantResult struct
- [x] `types.rs`: TaskInput, Constraints, RunningTask structs
- [x] `types.rs`: PredictionResult struct (class, confidence, path)
- [x] Serde Serialize/Deserialize for all types
- [x] Unit tests for serialization round-trip

**Tests (Definition of Done):**
- [x] Unit tests: all types serialize/deserialize correctly
- [x] Unit tests: enum variants match expected strings

**Traces to:** [REQ-010], [REQ-050]
**Depends on:** [TASK-100]
**Blocks:** [TASK-004], [TASK-005], [TASK-006]

---

### TASK-002: MCP Server Shell and Config Loader
🔴 P0 | ✅ DONE | Est: 2d

**Description:**
Implement MCP server skeleton: main.rs with CLI args, JSON-RPC 2.0 stdio loop,
initialize/initialized/tools-list handlers. Config loader for agents.toml and invariants.toml.

**Checklist:**
- [x] `main.rs`: CLI argument parsing (--tree, --config, --db, --log-level)
- [x] `main.rs`: tracing-subscriber init (stderr only)
- [x] `server.rs`: JSON-RPC 2.0 message parsing (stdin line-by-line)
- [x] `server.rs`: handle_initialize -> capabilities response
- [x] `server.rs`: handle_initialized notification
- [x] `server.rs`: handle_tools_list -> 3 tool schemas
- [x] `server.rs`: handle_tools_call dispatch
- [x] `server.rs`: error -32601 for unknown methods
- [x] `server.rs`: error -32602 for invalid params
- [x] `server.rs`: graceful shutdown on stdin EOF
- [x] `config.rs`: parse agents.toml -> AgentConfig structs
- [x] `config.rs`: parse invariants.toml -> InvariantConfig struct
- [x] `config.rs`: error on missing config file
- [x] `config.rs`: error on missing required fields

**Tests (Definition of Done):**
- [x] Unit tests: config parsing valid TOML (UT-19)
- [x] Unit tests: config parsing missing field error (UT-20)

**Traces to:** [REQ-001], [REQ-002], [REQ-070], [REQ-071]
**Depends on:** [TASK-100]
**Blocks:** [TASK-004], [TASK-007], [TASK-008], [TASK-010]

---

### TASK-003: Decision Tree Loader and Bootstrap Script
🔴 P0 | ✅ DONE | Est: 2d

**Description:**
Implement Decision Tree loading from JSON in arbiter-core (adapted from AI-OS PoC).
Write Python script for bootstrap tree generation from expert rules.

**Checklist:**
- [x] `policy/decision_tree.rs`: DecisionTree::from_json() sklearn JSON parsing
- [x] `policy/decision_tree.rs`: DecisionTree::predict() tree traversal
- [x] `policy/decision_tree.rs`: PredictionResult with decision_path
- [x] `policy/engine.rs`: evaluate_for_agents() multi-agent evaluation
- [x] `scripts/bootstrap_agent_tree.py`: 10 expert rules -> ~500 training examples
- [x] `scripts/bootstrap_agent_tree.py`: noise injection for robustness
- [x] `scripts/bootstrap_agent_tree.py`: DecisionTreeClassifier(max_depth=7)
- [x] `scripts/bootstrap_agent_tree.py`: export to Arbiter JSON format
- [x] `scripts/bootstrap_agent_tree.py`: accuracy report + confusion matrix
- [x] `models/agent_policy_tree.json`: generated bootstrap tree

**Tests (Definition of Done):**
- [x] Unit tests: bootstrap tree determinism (UT-21)
- [x] Unit tests: all agents filtered -> reject (UT-22)
- [x] Bootstrap tree accuracy > 95%
- [x] Tree depth <= 7, nodes <= 127

**Traces to:** [REQ-060], [REQ-061]
**Depends on:** [TASK-100]
**Blocks:** [TASK-004]

---

### TASK-004: route_task Tool Implementation
🔴 P0 | ✅ DONE | Est: 3d

**Description:**
Implement full route_task pipeline: validate input, build feature vectors,
run DT inference, check invariants, cascade fallback, log to SQLite.

**Checklist:**
- [x] `tools/route_task.rs`: input validation against schema
- [x] `tools/route_task.rs`: load agent states from registry
- [x] `tools/route_task.rs`: filter by hard constraints (type, lang, slots, exclude)
- [x] `tools/route_task.rs`: build feature vectors per candidate
- [x] `tools/route_task.rs`: run DT inference and rank
- [x] `tools/route_task.rs`: preferred_agent confidence boost (+0.1)
- [x] `tools/route_task.rs`: run invariant checks
- [x] `tools/route_task.rs`: cascade fallback on critical failure (max 2)
- [x] `tools/route_task.rs`: log decision to SQLite
- [x] `tools/route_task.rs`: increment running_tasks
- [x] `tools/route_task.rs`: return full decision with audit trail

**Tests (Definition of Done):**
- [x] Integration test: happy path route -> assign (IT-01)
- [x] Integration test: fallback on scope conflict (IT-02)
- [x] Integration test: all rejected (IT-03)
- [x] Integration test: cold start (IT-04)

**Traces to:** [REQ-010], [REQ-011], [REQ-012], [REQ-013]
**Depends on:** [TASK-001], [TASK-002], [TASK-003], [TASK-005], [TASK-006], [TASK-009]
**Blocks:** [TASK-011]

---

### TASK-005: Feature Vector Builder
🔴 P0 | ✅ DONE | Est: 1d

**Description:**
Implement conversion of task JSON + agent stats + system state into 22-dim float vector.
All encoding, capping, defaults per spec.

**Checklist:**
- [x] `features.rs`: task_type ordinal encoding (7 values)
- [x] `features.rs`: language ordinal encoding (6 values)
- [x] `features.rs`: complexity, priority ordinal encoding
- [x] `features.rs`: scope_size, estimated_tokens capping
- [x] `features.rs`: boolean features (has_dependencies, requires_internet)
- [x] `features.rs`: agent stats features (success_rate, duration, cost, failures)
- [x] `features.rs`: agent capability features (supports_type, supports_lang)
- [x] `features.rs`: system features (total_running, pending, budget, time, conflicts)
- [x] `features.rs`: default values for missing optional fields
- [x] `features.rs`: build_feature_vector() function

**Tests (Definition of Done):**
- [x] Unit tests: full task -> 22-dim vector (UT-01)
- [x] Unit tests: minimal task -> defaults (UT-02)
- [x] Unit tests: unknown type encoding (UT-03)
- [x] All values in documented ranges

**Traces to:** [REQ-040]
**Depends on:** [TASK-001]
**Blocks:** [TASK-004]

---

### TASK-006: Invariant Rules Implementation
🔴 P0 | ✅ DONE | Est: 2d

**Description:**
Implement 10 invariant rules in arbiter-core. 4 Critical (block + fallback)
and 6 Warning (log + allow). Full scope, branch, concurrency, budget checks.

**Checklist:**
- [x] `invariant/rules.rs`: agent_available (Critical)
- [x] `invariant/rules.rs`: scope_isolation (Critical) — file/directory overlap check
- [x] `invariant/rules.rs`: branch_not_locked (Critical) — exact match
- [x] `invariant/rules.rs`: concurrency_limit (Critical) — total < max
- [x] `invariant/rules.rs`: budget_remaining (Warning)
- [x] `invariant/rules.rs`: retry_limit (Warning)
- [x] `invariant/rules.rs`: rate_limit (Warning)
- [x] `invariant/rules.rs`: agent_health (Warning)
- [x] `invariant/rules.rs`: task_compatible (Warning)
- [x] `invariant/rules.rs`: sla_feasible (Warning)
- [x] `invariant/rules.rs`: check_all_invariants() returns all 10 results

**Tests (Definition of Done):**
- [x] Unit tests: scope_isolation overlap (UT-04)
- [x] Unit tests: scope_isolation no overlap (UT-05)
- [x] Unit tests: scope_isolation directory contains file (UT-06)
- [x] Unit tests: concurrency at limit (UT-07)
- [x] Unit tests: concurrency below limit (UT-08)
- [x] Unit tests: budget exceeded (UT-09)
- [x] Unit tests: budget ok (UT-10)
- [x] Unit tests: branch locked (UT-11)
- [x] Unit tests: agent health failures (UT-12)
- [x] Total check time < 1ms

**Traces to:** [REQ-050]
**Depends on:** [TASK-001]
**Blocks:** [TASK-004]

---

### TASK-007: report_outcome Tool Implementation
🔴 P0 | ✅ DONE | Est: 1-2d

**Description:**
Implement report_outcome tool: validate input, find decision, insert outcome,
update agent_stats, decrement running_tasks, check health.

**Checklist:**
- [x] `tools/report_outcome.rs`: input validation
- [x] `tools/report_outcome.rs`: find decision by task_id
- [x] `tools/report_outcome.rs`: insert outcome into outcomes table
- [x] `tools/report_outcome.rs`: update agent_stats aggregates
- [x] `tools/report_outcome.rs`: decrement running_tasks (clamp >= 0)
- [x] `tools/report_outcome.rs`: check failures_24h > threshold -> retrain_suggested
- [x] `tools/report_outcome.rs`: handle unknown task_id (decision_id=NULL, warning)
- [x] `tools/report_outcome.rs`: return updated_stats + retrain_suggested

**Tests (Definition of Done):**
- [x] Integration test: stats accumulation 10x (IT-05)
- [x] Integration test: agent failure detection (IT-06)
- [x] Unit tests: running_tasks clamp to 0 (UT-15)

**Traces to:** [REQ-020], [REQ-021], [REQ-022]
**Depends on:** [TASK-002], [TASK-009]
**Blocks:** [TASK-011]

---

### TASK-008: get_agent_status Tool Implementation
🟠 P1 | ✅ DONE | Est: 1d

**Description:**
Implement get_agent_status tool: query agent registry, aggregate stats
from agent_stats table, return capabilities and performance.

**Checklist:**
- [x] `tools/agent_status.rs`: handle empty params (return all agents)
- [x] `tools/agent_status.rs`: handle agent_id param (single agent)
- [x] `tools/agent_status.rs`: error for unknown agent_id
- [x] `tools/agent_status.rs`: aggregate stats by_language, by_type
- [x] `tools/agent_status.rs`: include capabilities from config
- [x] `tools/agent_status.rs`: include current_load (running_tasks, slots)

**Tests (Definition of Done):**
- [x] Unit tests: all agents returned
- [x] Unit tests: single agent returned
- [x] Unit tests: empty stats (fresh start)

**Traces to:** [REQ-030]
**Depends on:** [TASK-002], [TASK-009]
**Blocks:** —

---

### TASK-009: SQLite Persistence Layer
🔴 P0 | ✅ DONE | Est: 2d

**Description:**
Implement SQLite layer: schema creation, migrations, CRUD operations for
decisions, outcomes, agent_stats. Retry with backoff on contention.

**Checklist:**
- [x] `db.rs`: Database::open() with WAL mode
- [x] `db.rs`: migrate() -> create schema v1 (5 tables, 8 indices)
- [x] `db.rs`: insert_decision() -> returns id
- [x] `db.rs`: insert_outcome()
- [x] `db.rs`: update_agent_stats() from outcome
- [x] `db.rs`: get_agent_stats() with aggregation
- [x] `db.rs`: find_decision_by_task()
- [x] `db.rs`: get_recent_failures(agent_id, hours)
- [x] `db.rs`: increment/decrement running_tasks
- [x] `db.rs`: retry with backoff (50ms, 100ms, 200ms) on lock
- [x] `agents.rs`: AgentRegistry backed by SQLite
- [x] `agents.rs`: load agents from config, upsert into SQLite

**Tests (Definition of Done):**
- [x] Unit tests: insert/query decision (UT-16)
- [x] Unit tests: insert outcome + stats update (UT-17)
- [x] Unit tests: concurrent writes (UT-18)
- [x] Unit tests: running_tasks increment/decrement (UT-13, UT-14)

**Traces to:** [REQ-080]
**Depends on:** [TASK-100]
**Blocks:** [TASK-004], [TASK-007], [TASK-008]

---

### TASK-010: Python MCP Client
🟠 P1 | ✅ DONE | Est: 1-2d

**Description:**
Implement ArbiterClient for Python Orchestrator. Subprocess management,
JSON-RPC communication, reconnection logic.

**Checklist:**
- [x] `orchestrator/arbiter_client.py`: ArbiterClient class
- [x] `orchestrator/arbiter_client.py`: start() -> subprocess + handshake
- [x] `orchestrator/arbiter_client.py`: stop() -> graceful shutdown
- [x] `orchestrator/arbiter_client.py`: route_task() -> send + receive
- [x] `orchestrator/arbiter_client.py`: report_outcome()
- [x] `orchestrator/arbiter_client.py`: get_agent_status()
- [x] `orchestrator/arbiter_client.py`: reconnection on broken pipe
- [x] `orchestrator/arbiter_client.py`: FallbackScheduler class

**Tests (Definition of Done):**
- [x] Protocol test: handshake (PT-01)
- [x] Protocol test: route simple (PT-02)
- [x] Protocol test: route + report cycle (PT-03)
- [x] Protocol test: invalid params error (PT-04)
- [x] Protocol test: unknown tool error (PT-05)
- [x] Protocol test: server crash recovery (PT-06)
- [x] Protocol test: large batch 100x (PT-07)

**Traces to:** [REQ-090]
**Depends on:** [TASK-002]
**Blocks:** —

---

### TASK-011: Integration Tests and Benchmarks
🟠 P1 | ✅ DONE | Est: 2d

**Description:**
Write full integration tests (Rust) and benchmarks.
Verify end-to-end pipeline and performance targets.

**Checklist:**
- [x] Integration test: happy path (IT-01)
- [x] Integration test: fallback on scope conflict (IT-02)
- [x] Integration test: all rejected (IT-03)
- [x] Integration test: cold start (IT-04)
- [x] Integration test: stats accumulation 10x (IT-05)
- [x] Integration test: agent failure 6x (IT-06)
- [x] Integration test: concurrent routing 3x (IT-07)
- [x] Benchmark: route throughput > 10K/sec (BT-01)
- [x] Benchmark: route e2e latency < 5ms p99 (BT-02)
- [x] Benchmark: report latency < 10ms p99 (BT-03)
- [x] Benchmark: memory < 50MB (BT-04)
- [x] Benchmark: SQLite size < 10MB after 10K (BT-05)

**Tests (Definition of Done):**
- [x] All 7 integration tests pass
- [x] All 5 benchmarks meet targets

**Traces to:** [NFR-000], [NFR-001]
**Depends on:** [TASK-004], [TASK-007]
**Blocks:** —

---

## Milestone 2: Integration & Polish

### TASK-012: Configuration Files
🟠 P1 | ✅ DONE | Est: 4h

**Description:**
Create config/agents.toml and config/invariants.toml with full definitions
of three agents and thresholds for all invariant rules.

**Checklist:**
- [x] `config/agents.toml`: claude_code definition
- [x] `config/agents.toml`: codex_cli definition
- [x] `config/agents.toml`: aider definition
- [x] `config/invariants.toml`: budget threshold
- [x] `config/invariants.toml`: retries, rate_limit, agent_health
- [x] `config/invariants.toml`: concurrency, sla

**Traces to:** [REQ-070], [REQ-071]
**Depends on:** [TASK-002]
**Blocks:** —

---

### TASK-013: Error Handling and Degraded Mode
🟡 P2 | ✅ DONE | Est: 1d

**Description:**
Implement graceful degradation: fallback round-robin when tree is unavailable,
retry with backoff for SQLite, handling of unknown task_type/language.

**Checklist:**
- [x] Degraded mode: round-robin when tree unavailable
- [x] SQLite retry with backoff (50ms, 100ms, 200ms)
- [x] Unknown task_type -> default + warning
- [x] Unknown language -> default + warning
- [x] All agents failed -> reject with reasoning

**Traces to:** [REQ-003]
**Depends on:** [TASK-004], [TASK-009]
**Blocks:** —

---

### TASK-014: README and Documentation
🟡 P2 | ✅ DONE | Est: 4h

**Description:**
Write README.md with quick start, architecture diagram, usage examples,
and integration instructions for Claude Desktop and Orchestrator.

**Checklist:**
- [x] Quick start guide (build, configure, run)
- [x] Architecture overview diagram
- [x] MCP tool usage examples
- [x] Claude Desktop integration snippet
- [x] Orchestrator integration snippet
- [x] Performance characteristics
- [x] Configuration reference

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
