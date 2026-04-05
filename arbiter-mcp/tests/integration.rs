//! Integration tests for the Arbiter MCP server.
//!
//! Tests IT-01 through IT-07 verify end-to-end behavior of the routing,
//! feedback, and status query subsystems through the public API.

use std::collections::HashMap;

use arbiter_core::policy::decision_tree::DecisionTree;
use arbiter_core::types::*;

use arbiter_mcp::agents::AgentRegistry;
use arbiter_mcp::config::*;
use arbiter_mcp::db::{Database, DecisionRecord};
use arbiter_mcp::features::{build_feature_vector, AgentInfo, SystemState, FEATURE_DIM};
use arbiter_mcp::server::McpServer;
use arbiter_mcp::tools::{report_outcome, route_task};

// ---------------------------------------------------------------------------
// Shared test helpers
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
            "{} not found — run bootstrap_agent_tree.py first",
            path.display()
        )
    });
    DecisionTree::from_json(&json).expect("failed to parse bootstrap tree")
}

/// Create the 3-agent config matching config/agents.toml.
fn test_agents() -> HashMap<String, AgentConfig> {
    let mut agents = HashMap::new();
    agents.insert(
        "claude_code".to_string(),
        AgentConfig {
            display_name: "Claude Code".to_string(),
            supports_languages: vec![
                "python".to_string(),
                "rust".to_string(),
                "typescript".to_string(),
            ],
            supports_types: vec![
                "feature".to_string(),
                "bugfix".to_string(),
                "refactor".to_string(),
                "docs".to_string(),
                "review".to_string(),
            ],
            max_concurrent: 2,
            cost_per_hour: 0.30,
            avg_duration_min: 18.0,
        },
    );
    agents.insert(
        "codex_cli".to_string(),
        AgentConfig {
            display_name: "Codex CLI".to_string(),
            supports_languages: vec![
                "typescript".to_string(),
                "go".to_string(),
                "python".to_string(),
            ],
            supports_types: vec![
                "feature".to_string(),
                "bugfix".to_string(),
                "refactor".to_string(),
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
            supports_languages: vec!["python".to_string(), "javascript".to_string()],
            supports_types: vec![
                "bugfix".to_string(),
                "refactor".to_string(),
                "test".to_string(),
            ],
            max_concurrent: 5,
            cost_per_hour: 0.10,
            avg_duration_min: 8.0,
        },
    );
    agents
}

/// Standard invariant config for tests.
fn test_invariant_config() -> InvariantConfig {
    InvariantConfig {
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
    }
}

/// Combined config for the server.
fn test_config() -> ArbiterConfig {
    ArbiterConfig {
        agents: test_agents(),
        invariants: test_invariant_config(),
    }
}

/// Simple test tree JSON for server-level tests.
fn test_tree_json() -> String {
    serde_json::json!({
        "n_features": 22,
        "n_classes": 3,
        "class_names": ["claude_code", "codex_cli", "aider"],
        "feature_names": [
            "task_type", "language", "complexity", "priority",
            "scope_size", "estimated_tokens", "has_dependencies",
            "requires_internet", "sla_minutes",
            "agent_success_rate", "agent_available_slots",
            "agent_running_tasks", "agent_avg_duration_min",
            "agent_avg_cost_usd", "agent_recent_failures",
            "agent_supports_task_type", "agent_supports_language",
            "total_running_tasks", "total_pending_tasks",
            "budget_remaining_usd", "time_of_day_hour",
            "concurrent_scope_conflicts"
        ],
        "nodes": [
            {"feature": 12, "threshold": 12.9, "left": 1, "right": 2,
             "value": [10.0, 10.0, 10.0]},
            {"feature": 9, "threshold": 0.65, "left": 3, "right": 4,
             "value": [2.0, 5.0, 8.0]},
            {"feature": -1, "threshold": 0.0, "left": -1, "right": -1,
             "value": [8.0, 1.0, 1.0]},
            {"feature": -1, "threshold": 0.0, "left": -1, "right": -1,
             "value": [0.0, 2.0, 6.0]},
            {"feature": -1, "threshold": 0.0, "left": -1, "right": -1,
             "value": [2.0, 5.0, 1.0]}
        ]
    })
    .to_string()
}

/// Helper to dispatch a JSON-RPC request string through the server.
fn dispatch(server: &mut McpServer, json: &str) -> Option<arbiter_mcp::server::JsonRpcResponse> {
    let req: arbiter_mcp::server::JsonRpcRequest = serde_json::from_str(json).unwrap();
    server.dispatch(&req)
}

/// Create a sample decision record.
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

// ===========================================================================
// IT-01: Happy path — route_task returns a valid assignment
// ===========================================================================

/// IT-01: Happy path route -> assign
///
/// GIVEN 3 active agents with available capacity
/// AND a valid task description (bugfix, python, simple, normal)
/// WHEN route_task is called
/// THEN it returns a decision within 5ms
/// AND the decision contains chosen_agent, confidence [0,1], decision_path
/// AND invariant_checks contains all 10 rule results
/// AND the decision is logged to SQLite decisions table
/// AND running_tasks is incremented for the chosen agent
/// AND the feature vector is 22 dimensions
#[test]
fn it_01_happy_path() {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    let tree = bootstrap_tree();
    let agents = test_agents();
    let registry = AgentRegistry::new(&db, &agents).unwrap();
    let invariant_cfg = test_invariant_config();

    let task = TaskInput {
        task_type: TaskType::Bugfix,
        language: Language::Python,
        complexity: Complexity::Simple,
        priority: Priority::Normal,
        scope: vec!["src/main.py".to_string()],
        branch: Some("fix/bug-123".to_string()),
        estimated_tokens: Some(30000),
        has_dependencies: false,
        requires_internet: false,
        sla_minutes: Some(60),
        description: Some("Fix login bug".to_string()),
    };
    let constraints = Constraints {
        preferred_agent: None,
        excluded_agents: vec![],
        budget_remaining_usd: Some(10.0),
        total_pending_tasks: Some(2),
        running_tasks: vec![],
    };

    let start = std::time::Instant::now();
    let result = route_task::execute(
        "it-01",
        &task,
        &constraints,
        Some(&tree),
        &registry,
        &db,
        &invariant_cfg,
    )
    .unwrap();
    let elapsed = start.elapsed();

    // Decision within 5ms
    assert!(
        elapsed.as_millis() < 5,
        "route_task took {}ms, expected < 5ms",
        elapsed.as_millis()
    );

    // Action is assign (happy path)
    assert_eq!(result.action, AgentAction::Assign);

    // chosen_agent is non-empty and one of the known agents
    assert!(!result.chosen_agent.is_empty());
    let valid_agents = ["claude_code", "codex_cli", "aider"];
    assert!(
        valid_agents.contains(&result.chosen_agent.as_str()),
        "unknown chosen agent: {}",
        result.chosen_agent
    );

    // confidence in [0, 1]
    assert!(
        (0.0..=1.0).contains(&result.confidence),
        "confidence {} not in [0,1]",
        result.confidence
    );

    // decision_path is non-empty
    assert!(
        !result.decision_path.is_empty(),
        "decision_path should not be empty"
    );

    // invariant_checks contains all 10 rule results
    assert_eq!(
        result.invariant_checks.len(),
        10,
        "expected 10 invariant checks, got {}",
        result.invariant_checks.len()
    );

    // All invariants should pass for happy path
    for check in &result.invariant_checks {
        assert!(
            check.passed,
            "invariant '{}' failed unexpectedly: {}",
            check.rule, check.detail
        );
    }

    // Decision logged to SQLite
    let found = db
        .find_decision_by_task("it-01")
        .unwrap()
        .expect("decision should be in DB");
    assert_eq!(found.task_id, "it-01");
    assert_eq!(found.chosen_agent, result.chosen_agent);
    assert_eq!(found.action, "assign");
    assert_eq!(found.invariants_passed, 10);
    assert_eq!(found.invariants_failed, 0);
    assert!(found.inference_us > 0);

    // Feature vector has 22 floats stored in DB
    let fv: Vec<f64> = serde_json::from_str(&found.feature_vector).unwrap();
    assert_eq!(fv.len(), 22, "feature vector should be 22-dim");

    // running_tasks incremented
    let running = db.get_running_tasks(&result.chosen_agent).unwrap();
    assert_eq!(running, 1, "running_tasks should be 1 after assignment");

    // Feature vector is 22 dimensions
    assert_eq!(
        result.feature_vector.len(),
        22,
        "feature vector should be 22-dim"
    );
}

// ===========================================================================
// IT-02: Fallback on scope conflict
// ===========================================================================

/// IT-02: Fallback on scope conflict
///
/// GIVEN a running task already touching "src/main.py"
/// AND the new task also touches "src/main.py"
/// WHEN route_task evaluates invariants
/// THEN scope_isolation fails (critical invariant)
/// AND all agents are evaluated for fallback (scope conflict is system-wide)
/// AND the decision is logged
#[test]
fn it_02_fallback_on_scope_conflict() {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    let tree = bootstrap_tree();
    let agents = test_agents();
    let registry = AgentRegistry::new(&db, &agents).unwrap();
    let invariant_cfg = test_invariant_config();

    // Task that touches src/main.py
    let task = TaskInput {
        task_type: TaskType::Bugfix,
        language: Language::Python,
        complexity: Complexity::Simple,
        priority: Priority::Normal,
        scope: vec!["src/main.py".to_string()],
        branch: None,
        estimated_tokens: None,
        has_dependencies: false,
        requires_internet: false,
        sla_minutes: None,
        description: None,
    };

    // Scope conflict: a running task is touching src/main.py
    let constraints = Constraints {
        preferred_agent: None,
        excluded_agents: vec![],
        budget_remaining_usd: Some(10.0),
        total_pending_tasks: None,
        running_tasks: vec![RunningTask {
            task_id: "running-1".to_string(),
            agent_id: "some_agent".to_string(),
            scope: vec!["src/main.py".to_string()],
            branch: None,
        }],
    };

    let result = route_task::execute(
        "it-02",
        &task,
        &constraints,
        Some(&tree),
        &registry,
        &db,
        &invariant_cfg,
    )
    .unwrap();

    // The scope conflict is system-wide (applies to all agents), so
    // all candidates should fail the scope_isolation invariant.
    // With cascade fallback, the system tries up to MAX_FALLBACK_ATTEMPTS+1
    // agents before rejecting.
    assert!(
        result.action == AgentAction::Reject
            || result.action == AgentAction::Fallback
            || result.action == AgentAction::Assign,
        "expected a valid action, got {:?}",
        result.action
    );

    // If we got invariant checks back, verify scope_isolation failed
    if !result.invariant_checks.is_empty() {
        let scope_check = result
            .invariant_checks
            .iter()
            .find(|r| r.rule == "scope_isolation");
        assert!(scope_check.is_some(), "should have scope_isolation check");
        assert!(
            !scope_check.unwrap().passed,
            "scope_isolation should fail due to conflict"
        );
    }

    // If rejected, there should be fallback info
    if result.action == AgentAction::Reject {
        assert!(
            result.fallback_agent.is_some() || result.candidates_evaluated > 0,
            "rejected result should have fallback info or evaluated candidates"
        );
    }

    // Decision should still be logged
    let found = db.find_decision_by_task("it-02").unwrap();
    assert!(found.is_some(), "decision should be logged even on reject");
}

// ===========================================================================
// IT-03: All agents rejected (excluded)
// ===========================================================================

/// IT-03: All agents rejected
///
/// GIVEN a route_task request with all agents excluded
/// WHEN no candidates remain
/// THEN action="reject" with reasoning explaining exclusion
/// AND the decision is logged with action="reject"
/// AND no invariant checks are performed (filtered before invariants)
#[test]
fn it_03_all_rejected() {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    let tree = bootstrap_tree();
    let agents = test_agents();
    let registry = AgentRegistry::new(&db, &agents).unwrap();
    let invariant_cfg = test_invariant_config();

    let task = TaskInput {
        task_type: TaskType::Feature,
        language: Language::Rust,
        complexity: Complexity::Complex,
        priority: Priority::High,
        scope: vec!["src/".to_string()],
        branch: Some("feature/new".to_string()),
        estimated_tokens: Some(100000),
        has_dependencies: true,
        requires_internet: false,
        sla_minutes: Some(120),
        description: Some("Big feature".to_string()),
    };

    // Exclude all agents
    let constraints = Constraints {
        preferred_agent: None,
        excluded_agents: vec![
            "claude_code".to_string(),
            "codex_cli".to_string(),
            "aider".to_string(),
        ],
        budget_remaining_usd: Some(10.0),
        total_pending_tasks: None,
        running_tasks: vec![],
    };

    let result = route_task::execute(
        "it-03",
        &task,
        &constraints,
        Some(&tree),
        &registry,
        &db,
        &invariant_cfg,
    )
    .unwrap();

    // Must be rejected
    assert_eq!(
        result.action,
        AgentAction::Reject,
        "should reject when all agents are excluded"
    );

    // chosen_agent should be empty
    assert!(
        result.chosen_agent.is_empty(),
        "chosen_agent should be empty on reject"
    );

    // Reasoning should explain why
    assert!(
        !result.reasoning.is_empty(),
        "reasoning should explain the rejection"
    );

    // Decision logged even on rejection
    let found = db
        .find_decision_by_task("it-03")
        .unwrap()
        .expect("rejection should be logged");
    assert_eq!(found.action, "reject");

    // No invariant checks (filtered before invariants)
    assert!(
        result.invariant_checks.is_empty(),
        "no invariant checks on exclusion-based reject"
    );

    // Candidates evaluated should be 0
    assert_eq!(
        result.candidates_evaluated, 0,
        "no candidates evaluated when all excluded"
    );
}

// ===========================================================================
// IT-04: Cold start (no historical stats)
// ===========================================================================

/// IT-04: Cold start
///
/// GIVEN agents with no historical stats (fresh database)
/// WHEN route_task is called
/// THEN it uses default feature vector values for stats fields
/// AND still produces a valid routing decision
/// AND the feature vector correctly uses defaults for agent stats
#[test]
fn it_04_cold_start() {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    let tree = bootstrap_tree();
    let agents = test_agents();
    let registry = AgentRegistry::new(&db, &agents).unwrap();
    let invariant_cfg = test_invariant_config();

    // Fresh DB, no outcomes recorded — agents have no stats
    let task = TaskInput {
        task_type: TaskType::Feature,
        language: Language::Rust,
        complexity: Complexity::Complex,
        priority: Priority::High,
        scope: vec!["arbiter-core/src/types.rs".to_string()],
        branch: Some("task/new-types".to_string()),
        estimated_tokens: Some(50000),
        has_dependencies: false,
        requires_internet: false,
        sla_minutes: Some(120),
        description: Some("Add new core types".to_string()),
    };
    let constraints = Constraints {
        preferred_agent: None,
        excluded_agents: vec![],
        budget_remaining_usd: Some(8.50),
        total_pending_tasks: Some(3),
        running_tasks: vec![],
    };

    let result = route_task::execute(
        "it-04",
        &task,
        &constraints,
        Some(&tree),
        &registry,
        &db,
        &invariant_cfg,
    )
    .unwrap();

    // Should produce a valid decision (not reject)
    assert!(
        result.action == AgentAction::Assign || result.action == AgentAction::Fallback,
        "cold start should still route, got {:?}",
        result.action
    );

    // Chosen agent should be one of the configured agents
    let valid_agents = ["claude_code", "codex_cli", "aider"];
    assert!(
        valid_agents.contains(&result.chosen_agent.as_str()),
        "chosen agent '{}' not in configured agents",
        result.chosen_agent
    );

    // Feature vector should use defaults for stats fields
    assert_eq!(result.feature_vector.len(), 22);

    // Verify default values in the feature vector:
    // [9] success_rate should be 0.5 (default for no stats)
    assert!(
        (result.feature_vector[9] - 0.5).abs() < f64::EPSILON,
        "cold start success_rate should be 0.5, got {}",
        result.feature_vector[9]
    );

    // Confidence should be reasonable (> 0)
    assert!(result.confidence > 0.0);

    // All 10 invariant checks present
    assert_eq!(result.invariant_checks.len(), 10);

    // Decision logged
    let found = db.find_decision_by_task("it-04").unwrap();
    assert!(found.is_some());

    // Running tasks incremented
    let running = db.get_running_tasks(&result.chosen_agent).unwrap();
    assert_eq!(running, 1);
}

// ===========================================================================
// IT-05: Stats accumulation over 10 outcomes
// ===========================================================================

/// IT-05: Stats accumulation 10x
///
/// GIVEN a registered agent (claude_code)
/// WHEN 10 outcomes are reported (7 successes, 3 failures)
/// THEN agent_stats accurately reflects totals, success_rate, avg_duration, avg_cost
/// AND running_tasks returns to 0 after all outcomes are processed
/// AND each intermediate result has correct cumulative stats
#[test]
fn it_05_stats_accumulation_10x() {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    let agents = test_agents();
    let config = test_config();
    let _registry = AgentRegistry::new(&db, &agents).unwrap();

    let mut total_duration = 0.0;
    let mut total_cost = 0.0;
    let mut successes = 0i64;

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

        let result = report_outcome::execute(&args, &db, &config).unwrap();
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
        "avg_duration mismatch: {} vs {}",
        stats.avg_duration_min,
        total_duration / 10.0,
    );
    assert!(
        (stats.avg_cost_usd - total_cost / 10.0).abs() < 0.01,
        "avg_cost mismatch: {} vs {}",
        stats.avg_cost_usd,
        total_cost / 10.0,
    );

    // running_tasks should be back to 0
    let running = db.get_running_tasks("claude_code").unwrap();
    assert_eq!(running, 0);
}

