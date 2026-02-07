//! TOML configuration loader for agents and invariant thresholds.
//!
//! Reads `agents.toml` and `invariants.toml` from the config directory
//! and validates that all required fields are present.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Agent configuration
// ---------------------------------------------------------------------------

/// Configuration for a single coding agent, parsed from agents.toml.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Human-readable name (e.g. "Claude Code").
    pub display_name: String,
    /// Languages the agent supports (e.g. ["python", "rust"]).
    pub supports_languages: Vec<String>,
    /// Task types the agent supports (e.g. ["feature", "bugfix"]).
    pub supports_types: Vec<String>,
    /// Maximum concurrent tasks this agent can handle.
    pub max_concurrent: u32,
    /// Cost per hour in USD.
    pub cost_per_hour: f64,
    /// Average task duration in minutes.
    pub avg_duration_min: f64,
}

// ---------------------------------------------------------------------------
// Invariant configuration
// ---------------------------------------------------------------------------

/// Budget invariant thresholds.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct BudgetConfig {
    /// Minimum budget remaining to allow assignment (USD).
    pub threshold_usd: f64,
}

/// Retry invariant thresholds.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RetriesConfig {
    /// Maximum retry attempts before rejecting.
    pub max_retries: u32,
}

/// Rate-limit invariant thresholds.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RateLimitConfig {
    /// Maximum calls per minute.
    pub calls_per_minute: u32,
}

/// Agent health invariant thresholds.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AgentHealthConfig {
    /// Maximum failures in the last 24 hours.
    pub max_failures_24h: u32,
}

/// Concurrency invariant thresholds.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ConcurrencyConfig {
    /// Maximum total concurrent tasks across all agents.
    pub max_total_concurrent: u32,
}

/// SLA invariant thresholds.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SlaConfig {
    /// Multiplier applied to estimated duration for SLA feasibility.
    pub buffer_multiplier: f64,
}

/// Top-level invariant configuration, parsed from invariants.toml.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct InvariantConfig {
    pub budget: BudgetConfig,
    pub retries: RetriesConfig,
    pub rate_limit: RateLimitConfig,
    pub agent_health: AgentHealthConfig,
    pub concurrency: ConcurrencyConfig,
    pub sla: SlaConfig,
}

// ---------------------------------------------------------------------------
// Combined configuration
// ---------------------------------------------------------------------------

/// All configuration loaded from the config directory.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ArbiterConfig {
    /// Agent definitions keyed by agent ID (e.g. "claude_code").
    pub agents: HashMap<String, AgentConfig>,
    /// Invariant rule thresholds.
    pub invariants: InvariantConfig,
}

// ---------------------------------------------------------------------------
// Loaders
// ---------------------------------------------------------------------------

