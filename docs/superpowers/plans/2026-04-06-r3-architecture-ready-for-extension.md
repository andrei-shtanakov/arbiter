# R3 — Architecture Ready for Extension

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `anyhow` in arbiter-core with typed `thiserror` errors, introduce trait abstractions for storage and inference, add config validation, clean up unused deps, and add cross-validation to the bootstrap script.

**Architecture:** arbiter-core becomes a true library with its own error type (`ArbiterError`) and trait boundaries (`InferenceBackend`, `DecisionStore`, `AgentStore`). arbiter-mcp implements these traits for its concrete SQLite/file-based backends. Config validation catches semantic errors at load time rather than at runtime.

**Tech Stack:** Rust (thiserror, serde, rusqlite), Python (scikit-learn cross-validation)

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `arbiter-core/src/error.rs` | `ArbiterError` enum with thiserror |
| Modify | `arbiter-core/src/lib.rs` | Export `error` module |
| Modify | `arbiter-core/src/policy/decision_tree.rs` | Return `ArbiterError` instead of `anyhow::Result` |
| Modify | `arbiter-core/src/policy/engine.rs` | Use `ArbiterError` |
| Create | `arbiter-core/src/traits.rs` | `InferenceBackend`, `DecisionStore`, `AgentStore` traits |
| Modify | `arbiter-mcp/src/config.rs` | Add `validate()` method |
| Modify | `arbiter-mcp/Cargo.toml` | Remove `tokio` dep (unused, needed only in R4) |
| Modify | `arbiter-core/Cargo.toml` | Remove `anyhow` dep after migration |
| Modify | `scripts/bootstrap_agent_tree.py` | Add k-fold CV |

---

### Task 1: Typed Errors — `ArbiterError` enum

**Files:**
- Create: `arbiter-core/src/error.rs`
- Modify: `arbiter-core/src/lib.rs`
- Modify: `arbiter-core/src/policy/decision_tree.rs`
- Modify: `arbiter-core/src/policy/engine.rs`

This task replaces `anyhow::Result` in arbiter-core with a typed `ArbiterError` enum using `thiserror`. The `thiserror` crate is already in `Cargo.toml` but unused.

- [ ] **Step 1: Write tests for ArbiterError variants**

Create `arbiter-core/src/error.rs`:

```rust
//! Typed error types for the Arbiter core library.
//!
//! Uses `thiserror` for ergonomic error definitions.
//! All errors from arbiter-core are expressed as [`ArbiterError`].

use thiserror::Error;

/// Errors returned by arbiter-core operations.
#[derive(Debug, Error)]
pub enum ArbiterError {
    /// Decision tree JSON is malformed or violates structural constraints.
    #[error("invalid tree: {0}")]
    InvalidTree(String),

    /// Feature vector has wrong dimensions or contains invalid values.
    #[error("invalid features: {0}")]
    InvalidFeatures(String),

    /// Tree inference hit an unexpected state (empty leaf, missing node).
    #[error("inference error: {0}")]
    InferenceError(String),

    /// JSON serialization/deserialization failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Convenience alias for arbiter-core results.
pub type Result<T> = std::result::Result<T, ArbiterError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_invalid_tree() {
        let e = ArbiterError::InvalidTree("no nodes".to_string());
        assert_eq!(e.to_string(), "invalid tree: no nodes");
    }

    #[test]
    fn error_display_invalid_features() {
        let e = ArbiterError::InvalidFeatures(
            "length 5 does not match 22".to_string(),
        );
        assert!(e.to_string().contains("invalid features"));
    }

    #[test]
    fn error_display_inference() {
        let e = ArbiterError::InferenceError(
            "empty value vector".to_string(),
        );
        assert!(e.to_string().contains("inference error"));
    }

    #[test]
    fn error_from_serde_json() {
        let bad_json = serde_json::from_str::<serde_json::Value>("{{");
        let serde_err = bad_json.unwrap_err();
        let e: ArbiterError = serde_err.into();
        assert!(e.to_string().contains("json error"));
    }

    #[test]
    fn result_alias_works() {
        fn returns_error() -> Result<()> {
            Err(ArbiterError::InvalidTree("test".to_string()))
        }
        assert!(returns_error().is_err());
    }
}
```

