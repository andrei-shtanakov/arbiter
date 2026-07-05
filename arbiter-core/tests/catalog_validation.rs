//! Validation-rule tests on fixtures + happy-path on the vendored
//! dev-SSOT catalog copy. Fixtures are one-rule-per-file so they can be
//! shared with the ATP/Maestro loaders later (design §7-8).

use arbiter_core::catalog::{parse_catalog, validate, Severity};

fn load_fixture(name: &str) -> arbiter_core::catalog::Catalog {
    let path = format!(
        "{}/tests/fixtures/catalog/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
    parse_catalog(&text).unwrap_or_else(|e| panic!("{name} must parse: {e}"))
}

fn codes(issues: &[arbiter_core::catalog::Issue], severity: Severity) -> Vec<&str> {
    issues
        .iter()
        .filter(|i| i.severity == severity)
        .map(|i| i.code)
        .collect()
}

#[test]
fn v1_unknown_harness_is_error() {
    let issues = validate(&load_fixture("unknown_harness.toml"));
    assert_eq!(codes(&issues, Severity::Error), vec!["V1"]);
}

#[test]
fn v2_unknown_model_is_error() {
    let issues = validate(&load_fixture("unknown_model.toml"));
    assert_eq!(codes(&issues, Severity::Error), vec!["V2"]);
}

#[test]
fn v3_retired_reference_is_error() {
    let issues = validate(&load_fixture("retired_ref.toml"));
    assert_eq!(codes(&issues, Severity::Error), vec!["V3"]);
}

#[test]
fn v4_duplicate_agent_id_is_error() {
    let issues = validate(&load_fixture("dup_agent.toml"));
    assert!(codes(&issues, Severity::Error).contains(&"V4"));
}

#[test]
fn v5_routable_agent_on_nonroutable_harness_is_error() {
    let issues = validate(&load_fixture("routable_conflict.toml"));
    assert_eq!(codes(&issues, Severity::Error), vec!["V5"]);
}

#[test]
fn v6_deprecated_reference_is_warning_not_error() {
    let issues = validate(&load_fixture("deprecated_ref.toml"));
    assert_eq!(codes(&issues, Severity::Error), Vec::<&str>::new());
    assert_eq!(codes(&issues, Severity::Warning), vec!["V6"]);
}

#[test]
fn v7_unknown_enum_values_are_warnings_not_errors() {
    let issues = validate(&load_fixture("unknown_enum.toml"));
    assert_eq!(codes(&issues, Severity::Error), Vec::<&str>::new());
    let warns = codes(&issues, Severity::Warning);
    assert_eq!(warns.iter().filter(|c| **c == "V7").count(), 2);
}

#[test]
fn vendored_dev_ssot_catalog_is_valid() {
    // Happy-path reads the in-repo vendored SSOT copy directly — NOT a
    // hand-maintained fixture copy (design §7: no third drifting artifact).
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
