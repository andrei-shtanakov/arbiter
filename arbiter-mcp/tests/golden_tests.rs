//! Golden test regression suite for the MCP JSON-RPC protocol.
//!
//! Reads `.jsonl` fixture files from `tests/golden/`, each containing
//! a request line and an expected response pattern line. Dispatches
//! each request through the server and validates the response against
//! the pattern using flexible matchers.

#![allow(clippy::arc_with_non_send_sync)]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};

use arbiter_core::policy::decision_tree::DecisionTree;

use arbiter_mcp::agents::AgentRegistry;
use arbiter_mcp::config::{load_config, ArbiterConfig};
use arbiter_mcp::db::Database;
use arbiter_mcp::metrics::Metrics;
use arbiter_mcp::server::{JsonRpcRequest, McpServer};

// ---------------------------------------------------------------------------
// Test setup helpers
// ---------------------------------------------------------------------------

/// Load the bootstrap decision tree from models/.
fn bootstrap_tree() -> DecisionTree {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = std::path::Path::new(manifest_dir)
        .parent()
        .unwrap()
        .join("models/agent_policy_tree.json");
    let json = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "{} not found -- run bootstrap_agent_tree.py first",
            path.display()
        )
    });
    DecisionTree::from_json(&json).expect("failed to parse bootstrap tree")
}

/// Load the real config from config/.
fn real_config() -> ArbiterConfig {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let config_dir = std::path::Path::new(manifest_dir)
        .parent()
        .unwrap()
        .join("config");
    load_config(&config_dir).expect("failed to load config")
}

/// Build a fully initialized MCP server backed by an in-memory DB.
fn build_server() -> McpServer {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    let db = Arc::new(db);

    let config = real_config();
    let tree = bootstrap_tree();
    let registry = AgentRegistry::new(Arc::clone(&db), &config.agents).unwrap();

    let config = Arc::new(RwLock::new(config));
    let tree = Arc::new(RwLock::new(Some(tree)));
    let registry = Arc::new(RwLock::new(registry));
    let metrics = Arc::new(Metrics::new());
    let shutdown = Arc::new(AtomicBool::new(false));

    McpServer::new(config, db, tree, registry, metrics, shutdown)
}

/// Collect and sort `.jsonl` fixture files from the golden directory.
fn golden_fixtures() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden");
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                Some(path)
            } else {
                None
            }
        })
        .collect();
    files.sort();
    files
}

// ---------------------------------------------------------------------------
// Pattern matching engine
// ---------------------------------------------------------------------------

/// Compare an actual JSON value against an expected pattern.
///
/// Pattern rules:
/// - `"__HAS_TEXT__"` — value is an array with at least one entry
/// - `"__ARRAY_LENGTH_N__"` — value is an array of length N
/// - `"__CONTAINS__xyz"` — string value contains "xyz"
/// - Object patterns — recurse: every key in expected must match in actual
/// - Otherwise — exact equality on primitives
fn matches_pattern(
    actual: &serde_json::Value,
    expected: &serde_json::Value,
    path: &str,
) -> Result<(), String> {
    match expected {
        serde_json::Value::String(s) if s == "__HAS_TEXT__" => {
            let arr = actual
                .as_array()
                .ok_or_else(|| format!("{path}: expected array, got {actual}"))?;
            if arr.is_empty() {
                return Err(format!("{path}: expected non-empty array"));
            }
            Ok(())
        }
        serde_json::Value::String(s) if s.starts_with("__ARRAY_LENGTH_") => {
            let n: usize = s
                .trim_start_matches("__ARRAY_LENGTH_")
                .trim_end_matches("__")
                .parse()
                .map_err(|e| format!("{path}: bad length pattern '{s}': {e}"))?;
            let arr = actual
                .as_array()
                .ok_or_else(|| format!("{path}: expected array, got {actual}"))?;
            if arr.len() != n {
                return Err(format!(
                    "{path}: expected array length {n}, got {}",
                    arr.len()
                ));
            }
            Ok(())
        }
        serde_json::Value::String(s) if s.starts_with("__CONTAINS__") => {
            let needle = &s["__CONTAINS__".len()..];
            let hay = actual
                .as_str()
                .ok_or_else(|| format!("{path}: expected string, got {actual}"))?;
            if !hay.contains(needle) {
                return Err(format!(
                    "{path}: expected string containing '{needle}', got '{hay}'"
                ));
            }
            Ok(())
        }
        serde_json::Value::Object(expected_map) => {
            let actual_map = actual
                .as_object()
                .ok_or_else(|| format!("{path}: expected object, got {actual}"))?;
            for (key, expected_val) in expected_map {
                let child_path = format!("{path}.{key}");
                let actual_val = actual_map
                    .get(key)
                    .ok_or_else(|| format!("{child_path}: missing key"))?;
                matches_pattern(actual_val, expected_val, &child_path)?;
            }
            Ok(())
        }
        _ => {
            // Exact match for numbers, booleans, null, plain strings
            if actual != expected {
                return Err(format!("{path}: expected {expected}, got {actual}"));
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// The golden test
// ---------------------------------------------------------------------------

#[test]
fn run_golden_tests() {
    let mut server = build_server();
    let fixtures = golden_fixtures();

    assert!(
        !fixtures.is_empty(),
        "no .jsonl fixtures found in tests/golden/"
    );

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for fixture in &fixtures {
        let name = fixture.file_name().unwrap().to_string_lossy().to_string();
        let content = fs::read_to_string(fixture)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", fixture.display()));
        let lines: Vec<&str> = content.lines().collect();

        if lines.len() != 2 {
            failures.push(format!("{name}: expected 2 lines, got {}", lines.len()));
            failed += 1;
            continue;
        }

        let request_line = lines[0];
        let expected_line = lines[1].trim();

        // Parse the request
        let request: JsonRpcRequest = match serde_json::from_str(request_line) {
            Ok(r) => r,
            Err(e) => {
                failures.push(format!("{name}: invalid request JSON: {e}"));
                failed += 1;
                continue;
            }
        };

        // Dispatch
        let response = server.dispatch(&request);

        // Handle null expected (notification, no response)
        if expected_line == "null" {
            if response.is_none() {
                passed += 1;
            } else {
                failures.push(format!("{name}: expected no response (null), got Some"));
                failed += 1;
            }
            continue;
        }

        // We expect a response
        let response = match response {
            Some(r) => r,
            None => {
                failures.push(format!("{name}: expected a response, got None"));
                failed += 1;
                continue;
            }
        };

        // Serialize actual response to Value for comparison
        let actual: serde_json::Value = serde_json::to_value(&response).unwrap();

        // Parse expected pattern
        let expected: serde_json::Value = match serde_json::from_str(expected_line) {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!("{name}: invalid expected JSON: {e}"));
                failed += 1;
                continue;
            }
        };

        // Match
        match matches_pattern(&actual, &expected, &name) {
            Ok(()) => passed += 1,
            Err(msg) => {
                failures.push(format!("{name}: MISMATCH: {msg}\n  actual: {actual}"));
                failed += 1;
            }
        }
    }

    eprintln!("\n=== Golden tests: {passed} passed, {failed} failed ===\n");

    if !failures.is_empty() {
        for f in &failures {
            eprintln!("  FAIL: {f}");
        }
        panic!("{failed} golden test(s) failed out of {}", passed + failed);
    }
}
