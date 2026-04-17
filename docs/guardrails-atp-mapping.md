# Arbiter invariants ↔ ATP guardrails — semantic mapping

Status: first draft, 2026-04-17. Task R-13 from `TODO.md`. Reviewer: Andrei.

## TL;DR

- `arbiter-core/src/invariant/rules.rs` has **10 invariants**; `atp-platform/atp/evaluators/guardrails.py` has **3 guardrails**.
- The ATP module docstring says "inspired by arbiter". True for the **pattern**; **not** for the rule set.
- The two systems operate in **non-overlapping phases** of the task lifecycle — arbiter runs a **pre-hoc** gate before agent assignment; ATP runs a **post-hoc** gate before evaluator execution. The same word ("guardrail") covers two different responsibilities.
- Only 2 of the 3 ATP rules have a partial arbiter counterpart, and in each case the predicate is **inverted** (estimate vs. measurement).
- **Recommendation**: keep the rule sets disjoint, align terminology in docs, do **not** extract a shared types library yet (over-engineering for ~15 lines of struct).

## Phase diagram

```
                    ┌─────────────────────────────────────────┐
                    │               Task lifecycle            │
                    └─────────────────────────────────────────┘

 route_task()  ──►  ┌──────────┐      ┌──────────┐      ┌──────────┐
                    │ arbiter  │      │  agent   │      │   ATP    │
  ● invariants ●    │ decides  │ ───► │ executes │ ───► │ evaluates│
  (pre-dispatch)    │          │      │          │      │          │
                    └──────────┘      └──────────┘      └────┬─────┘
                                                             │
                                                    ● guardrails ●
                                                    (pre-evaluation)
```

Arbiter sees `TaskInput + AgentContext + SystemContext`. ATP sees `TestDefinition + ATPResponse`. **Neither module can observe the other's state** — there is no data contract between them.

## Rule-by-rule mapping

| # | ATP guardrail | arbiter invariant | Severity (arbiter) | Relation |
|---|---|---|---|---|
| 1 | `response_not_empty` | — | — | **ATP-only**: arbiter never sees agent output. |
| 2 | `timeout_not_exceeded` | `sla_feasible` | Warning | **Inverse pair**: arbiter asks *"can the agent likely meet the SLA?"* (pre-hoc, uses `agent.avg_duration_min * buffer_multiplier ≤ task.sla_minutes`). ATP asks *"did the agent timeout?"* (post-hoc, uses `response.status == timeout`). |
| 3 | `within_budget` | `budget_remaining` | Warning | **Inverse pair**: arbiter compares `agent.cost_per_hour × avg_duration ≤ system.budget_remaining_usd` (system-wide remaining budget, pre-hoc estimate). ATP compares `response.metrics.cost_usd ≤ test.constraints.budget_usd` (per-test actual cost, post-hoc measurement). |
| — | — | `agent_available` | Critical | arbiter-only. Slots/state check. |
| — | — | `scope_isolation` | Critical | arbiter-only. Multi-agent conflict. |
| — | — | `branch_not_locked` | Critical | arbiter-only. Git branch lock. |
| — | — | `concurrency_limit` | Critical | arbiter-only. System-wide cap. |
| — | — | `retry_limit` | Warning | arbiter-only. Retry policy. |
| — | — | `rate_limit` | Warning | arbiter-only. API QPS. |
| — | — | `agent_health` | Warning | arbiter-only. 24h failure rate / circuit breaker. |
| — | — | `task_compatible` | Warning | arbiter-only. Language/type capability. |

**Overlap score**: 0 rules share semantics. 2 rules share a *concept* (budget, time). 8 arbiter invariants have no ATP analogue; 1 ATP guardrail has no arbiter analogue.

## Structural comparison

