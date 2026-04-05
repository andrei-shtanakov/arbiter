# R1: Server Stops Crashing — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate all panic points on production paths, fix dead invariants, correct error codes, and compute agent state dynamically.

**Architecture:** Four independent fixes touching arbiter-core (panic-freedom, dead invariants) and arbiter-mcp (error codes, hardcoded state). Changes propagate from `predict()` signature change through `evaluate_for_agents()` and `route_task::execute()`.

**Tech Stack:** Rust, anyhow, serde_json, rusqlite

---

## File Map

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `arbiter-core/src/policy/decision_tree.rs:133-198` | Change `predict()` to return `Result<PredictionResult>` |
| Modify | `arbiter-core/src/policy/engine.rs:14-34` | Propagate `Result` from `predict()` |
| Modify | `arbiter-core/src/types.rs:205-221` | Add `retry_count`, `calls_per_minute` to `Constraints` |
| Modify | `arbiter-mcp/src/tools/route_task.rs:81-99` | Wire `retry_count`/`calls_per_minute` from Constraints into SystemContext |
| Modify | `arbiter-mcp/src/tools/route_task.rs:64-78` | Compute AgentState dynamically instead of hardcoding Active |
| Modify | `arbiter-mcp/src/server.rs:561-616` | Fix error codes: `-32602` → `-32000` for tool execution errors |

---

### Task 1: Panic-Freedom in `predict()`

**Files:**
- Modify: `arbiter-core/src/policy/decision_tree.rs:133-198`
- Modify: `arbiter-core/src/policy/engine.rs:14-34`
- Modify: `arbiter-mcp/src/tools/route_task.rs` (callers of `evaluate_for_agents`)

**Context:** Three panic points exist on the hot path:
1. `assert_eq!` at line 134 — panics on wrong feature vector length
2. `.unwrap()` at line 157 on `partial_cmp` — panics on NaN values
3. `.unwrap()` at line 157 on `max_by` — panics if `node.value` is empty (impossible after validation but still unsafe)

- [ ] **Step 1: Write failing test for `predict()` returning error on wrong feature count**

In `arbiter-core/src/policy/decision_tree.rs`, replace the `#[should_panic]` test with a Result-based test. Add this test (alongside the existing one for now):

```rust
#[test]
fn predict_wrong_feature_count_returns_error() {
    let tree = DecisionTree::from_json(&minimal_tree_json()).unwrap();
    let result = tree.predict(&[1.0]); // expects 3 features
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("feature vector length"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p arbiter-core predict_wrong_feature_count_returns_error`
Expected: FAIL — `predict()` currently returns `PredictionResult`, not `Result`

- [ ] **Step 3: Write failing test for NaN in feature vector**

```rust
#[test]
fn predict_nan_feature_returns_error() {
    let tree = DecisionTree::from_json(&minimal_tree_json()).unwrap();
    let result = tree.predict(&[f64::NAN, 0.0, 0.0]);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("NaN"));
}
```

- [ ] **Step 4: Change `predict()` signature and implementation**

In `arbiter-core/src/policy/decision_tree.rs`, change the `predict` method:

