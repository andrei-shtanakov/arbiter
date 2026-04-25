//! Cross-project observability emitter (Rust side of the v1 contract).
//!
//! Implements `_cowork_output/observability-contract/`:
//!   - Emits OpenTelemetry Logs Data Model JSON (one record per line).
//!   - Propagates W3C Trace Context across subprocess boundaries via `TRACEPARENT`.
//!   - Writes to `$ORCHESTRA_LOG_DIR/<service>-<pid>.jsonl` (file-per-pid avoids
//!     `write(O_APPEND)` interleaving beyond `PIPE_BUF`).
//!   - Redacts a default blocklist of sensitive `Attributes` keys.
//!
//! ## Dependency footprint
//!
//! Design doc §5.3 lists `tracing-opentelemetry` + `opentelemetry_sdk` +
//! `opentelemetry-stdout`. For v1 (file-only, our Attributes extensions,
//! per-pid sink) the OTel SDK's processor/sampler/exporter pipeline goes
//! unused — we still need a custom `tracing-subscriber::Layer` to inject
//! `parent_span_id`, `pipeline_id`, `ts_iso`, and apply redaction. So this
//! module uses only `tracing-subscriber` + `chrono` + `rand`. Wire format
//! is identical to what `opentelemetry-stdout` would emit; the v2 upgrade
//! swaps this file for an SDK-backed impl without changing callers.

use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, TimeZone, Utc};
use rand::RngCore;
use serde_json::{json, Map, Value};
use tracing::field::{Field, Visit};
use tracing::span;
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::EnvFilter;

/// Keys whose values are replaced with `<redacted>` before JSON rendering.
pub const DEFAULT_REDACT_KEYS: &[&str] = &[
    "api_key",
    "token",
    "password",
    "secret",
    "authorization",
    "cookie",
    "private_key",
];

const REDACTED: &str = "<redacted>";

struct Root {
    trace_id: String,
    /// Parent span id extracted from `TRACEPARENT`; inherited by the first
    /// (local root) span in this process.
    incoming_parent_span_id: Option<String>,
    pipeline_id: String,
    log_dir: PathBuf,
    service_name: String,
    redact_keys: Vec<String>,
    writer: Mutex<File>,
}

static ROOT: OnceLock<Root> = OnceLock::new();

thread_local! {
    /// Stack of currently-entered span ids on this thread. Populated by the
    /// Layer's `on_enter` / `on_exit`; consumed by [`child_env`] to pin the
    /// subprocess's parent span.
    static CURRENT_SPAN_IDS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Initialise the observability emitter for this process.
///
/// Reads `TRACEPARENT`, `ORCHESTRA_PIPELINE_ID`, `ORCHESTRA_LOG_DIR`,
/// `ORCHESTRA_LOG_LEVEL`, `ORCHESTRA_REDACT_KEYS` from the environment.
/// Idempotent — second call is a no-op.
pub fn init_logging(project: &str) -> std::io::Result<()> {
    if ROOT.get().is_some() {
        return Ok(());
    }

    let (trace_id, incoming_parent_span_id) = match env::var("TRACEPARENT") {
        Ok(v) if !v.is_empty() => {
            parse_traceparent(&v).unwrap_or_else(|| (random_trace_id(), None))
        }
        _ => (random_trace_id(), None),
    };

    let pipeline_id = env::var("ORCHESTRA_PIPELINE_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(generate_ulid);

    let log_dir = env::var("ORCHESTRA_LOG_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("logs")
                .join(&pipeline_id)
        });
    std::fs::create_dir_all(&log_dir)?;

    let mut redact_keys: Vec<String> = DEFAULT_REDACT_KEYS.iter().map(|s| s.to_string()).collect();
    if let Ok(extra) = env::var("ORCHESTRA_REDACT_KEYS") {
        for k in extra.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            redact_keys.push(k.to_string());
        }
    }

    let sink_path = log_dir.join(format!("{}-{}.jsonl", project, process::id()));
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&sink_path)?;

    let root = Root {
        trace_id,
        incoming_parent_span_id,
        pipeline_id,
        log_dir,
        service_name: project.to_string(),
        redact_keys,
        writer: Mutex::new(file),
    };

    if ROOT.set(root).is_err() {
        return Ok(());
    }

    let level_str = env::var("ORCHESTRA_LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    let filter = EnvFilter::try_new(&level_str).unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(OrchestraLayer)
        .try_init()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::AlreadyExists, e.to_string()))?;

    Ok(())
}

