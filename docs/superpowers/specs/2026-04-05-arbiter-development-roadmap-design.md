# Arbiter Development Roadmap

**Date:** 2026-04-05
**Status:** Draft
**Approach:** Incremental releases (3-5 tasks per release), sequential phases A → B → C

## Overview

Three-phase development plan for Arbiter — from production-hardening through feature expansion to ecosystem integration. Each phase is broken into 2-3 release batches. Every batch leaves the project in a working, tested state.

**Total estimate:** ~50-60 hours across 8 release batches.

---

## Phase A: Production Hardening

**Goal:** Arbiter does not crash, does not lie, meets p99 < 5ms, architecture is ready for extension.

### R1 — Server Stops Crashing (~4-6h)

| # | Task | Crate | Description |
|---|------|-------|-------------|
| 1 | Panic-freedom | arbiter-core | Replace `assert_eq!`, `unwrap()` with `Result` in `decision_tree.rs::predict()` — 3 panic points on hot path |
| 2 | Fix dead invariants | arbiter-core | `retry_limit` and `rate_limit` hardcode 0 → connect to real config values |
| 3 | Fix error codes | arbiter-mcp | `-32602` → `-32000` in `report_outcome` / `get_agent_status` (JSON-RPC spec) |
| 4 | Fix hardcoded state | arbiter-mcp | `AgentState::Active` hardcoded → compute from stats (failures, load) |

**Exit criteria:** `cargo test` green, zero `unwrap()`/`assert!` in production paths, all 10 invariants functional.

### R2 — p99 < 5ms (~4-6h)

| # | Task | Crate | Description |
|---|------|-------|-------------|
| 1 | SQLite PRAGMAs | arbiter-mcp | `synchronous=NORMAL` (safe with WAL), `mmap_size`, increase `cache_size` |
| 2 | `prepare_cached` | arbiter-mcp | Cache prepared statements instead of recompiling |
| 3 | Composite index | arbiter-mcp | Index on `(agent_id, task_type)` in `agent_stats` — eliminate sequential scan |
| 4 | Agent stats cache | arbiter-mcp | In-memory LRU/TTL cache for `get_agent_stats` — eliminate 9 queries/request |
| 5 | Lazy decision path | arbiter-core | Build `decision_path` string lazily (no allocation per tree node) |

**Exit criteria:** BT-02 benchmark shows p99 < 5ms, hot path allocations reduced by order of magnitude.

### R3 — Architecture Ready for Extension (~6-8h)

| # | Task | Crate | Description |
|---|------|-------|-------------|
| 1 | Typed errors (`thiserror`) | arbiter-core | Replace string errors with `ArbiterError` enum with variants |
| 2 | Trait abstractions | arbiter-core/mcp | `DecisionStore`, `AgentStore`, `InferenceBackend` — decouple from concrete implementations |
| 3 | Config validation | arbiter-mcp | Semantic validation: `max_concurrent > 0`, `cost > 0`, `threshold > 0` |
| 4 | Clean up deps | workspace | `thiserror` declared but unused → now used after task 1. Evaluate `tokio`: currently unused, but needed for signal handling in R4 — keep if so, remove if not |
| 5 | Bootstrap CV | scripts | Add k-fold cross-validation to `bootstrap_agent_tree.py`, verify accuracy on held-out data |

**Exit criteria:** `cargo clippy -- -D warnings` clean, traits enable mock testing, bootstrap gives accuracy > 90% on cross-validation.

---

## Phase B: Feature Expansion

**Goal:** Arbiter is observable, adaptive, learns from real data.

### R4 — Observability (~6-8h)

| # | Task | Crate | Description |
|---|------|-------|-------------|
| 1 | Structured metrics | arbiter-mcp | 3 key metrics: `decisions_total` (counter), `route_latency_ms` (histogram), `fallback_rate` (gauge). Export via new MCP tool `get_metrics` |
| 2 | Data retention | arbiter-mcp | Delete `decisions`/`outcomes` older than N days. Without this — 7.3 GB/year |
| 3 | Graceful shutdown | arbiter-mcp | SIGTERM/SIGHUP handling: flush pending writes, close DB cleanly |
| 4 | Ping support | arbiter-mcp | Handle `ping` method — Claude Desktop disconnects without a response |

**Exit criteria:** `get_metrics` returns current metrics, DB does not grow unbounded, server shuts down cleanly on signal.

### R5 — Hot Reload + Cost Tracking (~6-8h)

| # | Task | Crate | Description |
|---|------|-------|-------------|
| 1 | Hot reload config | arbiter-mcp | File watcher (`notify` crate) + `Arc<RwLock<Config>>` — re-read `agents.toml`/`invariants.toml` without restart |
| 2 | Hot reload tree | arbiter-mcp | Same for `agent_policy_tree.json` — `Arc<RwLock<DecisionTree>>` |
| 3 | Cost accumulator | arbiter-mcp | Accumulate `actual_cost_usd` from outcomes, enforce budget invariant on real data instead of estimates |
| 4 | Budget dashboard tool | arbiter-mcp | New MCP tool `get_budget_status` — spent / limit / by agent / by type |