- [ ] **Step 2: Register error module in lib.rs**

In `arbiter-core/src/lib.rs`, add `pub mod error;` and re-export the error type:

```rust
pub mod error;
pub mod invariant;
pub mod policy;
pub mod types;

pub use error::{ArbiterError, Result};
```

- [ ] **Step 3: Run tests to verify error module compiles**

Run: `cargo test -p arbiter-core error`
Expected: 5 new tests pass.

- [ ] **Step 4: Migrate `decision_tree.rs` from anyhow to ArbiterError**

Replace the `use anyhow::{bail, Context, Result};` import with:

```rust
use crate::error::{ArbiterError, Result};
```

Migrate `from_json()` — replace each `bail!("...")` with `return Err(ArbiterError::InvalidTree(...))`:

```rust
pub fn from_json(json: &str) -> Result<Self> {
    let tree: TreeJson =
        serde_json::from_str(json).map_err(|e| {
            ArbiterError::InvalidTree(format!(
                "failed to parse decision tree JSON: {e}"
            ))
        })?;

    if tree.nodes.is_empty() {
        return Err(ArbiterError::InvalidTree(
            "decision tree has no nodes".to_string(),
        ));
    }
    if tree.n_classes == 0 {
        return Err(ArbiterError::InvalidTree(
            "decision tree has zero classes".to_string(),
        ));
    }
    if tree.n_features == 0 {
        return Err(ArbiterError::InvalidTree(
            "decision tree has zero features".to_string(),
        ));
    }
    if tree.class_names.len() != tree.n_classes {
        return Err(ArbiterError::InvalidTree(format!(
            "class_names length {} does not match n_classes {}",
            tree.class_names.len(),
            tree.n_classes
        )));
    }

    // Validate node structure
    let n = tree.nodes.len() as i32;
    for (i, node) in tree.nodes.iter().enumerate() {
        let is_leaf = node.feature < 0;
        if !is_leaf {
            if node.left < 0 || node.left >= n {
                return Err(ArbiterError::InvalidTree(format!(
                    "node {i}: invalid left child index {}",
                    node.left
                )));
            }
            if node.right < 0 || node.right >= n {
                return Err(ArbiterError::InvalidTree(format!(
                    "node {i}: invalid right child index {}",
                    node.right
                )));
            }
            if node.feature as usize >= tree.n_features {
                return Err(ArbiterError::InvalidTree(format!(
                    "node {i}: feature index {} >= n_features {}",
                    node.feature, tree.n_features
                )));
            }
        }
        if node.value.len() != tree.n_classes {
            return Err(ArbiterError::InvalidTree(format!(
                "node {i}: value length {} does not match n_classes {}",
                node.value.len(),
                tree.n_classes
            )));
        }
    }

    Ok(Self {
        n_features: tree.n_features,
        n_classes: tree.n_classes,
        class_names: tree.class_names,
        feature_names: tree.feature_names,
        nodes: tree.nodes,
    })
}
```

Migrate `predict()` — replace `bail!` with `ArbiterError::InvalidFeatures` / `ArbiterError::InferenceError`:

```rust
pub fn predict(&self, features: &[f64]) -> Result<PredictionResult> {
    // ... PathEntry enum unchanged ...

    if features.len() != self.n_features {
        return Err(ArbiterError::InvalidFeatures(format!(
            "feature vector length {} does not match tree n_features {}",
            features.len(),
            self.n_features
        )));
    }

    if let Some(idx) = features.iter().position(|f| !f.is_finite()) {
        return Err(ArbiterError::InvalidFeatures(format!(
            "feature vector contains non-finite value at index {idx}"
        )));
    }

    // ... traversal loop unchanged until the leaf node ...

    // In the leaf node case, replace .context() with:
    let (class, &max_val) = node
        .value
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| {
            a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
        })
        .ok_or_else(|| {
            ArbiterError::InferenceError(
                "empty value vector in leaf node".to_string(),
            )
        })?;

    // ... rest unchanged ...
}
```

- [ ] **Step 5: Update `engine.rs` to use ArbiterError**

