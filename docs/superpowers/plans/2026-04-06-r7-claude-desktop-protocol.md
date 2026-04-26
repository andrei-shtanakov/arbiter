# R7 — Claude Desktop + Protocol Compliance

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Arbiter work stably as an MCP server in Claude Desktop with hardened JSON-RPC protocol handling and a file-based golden test regression suite.

**Architecture:** Protocol hardening adds jsonrpc version validation and line length limits (OOM protection) to the server's stdio loop. Golden tests use `.jsonl` fixture files (one request/response pair per test) that are loaded by a Rust test harness, dispatched through `McpServer::dispatch()`, and compared against expected output. An MCP config template enables one-paste Claude Desktop setup.

**Tech Stack:** Rust (serde_json, existing MCP server), JSON fixture files

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Modify | `arbiter-mcp/src/server.rs` | JSON-RPC validation, line length limit |
| Create | `arbiter-mcp/tests/golden/` | Directory for golden test fixtures |
| Create | `arbiter-mcp/tests/golden_tests.rs` | Test harness loading fixtures |
| Create | `arbiter-mcp/tests/golden/*.jsonl` | 12 fixture files (request/response pairs) |
| Create | `config/claude_desktop_config.json` | MCP config template for Claude Desktop |

---

### Task 1: JSON-RPC Protocol Hardening

**Files:**
- Modify: `arbiter-mcp/src/server.rs`

Add two protocol protections:
1. Validate `jsonrpc: "2.0"` field on every request
2. Line length limit (1MB) to prevent OOM from malformed input

- [ ] **Step 1: Add line length constant**

At the top of `server.rs` (near other constants):

```rust
/// Maximum line length in bytes (1 MB). Lines longer than this are rejected
/// to prevent out-of-memory from malformed input.
const MAX_LINE_LENGTH: usize = 1_048_576;
```

- [ ] **Step 2: Add line length check in `run()`**

In the `run()` method, after `let line = line.trim().to_string();` and before parsing JSON, add:

```rust
            if line.len() > MAX_LINE_LENGTH {
                warn!(len = line.len(), "line exceeds maximum length, rejecting");
                let resp = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: None,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32600,
                        message: format!(
                            "Request too large: {} bytes (max {})",
                            line.len(),
                            MAX_LINE_LENGTH
                        ),
                        data: None,
                    }),
                };
                write_response(&mut stdout, &resp)?;
                continue;
            }
```

- [ ] **Step 3: Add jsonrpc version validation in `dispatch()`**

At the top of `dispatch()`, before the method match:

```rust
        // Validate JSON-RPC version
        if req.jsonrpc != "2.0" {
            return Some(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id.clone(),
                result: None,
                error: Some(JsonRpcError {
                    code: -32600,
                    message: format!(
                        "Invalid JSON-RPC version: expected \"2.0\", got {:?}",
                        req.jsonrpc
                    ),
                    data: None,
                }),
            });
        }
```

- [ ] **Step 4: Add tests**

In `server.rs` `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn invalid_jsonrpc_version_rejected() {
        let (db, tree, metrics, config) = make_deps();
        let mut server = make_server(&db, Some(&tree), &metrics, config);
        let req = JsonRpcRequest {
            jsonrpc: "1.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "tools/list".to_string(),
            params: None,
        };
        let resp = server.dispatch(&req).unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32600);
    }
```

