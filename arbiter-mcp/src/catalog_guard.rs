//! Startup validation of `agents.toml` against the user-config catalog
//! (ADR-ECO-003b, last mile — PP-103).
//!
//! The catalog (`$ATP_CATALOG` / XDG, see `arbiter_core::catalog`) is the
//! truth about models and harnesses; `agents.toml` stays the server's own
//! policy config. At startup the server cross-checks the two and refuses to
//! start on an inconsistent pair (fail-loud), mirroring conformance Check 5
//! ("enrollment references missing/retired models").
//!
//! Validation is NOT a replacement for the enrollment plane: it never adds
//! or removes agents, it only vets that every fused `harness@model` key in
//! `agents.toml` is backed by the catalog. Legacy bare ids (`[aider]`) are
//! outside the SSOT by convention and only produce a warning.
//!
//! Scope: startup only. Hot reload of `agents.toml` (watcher.rs) does not
//! re-run this check — the catalog is re-read on the next restart.

use std::collections::HashMap;
use std::path::PathBuf;

use arbiter_core::catalog::{
    self, Catalog, CatalogError, CatalogSource, Issue, ModelStatus, Severity,
};

use crate::config::AgentConfig;

/// Outcome of the startup catalog check (the non-fatal paths).
#[derive(Debug)]
pub enum CatalogCheck {
    /// No user-config catalog on this machine (XDG layer without a file).
    /// Validation is skipped — the catalog is opt-in by presence.
    Skipped {
        /// Human-readable reason (logged as a warning).
        reason: String,
    },
    /// Catalog loaded and the pair is consistent; warnings are non-fatal.
    Validated {
        /// Path the catalog was loaded from.
        path: PathBuf,
        /// Non-fatal findings (catalog V6/V7 + agents.toml C-warnings).
        warnings: Vec<Issue>,
    },
}

/// Cross-check `agents.toml` section keys against a parsed catalog.
///
/// Pure and deterministic (keys are visited in sorted order). Errors mirror
/// conformance Check 5; warnings flag drift that the scaffold generator
/// (`gen_agents_scaffold.py`) reports as NEW/STALE:
///
/// | code | severity | rule |
/// |------|----------|------|
/// | C1 | warning | legacy bare id (no `@`) — outside the SSOT catalog |
/// | C2 | error   | fused key references an undeclared harness |
/// | C3 | error   | fused key references an undeclared model (Check 5) |
/// | C4 | error   | fused key references a retired model (Check 5) |
/// | C5 | warning | fused key references a deprecated model |
/// | C6 | warning | model status unknown to this build (`Other`) |
/// | C7 | warning | pair not enrolled `routable = true` in the catalog |
pub fn validate_agents_against_catalog(
    agents: &HashMap<String, AgentConfig>,
    cat: &Catalog,
) -> Vec<Issue> {
    let routable_pairs: std::collections::HashSet<String> = cat
        .agents
        .iter()
        .filter(|a| a.routable)
        .map(|a| a.agent_id())
        .collect();

    let mut keys: Vec<&String> = agents.keys().collect();
    keys.sort();

    let mut issues = Vec::new();
    for key in keys {
        let Some((harness, model)) = key.split_once('@') else {
            issues.push(warning(
                "C1",
                format!(
                    "agents.toml section '{key}' is a legacy bare id \
                     (not '<harness>@<model>') — outside the SSOT catalog"
                ),
            ));
            continue;
        };

        if !cat.harnesses.contains_key(harness) {
            issues.push(error(
                "C2",
                format!(
                    "agents.toml section '{key}' references harness \
                     '{harness}' not declared in the catalog"
                ),
            ));
        }

        match cat.models.get(model) {
            None => issues.push(error(
                "C3",
                format!(
                    "agents.toml section '{key}' references model '{model}' \
                     not declared in the catalog (conformance Check 5)"
                ),
            )),
            Some(entry) => match &entry.status {
                ModelStatus::Retired => issues.push(error(
                    "C4",
                    format!(
                        "agents.toml section '{key}' references retired model \
                         '{model}' (conformance Check 5)"
                    ),
                )),
                ModelStatus::Deprecated => issues.push(warning(
                    "C5",
                    format!(
                        "agents.toml section '{key}' references deprecated \
                         model '{model}'"
                    ),
                )),
                ModelStatus::Other(s) => issues.push(warning(
                    "C6",
                    format!(
                        "agents.toml section '{key}' references model \
                         '{model}' with status '{s}' unknown to this build"
                    ),
                )),
                ModelStatus::Active => {}
            },
        }

        if !routable_pairs.contains(key.as_str()) {
            issues.push(warning(
                "C7",
                format!(
                    "agents.toml section '{key}' is not enrolled \
                     routable = true in the catalog (stale section?)"
                ),
            ));
        }
    }
    issues
}