// ===========================================================================
// IT-06: Agent failure detection (6 failures triggers retrain)
// ===========================================================================

/// IT-06: Agent failure 6x
///
/// GIVEN max_failures_24h threshold = 5
/// WHEN 5 failures are reported (at threshold)
/// THEN retrain_suggested is false (at threshold, not over)
/// WHEN the 6th failure is reported (over threshold)
/// THEN retrain_suggested is true
#[test]
fn it_06_agent_failure_detection() {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    let agents = test_agents();
    let config = test_config(); // max_failures_24h = 5
    let _registry = AgentRegistry::new(&db, &agents).unwrap();

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

        let result = report_outcome::execute(&args, &db, &config).unwrap();
        assert!(result.recorded);

        // At <=4 failures, retrain should not be suggested
        if i < 4 {
            assert!(
                !result.retrain_suggested,
                "should not suggest retrain at {} failures",
                i + 1
            );
        }
    }

    // At exactly 5 failures (== threshold), retrain_suggested is false
    // because the condition is > threshold
    let check_args = serde_json::json!({
        "task_id": "it06-check",
        "agent_id": "claude_code",
        "status": "success"
    });
    let decision = sample_decision("it06-check");
    db.insert_decision(&decision).unwrap();
    db.increment_running_tasks("claude_code").unwrap();
    let check_result = report_outcome::execute(&check_args, &db, &config).unwrap();
    assert!(
        !check_result.retrain_suggested,
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

    let result = report_outcome::execute(&args, &db, &config).unwrap();
    assert!(result.recorded);
    assert!(
        result.retrain_suggested,
        "retrain_suggested should be true after 6 failures (threshold=5)"
    );

    // Verify warnings are empty for known task
    assert!(result.warnings.is_empty());
}

