//! Typed error types for arbiter-core.

use thiserror::Error;

/// Errors produced by the arbiter-core library.
#[derive(Debug, Error)]
pub enum ArbiterError {
    /// The decision tree JSON structure is invalid or fails validation.
    #[error("{0}")]
    InvalidTree(String),

    /// The feature vector supplied to `predict()` is invalid.
    #[error("{0}")]
    InvalidFeatures(String),

    /// An error during tree inference (e.g. empty leaf node).
    #[error("{0}")]
    InferenceError(String),

    /// JSON serialization / deserialization error.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Convenience alias used throughout arbiter-core.
pub type Result<T> = std::result::Result<T, ArbiterError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_invalid_tree() {
        let err = ArbiterError::InvalidTree("bad tree".to_string());
        assert_eq!(err.to_string(), "bad tree");
    }

    #[test]
    fn display_invalid_features() {
        let err = ArbiterError::InvalidFeatures("wrong length".to_string());
        assert_eq!(err.to_string(), "wrong length");
    }

    #[test]
    fn display_inference_error() {
        let err = ArbiterError::InferenceError("empty leaf".to_string());
        assert_eq!(err.to_string(), "empty leaf");
    }

    #[test]
    fn from_serde_json_error() {
        let json_err = serde_json::from_str::<String>("not valid json").unwrap_err();
        let err: ArbiterError = json_err.into();
        assert!(matches!(err, ArbiterError::Json(_)));
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn result_alias_works() {
        let ok: Result<i32> = Ok(42);
        assert_eq!(ok.ok(), Some(42));

        let err: Result<i32> = Err(ArbiterError::InvalidTree("test".to_string()));
        assert!(err.is_err());
    }
}