```rust
/// Run inference on a feature vector.
///
/// Returns a `PredictionResult` with the predicted class index,
/// confidence score, and the decision path through the tree.
///
/// Returns an error if the feature vector length doesn't match
/// or contains NaN values.
pub fn predict(&self, features: &[f64]) -> Result<PredictionResult> {
    if features.len() != self.n_features {
        bail!(
            "feature vector length {} does not match tree n_features {}",
            features.len(),
            self.n_features
        );
    }

    // Check for NaN values
    if let Some(idx) = features.iter().position(|f| f.is_nan()) {
        bail!("feature vector contains NaN at index {idx}");
    }

    let mut node_idx: usize = 0;
    let mut path = Vec::new();

    loop {
        let node = &self.nodes[node_idx];
        let is_leaf = node.feature < 0;

        if is_leaf {
            // Leaf node: compute prediction
            let total: f64 = node.value.iter().sum();
            let (class, &max_val) = node
                .value
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| {
                    a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                })
                .context("empty value vector in leaf node")?;

            let confidence = if total > 0.0 { max_val / total } else { 0.0 };

            let class_name = self
                .class_names
                .get(class)
                .cloned()
                .unwrap_or_else(|| format!("class_{class}"));
            path.push(format!("leaf: {class_name} (confidence={confidence:.3})"));

            return Ok(PredictionResult {
                class,
                confidence,
                path,
            });
        }

        // Internal node: traverse left or right
        let feat_idx = node.feature as usize;
        let feat_val = features[feat_idx];
        let feat_name = self
            .feature_names
            .get(feat_idx)
            .cloned()
            .unwrap_or_else(|| format!("feature[{feat_idx}]"));

        if feat_val <= node.threshold {
            path.push(format!(
                "node {node_idx}: {feat_name} ({feat_val:.2}) <= {:.2} -> left",
                node.threshold
            ));
            node_idx = node.left as usize;
        } else {
            path.push(format!(
                "node {node_idx}: {feat_name} ({feat_val:.2}) > {:.2} -> right",
                node.threshold
            ));
            node_idx = node.right as usize;
        }
    }
}
```

- [ ] **Step 5: Update all existing tests that call `predict()` to unwrap**

In `decision_tree.rs` tests, update every call to `tree.predict(...)` to `tree.predict(...).unwrap()`. These tests:
- `predict_left_branch`: `tree.predict(&[3.0, 0.0, 0.0]).unwrap()`
- `predict_right_branch`: `tree.predict(&[7.0, 0.0, 0.0]).unwrap()`
- `predict_deterministic`: `tree.predict(&features).unwrap()`
- `predict_threshold_boundary`: `tree.predict(&[5.0, 0.0, 0.0]).unwrap()`
- `from_json_without_feature_names`: `tree.predict(&[1.0, 2.0]).unwrap()`
- `bootstrap_tree_deterministic`: `tree.predict(&features).unwrap()`

Remove the old `#[should_panic]` test `predict_wrong_feature_count` entirely.

- [ ] **Step 6: Update `evaluate_for_agents()` to propagate `Result`**

In `arbiter-core/src/policy/engine.rs`:

```rust
use anyhow::Result;

/// Evaluate multiple agents against the decision tree and return
/// results ranked by confidence (highest first).
///
/// Agents whose prediction fails (e.g., bad feature vector) are
/// skipped with a warning logged to the result.
pub fn evaluate_for_agents(
    tree: &DecisionTree,
    feature_vectors: &[(String, [f64; 22])],
) -> Vec<(String, PredictionResult)> {
    let mut results: Vec<(String, PredictionResult)> = feature_vectors
        .iter()
        .filter_map(|(agent_id, features)| {
            match tree.predict(features) {
                Ok(prediction) => Some((agent_id.clone(), prediction)),
                Err(_) => None, // Skip agents with bad predictions
            }
        })
        .collect();

    // Sort by confidence descending (stable sort preserves order for equal confidence)
    results.sort_by(|a, b| {
        b.1.confidence
            .partial_cmp(&a.1.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    results
}
```

- [ ] **Step 7: Run all tests**

Run: `cargo test -p arbiter-core`
Expected: ALL PASS — including the 2 new error tests and all updated existing tests.

- [ ] **Step 8: Verify no `unwrap()` or `assert!` on production paths**

Run: `grep -n 'unwrap()\|assert!' arbiter-core/src/policy/decision_tree.rs | grep -v '#\[cfg(test)\]' | grep -v 'mod tests' | head -20`
Expected: Only `unwrap_or`/`unwrap_or_else` patterns, no bare `unwrap()` or `assert!` outside tests.

- [ ] **Step 9: Commit**

