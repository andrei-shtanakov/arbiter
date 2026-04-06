//! Criterion benchmarks for Arbiter routing pipeline.
//!
//! Two benchmarks:
//! - `bench_route_task` — full routing pipeline with in-memory DB
//! - `bench_dt_predict` — isolated decision tree prediction

#![allow(clippy::arc_with_non_send_sync)]

use std::collections::HashMap;
use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, Criterion};

use arbiter_core::policy::decision_tree::DecisionTree;
use arbiter_core::types::*;

use arbiter_mcp::agents::AgentRegistry;
use arbiter_mcp::config::*;
use arbiter_mcp::db::Database;
use arbiter_mcp::metrics::Metrics;
use arbiter_mcp::tools::route_task;

// ---------------------------------------------------------------------------
// Helpers (mirror arbiter-cli main.rs setup)
// ---------------------------------------------------------------------------

fn load_bootstrap_tree() -> DecisionTree {
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

fn bench_agents() -> HashMap<String, AgentConfig> {
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

fn bench_invariant_config() -> InvariantConfig {
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
            max_total_concurrent: 10,
        },
        sla: SlaConfig {
            buffer_multiplier: 1.5,
        },
    }
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

fn bench_route_task(c: &mut Criterion) {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    let db = Arc::new(db);
    let tree = load_bootstrap_tree();
    let agents = bench_agents();
    let registry = AgentRegistry::new(Arc::clone(&db), &agents).unwrap();
    let invariant_cfg = bench_invariant_config();
    let metrics = Metrics::new();

    let task = TaskInput {
        task_type: TaskType::Bugfix,
        language: Language::Python,
        complexity: Complexity::Simple,
        priority: Priority::Normal,
        scope: vec!["src/main.py".to_string()],
        branch: Some("fix/bench".to_string()),
        estimated_tokens: Some(25000),
        has_dependencies: false,
        requires_internet: false,
        sla_minutes: Some(60),
        description: Some("Benchmark task".to_string()),
    };

    let constraints = Constraints {
        preferred_agent: None,
        excluded_agents: vec![],
        budget_remaining_usd: Some(100.0),
        total_pending_tasks: Some(5),
        running_tasks: vec![],
        retry_count: None,
        calls_per_minute: None,
    };

    let mut i: u64 = 0;

    c.bench_function("route_task", |b| {
        b.iter(|| {
            let result = route_task::execute(
                black_box(&format!("bench-{i}")),
                black_box(&task),
                black_box(&constraints),
                Some(black_box(&tree)),
                &registry,
                &db,
                &invariant_cfg,
                &metrics,
            )
            .unwrap();
            // Prevent slot exhaustion
            let _ = db.decrement_running_tasks(&result.chosen_agent);
            i += 1;
        });
    });
}

fn bench_dt_predict(c: &mut Criterion) {
    let tree = load_bootstrap_tree();

    // A representative 22-dimensional feature vector
    let features: Vec<f64> = vec![
        1.0, 0.0, 1.0, 1.0, 1.0, 50.0, 0.0, 0.0, 120.0, 0.85, 2.0, 0.0, 15.0, 0.1, 0.0, 1.0, 1.0,
        0.0, 0.0, 10.0, 14.0, 0.0,
    ];

    c.bench_function("dt_predict", |b| {
        b.iter(|| {
            tree.predict(black_box(&features)).unwrap();
        });
    });
}

criterion_group!(benches, bench_route_task, bench_dt_predict);
criterion_main!(benches);
