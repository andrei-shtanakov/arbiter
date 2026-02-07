//! report_outcome tool implementation.
//!
//! Records task execution results, updates agent statistics,
//! and detects agent health issues for retraining suggestions.

use anyhow::{bail, Result};
use serde_json::Value;
use tracing::{info, warn};

use crate::config::ArbiterConfig;
use crate::db::{Database, OutcomeRecord};

/// Valid outcome status values.
const VALID_STATUSES: &[&str] = &["success", "failure", "timeout", "cancelled"];

/// Result of the report_outcome operation.
#[derive(Debug)]
pub struct ReportResult {
    pub task_id: String,
    pub recorded: bool,
    pub updated_stats: UpdatedStats,
    pub retrain_suggested: bool,
    pub warnings: Vec<String>,
}

/// Updated agent stats returned in the response.
#[derive(Debug)]
pub struct UpdatedStats {
    pub agent_id: String,
    pub total_tasks: i64,
    pub success_rate: f64,
    pub avg_duration_min: f64,
    pub avg_cost_usd: f64,
}

/// Execute the report_outcome logic.
///
/// Algorithm:
/// 1. Validate input (status must be valid enum value)
/// 2. Find decision by task_id (may be None for unknown tasks)
/// 3. Build and insert outcome record
/// 4. Update agent_stats aggregates
/// 5. Decrement running_tasks (only if decision was found)
/// 6. Check failures_24h against threshold for retrain_suggested
/// 7. Return updated stats and warnings
pub fn execute(args: &Value, db: &Database, config: &ArbiterConfig) -> Result<ReportResult> {
    let mut warnings = Vec::new();

    // --- Step 1: Extract and validate required fields ---
    let task_id = args.get("task_id").and_then(|v| v.as_str()).unwrap_or("");
    let agent_id = args.get("agent_id").and_then(|v| v.as_str()).unwrap_or("");
    let status = args.get("status").and_then(|v| v.as_str()).unwrap_or("");

    if task_id.is_empty() {
        bail!("Missing required field: task_id");
    }
    if agent_id.is_empty() {
        bail!("Missing required field: agent_id");
    }
    if status.is_empty() {
        bail!("Missing required field: status");
    }
    if !VALID_STATUSES.contains(&status) {
        bail!(
            "Invalid status '{}'. Must be one of: {}",
            status,
            VALID_STATUSES.join(", ")
        );
    }

    // --- Extract optional fields ---
    let duration_min = args.get("duration_min").and_then(|v| v.as_f64());
    let tokens_used = args.get("tokens_used").and_then(|v| v.as_i64());
    let cost_usd = args.get("cost_usd").and_then(|v| v.as_f64());
    let exit_code = args
        .get("exit_code")
        .and_then(|v| v.as_i64())
        .map(|v| v as i32);
    let files_changed = args
        .get("files_changed")
        .and_then(|v| v.as_i64())
        .map(|v| v as i32);
    let tests_passed = args.get("tests_passed").and_then(|v| v.as_bool());
    let validation_passed = args.get("validation_passed").and_then(|v| v.as_bool());
    let error_summary = args
        .get("error_summary")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let retry_count = args
        .get("retry_count")
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32;

    // --- Step 2: Find decision by task_id ---
    let decision_id = db.find_decision_id_by_task(task_id)?;

    // Determine task_type and language from the decision's task_json,
    // or fall back to "unknown".
    let (task_type, language) = if decision_id.is_some() {
        let decision = db.find_decision_by_task(task_id)?;
        match decision {
            Some(d) => {
                let task_json: Value = serde_json::from_str(&d.task_json).unwrap_or_default();
                let tt = task_json
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let lang = task_json
                    .get("language")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                (tt, lang)
            }
            None => ("unknown".to_string(), "unknown".to_string()),
        }
    } else {
        warn!(task_id = task_id, "No matching decision found for task_id");
        warnings.push("No matching decision found".to_string());
        ("unknown".to_string(), "unknown".to_string())
    };

    // --- Step 3: Build and insert outcome record ---
    let outcome = OutcomeRecord {
        task_id: task_id.to_string(),
        decision_id,
        agent_id: agent_id.to_string(),
        status: status.to_string(),
        duration_min,
        tokens_used,
        cost_usd,
        exit_code,
        files_changed,
        tests_passed,
        validation_passed,
        error_summary,
        retry_count,
    };

    db.insert_outcome(&outcome)?;

    // --- Step 4: Update agent_stats aggregates ---
    db.update_agent_stats(agent_id, &task_type, &language, &outcome)?;

    // --- Step 5: Decrement running_tasks (only if decision was found) ---
    if decision_id.is_some() {
        if let Err(e) = db.decrement_running_tasks(agent_id) {
            warn!(
                agent_id = agent_id,
                error = %e,
                "Failed to decrement running_tasks"
            );
        }
    }

    // --- Step 6: Check failures_24h for retrain_suggested ---
    let recent_failures = db.get_recent_failures(agent_id, 24)?;
    let max_failures = config.invariants.agent_health.max_failures_24h;
    let retrain_suggested = recent_failures > max_failures;

    if retrain_suggested {
        warn!(
            agent_id = agent_id,
            recent_failures = recent_failures,
            threshold = max_failures,
            "Agent health warning: failures exceed threshold"
        );
    }

    // --- Step 7: Get updated stats ---
    let stats = db.get_agent_stats(agent_id)?;

    info!(
        task_id = task_id,
        agent_id = agent_id,
        status = status,
        retrain_suggested = retrain_suggested,
        "report_outcome recorded"
    );

    Ok(ReportResult {
        task_id: task_id.to_string(),
        recorded: true,
        updated_stats: UpdatedStats {
            agent_id: agent_id.to_string(),
            total_tasks: stats.total_tasks,
            success_rate: stats.success_rate,
            avg_duration_min: stats.avg_duration_min,
            avg_cost_usd: stats.avg_cost_usd,
        },
        retrain_suggested,
        warnings,
    })
}

