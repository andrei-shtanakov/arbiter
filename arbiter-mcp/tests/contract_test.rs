use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

fn schema_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("contract")
        .join("report_benchmark-v1.schema.json")
}

fn validator() -> jsonschema::Validator {
    let raw = fs::read_to_string(schema_path()).expect("read schema file");
    let schema: Value = serde_json::from_str(&raw).expect("parse schema JSON");
    jsonschema::draft202012::options()
        .build(&schema)
        .expect("compile schema")
}

#[test]
fn valid_request_passes_schema() {
    let payload = json!({
        "payload_version": "1.0.0",
        "run_id": "r1",
        "benchmark_id": "b",
        "agent_id": "a",
        "ts": "2026-05-23T12:00:00Z",
        "score": 0.5,
        "score_components": {},
        "duration_seconds": 1.0,
        "per_task": [],
        "per_task_total_count": 0,
        "per_task_truncated": false
    });
    let v = validator();
    let errors: Vec<_> = v
        .evaluate(&payload)
        .iter_errors()
        .map(|e| e.error.to_string())
        .collect();
    assert!(errors.is_empty(), "validation errors: {:?}", errors);
}

#[test]
fn missing_required_field_fails_schema() {
    let payload = json!({
        "payload_version": "1.0.0",
        "run_id": "r1"
    });
    let v = validator();
    let evaluation = v.evaluate(&payload);
    let errors: Vec<_> = evaluation.iter_errors().collect();
    assert!(
        !errors.is_empty(),
        "expected validation errors for missing required fields"
    );
}

#[test]
fn valid_response_created_passes_schema() {
    let resp = json!({"status": "created", "run_id": "r1"});
    let v = validator();
    let errors: Vec<_> = v
        .evaluate(&resp)
        .iter_errors()
        .map(|e| e.error.to_string())
        .collect();
    assert!(errors.is_empty(), "errors: {:?}", errors);
}

#[test]
fn valid_response_duplicate_passes_schema() {
    let resp = json!({"status": "duplicate", "run_id": "r1"});
    let v = validator();
    let errors: Vec<_> = v
        .evaluate(&resp)
        .iter_errors()
        .map(|e| e.error.to_string())
        .collect();
    assert!(errors.is_empty(), "errors: {:?}", errors);
}
