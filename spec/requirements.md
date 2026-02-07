# Requirements Specification

> Arbiter — Coding Agent Policy Engine (MCP Server)

## 1. Context and Goals

### 1.1 Problem

When orchestrating multiple coding agents (Claude Code, Codex CLI, Aider), an
intelligent component is needed to decide which agent should handle a given task.
Without a policy engine, the orchestrator is forced to use naive round-robin,
leading to suboptimal task distribution, safety violations (scope and branch
conflicts), and budget overruns.

### 1.2 Project Goals

| ID | Goal | Success Metric |
|----|------|----------------|
| G-1 | Intelligent task routing | Accuracy > 95% on expert rules, < 5ms latency |
| G-2 | Safe agent orchestration | 10 invariant rules, 0 scope/branch conflicts |
| G-3 | Audit and feedback | 100% of decisions logged to SQLite, feedback loop |

### 1.3 Stakeholders

| Role | Interests | Influence |
|------|----------|---------|
| Agent Orchestrator (Python daemon) | Receives routing decisions, sends feedback | High |
| DevOps / Operator | Monitors agent operations, configures policy | Medium |
| Coding Agents (Claude Code, Codex, Aider) | Receive tasks, report results | Low |

### 1.4 Out of Scope

- HTTP SSE transport (Phase 2)
- `evaluate_strategy` tool (Phase 3)
- Hot reload of decision tree (Phase 2)
- Retraining pipeline (Phase 2, logging only in MVP)
- Dashboard / TUI (Phase 4)
- Docker Compose deployment (Phase 4)
- ONNX backend (not critical for MVP)

---

## 2. Functional Requirements

### 2.1 MCP Server Core

#### REQ-001: MCP Server Initialization
**As a** Agent Orchestrator
**I want** to start Arbiter as a subprocess and establish MCP handshake
**So that** I can route coding tasks through the policy engine

**Acceptance Criteria:**
```gherkin
GIVEN a valid config directory with agents.toml and invariants.toml
AND a valid decision tree JSON file
WHEN Arbiter is started with --tree, --config, --db arguments
THEN it initializes in < 500ms
AND responds to "initialize" with capabilities {"tools": {}}
AND responds to "initialized" notification
AND responds to "tools/list" with 3 tools
```

**Priority:** P0
**Traces to:** [TASK-001], [TASK-002], [DESIGN-001]

---

#### REQ-002: MCP Protocol Compliance
**As a** Agent Orchestrator
**I want** Arbiter to handle JSON-RPC 2.0 correctly
**So that** communication is reliable and errors are diagnosable

**Acceptance Criteria:**
```gherkin
GIVEN a running Arbiter server
WHEN an unknown method is sent
THEN it returns JSON-RPC error -32601

GIVEN a running Arbiter server
WHEN invalid params are sent for a known tool
THEN it returns JSON-RPC error -32602 with description

GIVEN a running Arbiter server
WHEN stdin reaches EOF
THEN it flushes SQLite WAL and exits with code 0
```

**Priority:** P0
**Traces to:** [TASK-002], [DESIGN-001]

---

#### REQ-003: Graceful Error Handling
**As a** Agent Orchestrator
**I want** Arbiter to degrade gracefully on failures
**So that** tasks can still be routed even when components fail

**Acceptance Criteria:**
```gherkin
GIVEN a server started without a valid decision tree
WHEN route_task is called
THEN it uses hardcoded round-robin fallback
AND includes warning in response

GIVEN a server with SQLite write failure
WHEN report_outcome is called
THEN it returns result with warning
AND logs error to stderr

GIVEN all agents in state=failed
WHEN route_task is called
THEN it returns action="reject" with reasoning="All agents unhealthy"
```

**Priority:** P1
**Traces to:** [TASK-002], [TASK-009], [DESIGN-001]

---

### 2.2 Task Routing

#### REQ-010: Route Task to Agent
**As a** Agent Orchestrator
**I want** to send a task description and receive an agent assignment
**So that** each task goes to the best-suited agent