/// Environment variables to inject into a spawned subprocess so it joins the
/// same pipeline trace.
///
/// The child's `TRACEPARENT` pins the currently-entered span as its parent.
/// Must be called from inside a span; outside any span the emitted parent is
/// the incoming `TRACEPARENT`'s span or all-zero (degraded but not invalid).
pub fn child_env() -> HashMap<String, String> {
    let mut m = HashMap::new();
    let Some(root) = ROOT.get() else {
        return m;
    };

    let span_id = CURRENT_SPAN_IDS
        .with(|c| c.borrow().last().cloned())
        .or_else(|| root.incoming_parent_span_id.clone())
        .unwrap_or_else(|| "0000000000000000".to_string());

    m.insert(
        "TRACEPARENT".into(),
        format!("00-{}-{}-01", root.trace_id, span_id),
    );
    m.insert("ORCHESTRA_PIPELINE_ID".into(), root.pipeline_id.clone());
    m.insert(
        "ORCHESTRA_LOG_DIR".into(),
        root.log_dir.to_string_lossy().into_owned(),
    );
    m
}

// --- internals -------------------------------------------------------------

#[derive(Clone)]
struct SpanData {
    trace_id: String,
    span_id: String,
    parent_span_id: Option<String>,
    pipeline_id: String,
    /// Span name (becomes `<name>.started` / `<name>.ended` events).
    name: String,
    /// Attrs accumulated from span creation (merged with ancestors). Copied
    /// into every event emitted inside this span, then overlaid with the
    /// event's own fields.
    attrs: Map<String, Value>,
}

struct OrchestraLayer;