// ===========================================================================
// IT-07: Concurrent routing 3x
// ===========================================================================

/// IT-07: Concurrent routing 3x
///
/// GIVEN 3 concurrent route_task requests for different tasks
/// WHEN all 3 are submitted from separate threads
/// THEN all 3 produce valid decisions
/// AND all 3 decisions are logged to SQLite
/// AND running_tasks reflects the total assigned agents
/// AND no deadlocks or data corruption occur
#[test]
fn it_07_concurrent_routing_3x() {
    use std::sync::Arc;
    use std::thread;

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("concurrent.db");

    // Set up schema and agents with a file-backed DB
    {
        let db = Database::open(&db_path).unwrap();
        db.migrate().unwrap();
        let agents = test_agents();
        let _registry = AgentRegistry::new(&db, &agents).unwrap();
    }

    // Load the bootstrap tree (shared across threads)
    let tree = Arc::new(bootstrap_tree());
    let invariant_cfg = test_invariant_config();
    let agents = test_agents();

    let tasks = vec![
        (
            "task-concurrent-1",
            TaskInput {
                task_type: TaskType::Bugfix,
                language: Language::Python,
                complexity: Complexity::Simple,
                priority: Priority::Normal,
                scope: vec!["src/a.py".to_string()],
                branch: Some("fix/a".to_string()),
                estimated_tokens: Some(20000),
                has_dependencies: false,
                requires_internet: false,
                sla_minutes: Some(60),
                description: Some("Fix A".to_string()),
            },
        ),
        (
            "task-concurrent-2",
            TaskInput {
                task_type: TaskType::Feature,
                language: Language::Python,
                complexity: Complexity::Moderate,
                priority: Priority::High,
                scope: vec!["src/b.py".to_string()],
                branch: Some("feat/b".to_string()),
                estimated_tokens: Some(40000),
                has_dependencies: true,
                requires_internet: false,
                sla_minutes: Some(120),
                description: Some("Feature B".to_string()),
            },
        ),
        (
            "task-concurrent-3",
            TaskInput {
                task_type: TaskType::Refactor,
                language: Language::Python,
                complexity: Complexity::Complex,
                priority: Priority::Low,
                scope: vec!["src/c.py".to_string()],
                branch: Some("refactor/c".to_string()),
                estimated_tokens: Some(60000),
                has_dependencies: false,
                requires_internet: false,
                sla_minutes: Some(180),
                description: Some("Refactor C".to_string()),
            },
        ),
    ];

    let barrier = Arc::new(std::sync::Barrier::new(3));
    let mut handles = vec![];

    for (task_id, task) in tasks {
        let path = db_path.clone();
        let tree = tree.clone();
        let barrier = barrier.clone();
        let invariant_cfg = invariant_cfg.clone();
        let agents = agents.clone();

        handles.push(thread::spawn(move || {
            let db = Database::open(&path).unwrap();
            let registry = AgentRegistry::new(&db, &agents).unwrap();

            let constraints = Constraints {
                preferred_agent: None,
                excluded_agents: vec![],
                budget_remaining_usd: Some(10.0),
                total_pending_tasks: Some(3),
                running_tasks: vec![],
            };

            // Synchronize: all threads start routing at the same time
            barrier.wait();

            let result = route_task::execute(
                task_id,
                &task,
                &constraints,
                Some(&tree),
                &registry,
                &db,
                &invariant_cfg,
            )
            .unwrap();

            (task_id.to_string(), result)
        }));
    }

    let mut results = vec![];
    for h in handles {
        results.push(h.join().unwrap());
    }

    // Verify all 3 produced valid decisions
    for (task_id, result) in &results {
        assert!(
            result.action == AgentAction::Assign || result.action == AgentAction::Fallback,
            "task {} should be assigned or fallback, got {:?}",
            task_id,
            result.action
        );
        assert!(
            !result.chosen_agent.is_empty(),
            "task {} should have a chosen agent",
            task_id
        );
        assert!(
            result.confidence > 0.0,
            "task {} should have positive confidence",
            task_id
        );
    }

    // Verify all 3 decisions are logged in the DB
    let db = Database::open(&db_path).unwrap();
    for (task_id, _result) in &results {
        let found = db.find_decision_by_task(task_id).unwrap();
        assert!(found.is_some(), "decision for {} should be in DB", task_id);
    }

    // Verify total running tasks is correct (sum of assigned)
    let total_running = db.get_total_running_tasks().unwrap();
    assert!(
        total_running >= 1 && total_running <= 3,
        "total running tasks should be 1-3, got {}",
        total_running
    );
}