**Acceptance Criteria:**
```gherkin
GIVEN 3 active agents with available capacity
AND a valid task description with type, language, complexity
WHEN route_task is called
THEN it returns a decision within 5ms
AND the decision contains chosen_agent, confidence [0,1], decision_path
AND invariant_checks contains all 10 rule results
AND the decision is logged to SQLite decisions table
AND running_tasks is incremented for the chosen agent
```

**Priority:** P0
**Traces to:** [TASK-004], [TASK-005], [TASK-006], [DESIGN-002], [DESIGN-003], [DESIGN-004]

---

#### REQ-011: Preferred Agent Boost
**As a** Agent Orchestrator
**I want** to suggest a preferred agent
**So that** the system respects my preference when feasible

**Acceptance Criteria:**
```gherkin
GIVEN a route_task request with preferred_agent="claude_code"
WHEN claude_code is available and passes invariants
THEN its confidence gets a +0.1 boost
AND it is selected (all else equal)
```

**Priority:** P1
**Traces to:** [TASK-004], [DESIGN-002]

---

#### REQ-012: Agent Exclusion
**As a** Agent Orchestrator
**I want** to exclude specific agents from a routing decision
**So that** I can prevent known-bad assignments

**Acceptance Criteria:**
```gherkin
GIVEN a route_task request with excluded_agents=["aider", "codex_cli"]
WHEN only claude_code remains as candidate
THEN claude_code is selected

GIVEN a route_task request with all agents excluded
WHEN no candidates remain
THEN action="reject" with reasoning explaining exclusion
```

**Priority:** P1
**Traces to:** [TASK-004], [DESIGN-002]

---

#### REQ-013: Cascade Fallback on Critical Invariant Violation
**As a** Arbiter system
**I want** to try the next-best agent when the top choice fails a critical invariant
**So that** tasks are routed safely without manual intervention

**Acceptance Criteria:**
```gherkin
GIVEN agent A is top-ranked but has a scope conflict (critical violation)
WHEN route_task evaluates invariants
THEN it falls back to agent B (next by score)
AND runs invariants on agent B
AND returns action="fallback" with fallback_reason
AND max fallback attempts = 2

GIVEN all candidate agents fail critical invariants
WHEN cascade is exhausted
THEN action="reject" with all invariant results
```

**Priority:** P0
**Traces to:** [TASK-004], [TASK-006], [DESIGN-002], [DESIGN-004]

---

### 2.3 Feedback Loop

#### REQ-020: Report Task Outcome
**As a** Agent Orchestrator
**I want** to report task execution results back to Arbiter
**So that** agent performance stats are updated for future routing

**Acceptance Criteria:**
```gherkin
GIVEN a completed task with known task_id
WHEN report_outcome is called with status, duration, cost, tokens
THEN outcome is inserted into outcomes table
AND agent_stats are updated (totals, success rate, cost)
AND running_tasks is decremented for the agent (never < 0)
AND response contains updated_stats and retrain_suggested flag

GIVEN > 5 failures for an agent in 24 hours
WHEN report_outcome is called
THEN retrain_suggested = true
AND agent_health warning is logged
```

**Priority:** P0
**Traces to:** [TASK-007], [DESIGN-005]

---

#### REQ-021: Report Outcome Idempotency
**As a** Agent Orchestrator
**I want** duplicate report_outcome calls to be handled safely
**So that** network retries don't corrupt stats

**Acceptance Criteria:**
```gherkin
GIVEN a report_outcome for task_id "task-1" already exists
WHEN report_outcome is called again for "task-1"
THEN the outcome is recorded (new row)
AND stats are updated accordingly
AND no error is returned
```

**Priority:** P2
**Traces to:** [TASK-007], [DESIGN-005]

---

#### REQ-022: Unknown Task ID in Outcome
**As a** Agent Orchestrator
**I want** to report outcomes for tasks not previously routed
**So that** external task results can still be logged

