# R4 — Observability

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Arbiter observable and operationally sound: in-memory metrics exported via `get_metrics` MCP tool, data retention to prevent unbounded DB growth, graceful shutdown on SIGTERM/SIGHUP, and ping support for Claude Desktop.

**Architecture:** Metrics are kept in-memory using `std::sync::atomic` counters (no external dep). A new `Metrics` struct lives in `arbiter-mcp/src/metrics.rs` and is shared via `&Metrics` reference. Data retention adds a `purge_older_than` method to `Database`. Signal handling uses `signal-hook` crate with an `AtomicBool` flag checked in the server's stdio loop. Ping is a simple JSON-RPC method response.

**Tech Stack:** Rust (signal-hook for Unix signals, std::sync::atomic for metrics, rusqlite for retention)

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `arbiter-mcp/src/metrics.rs` | In-memory metrics: counters, histogram, gauge |
| Create | `arbiter-mcp/src/tools/get_metrics.rs` | MCP tool handler for `get_metrics` |
| Modify | `arbiter-mcp/src/tools/mod.rs` | Export new tool module |
| Modify | `arbiter-mcp/src/server.rs` | Add `get_metrics` tool schema, ping handler, metrics integration |
| Modify | `arbiter-mcp/src/tools/route_task.rs` | Record metrics on each routing decision |
| Modify | `arbiter-mcp/src/db.rs` | Add `purge_older_than` for data retention |
| Modify | `arbiter-mcp/src/main.rs` | Signal handling, retention on startup, metrics wiring |
| Modify | `arbiter-mcp/src/lib.rs` | Export metrics module |
| Modify | `arbiter-mcp/Cargo.toml` | Add `signal-hook` dep |
| Modify | `Cargo.toml` (workspace) | Add `signal-hook` to workspace deps |

---

### Task 1: In-Memory Metrics

**Files:**
- Create: `arbiter-mcp/src/metrics.rs`
- Modify: `arbiter-mcp/src/lib.rs`

In-memory metrics using atomics. No external dependencies. Three metrics:
- `decisions_total` — counter (AtomicU64), incremented per route_task call
- `fallbacks_total` — counter (AtomicU64), incremented per fallback decision
- `route_latency_us` — tracks last/min/max/sum/count for computing avg (all AtomicU64/AtomicI64)

- [ ] **Step 1: Create `arbiter-mcp/src/metrics.rs`**

