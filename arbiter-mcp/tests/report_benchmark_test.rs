use arbiter_mcp::db::Database;
use arbiter_mcp::tools::report_benchmark;
use arbiter_mcp::tools::report_benchmark::ReportBenchmarkError;
use serde_json::json;

fn valid_payload(run_id: &str) -> serde_json::Value {
    json!({
        "payload_version": "1.0.0",
        "run_id": run_id,
        "benchmark_id": "swe-mini",
        "agent_id": "claude_code@claude-sonnet-4-6",
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

#[test]
fn duplicate_run_id_returns_duplicate() {
    let db = fresh_db();
    let payload = valid_payload("run-dup");

    let r1 = report_benchmark::execute(&payload, &db).expect("first insert");
    let r2 = report_benchmark::execute(&payload, &db).expect("second insert");

    assert_eq!(r1["status"], "created");
    assert_eq!(r2["status"], "duplicate");
    assert_eq!(r1["run_id"], "run-dup");
    assert_eq!(r2["run_id"], "run-dup");
}

/// Two parallel writers with the same run_id must produce exactly one
/// `"created"` + one `"duplicate"`, and leave exactly one row in
/// `benchmark_runs`.  Validates ON CONFLICT atomicity under contention
/// (SQLite is the contract being tested).
#[test]
fn concurrent_duplicate_run_id_one_created_one_duplicate() {
    use std::sync::Arc;

    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("bench-concurrent.db");

    // Bootstrap the schema via a dedicated connection, then drop it so
    // both worker threads start with a clean file lock.
    {
        let db = Database::open(&db_path).expect("open bootstrap db");
        db.migrate().expect("migrate");
    }

    // Wrap the shared path in an Arc so each thread gets its own clone.
    let path = Arc::new(db_path);

    let path1 = Arc::clone(&path);
    let path2 = Arc::clone(&path);

    let payload1 = valid_payload("run-conc");
    let payload2 = valid_payload("run-conc");

    let t1 = std::thread::spawn(move || {
        let db = Database::open(&path1).expect("open in t1");
        report_benchmark::execute(&payload1, &db).expect("execute t1")
    });
    let t2 = std::thread::spawn(move || {
        let db = Database::open(&path2).expect("open in t2");
        report_benchmark::execute(&payload2, &db).expect("execute t2")
    });

    let r1 = t1.join().expect("t1 panicked");
    let r2 = t2.join().expect("t2 panicked");

    // Exactly one created and one duplicate — order is non-deterministic.
    let mut statuses = vec![
        r1["status"].as_str().expect("status str").to_string(),
        r2["status"].as_str().expect("status str").to_string(),
    ];
    statuses.sort();
    assert_eq!(
        statuses,
        vec!["created", "duplicate"],
        "expected one created + one duplicate, got: {statuses:?}"
    );

    // Exactly one row in the table.
    let db = Database::open(&path).expect("open verify db");
    let count = db
        .count_benchmark_runs("run-conc")
        .expect("count_benchmark_runs");
    assert_eq!(count, 1, "exactly one row after concurrent inserts");
}

#[test]
fn missing_required_field_returns_error() {
    let db = fresh_db();
    let mut payload = valid_payload("run-err");
    // Remove a required field
    payload.as_object_mut().unwrap().remove("agent_id");

    let result = report_benchmark::execute(&payload, &db);
    assert!(result.is_err(), "execute should fail with missing agent_id");

    // Verify no INSERT happened
    let count = db.count_benchmark_runs("run-err").expect("count");
    assert_eq!(count, 0, "no row should be inserted on validation failure");
}

#[test]
fn unsupported_payload_version_rejected() {
    let db = fresh_db();
    let mut payload = valid_payload("run-pv");
    payload["payload_version"] = serde_json::json!("2.0.0");

    let result = report_benchmark::execute(&payload, &db);
    assert!(
        result.is_err(),
        "execute should fail on unsupported payload_version"
    );
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("unsupported payload_version"),
        "error message should mention 'unsupported payload_version', got: {}",
        err_msg
    );

    let count = db.count_benchmark_runs("run-pv").expect("count");
    assert_eq!(count, 0);
}

// ---------------------------------------------------------------------------
// Fix 1: validation errors map to -32602, runtime enum maps to -32000
// ---------------------------------------------------------------------------

#[test]
fn validation_error_uses_jsonrpc_invalid_params() {
    let db = fresh_db();
    let mut payload = valid_payload("run-val-code");
    payload.as_object_mut().unwrap().remove("run_id");

    let err = report_benchmark::execute(&payload, &db).unwrap_err();
    assert_eq!(
        err.jsonrpc_code(),
        -32602,
        "missing field should map to INVALID_PARAMS (-32602)"
    );
    assert!(matches!(err, ReportBenchmarkError::Validation(_)));
}

#[test]
fn runtime_error_variant_returns_server_error_code() {
    // Test the enum directly — proves the dispatch logic is correct.
    let err = ReportBenchmarkError::Runtime("simulated db failure".into());
    assert_eq!(
        err.jsonrpc_code(),
        -32000,
        "Runtime error should map to server error (-32000)"
    );
}

// ---------------------------------------------------------------------------
// Fix 3+4: validate score_components is object, per_task is array
// ---------------------------------------------------------------------------

#[test]
fn score_components_non_object_rejected() {
    let db = fresh_db();
    let mut payload = valid_payload("run-sc-type");
    payload["score_components"] = json!("not an object");

    let err = report_benchmark::execute(&payload, &db).unwrap_err();
    assert!(
        matches!(err, ReportBenchmarkError::Validation(_)),
        "expected Validation error, got {err:?}"
    );
    assert!(
        format!("{err}").contains("score_components"),
        "error should mention score_components, got: {err}"
    );
}

#[test]
fn per_task_non_array_rejected() {
    let db = fresh_db();
    let mut payload = valid_payload("run-pt-type");
    payload["per_task"] = json!("not an array");

    let err = report_benchmark::execute(&payload, &db).unwrap_err();
    assert!(
        matches!(err, ReportBenchmarkError::Validation(_)),
        "expected Validation error, got {err:?}"
    );
    assert!(
        format!("{err}").contains("per_task"),
        "error should mention per_task, got: {err}"
    );
}

#[test]
fn missing_score_components_rejected() {
    let db = fresh_db();
    let mut payload = valid_payload("run-sc-missing");
    payload.as_object_mut().unwrap().remove("score_components");

    let err = report_benchmark::execute(&payload, &db).unwrap_err();
    assert!(matches!(err, ReportBenchmarkError::Validation(_)));
}

#[test]
fn missing_per_task_rejected() {
    let db = fresh_db();
    let mut payload = valid_payload("run-pt-missing");
    payload.as_object_mut().unwrap().remove("per_task");

    let err = report_benchmark::execute(&payload, &db).unwrap_err();
    assert!(matches!(err, ReportBenchmarkError::Validation(_)));
}

// ---------------------------------------------------------------------------
// Fix 5: non-empty IDs + RFC3339 ts validation
// ---------------------------------------------------------------------------

#[test]
fn empty_run_id_rejected() {
    let db = fresh_db();
    let mut payload = valid_payload("run-empty-rid");
    payload["run_id"] = json!("");

    let err = report_benchmark::execute(&payload, &db).unwrap_err();
    assert!(
        matches!(err, ReportBenchmarkError::Validation(_)),
        "expected Validation error for empty run_id"
    );
}

#[test]
fn empty_benchmark_id_rejected() {
    let db = fresh_db();
    let mut payload = valid_payload("run-empty-bid");
    payload["benchmark_id"] = json!("");

    let err = report_benchmark::execute(&payload, &db).unwrap_err();
    assert!(matches!(err, ReportBenchmarkError::Validation(_)));
}

#[test]
fn empty_agent_id_rejected() {
    let db = fresh_db();
    let mut payload = valid_payload("run-empty-aid");
    payload["agent_id"] = json!("");

    let err = report_benchmark::execute(&payload, &db).unwrap_err();
    assert!(matches!(err, ReportBenchmarkError::Validation(_)));
}

#[test]
fn bad_ts_format_rejected() {
    let db = fresh_db();
    let mut payload = valid_payload("run-bad-ts");
    payload["ts"] = json!("not-a-date");

    let err = report_benchmark::execute(&payload, &db).unwrap_err();
    assert!(
        matches!(err, ReportBenchmarkError::Validation(_)),
        "expected Validation error for bad ts format"
    );
    assert!(
        format!("{err}").contains("RFC3339") || format!("{err}").contains("not RFC3339"),
        "error should mention RFC3339, got: {err}"
    );
}

#[test]
fn good_ts_rfc3339_accepted() {
    let db = fresh_db();
    // Valid RFC3339 with timezone offset
    let mut payload = valid_payload("run-good-ts");
    payload["ts"] = json!("2026-05-23T12:00:00+00:00");

    let result = report_benchmark::execute(&payload, &db).expect("should succeed");
    assert_eq!(result["status"], "created");
}

// ---------------------------------------------------------------------------
// score unit: a fraction in [0,1], never a percent (inbox #81)
// ---------------------------------------------------------------------------

/// The field had two producers writing different quantities into it — a pass
/// rate in [0,1] and ATP's `total_score` in percent — and the consumer clamped,
/// so every percent above 1% arrived as an indistinguishable perfect `1.0`.
/// Rejecting at ingest is what makes the unit checkable instead of assumed.
#[test]
fn percent_score_is_rejected_as_a_validation_error() {
    let db = fresh_db();
    let mut payload = valid_payload("run-percent");
    payload["score"] = json!(66.66666666666667);

    let err = report_benchmark::execute(&payload, &db).expect_err("must reject a percent");
    assert!(matches!(err, ReportBenchmarkError::Validation(_)));
    assert_eq!(
        err.jsonrpc_code(),
        -32602,
        "a unit error is a contract break the producer must fix, not a retryable one"
    );
    assert!(
        err.to_string().contains("fraction in [0,1]"),
        "the message must name the canonical unit: {err}"
    );
    assert_eq!(db.count_benchmark_runs("run-percent").unwrap(), 0);
}

#[test]
fn score_outside_the_unit_range_is_rejected() {
    for bad in [1.0000001_f64, -0.1, f64::NAN, f64::INFINITY] {
        let db = fresh_db();
        let mut payload = valid_payload("run-bad");
        payload["score"] = json!(bad);
        let err =
            report_benchmark::execute(&payload, &db).expect_err("must reject score outside [0,1]");
        assert!(matches!(err, ReportBenchmarkError::Validation(_)), "{bad}");
    }
}

#[test]
fn score_at_the_range_boundaries_is_accepted() {
    for (run_id, good) in [("run-zero", 0.0), ("run-one", 1.0)] {
        let db = fresh_db();
        let mut payload = valid_payload(run_id);
        payload["score"] = json!(good);
        let result = report_benchmark::execute(&payload, &db).expect("boundary is valid");
        assert_eq!(result["status"], "created");
    }
}

// ---------------------------------------------------------------------------
// score_semantics: accepted, stored, honoured (inbox #82)
// ---------------------------------------------------------------------------

#[test]
fn score_semantics_is_optional() {
    // Every producer predating ATP's contract omits it, and the field must not
    // become required by being read.
    let db = fresh_db();
    let payload = valid_payload("run-legacy");
    assert!(payload.get("score_semantics").is_none());
    let result = report_benchmark::execute(&payload, &db).expect("execute");
    assert_eq!(result["status"], "created");
}

#[test]
fn graded_run_is_stored_and_feeds_routing() {
    let db = fresh_db();
    let mut payload = valid_payload("run-graded");
    payload["score_semantics"] = json!({
        "schema_version": 1,
        "kind": "aggregated_evaluation",
        "quality_signal": true,
        "coverage": {"tasks_evaluated": 1, "tasks_completion_only": 0}
    });
    report_benchmark::execute(&payload, &db).expect("execute");
    assert_eq!(
        db.get_benchmark_score("claude_code@claude-sonnet-4-6", "swe-mini")
            .unwrap(),
        Some(0.85)
    );
}

/// The ask in one assertion: a completion rate is stored (the run happened and
/// stays inspectable) but never compared against a graded score as if the two
/// were the same quantity.
#[test]
fn completion_rate_run_is_stored_but_withheld_from_routing() {
    let db = fresh_db();
    let mut payload = valid_payload("run-completion");
    payload["score_semantics"] = json!({
        "schema_version": 1,
        "kind": "completion_rate",
        "quality_signal": false
    });
    let result = report_benchmark::execute(&payload, &db).expect("execute");

    assert_eq!(
        result["status"], "created",
        "an ungraded run is still recorded"
    );
    assert_eq!(db.count_benchmark_runs("run-completion").unwrap(), 1);
    assert_eq!(
        db.get_benchmark_score("claude_code@claude-sonnet-4-6", "swe-mini")
            .unwrap(),
        None,
        "completion is not quality — it must not act as a tiebreaker"
    );
}

#[test]
fn unknown_keys_inside_score_semantics_are_accepted() {
    // The contract says consumers must ignore unknown keys; that is what keeps
    // additions to the block additive rather than a lockstep version bump.
    let db = fresh_db();
    let mut payload = valid_payload("run-forward");
    payload["score_semantics"] = json!({
        "schema_version": 1,
        "quality_signal": true,
        "some_future_key": "consumers must ignore this"
    });
    let result = report_benchmark::execute(&payload, &db).expect("execute");
    assert_eq!(result["status"], "created");
    assert_eq!(
        db.get_benchmark_score("claude_code@claude-sonnet-4-6", "swe-mini")
            .unwrap(),
        Some(0.85)
    );
}

#[test]
fn non_object_score_semantics_is_a_validation_error() {
    let db = fresh_db();
    let mut payload = valid_payload("run-bad-semantics");
    payload["score_semantics"] = json!("aggregated_evaluation");
    let err = report_benchmark::execute(&payload, &db).expect_err("must reject");
    assert_eq!(err.jsonrpc_code(), -32602);
}

#[test]
fn null_score_semantics_is_treated_as_absent() {
    let db = fresh_db();
    let mut payload = valid_payload("run-null-semantics");
    payload["score_semantics"] = json!(null);
    let result = report_benchmark::execute(&payload, &db).expect("execute");
    assert_eq!(result["status"], "created");
}
