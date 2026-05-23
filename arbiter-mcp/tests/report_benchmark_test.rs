use arbiter_mcp::db::Database;
use arbiter_mcp::tools::report_benchmark;
use serde_json::json;

fn valid_payload(run_id: &str) -> serde_json::Value {
    json!({
        "payload_version": "1.0.0",
        "run_id": run_id,
        "benchmark_id": "swe-mini",
        "agent_id": "claude_code",
        "ts": "2026-05-23T12:00:00Z",
        "score": 0.85,
        "score_components": {"accuracy": 0.85},
        "total_tokens": 12345,
        "total_cost_usd": 0.12,
        "duration_seconds": 42.0,
        "per_task": [{
            "task_index": 0,
            "task_type": "bugfix",
            "score": 1.0,
            "tokens_used": 1234,
            "duration_seconds": 4.2,
            "error_class": null
        }],
        "per_task_total_count": 1,
        "per_task_truncated": false
    })
}

fn fresh_db() -> Database {
    let db = Database::open_in_memory().expect("open db");
    db.migrate().expect("migrate");
    db
}

#[test]
fn happy_path_returns_created() {
    let db = fresh_db();
    let result = report_benchmark::execute(&valid_payload("run-1"), &db).expect("execute");
    assert_eq!(result["status"], "created");
    assert_eq!(result["run_id"], "run-1");
}