fn error(code: &'static str, message: String) -> Issue {
    Issue {
        severity: Severity::Error,
        code,
        message,
    }
}

fn warning(code: &'static str, message: String) -> Issue {
    Issue {
        severity: Severity::Warning,
        code,
        message,
    }
}

/// Run the full startup check against the real environment (fail-loud).
///
/// Semantics (ADR-003b D2 + PP-103):
/// - `$ATP_CATALOG` set but unusable (non-UTF-8 value, missing file,
///   unreadable, unparsable) — hard error, no silent fallback.
/// - XDG layer resolved but no file there — `Skipped` (catalog is opt-in
///   by presence; the server keeps its historical no-catalog behavior).
/// - Catalog present: catalog validation errors (V1-V5) or `agents.toml`
///   cross-check errors (C2-C4) — hard error with every finding listed.
///   Warnings are returned for the caller to log, never fatal.
pub fn startup_catalog_check(
    agents: &HashMap<String, AgentConfig>,
) -> Result<CatalogCheck, String> {
    // A non-UTF-8 $ATP_CATALOG must be a loud error, not silently unset
    // (same rule as arbiter-cli catalog).
    if let Some(raw) = std::env::var_os(catalog::CATALOG_ENV_VAR) {
        if raw.to_str().is_none() {
            return Err(format!(
                "${} is set but is not valid UTF-8; cannot use it as a path",
                catalog::CATALOG_ENV_VAR
            ));
        }
    }
    let home = std::env::var("HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let resolved = match catalog::resolve_path(|key| std::env::var(key).ok(), home.as_deref()) {
        Ok(r) => r,
        Err(e @ CatalogError::NotConfigured) => {
            return Ok(CatalogCheck::Skipped {
                reason: e.to_string(),
            })
        }
        Err(e) => return Err(e.to_string()),
    };
    if !resolved.path.exists() {
        let miss = catalog::missing_file_error(&resolved);
        return match resolved.source {
            CatalogSource::AtpCatalogEnv => Err(miss.to_string()),
            CatalogSource::XdgConfigHome | CatalogSource::HomeDefault => {
                Ok(CatalogCheck::Skipped {
                    reason: miss.to_string(),
                })
            }
        };
    }

    let text = std::fs::read_to_string(&resolved.path)
        .map_err(|e| format!("failed to read {}: {e}", resolved.path.display()))?;
    let cat =
        catalog::parse_catalog(&text).map_err(|e| format!("{}: {e}", resolved.path.display()))?;

    let mut findings = catalog::validate(&cat);
    findings.extend(validate_agents_against_catalog(agents, &cat));

    let (errors, warnings): (Vec<Issue>, Vec<Issue>) = findings
        .into_iter()
        .partition(|i| i.severity == Severity::Error);
    if !errors.is_empty() {
        let mut msg = format!(
            "agents.toml is inconsistent with the model catalog {} \
             ({} errors) — refusing to start:",
            resolved.path.display(),
            errors.len()
        );
        for issue in &errors {
            msg.push_str(&format!("\n  [{}] {}", issue.code, issue.message));
        }
        return Err(msg);
    }
    Ok(CatalogCheck::Validated {
        path: resolved.path,
        warnings,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const CATALOG: &str = r#"
        [models."model-x"]
        vendor = "acme"
        status = "active"

        [models."model-old"]
        vendor = "acme"
        status = "retired"

        [models."model-sunset"]
        vendor = "acme"
        status = "deprecated"

        [models."model-next"]
        vendor = "acme"
        status = "preview"

        [harnesses.claude_code]
        kind = "cli"
        shim = "shims/claude_code.py"
        routable = true

        [[agents]]
        harness = "claude_code"
        model = "model-x"
        tested = true
        routable = true
    "#;

    fn agent(display: &str) -> AgentConfig {
        AgentConfig {
            display_name: display.into(),
            supports_languages: vec!["python".into()],
            supports_types: vec!["review".into()],
            max_concurrent: 1,
            cost_per_hour: 0.1,
            avg_duration_min: 5.0,
        }
    }

    fn agents_of(keys: &[&str]) -> HashMap<String, AgentConfig> {
        keys.iter().map(|k| (k.to_string(), agent(k))).collect()
    }

    fn cat() -> Catalog {
        catalog::parse_catalog(CATALOG).expect("test catalog must parse")
    }

    fn codes(issues: &[Issue]) -> Vec<&'static str> {
        issues.iter().map(|i| i.code).collect()
    }

    #[test]
    fn consistent_pair_has_no_issues() {
        let issues = validate_agents_against_catalog(&agents_of(&["claude_code@model-x"]), &cat());
        assert!(issues.is_empty(), "unexpected issues: {issues:?}");
    }

    #[test]
    fn bare_legacy_id_is_a_warning_only() {
        let issues = validate_agents_against_catalog(&agents_of(&["aider"]), &cat());
        assert_eq!(codes(&issues), vec!["C1"]);
        assert_eq!(issues[0].severity, Severity::Warning);
    }

    #[test]
    fn undeclared_harness_is_an_error() {
        let issues = validate_agents_against_catalog(&agents_of(&["ghost@model-x"]), &cat());
        assert!(codes(&issues).contains(&"C2"), "issues: {issues:?}");
        assert!(issues
            .iter()
            .any(|i| i.code == "C2" && i.severity == Severity::Error));
    }

    #[test]
    fn missing_model_is_an_error_mentioning_check_5() {
        let issues =
            validate_agents_against_catalog(&agents_of(&["claude_code@no-such-model"]), &cat());
        let c3 = issues.iter().find(|i| i.code == "C3").expect("C3 expected");
        assert_eq!(c3.severity, Severity::Error);
        assert!(c3.message.contains("Check 5"), "message: {}", c3.message);
    }

    #[test]
    fn retired_model_is_an_error_mentioning_check_5() {
        let issues =
            validate_agents_against_catalog(&agents_of(&["claude_code@model-old"]), &cat());
        let c4 = issues.iter().find(|i| i.code == "C4").expect("C4 expected");
        assert_eq!(c4.severity, Severity::Error);
        assert!(c4.message.contains("retired"), "message: {}", c4.message);
        assert!(c4.message.contains("Check 5"), "message: {}", c4.message);
    }

    #[test]
    fn deprecated_model_is_a_warning() {
        let issues =
            validate_agents_against_catalog(&agents_of(&["claude_code@model-sunset"]), &cat());
        assert!(issues
            .iter()
            .any(|i| i.code == "C5" && i.severity == Severity::Warning));
        assert!(!issues.iter().any(|i| i.severity == Severity::Error));
    }

    #[test]
    fn unknown_status_is_a_warning_not_an_error() {
        // Forward-compat: a status this build does not know (e.g. "preview")
        // must not block startup — but it must not pass silently either.
        let issues =
            validate_agents_against_catalog(&agents_of(&["claude_code@model-next"]), &cat());
        assert!(issues
            .iter()
            .any(|i| i.code == "C6" && i.severity == Severity::Warning));
        assert!(!issues.iter().any(|i| i.severity == Severity::Error));
    }

    #[test]
    fn pair_not_routable_in_catalog_is_a_warning() {
        // model-sunset has no [[agents]] enrollment at all.
        let issues =
            validate_agents_against_catalog(&agents_of(&["claude_code@model-sunset"]), &cat());
        assert!(issues.iter().any(|i| i.code == "C7"), "issues: {issues:?}");
    }

    #[test]
    fn issues_are_deterministically_ordered() {
        let agents = agents_of(&["b@no-such-model", "a@no-such-model"]);
        let first = validate_agents_against_catalog(&agents, &cat());
        let second = validate_agents_against_catalog(&agents, &cat());
        assert_eq!(codes(&first), codes(&second));
        let messages: Vec<&str> = first.iter().map(|i| i.message.as_str()).collect();
        let a_pos = messages.iter().position(|m| m.contains("'a@")).unwrap();
        let b_pos = messages.iter().position(|m| m.contains("'b@")).unwrap();
        assert!(a_pos < b_pos, "sorted key order expected: {messages:?}");
    }

    #[test]
    fn vendored_catalog_and_repo_agents_toml_are_consistent() {
        // The dev-vendored SSOT copy and the shipped agents.toml must stay a
        // consistent pair — otherwise a user pointing $ATP_CATALOG at the
        // SSOT would be unable to start the server (fail-loud regression).
        let repo_config = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("config");
        let cat_text = std::fs::read_to_string(repo_config.join("agents-catalog.toml"))
            .expect("vendored catalog must exist");
        let cat = catalog::parse_catalog(&cat_text).expect("vendored catalog must parse");
        let agents = crate::config::load_agents(&repo_config).expect("agents.toml must load");

        let issues = validate_agents_against_catalog(&agents, &cat);
        let errors: Vec<&Issue> = issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "inconsistent pair: {errors:?}");
    }
}
