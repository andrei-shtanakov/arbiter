//! get_metrics tool implementation.
//!
//! Returns a point-in-time snapshot of server metrics including
//! decision counts, latency statistics, and derived rates.

use serde_json::Value;

use crate::metrics::Metrics;

/// Execute the get_metrics tool.
///
/// Takes a snapshot of the in-memory metrics, computes derived
/// rates (fallback_rate, reject_rate), and returns a JSON value.
pub fn execute(metrics: &Metrics) -> Value {
    let snap = metrics.snapshot();

    let fallback_rate = if snap.decisions_total > 0 {
        snap.fallbacks_total as f64 / snap.decisions_total as f64
    } else {
        0.0
    };

    let reject_rate = if snap.decisions_total > 0 {
        snap.rejects_total as f64 / snap.decisions_total as f64
    } else {
        0.0
    };

    let avg_latency_us = if snap.latency.count > 0 {
        snap.latency.sum_us as f64 / snap.latency.count as f64
    } else {
        0.0
    };

    serde_json::json!({
        "decisions_total": snap.decisions_total,
        "fallbacks_total": snap.fallbacks_total,
        "rejects_total": snap.rejects_total,
        "fallback_rate": fallback_rate,
        "reject_rate": reject_rate,
        "latency": {
            "count": snap.latency.count,
            "sum_us": snap.latency.sum_us,
            "avg_us": avg_latency_us,
            "min_us": snap.latency.min_us,
            "max_us": snap.latency.max_us
        }
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_empty_metrics() {
        let metrics = Metrics::new();
        let json = execute(&metrics);

        assert_eq!(json["decisions_total"], 0);
        assert_eq!(json["fallbacks_total"], 0);
        assert_eq!(json["rejects_total"], 0);
        assert_eq!(json["fallback_rate"], 0.0);
        assert_eq!(json["reject_rate"], 0.0);
        assert_eq!(json["latency"]["count"], 0);
        assert_eq!(json["latency"]["avg_us"], 0.0);
        assert_eq!(json["latency"]["min_us"], -1);
        assert_eq!(json["latency"]["max_us"], -1);
    }

    #[test]
    fn execute_with_data() {
        let metrics = Metrics::new();
        metrics.record_decision(100, false, false); // assign
        metrics.record_decision(200, true, false); // fallback
        metrics.record_decision(300, false, true); // reject
        metrics.record_decision(400, false, false); // assign

        let json = execute(&metrics);

        assert_eq!(json["decisions_total"], 4);
        assert_eq!(json["fallbacks_total"], 1);
        assert_eq!(json["rejects_total"], 1);
        // fallback_rate = 1/4 = 0.25
        assert_eq!(json["fallback_rate"], 0.25);
        // reject_rate = 1/4 = 0.25
        assert_eq!(json["reject_rate"], 0.25);
        assert_eq!(json["latency"]["count"], 4);
        assert_eq!(json["latency"]["sum_us"], 1000);
        assert_eq!(json["latency"]["avg_us"], 250.0);
        assert_eq!(json["latency"]["min_us"], 100);
        assert_eq!(json["latency"]["max_us"], 400);
    }
}