```bash
git add arbiter-core/src/policy/decision_tree.rs arbiter-core/src/policy/engine.rs
git commit -m "fix: make predict() return Result, eliminate panics on hot path

Replace assert_eq! and unwrap() in DecisionTree::predict() with
proper error handling via anyhow::Result. Add NaN detection.
Update evaluate_for_agents() to filter failed predictions.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Fix Dead Invariants (`retry_limit`, `rate_limit`)

**Files:**
- Modify: `arbiter-core/src/types.rs:205-221` — add fields to `Constraints`
- Modify: `arbiter-mcp/src/tools/route_task.rs:81-99` — wire fields into `SystemContext`
- Modify: `arbiter-mcp/src/server.rs` — pass through from MCP input schema

**Context:** In `route_task.rs:98-99`, `SystemContext` is built with `retry_count: 0, calls_per_minute: 0`. Since invariant thresholds are `max_retries: 3` and `calls_per_minute: 60`, these invariants always pass — they are dead code. The fix is to accept these values from the caller via `Constraints`.

- [ ] **Step 1: Write failing test for retry_limit invariant activation**

In `arbiter-core/src/invariant/rules.rs` tests, verify the rule works when fed real values (this test should already pass since the rule logic is correct — the bug is in the caller). Instead, write a test in `arbiter-mcp/src/tools/route_task.rs` tests that verifies retry_count flows through:

First, add a unit test in `arbiter-core/src/invariant/rules.rs` (in the test module) that confirms retry_limit fails when retry_count >= max_retries:

```rust
#[test]
fn retry_limit_fails_at_max() {
    let system = SystemContext {
        total_running_tasks: 0,
        running_scopes: vec![],
        running_branches: vec![],
        budget_remaining_usd: None,
        retry_count: 3,
        calls_per_minute: 0,
    };
    let thresholds = InvariantThresholds {
        max_total_concurrent: 5,
        max_retries: 3,
        calls_per_minute: 60,
        max_failures_24h: 5,
        buffer_multiplier: 1.5,
    };
    let result = retry_limit(&system, &thresholds);
    assert!(!result.passed, "retry_limit should fail when retry_count >= max_retries");
}

#[test]
fn rate_limit_fails_at_max() {
    let system = SystemContext {
        total_running_tasks: 0,
        running_scopes: vec![],
        running_branches: vec![],
        budget_remaining_usd: None,
        retry_count: 0,
        calls_per_minute: 60,
    };
    let thresholds = InvariantThresholds {
        max_total_concurrent: 5,
        max_retries: 3,
        calls_per_minute: 60,
        max_failures_24h: 5,
        buffer_multiplier: 1.5,
    };
    let result = rate_limit(&system, &thresholds);
    assert!(!result.passed, "rate_limit should fail when calls_per_minute >= limit");
}
```

- [ ] **Step 2: Run tests to confirm rule logic is correct**

Run: `cargo test -p arbiter-core retry_limit_fails_at_max rate_limit_fails_at_max`
Expected: PASS — the invariant rules themselves work fine, the bug is in the caller.

- [ ] **Step 3: Add `retry_count` and `calls_per_minute` to `Constraints`**

In `arbiter-core/src/types.rs`, add two fields to the `Constraints` struct after `running_tasks`:

```rust
    /// Number of retries for this task so far.
    #[serde(default)]
    pub retry_count: Option<u32>,
    /// Current API calls per minute across all agents.
    #[serde(default)]
    pub calls_per_minute: Option<u32>,
