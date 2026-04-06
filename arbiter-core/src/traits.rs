//! Trait abstractions for Arbiter components.
//!
//! These traits decouple arbiter-core's policy logic from concrete
//! storage and inference implementations. arbiter-mcp provides the
//! production implementations (SQLite-backed stores); arbiter-core
//! tests use in-memory mocks.

use crate::error::Result;
use crate::types::PredictionResult;

/// Inference backend: predicts agent class from a feature vector.
///
/// Implementors take a fixed-length float slice (the 22-dim feature
/// vector) and return a [`PredictionResult`] with the predicted class
/// index, confidence, and decision path.
pub trait InferenceBackend {
    /// Run inference on `features` and return the prediction.
    ///
    /// # Errors
    /// Returns an error if the feature vector length is wrong or
    /// contains non-finite values.
    fn predict(&self, features: &[f64]) -> Result<PredictionResult>;

    /// Number of features expected by this backend.
    fn n_features(&self) -> usize;

    /// Number of output classes this backend can predict.
    fn n_classes(&self) -> usize;

    /// Class label names corresponding to each output class index.
    fn class_names(&self) -> &[String];
}

/// Storage for routing decisions (audit trail).
///
/// Each routing decision is persisted so that operators can review
/// what the engine decided, why, and which invariants were checked.
pub trait DecisionStore {
    /// Persist a routing decision and return its row id.
    ///
    /// # Errors
    /// Returns an error if the underlying store is unavailable.
    fn store_decision(
        &self,
        task_id: &str,
        chosen_agent: &str,
        action: &str,
        confidence: f64,
        decision_path: &str,
        invariants_json: &str,
    ) -> Result<i64>;

    /// Look up a decision by task id, returning the row id if found.
    ///
    /// # Errors
    /// Returns an error if the underlying store is unavailable.
    fn find_decision_by_task(&self, task_id: &str) -> Result<Option<i64>>;
}

/// Storage for agent performance statistics.
///
/// Tracks running task counts and recent failure counts per agent,
/// used by invariant rules to decide whether an agent is available.
pub trait AgentStore {
    /// Number of tasks currently running on `agent_id`.
    ///
    /// # Errors
    /// Returns an error if the underlying store is unavailable.
    fn running_tasks(&self, agent_id: &str) -> Result<u32>;

    /// Total number of tasks running across all agents.
    ///
    /// # Errors
    /// Returns an error if the underlying store is unavailable.
    fn total_running_tasks(&self) -> Result<u32>;

    /// Number of task failures for `agent_id` in the last `hours`.
    ///
    /// # Errors
    /// Returns an error if the underlying store is unavailable.
    fn recent_failures(&self, agent_id: &str, hours: u32) -> Result<u32>;

    /// Increment the running task counter for `agent_id`.
    ///
    /// # Errors
    /// Returns an error if the underlying store is unavailable.
    fn increment_running(&self, agent_id: &str) -> Result<()>;

    /// Decrement the running task counter for `agent_id`.
    ///
    /// # Errors
    /// Returns an error if the underlying store is unavailable.
    fn decrement_running(&self, agent_id: &str) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ArbiterError;

    /// Mock inference backend for testing trait usage.
    struct MockBackend {
        classes: Vec<String>,
        feature_count: usize,
    }

    impl MockBackend {
        fn new(classes: Vec<String>, feature_count: usize) -> Self {
            Self {
                classes,
                feature_count,
            }
        }
    }

    impl InferenceBackend for MockBackend {
        fn predict(&self, features: &[f64]) -> Result<PredictionResult> {
            if features.len() != self.feature_count {
                return Err(ArbiterError::InvalidFeatures(format!(
                    "expected {} features, got {}",
                    self.feature_count,
                    features.len()
                )));
            }
            // Always predict class 0 with full confidence.
            Ok(PredictionResult {
                class: 0,
                confidence: 1.0,
                path: vec!["mock: always class 0".to_string()],
            })
        }

        fn n_features(&self) -> usize {
            self.feature_count
        }

        fn n_classes(&self) -> usize {
            self.classes.len()
        }

        fn class_names(&self) -> &[String] {
            &self.classes
        }
    }

    #[test]
    fn mock_backend_predict() {
        let backend = MockBackend::new(vec!["a".to_string(), "b".to_string()], 3);
        let result = backend.predict(&[1.0, 2.0, 3.0]).unwrap();
        assert_eq!(result.class, 0);
        assert_eq!(result.confidence, 1.0);
        assert_eq!(result.path.len(), 1);
    }

    #[test]
    fn mock_backend_wrong_features() {
        let backend = MockBackend::new(vec!["a".to_string()], 2);
        let err = backend.predict(&[1.0]).unwrap_err();
        assert!(err.to_string().contains("expected 2"));
    }

    #[test]
    fn mock_backend_metadata() {
        let backend = MockBackend::new(vec!["x".to_string(), "y".to_string(), "z".to_string()], 5);
        assert_eq!(backend.n_features(), 5);
        assert_eq!(backend.n_classes(), 3);
        assert_eq!(backend.class_names(), &["x", "y", "z"]);
    }

    #[test]
    fn trait_object_works() {
        // Verify InferenceBackend is object-safe.
        let backend: Box<dyn InferenceBackend> = Box::new(MockBackend::new(
            vec!["cat".to_string(), "dog".to_string()],
            2,
        ));
        let result = backend.predict(&[0.5, 0.5]).unwrap();
        assert_eq!(result.class, 0);
        assert_eq!(backend.n_features(), 2);
        assert_eq!(backend.n_classes(), 2);
    }
}
