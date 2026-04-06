//! get_budget_status tool implementation.
//!
//! Returns a budget overview including total spent, budget limit,
//! remaining amount, and per-agent cost breakdown.

use anyhow::Result;
use serde_json::Value;

use crate::config::ArbiterConfig;
use crate::db::Database;

/// Execute the get_budget_status tool.
///
/// Queries the database for total and per-agent costs, compares
/// against the configured budget threshold, and returns a JSON
/// summary.
pub fn execute(db: &Database, config: &ArbiterConfig) -> Result<Value> {
    let budget_limit = config.invariants.budget.threshold_usd;
    let total_spent = db.get_total_cost()?;
    let remaining = budget_limit - total_spent;
    let over_budget = total_spent > budget_limit;

    let by_agent: Vec<Value> = db
        .get_cost_by_agent()?
        .into_iter()
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
        "over_budget": over_budget,
        "by_agent": by_agent,
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AgentConfig, AgentHealthConfig, BudgetConfig, ConcurrencyConfig, InvariantConfig,
        RateLimitConfig, RetriesConfig, SlaConfig,
    };
    use crate::db::{DecisionRecord, OutcomeRecord};
    use std::collections::HashMap;

    fn test_config() -> ArbiterConfig {
        let mut agents = HashMap::new();
        agents.insert(
            "a1".to_string(),
            AgentConfig {
                display_name: "Agent One".to_string(),
                supports_languages: vec!["python".to_string()],
                supports_types: vec!["bugfix".to_string()],
                max_concurrent: 2,
                cost_per_hour: 0.30,
                avg_duration_min: 10.0,
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

    fn setup_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db
    }

    fn insert_agent(db: &Database, agent_id: &str) {
        db.upsert_agent(agent_id, "Test", 2, r#"{"display_name":"Test"}"#)
            .unwrap();
    }

    fn sample_decision(agent_id: &str) -> DecisionRecord {
        DecisionRecord {
            task_id: "task-budget-001".to_string(),
            task_json: r#"{"type":"bugfix","language":"python","complexity":"simple","priority":"normal"}"#.to_string(),
            feature_vector: "[1.0,0.0,1.0,1.0,1.0,50.0,0.0,0.0,120.0,0.5,2.0,0.0,15.0,0.1,0.0,1.0,1.0,0.0,0.0,10.0,14.0,0.0]".to_string(),
            constraints_json: None,
            chosen_agent: agent_id.to_string(),
            action: "assign".to_string(),
            confidence: 0.9,
            decision_path: "[]".to_string(),
            fallback_agent: None,
            fallback_reason: None,
            invariants_json: "[]".to_string(),
            invariants_passed: 10,
            invariants_failed: 0,
            inference_us: 42,
        }
    }

    #[test]
    fn budget_status_empty() {
        let db = setup_db();
        let config = test_config();
        let result = execute(&db, &config).unwrap();

        assert_eq!(result["budget_limit_usd"], "10.00");
        assert_eq!(result["total_spent_usd"], "0.00");
        assert_eq!(result["remaining_usd"], "10.00");
        assert_eq!(result["over_budget"], false);
        assert!(result["by_agent"].as_array().unwrap().is_empty());
    }

    #[test]
    fn budget_status_with_spend() {
        let db = setup_db();
        let config = test_config();

        insert_agent(&db, "a1");
        let decision = sample_decision("a1");
        let decision_id = db.insert_decision(&decision).unwrap();

        let outcome = OutcomeRecord {
            task_id: "task-budget-001".to_string(),
            decision_id: Some(decision_id),
            agent_id: "a1".to_string(),
            status: "success".to_string(),
            duration_min: Some(5.0),
            tokens_used: Some(1000),
            cost_usd: Some(3.50),
            exit_code: Some(0),
            files_changed: Some(2),
            tests_passed: Some(true),
            validation_passed: Some(true),
            error_summary: None,
            retry_count: 0,
        };
        db.insert_outcome(&outcome).unwrap();
        db.update_agent_stats("a1", "bugfix", "python", &outcome)
            .unwrap();

        let result = execute(&db, &config).unwrap();

        assert_eq!(result["budget_limit_usd"], "10.00");
        assert_eq!(result["total_spent_usd"], "3.50");
        assert_eq!(result["remaining_usd"], "6.50");
        assert_eq!(result["over_budget"], false);

        let by_agent = result["by_agent"].as_array().unwrap();
        assert_eq!(by_agent.len(), 1);
        assert_eq!(by_agent[0]["agent_id"], "a1");
        assert_eq!(by_agent[0]["total_cost_usd"], "3.50");
        assert_eq!(by_agent[0]["total_tasks"], 1);
    }

    #[test]
    fn budget_status_over_budget() {
        let db = setup_db();
        let config = test_config();

        insert_agent(&db, "a1");
        let decision = sample_decision("a1");
        let decision_id = db.insert_decision(&decision).unwrap();

        let outcome = OutcomeRecord {
            task_id: "task-budget-002".to_string(),
            decision_id: Some(decision_id),
            agent_id: "a1".to_string(),
            status: "success".to_string(),
            duration_min: Some(60.0),
            tokens_used: Some(50000),
            cost_usd: Some(15.00),
            exit_code: Some(0),
            files_changed: Some(10),
            tests_passed: Some(true),
            validation_passed: Some(true),
            error_summary: None,
            retry_count: 0,
        };
        db.insert_outcome(&outcome).unwrap();
        db.update_agent_stats("a1", "bugfix", "python", &outcome)
            .unwrap();

        let result = execute(&db, &config).unwrap();

        assert_eq!(result["budget_limit_usd"], "10.00");
        assert_eq!(result["total_spent_usd"], "15.00");
        assert_eq!(result["remaining_usd"], "-5.00");
        assert_eq!(result["over_budget"], true);
    }
}