```rust
//! In-memory metrics for the Arbiter MCP server.
//!
//! All metrics use atomics for lock-free thread-safe access.
//! No external dependencies — pure `std::sync::atomic`.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

/// Server-wide metrics, shared by reference.
pub struct Metrics {
    /// Total routing decisions made (counter).
    decisions_total: AtomicU64,
    /// Total fallback decisions (counter).
    fallbacks_total: AtomicU64,
    /// Total rejected decisions (counter).
    rejects_total: AtomicU64,
    /// Route latency: count of observations.
    latency_count: AtomicU64,
    /// Route latency: sum of microseconds.
    latency_sum_us: AtomicU64,
    /// Route latency: minimum microseconds (-1 = no observations).
    latency_min_us: AtomicI64,
    /// Route latency: maximum microseconds.
    latency_max_us: AtomicI64,
}

impl Metrics {
    /// Create a new zeroed metrics instance.
    pub fn new() -> Self {
        Self {
            decisions_total: AtomicU64::new(0),
            fallbacks_total: AtomicU64::new(0),
            rejects_total: AtomicU64::new(0),
            latency_count: AtomicU64::new(0),
            latency_sum_us: AtomicU64::new(0),
            latency_min_us: AtomicI64::new(-1),
            latency_max_us: AtomicI64::new(0),
        }
    }

    /// Record a routing decision.
    pub fn record_decision(&self, latency_us: u64, is_fallback: bool, is_reject: bool) {
        self.decisions_total.fetch_add(1, Ordering::Relaxed);
        if is_fallback {
            self.fallbacks_total.fetch_add(1, Ordering::Relaxed);
        }
        if is_reject {
            self.rejects_total.fetch_add(1, Ordering::Relaxed);
        }
        self.latency_count.fetch_add(1, Ordering::Relaxed);
        self.latency_sum_us.fetch_add(latency_us, Ordering::Relaxed);

        // Update min (CAS loop)
        let latency_i64 = latency_us as i64;
        loop {
            let current = self.latency_min_us.load(Ordering::Relaxed);
            if current != -1 && current <= latency_i64 {
                break;
            }
            if self
                .latency_min_us
                .compare_exchange_weak(
                    current,
                    latency_i64,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                break;
            }
        }

        // Update max (CAS loop)
        loop {
            let current = self.latency_max_us.load(Ordering::Relaxed);
            if current >= latency_i64 {
                break;
            }
            if self
                .latency_max_us
                .compare_exchange_weak(
                    current,
                    latency_i64,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                break;
            }
        }
    }

    /// Snapshot current metrics as a JSON-serializable struct.
    pub fn snapshot(&self) -> MetricsSnapshot {
        let count = self.latency_count.load(Ordering::Relaxed);
        let sum = self.latency_sum_us.load(Ordering::Relaxed);
        let min_raw = self.latency_min_us.load(Ordering::Relaxed);

        MetricsSnapshot {
            decisions_total: self.decisions_total.load(Ordering::Relaxed),
            fallbacks_total: self.fallbacks_total.load(Ordering::Relaxed),
            rejects_total: self.rejects_total.load(Ordering::Relaxed),
            route_latency_us: LatencySnapshot {
                count,
                sum_us: sum,
                avg_us: if count > 0 {
                    sum as f64 / count as f64
                } else {
                    0.0
                },
                min_us: if min_raw < 0 { 0 } else { min_raw as u64 },
                max_us: self.latency_max_us.load(Ordering::Relaxed) as u64,
            },
        }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Point-in-time snapshot of all metrics.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricsSnapshot {
    pub decisions_total: u64,
    pub fallbacks_total: u64,
    pub rejects_total: u64,
    pub route_latency_us: LatencySnapshot,
}

/// Latency histogram summary.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LatencySnapshot {
    pub count: u64,
    pub sum_us: u64,
    pub avg_us: f64,
    pub min_us: u64,
    pub max_us: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_metrics_are_zero() {
        let m = Metrics::new();
        let s = m.snapshot();
        assert_eq!(s.decisions_total, 0);
        assert_eq!(s.fallbacks_total, 0);
        assert_eq!(s.rejects_total, 0);
        assert_eq!(s.route_latency_us.count, 0);
        assert_eq!(s.route_latency_us.avg_us, 0.0);
    }

    #[test]
    fn record_decision_increments_counters() {
        let m = Metrics::new();
        m.record_decision(100, false, false);
        m.record_decision(200, true, false);
        m.record_decision(50, false, true);

        let s = m.snapshot();
        assert_eq!(s.decisions_total, 3);
        assert_eq!(s.fallbacks_total, 1);
        assert_eq!(s.rejects_total, 1);
    }

    #[test]
    fn latency_stats_correct() {
        let m = Metrics::new();
        m.record_decision(100, false, false);
        m.record_decision(300, false, false);
        m.record_decision(200, false, false);

        let s = m.snapshot();
        assert_eq!(s.route_latency_us.count, 3);
        assert_eq!(s.route_latency_us.sum_us, 600);
        assert!((s.route_latency_us.avg_us - 200.0).abs() < 0.01);
        assert_eq!(s.route_latency_us.min_us, 100);
        assert_eq!(s.route_latency_us.max_us, 300);
    }

    #[test]
    fn snapshot_serializes_to_json() {
        let m = Metrics::new();
        m.record_decision(42, false, false);
        let s = m.snapshot();
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["decisions_total"], 1);
        assert_eq!(json["route_latency_us"]["min_us"], 42);
    }

    #[test]
    fn default_impl() {
        let m = Metrics::default();
        assert_eq!(m.snapshot().decisions_total, 0);
    }
}
```

- [ ] **Step 2: Register metrics module in lib.rs**