In `arbiter-core/src/policy/engine.rs`, the `evaluate_for_agents` function calls `tree.predict()` which now returns `crate::error::Result`. The function already handles errors via `filter_map`, so the only change is the import — remove the unused `warn` import if it came from anyhow context. Actually, `warn` comes from `tracing`, so it stays. The error type change is transparent since `evaluate_for_agents` pattern-matches on `Ok`/`Err`.

No code change needed — the `Err(e)` branch already uses `%e` (Display), which works for both `anyhow::Error` and `ArbiterError`.

- [ ] **Step 6: Run full test suite**

Run: `cargo test --workspace`
Expected: All 245 tests pass. Existing tests in `decision_tree.rs` check `.unwrap_err().to_string().contains(...)` — the error messages are preserved so these still pass.

- [ ] **Step 7: Remove `anyhow` from arbiter-core**

In `arbiter-core/Cargo.toml`, remove the `anyhow` line:

```toml
[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }
thiserror = { workspace = true }
```

- [ ] **Step 8: Fix compilation after removing anyhow**

Run: `cargo check -p arbiter-core`
Expected: Clean compile. If any residual `anyhow` import exists, remove it.

- [ ] **Step 9: Run full tests again**

Run: `cargo test --workspace`
Expected: All 245 tests pass.

- [ ] **Step 10: Commit**

```bash
git add arbiter-core/src/error.rs arbiter-core/src/lib.rs \
  arbiter-core/src/policy/decision_tree.rs \
  arbiter-core/src/policy/engine.rs \
  arbiter-core/Cargo.toml
git commit -m "refactor: replace anyhow with typed ArbiterError in arbiter-core"
```

---

### Task 2: Trait Abstractions

**Files:**
- Create: `arbiter-core/src/traits.rs`
- Modify: `arbiter-core/src/lib.rs`

This task defines three trait boundaries that decouple arbiter-core's policy logic from concrete storage/inference. arbiter-mcp will implement them later (in its existing structs). For now, we define the traits and make them available.

- [ ] **Step 1: Write the traits file with tests**

Create `arbiter-core/src/traits.rs`:

```rust
//! Trait abstractions for Arbiter components.
//!
//! These traits define boundaries between the policy engine and its
//! concrete implementations (SQLite, file-based, in-memory mocks).
//! arbiter-mcp implements these; arbiter-core only defines them.

use crate::error::Result;
use crate::types::PredictionResult;

/// Inference backend: predicts agent class from a feature vector.
///
/// The default implementation is `DecisionTree`, but this trait
/// enables alternative backends (ensemble, neural, rule-based).
pub trait InferenceBackend {
    /// Run inference on a 22-dimensional feature vector.
    fn predict(&self, features: &[f64]) -> Result<PredictionResult>;

    /// Number of features expected.
    fn n_features(&self) -> usize;

    /// Number of classes the backend can predict.
    fn n_classes(&self) -> usize;

    /// Class name labels.
    fn class_names(&self) -> &[String];
}

/// Storage for routing decisions (audit trail).
pub trait DecisionStore {
    /// Record a routing decision, return its ID.
    fn store_decision(
        &self,
        task_id: &str,
        chosen_agent: &str,
        action: &str,
        confidence: f64,
        decision_path: &str,
        invariants_json: &str,
    ) -> Result<i64>;

    /// Find the most recent decision ID for a task.
    fn find_decision_by_task(
        &self,
        task_id: &str,
    ) -> Result<Option<i64>>;
}

/// Storage for agent performance statistics.
pub trait AgentStore {
    /// Get the number of tasks currently running on an agent.
    fn running_tasks(&self, agent_id: &str) -> Result<u32>;

    /// Get the total running tasks across all agents.
    fn total_running_tasks(&self) -> Result<u32>;

    /// Get the number of failures in the last N hours.
    fn recent_failures(
        &self,
        agent_id: &str,
        hours: u32,
    ) -> Result<u32>;

    /// Increment the running task count for an agent.
    fn increment_running(&self, agent_id: &str) -> Result<()>;

    /// Decrement the running task count for an agent.
    fn decrement_running(&self, agent_id: &str) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PredictionResult;

    /// Mock inference backend for testing.
    struct MockBackend {
        class_names: Vec<String>,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                class_names: vec![
                    "claude_code".to_string(),
                    "codex_cli".to_string(),
                    "aider".to_string(),
                ],
            }
        }
    }

    impl InferenceBackend for MockBackend {
        fn predict(
            &self,
            features: &[f64],
        ) -> Result<PredictionResult> {
            if features.len() != 22 {
                return Err(crate::error::ArbiterError::InvalidFeatures(
                    format!("expected 22, got {}", features.len()),
                ));
            }
            Ok(PredictionResult {
                class: 0,
                confidence: 0.9,
                path: vec!["mock: class 0".to_string()],
            })
        }

        fn n_features(&self) -> usize {
            22
        }

        fn n_classes(&self) -> usize {
            3
        }

        fn class_names(&self) -> &[String] {
            &self.class_names
        }
    }

    #[test]
    fn mock_backend_predict() {
        let backend = MockBackend::new();
        let features = [0.0f64; 22];
        let result = backend.predict(&features).unwrap();
        assert_eq!(result.class, 0);
        assert_eq!(result.confidence, 0.9);
    }

    #[test]
    fn mock_backend_wrong_features() {
        let backend = MockBackend::new();
        let result = backend.predict(&[1.0, 2.0]);
        assert!(result.is_err());
    }

    #[test]
    fn mock_backend_metadata() {
        let backend = MockBackend::new();
        assert_eq!(backend.n_features(), 22);
        assert_eq!(backend.n_classes(), 3);
        assert_eq!(backend.class_names().len(), 3);
    }
}
```

