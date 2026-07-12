//! Authority plane (RD-006): role/phase-scoped agent allowlists.
//!
//! Answers "MAY this agent act here", strictly separate from capability
//! ("CAN this agent do the task"). Pure logic — the policy data is injected
//! by the caller (arbiter-mcp loads `config/authority.toml`, a pinned vendored
//! copy of steward's `profiles/authority.yaml`; see
//! `docs/2026-07-12-authority-split-design.md`).
//!
//! Semantics (design §4-§5): pure allowlist, default deny; agent patterns are
//! exactly two forms — `harness@model` (exact) or `harness@*` (whole harness);
//! a request without `authority_context` is decided by the policy's
//! `unknown_context` (fail-closed `deny` is the intended default). An
//! authority denial is a first-class audited outcome, never a silently
//! missing candidate.

use serde::{Deserialize, Serialize};

/// Closed v1 vocabulary of agent-run roles (design §2). NOT human approval roles.
pub const ROLES: [&str; 4] = ["decompose", "implement", "review", "benchmark"];

/// Closed v1 vocabulary of coarse lifecycle phases (design §2).
pub const PHASES: [&str; 4] = ["authoring", "execution", "merge", "pr"];

/// Reason code used when authority filtering leaves zero candidates.
pub const REASON_NO_AUTHORIZED: &str = "authority_no_authorized_candidates";

/// Execution context supplied by the caller in `constraints.authority_context`.
///
/// Deliberately NOT part of `TaskInput`: it must never leak into the 22-dim
/// feature vector or DT training semantics (design §3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthorityContext {
    /// Agent-run function: one of [`ROLES`].
    pub role: String,
    /// Lifecycle phase: one of [`PHASES`].
    pub phase: String,
}

/// One allowlist rule: agents permitted for a (role, phase) pair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthorityRule {
    pub role: String,
    pub phase: String,
    /// Patterns: exact `harness@model` or `harness@*` only.
    pub agents: Vec<String>,
}

/// What to do with a request that carries no `authority_context`
/// while the authority feature is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UnknownContext {
    /// Fail-closed default: no context -> no authorized candidates.
    Deny,
    /// Migration mode: no context -> authority does not filter.
    Allow,
}

/// Parsed authority policy plus its provenance hash.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthorityPolicy {
    pub version: u32,
    pub unknown_context: UnknownContext,
    pub rules: Vec<AuthorityRule>,
    /// `sha256:<hex>` of the policy file bytes — travels into every audit
    /// record so a decision can be reproduced post-mortem (design §5).
    pub policy_sha: String,
}

/// One denied candidate with the reason, part of the audit payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthorityDenied {
    pub agent_id: String,
    pub reason: String,
}

/// Audit block attached to the routing decision (`metadata.authority`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthorityAudit {
    pub policy_sha: String,
    pub role: Option<String>,
    pub phase: Option<String>,
    pub denied: Vec<AuthorityDenied>,
}

/// Validate a policy against the closed vocabularies and pattern forms.
///
/// Returns the first problem found; a policy that does not validate must not
/// be installed (config error at load, keep-previous on hot reload).
pub fn validate_policy(policy: &AuthorityPolicy) -> Result<(), String> {
    for (i, rule) in policy.rules.iter().enumerate() {
        if !ROLES.contains(&rule.role.as_str()) {
            return Err(format!(
                "rules[{i}]: unknown role '{}' (expected one of {ROLES:?})",
                rule.role
            ));
        }
        if !PHASES.contains(&rule.phase.as_str()) {
            return Err(format!(
                "rules[{i}]: unknown phase '{}' (expected one of {PHASES:?})",
                rule.phase
            ));
        }
        if rule.agents.is_empty() {
            return Err(format!(
                "rules[{i}]: empty agents list (allowlist rule must allow something)"
            ));
        }
        for pattern in &rule.agents {
            if !valid_pattern(pattern) {
                return Err(format!(
                    "rules[{i}]: invalid agent pattern '{pattern}' \
                     (only exact 'harness@model' or 'harness@*' are allowed)"
                ));
            }
        }
    }
    Ok(())
}

/// A pattern is either exact `harness@model` or `harness@*` — nothing else.
fn valid_pattern(pattern: &str) -> bool {
    match pattern.split_once('@') {
        Some((harness, model)) => {
            !harness.is_empty()
                && !harness.contains('*')
                && !model.is_empty()
                && (model == "*" || !model.contains('*'))
        }
        None => false,
    }
}

