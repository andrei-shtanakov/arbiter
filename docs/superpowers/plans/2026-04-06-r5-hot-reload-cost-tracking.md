# R5 — Hot Reload + Cost Tracking

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable live config/tree reload without restart and add real cost tracking with a `get_budget_status` MCP tool.

**Architecture:** Config and tree are wrapped in `Arc<RwLock<>>` so the server can swap them atomically. A file watcher thread (`notify` crate) monitors `config/` and `models/` directories, reloading on change. Cost data is already accumulated in `agent_stats.total_cost_usd` — we add a new DB query to aggregate it and expose via `get_budget_status`. The budget invariant is enhanced to use real spend instead of per-task estimates.

**Tech Stack:** Rust (notify 6.x for file watching, std::sync::{Arc, RwLock})

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `arbiter-mcp/src/watcher.rs` | File watcher: monitors config/tree files, triggers reload |
| Create | `arbiter-mcp/src/tools/get_budget.rs` | `get_budget_status` MCP tool handler |
| Modify | `arbiter-mcp/src/server.rs` | Switch from `&'a` borrows to `Arc<RwLock>` for config/tree/registry |
| Modify | `arbiter-mcp/src/agents.rs` | Remove lifetime parameter, accept owned data |
| Modify | `arbiter-mcp/src/main.rs` | Set up file watcher thread, create `Arc` wrappers |
| Modify | `arbiter-mcp/src/db.rs` | Add `get_total_cost` and `get_cost_by_agent` queries |
| Modify | `arbiter-mcp/src/tools/mod.rs` | Export new tool module |
| Modify | `arbiter-mcp/Cargo.toml` | Add `notify` dep |
| Modify | `Cargo.toml` (workspace) | Add `notify` to workspace deps |

---

### Task 1: Switch McpServer from Borrows to Arc

**Files:**
- Modify: `arbiter-mcp/src/server.rs`
- Modify: `arbiter-mcp/src/agents.rs`
- Modify: `arbiter-mcp/src/main.rs`

This is the foundational refactor. Currently `McpServer<'a>` borrows config, db, tree, registry, and metrics. For hot reload, config/tree/registry need to be swappable. We wrap them in `Arc<RwLock<>>`.