In `arbiter-mcp/src/lib.rs`, add `pub mod metrics;`:

```rust
pub mod agents;
pub mod config;
pub mod db;
pub mod features;
pub mod metrics;
pub mod server;
pub mod tools;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p arbiter-mcp metrics`
Expected: 5 new tests pass.

- [ ] **Step 4: Commit**

```bash
git add arbiter-mcp/src/metrics.rs arbiter-mcp/src/lib.rs
git commit -m "feat: add in-memory Metrics with atomic counters and latency tracking"
```

---

### Task 2: `get_metrics` MCP Tool

**Files:**
- Create: `arbiter-mcp/src/tools/get_metrics.rs`
- Modify: `arbiter-mcp/src/tools/mod.rs`
- Modify: `arbiter-mcp/src/server.rs`

Wire the metrics snapshot into a new MCP tool.

- [ ] **Step 1: Create `arbiter-mcp/src/tools/get_metrics.rs`**

```rust
//! get_metrics tool implementation.
//!
//! Returns current server metrics: decision counters, latency stats,
//! and fallback/reject rates.

use crate::metrics::Metrics;
use serde_json::Value;

/// Execute the get_metrics logic.
pub fn execute(metrics: &Metrics) -> Value {
    let snapshot = metrics.snapshot();

    let fallback_rate = if snapshot.decisions_total > 0 {
        snapshot.fallbacks_total as f64 / snapshot.decisions_total as f64
    } else {
        0.0
    };
    let reject_rate = if snapshot.decisions_total > 0 {
        snapshot.rejects_total as f64 / snapshot.decisions_total as f64
    } else {
        0.0
    };

    serde_json::json!({
        "decisions_total": snapshot.decisions_total,
        "fallbacks_total": snapshot.fallbacks_total,
        "rejects_total": snapshot.rejects_total,
        "fallback_rate": format!("{:.4}", fallback_rate),
        "reject_rate": format!("{:.4}", reject_rate),
        "route_latency_us": {
            "count": snapshot.route_latency_us.count,
            "avg_us": format!("{:.1}", snapshot.route_latency_us.avg_us),
            "min_us": snapshot.route_latency_us.min_us,
            "max_us": snapshot.route_latency_us.max_us,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_empty_metrics() {
        let m = Metrics::new();
        let result = execute(&m);
        assert_eq!(result["decisions_total"], 0);
        assert_eq!(result["fallback_rate"], "0.0000");
    }

    #[test]
    fn execute_with_data() {
        let m = Metrics::new();
        m.record_decision(100, false, false);
        m.record_decision(200, true, false);

        let result = execute(&m);
        assert_eq!(result["decisions_total"], 2);
        assert_eq!(result["fallbacks_total"], 1);
        assert_eq!(result["fallback_rate"], "0.5000");
        assert_eq!(result["route_latency_us"]["min_us"], 100);
    }
}
```

- [ ] **Step 2: Add module to tools/mod.rs**

```rust
pub mod agent_status;
pub mod get_metrics;
pub mod report_outcome;
pub mod route_task;
```

- [ ] **Step 3: Add `get_metrics` tool schema and handler to `server.rs`**

In the `tool_schemas()` function, add a 4th tool to the `"tools"` array (after the `get_agent_status` entry):

```rust
            {
                "name": "get_metrics",
                "description": "Get current server metrics: decision counts, latency stats, fallback and reject rates.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            }
```

Add `Metrics` to the `McpServer` struct:

```rust
use crate::metrics::Metrics;

pub struct McpServer<'a> {
    config: ArbiterConfig,
    initialized: bool,
    db: &'a Database,
    tree: Option<&'a DecisionTree>,
    registry: AgentRegistry<'a>,
    metrics: &'a Metrics,
}
```

Update the `new()` constructor to accept `metrics: &'a Metrics`.

Add `get_metrics` to the `handle_tools_call` match:

```rust
            "get_metrics" => self.handle_get_metrics(req),
```

Add handler:

```rust
    /// Handle get_metrics: return current server metrics.
    fn handle_get_metrics(&self, req: &JsonRpcRequest) -> JsonRpcResponse {
        debug!("get_metrics called");
        let response_json = crate::tools::get_metrics::execute(self.metrics);
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id.clone(),
            result: Some(serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": response_json.to_string()
                }]
            })),
            error: None,
        }
    }
```

- [ ] **Step 4: Update `main.rs` to create and pass `Metrics`**

In `main()`, before creating the server:

```rust
    let metrics = arbiter_mcp::metrics::Metrics::new();
```

Update the `McpServer::new(...)` call to pass `&metrics`.

- [ ] **Step 5: Fix all existing tests that call `McpServer::new`**

Find all test code that constructs `McpServer` and add a `&Metrics::new()` or `&metrics` parameter. The test helper in `server.rs` tests likely uses `McpServer::new(config, &db, tree.as_ref(), registry)` — add `&Metrics::new()` as the last parameter. Create a local `let metrics = Metrics::new();` in each test setup.

- [ ] **Step 6: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 7: Commit**

```bash
git add arbiter-mcp/src/tools/get_metrics.rs arbiter-mcp/src/tools/mod.rs \
  arbiter-mcp/src/server.rs arbiter-mcp/src/main.rs
git commit -m "feat: add get_metrics MCP tool with decision counters and latency stats"
```

---

### Task 3: Wire Metrics into route_task

**Files:**
- Modify: `arbiter-mcp/src/tools/route_task.rs`
- Modify: `arbiter-mcp/src/server.rs` (pass metrics to execute)

Record metrics on every routing decision.

- [ ] **Step 1: Add `metrics` parameter to `route_task::execute`**

Update the function signature:

```rust
pub fn execute(
    task_id: &str,
    task: &TaskInput,
    constraints: &Constraints,
    tree: Option<&DecisionTree>,
    registry: &AgentRegistry,
    db: &Database,
    invariant_config: &InvariantConfig,
    metrics: &crate::metrics::Metrics,
) -> Result<RouteResult> {
```

- [ ] **Step 2: Add metrics recording at each return point**

Before each `return Ok(result)` and at the final `Ok(result)` in `execute()`, add:

```rust
metrics.record_decision(
    result.inference_us as u64,
    result.action == AgentAction::Fallback,
    result.action == AgentAction::Reject,
);
```

There are 4 return points in `execute()`:
1. "All agents unhealthy" reject (early return ~line 170)
2. "No eligible agents after filtering" reject (early return ~line 225)
3. Successful assign/fallback (inside the loop ~line 353)
4. "All candidates failed critical invariants" reject (final Ok ~line 410)

Add `metrics.record_decision(...)` before each `return Ok(result)` / final `Ok(result)`.

- [ ] **Step 3: Update the server call site**

In `server.rs`, update the `handle_route_task` method's call to `route_task::execute(...)` to pass `self.metrics` as the last argument.

- [ ] **Step 4: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass. Tests in `route_task.rs` that call `execute` directly will need `&Metrics::new()` added.

- [ ] **Step 5: Commit**

```bash
git add arbiter-mcp/src/tools/route_task.rs arbiter-mcp/src/server.rs
git commit -m "feat: record metrics on every routing decision"
```

---

### Task 4: Data Retention

**Files:**
- Modify: `arbiter-mcp/src/db.rs`
- Modify: `arbiter-mcp/src/main.rs`

Add a method to purge old decisions/outcomes and call it on startup.

- [ ] **Step 1: Add `purge_older_than` to Database**

Add to `impl Database` in `db.rs`:

```rust
    /// Delete decisions and outcomes older than `days` days.
    ///
    /// Returns the number of rows deleted (decisions + outcomes).
    pub fn purge_older_than(&self, days: u32) -> Result<usize> {
        let threshold = format!("-{days} days");

        let outcomes_deleted: usize = self
            .conn
            .execute(
                "DELETE FROM outcomes WHERE timestamp < datetime('now', ?1)",
                params![threshold],
            )
            .context("Failed to purge outcomes")?;

        let decisions_deleted: usize = self
            .conn
            .execute(
                "DELETE FROM decisions WHERE timestamp < datetime('now', ?1)",
                params![threshold],
            )
            .context("Failed to purge decisions")?;

        let total = outcomes_deleted + decisions_deleted;
        if total > 0 {
            tracing::info!(
                outcomes = outcomes_deleted,
                decisions = decisions_deleted,
                days = days,
                "purged old records"
            );
        }
        Ok(total)
    }
```