```

- [ ] **Step 4: Wire the new fields into `to_system_context()`**

In `arbiter-mcp/src/tools/route_task.rs`, update the `to_system_context` function:

```rust
fn to_system_context(constraints: &Constraints, total_running: u32) -> SystemContext {
    let running_scopes: Vec<Vec<String>> = constraints
        .running_tasks
        .iter()
        .map(|rt| rt.scope.clone())
        .collect();
    let running_branches: Vec<String> = constraints
        .running_tasks
        .iter()
        .filter_map(|rt| rt.branch.clone())
        .collect();

    SystemContext {
        total_running_tasks: total_running,
        running_scopes,
        running_branches,
        budget_remaining_usd: constraints.budget_remaining_usd,
        retry_count: constraints.retry_count.unwrap_or(0),
        calls_per_minute: constraints.calls_per_minute.unwrap_or(0),
    }
}
```

- [ ] **Step 5: Update the MCP input schema for `route_task`**

In `arbiter-mcp/src/server.rs`, in the `tool_schemas()` function, add the two new properties to the `constraints` object schema (after `"running_tasks"`):

```json
"retry_count": { "type": "integer", "description": "Number of retries for this task so far" },
"calls_per_minute": { "type": "integer", "description": "Current API calls per minute" }
```

- [ ] **Step 6: Fix all places that construct `Constraints` in tests**

Search for `Constraints {` in test files. Each construction that doesn't have the new fields will get defaults via `#[serde(default)]` for deserialization, but direct struct construction needs the fields. Add `retry_count: None, calls_per_minute: None` to each:

- `arbiter-mcp/src/server.rs:473-479` (the default Constraints in handle_route_task)
- Any test files that construct `Constraints` directly

- [ ] **Step 7: Run all tests**

Run: `cargo test --workspace`
Expected: ALL PASS

- [ ] **Step 8: Commit**

```bash
git add arbiter-core/src/types.rs arbiter-core/src/invariant/rules.rs arbiter-mcp/src/tools/route_task.rs arbiter-mcp/src/server.rs
git commit -m "fix: wire retry_count and calls_per_minute into invariant checks

retry_limit and rate_limit invariants were always seeing 0 values
because SystemContext hardcoded them. Add retry_count and
calls_per_minute to Constraints so callers can pass real values.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Fix Error Codes in MCP Tool Handlers

**Files:**
- Modify: `arbiter-mcp/src/server.rs:561-574` — report_outcome error handler
- Modify: `arbiter-mcp/src/server.rs:603-616` — get_agent_status error handler

**Context:** When `report_outcome::execute()` or `agent_status::execute()` fail (e.g., "agent not found", "Missing required field"), the server returns error code `-32602` (`INVALID_PARAMS`). Per JSON-RPC 2.0 spec, `-32602` means the request structure is wrong (wrong param types, missing required params). Tool execution errors (business logic failures) should use `-32000` (server error). `route_task` already correctly uses `-32000`.

- [ ] **Step 1: Write failing test for report_outcome error code**

In `arbiter-mcp/src/server.rs` tests, add:

```rust
#[test]
fn report_outcome_business_error_uses_server_error_code() {
    let (config, db, tree, registry) = test_setup();
    let mut server = McpServer::new(config, &db, Some(&tree), registry);

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(Value::Number(42.into())),
        method: "tools/call".to_string(),
        params: Some(serde_json::json!({
            "name": "report_outcome",
            "arguments": {
                "task_id": "t1",
                "agent_id": "nonexistent_agent",
                "status": "invalid_status"
            }
        })),
    };

    let resp = server.dispatch(&req).unwrap();
    let err = resp.error.expect("should be an error");
    assert_eq!(err.code, -32000, "tool execution errors should use -32000, not -32602");
}
```

Note: This test depends on the existing test infrastructure in server.rs. Adapt the setup function name to match what already exists in the test module (likely `test_config()` + manual setup). Check the existing test module for the exact setup pattern.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p arbiter-mcp report_outcome_business_error_uses_server_error_code`
Expected: FAIL — current code returns -32602

- [ ] **Step 3: Write failing test for get_agent_status error code**