impl<S> Layer<S> for OrchestraLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let Some(root) = ROOT.get() else {
            return;
        };
        let Some(span_ref) = ctx.span(id) else {
            return;
        };

        let parent_data: Option<SpanData> = span_ref.parent().and_then(|p| {
            let ext = p.extensions();
            ext.get::<SpanData>().cloned()
        });

        let (trace_id, parent_span_id) = match &parent_data {
            Some(pd) => (pd.trace_id.clone(), Some(pd.span_id.clone())),
            None => (root.trace_id.clone(), root.incoming_parent_span_id.clone()),
        };
        let pipeline_id = parent_data
            .as_ref()
            .map(|p| p.pipeline_id.clone())
            .unwrap_or_else(|| root.pipeline_id.clone());

        let new_span_id = random_span_id();

        let mut visitor = AttrVisitor::default();
        attrs.record(&mut visitor);
        visitor.fields.remove("message");

        let mut merged: Map<String, Value> = parent_data
            .as_ref()
            .map(|p| p.attrs.clone())
            .unwrap_or_default();
        for (k, v) in visitor.fields.clone() {
            merged.insert(k, v);
        }

        let name = span_ref.name().to_string();
        let data = SpanData {
            trace_id: trace_id.clone(),
            span_id: new_span_id.clone(),
            parent_span_id: parent_span_id.clone(),
            pipeline_id: pipeline_id.clone(),
            name: name.clone(),
            attrs: merged.clone(),
        };
        span_ref.extensions_mut().insert(data);

        let mut attrs_map = merged;
        let (event_name, body) = suffixed_event_name(&name, "started", &mut attrs_map);
        attrs_map.insert("event".into(), Value::String(event_name));
        attrs_map.insert("pipeline_id".into(), Value::String(pipeline_id));
        if let Some(p) = &parent_span_id {
            attrs_map.insert("parent_span_id".into(), Value::String(p.clone()));
        }
        redact_map(&mut attrs_map, &root.redact_keys);

        write_record(root, &trace_id, &new_span_id, "INFO", 9, &body, attrs_map);
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
        if let Some(span_ref) = ctx.span(id) {
            let ext = span_ref.extensions();
            if let Some(data) = ext.get::<SpanData>() {
                let span_id = data.span_id.clone();
                CURRENT_SPAN_IDS.with(|c| c.borrow_mut().push(span_id));
            }
        }
    }

    fn on_exit(&self, _id: &span::Id, _ctx: Context<'_, S>) {
        CURRENT_SPAN_IDS.with(|c| {
            c.borrow_mut().pop();
        });
    }

    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let Some(root) = ROOT.get() else {
            return;
        };
        let Some(span_ref) = ctx.span(&id) else {
            return;
        };
        let data: SpanData = {
            let ext = span_ref.extensions();
            let Some(d) = ext.get::<SpanData>() else {
                return;
            };
            d.clone()
        };

        let mut attrs_map = data.attrs;
        let (event_name, body) = suffixed_event_name(&data.name, "ended", &mut attrs_map);
        attrs_map.insert("event".into(), Value::String(event_name));
        attrs_map.insert("pipeline_id".into(), Value::String(data.pipeline_id));
        if let Some(p) = data.parent_span_id {
            attrs_map.insert("parent_span_id".into(), Value::String(p));
        }
        redact_map(&mut attrs_map, &root.redact_keys);

        write_record(
            root,
            &data.trace_id,
            &data.span_id,
            "INFO",
            9,
            &body,
            attrs_map,
        );
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let Some(root) = ROOT.get() else {
            return;
        };

        let span_data: Option<SpanData> = ctx.event_span(event).and_then(|s| {
            let ext = s.extensions();
            ext.get::<SpanData>().cloned()
        });

        let (trace_id, span_id, parent_span_id, pipeline_id, mut attrs_map) = match span_data {
            Some(sd) => (
                sd.trace_id,
                sd.span_id,
                sd.parent_span_id,
                sd.pipeline_id,
                sd.attrs,
            ),
            None => (
                root.trace_id.clone(),
                random_span_id(),
                root.incoming_parent_span_id.clone(),
                root.pipeline_id.clone(),
                Map::new(),
            ),
        };

        let mut visitor = AttrVisitor::default();
        event.record(&mut visitor);
        let body = visitor
            .fields
            .remove("message")
            .map(|v| match v {
                Value::String(s) => s,
                other => other.to_string(),
            })
            .unwrap_or_default();
        let explicit_event = visitor.fields.remove("event").and_then(|v| match v {
            Value::String(s) => Some(s),
            _ => None,
        });

        for (k, v) in visitor.fields {
            attrs_map.insert(k, v);
        }

        let event_name = explicit_event.unwrap_or_else(|| "log.emitted".to_string());
        let body = if body.is_empty() {
            event_name.clone()
        } else {
            body
        };

        attrs_map.insert("event".into(), Value::String(event_name));
        attrs_map.insert("pipeline_id".into(), Value::String(pipeline_id));
        if let Some(p) = parent_span_id {
            attrs_map.insert("parent_span_id".into(), Value::String(p));
        }
        redact_map(&mut attrs_map, &root.redact_keys);

        let (sev_text, sev_num) = level_to_severity(*event.metadata().level());
        write_record(
            root, &trace_id, &span_id, sev_text, sev_num, &body, attrs_map,
        );
    }
}

fn write_record(
    root: &Root,
    trace_id: &str,
    span_id: &str,
    severity_text: &str,
    severity_number: u8,
    body: &str,
    attributes: Map<String, Value>,
) {
    let (ts_ns, ts_iso) = now_timestamps();
    let record = json!({
        "Timestamp": ts_ns,
        "ts_iso": ts_iso,
        "SeverityText": severity_text,
        "SeverityNumber": severity_number,
        "TraceId": trace_id,
        "SpanId": span_id,
        "TraceFlags": "01",
        "Body": body,
        "Resource": {"service.name": root.service_name},
        "Attributes": attributes,
    });
    let Ok(line) = serde_json::to_string(&record) else {
        return;
    };
    if let Ok(mut w) = root.writer.lock() {
        let _ = writeln!(w, "{line}");
        let _ = w.flush();
    }
}

#[derive(Default)]
struct AttrVisitor {
    fields: Map<String, Value>,
}

