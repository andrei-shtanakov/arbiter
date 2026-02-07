//! get_agent_status tool implementation.
//!
//! Queries agent capabilities, current load, and performance history.
//! Supports querying a single agent by ID or all registered agents.

use std::collections::HashMap;

use anyhow::{bail, Result};
use serde_json::Value;
use tracing::{debug, info};

use crate::agents::AgentRegistry;
use crate::db::Database;

/// Result of the get_agent_status operation.
#[derive(Debug)]
pub struct StatusResult {
    pub agents: Vec<AgentStatus>,
}

/// Status information for a single agent.
#[derive(Debug)]
pub struct AgentStatus {
    pub id: String,
    pub display_name: String,
    pub state: String,
    pub capabilities: Capabilities,
    pub current_load: CurrentLoad,
    pub performance: Performance,
}

/// Static agent capabilities from config.
#[derive(Debug)]
pub struct Capabilities {
    pub languages: Vec<String>,
    pub task_types: Vec<String>,
    pub max_concurrent: u32,
    pub cost_per_hour: f64,
}

/// Current agent load information.
#[derive(Debug)]
pub struct CurrentLoad {
    pub running_tasks: u32,
    pub available_slots: u32,
}

/// Performance statistics for an agent.
#[derive(Debug)]
pub struct Performance {
    pub total_tasks: i64,
    pub success_rate: f64,
    pub avg_duration_min: f64,
    pub avg_cost_usd: f64,
    pub by_language: HashMap<String, CategoryStats>,
    pub by_type: HashMap<String, CategoryStats>,
}

/// Stats for a single language or task type category.
#[derive(Debug)]
pub struct CategoryStats {
    pub tasks: i64,
    pub success_rate: f64,
}

/// Determine agent state based on runtime metrics.
///
/// State FSM (MVP):
/// - `running_tasks >= max_concurrent` → "busy"
/// - `recent_failures > threshold` → "failed"
/// - otherwise → "active"
fn determine_state(
    running_tasks: u32,
    max_concurrent: u32,
    recent_failures: u32,
    max_failures_threshold: u32,
) -> String {
    if recent_failures > max_failures_threshold {
        "failed".to_string()
    } else if running_tasks >= max_concurrent {
        "busy".to_string()
    } else {
        "active".to_string()
    }
}

/// Execute the get_agent_status logic.
///
/// - If `agent_id` is provided, returns status for that single agent.
/// - If `agent_id` is absent, returns status for all registered agents.
/// - Returns an error if an unknown `agent_id` is specified.
pub fn execute(
    args: &Value,
    db: &Database,
    registry: &AgentRegistry,
    max_failures_24h: u32,
) -> Result<StatusResult> {
    let agent_id = args.get("agent_id").and_then(|v| v.as_str());

    let agents = match agent_id {
        Some(id) => {
            debug!(agent_id = id, "querying single agent status");
            let info = registry.get_agent_info(id)?;
            match info {
                Some(info) => vec![info],
                None => bail!("agent not found: {id}"),
            }
        }
        None => {
            debug!("querying all agent status");
            registry.get_all_agent_info()?
        }
    };

    let mut statuses = Vec::with_capacity(agents.len());

    for agent in &agents {
        let stats = db.get_agent_stats(&agent.agent_id)?;
        let (by_lang_raw, by_type_raw) = db.get_agent_stats_by_category(&agent.agent_id)?;

        let state = determine_state(
            agent.running_tasks,
            agent.config.max_concurrent,
            agent.recent_failures,
            max_failures_24h,
        );

        let available_slots = agent
            .config
            .max_concurrent
            .saturating_sub(agent.running_tasks);

        let mut by_language = HashMap::new();
        for (lang, total, successful) in &by_lang_raw {
            let rate = if *total > 0 {
                *successful as f64 / *total as f64
            } else {
                0.0
            };
            by_language.insert(
                lang.clone(),
                CategoryStats {
                    tasks: *total,
                    success_rate: rate,
                },
            );
        }

        let mut by_type = HashMap::new();
        for (tt, total, successful) in &by_type_raw {
            let rate = if *total > 0 {
                *successful as f64 / *total as f64
            } else {
                0.0
            };
            by_type.insert(
                tt.clone(),
                CategoryStats {
                    tasks: *total,
                    success_rate: rate,
                },
            );
        }

        statuses.push(AgentStatus {
            id: agent.agent_id.clone(),
            display_name: agent.config.display_name.clone(),
            state,
            capabilities: Capabilities {
                languages: agent.config.supports_languages.clone(),
                task_types: agent.config.supports_types.clone(),
                max_concurrent: agent.config.max_concurrent,
                cost_per_hour: agent.config.cost_per_hour,
            },
            current_load: CurrentLoad {
                running_tasks: agent.running_tasks,
                available_slots,
            },
            performance: Performance {
                total_tasks: stats.total_tasks,
                success_rate: stats.success_rate,
                avg_duration_min: stats.avg_duration_min,
                avg_cost_usd: stats.avg_cost_usd,
                by_language,
                by_type,
            },
        });
    }

    info!(count = statuses.len(), "get_agent_status returned");

    Ok(StatusResult { agents: statuses })
}