```rust
#[test]
fn get_agent_status_business_error_uses_server_error_code() {
    let (config, db, tree, registry) = test_setup();
    let mut server = McpServer::new(config, &db, Some(&tree), registry);

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(Value::Number(43.into())),
        method: "tools/call".to_string(),
        params: Some(serde_json::json!({
            "name": "get_agent_status",
            "arguments": {
                "agent_id": "nonexistent_agent"
            }
        })),
    };

    let resp = server.dispatch(&req).unwrap();
    let err = resp.error.expect("should be an error");
    assert_eq!(err.code, -32000, "tool execution errors should use -32000, not -32602");
}
```

- [ ] **Step 4: Fix the error codes**

In `arbiter-mcp/src/server.rs`, change two lines:

Line 568 (in `handle_report_outcome`): change `code: INVALID_PARAMS,` to `code: -32000,`

Line 610 (in `handle_get_agent_status`): change `code: INVALID_PARAMS,` to `code: -32000,`

- [ ] **Step 5: Run all tests**

Run: `cargo test -p arbiter-mcp`
Expected: ALL PASS

- [ ] **Step 6: Commit**

```bash
git add arbiter-mcp/src/server.rs
git commit -m "fix: use -32000 error code for tool execution failures

report_outcome and get_agent_status were using -32602 (INVALID_PARAMS)
for business logic errors. Per JSON-RPC 2.0, -32602 is for malformed
requests; -32000 is the correct code for server/application errors.
route_task already used -32000 correctly.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Fix Hardcoded `AgentState::Active`

**Files:**
- Modify: `arbiter-mcp/src/tools/route_task.rs:64-78` — `to_agent_context()` function

**Context:** In `to_agent_context()` at line 67, `state: AgentState::Active` is hardcoded. This means the `agent_available` invariant (rule 1) never sees a non-active agent. The state should be computed from the agent's runtime metrics, matching the `determine_state()` logic already in `agent_status.rs:72-85`.

- [ ] **Step 1: Write failing test**

In `arbiter-mcp/src/tools/route_task.rs` tests (or a new test if the module doesn't have a test for this), add a test that creates an AgentInfo with high failures and verifies the constructed AgentContext has a non-Active state:

```rust
#[test]
fn to_agent_context_computes_state_from_metrics() {
    let info = AgentInfo {
        agent_id: "test_agent".to_string(),
        config: AgentConfig {
            display_name: "Test".to_string(),
            supports_languages: vec!["python".to_string()],
            supports_types: vec!["bugfix".to_string()],
            max_concurrent: 2,
            cost_per_hour: 0.10,
            avg_duration_min: 10.0,
        },
        running_tasks: 2,   // at capacity
        success_rate: Some(0.5),
        avg_duration_min: Some(10.0),
        avg_cost_usd: Some(0.10),
        recent_failures: 0,
    };

    let ctx = to_agent_context(&info, 5); // max_failures_24h = 5
    assert_eq!(ctx.state, AgentState::Busy, "agent at capacity should be Busy");
}

#[test]
fn to_agent_context_failed_state_on_high_failures() {
    let info = AgentInfo {
        agent_id: "test_agent".to_string(),
        config: AgentConfig {
            display_name: "Test".to_string(),
            supports_languages: vec!["python".to_string()],
            supports_types: vec!["bugfix".to_string()],
            max_concurrent: 2,
            cost_per_hour: 0.10,
            avg_duration_min: 10.0,
        },
        running_tasks: 0,
        success_rate: Some(0.5),
        avg_duration_min: Some(10.0),
        avg_cost_usd: Some(0.10),
        recent_failures: 6,
    };

    let ctx = to_agent_context(&info, 5); // max_failures_24h = 5
    assert_eq!(ctx.state, AgentState::Failed, "agent with 6 failures (threshold 5) should be Failed");
}
```

Note: `AgentState::Busy` and `AgentState::Failed` may not exist yet. Check if they do. If `AgentState` only has `Active` and `Inactive`, we need to add `Busy` and `Failed` variants.

- [ ] **Step 2: Check if `AgentState` has the needed variants**

Run: `grep -A 10 'pub enum AgentState' arbiter-core/src/types.rs`

If it only has `Active`/`Inactive`, add `Busy` and `Failed` variants:

In `arbiter-core/src/types.rs`:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Active,
    Inactive,
    Busy,
    Failed,
}
```