- [ ] **Step 5: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add arbiter-mcp/src/server.rs
git commit -m "feat: add JSON-RPC version validation and line length limit"
```

---

### Task 2: Golden Tests

**Files:**
- Create: `arbiter-mcp/tests/golden/` (directory)
- Create: `arbiter-mcp/tests/golden_tests.rs` (test harness)
- Create: 12 fixture files in `arbiter-mcp/tests/golden/`

Each `.jsonl` fixture has exactly 2 lines: the JSON-RPC request and the expected response pattern. The test harness parses both, dispatches the request through `McpServer::dispatch()`, and verifies the response matches.

- [ ] **Step 1: Create the fixture directory and files**

Create `arbiter-mcp/tests/golden/` directory.

Create these 12 fixture files (each file has 2 lines: request, then expected response):

**`01_initialize.jsonl`:**
```
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}
{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"arbiter"}}}
```

**`02_initialized.jsonl`:**
```
{"jsonrpc":"2.0","method":"notifications/initialized"}
null
```

**`03_tools_list.jsonl`:**
```
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
{"jsonrpc":"2.0","id":2,"result":{"tools":"__ARRAY_LENGTH_5__"}}
```

**`04_ping.jsonl`:**
```
{"jsonrpc":"2.0","id":3,"method":"ping"}
{"jsonrpc":"2.0","id":3,"result":{}}
```

**`05_route_task_valid.jsonl`:**
```
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"route_task","arguments":{"task_id":"golden-1","task":{"type":"bugfix","language":"python","complexity":"simple","priority":"normal"}}}}
{"jsonrpc":"2.0","id":4,"result":{"content":"__HAS_TEXT__"}}
```

**`06_route_task_missing_task_id.jsonl`:**
```
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"route_task","arguments":{"task":{}}}}
{"jsonrpc":"2.0","id":5,"error":{"code":-32602,"message":"__CONTAINS__task_id"}}
```

**`07_route_task_missing_task.jsonl`:**
```
{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"route_task","arguments":{"task_id":"t1"}}}
{"jsonrpc":"2.0","id":6,"error":{"code":-32602,"message":"__CONTAINS__task"}}
```

**`08_report_outcome_valid.jsonl`:**
```
{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"report_outcome","arguments":{"task_id":"golden-1","agent_id":"claude_code","status":"success","duration_min":5.0}}}
{"jsonrpc":"2.0","id":7,"result":{"content":"__HAS_TEXT__"}}
```

**`09_report_outcome_invalid_status.jsonl`:**
```
{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"report_outcome","arguments":{"task_id":"t1","agent_id":"claude_code","status":"exploded"}}}
{"jsonrpc":"2.0","id":8,"error":{"code":-32000}}
```

**`10_get_agent_status.jsonl`:**
```
{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"get_agent_status","arguments":{}}}
{"jsonrpc":"2.0","id":9,"result":{"content":"__HAS_TEXT__"}}
```

**`11_get_metrics.jsonl`:**
```
{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"get_metrics","arguments":{}}}
{"jsonrpc":"2.0","id":10,"result":{"content":"__HAS_TEXT__"}}
```

**`12_unknown_method.jsonl`:**
```
{"jsonrpc":"2.0","id":11,"method":"nonexistent/method"}
{"jsonrpc":"2.0","id":11,"error":{"code":-32601}}
```

- [ ] **Step 2: Create the test harness**

Create `arbiter-mcp/tests/golden_tests.rs`:

```rust
//! Golden tests: file-based JSON-RPC protocol regression suite.
//!
//! Each `.jsonl` file in `tests/golden/` contains exactly 2 lines:
//! 1. The JSON-RPC request to send
//! 2. The expected response (with pattern matchers)
//!
//! Pattern matchers in expected responses:
//! - `"__HAS_TEXT__"` — result.content array has at least one text entry
//! - `"__ARRAY_LENGTH_N__"` — value is an array of length N
//! - `"__CONTAINS__X"` — string contains X
//! - `null` — expect no response (notification)
//! - Other values — exact match

#![allow(clippy::arc_with_non_send_sync)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};

use arbiter_core::policy::decision_tree::DecisionTree;
use arbiter_mcp::agents::AgentRegistry;
use arbiter_mcp::config::*;
use arbiter_mcp::db::Database;
use arbiter_mcp::metrics::Metrics;
use arbiter_mcp::server::{JsonRpcRequest, McpServer};

fn load_tree() -> DecisionTree {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = std::path::Path::new(manifest_dir)
        .parent()
        .unwrap()
        .join("models/agent_policy_tree.json");
    let json = std::fs::read_to_string(&path).unwrap();
    DecisionTree::from_json(&json).unwrap()
}

fn test_config() -> ArbiterConfig {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let config_dir = std::path::Path::new(manifest_dir)
        .parent()
        .unwrap()
        .join("config");
    load_config(&config_dir).unwrap()
}