**Exit criteria:** TOML/JSON changes picked up within <2s without restart. Budget enforcement works on real expenditures.

### R6 — Retrain Pipeline + Eval (~8-10h)

| # | Task | Crate | Description |
|---|------|-------|-------------|
| 1 | `--from-db` in bootstrap | scripts | Implement the documented but non-existent argument: read outcomes from SQLite → training data |
| 2 | Retrain pipeline | scripts | Script: extract → train → validate (CV accuracy > threshold) → export JSON. Without validation — do not overwrite |
| 3 | Eval framework | scripts/tests | A/B comparison: DT vs round-robin vs always-best on 50-task benchmark suite. Metrics: accuracy, cost, latency |
| 4 | Feature importance | scripts | Feature importance report from sklearn — which features actually affect routing |
| 5 | Criterion benchmarks | arbiter-cli | Replace custom timing with `criterion` — statistical significance, regression tracking |

**Exit criteria:** Tree can be retrained on real data. Eval shows DT beats random on benchmark suite. Benchmarks with statistics.

---

## Phase C: Ecosystem Integration

**Goal:** Arbiter works in real environment — with Claude Desktop, Maestro, on real tasks.

### R7 — Claude Desktop + Protocol Compliance (~4-6h)

| # | Task | Crate | Description |
|---|------|-------|-------------|
| 1 | Claude Desktop compatibility | arbiter-mcp | Verify and fix: `notifications/initialized`, capabilities handshake, keep-alive ping/pong — everything needed for stable MCP server in Claude Desktop |
| 2 | JSON-RPC hardening | arbiter-mcp | Validate `jsonrpc: "2.0"`, line length limit (OOM protection), batch request support (optional) |
| 3 | Golden tests | tests | 10-15 reference request/response pairs in files — regression suite for protocol. Any format change breaks a test |
| 4 | MCP config template | config | Ready `claude_desktop_config.json` snippet to connect Arbiter with a single paste |

**Exit criteria:** Arbiter works stably as MCP server in Claude Desktop. Golden tests green. README contains copy-paste instruction.

### R8 — Maestro Integration + Production Readiness (~6-8h)

| # | Task | Crate | Description |
|---|------|-------|-------------|
| 1 | Maestro contract | orchestrator | Formalize interface: `ArbiterClient` <-> Maestro. Typed DTOs, versioning, backward compatibility |
| 2 | `running_tasks` reconciliation | arbiter-mcp | On start/reconnect — reset orphaned running_tasks (crash recovery). Counter must not drift |
| 3 | Provider-aware routing | arbiter-core/mcp | 2-3 new features in feature vector: provider health, provider latency. Invariant `provider_available` — fallback on LLM provider unavailability |
| 4 | End-to-end smoke test | orchestrator/tests | Full cycle: Maestro → ArbiterClient → route → execute (mock) → report_outcome → verify stats |
| 5 | Ops documentation | docs | Runbook: startup, monitoring, troubleshooting, retrain, DB backup |

**Exit criteria:** Maestro uses Arbiter via stable contract. Crash recovery does not lose state. Ops runbook exists.

---

## Summary

| Phase | Batches | Estimated Hours | Key Outcome |
|-------|---------|-----------------|-------------|
| A: Production Hardening | R1, R2, R3 | 14-20h | Safe, fast, extensible |
| B: Feature Expansion | R4, R5, R6 | 20-26h | Observable, adaptive, data-driven |
| C: Ecosystem Integration | R7, R8 | 10-14h | Real-world deployment ready |
| **Total** | **8 batches** | **44-60h** | **Production-grade agent router** |

## Dependencies Between Batches

```
R1 ──→ R2 ──→ R3 ──→ R4 ──→ R5 ──→ R6
                       │               
                       └──→ R7 ──→ R8  
```

- R1 → R2: Panic-freedom before performance optimization
- R2 → R3: Performance baseline before architecture changes
- R3 → R4: Traits needed for metrics implementation
- R5 depends on R3 (traits for `Arc<RwLock<>>` wrappers) and R4 (ping/shutdown infra)
- R6 depends on R5 (cost data for retrain)
- R7 can start after R4 (needs ping support from R4). R7 and R5/R6 are parallelizable
- R8 depends on R7 (protocol compliance first)

## Open Questions

1. **Retention policy:** How many days to keep decisions/outcomes? (Suggested: 90 days)
2. **Primary consumer:** Claude Desktop or Maestro first? (Design assumes Claude Desktop in R7, Maestro in R8)
3. **Provider health source:** How to get LLM provider status? (API ping, external monitor, manual config?)
