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

/// Require a non-empty string field from args.
fn require_non_empty<'a>(args: &'a Value, key: &str) -> Result<&'a str, ReportBenchmarkError> {
    let value = args
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| ReportBenchmarkError::Validation(format!("{key} required")))?;
    if value.is_empty() {
        return Err(ReportBenchmarkError::Validation(format!(
            "{key} must be non-empty"
        )));
    }
    Ok(value)
}

/// Require a non-empty string that parses as RFC3339.
fn require_rfc3339<'a>(args: &'a Value, key: &str) -> Result<&'a str, ReportBenchmarkError> {
    let value = require_non_empty(args, key)?;
    chrono::DateTime::parse_from_rfc3339(value)
        .map_err(|e| ReportBenchmarkError::Validation(format!("{key} not RFC3339: {e}")))?;
    Ok(value)
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

    // --- required non-empty ID fields ---
    let run_id = require_non_empty(args, "run_id")?;
    let benchmark_id = require_non_empty(args, "benchmark_id")?;
    let agent_id = require_non_empty(args, "agent_id")?;

    // --- ts: RFC3339 ---
    let ts = require_rfc3339(args, "ts")?;
    let score = args["score"]
        .as_f64()
        .ok_or_else(|| ReportBenchmarkError::Validation("score required".into()))?;
    // --- score_components: must be an object ---
    let score_components_val = &args["score_components"];
    if !score_components_val.is_object() {
        return Err(ReportBenchmarkError::Validation(
            "score_components must be an object".into(),
        ));
    }
    let score_components = serde_json::to_string(score_components_val)
        .map_err(|e| ReportBenchmarkError::Runtime(format!("serialize score_components: {e}")))?;
    let total_tokens = args["total_tokens"].as_i64();
    let total_cost_usd = args["total_cost_usd"].as_f64();
    let duration_seconds = args["duration_seconds"]
        .as_f64()
        .ok_or_else(|| ReportBenchmarkError::Validation("duration_seconds required".into()))?;
    // --- per_task: must be an array ---
    let per_task_val = &args["per_task"];
    if !per_task_val.is_array() {
        return Err(ReportBenchmarkError::Validation(
            "per_task must be an array".into(),
        ));
    }
    let per_task = serde_json::to_string(per_task_val)
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