**Strategy:** Change `McpServer` to own `Arc` values instead of references. Database and metrics stay as references (they don't change). Config, tree, and registry become `Arc<RwLock<>>`.

- [ ] **Step 1: Update AgentRegistry to remove lifetime**

In `arbiter-mcp/src/agents.rs`, change `AgentRegistry<'a>` to own its Database via `Arc`:

```rust
use std::sync::Arc;

pub struct AgentRegistry {
    db: Arc<Database>,
    configs: HashMap<String, AgentConfig>,
    cache: RefCell<HashMap<String, CachedAgentInfo>>,
}

impl AgentRegistry {
    pub fn new(db: Arc<Database>, agents: &HashMap<String, AgentConfig>) -> Result<Self> {
        // ... same body, just use db.as_ref() or &*db where needed ...
        Ok(Self {
            db,
            configs: agents.clone(),
            cache: RefCell::new(HashMap::new()),
        })
    }
    // Update all methods: self.db.method() stays the same since Arc<T> derefs to T
}
```

The methods stay the same — `Arc<Database>` auto-derefs to `&Database`.

- [ ] **Step 2: Update McpServer to use Arc<RwLock<>> for hot-reloadable state**

In `server.rs`:

```rust
use std::sync::{Arc, RwLock};

pub struct McpServer {
    config: Arc<RwLock<ArbiterConfig>>,
    initialized: bool,
    db: Arc<Database>,
    tree: Arc<RwLock<Option<DecisionTree>>>,
    registry: Arc<RwLock<AgentRegistry>>,
    metrics: Arc<Metrics>,
    shutdown: Arc<AtomicBool>,
}

impl McpServer {
    pub fn new(
        config: Arc<RwLock<ArbiterConfig>>,
        db: Arc<Database>,
        tree: Arc<RwLock<Option<DecisionTree>>>,
        registry: Arc<RwLock<AgentRegistry>>,
        metrics: Arc<Metrics>,
        shutdown: Arc<AtomicBool>,
    ) -> Self { ... }
}
```

In each handler, acquire read locks as needed:

```rust
fn handle_route_task(&self, req: &JsonRpcRequest, arguments: Option<&Value>) -> JsonRpcResponse {
    // ...validation...
    let config = self.config.read().unwrap();
    let tree_guard = self.tree.read().unwrap();
    let registry = self.registry.read().unwrap();
    match route_task::execute(
        task_id, &task, &constraints,
        tree_guard.as_ref(), &registry, &self.db,
        &config.invariants, &self.metrics,
    ) { ... }
}
```

Similarly for `handle_report_outcome` and `handle_get_agent_status`.

- [ ] **Step 3: Update main.rs to create Arc wrappers**

```rust
let config = Arc::new(RwLock::new(config));
let database = Arc::new(database);
let tree = Arc::new(RwLock::new(tree));
let metrics = Arc::new(arbiter_mcp::metrics::Metrics::new());

let registry = match agents::AgentRegistry::new(
    Arc::clone(&database),
    &config.read().unwrap().agents,
) { ... };
let registry = Arc::new(RwLock::new(registry));

let mut server = server::McpServer::new(
    Arc::clone(&config),
    Arc::clone(&database),
    Arc::clone(&tree),
    Arc::clone(&registry),
    Arc::clone(&metrics),
    shutdown,
);
```

- [ ] **Step 4: Fix ALL tests across the codebase**

Every test that creates `McpServer::new(...)`, `AgentRegistry::new(...)`, or references their types needs updating. This affects:
- `server.rs` tests (~20 tests)
- `agents.rs` tests (~10 tests)
- `route_task.rs` tests (if any call execute directly)
- Integration tests in `tests/integration.rs`
- Benchmark code in `arbiter-cli`

For `AgentRegistry` tests: wrap `db` in `Arc::new()`.
For `McpServer` tests: wrap config in `Arc::new(RwLock::new())`, tree in `Arc::new(RwLock::new(Some(tree)))` or `Arc::new(RwLock::new(None))`, etc.

- [ ] **Step 5: Run all tests**

Run: `cargo test --workspace`
Expected: All 275 tests pass.

- [ ] **Step 6: Commit**

```bash
git commit -am "refactor: switch McpServer and AgentRegistry from borrows to Arc<RwLock>"
```

---

### Task 2: File Watcher for Hot Reload

**Files:**
- Create: `arbiter-mcp/src/watcher.rs`
- Modify: `arbiter-mcp/src/lib.rs` (add `pub mod watcher;`)
- Modify: `arbiter-mcp/src/main.rs`
- Modify: `arbiter-mcp/Cargo.toml`
- Modify: `Cargo.toml` (workspace)

A background thread watches `config/` and `models/` for changes, reloads config/tree/registry when files change.

- [ ] **Step 1: Add `notify` dependency**

In workspace `Cargo.toml` `[workspace.dependencies]`:
```toml
notify = { version = "6", default-features = false, features = ["macos_fsevent"] }
```

In `arbiter-mcp/Cargo.toml` `[dependencies]`:
```toml
notify = { workspace = true }
```

- [ ] **Step 2: Create `arbiter-mcp/src/watcher.rs`**

```rust
//! File watcher for hot-reloading config and decision tree.
//!
//! Spawns a background thread that monitors config and model files.
//! On change, reloads the file and swaps the shared state behind
//! `Arc<RwLock<>>`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, Result};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tracing::{error, info, warn};

use arbiter_core::policy::decision_tree::DecisionTree;

use crate::agents::AgentRegistry;
use crate::config::{load_config, ArbiterConfig};
use crate::db::Database;

/// Paths the watcher monitors.
pub struct WatchPaths {
    pub config_dir: PathBuf,
    pub tree_path: PathBuf,
}

/// Shared state that can be hot-reloaded.
pub struct ReloadableState {
    pub config: Arc<RwLock<ArbiterConfig>>,
    pub tree: Arc<RwLock<Option<DecisionTree>>>,
    pub registry: Arc<RwLock<AgentRegistry>>,
    pub db: Arc<Database>,
}

/// Start the file watcher in a background thread.
///
/// Returns the watcher handle (must be kept alive — dropping it stops watching).
pub fn start_watcher(
    paths: WatchPaths,
    state: ReloadableState,
) -> Result<RecommendedWatcher> {
    let config_dir = paths.config_dir.clone();
    let tree_path = paths.tree_path.clone();

    let mut watcher = notify::recommended_watcher(
        move |res: std::result::Result<notify::Event, notify::Error>| {
            let event = match res {
                Ok(e) => e,
                Err(e) => {
                    warn!("watcher error: {e}");
                    return;
                }
            };

            // Only react to create/modify events.
            match event.kind {
                EventKind::Create(_) | EventKind::Modify(_) => {}
                _ => return,
            }

            for path in &event.paths {
                if path_matches_config(path, &config_dir) {
                    info!(path = %path.display(), "config file changed, reloading");
                    if let Err(e) = reload_config(
                        &config_dir,
                        &state.config,
                        &state.registry,
                        &state.db,
                    ) {
                        error!("config reload failed: {e:#}");
                    }
                } else if path_matches_tree(path, &tree_path) {
                    info!(path = %path.display(), "tree file changed, reloading");
                    if let Err(e) = reload_tree(&tree_path, &state.tree) {
                        error!("tree reload failed: {e:#}");
                    }
                }
            }
        },
    )
    .context("failed to create file watcher")?;

    watcher
        .watch(&paths.config_dir, RecursiveMode::NonRecursive)
        .with_context(|| {
            format!(
                "failed to watch config dir: {}",
                paths.config_dir.display()
            )
        })?;

    // Watch the parent directory of the tree file.
    if let Some(parent) = paths.tree_path.parent() {
        watcher
            .watch(parent, RecursiveMode::NonRecursive)
            .with_context(|| {
                format!("failed to watch tree dir: {}", parent.display())
            })?;
    }

    info!(
        config = %paths.config_dir.display(),
        tree = %paths.tree_path.display(),
        "file watcher started"
    );

    Ok(watcher)
}

/// Check if a changed path is a config file.
fn path_matches_config(path: &Path, config_dir: &Path) -> bool {
    if let Some(parent) = path.parent() {
        if parent == config_dir {
            if let Some(ext) = path.extension() {
                return ext == "toml";
            }
        }
    }
    false
}

/// Check if a changed path is the tree file.
fn path_matches_tree(path: &Path, tree_path: &Path) -> bool {
    path == tree_path
}

/// Reload config from disk and update shared state.
fn reload_config(
    config_dir: &Path,
    config: &Arc<RwLock<ArbiterConfig>>,
    registry: &Arc<RwLock<AgentRegistry>>,
    db: &Arc<Database>,
) -> Result<()> {
    let new_config = load_config(config_dir)?;
    let new_registry =
        AgentRegistry::new(Arc::clone(db), &new_config.agents)?;

    info!(
        agents = new_config.agents.len(),
        "config reloaded successfully"
    );

    *config.write().unwrap() = new_config;
    *registry.write().unwrap() = new_registry;

    Ok(())
}

/// Reload decision tree from disk and update shared state.
fn reload_tree(
    tree_path: &Path,
    tree: &Arc<RwLock<Option<DecisionTree>>>,
) -> Result<()> {
    let json = std::fs::read_to_string(tree_path)
        .with_context(|| format!("failed to read {}", tree_path.display()))?;
    let new_tree = DecisionTree::from_json(&json)
        .map_err(|e| anyhow::anyhow!("failed to parse tree: {e}"))?;

    info!(
        nodes = new_tree.node_count(),
        depth = new_tree.depth(),
        "tree reloaded successfully"
    );

    *tree.write().unwrap() = Some(new_tree);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn path_matches_config_toml_in_dir() {
        let dir = PathBuf::from("/tmp/config");
        assert!(path_matches_config(
            &PathBuf::from("/tmp/config/agents.toml"),
            &dir,
        ));
        assert!(path_matches_config(
            &PathBuf::from("/tmp/config/invariants.toml"),
            &dir,
        ));
    }

    #[test]
    fn path_matches_config_rejects_non_toml() {
        let dir = PathBuf::from("/tmp/config");
        assert!(!path_matches_config(
            &PathBuf::from("/tmp/config/readme.md"),
            &dir,
        ));
    }

    #[test]
    fn path_matches_config_rejects_wrong_dir() {
        let dir = PathBuf::from("/tmp/config");
        assert!(!path_matches_config(
            &PathBuf::from("/other/agents.toml"),
            &dir,
        ));
    }

    #[test]
    fn path_matches_tree_exact() {
        let tree = PathBuf::from("/tmp/models/tree.json");
        assert!(path_matches_tree(&tree, &tree));
    }

    #[test]
    fn path_matches_tree_rejects_other() {
        let tree = PathBuf::from("/tmp/models/tree.json");
        assert!(!path_matches_tree(
            &PathBuf::from("/tmp/models/other.json"),
            &tree,
        ));
    }
}
```

- [ ] **Step 3: Register watcher module in lib.rs**

Add `pub mod watcher;` to `arbiter-mcp/src/lib.rs`.

- [ ] **Step 4: Wire file watcher into main.rs**

After creating the server (before `server.run()`), add:

```rust
    // Start file watcher for hot reload.
    let _watcher = match arbiter_mcp::watcher::start_watcher(
        arbiter_mcp::watcher::WatchPaths {
            config_dir: args.config_dir.clone(),
            tree_path: args.tree.clone(),
        },
        arbiter_mcp::watcher::ReloadableState {
            config: Arc::clone(&config),
            tree: Arc::clone(&tree),
            registry: Arc::clone(&registry),
            db: Arc::clone(&database),
        },
    ) {
        Ok(w) => Some(w),
        Err(e) => {
            eprintln!("WARNING: file watcher failed to start: {e:#}. Hot reload disabled.");
            None
        }
    };
```

The `_watcher` must stay in scope (dropping it stops watching).

- [ ] **Step 5: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git commit -am "feat: add file watcher for hot-reloading config and decision tree"
```

---

### Task 3: Cost Tracking Queries

**Files:**
- Modify: `arbiter-mcp/src/db.rs`

Add two DB queries for cost aggregation that the budget tool will use.

- [ ] **Step 1: Add `get_total_cost` to Database**

```rust
    /// Get total cost across all agents and task types.
    pub fn get_total_cost(&self) -> Result<f64> {
        let cost: f64 = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(total_cost_usd), 0.0) FROM agent_stats",
                [],
                |row| row.get(0),
            )
            .context("Failed to get total cost")?;
        Ok(cost)
    }
