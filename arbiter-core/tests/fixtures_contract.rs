//! Sanity: every fixture jsonl record matches log-schema.json.
//!
//! If this test fails, either the schema or a fixture drifted — the contract
//! is the shared artefact for Python and Rust emitters, so neither is allowed
//! to drift unilaterally.

use std::fs;
use std::path::PathBuf;

fn contract_dir() -> PathBuf {
    // Canonical location: Maestro/_cowork_output/observability-contract/
    // (same tree as the design doc at Maestro/_cowork_output/decisions/).
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("arbiter-core must sit two dirs under the monorepo root")
        .join("Maestro/_cowork_output/observability-contract")
}

fn load_schema() -> jsonschema::JSONSchema {
    let schema_path = contract_dir().join("log-schema.json");
    let raw = fs::read_to_string(&schema_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", schema_path.display()));
    let schema_json: serde_json::Value =
        serde_json::from_str(&raw).expect("schema must be valid JSON");
    jsonschema::JSONSchema::compile(&schema_json).expect("schema must compile")
}

#[test]
fn every_fixture_record_matches_schema() {
    let schema = load_schema();
    let fixtures_dir = contract_dir().join("fixtures");
    let mut seen = 0usize;

    for entry in fs::read_dir(&fixtures_dir).expect("fixtures dir readable") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let content = fs::read_to_string(&path).expect("fixture readable");
        for (i, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let record: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("{}:{}: bad JSON: {e}", path.display(), i + 1));
            if let Err(errors) = schema.validate(&record) {
                let msgs: Vec<String> = errors
                    .map(|e| format!("  at {}: {}", e.instance_path, e))
                    .collect();
                panic!(
                    "{}:{} does not match log-schema.json:\n{}\n--record:\n{}",
                    path.display(),
                    i + 1,
                    msgs.join("\n"),
                    line
                );
            }
            seen += 1;
        }
    }

    assert!(seen > 0, "no fixture records found");
}

#[test]
fn nested_fixture_parent_span_link_is_consistent() {
    let path = contract_dir().join("fixtures/nested-span.jsonl");
    let content = fs::read_to_string(&path).expect("nested fixture readable");
    let records: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("valid json"))
        .collect();

    assert_eq!(records.len(), 2, "nested-span.jsonl must have two records");
    assert_eq!(
        records[0]["TraceId"], records[1]["TraceId"],
        "nested-span records must share TraceId"
    );
    assert_eq!(
        records[1]["Attributes"]["parent_span_id"], records[0]["SpanId"],
        "child's parent_span_id must equal parent's SpanId"
    );
}