- [ ] **Step 2: Add tests for purge**

Add to `#[cfg(test)] mod tests` in `db.rs`:

```rust
    #[test]
    fn purge_older_than_deletes_old_records() {
        let db = setup_db();
        insert_test_agent(&db, "agent1");

        // Insert a decision and outcome
        let d = DecisionRecord {
            task_id: "old-task".to_string(),
            task_json: "{}".to_string(),
            feature_vector: "[]".to_string(),
            constraints_json: None,
            chosen_agent: "agent1".to_string(),
            action: "assign".to_string(),
            confidence: 0.9,
            decision_path: "[]".to_string(),
            fallback_agent: None,
            fallback_reason: None,
            invariants_json: "[]".to_string(),
            invariants_passed: 10,
            invariants_failed: 0,
            inference_us: 50,
        };
        let dec_id = db.insert_decision(&d).unwrap();

        let o = OutcomeRecord {
            task_id: "old-task".to_string(),
            decision_id: Some(dec_id),
            agent_id: "agent1".to_string(),
            status: "success".to_string(),
            duration_min: Some(5.0),
            tokens_used: None,
            cost_usd: None,
            exit_code: Some(0),
            files_changed: None,
            tests_passed: None,
            validation_passed: None,
            error_summary: None,
            retry_count: 0,
        };
        db.insert_outcome(&o).unwrap();

        // Purge with 0 days — should delete everything
        let deleted = db.purge_older_than(0).unwrap();
        assert!(deleted >= 2, "expected at least 2 deleted, got {deleted}");

        // Verify they're gone
        let found = db.find_decision_id_by_task("old-task").unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn purge_older_than_keeps_recent() {
        let db = setup_db();
        insert_test_agent(&db, "agent1");

        let d = DecisionRecord {
            task_id: "new-task".to_string(),
            task_json: "{}".to_string(),
            feature_vector: "[]".to_string(),
            constraints_json: None,
            chosen_agent: "agent1".to_string(),
            action: "assign".to_string(),
            confidence: 0.9,
            decision_path: "[]".to_string(),
            fallback_agent: None,
            fallback_reason: None,
            invariants_json: "[]".to_string(),
            invariants_passed: 10,
            invariants_failed: 0,
            inference_us: 50,
        };
        db.insert_decision(&d).unwrap();

        // Purge with 30 days — recent record should survive
        let deleted = db.purge_older_than(30).unwrap();
        assert_eq!(deleted, 0);

        let found = db.find_decision_id_by_task("new-task").unwrap();
        assert!(found.is_some());
    }

    #[test]
    fn purge_empty_db_returns_zero() {
        let db = setup_db();
        let deleted = db.purge_older_than(0).unwrap();
        assert_eq!(deleted, 0);
    }
```

- [ ] **Step 3: Call purge on startup in main.rs**

In `main()`, after the database is opened and migrated, add:

```rust
    // Purge records older than 90 days on startup.
    match database.purge_older_than(90) {
        Ok(n) if n > 0 => info!(deleted = n, "startup retention purge"),
        Ok(_) => {}
        Err(e) => eprintln!("WARNING: retention purge failed: {e:#}"),
    }
```

- [ ] **Step 4: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add arbiter-mcp/src/db.rs arbiter-mcp/src/main.rs
git commit -m "feat: add data retention with purge_older_than and 90-day startup cleanup"
```

---

### Task 5: Ping Support

**Files:**
- Modify: `arbiter-mcp/src/server.rs`

Add `ping` method handling. Claude Desktop disconnects if ping has no response.

- [ ] **Step 1: Add ping to dispatch**

In `McpServer::dispatch()`, add a new arm before the catch-all `_`:

```rust
            "ping" => {
                debug!("ping");
                Some(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id.clone(),
                    result: Some(serde_json::json!({})),
                    error: None,
                })
            }