**Acceptance Criteria:**
```gherkin
GIVEN report_outcome for a task_id with no matching decision
WHEN the outcome is processed
THEN it is recorded with decision_id=NULL
AND response includes warning "No matching decision found"
```

**Priority:** P2
**Traces to:** [TASK-007], [DESIGN-005]

---

### 2.4 Agent Status

#### REQ-030: Query Agent Status
**As a** Agent Orchestrator
**I want** to query agent capabilities and performance
**So that** I can display status and make manual decisions

**Acceptance Criteria:**
```gherkin
GIVEN a running Arbiter with 3 registered agents
WHEN get_agent_status is called without params
THEN it returns all 3 agents with capabilities and stats

GIVEN a running Arbiter with agent_id="claude_code"
WHEN get_agent_status is called with agent_id
THEN it returns only claude_code's data
AND performance stats are calculated from agent_stats table
AND by_language and by_type groupings are present

GIVEN an unknown agent_id
WHEN get_agent_status is called
THEN it returns error "agent not found"
```

**Priority:** P1
**Traces to:** [TASK-008], [DESIGN-006]

---

### 2.5 Feature Vector

#### REQ-040: Build 22-Dimensional Feature Vector
**As a** Arbiter policy engine
**I want** to convert task JSON + agent stats into a numeric feature vector
**So that** the Decision Tree can make inference

**Acceptance Criteria:**
```gherkin
GIVEN a task with type, language, complexity, priority and agent stats
WHEN feature vector is built for 3 agents
THEN 3 vectors of exactly 22 floats are produced
AND all values are within specified ranges (capped)
AND missing optional fields use documented defaults
AND feature builder works without SQLite (for unit tests)
```

**Priority:** P0
**Traces to:** [TASK-005], [DESIGN-003]

---

### 2.6 Invariant Rules

#### REQ-050: 10 Safety Invariant Checks
**As a** Arbiter system
**I want** to enforce 10 invariant rules before every agent assignment
**So that** safety and resource constraints are never violated

**Acceptance Criteria:**
```gherkin
GIVEN a routing decision for agent A on task T
WHEN invariant checks run
THEN all 10 rules are evaluated and returned in response
AND critical violations block assignment (trigger fallback)
AND warning violations are logged but allow assignment
AND total invariant check time < 1ms
```

Rules:
1. `agent_available` (Critical) - agent active AND has slots
2. `scope_isolation` (Critical) - no file/dir overlap with running tasks
3. `branch_not_locked` (Critical) - task branch not in use
4. `concurrency_limit` (Critical) - total running < max
5. `budget_remaining` (Warning) - estimated cost <= budget
6. `retry_limit` (Warning) - retry count < max
7. `rate_limit` (Warning) - API calls < limit/min
8. `agent_health` (Warning) - failures_24h < threshold
9. `task_compatible` (Warning) - agent supports language AND type
10. `sla_feasible` (Warning) - estimated duration * buffer <= SLA

**Priority:** P0
**Traces to:** [TASK-006], [DESIGN-004]

---

### 2.7 Decision Tree & Bootstrap

#### REQ-060: Decision Tree Inference
**As a** Arbiter policy engine
**I want** to load and evaluate a sklearn-exported Decision Tree
**So that** routing decisions are data-driven

**Acceptance Criteria:**
```gherkin
GIVEN a valid agent_policy_tree.json
WHEN loaded at startup
THEN tree is parsed and ready for inference

GIVEN a 22-dim feature vector
WHEN Decision Tree inference runs
THEN it returns a class (agent) with confidence score
AND same input always produces same output (deterministic)
```

**Priority:** P0
**Traces to:** [TASK-003], [DESIGN-007]

---

#### REQ-061: Bootstrap Tree from Expert Rules
**As a** a developer setting up Arbiter
**I want** to generate a decision tree from expert rules
**So that** the system works well from cold start

