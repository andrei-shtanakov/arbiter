//! Per-request trace binding (M3-obs, maestro#88 counterpart).
//!
//! `bind_request_trace` must make records emitted while the guard is held
//! carry the caller's TraceId (and parent_span_id for root spans), and
//! restore the process root context once dropped.
//!
//! Own test binary (one file per binary in `tests/`) because `init_logging`
//! installs a global tracing subscriber.

use std::fs;

use serde_json::Value;

const CALLER_TRACE: &str = "aaaabbbbccccddddeeeeffff00001111";
const CALLER_SPAN: &str = "1234abcd5678ef90";

fn read_records(log_dir: &std::path::Path) -> Vec<Value> {
    let entry = fs::read_dir(log_dir)
        .expect("log dir readable")
        .filter_map(Result::ok)
        .find(|e| e.file_name().to_string_lossy().starts_with("arbiter-"))
        .expect("jsonl file present");
    fs::read_to_string(entry.path())
        .expect("jsonl readable")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("valid json line"))
        .collect()
}

fn find_by_event<'a>(records: &'a [Value], event: &str) -> &'a Value {
    records
        .iter()
        .find(|r| r["Attributes"]["event"] == event)
        .unwrap_or_else(|| panic!("record with event={event} not found"))
}

#[test]
fn request_trace_binding_overrides_root_until_dropped() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    std::env::set_var("ORCHESTRA_LOG_DIR", tmp.path());
    std::env::remove_var("TRACEPARENT"); // random process-root trace

    arbiter_core::obs::init_logging("arbiter").expect("init_logging succeeds");

    // Malformed traceparent must not bind.
    assert!(arbiter_core::obs::bind_request_trace("garbage").is_none());
    assert!(
        arbiter_core::obs::bind_request_trace(&format!("00-{}-{}-01", "0".repeat(32), CALLER_SPAN))
            .is_none(),
        "all-zero trace id must be rejected"
    );

    {
        let _guard =
            arbiter_core::obs::bind_request_trace(&format!("00-{CALLER_TRACE}-{CALLER_SPAN}-01"))
                .expect("valid traceparent binds");

        tracing::info!(event = "bound.plain", "plain event under request trace");

        let span = tracing::info_span!("request.handling", module = "server");
        let _e = span.enter();
        tracing::info!(event = "bound.spanned", "spanned event under request trace");
    }

    tracing::info!(event = "unbound.after", "event after guard dropped");

    let records = read_records(tmp.path());

    let plain = find_by_event(&records, "bound.plain");
    assert_eq!(plain["TraceId"], CALLER_TRACE);
    assert_eq!(plain["Attributes"]["parent_span_id"], CALLER_SPAN);

    let spanned = find_by_event(&records, "bound.spanned");
    assert_eq!(spanned["TraceId"], CALLER_TRACE);

    // The root span opened under the guard has the caller's span as parent.
    let span_started = records
        .iter()
        .find(|r| r["Attributes"]["event"] == "request.handling.started")
        .expect("span start record present");
    assert_eq!(span_started["TraceId"], CALLER_TRACE);
    assert_eq!(span_started["Attributes"]["parent_span_id"], CALLER_SPAN);

    let after = find_by_event(&records, "unbound.after");
    assert_ne!(after["TraceId"], CALLER_TRACE, "root trace must return");
    assert_ne!(after["TraceId"], "0".repeat(32));
}