| Concern | arbiter | ATP |
|---|---|---|
| Result type | `InvariantResult { rule, severity, passed, detail }` (Rust, 4 fields) | `CheckResult { name, passed, reason }` (Python, 3 fields) |
| Severity model | `Critical` blocks assignment + cascade fallback; `Warning` is logged | Flat pass/fail — first failure skips the evaluator |
| Aggregator | `check_all_invariants` always returns exactly 10 results | `run_guardrails` returns a list; `should_skip_evaluation` returns the first failure's reason |
| Blocking semantics | Critical failure → try next agent, up to 2 cascades, then reject | Any failure → skip evaluators, mark the test accordingly |
| Naming style | snake_case positive ("branch_not_locked", "agent_available") | snake_case positive ("response_not_empty", "within_budget") — already consistent |

## What "inspired by arbiter" actually means

The pattern ATP borrowed is real and sound:

1. Express the gate as a flat list of independent predicates, each returning a small result object.
2. Short-circuit the caller when any predicate fails.
3. Log the failure reason even when passing (useful for traces).

That's a good design borrowed verbatim. But the **predicate bodies** are disjoint because the two modules guard different things at different times. Documenting this explicitly closes the ambiguity introduced by the comment.

## Recommendations

### 1. Do not extract a shared types library (R-14, XL)

- The cross-language surface in scope is ~15 lines (`CheckResult` + `InvariantResult` + `Severity` enum).
- Maintaining a Rust→Python codegen pipeline for 15 lines is pure overhead.
- The two systems evolve on different cadences (arbiter ships MCP schema changes behind a contract freeze; ATP's evaluators are added per-domain).
- Revisit **only** if a third project needs guardrails and there is a real pull for unification.

### 2. Keep rule sets disjoint; rename ATP docstring for precision

Small ATP follow-up (low effort, not in arbiter's repo):

- Change the opening line of `atp/evaluators/guardrails.py` from `"Pre-evaluation guardrails inspired by arbiter's invariant rules."` to a wording that makes clear ATP inherited **the pattern, not the rules**, and names the lifecycle phase ("post-execution, pre-evaluation gate").
- Document in ATP's top-level docs how its guardrails relate to arbiter's invariants (link to this file).

### 3. Align `budget` and `time` rule descriptions, not names

- `budget_remaining` (arbiter) and `within_budget` (ATP) measure different things on different axes. Renaming either would hide the difference. Keep both names, but add a one-liner to each docstring clarifying the phase + axis (estimated vs. measured, system-wide vs. per-test).
- Same for `sla_feasible` (arbiter) vs. `timeout_not_exceeded` (ATP).

### 4. Consider severity levels in ATP — only if guardrails grow past 5

At 3 rules, ATP's flat pass/fail model is fine. If the module grows (e.g. "response format is parseable JSON", "response contains required tool calls") a `Warning` tier would let ATP continue evaluation with a penalty score instead of hard-skipping. No action now.

### 5. Update cross-project docs

- `_cowork_output/contracts/contract-analysis.md` should reference this file.
- `CLAUDE.md` at the repo root (`../CLAUDE.md`) mentions R-13 — link here from the ecosystem roadmap.
- `TODO.md` in arbiter: mark R-13 closed with this commit hash.

## Non-goals

- Rewriting ATP guardrails to match arbiter's 10-rule canonical form. They serve different purposes.
- Exposing arbiter invariants via MCP for ATP consumption. ATP doesn't need pre-dispatch checks; it sees tasks after execution.
- Changing `InvariantResult` / `CheckResult` shapes to be byte-compatible across FFI. Premature.

## Appendix: concrete file pointers

- arbiter invariants: `arbiter-core/src/invariant/rules.rs:77-390` (one function per rule)
- arbiter thresholds config: `config/invariants.toml` (mirrors `InvariantThresholds`)
- arbiter aggregator: `arbiter-core/src/invariant/rules.rs:409` (`check_all_invariants`)
- ATP guardrails: `../atp-platform/atp/evaluators/guardrails.py:25-68` (three `check_*` functions)
- ATP aggregator: `../atp-platform/atp/evaluators/guardrails.py:71-102` (`run_guardrails` + `should_skip_evaluation`)