**Acceptance Criteria:**
```gherkin
GIVEN 10 expert routing rules
WHEN bootstrap_agent_tree.py runs
THEN it generates ~500 training examples
AND trains a DecisionTreeClassifier(max_depth=7, min_samples_leaf=10)
AND exports to Arbiter JSON format
AND accuracy > 95% on training data
AND tree depth <= 7, node count <= 127
AND the JSON loads in Rust without errors
```

**Priority:** P0
**Traces to:** [TASK-003], [DESIGN-007]

---

### 2.8 Configuration

#### REQ-070: Agent Configuration via TOML
**As a** an operator
**I want** to define agents and their capabilities in a config file
**So that** I can add/modify agents without recompiling

**Acceptance Criteria:**
```gherkin
GIVEN a valid agents.toml with 3 agent definitions
WHEN Arbiter starts
THEN agents are registered with capabilities, cost, concurrency limits
AND inserted into SQLite if not existing

GIVEN agents.toml is missing
WHEN Arbiter starts
THEN it exits with code 1 and stderr: "Config not found: {path}"

GIVEN agents.toml has a missing required field
WHEN Arbiter starts
THEN it exits with code 1 and stderr names the missing field
```

**Priority:** P0
**Traces to:** [TASK-002], [DESIGN-008]

---

#### REQ-071: Invariant Thresholds via TOML
**As a** an operator
**I want** to configure invariant rule thresholds
**So that** I can tune safety parameters without recompiling

**Acceptance Criteria:**
```gherkin
GIVEN a valid invariants.toml with budget, retry, rate_limit thresholds
WHEN invariant rules evaluate
THEN they use thresholds from config (not hardcoded)
```

**Priority:** P1
**Traces to:** [TASK-002], [DESIGN-008]

---

### 2.9 Persistence

#### REQ-080: SQLite Persistence
**As a** Arbiter system
**I want** to persist all decisions, outcomes, and stats in SQLite
**So that** there is a full audit trail and stats survive restarts

**Acceptance Criteria:**
```gherkin
GIVEN Arbiter starts with --db arbiter.db
WHEN the database doesn't exist
THEN it creates it with schema v1 (5 tables, 8 indices)

GIVEN decisions and outcomes are recorded
WHEN Arbiter restarts
THEN all historical data is available
AND agent_stats reflect accumulated history

GIVEN concurrent reads/writes
WHEN SQLite contention occurs
THEN retry with backoff (50ms, 100ms, 200ms), max 3 attempts
```

**Priority:** P0
**Traces to:** [TASK-009], [DESIGN-009]

---

### 2.10 Python MCP Client

#### REQ-090: Python MCP Client for Orchestrator
**As a** Agent Orchestrator developer
**I want** an ArbiterClient class that manages the subprocess
**So that** I can integrate Arbiter with minimal boilerplate

**Acceptance Criteria:**
```gherkin
GIVEN ArbiterClient configured with binary_path, tree, config, db
WHEN client.start() is called
THEN Arbiter subprocess starts and MCP handshake completes

GIVEN a started client
WHEN client.route_task(task_id, task) is called
THEN it sends JSON-RPC and returns the decision

GIVEN Arbiter crashes during operation
WHEN client detects broken pipe
THEN it reconnects after 1s and retries
```

**Priority:** P1
**Traces to:** [TASK-010], [DESIGN-010]

---

## 3. Non-Functional Requirements

### NFR-000: Testing Requirements
| Aspect | Requirement |
|--------|------------|
| Unit tests (Rust) | 22 tests, arbiter-core + arbiter-mcp |
| Integration tests (Rust) | 7 tests, full route+report cycles |
| Protocol tests (Python) | 7 tests, MCP protocol over stdio |
| Benchmarks (Rust) | 5 benchmarks, throughput + latency |
| Test framework (Rust) | `cargo test` |
| Test framework (Python) | `pytest` with `pytest-asyncio` |
| CI requirement | All tests pass before merge |

**Definition of Done for any task:**
- [ ] Unit tests written and passing
- [ ] Coverage has not decreased
- [ ] Integration test if interfaces are affected
- [ ] Documentation updated