fn make_server(
    db: &Arc<Database>,
    tree: &DecisionTree,
    metrics: &Arc<Metrics>,
    config: ArbiterConfig,
) -> McpServer {
    let registry = AgentRegistry::new(
        Arc::clone(db),
        &config.agents,
    )
    .unwrap();
    McpServer::new(
        Arc::new(RwLock::new(config)),
        Arc::clone(db),
        Arc::new(RwLock::new(Some(tree.clone()))),
        Arc::new(RwLock::new(registry)),
        Arc::clone(metrics),
        Arc::new(AtomicBool::new(false)),
    )
}

/// Check if actual response matches expected pattern.
fn matches_pattern(
    actual: &serde_json::Value,
    expected: &serde_json::Value,
) -> bool {
    match (actual, expected) {
        (_, serde_json::Value::String(s)) if s == "__HAS_TEXT__" => {
            // Check that actual.content is an array with text
            actual
                .get("content")
                .and_then(|c| c.as_array())
                .map(|arr| !arr.is_empty())
                .unwrap_or(false)
        }
        (_, serde_json::Value::String(s)) if s.starts_with("__ARRAY_LENGTH_") => {
            let n: usize = s
                .trim_start_matches("__ARRAY_LENGTH_")
                .trim_end_matches("__")
                .parse()
                .unwrap_or(0);
            actual.as_array().map(|a| a.len() == n).unwrap_or(false)
        }
        (_, serde_json::Value::String(s)) if s.starts_with("__CONTAINS__") => {
            let needle = s.trim_start_matches("__CONTAINS__");
            actual
                .as_str()
                .map(|a| a.contains(needle))
                .unwrap_or(false)
        }
        (serde_json::Value::Object(a), serde_json::Value::Object(e)) => {
            // All expected keys must match (actual may have extra keys)
            e.iter().all(|(k, v)| {
                a.get(k)
                    .map(|av| matches_pattern(av, v))
                    .unwrap_or(false)
            })
        }
        (serde_json::Value::Array(a), serde_json::Value::Array(e)) => {
            a.len() == e.len()
                && a.iter()
                    .zip(e.iter())
                    .all(|(av, ev)| matches_pattern(av, ev))
        }
        _ => actual == expected,
    }
}

fn golden_dir() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).join("tests/golden")
}

#[test]
fn run_golden_tests() {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    let db = Arc::new(db);
    let tree = load_tree();
    let metrics = Arc::new(Metrics::new());
    let config = test_config();
    let mut server = make_server(&db, &tree, &metrics, config);

    let dir = golden_dir();
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("Failed to read {}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "jsonl")
                .unwrap_or(false)
        })
        .collect();
    files.sort_by_key(|e| e.file_name());

    assert!(
        !files.is_empty(),
        "No golden test files found in {}",
        dir.display()
    );

    let mut passed = 0;
    let mut failed = 0;

    for entry in &files {
        let path = entry.path();
        let name = path.file_name().unwrap().to_string_lossy();
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();

        assert!(
            lines.len() >= 2,
            "Golden file {name} must have at least 2 lines"
        );

        let request: JsonRpcRequest =
            serde_json::from_str(lines[0]).unwrap_or_else(|e| {
                panic!("Failed to parse request in {name}: {e}")
            });

        let expected_line = lines[1].trim();

        let response = server.dispatch(&request);

        if expected_line == "null" {
            // Expect no response (notification)
            if response.is_none() {
                passed += 1;
            } else {
                eprintln!("FAIL {name}: expected no response, got one");
                failed += 1;
            }
            continue;
        }

        let resp = match response {
            Some(r) => r,
            None => {
                eprintln!("FAIL {name}: expected response, got None");
                failed += 1;
                continue;
            }
        };

        let actual: serde_json::Value =
            serde_json::to_value(&resp).unwrap();
        let expected: serde_json::Value =
            serde_json::from_str(expected_line).unwrap_or_else(|e| {
                panic!("Failed to parse expected in {name}: {e}")
            });

        if matches_pattern(&actual, &expected) {
            passed += 1;
        } else {
            eprintln!("FAIL {name}:");
            eprintln!("  expected: {expected}");
            eprintln!("  actual:   {actual}");
            failed += 1;
        }
    }

    eprintln!(
        "\nGolden tests: {passed} passed, {failed} failed, {} total",
        passed + failed
    );
    assert_eq!(failed, 0, "{failed} golden tests failed");
}
```

- [ ] **Step 3: Run golden tests**

Run: `cargo test -p arbiter-mcp golden`
Expected: 12 golden tests pass.

- [ ] **Step 4: Run full test suite**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add arbiter-mcp/tests/golden/ arbiter-mcp/tests/golden_tests.rs
git commit -m "feat: add golden test regression suite with 12 protocol fixtures"
```