- [ ] **Step 2: Register traits module in lib.rs**

In `arbiter-core/src/lib.rs`, add `pub mod traits;`:

```rust
pub mod error;
pub mod invariant;
pub mod policy;
pub mod traits;
pub mod types;

pub use error::{ArbiterError, Result};
```

- [ ] **Step 3: Implement InferenceBackend for DecisionTree**

In `arbiter-core/src/policy/decision_tree.rs`, add the trait impl after the existing `impl DecisionTree` block:

```rust
impl crate::traits::InferenceBackend for DecisionTree {
    fn predict(&self, features: &[f64]) -> crate::error::Result<PredictionResult> {
        self.predict(features)
    }

    fn n_features(&self) -> usize {
        self.n_features
    }

    fn n_classes(&self) -> usize {
        self.n_classes
    }

    fn class_names(&self) -> &[String] {
        &self.class_names
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test --workspace`
Expected: 248+ tests pass (3 new mock tests + original 245).

- [ ] **Step 5: Commit**

```bash
git add arbiter-core/src/traits.rs arbiter-core/src/lib.rs \
  arbiter-core/src/policy/decision_tree.rs
git commit -m "refactor: add InferenceBackend, DecisionStore, AgentStore traits"
```

---

### Task 3: Config Validation

**Files:**
- Modify: `arbiter-mcp/src/config.rs`

Add semantic validation that catches impossible/nonsensical config values at load time.

- [ ] **Step 1: Write failing tests for validation**

Add these tests to the existing `#[cfg(test)] mod tests` block in `arbiter-mcp/src/config.rs`:

```rust
// -- Semantic validation tests --

#[test]
fn validate_agents_zero_max_concurrent() {
    let mut agents = HashMap::new();
    agents.insert(
        "bad".to_string(),
        AgentConfig {
            display_name: "Bad Agent".to_string(),
            supports_languages: vec!["python".to_string()],
            supports_types: vec!["bugfix".to_string()],
            max_concurrent: 0,
            cost_per_hour: 0.10,
            avg_duration_min: 10.0,
        },
    );
    let err = validate_agents(&agents).unwrap_err();
    assert!(err.to_string().contains("max_concurrent"));
}

#[test]
fn validate_agents_negative_cost() {
    let mut agents = HashMap::new();
    agents.insert(
        "bad".to_string(),
        AgentConfig {
            display_name: "Bad Agent".to_string(),
            supports_languages: vec!["python".to_string()],
            supports_types: vec!["bugfix".to_string()],
            max_concurrent: 1,
            cost_per_hour: -0.10,
            avg_duration_min: 10.0,
        },
    );
    let err = validate_agents(&agents).unwrap_err();
    assert!(err.to_string().contains("cost_per_hour"));
}

#[test]
fn validate_agents_zero_duration() {
    let mut agents = HashMap::new();
    agents.insert(
        "bad".to_string(),
        AgentConfig {
            display_name: "Bad Agent".to_string(),
            supports_languages: vec!["python".to_string()],
            supports_types: vec!["bugfix".to_string()],
            max_concurrent: 2,
            cost_per_hour: 0.10,
            avg_duration_min: 0.0,
        },
    );
    let err = validate_agents(&agents).unwrap_err();
    assert!(err.to_string().contains("avg_duration_min"));
}

#[test]
fn validate_agents_empty_languages() {
    let mut agents = HashMap::new();
    agents.insert(
        "bad".to_string(),
        AgentConfig {
            display_name: "Bad Agent".to_string(),
            supports_languages: vec![],
            supports_types: vec!["bugfix".to_string()],
            max_concurrent: 1,
            cost_per_hour: 0.10,
            avg_duration_min: 10.0,
        },
    );
    let err = validate_agents(&agents).unwrap_err();
    assert!(err.to_string().contains("supports_languages"));
}

#[test]
fn validate_agents_empty_types() {
    let mut agents = HashMap::new();
    agents.insert(
        "bad".to_string(),
        AgentConfig {
            display_name: "Bad Agent".to_string(),
            supports_languages: vec!["python".to_string()],
            supports_types: vec![],
            max_concurrent: 1,
            cost_per_hour: 0.10,
            avg_duration_min: 10.0,
        },
    );
    let err = validate_agents(&agents).unwrap_err();
    assert!(err.to_string().contains("supports_types"));
}

#[test]
fn validate_invariants_zero_max_retries() {
    let inv = InvariantConfig {
        budget: BudgetConfig { threshold_usd: 10.0 },
        retries: RetriesConfig { max_retries: 0 },
        rate_limit: RateLimitConfig { calls_per_minute: 60 },
        agent_health: AgentHealthConfig { max_failures_24h: 5 },
        concurrency: ConcurrencyConfig { max_total_concurrent: 5 },
        sla: SlaConfig { buffer_multiplier: 1.5 },
    };
    // max_retries=0 means no retries allowed — this is valid (strict mode)
    validate_invariants(&inv).unwrap();
}

#[test]
fn validate_invariants_zero_concurrency() {
    let inv = InvariantConfig {
        budget: BudgetConfig { threshold_usd: 10.0 },
        retries: RetriesConfig { max_retries: 3 },
        rate_limit: RateLimitConfig { calls_per_minute: 60 },
        agent_health: AgentHealthConfig { max_failures_24h: 5 },
        concurrency: ConcurrencyConfig { max_total_concurrent: 0 },
        sla: SlaConfig { buffer_multiplier: 1.5 },
    };
    let err = validate_invariants(&inv).unwrap_err();
    assert!(err.to_string().contains("max_total_concurrent"));
}

#[test]
fn validate_invariants_zero_buffer() {
    let inv = InvariantConfig {
        budget: BudgetConfig { threshold_usd: 10.0 },
        retries: RetriesConfig { max_retries: 3 },
        rate_limit: RateLimitConfig { calls_per_minute: 60 },
        agent_health: AgentHealthConfig { max_failures_24h: 5 },
        concurrency: ConcurrencyConfig { max_total_concurrent: 5 },
        sla: SlaConfig { buffer_multiplier: 0.0 },
    };
    let err = validate_invariants(&inv).unwrap_err();
    assert!(err.to_string().contains("buffer_multiplier"));
}

#[test]
fn validate_invariants_negative_budget() {
    let inv = InvariantConfig {
        budget: BudgetConfig { threshold_usd: -5.0 },
        retries: RetriesConfig { max_retries: 3 },
        rate_limit: RateLimitConfig { calls_per_minute: 60 },
        agent_health: AgentHealthConfig { max_failures_24h: 5 },
        concurrency: ConcurrencyConfig { max_total_concurrent: 5 },
        sla: SlaConfig { buffer_multiplier: 1.5 },
    };
    let err = validate_invariants(&inv).unwrap_err();
    assert!(err.to_string().contains("threshold_usd"));
}

#[test]
fn validate_valid_config_passes() {
    let dir = tempfile::tempdir().unwrap();
    write_valid_config(dir.path());
    // load_config now calls validate internally
    load_config(dir.path()).unwrap();
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p arbiter-mcp config::tests::validate`
Expected: FAIL — `validate_agents` and `validate_invariants` functions don't exist yet.