**Traces to:** [TASK-100]

---

### NFR-001: Performance
| Metric | Requirement |
|---------|------------|
| route_task throughput | > 10,000 decisions/sec (in-process) |
| route_task e2e latency | < 5ms p99 (over MCP stdio) |
| report_outcome latency | < 10ms p99 (including SQLite write) |
| Memory usage | < 50MB RSS |
| SQLite size after 10K decisions | < 10MB |
| Startup time | < 500ms |
| Invariant check time | < 1ms (all 10 rules) |

**Traces to:** [TASK-011]

---

### NFR-002: Reliability
| Aspect | Requirement |
|--------|------------|
| No `unsafe` code | Production code must not use `unsafe` |
| No `unwrap()` | Use `?` operator, `anyhow::Result`, or explicit handling |
| Graceful shutdown | Flush SQLite WAL on stdin EOF, exit code 0 |
| SQLite contention | Retry with backoff, max 3 attempts |
| Degraded mode | Hardcoded round-robin when tree unavailable |

**Traces to:** [TASK-002], [TASK-009]

---

### NFR-003: Observability
| Metric | Requirement |
|---------|------------|
| Logging | `tracing` crate, all output to stderr |
| Log levels | trace, debug, info, warn, error (configurable via --log-level) |
| Audit trail | Every decision logged to SQLite with full context |
| Metrics | Atomic counters for decisions, outcomes, errors |

**Traces to:** [TASK-002], [TASK-009]

---

## 4. Constraints and Tech Stack

### 4.1 Technology Constraints

| Aspect | Decision | Rationale |
|--------|---------|-------------|
| Core language | Rust | Performance, safety, reuse from AI-OS PoC |
| Client language | Python | Orchestrator is Python-based |
| Database | SQLite (bundled) | No external dependencies, embedded |
| Protocol | JSON-RPC 2.0 over stdio | MCP standard, simple, no HTTP overhead |
| Config | TOML | Human-readable, Rust ecosystem standard |
| ML inference | sklearn Decision Tree (JSON export) | Simple, fast, interpretable |

### 4.2 Integration Constraints

- MCP protocol is hand-rolled (no SDK dependency)
- stdout is reserved for MCP protocol only
- All logging goes to stderr via `tracing`
- Decision Tree JSON format must be compatible with `arbiter-core::policy::decision_tree`

### 4.3 Business Constraints

- Scope: MVP only (Phase 1)
- Team: Solo developer
- Dependencies: Reuse arbiter-core from AI-OS PoC

---

## 5. Acceptance Criteria

### Milestone 1: MVP
- [ ] REQ-001 — MCP server starts and handshakes
- [ ] REQ-002 — JSON-RPC protocol compliance
- [ ] REQ-010 — route_task returns correct decisions
- [ ] REQ-013 — Cascade fallback works
- [ ] REQ-020 — report_outcome updates stats
- [ ] REQ-040 — Feature vector built correctly
- [ ] REQ-050 — All 10 invariant rules work
- [ ] REQ-060 — Decision Tree inference works
- [ ] REQ-061 — Bootstrap tree generated
- [ ] REQ-070 — Agent config loaded from TOML
- [ ] REQ-080 — SQLite persistence works
- [ ] NFR-001 — Performance targets met
- [ ] NFR-000 — All tests pass (22 + 7 + 7 = 36 tests)

### Milestone 2: Integration
- [ ] REQ-003 — Graceful degradation
- [ ] REQ-011 — Preferred agent boost
- [ ] REQ-012 — Agent exclusion
- [ ] REQ-021 — Report outcome idempotency
- [ ] REQ-030 — Agent status query
- [ ] REQ-071 — Invariant thresholds configurable
- [ ] REQ-090 — Python MCP client ready

### Milestone 3: Production Readiness
- [ ] REQ-022 — Unknown task ID handling
- [ ] NFR-002 — All reliability requirements met
- [ ] NFR-003 — Full observability
- [ ] Documentation complete (README with quick start)
