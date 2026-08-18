//! Happy-path validation of the in-repo vendored dev-SSOT catalog copy.
//!
//! The rule-by-rule tests (V1..V7) live in `catalog_conformance.rs` and run
//! against the vendored copy of the shared cross-loader fixture set owned by
//! devtools. This file keeps only what that set does not cover: arbiter's own
//! `config/agents-catalog.toml` must stay valid. Reading the vendor file
//! directly is deliberate (design §7) — a hand-maintained fixture copy of it
//! would be a third artifact free to drift.

use arbiter_core::catalog::{parse_catalog, validate, Severity};

#[test]
fn vendored_dev_ssot_catalog_is_valid() {
    // Assertions are invariants, not exact counts: the vendor file moves.
    let path = format!(
        "{}/../config/agents-catalog.toml",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read vendored catalog {path}: {e}"));
    let cat = parse_catalog(&text).expect("vendored catalog must parse");
    let issues = validate(&cat);
    let errors: Vec<_> = issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "vendored catalog has errors: {errors:?}");
    assert!(!cat.models.is_empty());
    assert!(!cat.harnesses.is_empty());
    assert!(cat.agents.iter().any(|a| a.routable));
}