```

- [ ] **Step 2: Add `get_cost_by_agent` to Database**

```rust
    /// Get total cost per agent.
    ///
    /// Returns a vec of `(agent_id, total_cost_usd, total_tasks)`.
    pub fn get_cost_by_agent(&self) -> Result<Vec<(String, f64, i64)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT agent_id,
                    COALESCE(SUM(total_cost_usd), 0.0),
                    COALESCE(SUM(total_tasks), 0)
             FROM agent_stats
             GROUP BY agent_id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("Failed to query cost by agent")?;
        Ok(rows)
    }
```

- [ ] **Step 3: Add tests**

```rust
    #[test]
    fn get_total_cost_empty() {
        let db = setup_db();
        assert_eq!(db.get_total_cost().unwrap(), 0.0);
    }

    #[test]
    fn get_total_cost_with_data() {
        let db = setup_db();
        insert_test_agent(&db, "a1");
        insert_test_agent(&db, "a2");

        let o1 = OutcomeRecord {
            task_id: "t1".to_string(),
            decision_id: None,
            agent_id: "a1".to_string(),
            status: "success".to_string(),
            duration_min: Some(10.0),
            tokens_used: None,
            cost_usd: Some(0.50),
            exit_code: Some(0),
            files_changed: None,
            tests_passed: None,
            validation_passed: None,
            error_summary: None,
            retry_count: 0,
        };
        db.insert_outcome(&o1).unwrap();
        db.update_agent_stats("a1", "feature", "python", &o1).unwrap();

        let o2 = OutcomeRecord {
            task_id: "t2".to_string(),
            decision_id: None,
            agent_id: "a2".to_string(),
            status: "success".to_string(),
            duration_min: Some(5.0),
            tokens_used: None,
            cost_usd: Some(0.25),
            exit_code: Some(0),
            files_changed: None,
            tests_passed: None,
            validation_passed: None,
            error_summary: None,
            retry_count: 0,
        };
        db.insert_outcome(&o2).unwrap();
        db.update_agent_stats("a2", "bugfix", "rust", &o2).unwrap();

        let total = db.get_total_cost().unwrap();
        assert!((total - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn get_cost_by_agent_returns_per_agent() {
        let db = setup_db();
        insert_test_agent(&db, "a1");

        let o = OutcomeRecord {
            task_id: "t1".to_string(),
            decision_id: None,
            agent_id: "a1".to_string(),
            status: "success".to_string(),
            duration_min: Some(10.0),
            tokens_used: None,
            cost_usd: Some(0.30),
            exit_code: Some(0),
            files_changed: None,
            tests_passed: None,
            validation_passed: None,
            error_summary: None,
            retry_count: 0,
        };
        db.insert_outcome(&o).unwrap();
        db.update_agent_stats("a1", "feature", "python", &o).unwrap();

        let by_agent = db.get_cost_by_agent().unwrap();
        assert_eq!(by_agent.len(), 1);
        assert_eq!(by_agent[0].0, "a1");
        assert!((by_agent[0].1 - 0.30).abs() < f64::EPSILON);
        assert_eq!(by_agent[0].2, 1);
    }
```

- [ ] **Step 4: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add arbiter-mcp/src/db.rs
git commit -m "feat: add get_total_cost and get_cost_by_agent DB queries"
```

---

### Task 4: `get_budget_status` MCP Tool

**Files:**
- Create: `arbiter-mcp/src/tools/get_budget.rs`
- Modify: `arbiter-mcp/src/tools/mod.rs`
- Modify: `arbiter-mcp/src/server.rs`

New MCP tool that returns budget overview: total spent, limit, by-agent breakdown.

- [ ] **Step 1: Create `arbiter-mcp/src/tools/get_budget.rs`**

```rust
//! get_budget_status tool implementation.
//!
//! Returns budget overview: total spent, budget limit, remaining,
//! and per-agent cost breakdown.

use anyhow::Result;
use serde_json::Value;

use crate::config::ArbiterConfig;
use crate::db::Database;

/// Execute the get_budget_status logic.
pub fn execute(db: &Database, config: &ArbiterConfig) -> Result<Value> {
    let total_spent = db.get_total_cost()?;
    let budget_limit = config.invariants.budget.threshold_usd;
    let remaining = budget_limit - total_spent;

    let by_agent = db.get_cost_by_agent()?;
    let agents_json: Vec<Value> = by_agent
        .iter()
        .map(|(agent_id, cost, tasks)| {
            serde_json::json!({
                "agent_id": agent_id,
                "total_cost_usd": format!("{:.2}", cost),
                "total_tasks": tasks,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "budget_limit_usd": format!("{:.2}", budget_limit),
        "total_spent_usd": format!("{:.2}", total_spent),
        "remaining_usd": format!("{:.2}", remaining),
        "over_budget": total_spent > budget_limit,
        "by_agent": agents_json,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;
    use crate::db::OutcomeRecord;
    use std::collections::HashMap;

    fn test_config() -> ArbiterConfig {
        let mut agents = HashMap::new();
        agents.insert(
            "a1".to_string(),
            AgentConfig {
                display_name: "Agent 1".to_string(),
                supports_languages: vec!["python".to_string()],
                supports_types: vec!["bugfix".to_string()],
                max_concurrent: 2,
                cost_per_hour: 0.30,
                avg_duration_min: 18.0,
            },
        );
        ArbiterConfig {
            agents,
            invariants: InvariantConfig {
                budget: BudgetConfig {
                    threshold_usd: 10.0,
                },
                retries: RetriesConfig { max_retries: 3 },
                rate_limit: RateLimitConfig {
                    calls_per_minute: 60,
                },
                agent_health: AgentHealthConfig {
                    max_failures_24h: 5,
                },
                concurrency: ConcurrencyConfig {
                    max_total_concurrent: 5,
                },
                sla: SlaConfig {
                    buffer_multiplier: 1.5,
                },
            },
        }
    }

    #[test]
    fn budget_status_empty() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let config = test_config();

        let result = execute(&db, &config).unwrap();
        assert_eq!(result["total_spent_usd"], "0.00");
        assert_eq!(result["budget_limit_usd"], "10.00");
        assert_eq!(result["remaining_usd"], "10.00");
        assert_eq!(result["over_budget"], false);
        assert_eq!(result["by_agent"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn budget_status_with_spend() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let config = test_config();

        db.upsert_agent("a1", "Agent 1", 2, "{}").unwrap();
        let o = OutcomeRecord {
            task_id: "t1".to_string(),
            decision_id: None,
            agent_id: "a1".to_string(),
            status: "success".to_string(),
            duration_min: Some(10.0),
            tokens_used: None,
            cost_usd: Some(3.50),
            exit_code: Some(0),
            files_changed: None,
            tests_passed: None,
            validation_passed: None,
            error_summary: None,
            retry_count: 0,
        };
        db.insert_outcome(&o).unwrap();
        db.update_agent_stats("a1", "bugfix", "python", &o).unwrap();

        let result = execute(&db, &config).unwrap();
        assert_eq!(result["total_spent_usd"], "3.50");
        assert_eq!(result["remaining_usd"], "6.50");
        assert_eq!(result["over_budget"], false);
        assert_eq!(result["by_agent"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn budget_status_over_budget() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let config = test_config();

        db.upsert_agent("a1", "Agent 1", 2, "{}").unwrap();
        let o = OutcomeRecord {
            task_id: "t1".to_string(),
            decision_id: None,
            agent_id: "a1".to_string(),
            status: "success".to_string(),
            duration_min: Some(60.0),
            tokens_used: None,
            cost_usd: Some(15.00),
            exit_code: Some(0),
            files_changed: None,
            tests_passed: None,
            validation_passed: None,
            error_summary: None,
            retry_count: 0,
        };
        db.insert_outcome(&o).unwrap();
        db.update_agent_stats("a1", "bugfix", "python", &o).unwrap();

        let result = execute(&db, &config).unwrap();
        assert_eq!(result["over_budget"], true);
    }
}
```

- [ ] **Step 2: Register module and add tool schema**

In `tools/mod.rs`:
```rust
pub mod agent_status;
pub mod get_budget;
pub mod get_metrics;
pub mod report_outcome;
pub mod route_task;
```

In `server.rs` `tool_schemas()`, add 5th tool to the array:
```json
{
    "name": "get_budget_status",
    "description": "Get budget overview: total spent, budget limit, remaining amount, and per-agent cost breakdown.",
    "inputSchema": {
        "type": "object",
        "properties": {}
    }
}
```

Add to `handle_tools_call` match:
```rust
"get_budget_status" => self.handle_get_budget_status(req),
```

Add handler:
```rust
    fn handle_get_budget_status(&self, req: &JsonRpcRequest) -> JsonRpcResponse {
        debug!("get_budget_status called");
        let config = self.config.read().unwrap();
        match crate::tools::get_budget::execute(&self.db, &config) {
            Ok(response_json) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id.clone(),
                result: Some(serde_json::json!({
                    "content": [{"type": "text", "text": response_json.to_string()}]
                })),
                error: None,
            },
            Err(e) => {
                error!("get_budget_status failed: {e:#}");
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id.clone(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!("{e}"),
                        data: None,
                    }),
                }
            }
        }
    }
```

- [ ] **Step 3: Update tools/list test assertions**

In server.rs tests, update any test that checks tools count from 4 to 5.

- [ ] **Step 4: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git commit -am "feat: add get_budget_status MCP tool with per-agent cost breakdown"
```

---

## Exit Criteria Checklist

- [ ] `cargo clippy --workspace -- -D warnings` — clean
- [ ] `cargo test --workspace` — all tests pass (285+ tests)
- [ ] Config changes in `config/agents.toml` or `config/invariants.toml` picked up within <2s
- [ ] Tree changes in `models/agent_policy_tree.json` picked up within <2s
- [ ] `get_budget_status` returns total_spent, remaining, by_agent breakdown
- [ ] File watcher errors are non-fatal (logged, server continues)
- [ ] Reload errors are non-fatal (logged, old state preserved)