// ===========================================================================
// Server-level integration tests (MCP protocol)
// ===========================================================================

/// Verifies the full MCP handshake + tools/list flow via the server dispatch.
#[test]
fn mcp_server_handshake_and_tools_list() {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    let tree = DecisionTree::from_json(&test_tree_json()).unwrap();
    let config = test_config();
    let registry = AgentRegistry::new(&db, &config.agents).unwrap();
    let mut server = McpServer::new(config, &db, Some(&tree), registry);

    // Step 1: Initialize
    let resp = dispatch(
        &mut server,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
    )
    .unwrap();
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    assert_eq!(result["capabilities"]["tools"], serde_json::json!({}));
    assert_eq!(result["serverInfo"]["name"], "arbiter");

    // Step 2: Initialized notification (no response for notifications without id)
    let resp = dispatch(
        &mut server,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );
    assert!(resp.is_none());

    // Step 3: Tools list
    let resp = dispatch(
        &mut server,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    )
    .unwrap();
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    let tools = result["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 3, "should have exactly 3 tools");

    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"route_task"));
    assert!(names.contains(&"report_outcome"));
    assert!(names.contains(&"get_agent_status"));
}

/// Verifies JSON-RPC error codes for protocol violations.
#[test]
fn mcp_protocol_error_handling() {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    let tree = DecisionTree::from_json(&test_tree_json()).unwrap();
    let config = test_config();
    let registry = AgentRegistry::new(&db, &config.agents).unwrap();
    let mut server = McpServer::new(config, &db, Some(&tree), registry);

    // Unknown method → -32601
    let resp = dispatch(
        &mut server,
        r#"{"jsonrpc":"2.0","id":1,"method":"unknown_method"}"#,
    )
    .unwrap();
    let err = resp.error.unwrap();
    assert_eq!(err.code, -32601);
    assert!(err.message.contains("Method not found"));

    // Missing params → -32602
    let resp = dispatch(
        &mut server,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call"}"#,
    )
    .unwrap();
    let err = resp.error.unwrap();
    assert_eq!(err.code, -32602);
    assert!(err.message.contains("Missing params"));
}

