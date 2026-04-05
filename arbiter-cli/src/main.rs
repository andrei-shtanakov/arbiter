//! Arbiter CLI — smoke tests and benchmarks.
//!
//! Usage:
//!   arbiter-cli bench   Run all benchmarks
//!   arbiter-cli help    Print usage

use std::collections::HashMap;
use std::time::Instant;

use anyhow::Result;

use arbiter_core::policy::decision_tree::DecisionTree;
use arbiter_core::types::*;

use arbiter_mcp::agents::AgentRegistry;
use arbiter_mcp::config::*;
use arbiter_mcp::db::{Database, DecisionRecord, OutcomeRecord};
use arbiter_mcp::tools::route_task;

// ---------------------------------------------------------------------------
// Config helpers
// ---------------------------------------------------------------------------

/// Load the bootstrap decision tree from models/.
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

/// Create the 3-agent config.
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

fn bench_config() -> ArbiterConfig {
    ArbiterConfig {
        agents: bench_agents(),
        invariants: bench_invariant_config(),
    }
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

/// BT-01: Route throughput > 10,000 decisions/sec (in-process)
///
/// Measures how many route_task invocations complete per second
/// without I/O overhead. Uses in-memory DB and pre-loaded tree.
fn bench_route_throughput() -> Result<()> {
    eprintln!("BT-01: Route throughput benchmark");

    let db = Database::open_in_memory()?;
    db.migrate()?;
    let tree = load_bootstrap_tree();
    let agents = bench_agents();
    let registry = AgentRegistry::new(&db, &agents)?;
    let invariant_cfg = bench_invariant_config();

    let task_types = [
        TaskType::Bugfix,
        TaskType::Feature,
        TaskType::Refactor,
        TaskType::Test,
    ];
    let languages = [Language::Python, Language::Rust, Language::Typescript];
    let complexities = [
        Complexity::Simple,
        Complexity::Moderate,
        Complexity::Complex,
    ];

    // Pre-generate 1000 task payloads with minor variations
    let n = 1000;
    let mut tasks = Vec::with_capacity(n);
    for i in 0..n {
        tasks.push(TaskInput {
            task_type: task_types[i % task_types.len()],
            language: languages[i % languages.len()],
            complexity: complexities[i % complexities.len()],
            priority: Priority::Normal,
            scope: vec![format!("src/file_{}.py", i % 50)],
            branch: Some(format!("task/bench-{i}")),
            estimated_tokens: Some(20000 + (i as u64 * 100)),
            has_dependencies: i % 5 == 0,
            requires_internet: false,
            sla_minutes: Some(60 + (i as u32 % 120)),
            description: Some(format!("Benchmark task {i}")),
        });
    }

    let constraints = Constraints {
        preferred_agent: None,
        excluded_agents: vec![],
        budget_remaining_usd: Some(100.0),
        total_pending_tasks: Some(5),
        running_tasks: vec![],
        retry_count: None,
        calls_per_minute: None,
    };

    // Warm up
    for (i, task) in tasks.iter().enumerate().take(10) {
        let _ = route_task::execute(
            &format!("warmup-{i}"),
            task,
            &constraints,
            Some(&tree),
            &registry,
            &db,
            &invariant_cfg,
        );
        // Reset running tasks for warmup
        let _ = db.decrement_running_tasks("claude_code");
        let _ = db.decrement_running_tasks("codex_cli");
        let _ = db.decrement_running_tasks("aider");
    }

    // Measure
    let start = Instant::now();
    for (i, task) in tasks.iter().enumerate() {
        let _ = route_task::execute(
            &format!("bench-{i}"),
            task,
            &constraints,
            Some(&tree),
            &registry,
            &db,
            &invariant_cfg,
        )?;
        // Don't accumulate running tasks (would exhaust slots quickly)
        let _ = db.decrement_running_tasks("claude_code");
        let _ = db.decrement_running_tasks("codex_cli");
        let _ = db.decrement_running_tasks("aider");
    }
    let elapsed = start.elapsed();
    let throughput = n as f64 / elapsed.as_secs_f64();

    eprintln!(
        "  {n} decisions in {:.2}ms ({throughput:.0} decisions/sec)",
        elapsed.as_secs_f64() * 1000.0
    );
    eprintln!(
        "  Target: > 10,000 decisions/sec — {}",
        if throughput > 10_000.0 {
            "PASS"
        } else {
            "FAIL"
        }
    );

    // In debug builds, the threshold may not be met due to lack of
    // optimizations.  Enforce only in release mode.
    #[cfg(not(debug_assertions))]
    assert!(
        throughput > 10_000.0,
        "BT-01 FAIL: throughput {throughput:.0}/sec < 10,000/sec"
    );
    #[cfg(debug_assertions)]
    if throughput <= 10_000.0 {
        eprintln!("  (debug build — skipping assertion)");
    }
    Ok(())
}

/// BT-02: Route e2e latency < 5ms p99
///
/// Measures real-world latency including JSON serialization,
/// feature building, DT inference, invariant checks, and DB write.
fn bench_route_latency_p99() -> Result<()> {
    eprintln!("BT-02: Route e2e latency p99 benchmark");

    let db = Database::open_in_memory()?;
    db.migrate()?;
    let tree = load_bootstrap_tree();
    let agents = bench_agents();
    let registry = AgentRegistry::new(&db, &agents)?;
    let invariant_cfg = bench_invariant_config();

    let n = 100;
    let mut latencies_us = Vec::with_capacity(n);

    let constraints = Constraints {
        preferred_agent: None,
        excluded_agents: vec![],
        budget_remaining_usd: Some(50.0),
        total_pending_tasks: Some(3),
        running_tasks: vec![],
        retry_count: None,
        calls_per_minute: None,
    };

    for i in 0..n {
        let task = TaskInput {
            task_type: if i % 2 == 0 {
                TaskType::Bugfix
            } else {
                TaskType::Feature
            },
            language: Language::Python,
            complexity: Complexity::Simple,
            priority: Priority::Normal,
            scope: vec![format!("src/mod_{}.py", i)],
            branch: Some(format!("fix/lat-{i}")),
            estimated_tokens: Some(25000),
            has_dependencies: false,
            requires_internet: false,
            sla_minutes: Some(60),
            description: Some(format!("Latency test {i}")),
        };

        let start = Instant::now();
        let _ = route_task::execute(
            &format!("lat-{i}"),
            &task,
            &constraints,
            Some(&tree),
            &registry,
            &db,
            &invariant_cfg,
        )?;
        let elapsed = start.elapsed();
        latencies_us.push(elapsed.as_micros());

        // Reset running tasks
        let _ = db.decrement_running_tasks("claude_code");
        let _ = db.decrement_running_tasks("codex_cli");
        let _ = db.decrement_running_tasks("aider");
    }

    latencies_us.sort();
    let p99_idx = (n * 99 / 100).max(1) - 1;
    let p99_us = latencies_us[p99_idx];
    let p99_ms = p99_us as f64 / 1000.0;
    let median_us = latencies_us[n / 2];
    let mean_us = latencies_us.iter().sum::<u128>() / n as u128;

    eprintln!(
        "  {n} requests: mean={mean_us}us, median={median_us}us, p99={p99_us}us ({p99_ms:.2}ms)"
    );
    eprintln!(
        "  Target: p99 < 5ms — {}",
        if p99_ms < 5.0 { "PASS" } else { "FAIL" }
    );

    assert!(p99_ms < 5.0, "BT-02 FAIL: p99 latency {p99_ms:.2}ms > 5ms");
    Ok(())
}

/// BT-03: Report outcome latency < 10ms p99
///
/// Measures end-to-end latency of report_outcome including SQLite write.
fn bench_report_latency_p99() -> Result<()> {
    eprintln!("BT-03: Report outcome latency p99 benchmark");

    let db = Database::open_in_memory()?;
    db.migrate()?;
    let agents = bench_agents();
    let config = bench_config();
    let _registry = AgentRegistry::new(&db, &agents)?;

    let n = 100;
    let mut latencies_us = Vec::with_capacity(n);
    let agent_ids = ["claude_code", "codex_cli", "aider"];

    // Pre-insert decisions so report_outcome can find them
    for i in 0..n {
        let decision = DecisionRecord {
            task_id: format!("rpt-{i}"),
            task_json:
                r#"{"type":"bugfix","language":"python","complexity":"simple","priority":"normal"}"#
                    .to_string(),
            feature_vector: "[]".to_string(),
            constraints_json: None,
            chosen_agent: agent_ids[i % agent_ids.len()].to_string(),
            action: "assign".to_string(),
            confidence: 0.85,
            decision_path: "[]".to_string(),
            fallback_agent: None,
            fallback_reason: None,
            invariants_json: "[]".to_string(),
            invariants_passed: 10,
            invariants_failed: 0,
            inference_us: 50,
        };
        db.insert_decision(&decision)?;
        db.increment_running_tasks(agent_ids[i % agent_ids.len()])?;
    }

    // Measure report_outcome latency
    for i in 0..n {
        let agent_id = agent_ids[i % agent_ids.len()];
        let is_success = i % 4 != 0;

        let args = serde_json::json!({
            "task_id": format!("rpt-{i}"),
            "agent_id": agent_id,
            "status": if is_success { "success" } else { "failure" },
            "duration_min": 5.0 + (i as f64 * 0.1),
            "tokens_used": 10000 + (i as i64 * 100),
            "cost_usd": 0.05 + (i as f64 * 0.01),
            "exit_code": if is_success { 0 } else { 1 },
            "files_changed": i as i64 % 5,
            "tests_passed": is_success,
            "validation_passed": is_success,
            "retry_count": 0
        });

        let start = Instant::now();
        let _ = arbiter_mcp::tools::report_outcome::execute(&args, &db, &config)?;
        let elapsed = start.elapsed();
        latencies_us.push(elapsed.as_micros());
    }

    latencies_us.sort();
    let p99_idx = (n * 99 / 100).max(1) - 1;
    let p99_us = latencies_us[p99_idx];
    let p99_ms = p99_us as f64 / 1000.0;
    let median_us = latencies_us[n / 2];
    let mean_us = latencies_us.iter().sum::<u128>() / n as u128;

    eprintln!(
        "  {n} reports: mean={mean_us}us, median={median_us}us, p99={p99_us}us ({p99_ms:.2}ms)"
    );
    eprintln!(
        "  Target: p99 < 10ms — {}",
        if p99_ms < 10.0 { "PASS" } else { "FAIL" }
    );

    assert!(
        p99_ms < 10.0,
        "BT-03 FAIL: p99 latency {p99_ms:.2}ms > 10ms"
    );
    Ok(())
}

/// BT-04: Memory usage < 50MB RSS
///
/// Measures peak process memory after initialization and 1000 route+report cycles.
fn bench_memory_usage() -> Result<()> {
    eprintln!("BT-04: Memory usage benchmark");

    let db = Database::open_in_memory()?;
    db.migrate()?;
    let tree = load_bootstrap_tree();
    let agents = bench_agents();
    let config = bench_config();
    let registry = AgentRegistry::new(&db, &agents)?;
    let invariant_cfg = bench_invariant_config();

    // Measure RSS after initialization
    let init_rss = get_rss_mb();
    eprintln!("  RSS after init: {init_rss:.1} MB");

    // Execute 1000 route_task + report_outcome cycles
    let n = 1000;
    let constraints = Constraints {
        preferred_agent: None,
        excluded_agents: vec![],
        budget_remaining_usd: Some(100.0),
        total_pending_tasks: Some(5),
        running_tasks: vec![],
        retry_count: None,
        calls_per_minute: None,
    };

    let agent_ids = ["claude_code", "codex_cli", "aider"];

    for i in 0..n {
        let task = TaskInput {
            task_type: TaskType::Bugfix,
            language: Language::Python,
            complexity: Complexity::Simple,
            priority: Priority::Normal,
            scope: vec![format!("src/mem_{}.py", i % 50)],
            branch: None,
            estimated_tokens: Some(20000),
            has_dependencies: false,
            requires_internet: false,
            sla_minutes: Some(60),
            description: None,
        };

        let _ = route_task::execute(
            &format!("mem-{i}"),
            &task,
            &constraints,
            Some(&tree),
            &registry,
            &db,
            &invariant_cfg,
        )?;

        // Report outcome
        let agent_id = agent_ids[i % agent_ids.len()];
        let args = serde_json::json!({
            "task_id": format!("mem-{i}"),
            "agent_id": agent_id,
            "status": "success",
            "duration_min": 5.0,
            "cost_usd": 0.10
        });
        let _ = arbiter_mcp::tools::report_outcome::execute(&args, &db, &config)?;

        // Reset running tasks to avoid slot exhaustion
        let _ = db.decrement_running_tasks("claude_code");
        let _ = db.decrement_running_tasks("codex_cli");
        let _ = db.decrement_running_tasks("aider");
    }

    let peak_rss = get_rss_mb();
    eprintln!("  RSS after {n} cycles: {peak_rss:.1} MB");
    eprintln!(
        "  Target: < 50 MB — {}",
        if peak_rss < 50.0 { "PASS" } else { "FAIL" }
    );

    assert!(
        peak_rss < 50.0,
        "BT-04 FAIL: peak RSS {peak_rss:.1} MB > 50 MB"
    );
    Ok(())
}

/// BT-05: SQLite size < 10MB after 10K decisions
///
/// Measures database file size after inserting 10K decisions and 10K outcomes.
fn bench_sqlite_size() -> Result<()> {
    eprintln!("BT-05: SQLite size after 10K decisions benchmark");

    let dir = tempfile::tempdir()?;
    let db_path = dir.path().join("bench.db");

    let db = Database::open(&db_path)?;
    db.migrate()?;

    // Register agents
    let agents = bench_agents();
    let _registry = AgentRegistry::new(&db, &agents)?;

    let agent_ids = ["claude_code", "codex_cli", "aider"];
    let n = 10_000;

    // Insert 10K decision records
    for i in 0..n {
        let decision = DecisionRecord {
            task_id: format!("task-{i:05}"),
            task_json: format!(
                r#"{{"type":"bugfix","language":"python","complexity":"simple","priority":"normal","description":"Task {i}"}}"#
            ),
            feature_vector: format!(
                "[{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}]",
                1.0,
                0.0,
                1.0,
                1.0,
                1.0,
                50.0,
                0.0,
                0.0,
                120.0,
                0.5 + (i as f64 % 50.0) / 100.0,
                2.0,
                0.0,
                15.0,
                0.1,
                0.0,
                1.0,
                1.0,
                0.0,
                0.0,
                10.0,
                14.0,
                0.0
            ),
            constraints_json: Some(r#"{"budget_remaining_usd":8.5}"#.to_string()),
            chosen_agent: agent_ids[i % agent_ids.len()].to_string(),
            action: if i % 20 == 0 { "fallback" } else { "assign" }.to_string(),
            confidence: 0.70 + (i as f64 % 30.0) / 100.0,
            decision_path: r#"["node 0: feature[2] <= 2.5","leaf: class 0"]"#.to_string(),
            fallback_agent: if i % 20 == 0 {
                Some("codex_cli".to_string())
            } else {
                None
            },
            fallback_reason: if i % 20 == 0 {
                Some("scope conflict".to_string())
            } else {
                None
            },
            invariants_json:
                r#"[{"rule":"agent_available","severity":"critical","passed":true,"detail":"ok"}]"#
                    .to_string(),
            invariants_passed: 10,
            invariants_failed: 0,
            inference_us: 50 + (i as i64 % 100),
        };
        db.insert_decision(&decision)?;
    }

    // Insert 10K outcome records
    for i in 0..n {
        let is_success = i % 5 != 0;
        let outcome = OutcomeRecord {
            task_id: format!("task-{i:05}"),
            decision_id: Some(i as i64 + 1),
            agent_id: agent_ids[i % agent_ids.len()].to_string(),
            status: if is_success {
                "success".to_string()
            } else {
                "failure".to_string()
            },
            duration_min: Some(5.0 + (i as f64 % 20.0)),
            tokens_used: Some(10000 + (i as i64 % 50000)),
            cost_usd: Some(0.05 + (i as f64 % 100.0) / 1000.0),
            exit_code: Some(if is_success { 0 } else { 1 }),
            files_changed: Some((i % 10) as i32),
            tests_passed: Some(is_success),
            validation_passed: Some(is_success),
            error_summary: if !is_success {
                Some(format!("error at step {i}"))
            } else {
                None
            },
            retry_count: (i % 3) as i32,
        };
        db.insert_outcome(&outcome)?;
    }

    // Measure file size
    let metadata = std::fs::metadata(&db_path)?;
    let size_bytes = metadata.len();
    let size_mb = size_bytes as f64 / (1024.0 * 1024.0);

    eprintln!("  {n} decisions + {n} outcomes");
    eprintln!("  DB size: {size_bytes} bytes ({size_mb:.2} MB)");
    eprintln!(
        "  Target: < 10 MB — {}",
        if size_mb < 10.0 { "PASS" } else { "FAIL" }
    );

    assert!(
        size_mb < 10.0,
        "BT-05 FAIL: DB size {size_mb:.2} MB > 10 MB"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Memory measurement
// ---------------------------------------------------------------------------

/// Get current process RSS in MB via the `ps` command.
fn get_rss_mb() -> f64 {
    let pid = std::process::id();
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output();
    match output {
        Ok(o) => {
            let rss_str = String::from_utf8_lossy(&o.stdout).trim().to_string();
            rss_str.parse::<f64>().unwrap_or(0.0) / 1024.0
        }
        Err(_) => 0.0,
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 || args[1] == "help" || args[1] == "--help" {
        eprintln!("arbiter-cli — smoke tests and benchmarks");
        eprintln!();
        eprintln!("USAGE:");
        eprintln!("  arbiter-cli bench   Run all benchmarks");
        eprintln!("  arbiter-cli help    Print this help");
        return;
    }

    if args[1] == "bench" {
        eprintln!("=== Arbiter Benchmarks ===\n");

        #[allow(clippy::type_complexity)]
        let benchmarks: Vec<(&str, fn() -> Result<()>)> = vec![
            ("BT-01: Route throughput", bench_route_throughput),
            ("BT-02: Route latency p99", bench_route_latency_p99),
            ("BT-03: Report latency p99", bench_report_latency_p99),
            ("BT-04: Memory usage", bench_memory_usage),
            ("BT-05: SQLite size", bench_sqlite_size),
        ];

        let mut passed = 0;
        let mut failed = 0;

        for (name, bench_fn) in &benchmarks {
            match bench_fn() {
                Ok(()) => {
                    passed += 1;
                    eprintln!("  [{name}] PASSED\n");
                }
                Err(e) => {
                    failed += 1;
                    eprintln!("  [{name}] FAILED: {e}\n");
                }
            }
        }

        eprintln!("=== Results: {passed} passed, {failed} failed ===");

        if failed > 0 {
            std::process::exit(1);
        }
    } else {
        eprintln!("Unknown command: {}", args[1]);
        eprintln!("Run 'arbiter-cli help' for usage.");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// Tests (benchmark assertions as tests)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bt_01_route_throughput() {
        bench_route_throughput().unwrap();
    }

    #[test]
    fn bt_02_route_latency_p99() {
        bench_route_latency_p99().unwrap();
    }

    #[test]
    fn bt_03_report_latency_p99() {
        bench_report_latency_p99().unwrap();
    }

    #[test]
    fn bt_04_memory_usage() {
        bench_memory_usage().unwrap();
    }

    #[test]
    fn bt_05_sqlite_size() {
        bench_sqlite_size().unwrap();
    }
}
