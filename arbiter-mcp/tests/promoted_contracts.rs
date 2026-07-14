//! Contract tests for the promoted Budget v1 / PolicyDecisionRef v1
//! schemas (contracts roadmap phase 2, RD-002).
//!
//! Unlike fixture-only golden tests these validate the LIVE tool output
//! against `contracts/*/schema.json`, so wire drift fails CI in this repo
//! before any consumer sees it.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

use arbiter_core::types::AgentAction;
use arbiter_mcp::config::{
    AgentConfig, AgentHealthConfig, ArbiterConfig, BudgetConfig, ConcurrencyConfig,
    InvariantConfig, RateLimitConfig, RetriesConfig, SlaConfig,
};
use arbiter_mcp::db::{Database, DecisionRecord, OutcomeRecord};
use arbiter_mcp::tools::get_budget;
use arbiter_mcp::tools::route_task::{result_to_json, RouteResult};

fn contract_schema(name: &str) -> jsonschema::Validator {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("contracts")
        .join(name)
        .join("schema.json");
    let raw = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let schema: Value = serde_json::from_str(&raw).expect("parse schema JSON");
    jsonschema::validator_for(&schema).expect("compile schema")
}

fn assert_valid(v: &jsonschema::Validator, payload: &Value) {
    let errors: Vec<String> = v
        .evaluate(payload)
        .iter_errors()
        .map(|e| e.error.to_string())
        .collect();
    assert!(
        errors.is_empty(),
        "validation errors: {errors:?}\npayload: {payload}"
    );
}

fn fixtures(name: &str) -> Vec<(PathBuf, Value)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("contracts")
        .join(name)
        .join("fixtures");
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {dir:?}: {e}")) {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "json") {
            let value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            out.push((path, value));
        }
    }
    assert!(!out.is_empty(), "no fixtures under {dir:?}");
    out
}