Update the `Display` impl for `AgentState` to handle new variants:
```rust
impl std::fmt::Display for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Inactive => write!(f, "inactive"),
            Self::Busy => write!(f, "busy"),
            Self::Failed => write!(f, "failed"),
        }
    }
}
```

- [ ] **Step 3: Update `to_agent_context()` to compute state**

In `arbiter-mcp/src/tools/route_task.rs`, change the function signature and body:

```rust
/// Build an AgentContext from AgentInfo for invariant checks.
///
/// Computes agent state from runtime metrics:
/// - Failed: recent_failures > max_failures_24h
/// - Busy: running_tasks >= max_concurrent
/// - Active: otherwise
fn to_agent_context(info: &AgentInfo, max_failures_24h: u32) -> AgentContext {
    let state = if info.recent_failures > max_failures_24h {
        AgentState::Failed
    } else if info.running_tasks >= info.config.max_concurrent {
        AgentState::Busy
    } else {
        AgentState::Active
    };

    AgentContext {
        agent_id: info.agent_id.clone(),
        state,
        running_tasks: info.running_tasks,
        max_concurrent: info.config.max_concurrent,
        supports_languages: info.config.supports_languages.clone(),
        supports_types: info.config.supports_types.clone(),
        failures_24h: info.recent_failures,
        avg_duration_min: info
            .avg_duration_min
            .unwrap_or(info.config.avg_duration_min),
        cost_per_hour: info.config.cost_per_hour,
    }
}
```

- [ ] **Step 4: Update all call sites of `to_agent_context`**

Search for `to_agent_context(` in `route_task.rs` and add the `max_failures_24h` parameter. The value comes from `config.agent_health.max_failures_24h` which is already available in the `execute()` function via `invariant_config`.

Each call like:
```rust
let agent_ctx = to_agent_context(&info);
```
becomes:
```rust
let agent_ctx = to_agent_context(&info, invariant_config.agent_health.max_failures_24h);
```

Find all call sites: `grep -n 'to_agent_context' arbiter-mcp/src/tools/route_task.rs`

- [ ] **Step 5: Run all tests**

Run: `cargo test --workspace`
Expected: ALL PASS

- [ ] **Step 6: Commit**

```bash
git add arbiter-core/src/types.rs arbiter-mcp/src/tools/route_task.rs
git commit -m "fix: compute AgentState dynamically from runtime metrics

to_agent_context() was hardcoding AgentState::Active, making the
agent_available invariant unable to detect failed or busy agents.
Now computes state from running_tasks and recent_failures, matching
the logic in agent_status::determine_state().

Add Busy and Failed variants to AgentState enum.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Final Verification

- [ ] **Step 1: Run full workspace test suite**

Run: `cargo test --workspace`
Expected: ALL PASS

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: No warnings

- [ ] **Step 3: Run formatter**

Run: `cargo fmt --all`

- [ ] **Step 4: Verify no unwrap/assert on production paths**

Run: `grep -rn 'unwrap()\|assert!' arbiter-core/src/ arbiter-mcp/src/ --include='*.rs' | grep -v '#\[cfg(test)\]' | grep -v 'mod tests' | grep -v 'unwrap_or\|unwrap_or_else\|unwrap_or_default'`

Review output — there should be no bare `unwrap()` or `assert!` on production code paths. `unwrap_or`, `unwrap_or_else`, and `unwrap_or_default` are safe.

- [ ] **Step 5: Final commit if any formatting changes**

```bash
git add -A
git commit -m "style: format after R1 changes

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
```
