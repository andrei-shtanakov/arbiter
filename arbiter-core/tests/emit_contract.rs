//! End-to-end: `init_logging` → tracing events → `<service>-<pid>.jsonl`
//! that matches the shared contract schema.
//!
//! Runs in its own test binary (one file per binary in `tests/`), so the
//! global tracing subscriber set by `init_logging` does not collide with the
//! fixture-validation test.

use std::fs;
use std::path::PathBuf;

use serde_json::Value;

fn contract_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("arbiter-core must sit two dirs under the monorepo root")
        .join("Maestro/_cowork_output/observability-contract")
}

fn load_schema() -> jsonschema::JSONSchema {
    let schema_path = contract_dir().join("log-schema.json");
    let raw = fs::read_to_string(&schema_path).expect("schema readable");
    let schema_json: Value = serde_json::from_str(&raw).expect("schema valid json");
    jsonschema::JSONSchema::compile(&schema_json).expect("schema compiles")
}

const TRACE_ID: &str = "3f2e8c1a9b7d450f6e2c8a1b9f4d730e";
const PARENT_SPAN_ID: &str = "9f2e4a1b6c0d3387";
const PIPELINE_ID: &str = "01HZKX3P9M7Q2VFGR8BNDAW5YT";

#[test]
fn init_logging_emits_contract_compliant_jsonl() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let log_dir = tmp.path().to_path_buf();

    std::env::set_var("ORCHESTRA_LOG_DIR", &log_dir);
    std::env::set_var("TRACEPARENT", format!("00-{TRACE_ID}-{PARENT_SPAN_ID}-01"));
    std::env::set_var("ORCHESTRA_PIPELINE_ID", PIPELINE_ID);

    arbiter_core::obs::init_logging("arbiter").expect("init_logging succeeds");

    let child_env_snapshot;
    {
        let span = tracing::info_span!("spec.verify", task_id = "T-042", module = "execution");
        let _g = span.enter();
        tracing::info!(
            event = "check.started",
            check_type = "invariant",
            "Running invariant check"
        );
        tracing::warn!(
            event = "auth.attempt",
            api_key = "super-secret-value",
            password = "hunter2",
            user = "alice",
            "Auth attempt"
        );
        child_env_snapshot = arbiter_core::obs::child_env();
    }

    let files: Vec<PathBuf> = fs::read_dir(&log_dir)
        .expect("log dir readable")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .collect();
    assert_eq!(
        files.len(),
        1,
        "expected exactly one .jsonl file under {}: got {:?}",
        log_dir.display(),
        files
    );

    let expected_filename = format!("arbiter-{}.jsonl", std::process::id());
    assert_eq!(
        files[0].file_name().and_then(|s| s.to_str()),
        Some(expected_filename.as_str()),
        "sink filename must be <service>-<pid>.jsonl"
    );

    let raw = fs::read_to_string(&files[0]).expect("sink readable");
    let records: Vec<Value> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).expect("jsonl line parses"))
        .collect();
    assert!(!records.is_empty(), "sink must contain at least one record");

    let schema = load_schema();
    for (i, r) in records.iter().enumerate() {
        if let Err(errors) = schema.validate(r) {
            let msgs: Vec<String> = errors
                .map(|e| format!("  at {}: {}", e.instance_path, e))
                .collect();
            panic!(
                "record {i} does not match schema:\n{}\n--record:\n{}",
                msgs.join("\n"),
                serde_json::to_string_pretty(r).unwrap()
            );
        }

        assert_eq!(
            r["TraceId"], TRACE_ID,
            "record {i}: TraceId must propagate from TRACEPARENT"
        );
        assert_eq!(r["Resource"]["service.name"], "arbiter");
        assert_eq!(r["Attributes"]["pipeline_id"], PIPELINE_ID);
        assert_eq!(r["TraceFlags"], "01");
    }

    let check_started = records
        .iter()
        .find(|r| r["Attributes"]["event"] == "check.started")
        .expect("check.started event emitted");
    assert_eq!(check_started["Body"], "Running invariant check");
    assert_eq!(check_started["SeverityText"], "INFO");
    assert_eq!(
        check_started["Attributes"]["parent_span_id"], PARENT_SPAN_ID,
        "event inside the local root span inherits TRACEPARENT's span as parent"
    );
    assert_eq!(
        check_started["Attributes"]["task_id"], "T-042",
        "span-level attrs propagate into events"
    );
    assert_eq!(check_started["Attributes"]["module"], "execution");
    assert_eq!(
        check_started["Attributes"]["check_type"], "invariant",
        "event-level attrs land in Attributes"
    );

    let span_start = records
        .iter()
        .find(|r| r["Attributes"]["event"] == "spec.verify.started")
        .expect("span creation emits <name>.started per Python convention");
    assert_eq!(
        span_start["Attributes"]["parent_span_id"], PARENT_SPAN_ID,
        "local-root span's parent_span_id = TRACEPARENT span_id"
    );
    assert_ne!(
        span_start["SpanId"], PARENT_SPAN_ID,
        "local-root span must have its own fresh SpanId"
    );
    assert_eq!(
        span_start["SpanId"], check_started["SpanId"],
        "events inside a span share the span's SpanId"
    );

    let span_end = records
        .iter()
        .find(|r| r["Attributes"]["event"] == "spec.verify.ended")
        .expect("span close emits <name>.ended");
    assert_eq!(
        span_end["SpanId"], span_start["SpanId"],
        "span-end event shares the span's SpanId with span-start"
    );
    assert_eq!(
        span_end["Attributes"]["task_id"], "T-042",
        "span-end inherits span-level attrs"
    );

    let auth = records
        .iter()
        .find(|r| r["Attributes"]["event"] == "auth.attempt")
        .expect("auth.attempt event emitted");
    assert_eq!(auth["SeverityText"], "WARN");
    assert_eq!(
        auth["Attributes"]["api_key"], "<redacted>",
        "api_key is redacted by default"
    );
    assert_eq!(
        auth["Attributes"]["password"], "<redacted>",
        "password is redacted by default"
    );
    assert_eq!(
        auth["Attributes"]["user"], "alice",
        "non-sensitive attrs are not redacted"
    );

    let tp = child_env_snapshot
        .get("TRACEPARENT")
        .expect("child_env exposes TRACEPARENT");
    assert!(
        tp.starts_with(&format!("00-{TRACE_ID}-")),
        "child TRACEPARENT keeps parent's trace_id, got {tp}"
    );
    assert!(
        tp.ends_with("-01"),
        "child TRACEPARENT ends with sampled flag, got {tp}"
    );
    let tp_parts: Vec<&str> = tp.split('-').collect();
    assert_eq!(tp_parts.len(), 4, "TRACEPARENT has 4 segments");
    assert_eq!(tp_parts[2].len(), 16, "child-side span_id is 16 hex");
    assert_ne!(
        tp_parts[2], PARENT_SPAN_ID,
        "child_env propagates the current local span, not TRACEPARENT's parent span"
    );
    assert_eq!(
        child_env_snapshot
            .get("ORCHESTRA_PIPELINE_ID")
            .map(String::as_str),
        Some(PIPELINE_ID)
    );
    assert!(
        child_env_snapshot.contains_key("ORCHESTRA_LOG_DIR"),
        "child_env includes ORCHESTRA_LOG_DIR"
    );
}