/// Serialize a StatusResult into the MCP response JSON Value.
pub fn result_to_json(result: &StatusResult) -> Value {
    let agents: Vec<Value> = result
        .agents
        .iter()
        .map(|a| {
            let by_language: HashMap<&str, Value> = a
                .performance
                .by_language
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str(),
                        serde_json::json!({
                            "tasks": v.tasks,
                            "success_rate": v.success_rate,
                        }),
                    )
                })
                .collect();

            let by_type: HashMap<&str, Value> = a
                .performance
                .by_type
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str(),
                        serde_json::json!({
                            "tasks": v.tasks,
                            "success_rate": v.success_rate,
                        }),
                    )
                })
                .collect();

            serde_json::json!({
                "id": a.id,
                "display_name": a.display_name,
                "state": a.state,
                "capabilities": {
                    "languages": a.capabilities.languages,
                    "task_types": a.capabilities.task_types,
                    "max_concurrent": a.capabilities.max_concurrent,
                    "cost_per_hour": a.capabilities.cost_per_hour,
                },
                "current_load": {
                    "running_tasks": a.current_load.running_tasks,
                    "available_slots": a.current_load.available_slots,
                },
                "performance": {
                    "total_tasks": a.performance.total_tasks,
                    "success_rate": a.performance.success_rate,
                    "avg_duration_min": a.performance.avg_duration_min,
                    "avg_cost_usd": a.performance.avg_cost_usd,
                    "by_language": by_language,
                    "by_type": by_type,
                },
            })
        })
        .collect();

    serde_json::json!({ "agents": agents })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentRegistry;
    use crate::config::AgentConfig;
    use crate::db::{Database, OutcomeRecord};
    use std::collections::HashMap;

    fn test_agents() -> HashMap<String, AgentConfig> {
        let mut agents = HashMap::new();
        agents.insert(
            "claude_code".to_string(),
            AgentConfig {
                display_name: "Claude Code".to_string(),
                supports_languages: vec!["python".to_string(), "rust".to_string()],
                supports_types: vec!["feature".to_string(), "bugfix".to_string()],
                max_concurrent: 2,
                cost_per_hour: 0.30,
                avg_duration_min: 18.0,
            },
        );
        agents.insert(
            "codex_cli".to_string(),
            AgentConfig {
                display_name: "Codex CLI".to_string(),
                supports_languages: vec!["typescript".to_string(), "python".to_string()],
                supports_types: vec![
                    "feature".to_string(),
                    "bugfix".to_string(),
                    "test".to_string(),
                ],
                max_concurrent: 3,
                cost_per_hour: 0.20,
                avg_duration_min: 12.0,
            },
        );
        agents.insert(
            "aider".to_string(),
            AgentConfig {
                display_name: "Aider".to_string(),
                supports_languages: vec!["python".to_string()],
                supports_types: vec!["bugfix".to_string(), "refactor".to_string()],
                max_concurrent: 5,
                cost_per_hour: 0.10,
                avg_duration_min: 8.0,
            },
        );
        agents
    }

    fn setup() -> (Database, HashMap<String, AgentConfig>) {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let agents = test_agents();
        (db, agents)
    }

    // -- AC-4.4.1: All agents returned --

    #[test]
    fn all_agents_returned_without_params() {
        let (db, agents) = setup();
        let registry = AgentRegistry::new(&db, &agents).unwrap();

        let args = serde_json::json!({});
        let result = execute(&args, &db, &registry, 5).unwrap();

        assert_eq!(result.agents.len(), 3);
        let ids: Vec<&str> = result.agents.iter().map(|a| a.id.as_str()).collect();
        assert!(ids.contains(&"claude_code"));
        assert!(ids.contains(&"codex_cli"));
        assert!(ids.contains(&"aider"));
    }

    // -- AC-4.4.2: Single agent returned --

    #[test]
    fn single_agent_returned_with_agent_id() {
        let (db, agents) = setup();
        let registry = AgentRegistry::new(&db, &agents).unwrap();

        let args = serde_json::json!({ "agent_id": "claude_code" });
        let result = execute(&args, &db, &registry, 5).unwrap();

        assert_eq!(result.agents.len(), 1);
        assert_eq!(result.agents[0].id, "claude_code");
        assert_eq!(result.agents[0].display_name, "Claude Code");
    }

    // -- AC-4.4.2: Unknown agent_id → error --

    #[test]
    fn unknown_agent_returns_error() {
        let (db, agents) = setup();
        let registry = AgentRegistry::new(&db, &agents).unwrap();

        let args = serde_json::json!({ "agent_id": "nonexistent" });
        let result = execute(&args, &db, &registry, 5);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("agent not found"),
            "expected 'agent not found' in: {err}"
        );
    }

    // -- AC-4.4.4: Empty stats (fresh start) --

    #[test]
    fn empty_stats_fresh_start() {
        let (db, agents) = setup();
        let registry = AgentRegistry::new(&db, &agents).unwrap();

        let args = serde_json::json!({ "agent_id": "claude_code" });
        let result = execute(&args, &db, &registry, 5).unwrap();

        let agent = &result.agents[0];
        assert_eq!(agent.performance.total_tasks, 0);
        assert!((agent.performance.success_rate - 0.0).abs() < f64::EPSILON);
        assert!(agent.performance.by_language.is_empty());
        assert!(agent.performance.by_type.is_empty());
        assert_eq!(agent.current_load.running_tasks, 0);
        assert_eq!(agent.current_load.available_slots, 2);
        assert_eq!(agent.state, "active");
    }

    // -- Capabilities from config --

    #[test]
    fn capabilities_from_config() {
        let (db, agents) = setup();
        let registry = AgentRegistry::new(&db, &agents).unwrap();

        let args = serde_json::json!({ "agent_id": "claude_code" });
        let result = execute(&args, &db, &registry, 5).unwrap();

        let agent = &result.agents[0];
        assert_eq!(agent.capabilities.languages, vec!["python", "rust"]);
        assert_eq!(agent.capabilities.task_types, vec!["feature", "bugfix"]);
        assert_eq!(agent.capabilities.max_concurrent, 2);
        assert!((agent.capabilities.cost_per_hour - 0.30).abs() < f64::EPSILON);
    }

    // -- Current load --

    #[test]
    fn current_load_reflects_running_tasks() {
        let (db, agents) = setup();
        let registry = AgentRegistry::new(&db, &agents).unwrap();

        db.increment_running_tasks("claude_code").unwrap();

        let args = serde_json::json!({ "agent_id": "claude_code" });
        let result = execute(&args, &db, &registry, 5).unwrap();

        let agent = &result.agents[0];
        assert_eq!(agent.current_load.running_tasks, 1);
        assert_eq!(agent.current_load.available_slots, 1);
        assert_eq!(agent.state, "active");
    }

    // -- State: busy when at capacity --

    #[test]
    fn state_busy_at_capacity() {
        let (db, agents) = setup();
        let registry = AgentRegistry::new(&db, &agents).unwrap();

        // Fill both slots (max_concurrent = 2)
        db.increment_running_tasks("claude_code").unwrap();
        db.increment_running_tasks("claude_code").unwrap();

        let args = serde_json::json!({ "agent_id": "claude_code" });
        let result = execute(&args, &db, &registry, 5).unwrap();

        let agent = &result.agents[0];
        assert_eq!(agent.state, "busy");
        assert_eq!(agent.current_load.running_tasks, 2);
        assert_eq!(agent.current_load.available_slots, 0);
    }

    // -- AC-4.4.3: Stats from database, by_language and by_type --

    #[test]
    fn stats_reflect_outcomes_with_by_language_and_by_type() {
        let (db, agents) = setup();
        let registry = AgentRegistry::new(&db, &agents).unwrap();

        // Record outcomes for claude_code
        let outcome1 = OutcomeRecord {
            task_id: "t1".to_string(),
            decision_id: None,
            agent_id: "claude_code".to_string(),
            status: "success".to_string(),
            duration_min: Some(10.0),
            tokens_used: Some(1000),
            cost_usd: Some(0.10),
            exit_code: Some(0),
            files_changed: Some(1),
            tests_passed: Some(true),
            validation_passed: Some(true),
            error_summary: None,
            retry_count: 0,
        };
        db.insert_outcome(&outcome1).unwrap();
        db.update_agent_stats("claude_code", "bugfix", "python", &outcome1)
            .unwrap();

        let outcome2 = OutcomeRecord {
            task_id: "t2".to_string(),
            decision_id: None,
            agent_id: "claude_code".to_string(),
            status: "success".to_string(),
            duration_min: Some(20.0),
            tokens_used: Some(2000),
            cost_usd: Some(0.20),
            exit_code: Some(0),
            files_changed: Some(2),
            tests_passed: Some(true),
            validation_passed: Some(true),
            error_summary: None,
            retry_count: 0,
        };
        db.insert_outcome(&outcome2).unwrap();
        db.update_agent_stats("claude_code", "feature", "rust", &outcome2)
            .unwrap();

        let outcome3 = OutcomeRecord {
            task_id: "t3".to_string(),
            decision_id: None,
            agent_id: "claude_code".to_string(),
            status: "failure".to_string(),
            duration_min: Some(5.0),
            tokens_used: Some(500),
            cost_usd: Some(0.05),
            exit_code: Some(1),
            files_changed: Some(0),
            tests_passed: Some(false),
            validation_passed: Some(false),
            error_summary: Some("test failed".to_string()),
            retry_count: 0,
        };
        db.insert_outcome(&outcome3).unwrap();
        db.update_agent_stats("claude_code", "bugfix", "python", &outcome3)
            .unwrap();

        let args = serde_json::json!({ "agent_id": "claude_code" });
        let result = execute(&args, &db, &registry, 5).unwrap();

        let agent = &result.agents[0];
        assert_eq!(agent.performance.total_tasks, 3);

        // by_language: python has 2 tasks (1 success, 1 failure)
        let python_stats = agent
            .performance
            .by_language
            .get("python")
            .expect("python stats should exist");
        assert_eq!(python_stats.tasks, 2);
        assert!((python_stats.success_rate - 0.5).abs() < f64::EPSILON);

        // by_language: rust has 1 task (1 success)
        let rust_stats = agent
            .performance
            .by_language
            .get("rust")
            .expect("rust stats should exist");
        assert_eq!(rust_stats.tasks, 1);
        assert!((rust_stats.success_rate - 1.0).abs() < f64::EPSILON);

        // by_type: bugfix has 2 tasks (1 success, 1 failure)
        let bugfix_stats = agent
            .performance
            .by_type
            .get("bugfix")
            .expect("bugfix stats should exist");
        assert_eq!(bugfix_stats.tasks, 2);
        assert!((bugfix_stats.success_rate - 0.5).abs() < f64::EPSILON);

        // by_type: feature has 1 task (1 success)
        let feature_stats = agent
            .performance
            .by_type
            .get("feature")
            .expect("feature stats should exist");
        assert_eq!(feature_stats.tasks, 1);
        assert!((feature_stats.success_rate - 1.0).abs() < f64::EPSILON);
    }

    // -- result_to_json structure --

    #[test]
    fn result_to_json_structure() {
        let result = StatusResult {
            agents: vec![AgentStatus {
                id: "claude_code".to_string(),
                display_name: "Claude Code".to_string(),
                state: "active".to_string(),
                capabilities: Capabilities {
                    languages: vec!["python".to_string(), "rust".to_string()],
                    task_types: vec!["feature".to_string(), "bugfix".to_string()],
                    max_concurrent: 2,
                    cost_per_hour: 0.30,
                },
                current_load: CurrentLoad {
                    running_tasks: 0,
                    available_slots: 2,
                },
                performance: Performance {
                    total_tasks: 0,
                    success_rate: 0.0,
                    avg_duration_min: 0.0,
                    avg_cost_usd: 0.0,
                    by_language: HashMap::new(),
                    by_type: HashMap::new(),
                },
            }],
        };

        let json = result_to_json(&result);
        let agents = json["agents"].as_array().unwrap();
        assert_eq!(agents.len(), 1);

        let a = &agents[0];
        assert_eq!(a["id"], "claude_code");
        assert_eq!(a["display_name"], "Claude Code");
        assert_eq!(a["state"], "active");
        assert_eq!(
            a["capabilities"]["languages"],
            serde_json::json!(["python", "rust"])
        );
        assert_eq!(a["capabilities"]["max_concurrent"], 2);
        assert_eq!(a["current_load"]["running_tasks"], 0);
        assert_eq!(a["current_load"]["available_slots"], 2);
        assert_eq!(a["performance"]["total_tasks"], 0);
        assert_eq!(a["performance"]["by_language"], serde_json::json!({}));
        assert_eq!(a["performance"]["by_type"], serde_json::json!({}));
    }

    // -- determine_state tests --

    #[test]
    fn state_determination() {
        // Active: no issues
        assert_eq!(determine_state(0, 2, 0, 5), "active");

        // Active: running but not full
        assert_eq!(determine_state(1, 2, 0, 5), "active");

        // Busy: at max concurrent
        assert_eq!(determine_state(2, 2, 0, 5), "busy");

        // Busy: over max concurrent
        assert_eq!(determine_state(3, 2, 0, 5), "busy");

        // Failed: too many failures (even with capacity)
        assert_eq!(determine_state(0, 2, 6, 5), "failed");

        // Failed takes priority over busy
        assert_eq!(determine_state(2, 2, 6, 5), "failed");

        // At exactly threshold → still active
        assert_eq!(determine_state(0, 2, 5, 5), "active");
    }
}
