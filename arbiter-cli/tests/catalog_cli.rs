//! Smoke tests for `arbiter-cli catalog` subcommands: exit codes and
//! fail-loud messages (design §6). Uses the compiled binary directly.

use std::process::{Command, Output};

fn run_catalog(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_arbiter-cli"));
    cmd.arg("catalog").args(args);
    // Isolate from the developer's real environment.
    cmd.env_remove("ATP_CATALOG")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("HOME");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("failed to run arbiter-cli")
}

fn vendored_catalog() -> String {
    format!(
        "{}/../config/agents-catalog.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn fixture(name: &str) -> String {
    format!(
        "{}/../arbiter-core/tests/fixtures/catalog/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn check_passes_on_vendored_catalog() {
    let out = run_catalog(&["check"], &[("ATP_CATALOG", &vendored_catalog())]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("catalog OK"));
}

#[test]
fn check_fails_on_retired_reference() {
    let out = run_catalog(&["check"], &[("ATP_CATALOG", &fixture("retired_ref.toml"))]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("V3"));
}

#[test]
fn check_fails_loud_without_any_config() {
    let out = run_catalog(&["check"], &[]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("$ATP_CATALOG"), "stderr: {stderr}");
}

#[test]
fn check_fails_when_env_path_missing() {
    let out = run_catalog(&["check"], &[("ATP_CATALOG", "/nonexistent/cat.toml")]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("/nonexistent/cat.toml"), "stderr: {stderr}");
}

#[test]
fn path_prints_resolved_path() {
    let cat = vendored_catalog();
    let out = run_catalog(&["path"], &[("ATP_CATALOG", &cat)]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), cat);
}

#[test]
fn path_exits_nonzero_when_file_missing() {
    let out = run_catalog(&["path"], &[("ATP_CATALOG", "/nonexistent/cat.toml")]);
    assert_eq!(out.status.code(), Some(1));
    // Path is still printed (useful for debugging), error goes to stderr.
    assert!(String::from_utf8_lossy(&out.stdout).contains("/nonexistent/cat.toml"));
}

#[test]
fn list_prints_agents_table() {
    let out = run_catalog(&["list"], &[("ATP_CATALOG", &vendored_catalog())]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("AGENT_ID"));
    assert!(stdout.contains("claude_code@claude-sonnet-4-6"));
}

#[test]
fn unknown_subcommand_exits_nonzero() {
    let out = run_catalog(&["frobnicate"], &[]);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn empty_home_is_treated_as_unset() {
    let out = run_catalog(&["path"], &[("HOME", "")]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not configured"), "stderr: {stderr}");
    // No cwd-relative path must be printed for an empty HOME.
    assert!(!String::from_utf8_lossy(&out.stdout).contains(".config"));
}