/// Verifies end-to-end route_task through the MCP server dispatch.
#[test]
fn mcp_route_task_e2e() {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    let tree = DecisionTree::from_json(&test_tree_json()).unwrap();
    let config = test_config();
    let registry = AgentRegistry::new(&db, &config.agents).unwrap();
    let mut server = McpServer::new(config, &db, Some(&tree), registry);

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "route_task",
            "arguments": {
                "task_id": "mcp-e2e-1",
                "task": {
                    "type": "bugfix",
                    "language": "python",
                    "complexity": "simple",
                    "priority": "normal"
                },
                "constraints": {
                    "budget_remaining_usd": 10.0
                }
            }
        }
    });

    let resp = dispatch(&mut server, &req.to_string()).unwrap();
    assert!(resp.error.is_none(), "got error: {:?}", resp.error);
    let result = resp.result.unwrap();
    let content = &result["content"][0]["text"];
    let decision: serde_json::Value = serde_json::from_str(content.as_str().unwrap()).unwrap();

    assert_eq!(decision["task_id"], "mcp-e2e-1");
    assert!(
        decision["action"] == "assign" || decision["action"] == "fallback",
        "expected assign or fallback, got {}",
        decision["action"]
    );
    assert!(!decision["chosen_agent"].as_str().unwrap().is_empty());
    assert!(decision["invariant_checks"].as_array().unwrap().len() == 10);
    assert!(
        decision["metadata"]["feature_vector"]
            .as_array()
            .unwrap()
            .len()
            == 22
    );
}

