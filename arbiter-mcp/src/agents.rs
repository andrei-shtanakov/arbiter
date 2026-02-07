//! Agent registry backed by SQLite.
//!
//! Loads agent definitions from TOML config, upserts them into the database,
//! and provides runtime queries for agent state, stats, and load.

use std::collections::HashMap;

use anyhow::Result;
use tracing::info;

use crate::config::AgentConfig;
use crate::db::Database;
use crate::features::AgentInfo;

// ---------------------------------------------------------------------------
// AgentRegistry
// ---------------------------------------------------------------------------

/// Agent registry that persists agent state in SQLite.
///
/// On construction, loads agents from config and upserts them into the
/// database. At runtime, provides queries for feature vector building,
/// invariant checks, and status reporting.
pub struct AgentRegistry<'a> {
    db: &'a Database,
    configs: HashMap<String, AgentConfig>,
}

impl<'a> AgentRegistry<'a> {
    /// Create a registry from config agents, upserting each into the database.
    pub fn new(db: &'a Database, agents: &HashMap<String, AgentConfig>) -> Result<Self> {
        for (id, config) in agents {
            let config_json = serde_json::to_string(config)?;
            db.upsert_agent(
                id,
                &config.display_name,
                config.max_concurrent,
                &config_json,
            )?;
        }

        info!(count = agents.len(), "agents registered in database");

        Ok(Self {
            db,
            configs: agents.clone(),
        })
    }

    /// Build an [`AgentInfo`] for the given agent, combining config with live stats.
    ///
    /// Returns `None` if the agent ID is unknown.
    pub fn get_agent_info(&self, agent_id: &str) -> Result<Option<AgentInfo>> {
        let config = match self.configs.get(agent_id) {
            Some(c) => c.clone(),
            None => return Ok(None),
        };

        let running_tasks = self.db.get_running_tasks(agent_id)?;
        let stats = self.db.get_agent_stats(agent_id)?;
        let recent_failures = self.db.get_recent_failures(agent_id, 24)?;

        let success_rate = if stats.total_tasks > 0 {
            Some(stats.success_rate)
        } else {
            None
        };
        let avg_duration = if stats.total_tasks > 0 {
            Some(stats.avg_duration_min)
        } else {
            None
        };
        let avg_cost = if stats.total_tasks > 0 {
            Some(stats.avg_cost_usd)
        } else {
            None
        };

        Ok(Some(AgentInfo {
            agent_id: agent_id.to_string(),
            config,
            running_tasks,
            success_rate,
            avg_duration_min: avg_duration,
            avg_cost_usd: avg_cost,
            recent_failures,
        }))
    }

    /// Get agent info for all registered agents.
    pub fn get_all_agent_info(&self) -> Result<Vec<AgentInfo>> {
        let mut infos = Vec::with_capacity(self.configs.len());
        for agent_id in self.configs.keys() {
            if let Some(info) = self.get_agent_info(agent_id)? {
                infos.push(info);
            }
        }
        Ok(infos)
    }

    /// Get the total running tasks across all agents.
    pub fn get_total_running_tasks(&self) -> Result<u32> {
        self.db.get_total_running_tasks()
    }

    /// Get the config for a specific agent.
    #[allow(dead_code)]
    pub fn get_config(&self, agent_id: &str) -> Option<&AgentConfig> {
        self.configs.get(agent_id)
    }