---

### Task 3: Claude Desktop Config Template

**Files:**
- Create: `config/claude_desktop_config.json`

Provide a ready-to-paste config snippet for Claude Desktop.

- [ ] **Step 1: Create the config template**

Create `config/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "arbiter": {
      "command": "cargo",
      "args": [
        "run",
        "--release",
        "--manifest-path",
        "__ARBITER_DIR__/Cargo.toml",
        "--bin",
        "arbiter-mcp",
        "--",
        "--config",
        "__ARBITER_DIR__/config",
        "--tree",
        "__ARBITER_DIR__/models/agent_policy_tree.json",
        "--db",
        "__ARBITER_DIR__/arbiter.db",
        "--log-level",
        "info"
      ]
    }
  }
}
```

- [ ] **Step 2: Commit**

```bash
git add config/claude_desktop_config.json
git commit -m "feat: add Claude Desktop MCP config template"
```

---

### Task 4: Running Tasks Reconciliation (R8.2)

**Files:**
- Modify: `arbiter-mcp/src/db.rs`
- Modify: `arbiter-mcp/src/main.rs`

On startup, reset all `running_tasks` counters to 0. After a crash, counters may have drifted — tasks that were "running" are now orphaned.

- [ ] **Step 1: Add `reset_all_running_tasks` to Database**

```rust
    /// Reset running_tasks to 0 for all agents.
    ///
    /// Called on startup to recover from crashes where counters drifted.
    pub fn reset_all_running_tasks(&self) -> Result<usize> {
        let rows = self
            .conn
            .execute(
                "UPDATE agents SET running_tasks = 0, updated_at = datetime('now')
                 WHERE running_tasks > 0",
                [],
            )
            .context("Failed to reset running_tasks")?;
        if rows > 0 {
            tracing::info!(agents_reset = rows, "reset orphaned running_tasks on startup");
        }
        Ok(rows)
    }
```

- [ ] **Step 2: Add test**

```rust
    #[test]
    fn reset_all_running_tasks() {
        let db = setup_db();
        insert_test_agent(&db, "a1");
        insert_test_agent(&db, "a2");

        db.increment_running_tasks("a1").unwrap();
        db.increment_running_tasks("a1").unwrap();
        db.increment_running_tasks("a2").unwrap();

        let reset = db.reset_all_running_tasks().unwrap();
        assert_eq!(reset, 2); // 2 agents had running > 0

        assert_eq!(db.get_running_tasks("a1").unwrap(), 0);
        assert_eq!(db.get_running_tasks("a2").unwrap(), 0);
    }

    #[test]
    fn reset_all_running_tasks_noop_when_zero() {
        let db = setup_db();
        insert_test_agent(&db, "a1");
        let reset = db.reset_all_running_tasks().unwrap();
        assert_eq!(reset, 0);
    }
```

- [ ] **Step 3: Call on startup in main.rs**

After `database.migrate()` and the retention purge, before creating the registry:

```rust
    // Reset orphaned running_tasks counters (crash recovery).
    match database.reset_all_running_tasks() {
        Ok(n) if n > 0 => info!(agents_reset = n, "startup: reset orphaned running_tasks"),
        Ok(_) => {}
        Err(e) => eprintln!("WARNING: running_tasks reset failed: {e:#}"),
    }
```

- [ ] **Step 4: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add arbiter-mcp/src/db.rs arbiter-mcp/src/main.rs
git commit -m "feat: reset orphaned running_tasks counters on startup (crash recovery)"
```

---

## Exit Criteria Checklist

- [ ] `cargo test --workspace` — all tests pass (300+ tests)
- [ ] `cargo clippy --workspace -- -D warnings` — clean
- [ ] Invalid `jsonrpc` version rejected with -32600
- [ ] Lines > 1MB rejected with -32600
- [ ] 12 golden tests pass
- [ ] `config/claude_desktop_config.json` exists
- [ ] `running_tasks` reset to 0 on startup