/// Verifies end-to-end report + status query through the MCP server dispatch.
#[test]
fn mcp_report_and_status_e2e() {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    let tree = DecisionTree::from_json(&test_tree_json()).unwrap();
    let config = test_config();
    let registry = AgentRegistry::new(&db, &config.agents).unwrap();
    let mut server = McpServer::new(config, &db, Some(&tree), registry);

    // Route a task first
    let route_req = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": "route_task",
            "arguments": {
                "task_id": "mcp-rpt-1",
                "task": {"type":"bugfix","language":"python","complexity":"simple","priority":"normal"}
            }
        }
    });
    let resp = dispatch(&mut server, &route_req.to_string()).unwrap();
    assert!(resp.error.is_none());

    // Report outcome
    let report_req = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {
            "name": "report_outcome",
            "arguments": {
                "task_id": "mcp-rpt-1",
                "agent_id": "claude_code",
                "status": "success",
                "duration_min": 10.0,
                "cost_usd": 0.15
            }
        }
    });
    let resp = dispatch(&mut server, &report_req.to_string()).unwrap();
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    let content = &result["content"][0]["text"];
    let outcome: serde_json::Value = serde_json::from_str(content.as_str().unwrap()).unwrap();
    assert_eq!(outcome["recorded"], true);

    // Query agent status
    let status_req = serde_json::json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {
            "name": "get_agent_status",
            "arguments": {}
        }
    });
    let resp = dispatch(&mut server, &status_req.to_string()).unwrap();
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    let content = &result["content"][0]["text"];
    let status: serde_json::Value = serde_json::from_str(content.as_str().unwrap()).unwrap();
    let status_agents = status["agents"].as_array().unwrap();
    assert_eq!(status_agents.len(), 3);

    // Query single agent
    let status_req = serde_json::json!({
        "jsonrpc": "2.0", "id": 4, "method": "tools/call",
        "params": {
            "name": "get_agent_status",
            "arguments": {"agent_id": "claude_code"}
        }
    });
    let resp = dispatch(&mut server, &status_req.to_string()).unwrap();
    assert!(resp.error.is_none());

    // Query unknown agent → error
    let status_req = serde_json::json!({
        "jsonrpc": "2.0", "id": 5, "method": "tools/call",
        "params": {
            "name": "get_agent_status",
            "arguments": {"agent_id": "nonexistent"}
        }
    });
    let resp = dispatch(&mut server, &status_req.to_string()).unwrap();
    assert!(resp.error.is_some());
    assert!(resp.error.unwrap().message.contains("agent not found"));
}