    /// Get all agent IDs.
    #[allow(dead_code)]
    pub fn agent_ids(&self) -> Vec<&String> {
        self.configs.keys().collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, DecisionRecord, OutcomeRecord};

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
            "aider".to_string(),
            AgentConfig {
                display_name: "Aider".to_string(),
                supports_languages: vec!["python".to_string()],
                supports_types: vec!["bugfix".to_string()],
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

    #[test]
    fn new_registry_upserts_agents() {
        let (db, agents) = setup();
        let _registry = AgentRegistry::new(&db, &agents).unwrap();

        // Verify agents are in the database.
        let ids = db.list_agent_ids().unwrap();
        assert!(ids.contains(&"claude_code".to_string()));
        assert!(ids.contains(&"aider".to_string()));
    }

    #[test]
    fn get_agent_info_returns_config_and_defaults() {
        let (db, agents) = setup();
        let registry = AgentRegistry::new(&db, &agents).unwrap();

        let info = registry
            .get_agent_info("claude_code")
            .unwrap()
            .expect("agent should exist");

        assert_eq!(info.agent_id, "claude_code");
        assert_eq!(info.config.display_name, "Claude Code");
        assert_eq!(info.running_tasks, 0);
        assert!(info.success_rate.is_none()); // No stats yet.
        assert!(info.avg_duration_min.is_none());
        assert!(info.avg_cost_usd.is_none());
        assert_eq!(info.recent_failures, 0);
    }

    #[test]
    fn get_agent_info_unknown_returns_none() {
        let (db, agents) = setup();
        let registry = AgentRegistry::new(&db, &agents).unwrap();

        let info = registry.get_agent_info("nonexistent").unwrap();
        assert!(info.is_none());
    }

    #[test]
    fn get_agent_info_reflects_stats() {
        let (db, agents) = setup();
        let registry = AgentRegistry::new(&db, &agents).unwrap();

        // Record a decision + outcome.
        let decision = DecisionRecord {
            task_id: "t1".to_string(),
            task_json: "{}".to_string(),
            feature_vector: "[]".to_string(),
            constraints_json: None,
            chosen_agent: "claude_code".to_string(),
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
        let decision_id = db.insert_decision(&decision).unwrap();

        let outcome = OutcomeRecord {
            task_id: "t1".to_string(),
            decision_id: Some(decision_id),
            agent_id: "claude_code".to_string(),
            status: "success".to_string(),
            duration_min: Some(15.0),
            tokens_used: Some(20000),
            cost_usd: Some(0.20),
            exit_code: Some(0),
            files_changed: Some(2),
            tests_passed: Some(true),
            validation_passed: Some(true),
            error_summary: None,
            retry_count: 0,
        };
        db.insert_outcome(&outcome).unwrap();
        db.update_agent_stats("claude_code", "bugfix", "python", &outcome)
            .unwrap();

        // Now info should reflect stats.
        let info = registry.get_agent_info("claude_code").unwrap().unwrap();
        assert_eq!(info.success_rate, Some(1.0));
        assert_eq!(info.avg_duration_min, Some(15.0));
        assert_eq!(info.avg_cost_usd, Some(0.20));
    }

    // -- UT-13/UT-14: Running tasks via registry --

    #[test]
    fn running_tasks_reflected_in_agent_info() {
        let (db, agents) = setup();
        let registry = AgentRegistry::new(&db, &agents).unwrap();

        db.increment_running_tasks("claude_code").unwrap();

        let info = registry.get_agent_info("claude_code").unwrap().unwrap();
        assert_eq!(info.running_tasks, 1);

        db.decrement_running_tasks("claude_code").unwrap();
        let info = registry.get_agent_info("claude_code").unwrap().unwrap();
        assert_eq!(info.running_tasks, 0);
    }

    #[test]
    fn get_all_agent_info_returns_all() {
        let (db, agents) = setup();
        let registry = AgentRegistry::new(&db, &agents).unwrap();

        let all = registry.get_all_agent_info().unwrap();
        assert_eq!(all.len(), 2);

        let ids: Vec<&str> = all.iter().map(|a| a.agent_id.as_str()).collect();
        assert!(ids.contains(&"claude_code"));
        assert!(ids.contains(&"aider"));
    }

    #[test]
    fn total_running_tasks_via_registry() {
        let (db, agents) = setup();
        let registry = AgentRegistry::new(&db, &agents).unwrap();

        db.increment_running_tasks("claude_code").unwrap();
        db.increment_running_tasks("aider").unwrap();
        db.increment_running_tasks("aider").unwrap();

        assert_eq!(registry.get_total_running_tasks().unwrap(), 3);
    }

    #[test]
    fn registry_re_creation_preserves_running_tasks() {
        let (db, agents) = setup();

        // First registry.
        let _reg1 = AgentRegistry::new(&db, &agents).unwrap();
        db.increment_running_tasks("claude_code").unwrap();
        db.increment_running_tasks("claude_code").unwrap();

        // Second registry (simulating restart with re-upsert).
        let reg2 = AgentRegistry::new(&db, &agents).unwrap();
        let info = reg2.get_agent_info("claude_code").unwrap().unwrap();

        // running_tasks should be preserved (upsert doesn't reset them).
        assert_eq!(info.running_tasks, 2);
    }
}
