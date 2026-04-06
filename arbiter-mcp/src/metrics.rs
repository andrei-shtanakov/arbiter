//! In-memory metrics for the Arbiter MCP server.
//!
//! Uses `std::sync::atomic` for lock-free counter updates.
//! All operations use `Ordering::Relaxed` — sufficient for metrics
//! where we don't need sequential consistency.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

/// Thread-safe in-memory metrics collector.
pub struct Metrics {
    decisions_total: AtomicU64,
    fallbacks_total: AtomicU64,
    rejects_total: AtomicU64,
    latency_count: AtomicU64,
    latency_sum_us: AtomicU64,
    latency_min_us: AtomicI64,
    latency_max_us: AtomicI64,
}

/// Point-in-time snapshot of all metrics.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricsSnapshot {
    pub decisions_total: u64,
    pub fallbacks_total: u64,
    pub rejects_total: u64,
    pub latency: LatencySnapshot,
}

/// Point-in-time snapshot of latency statistics.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LatencySnapshot {
    pub count: u64,
    pub sum_us: u64,
    pub min_us: i64,
    pub max_us: i64,
}

impl Metrics {
    /// Create a new zeroed metrics collector.
    pub fn new() -> Self {
        Self {
            decisions_total: AtomicU64::new(0),
            fallbacks_total: AtomicU64::new(0),
            rejects_total: AtomicU64::new(0),
            latency_count: AtomicU64::new(0),
            latency_sum_us: AtomicU64::new(0),
            latency_min_us: AtomicI64::new(-1),
            latency_max_us: AtomicI64::new(-1),
        }
    }

    /// Record a single routing decision.
    ///
    /// Increments the decisions counter, optionally increments
    /// fallback/reject counters, and updates latency statistics.
    /// Uses CAS loops for min/max updates.
    pub fn record_decision(&self, latency_us: u64, is_fallback: bool, is_reject: bool) {
        self.decisions_total.fetch_add(1, Ordering::Relaxed);
        if is_fallback {
            self.fallbacks_total.fetch_add(1, Ordering::Relaxed);
        }
        if is_reject {
            self.rejects_total.fetch_add(1, Ordering::Relaxed);
        }

        self.latency_count.fetch_add(1, Ordering::Relaxed);
        self.latency_sum_us.fetch_add(latency_us, Ordering::Relaxed);

        let latency_signed = latency_us as i64;

        // CAS loop for min
        loop {
            let current = self.latency_min_us.load(Ordering::Relaxed);
            if current != -1 && current <= latency_signed {
                break;
            }
            match self.latency_min_us.compare_exchange_weak(
                current,
                latency_signed,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(_) => continue,
            }
        }

        // CAS loop for max
        loop {
            let current = self.latency_max_us.load(Ordering::Relaxed);
            if current >= latency_signed {
                break;
            }
            match self.latency_max_us.compare_exchange_weak(
                current,
                latency_signed,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(_) => continue,
            }
        }
    }

    /// Take a point-in-time snapshot of all metrics.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            decisions_total: self.decisions_total.load(Ordering::Relaxed),
            fallbacks_total: self.fallbacks_total.load(Ordering::Relaxed),
            rejects_total: self.rejects_total.load(Ordering::Relaxed),
            latency: LatencySnapshot {
                count: self.latency_count.load(Ordering::Relaxed),
                sum_us: self.latency_sum_us.load(Ordering::Relaxed),
                min_us: self.latency_min_us.load(Ordering::Relaxed),
                max_us: self.latency_max_us.load(Ordering::Relaxed),
            },
        }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_metrics_are_zero() {
        let m = Metrics::new();
        let s = m.snapshot();
        assert_eq!(s.decisions_total, 0);
        assert_eq!(s.fallbacks_total, 0);
        assert_eq!(s.rejects_total, 0);
        assert_eq!(s.latency.count, 0);
        assert_eq!(s.latency.sum_us, 0);
        assert_eq!(s.latency.min_us, -1);
        assert_eq!(s.latency.max_us, -1);
    }

    #[test]
    fn record_decision_increments_counters() {
        let m = Metrics::new();
        m.record_decision(100, false, false); // normal
        m.record_decision(200, true, false); // fallback
        m.record_decision(300, false, true); // reject

        let s = m.snapshot();
        assert_eq!(s.decisions_total, 3);
        assert_eq!(s.fallbacks_total, 1);
        assert_eq!(s.rejects_total, 1);
    }

    #[test]
    fn latency_stats_correct() {
        let m = Metrics::new();
        m.record_decision(100, false, false);
        m.record_decision(500, false, false);
        m.record_decision(200, false, false);

        let s = m.snapshot();
        assert_eq!(s.latency.count, 3);
        assert_eq!(s.latency.sum_us, 800);
        assert_eq!(s.latency.min_us, 100);
        assert_eq!(s.latency.max_us, 500);
        // avg = 800 / 3 = 266
        assert_eq!(s.latency.sum_us / s.latency.count, 266);
    }

    #[test]
    fn snapshot_serializes_to_json() {
        let m = Metrics::new();
        m.record_decision(42, true, false);

        let s = m.snapshot();
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("\"decisions_total\":1"));
        assert!(json.contains("\"fallbacks_total\":1"));
        assert!(json.contains("\"min_us\":42"));
    }

    #[test]
    fn default_impl() {
        let m = Metrics::default();
        let s = m.snapshot();
        assert_eq!(s.decisions_total, 0);
        assert_eq!(s.latency.min_us, -1);
    }
}