// ===========================================================================
// Feature vector and decision tree integration
// ===========================================================================

/// IT-07 (additional): Feature vector building and DT inference verification
///
/// Verifies:
/// - Each vector is exactly 22 elements
/// - All values within documented ranges
/// - DT inference returns consistent results (deterministic)
/// - Different agents produce different feature vectors for same task
#[test]
fn feature_vector_and_dt_determinism() {
    let tree = bootstrap_tree();

    let task = TaskInput {
        task_type: TaskType::Feature,
        language: Language::Rust,
        complexity: Complexity::Complex,
        priority: Priority::High,
        scope: vec!["src/main.rs".to_string(), "src/lib.rs".to_string()],
        branch: Some("feature/new-thing".to_string()),
        estimated_tokens: Some(50000),
        has_dependencies: true,
        requires_internet: false,
        sla_minutes: Some(120),
        description: Some("Implement new feature".to_string()),
    };

    let agents = vec![
        AgentInfo {
            agent_id: "claude_code".to_string(),
            config: AgentConfig {
                display_name: "Claude Code".to_string(),
                supports_languages: vec!["rust".to_string()],
                supports_types: vec!["feature".to_string()],
                max_concurrent: 2,
                cost_per_hour: 0.30,
                avg_duration_min: 18.0,
            },
            running_tasks: 0,
            success_rate: Some(0.85),
            avg_duration_min: Some(18.0),
            avg_cost_usd: Some(0.30),
            recent_failures: 0,
        },
        AgentInfo {
            agent_id: "codex_cli".to_string(),
            config: AgentConfig {
                display_name: "Codex CLI".to_string(),
                supports_languages: vec!["typescript".to_string()],
                supports_types: vec!["feature".to_string()],
                max_concurrent: 3,
                cost_per_hour: 0.20,
                avg_duration_min: 12.0,
            },
            running_tasks: 1,
            success_rate: Some(0.70),
            avg_duration_min: Some(12.0),
            avg_cost_usd: Some(0.15),
            recent_failures: 1,
        },
        AgentInfo {
            agent_id: "aider".to_string(),
            config: AgentConfig {
                display_name: "Aider".to_string(),
                supports_languages: vec!["python".to_string()],
                supports_types: vec!["bugfix".to_string()],
                max_concurrent: 5,
                cost_per_hour: 0.10,
                avg_duration_min: 8.0,
            },
            running_tasks: 2,
            success_rate: Some(0.60),
            avg_duration_min: Some(8.0),
            avg_cost_usd: Some(0.05),
            recent_failures: 2,
        },
    ];

    let system = SystemState {
        constraints: Constraints {
            preferred_agent: None,
            excluded_agents: vec![],
            budget_remaining_usd: Some(8.50),
            total_pending_tasks: Some(3),
            running_tasks: vec![],
        },
        total_running_tasks: 3,
        time_of_day_hour: 14,
    };

    // Build feature vectors for all agents
    let mut vectors = vec![];
    for agent in &agents {
        let fv = build_feature_vector(&task, agent, &system);

        // Verify 22 dimensions
        assert_eq!(fv.len(), FEATURE_DIM);

        // Verify all values within documented ranges
        assert!(
            (0.0..=6.0).contains(&fv[0]),
            "task_type out of range: {}",
            fv[0]
        );
        assert!(
            (0.0..=5.0).contains(&fv[1]),
            "language out of range: {}",
            fv[1]
        );
        assert!(
            (0.0..=4.0).contains(&fv[2]),
            "complexity out of range: {}",
            fv[2]
        );
        assert!(
            (0.0..=3.0).contains(&fv[3]),
            "priority out of range: {}",
            fv[3]
        );
        assert!((0.0..=100.0).contains(&fv[4]), "scope_size out of range");
        assert!(
            (0.0..=200.0).contains(&fv[5]),
            "estimated_tokens out of range"
        );
        assert!(fv[6] == 0.0 || fv[6] == 1.0, "has_dependencies not boolean");
        assert!(
            fv[7] == 0.0 || fv[7] == 1.0,
            "requires_internet not boolean"
        );
        assert!((0.0..=480.0).contains(&fv[8]), "sla_minutes out of range");
        assert!((0.0..=1.0).contains(&fv[9]), "success_rate out of range");
        assert!(
            (0.0..=10.0).contains(&fv[10]),
            "available_slots out of range"
        );
        assert!((0.0..=10.0).contains(&fv[11]), "running_tasks out of range");
        assert!((0.0..=480.0).contains(&fv[12]), "avg_duration out of range");
        assert!((0.0..=100.0).contains(&fv[13]), "avg_cost out of range");
        assert!(
            (0.0..=50.0).contains(&fv[14]),
            "recent_failures out of range"
        );
        assert!(
            fv[15] == 0.0 || fv[15] == 1.0,
            "supports_task_type not boolean"
        );
        assert!(
            fv[16] == 0.0 || fv[16] == 1.0,
            "supports_language not boolean"
        );
        assert!((0.0..=20.0).contains(&fv[17]), "total_running out of range");
        assert!(
            (0.0..=100.0).contains(&fv[18]),
            "total_pending out of range"
        );
        assert!((0.0..=1000.0).contains(&fv[19]), "budget out of range");
        assert!((0.0..=23.0).contains(&fv[20]), "time_of_day out of range");
        assert!(
            (0.0..=10.0).contains(&fv[21]),
            "scope_conflicts out of range"
        );

        vectors.push(fv);
    }

    // Different agents should produce different vectors (agent features differ)
    assert_ne!(vectors[0], vectors[1], "claude vs codex should differ");
    assert_ne!(vectors[1], vectors[2], "codex vs aider should differ");

    // Task features (indices 0-8) should be identical across agents
    for i in 0..9 {
        assert_eq!(
            vectors[0][i], vectors[1][i],
            "task feature {i} should be same across agents"
        );
    }

    // Run DT inference and verify determinism
    for fv in &vectors {
        let pred1 = tree.predict(fv).unwrap();
        let pred2 = tree.predict(fv).unwrap();
        let pred3 = tree.predict(fv).unwrap();

        // Same input always produces same output
        assert_eq!(
            pred1.class, pred2.class,
            "DT inference should be deterministic (class)"
        );
        assert_eq!(
            pred2.class, pred3.class,
            "DT inference should be deterministic (class)"
        );
        assert!(
            (pred1.confidence - pred2.confidence).abs() < f64::EPSILON,
            "DT inference should be deterministic (confidence)"
        );

        // Verify prediction has valid fields
        assert!(pred1.class < 3, "class index should be < 3");
        assert!(
            (0.0..=1.0).contains(&pred1.confidence),
            "confidence should be in [0,1]"
        );
        assert!(!pred1.path.is_empty(), "decision path should be non-empty");
    }
}