```

- [ ] **Step 2: Add test**

In the `#[cfg(test)] mod tests` in `server.rs`, add:

```rust
    #[test]
    fn ping_returns_empty_object() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let config = test_config();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let metrics = Metrics::new();
        let mut server = McpServer::new(config, &db, None, registry, &metrics);

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(99)),
            method: "ping".to_string(),
            params: None,
        };

        let resp = server.dispatch(&req).unwrap();
        assert!(resp.error.is_none());
        assert_eq!(resp.result, Some(serde_json::json!({})));
    }
```

- [ ] **Step 3: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add arbiter-mcp/src/server.rs
git commit -m "feat: add ping method support for Claude Desktop keepalive"
```

---

### Task 6: Graceful Shutdown (SIGTERM/SIGHUP)

**Files:**
- Modify: `Cargo.toml` (workspace)
- Modify: `arbiter-mcp/Cargo.toml`
- Modify: `arbiter-mcp/src/main.rs`
- Modify: `arbiter-mcp/src/server.rs`

Use `signal-hook` crate with `AtomicBool` flag. The stdio loop checks the flag each iteration.

- [ ] **Step 1: Add `signal-hook` dependency**

In workspace `Cargo.toml`, add to `[workspace.dependencies]`:

```toml
signal-hook = "0.3"
```

In `arbiter-mcp/Cargo.toml`, add to `[dependencies]`:

```toml
signal-hook = { workspace = true }
```

- [ ] **Step 2: Add shutdown flag to server**

In `server.rs`, update `McpServer` to accept a shutdown flag:

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
```

Add field to `McpServer`:

```rust
pub struct McpServer<'a> {
    config: ArbiterConfig,
    initialized: bool,
    db: &'a Database,
    tree: Option<&'a DecisionTree>,
    registry: AgentRegistry<'a>,
    metrics: &'a Metrics,
    shutdown: Arc<AtomicBool>,
}
```

Update `new()` to accept `shutdown: Arc<AtomicBool>`.

In `run()`, check the shutdown flag at the start of each loop iteration:

```rust
        for line in stdin.lock().lines() {
            if self.shutdown.load(Ordering::Relaxed) {
                info!("shutdown signal received, stopping");
                break;
            }
            // ... rest of loop ...
        }
```

- [ ] **Step 3: Register signal handlers in main.rs**

In `main()`, before creating the server:

```rust
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    let shutdown = Arc::new(AtomicBool::new(false));

    // Register SIGTERM and SIGINT handlers.
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        signal_hook::flag::register(sig, Arc::clone(&shutdown))
            .unwrap_or_else(|e| {
                eprintln!("WARNING: failed to register signal handler: {e}");
            });
    }
```

Pass `shutdown` to `McpServer::new(...)`.

Add a shutdown log after `server.run()` returns:

```rust
    info!("server stopped, cleaning up");
```

- [ ] **Step 4: Fix all tests that construct McpServer**

Add `shutdown: Arc::new(AtomicBool::new(false))` to all test `McpServer::new(...)` calls.

- [ ] **Step 5: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml arbiter-mcp/Cargo.toml arbiter-mcp/src/server.rs arbiter-mcp/src/main.rs
git commit -m "feat: add graceful shutdown on SIGTERM/SIGINT via signal-hook"
```

---

## Exit Criteria Checklist

- [ ] `cargo clippy --workspace -- -D warnings` — clean
- [ ] `cargo test --workspace` — all tests pass (275+ tests)
- [ ] `get_metrics` MCP tool returns decisions_total, fallback_rate, latency stats
- [ ] Metrics recorded on every route_task call (assign, fallback, reject)
- [ ] `purge_older_than(days)` deletes old decisions/outcomes
- [ ] 90-day retention purge runs on startup
- [ ] `ping` method returns `{}` (Claude Desktop keepalive)
- [ ] SIGTERM/SIGINT set shutdown flag, server exits cleanly
