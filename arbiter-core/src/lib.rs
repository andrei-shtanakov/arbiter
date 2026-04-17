//! Arbiter Core — shared library for the Arbiter policy engine.
//!
//! Pure logic: types, Decision Tree inference, invariant rules, metrics.
//! No I/O, no SQLite, no network.

pub mod error;
pub mod invariant;
pub mod policy;
pub mod traits;
pub mod types;

pub use error::{ArbiterError, Result};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbiter_core_reexports_error_types() {
        let _: Result<()> = Ok(());
        let _ = ArbiterError::InvalidTree("smoke".to_string());
    }
}
