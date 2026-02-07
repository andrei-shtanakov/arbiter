//! Arbiter Core — shared library for the Arbiter policy engine.
//!
//! Pure logic: types, Decision Tree inference, invariant rules, metrics.
//! No I/O, no SQLite, no network.

pub mod invariant;
pub mod policy;
pub mod types;

#[cfg(test)]
mod tests {
    #[test]
    fn arbiter_core_compiles() {
        assert!(true);
    }
}