fn test_config(threshold_usd: f64) -> ArbiterConfig {
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
        authority: None,
        invariants: InvariantConfig {
            budget: BudgetConfig { threshold_usd },
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

fn seeded_db(costs: &[f64]) -> Database {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    db.upsert_agent("a1", "Agent One", 2, r#"{"display_name":"Agent One"}"#)
        .unwrap();
    for (i, cost) in costs.iter().enumerate() {
        let decision = DecisionRecord {
            task_id: format!("task-{i}"),
            task_json:
                r#"{"type":"bugfix","language":"python","complexity":"simple","priority":"normal"}"#
                    .to_string(),
            feature_vector: "[]".to_string(),
            constraints_json: None,
            chosen_agent: "a1".to_string(),
            action: "assign".to_string(),
            confidence: 0.9,
            decision_path: "[]".to_string(),
            fallback_agent: None,
            fallback_reason: None,
            invariants_json: "[]".to_string(),
            invariants_passed: 10,
            invariants_failed: 0,
            inference_us: 42,
            shadow_json: None,
        };
        let decision_id = db.insert_decision(&decision).unwrap();
        let outcome = OutcomeRecord {
            task_id: format!("task-{i}"),
            decision_id: Some(decision_id),
            agent_id: "a1".to_string(),
            status: "success".to_string(),
            duration_min: Some(1.0),
            tokens_used: Some(100),
            cost_usd: Some(*cost),
            exit_code: Some(0),
            files_changed: Some(1),
            tests_passed: Some(true),
            validation_passed: Some(true),
            error_summary: None,
            retry_count: 0,
        };
        db.insert_outcome(&outcome).unwrap();
        // Budget totals read from agent_stats, which the report_outcome
        // tool layer updates alongside the insert — mirror that here.
        db.update_agent_stats("a1", "bugfix", "python", &outcome)
            .unwrap();
    }
    db
}

// ------------------------------------------------------------- Budget v1

#[test]
fn budget_live_output_empty_db_matches_schema() {
    let schema = contract_schema("budget");
    let db = seeded_db(&[]);
    let out = get_budget::execute(&db, &test_config(10.0)).unwrap();
    assert_valid(&schema, &out);
}

#[test]
fn budget_live_output_with_spend_matches_schema() {
    let schema = contract_schema("budget");
    let db = seeded_db(&[2.5, 1.25]);
    let out = get_budget::execute(&db, &test_config(10.0)).unwrap();
    assert_valid(&schema, &out);
    assert_eq!(out["over_budget"], json!(false));
}

#[test]
fn budget_live_output_over_budget_matches_schema() {
    let schema = contract_schema("budget");
    let db = seeded_db(&[8.0, 5.5]);
    let out = get_budget::execute(&db, &test_config(10.0)).unwrap();
    assert_valid(&schema, &out);
    assert_eq!(out["over_budget"], json!(true));
    // negative remaining is part of the promoted format
    let remaining = out["remaining_usd"].as_str().unwrap();
    assert!(remaining.starts_with('-'), "remaining: {remaining}");
}

#[test]
fn budget_fixtures_match_schema() {
    let schema = contract_schema("budget");
    for (path, fixture) in fixtures("budget") {
        let errors: Vec<String> = schema
            .evaluate(&fixture)
            .iter_errors()
            .map(|e| e.error.to_string())
            .collect();
        assert!(errors.is_empty(), "{path:?}: {errors:?}");
    }
}

// --------------------------------------------------- PolicyDecisionRef v1

/// Build the ref exactly as a consumer would from a route_task response.
fn ref_from_response(response: &Value, ts: Option<&str>) -> Value {
    json!({
        "schema_version": "1",
        "decision_id": response["metadata"]["decision_id"],
        "task_id": response["task_id"],
        "action": response["action"],
        "chosen_agent": response["chosen_agent"],
        "confidence": response["confidence"],
        "ts": ts,
    })
}

#[test]
fn policy_decision_ref_from_live_route_response_matches_schema() {
    let schema = contract_schema("policy-decision-ref");
    let result = RouteResult {
        task_id: "corr-demo-note-2".to_string(),
        action: AgentAction::Assign,
        chosen_agent: "claude_code@claude-sonnet-4-6".to_string(),
        confidence: 1.0,
        reasoning: "capability match".to_string(),
        decision_path: vec![],
        fallback_agent: None,
        fallback_reason: None,
        invariant_checks: vec![],
        inference_us: 42,
        feature_vector: vec![],
        candidates_evaluated: 4,
        warnings: vec![],
        decision_id: Some(22),
        authority: None,
    };
    let response = result_to_json(&result);
    assert_valid(
        &schema,
        &ref_from_response(&response, Some("2026-07-11T15:02:46Z")),
    );
}

#[test]
fn policy_decision_ref_requires_decision_id() {
    let schema = contract_schema("policy-decision-ref");
    let payload = json!({
        "schema_version": "1",
        "task_id": "t-1",
        "action": "assign"
    });
    let has_errors = schema.evaluate(&payload).iter_errors().next().is_some();
    assert!(has_errors, "decision_id must be required");
}

#[test]
fn policy_decision_ref_rejects_unknown_action() {
    let schema = contract_schema("policy-decision-ref");
    let payload = json!({
        "schema_version": "1",
        "decision_id": 1,
        "task_id": "t-1",
        "action": "interrupted"
    });
    let has_errors = schema.evaluate(&payload).iter_errors().next().is_some();
    assert!(has_errors, "unknown action must fail");
}

#[test]
fn policy_decision_ref_fixtures_match_schema() {
    let schema = contract_schema("policy-decision-ref");
    for (path, fixture) in fixtures("policy-decision-ref") {
        let errors: Vec<String> = schema
            .evaluate(&fixture)
            .iter_errors()
            .map(|e| e.error.to_string())
            .collect();
        assert!(errors.is_empty(), "{path:?}: {errors:?}");
    }
}

// --------------------------------------------------------- Authority v1

#[test]
fn authority_fixtures_match_schema() {
    let schema = contract_schema("authority");
    for (path, fixture) in fixtures("authority") {
        let errors: Vec<String> = schema
            .evaluate(&fixture)
            .iter_errors()
            .map(|e| e.error.to_string())
            .collect();
        assert!(errors.is_empty(), "{path:?}: {errors:?}");
    }
}

#[test]
fn authority_live_audit_matches_schema() {
    use arbiter_core::authority::{
        AuthorityContext, AuthorityPolicy, AuthorityRule, UnknownContext,
    };
    use arbiter_core::types::{Complexity, Language, Priority, TaskInput, TaskType};
    use arbiter_mcp::agents::AgentRegistry;
    use arbiter_mcp::tools::route_task;
    use std::sync::Arc;

    let schema = contract_schema("authority");
    let config = test_config(100.0);
    let db = Arc::new(Database::open_in_memory().unwrap());
    db.migrate().unwrap();
    let registry = AgentRegistry::new(Arc::clone(&db), &config.agents).unwrap();

    let policy = AuthorityPolicy {
        version: 1,
        unknown_context: UnknownContext::Deny,
        rules: vec![AuthorityRule {
            role: "review".to_string(),
            phase: "execution".to_string(),
            // Allow an agent that is not in the registry -> live REJECT + denied list.
            agents: vec!["gemini_cli@gemini-3".to_string()],
        }],
        policy_sha: format!("sha256:{}", "c".repeat(64)),
    };
    let task = TaskInput {
        task_type: TaskType::Bugfix,
        language: Language::Python,
        complexity: Complexity::Simple,
        priority: Priority::Normal,
        scope: vec![],
        branch: None,
        estimated_tokens: None,
        has_dependencies: false,
        requires_internet: false,
        sla_minutes: None,
        description: None,
    };
    let mut constraints = arbiter_core::types::Constraints {
        preferred_agent: None,
        excluded_agents: vec![],
        budget_remaining_usd: None,
        total_pending_tasks: None,
        running_tasks: vec![],
        retry_count: None,
        calls_per_minute: None,
        authority_context: None,
    };
    constraints.authority_context = Some(AuthorityContext {
        role: "review".to_string(),
        phase: "execution".to_string(),
    });

    let result = route_task::execute(
        "authority-live",
        &task,
        &constraints,
        Some(&policy),
        None,
        &registry,
        &db,
        &config.invariants,
        &arbiter_mcp::metrics::Metrics::new(),
    )
    .unwrap();
    let response = result_to_json(&result);

    assert_eq!(response["reasoning"], "authority_no_authorized_candidates");
    let audit = &response["metadata"]["authority"];
    assert!(
        !audit.is_null(),
        "audit must be surfaced at metadata.authority"
    );
    assert_valid(&schema, audit);
}

// -------------------------------------------------------- Capability v1

/// Build the capability projection exactly as a consumer would.
fn capability_record(agent_id: &str, config: &AgentConfig) -> Value {
    json!({
        "agent_id": agent_id,
        "supports_types": config.supports_types,
        "supports_languages": config.supports_languages,
        "max_concurrent": config.max_concurrent,
    })
}

#[test]
fn capability_fixtures_match_schema() {
    let schema = contract_schema("capability");
    for (path, fixture) in fixtures("capability") {
        let errors: Vec<String> = schema
            .evaluate(&fixture)
            .iter_errors()
            .map(|e| e.error.to_string())
            .collect();
        assert!(errors.is_empty(), "{path:?}: {errors:?}");
    }
}

#[test]
fn capability_live_projection_matches_schema() {
    let schema = contract_schema("capability");
    let config = test_config(100.0);
    for (agent_id, agent) in &config.agents {
        assert_valid(&schema, &capability_record(agent_id, agent));
    }
}