impl Visit for AttrVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_string(), Value::String(value.to_string()));
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), Value::from(value));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), Value::from(value));
    }
    fn record_i128(&mut self, field: &Field, value: i128) {
        self.fields
            .insert(field.name().to_string(), Value::String(value.to_string()));
    }
    fn record_u128(&mut self, field: &Field, value: u128) {
        self.fields
            .insert(field.name().to_string(), Value::String(value.to_string()));
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        match serde_json::Number::from_f64(value) {
            Some(n) => {
                self.fields
                    .insert(field.name().to_string(), Value::Number(n));
            }
            None => {
                self.fields
                    .insert(field.name().to_string(), Value::String(value.to_string()));
            }
        }
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), Value::Bool(value));
    }
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.fields.insert(
            field.name().to_string(),
            Value::String(format!("{value:?}")),
        );
    }
    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.fields
            .insert(field.name().to_string(), Value::String(value.to_string()));
    }
}

fn redact_map(map: &mut Map<String, Value>, keys: &[String]) {
    let lower: Vec<String> = keys.iter().map(|s| s.to_ascii_lowercase()).collect();
    redact_recursive(map, &lower);
}

fn redact_recursive(map: &mut Map<String, Value>, lower_keys: &[String]) {
    for (k, v) in map.iter_mut() {
        let kl = k.to_ascii_lowercase();
        if lower_keys.iter().any(|x| x == &kl) {
            *v = Value::String(REDACTED.to_string());
        } else if let Value::Object(inner) = v {
            redact_recursive(inner, lower_keys);
        }
    }
}

/// Build the `<name>.<suffix>` event name for a span lifecycle transition.
///
/// Matches the Python reference convention (`obs.span()` emits `<event>.started`
/// / `<event>.ended`). Returns `(event_name, body)` — body mirrors event_name,
/// matching Python's default where no explicit `_body` is set.
///
/// If the resulting `<name>.<suffix>` does not satisfy the event regex, falls
/// back to `span.<suffix>` and records the raw span name under `span_name`.
fn suffixed_event_name(
    name: &str,
    suffix: &str,
    attrs_map: &mut Map<String, Value>,
) -> (String, String) {
    let candidate = format!("{name}.{suffix}");
    if is_valid_event_name(&candidate) {
        (candidate.clone(), candidate)
    } else {
        attrs_map.insert("span_name".into(), Value::String(name.to_string()));
        let fallback = format!("span.{suffix}");
        (fallback.clone(), fallback)
    }
}

fn is_valid_event_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    let mut has_dot = false;
    let mut prev_dot = false;
    for c in chars {
        if c == '.' {
            if prev_dot {
                return false;
            }
            has_dot = true;
            prev_dot = true;
        } else if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
            prev_dot = false;
        } else {
            return false;
        }
    }
    has_dot && !prev_dot
}

fn level_to_severity(level: tracing::Level) -> (&'static str, u8) {
    match level {
        tracing::Level::TRACE => ("DEBUG", 1),
        tracing::Level::DEBUG => ("DEBUG", 5),
        tracing::Level::INFO => ("INFO", 9),
        tracing::Level::WARN => ("WARN", 13),
        tracing::Level::ERROR => ("ERROR", 17),
    }
}

fn now_timestamps() -> (String, String) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let ns = now.as_nanos();
    let secs = (ns / 1_000_000_000) as i64;
    let frac_ns = (ns % 1_000_000_000) as u32;
    let dt: DateTime<Utc> = Utc
        .timestamp_opt(secs, frac_ns)
        .single()
        .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().expect("epoch valid"));
    let ts_iso = dt.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string();
    (ns.to_string(), ts_iso)
}

fn random_trace_id() -> String {
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    hex_of(&b)
}

fn random_span_id() -> String {
    let mut b = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut b);
    hex_of(&b)
}

fn hex_of(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

fn parse_traceparent(s: &str) -> Option<(String, Option<String>)> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 4 {
        return None;
    }
    if parts[0] != "00" {
        return None;
    }
    let tid = parts[1];
    let pid = parts[2];
    if tid.len() != 32 || !tid.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    if pid.len() != 16 || !pid.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    if tid.chars().all(|c| c == '0') {
        return None;
    }
    if pid.chars().all(|c| c == '0') {
        return None;
    }
    Some((tid.to_ascii_lowercase(), Some(pid.to_ascii_lowercase())))
}

