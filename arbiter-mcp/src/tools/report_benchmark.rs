//! report_benchmark MCP tool — R-06b M4.

use crate::db::Database;
use anyhow::{bail, Result};
use serde_json::Value;

/// Execute the report_benchmark logic.
///
/// Validates required fields, checks payload_version, and inserts a row
/// into `benchmark_runs` (ON CONFLICT DO NOTHING for idempotency).
/// Returns `{"status": "created"|"duplicate", "run_id": "<run_id>"}`.
pub fn execute(args: &Value, db: &Database) -> Result<Value> {
    let payload_version = args["payload_version"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("payload_version required"))?;
    if payload_version != "1.0.0" {
        bail!("unsupported payload_version: {}", payload_version);
    }

    let run_id = args["run_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("run_id required"))?;
    let benchmark_id = args["benchmark_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("benchmark_id required"))?;
    let agent_id = args["agent_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("agent_id required"))?;
    let ts = args["ts"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("ts required"))?;
    let score = args["score"]
        .as_f64()
        .ok_or_else(|| anyhow::anyhow!("score required"))?;
    let score_components = serde_json::to_string(&args["score_components"])?;
    let total_tokens = args["total_tokens"].as_i64();
    let total_cost_usd = args["total_cost_usd"].as_f64();
    let duration_seconds = args["duration_seconds"]
        .as_f64()
        .ok_or_else(|| anyhow::anyhow!("duration_seconds required"))?;
    let per_task = serde_json::to_string(&args["per_task"])?;
    let per_task_total_count = args["per_task_total_count"]
        .as_i64()
        .ok_or_else(|| anyhow::anyhow!("per_task_total_count required"))?;
    let per_task_truncated = args["per_task_truncated"]
        .as_bool()
        .ok_or_else(|| anyhow::anyhow!("per_task_truncated required"))? as i64;

    let status = db.insert_benchmark_run(
        run_id,
        payload_version,
        benchmark_id,
        agent_id,
        ts,
        score,
        &score_components,
        total_tokens,
        total_cost_usd,
        duration_seconds,
        &per_task,
        per_task_total_count,
        per_task_truncated,
    )?;

    Ok(serde_json::json!({"status": status, "run_id": run_id}))
}