- [ ] **Step 3: Implement validation functions**

Add these functions to `arbiter-mcp/src/config.rs` (before `load_config`):

```rust
/// Validate agent configs for semantic correctness.
///
/// Checks:
/// - `max_concurrent > 0`
/// - `cost_per_hour > 0`
/// - `avg_duration_min > 0`
/// - `supports_languages` non-empty
/// - `supports_types` non-empty
pub fn validate_agents(
    agents: &HashMap<String, AgentConfig>,
) -> Result<()> {
    for (id, config) in agents {
        if config.max_concurrent == 0 {
            bail!(
                "Agent '{}': max_concurrent must be > 0",
                id
            );
        }
        if config.cost_per_hour <= 0.0 {
            bail!(
                "Agent '{}': cost_per_hour must be > 0, got {}",
                id,
                config.cost_per_hour
            );
        }
        if config.avg_duration_min <= 0.0 {
            bail!(
                "Agent '{}': avg_duration_min must be > 0, got {}",
                id,
                config.avg_duration_min
            );
        }
        if config.supports_languages.is_empty() {
            bail!(
                "Agent '{}': supports_languages must not be empty",
                id
            );
        }
        if config.supports_types.is_empty() {
            bail!(
                "Agent '{}': supports_types must not be empty",
                id
            );
        }
    }
    Ok(())
}

/// Validate invariant config for semantic correctness.
///
/// Checks:
/// - `max_total_concurrent > 0`
/// - `buffer_multiplier > 0`
/// - `threshold_usd >= 0`
/// - `calls_per_minute > 0`
pub fn validate_invariants(config: &InvariantConfig) -> Result<()> {
    if config.concurrency.max_total_concurrent == 0 {
        bail!(
            "Invariant concurrency.max_total_concurrent must be > 0"
        );
    }
    if config.sla.buffer_multiplier <= 0.0 {
        bail!(
            "Invariant sla.buffer_multiplier must be > 0, got {}",
            config.sla.buffer_multiplier
        );
    }
    if config.budget.threshold_usd < 0.0 {
        bail!(
            "Invariant budget.threshold_usd must be >= 0, got {}",
            config.budget.threshold_usd
        );
    }
    if config.rate_limit.calls_per_minute == 0 {
        bail!(
            "Invariant rate_limit.calls_per_minute must be > 0"
        );
    }
    Ok(())
}
```

- [ ] **Step 4: Wire validation into load_config**

Update `load_config` to call validation after parsing:

```rust
pub fn load_config(config_dir: &Path) -> Result<ArbiterConfig> {
    let agents = load_agents(config_dir)?;
    validate_agents(&agents)?;
    let invariants = load_invariants(config_dir)?;
    validate_invariants(&invariants)?;
    Ok(ArbiterConfig { agents, invariants })
}
```

- [ ] **Step 5: Add `bail` import if not present**

Ensure `use anyhow::bail;` is in the imports at the top of `config.rs`. It already has `use anyhow::{bail, Context, Result};`.

- [ ] **Step 6: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass (245 original + new validation tests).

- [ ] **Step 7: Commit**

```bash
git add arbiter-mcp/src/config.rs
git commit -m "feat: add semantic config validation for agents and invariants"
```

---

### Task 4: Clean Up Dependencies

**Files:**
- Modify: `arbiter-mcp/Cargo.toml`
- Modify: `Cargo.toml` (workspace)

`tokio` is declared in arbiter-mcp but unused in source code. It will be needed in R4 for signal handling (SIGTERM/SIGHUP). Decision: **keep it in workspace deps** but **remove from arbiter-mcp** until R4 actually needs it. This keeps the build clean.

- [ ] **Step 1: Verify tokio is unused**

Run: `grep -r "tokio" arbiter-mcp/src/`
Expected: No matches.

- [ ] **Step 2: Remove tokio from arbiter-mcp/Cargo.toml**

Remove the `tokio = { workspace = true }` line from `[dependencies]`:

```toml
[dependencies]
arbiter-core = { path = "../arbiter-core" }
serde = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
anyhow = { workspace = true }
thiserror = { workspace = true }
rusqlite = { workspace = true }
toml = { workspace = true }
chrono = { workspace = true }
```

- [ ] **Step 3: Verify build**

Run: `cargo build --workspace`
Expected: Clean build with no errors.

- [ ] **Step 4: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: Clean. No warnings.

- [ ] **Step 5: Run full tests**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add arbiter-mcp/Cargo.toml
git commit -m "chore: remove unused tokio dep from arbiter-mcp (re-add in R4)"
```

---

### Task 5: Bootstrap Cross-Validation

**Files:**
- Modify: `scripts/bootstrap_agent_tree.py`

Add k-fold cross-validation to verify the bootstrap tree generalizes and isn't just memorizing training data. The spec requires accuracy > 90% on held-out data.

- [ ] **Step 1: Add cross-validation to `train_and_export`**

In `scripts/bootstrap_agent_tree.py`, add the import at the top:

```python
from sklearn.model_selection import cross_val_score
```

In `train_and_export()`, after the line `X, y = inject_noise(X_raw, y_raw, rng, noise_scale=0.05)` and before `clf = DecisionTreeClassifier(...)`, add:

```python
    # Cross-validation on noised data
    cv_clf = DecisionTreeClassifier(
        max_depth=7,
        min_samples_leaf=5,
        random_state=seed,
    )
    cv_scores = cross_val_score(cv_clf, X, y, cv=5, scoring="accuracy")
    cv_mean = cv_scores.mean()
    cv_std = cv_scores.std()
    print(f"\n5-fold CV accuracy: {cv_mean:.4f} (+/- {cv_std:.4f})")
    print(f"  Per-fold: {[f'{s:.4f}' for s in cv_scores]}")

    assert cv_mean > 0.90, (
        f"Cross-validation accuracy {cv_mean:.4f} below 90% threshold"
    )
```

- [ ] **Step 2: Add CV stats to return dict**

In the `return` dict at the end of `train_and_export()`, add the CV fields:

```python
    return {
        "accuracy": accuracy,
        "cv_mean": cv_mean,
        "cv_std": cv_std,
        "depth": clf.get_depth(),
        "node_count": clf.tree_.node_count,
        "n_examples": len(X),
        "output_path": output_path,
    }
```

- [ ] **Step 3: Update the CLI check**

In `main()`, after `stats = train_and_export(...)`, add:

```python
    if stats["cv_mean"] < 0.90:
        print(
            f"\nWARNING: CV accuracy {stats['cv_mean']:.4f} below 90%!",
            file=sys.stderr,
        )
        sys.exit(1)
```

- [ ] **Step 4: Run the bootstrap script**

Run: `cd /Users/Andrei_Shtanakov/labs/all_ai_orchestrators/arbiter && uv run python scripts/bootstrap_agent_tree.py`
Expected: Output includes "5-fold CV accuracy: 0.9X+", script exits 0, `models/agent_policy_tree.json` is regenerated.

- [ ] **Step 5: Verify Rust tests still pass with regenerated tree**

Run: `cargo test --workspace`
Expected: All tests pass (the bootstrap tree tests in `decision_tree.rs` use the JSON file).

- [ ] **Step 6: Commit**

```bash
git add scripts/bootstrap_agent_tree.py
git commit -m "feat: add 5-fold cross-validation to bootstrap tree script"
```

---

## Exit Criteria Checklist

- [ ] `cargo clippy --workspace -- -D warnings` — clean
- [ ] `cargo test --workspace` — all tests pass (250+ tests)
- [ ] `anyhow` removed from arbiter-core deps
- [ ] `thiserror` used in arbiter-core for `ArbiterError`
- [ ] `InferenceBackend` trait implemented by `DecisionTree`
- [ ] `DecisionStore` and `AgentStore` traits defined
- [ ] Config validation catches: `max_concurrent=0`, negative costs, empty capabilities, `buffer_multiplier=0`
- [ ] `tokio` removed from arbiter-mcp (kept in workspace for R4)
- [ ] Bootstrap script reports 5-fold CV accuracy > 90%
