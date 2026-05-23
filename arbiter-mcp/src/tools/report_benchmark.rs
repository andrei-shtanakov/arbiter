//! report_benchmark MCP tool — R-06b M4.

use crate::db::{BenchmarkRunInput, Database};
use serde_json::Value;
use thiserror::Error;

/// Error type distinguishing validation failures from runtime/DB errors.
///
/// Validation → JSON-RPC -32602 (INVALID_PARAMS): classified by Maestro
/// as ArbiterContractError — hard contract break, no retry.
///
/// Runtime → JSON-RPC -32000 (server error): classified as ArbiterUnavailable
/// — transient, retryable.
#[derive(Debug, Error)]
pub enum ReportBenchmarkError {
    #[error("validation: {0}")]
    Validation(String),
    #[error("runtime: {0}")]
    Runtime(String),
}

impl ReportBenchmarkError {
    /// Return the JSON-RPC error code for this error.
    pub fn jsonrpc_code(&self) -> i32 {
        match self {
            Self::Validation(_) => -32602,
            Self::Runtime(_) => -32000,
        }
    }
}

/// Execute the report_benchmark logic.
///
/// Validates required fields, checks payload_version, and inserts a row
/// into `benchmark_runs` (ON CONFLICT DO NOTHING for idempotency).
/// Returns `{"status": "created"|"duplicate", "run_id": "<run_id>"}`.
pub fn execute(args: &Value, db: &Database) -> Result<Value, ReportBenchmarkError> {
    let payload_version = args["payload_version"]
        .as_str()
        .ok_or_else(|| ReportBenchmarkError::Validation("payload_version required".into()))?;
    if payload_version != "1.0.0" {
        return Err(ReportBenchmarkError::Validation(format!(
            "unsupported payload_version: {payload_version}"
        )));
    }

    let run_id = args["run_id"]
        .as_str()
        .ok_or_else(|| ReportBenchmarkError::Validation("run_id required".into()))?;
    let benchmark_id = args["benchmark_id"]
        .as_str()
        .ok_or_else(|| ReportBenchmarkError::Validation("benchmark_id required".into()))?;
    let agent_id = args["agent_id"]
        .as_str()
        .ok_or_else(|| ReportBenchmarkError::Validation("agent_id required".into()))?;
    let ts = args["ts"]
        .as_str()
        .ok_or_else(|| ReportBenchmarkError::Validation("ts required".into()))?;
    let score = args["score"]
        .as_f64()
        .ok_or_else(|| ReportBenchmarkError::Validation("score required".into()))?;
    let score_components = serde_json::to_string(&args["score_components"])
        .map_err(|e| ReportBenchmarkError::Runtime(format!("serialize score_components: {e}")))?;
    let total_tokens = args["total_tokens"].as_i64();
    let total_cost_usd = args["total_cost_usd"].as_f64();
    let duration_seconds = args["duration_seconds"]
        .as_f64()
        .ok_or_else(|| ReportBenchmarkError::Validation("duration_seconds required".into()))?;
    let per_task = serde_json::to_string(&args["per_task"])
        .map_err(|e| ReportBenchmarkError::Runtime(format!("serialize per_task: {e}")))?;
    let per_task_total_count = args["per_task_total_count"]
        .as_i64()
        .ok_or_else(|| ReportBenchmarkError::Validation("per_task_total_count required".into()))?;
    let per_task_truncated = args["per_task_truncated"]
        .as_bool()
        .ok_or_else(|| ReportBenchmarkError::Validation("per_task_truncated required".into()))?
        as i64;

    let input = BenchmarkRunInput {
        run_id,
        payload_version,
        benchmark_id,
        agent_id,
        ts,
        score,
        score_components: &score_components,
        total_tokens,
        total_cost_usd,
        duration_seconds,
        per_task: &per_task,
        per_task_total_count,
        per_task_truncated,
    };
    let status = db
        .insert_benchmark_run(&input)
        .map_err(|e| ReportBenchmarkError::Runtime(format!("{e:#}")))?;

    Ok(serde_json::json!({"status": status, "run_id": run_id}))
}