fn generate_ulid() -> String {
    const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
        & 0x0000_FFFF_FFFF_FFFF;
    let mut rnd = [0u8; 10];
    rand::thread_rng().fill_bytes(&mut rnd);

    // Assemble 128-bit value: top 48 bits = ms, bottom 80 bits = random.
    let mut v: u128 = ms;
    for &b in &rnd {
        v = (v << 8) | (b as u128);
    }

    let mut out = [0u8; 26];
    // 26 chars × 5 bits = 130 bits; the top 2 bits of the conceptual value
    // are zero (ULID spec). Shift values per character: 125 - 5*i.
    for (i, slot) in out.iter_mut().enumerate() {
        let shift = 125_i32 - 5 * (i as i32);
        let idx = if shift >= 0 {
            ((v >> shift as u32) & 0x1F) as usize
        } else {
            0
        };
        *slot = CROCKFORD[idx];
    }
    // SAFETY: CROCKFORD is ASCII so `out` is valid UTF-8.
    String::from_utf8(out.to_vec()).expect("crockford ascii")
}

// --- unit tests ------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traceparent_roundtrip() {
        let s = "00-3f2e8c1a9b7d450f6e2c8a1b9f4d730e-9f2e4a1b6c0d3387-01";
        let parsed = parse_traceparent(s).expect("parses");
        assert_eq!(parsed.0, "3f2e8c1a9b7d450f6e2c8a1b9f4d730e");
        assert_eq!(parsed.1.as_deref(), Some("9f2e4a1b6c0d3387"));
    }

    #[test]
    fn traceparent_rejects_malformed() {
        assert!(parse_traceparent("").is_none());
        assert!(parse_traceparent("garbage").is_none());
        assert!(parse_traceparent("01-aa-bb-01").is_none());
        // wrong version
        assert!(
            parse_traceparent("ff-3f2e8c1a9b7d450f6e2c8a1b9f4d730e-9f2e4a1b6c0d3387-01").is_none()
        );
        // all-zero trace_id
        assert!(
            parse_traceparent("00-00000000000000000000000000000000-9f2e4a1b6c0d3387-01").is_none()
        );
        // all-zero span_id
        assert!(
            parse_traceparent("00-3f2e8c1a9b7d450f6e2c8a1b9f4d730e-0000000000000000-01").is_none()
        );
    }

    #[test]
    fn ulid_has_correct_shape() {
        for _ in 0..20 {
            let u = generate_ulid();
            assert_eq!(u.len(), 26, "{u}");
            assert!(
                u.chars()
                    .all(|c| matches!(c, '0'..='9' | 'A'..='H' | 'J' | 'K' | 'M' | 'N' | 'P'..='T' | 'V'..='Z')),
                "crockford chars only: {u}"
            );
        }
    }

    #[test]
    fn event_regex_accepts_and_rejects() {
        assert!(is_valid_event_name("task.started"));
        assert!(is_valid_event_name("a.b.c"));
        assert!(is_valid_event_name("spec.verify"));
        assert!(!is_valid_event_name(""));
        assert!(!is_valid_event_name("no_dot"));
        assert!(!is_valid_event_name(".leading"));
        assert!(!is_valid_event_name("trailing."));
        assert!(!is_valid_event_name("Upper.case"));
        assert!(!is_valid_event_name("double..dot"));
    }

    #[test]
    fn redact_replaces_listed_keys_case_insensitive() {
        let mut m = Map::new();
        m.insert("Api_Key".into(), Value::String("xyz".into()));
        m.insert("user".into(), Value::String("alice".into()));
        let mut nested = Map::new();
        nested.insert("password".into(), Value::String("p".into()));
        m.insert("inner".into(), Value::Object(nested));
        redact_map(&mut m, &["api_key".to_string(), "password".to_string()]);
        assert_eq!(m["Api_Key"], Value::String(REDACTED.into()));
        assert_eq!(m["user"], Value::String("alice".into()));
        assert_eq!(m["inner"]["password"], Value::String(REDACTED.into()));
    }

    #[test]
    fn hex_encoding_produces_lowercase_hex() {
        assert_eq!(hex_of(&[0x00, 0xff, 0xab]), "00ffab");
    }

    #[test]
    fn random_ids_have_expected_lengths() {
        assert_eq!(random_trace_id().len(), 32);
        assert_eq!(random_span_id().len(), 16);
    }
}