/// Does `pattern` authorize `agent_id`?
///
/// Exact match, or `harness@*` matching any model of that harness. A plain
/// legacy agent id without `@` can only be matched exactly — `harness@*`
/// intentionally does not match the bare harness name (ambiguous identity).
pub fn pattern_matches(pattern: &str, agent_id: &str) -> bool {
    if pattern == agent_id {
        return true;
    }
    if let Some((harness, "*")) = pattern.split_once('@') {
        if let Some((agent_harness, _model)) = agent_id.split_once('@') {
            return agent_harness == harness;
        }
    }
    false
}

/// Apply the authority allowlist to candidate agent ids.
///
/// Returns the authorized subset (order preserved) and the audit block.
/// Fail-closed: an invalid context (unknown role/phase) denies everything —
/// wire-schema validation should reject it earlier, but the engine must not
/// depend on that.
pub fn check_authority(
    policy: &AuthorityPolicy,
    context: Option<&AuthorityContext>,
    candidates: &[String],
) -> (Vec<String>, AuthorityAudit) {
    let mut audit = AuthorityAudit {
        policy_sha: policy.policy_sha.clone(),
        role: context.map(|c| c.role.clone()),
        phase: context.map(|c| c.phase.clone()),
        denied: Vec::new(),
    };

    let Some(ctx) = context else {
        return match policy.unknown_context {
            UnknownContext::Allow => (candidates.to_vec(), audit),
            UnknownContext::Deny => {
                audit.denied = deny_all(candidates, "no authority_context (unknown_context=deny)");
                (Vec::new(), audit)
            }
        };
    };

    if !ROLES.contains(&ctx.role.as_str()) || !PHASES.contains(&ctx.phase.as_str()) {
        audit.denied = deny_all(
            candidates,
            &format!(
                "invalid authority_context (role '{}', phase '{}')",
                ctx.role, ctx.phase
            ),
        );
        return (Vec::new(), audit);
    }

    let patterns: Vec<&String> = policy
        .rules
        .iter()
        .filter(|r| r.role == ctx.role && r.phase == ctx.phase)
        .flat_map(|r| r.agents.iter())
        .collect();

    let mut allowed = Vec::new();
    for agent_id in candidates {
        if patterns.iter().any(|p| pattern_matches(p, agent_id)) {
            allowed.push(agent_id.clone());
        } else {
            audit.denied.push(AuthorityDenied {
                agent_id: agent_id.clone(),
                reason: format!("no rule for ({}, {}) matches", ctx.role, ctx.phase),
            });
        }
    }
    (allowed, audit)
}

