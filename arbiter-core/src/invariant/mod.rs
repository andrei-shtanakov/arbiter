//! Invariant checking layer for the Arbiter policy engine.
//!
//! Evaluates 10 safety rules before every agent assignment.
//! Critical violations trigger cascade fallback; warning violations
//! are logged but don't block assignment.

pub mod rules;