/// Serialize a ReportResult into the MCP response JSON Value.
pub fn result_to_json(result: &ReportResult) -> Value {
    serde_json::json!({
        "task_id": result.task_id,
        "recorded": result.recorded,
        "updated_stats": {
            "agent_id": result.updated_stats.agent_id,
            "total_tasks": result.updated_stats.total_tasks,
            "success_rate": result.updated_stats.success_rate,
            "avg_duration_min": result.updated_stats.avg_duration_min,
            "avg_cost_usd": result.updated_stats.avg_cost_usd,
        },
        "retrain_suggested": result.retrain_suggested,
        "warnings": result.warnings,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;
    use crate::db::DecisionRecord;
    use std::collections::HashMap;

    fn test_config() -> ArbiterConfig {
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
                supports_types: vec!["feature".to_string(), "bugfix".to_string()],
                max_concurrent: 3,
                cost_per_hour: 0.20,
                avg_duration_min: 12.0,
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

    fn insert_test_agent(db: &Database, agent_id: &str) {
        db.upsert_agent(
            agent_id,
            "Test Agent",
            2,
            r#"{"display_name":"Test Agent"}"#,
        )
        .unwrap();
    }

    fn sample_decision(task_id: &str) -> DecisionRecord {
        DecisionRecord {
            task_id: task_id.to_string(),
            task_json:
                r#"{"type":"bugfix","language":"python","complexity":"simple","priority":"normal"}"#
                    .to_string(),
            feature_vector: "[]".to_string(),
            constraints_json: None,
            chosen_agent: "claude_code".to_string(),
            action: "assign".to_string(),
            confidence: 0.92,
            decision_path: "[]".to_string(),
            fallback_agent: None,
            fallback_reason: None,
            invariants_json: "[]".to_string(),
            invariants_passed: 10,
            invariants_failed: 0,
            inference_us: 42,
        }
    }

    // -- Input validation --

    #[test]
    fn rejects_invalid_status() {
        let db = setup_db();
        insert_test_agent(&db, "claude_code");
        let config = test_config();

        let args = serde_json::json!({
            "task_id": "t1",
            "agent_id": "claude_code",
            "status": "invalid_status"
        });

        let result = execute(&args, &db, &config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid status"), "got: {err}");
    }

    #[test]
    fn rejects_empty_task_id() {
        let db = setup_db();
        let config = test_config();

        let args = serde_json::json!({
            "task_id": "",
            "agent_id": "claude_code",
            "status": "success"
        });

        let result = execute(&args, &db, &config);
        assert!(result.is_err());
    }

    // -- Happy path --

    #[test]
    fn records_outcome_with_known_decision() {
        let db = setup_db();
        insert_test_agent(&db, "claude_code");
        let config = test_config();

        // Insert a decision first
        let decision = sample_decision("t1");
        db.insert_decision(&decision).unwrap();
        db.increment_running_tasks("claude_code").unwrap();

        let args = serde_json::json!({
            "task_id": "t1",
            "agent_id": "claude_code",
            "status": "success",
            "duration_min": 12.5,
            "tokens_used": 35000,
            "cost_usd": 0.25
        });

        let result = execute(&args, &db, &config).unwrap();

        assert!(result.recorded);
        assert_eq!(result.task_id, "t1");
        assert_eq!(result.updated_stats.agent_id, "claude_code");
        assert_eq!(result.updated_stats.total_tasks, 1);
        assert!((result.updated_stats.success_rate - 1.0).abs() < f64::EPSILON);
        assert!(!result.retrain_suggested);
        assert!(result.warnings.is_empty());

        // running_tasks should be decremented back to 0
        let running = db.get_running_tasks("claude_code").unwrap();
        assert_eq!(running, 0);
    }

    // -- Unknown task_id --

    #[test]
    fn records_outcome_with_unknown_task_id() {
        let db = setup_db();
        insert_test_agent(&db, "claude_code");
        let config = test_config();

        let args = serde_json::json!({
            "task_id": "unknown-task",
            "agent_id": "claude_code",
            "status": "success",
            "duration_min": 5.0
        });

        let result = execute(&args, &db, &config).unwrap();

        assert!(result.recorded);
        assert_eq!(result.task_id, "unknown-task");
        assert!(result
            .warnings
            .contains(&"No matching decision found".to_string()));

        // running_tasks should NOT be decremented (no decision found)
        let running = db.get_running_tasks("claude_code").unwrap();
        assert_eq!(running, 0);
    }

    // -- UT-15: Running tasks clamp to 0 --

    #[test]
    fn ut_15_running_tasks_clamp_to_zero() {
        let db = setup_db();
        insert_test_agent(&db, "claude_code");
        let config = test_config();

        // running_tasks starts at 0, decision exists so decrement will run
        let decision = sample_decision("t-clamp");
        db.insert_decision(&decision).unwrap();

        // Do NOT increment running_tasks - it's already 0
        assert_eq!(db.get_running_tasks("claude_code").unwrap(), 0);

        let args = serde_json::json!({
            "task_id": "t-clamp",
            "agent_id": "claude_code",
            "status": "success"
        });

        let result = execute(&args, &db, &config).unwrap();
        assert!(result.recorded);

        // running_tasks should be clamped at 0, not go negative
        let running = db.get_running_tasks("claude_code").unwrap();
        assert_eq!(running, 0, "running_tasks should be clamped at 0");
    }

    // -- Retrain suggested --

    #[test]
    fn retrain_suggested_on_high_failures() {
        let db = setup_db();
        insert_test_agent(&db, "claude_code");
        let config = test_config(); // max_failures_24h = 5

        // Insert 5 prior failure outcomes to bring count to threshold
        for i in 0..5 {
            let decision = sample_decision(&format!("t-fail-{i}"));
            let did = db.insert_decision(&decision).unwrap();
            let outcome = OutcomeRecord {
                task_id: format!("t-fail-{i}"),
                decision_id: Some(did),
                agent_id: "claude_code".to_string(),
                status: "failure".to_string(),
                duration_min: Some(5.0),
                tokens_used: None,
                cost_usd: None,
                exit_code: Some(1),
                files_changed: None,
                tests_passed: None,
                validation_passed: None,
                error_summary: Some("test failure".to_string()),
                retry_count: 0,
            };
            db.insert_outcome(&outcome).unwrap();
        }

        // Now report one more failure (the 6th)
        let decision = sample_decision("t-fail-trigger");
        db.insert_decision(&decision).unwrap();
        db.increment_running_tasks("claude_code").unwrap();

        let args = serde_json::json!({
            "task_id": "t-fail-trigger",
            "agent_id": "claude_code",
            "status": "failure",
            "error_summary": "another failure"
        });

        let result = execute(&args, &db, &config).unwrap();
        assert!(result.recorded);
        assert!(
            result.retrain_suggested,
            "retrain_suggested should be true after 6 failures (threshold=5)"
        );
    }

    #[test]
    fn retrain_not_suggested_below_threshold() {
        let db = setup_db();
        insert_test_agent(&db, "claude_code");
        let config = test_config(); // max_failures_24h = 5

        // Insert only 2 failures
        for i in 0..2 {
            let decision = sample_decision(&format!("t-few-{i}"));
            let did = db.insert_decision(&decision).unwrap();
            let outcome = OutcomeRecord {
                task_id: format!("t-few-{i}"),
                decision_id: Some(did),
                agent_id: "claude_code".to_string(),
                status: "failure".to_string(),
                duration_min: None,
                tokens_used: None,
                cost_usd: None,
                exit_code: Some(1),
                files_changed: None,
                tests_passed: None,
                validation_passed: None,
                error_summary: None,
                retry_count: 0,
            };
            db.insert_outcome(&outcome).unwrap();
        }

        // Report a success
        let decision = sample_decision("t-ok");
        db.insert_decision(&decision).unwrap();

        let args = serde_json::json!({
            "task_id": "t-ok",
            "agent_id": "claude_code",
            "status": "success"
        });

        let result = execute(&args, &db, &config).unwrap();
        assert!(!result.retrain_suggested);
    }

    // -- IT-05: Stats accumulation over 10 outcomes --

    #[test]
    fn it_05_stats_accumulation_10x() {
        let db = setup_db();
        insert_test_agent(&db, "claude_code");
        let config = test_config();

        let mut total_duration = 0.0;
        let mut total_cost = 0.0;
        let mut successes = 0;

        for i in 0..10 {
            let decision = sample_decision(&format!("it05-{i}"));
            db.insert_decision(&decision).unwrap();
            db.increment_running_tasks("claude_code").unwrap();

            let is_success = i % 3 != 0; // 7 successes, 3 failures
            let dur = 5.0 + i as f64;
            let cost = 0.10 + (i as f64 * 0.02);

            if is_success {
                successes += 1;
            }
            total_duration += dur;
            total_cost += cost;

            let args = serde_json::json!({
                "task_id": format!("it05-{i}"),
                "agent_id": "claude_code",
                "status": if is_success { "success" } else { "failure" },
                "duration_min": dur,
                "cost_usd": cost
            });

            let result = execute(&args, &db, &config).unwrap();
            assert!(result.recorded);
            assert_eq!(result.updated_stats.total_tasks, (i + 1) as i64);
        }

        // Verify final stats
        let stats = db.get_agent_stats("claude_code").unwrap();
        assert_eq!(stats.total_tasks, 10);
        assert_eq!(stats.successful_tasks, successes);
        assert_eq!(stats.failed_tasks, 10 - successes);
        assert!(
            (stats.success_rate - (successes as f64 / 10.0)).abs() < 0.01,
            "success_rate mismatch: {} vs {}",
            stats.success_rate,
            successes as f64 / 10.0,
        );
        assert!(
            (stats.avg_duration_min - total_duration / 10.0).abs() < 0.01,
            "avg_duration mismatch"
        );
        assert!(
            (stats.avg_cost_usd - total_cost / 10.0).abs() < 0.01,
            "avg_cost mismatch"
        );

        // running_tasks should be back to 0
        let running = db.get_running_tasks("claude_code").unwrap();
        assert_eq!(running, 0);
    }

    // -- IT-06: Agent failure detection --

    #[test]
    fn it_06_agent_failure_detection() {
        let db = setup_db();
        insert_test_agent(&db, "claude_code");
        let config = test_config(); // max_failures_24h = 5

        // Report 5 failures (at threshold, not over)
        for i in 0..5 {
            let decision = sample_decision(&format!("it06-{i}"));
            db.insert_decision(&decision).unwrap();
            db.increment_running_tasks("claude_code").unwrap();

            let args = serde_json::json!({
                "task_id": format!("it06-{i}"),
                "agent_id": "claude_code",
                "status": "failure",
                "error_summary": format!("crash #{i}")
            });

            let result = execute(&args, &db, &config).unwrap();
            assert!(result.recorded);

            // At exactly 5 failures, we're AT the threshold,
            // retrain_suggested requires > threshold
            if i < 4 {
                assert!(
                    !result.retrain_suggested,
                    "should not suggest retrain at {} failures",
                    i + 1
                );
            }
        }

        // The 5th failure puts us at exactly 5 which equals the threshold.
        // retrain_suggested is only when > threshold, so still false at 5.
        let last_at_5 = {
            let decision = sample_decision("it06-check-5");
            db.insert_decision(&decision).unwrap();
            db.increment_running_tasks("claude_code").unwrap();

            let args = serde_json::json!({
                "task_id": "it06-check-5",
                "agent_id": "claude_code",
                "status": "success"
            });
            execute(&args, &db, &config).unwrap()
        };
        // 5 failures is at threshold, not over
        assert!(
            !last_at_5.retrain_suggested,
            "retrain not suggested at exactly threshold"
        );

        // Report the 6th failure (one over threshold)
        let decision = sample_decision("it06-over");
        db.insert_decision(&decision).unwrap();
        db.increment_running_tasks("claude_code").unwrap();

        let args = serde_json::json!({
            "task_id": "it06-over",
            "agent_id": "claude_code",
            "status": "timeout",
            "error_summary": "timed out"
        });

        let result = execute(&args, &db, &config).unwrap();
        assert!(result.recorded);
        assert!(
            result.retrain_suggested,
            "retrain_suggested should be true after 6 failures (threshold=5)"
        );

        // Verify warnings are empty for known task
        assert!(result.warnings.is_empty());
    }

    // -- result_to_json --

    #[test]
    fn result_to_json_structure() {
        let result = ReportResult {
            task_id: "t1".to_string(),
            recorded: true,
            updated_stats: UpdatedStats {
                agent_id: "claude_code".to_string(),
                total_tasks: 5,
                success_rate: 0.8,
                avg_duration_min: 10.0,
                avg_cost_usd: 0.15,
            },
            retrain_suggested: false,
            warnings: vec!["test warning".to_string()],
        };

        let json = result_to_json(&result);
        assert_eq!(json["task_id"], "t1");
        assert_eq!(json["recorded"], true);
        assert_eq!(json["updated_stats"]["agent_id"], "claude_code");
        assert_eq!(json["updated_stats"]["total_tasks"], 5);
        assert_eq!(json["updated_stats"]["success_rate"], 0.8);
        assert_eq!(json["retrain_suggested"], false);
        assert_eq!(json["warnings"][0], "test warning");
    }
}