fn deny_all(candidates: &[String], reason: &str) -> Vec<AuthorityDenied> {
    candidates
        .iter()
        .map(|agent_id| AuthorityDenied {
            agent_id: agent_id.clone(),
            reason: reason.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(rules: Vec<AuthorityRule>, unknown: UnknownContext) -> AuthorityPolicy {
        AuthorityPolicy {
            version: 1,
            unknown_context: unknown,
            rules,
            policy_sha: format!("sha256:{}", "0".repeat(64)),
        }
    }

    fn rule(role: &str, phase: &str, agents: &[&str]) -> AuthorityRule {
        AuthorityRule {
            role: role.to_string(),
            phase: phase.to_string(),
            agents: agents.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn ctx(role: &str, phase: &str) -> AuthorityContext {
        AuthorityContext {
            role: role.to_string(),
            phase: phase.to_string(),
        }
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // ---------------------------------------------------------- patterns

    #[test]
    fn exact_pattern_matches_only_itself() {
        assert!(pattern_matches(
            "claude_code@claude-sonnet-4-6",
            "claude_code@claude-sonnet-4-6"
        ));
        assert!(!pattern_matches(
            "claude_code@claude-sonnet-4-6",
            "claude_code@claude-opus-4-8"
        ));
    }

    #[test]
    fn harness_wildcard_matches_any_model_of_that_harness() {
        assert!(pattern_matches(
            "claude_code@*",
            "claude_code@claude-opus-4-8"
        ));
        assert!(pattern_matches(
            "claude_code@*",
            "claude_code@claude-sonnet-4-6"
        ));
        assert!(!pattern_matches("claude_code@*", "codex_cli@gpt-5.5"));
    }

    #[test]
    fn harness_wildcard_does_not_match_bare_legacy_id() {
        // `aider` (no @) has ambiguous identity; only an exact rule may allow it.
        assert!(!pattern_matches("aider@*", "aider"));
        assert!(pattern_matches("aider", "aider"));
    }

    #[test]
    fn pattern_validation_rejects_arbitrary_globs() {
        let bad = [
            "*",
            "claude_code@son*",
            "*@gpt-5.5",
            "claude_*@x",
            "@model",
            "harness@",
        ];
        for pattern in bad {
            let p = policy(
                vec![rule("implement", "execution", &[pattern])],
                UnknownContext::Deny,
            );
            assert!(
                validate_policy(&p).is_err(),
                "pattern {pattern:?} must be rejected"
            );
        }
    }

    #[test]
    fn policy_validation_rejects_unknown_vocab() {
        let p = policy(
            vec![rule("hacker", "execution", &["a@b"])],
            UnknownContext::Deny,
        );
        assert!(validate_policy(&p).unwrap_err().contains("hacker"));
        let p = policy(
            vec![rule("implement", "deploy", &["a@b"])],
            UnknownContext::Deny,
        );
        assert!(validate_policy(&p).unwrap_err().contains("deploy"));
    }

    // ---------------------------------------------------------- filtering

    #[test]
    fn allowlist_filters_and_audits_denied() {
        let p = policy(
            vec![rule("implement", "execution", &["claude_code@*"])],
            UnknownContext::Deny,
        );
        let candidates = ids(&["claude_code@claude-opus-4-8", "opencode@glm-5.1"]);
        let (allowed, audit) =
            check_authority(&p, Some(&ctx("implement", "execution")), &candidates);
        assert_eq!(allowed, ids(&["claude_code@claude-opus-4-8"]));
        assert_eq!(audit.denied.len(), 1);
        assert_eq!(audit.denied[0].agent_id, "opencode@glm-5.1");
        assert!(audit.denied[0].reason.contains("implement"));
        assert_eq!(audit.role.as_deref(), Some("implement"));
        assert!(audit.policy_sha.starts_with("sha256:"));
    }

    #[test]
    fn default_deny_without_matching_rule() {
        let p = policy(
            vec![rule("review", "execution", &["claude_code@*"])],
            UnknownContext::Deny,
        );
        let candidates = ids(&["claude_code@claude-opus-4-8"]);
        let (allowed, audit) =
            check_authority(&p, Some(&ctx("implement", "execution")), &candidates);
        assert!(allowed.is_empty());
        assert_eq!(audit.denied.len(), 1);
    }

    #[test]
    fn missing_context_denies_when_policy_says_deny() {
        let p = policy(vec![], UnknownContext::Deny);
        let (allowed, audit) = check_authority(&p, None, &ids(&["a@b"]));
        assert!(allowed.is_empty());
        assert!(audit.denied[0].reason.contains("unknown_context=deny"));
        assert_eq!(audit.role, None);
    }

    #[test]
    fn missing_context_passes_through_in_allow_migration_mode() {
        let p = policy(vec![], UnknownContext::Allow);
        let (allowed, audit) = check_authority(&p, None, &ids(&["a@b", "c@d"]));
        assert_eq!(allowed.len(), 2);
        assert!(audit.denied.is_empty());
    }

    #[test]
    fn invalid_context_fails_closed() {
        let p = policy(
            vec![rule("implement", "execution", &["a@b"])],
            UnknownContext::Allow, // even in allow mode: an *invalid* context denies
        );
        let (allowed, audit) = check_authority(&p, Some(&ctx("root", "execution")), &ids(&["a@b"]));
        assert!(allowed.is_empty());
        assert!(audit.denied[0].reason.contains("invalid authority_context"));
    }

    #[test]
    fn multiple_rules_for_same_pair_union() {
        let p = policy(
            vec![
                rule("implement", "execution", &["claude_code@*"]),
                rule("implement", "execution", &["codex_cli@gpt-5.5"]),
            ],
            UnknownContext::Deny,
        );
        let candidates = ids(&["claude_code@x", "codex_cli@gpt-5.5", "opencode@y"]);
        let (allowed, _) = check_authority(&p, Some(&ctx("implement", "execution")), &candidates);
        assert_eq!(allowed, ids(&["claude_code@x", "codex_cli@gpt-5.5"]));
    }
}