/// Load agent configuration from `agents.toml` in the given directory.
///
/// Returns a map of agent ID to `AgentConfig`.
/// Errors if the file is missing or any required field is absent.
pub fn load_agents(config_dir: &Path) -> Result<HashMap<String, AgentConfig>> {
    let path = config_dir.join("agents.toml");
    if !path.exists() {
        bail!("Config not found: {}", path.display());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let agents: HashMap<String, AgentConfig> =
        toml::from_str(&content).with_context(|| format!("Failed to parse {}", path.display()))?;
    if agents.is_empty() {
        bail!("No agents defined in {}", path.display());
    }
    Ok(agents)
}

/// Load invariant configuration from `invariants.toml` in the given directory.
///
/// Errors if the file is missing or any required field is absent.
pub fn load_invariants(config_dir: &Path) -> Result<InvariantConfig> {
    let path = config_dir.join("invariants.toml");
    if !path.exists() {
        bail!("Config not found: {}", path.display());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let config: InvariantConfig =
        toml::from_str(&content).with_context(|| format!("Failed to parse {}", path.display()))?;
    Ok(config)
}

/// Load all configuration from the given directory.
///
/// Reads `agents.toml` and `invariants.toml`, validates required fields.
pub fn load_config(config_dir: &Path) -> Result<ArbiterConfig> {
    let agents = load_agents(config_dir)?;
    let invariants = load_invariants(config_dir)?;
    Ok(ArbiterConfig { agents, invariants })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Helper: create a temp dir with valid config files.
    fn write_valid_config(dir: &Path) {
        let agents = r#"
[claude_code]
display_name = "Claude Code"
supports_languages = ["python", "rust", "typescript"]
supports_types = ["feature", "bugfix", "refactor", "docs", "review"]
max_concurrent = 2
cost_per_hour = 0.30
avg_duration_min = 18.0

[codex_cli]
display_name = "Codex CLI"
supports_languages = ["typescript", "go", "python"]
supports_types = ["feature", "bugfix", "refactor", "test"]
max_concurrent = 3
cost_per_hour = 0.20
avg_duration_min = 12.0

[aider]
display_name = "Aider"
supports_languages = ["python", "javascript"]
supports_types = ["bugfix", "refactor", "test"]
max_concurrent = 5
cost_per_hour = 0.10
avg_duration_min = 8.0
"#;
        let invariants = r#"
[budget]
threshold_usd = 10.0

[retries]
max_retries = 3

[rate_limit]
calls_per_minute = 60

[agent_health]
max_failures_24h = 5

[concurrency]
max_total_concurrent = 5

[sla]
buffer_multiplier = 1.5
"#;
        fs::write(dir.join("agents.toml"), agents).unwrap();
        fs::write(dir.join("invariants.toml"), invariants).unwrap();
    }

    // UT-19: Config parsing valid TOML

    #[test]
    fn load_agents_valid_toml() {
        let dir = tempfile::tempdir().unwrap();
        write_valid_config(dir.path());

        let agents = load_agents(dir.path()).unwrap();
        assert_eq!(agents.len(), 3);

        let claude = &agents["claude_code"];
        assert_eq!(claude.display_name, "Claude Code");
        assert_eq!(
            claude.supports_languages,
            vec!["python", "rust", "typescript"]
        );
        assert_eq!(
            claude.supports_types,
            vec!["feature", "bugfix", "refactor", "docs", "review"]
        );
        assert_eq!(claude.max_concurrent, 2);
        assert!((claude.cost_per_hour - 0.30).abs() < f64::EPSILON);
        assert!((claude.avg_duration_min - 18.0).abs() < f64::EPSILON);

        let codex = &agents["codex_cli"];
        assert_eq!(codex.display_name, "Codex CLI");
        assert_eq!(codex.max_concurrent, 3);

        let aider = &agents["aider"];
        assert_eq!(aider.display_name, "Aider");
        assert_eq!(aider.max_concurrent, 5);
    }

    #[test]
    fn load_invariants_valid_toml() {
        let dir = tempfile::tempdir().unwrap();
        write_valid_config(dir.path());

        let inv = load_invariants(dir.path()).unwrap();
        assert!((inv.budget.threshold_usd - 10.0).abs() < f64::EPSILON);
        assert_eq!(inv.retries.max_retries, 3);
        assert_eq!(inv.rate_limit.calls_per_minute, 60);
        assert_eq!(inv.agent_health.max_failures_24h, 5);
        assert_eq!(inv.concurrency.max_total_concurrent, 5);
        assert!((inv.sla.buffer_multiplier - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn load_config_valid_toml() {
        let dir = tempfile::tempdir().unwrap();
        write_valid_config(dir.path());

        let config = load_config(dir.path()).unwrap();
        assert_eq!(config.agents.len(), 3);
        assert_eq!(config.invariants.retries.max_retries, 3);
    }

    // UT-20: Config parsing missing field error

    #[test]
    fn load_agents_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let err = load_agents(dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Config not found"),
            "Expected 'Config not found' in: {msg}"
        );
    }

    #[test]
    fn load_invariants_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let err = load_invariants(dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Config not found"),
            "Expected 'Config not found' in: {msg}"
        );
    }

    #[test]
    fn load_agents_missing_required_field() {
        let dir = tempfile::tempdir().unwrap();
        // Missing display_name
        let toml = r#"
[bad_agent]
supports_languages = ["python"]
supports_types = ["bugfix"]
max_concurrent = 1
cost_per_hour = 0.10
"#;
        fs::write(dir.path().join("agents.toml"), toml).unwrap();
        let err = load_agents(dir.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("display_name") || msg.contains("missing field"),
            "Expected field name in error: {msg}"
        );
    }

    #[test]
    fn load_agents_missing_another_required_field() {
        let dir = tempfile::tempdir().unwrap();
        // Missing max_concurrent
        let toml = r#"
[bad_agent]
display_name = "Bad"
supports_languages = ["python"]
supports_types = ["bugfix"]
cost_per_hour = 0.10
avg_duration_min = 5.0
"#;
        fs::write(dir.path().join("agents.toml"), toml).unwrap();
        let err = load_agents(dir.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("max_concurrent") || msg.contains("missing field"),
            "Expected field name in error: {msg}"
        );
    }

    #[test]
    fn load_invariants_missing_section() {
        let dir = tempfile::tempdir().unwrap();
        // Missing [sla] section
        let toml = r#"
[budget]
threshold_usd = 10.0

[retries]
max_retries = 3

[rate_limit]
calls_per_minute = 60

[agent_health]
max_failures_24h = 5

[concurrency]
max_total_concurrent = 5
"#;
        fs::write(dir.path().join("invariants.toml"), toml).unwrap();
        let err = load_invariants(dir.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("sla") || msg.contains("missing field"),
            "Expected 'sla' in error: {msg}"
        );
    }

    #[test]
    fn load_invariants_missing_nested_field() {
        let dir = tempfile::tempdir().unwrap();
        // [budget] missing threshold_usd
        let toml = r#"
[budget]

[retries]
max_retries = 3

[rate_limit]
calls_per_minute = 60

[agent_health]
max_failures_24h = 5

[concurrency]
max_total_concurrent = 5

[sla]
buffer_multiplier = 1.5
"#;
        fs::write(dir.path().join("invariants.toml"), toml).unwrap();
        let err = load_invariants(dir.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("threshold_usd") || msg.contains("missing field"),
            "Expected field name in error: {msg}"
        );
    }
}
