//! 10 safety invariant rules evaluated before every agent assignment.
//!
//! Each rule returns an [`InvariantResult`] with severity, pass/fail, and detail.
//! Critical violations block assignment and trigger cascade fallback.
//! Warning violations are logged but allow assignment.
//!
//! The [`check_all_invariants`] function always returns exactly 10 results.

use crate::types::{AgentState, InvariantResult, Severity, TaskInput};

// ---------------------------------------------------------------------------
// Context types (pure data — no I/O)
// ---------------------------------------------------------------------------

/// Agent information needed for invariant checks.
#[derive(Debug, Clone)]
pub struct AgentContext {
    /// Agent identifier (e.g. "claude_code").
    pub agent_id: String,
    /// Current lifecycle state.
    pub state: AgentState,
    /// Number of tasks currently running on this agent.
    pub running_tasks: u32,
    /// Maximum concurrent tasks this agent can handle.
    pub max_concurrent: u32,
    /// Languages the agent supports (e.g. ["python", "rust"]).
    pub supports_languages: Vec<String>,
    /// Task types the agent supports (e.g. ["feature", "bugfix"]).
    pub supports_types: Vec<String>,
    /// Number of failures in the last 24 hours.
    pub failures_24h: u32,
    /// Average task duration in minutes.
    pub avg_duration_min: f64,
    /// Cost per hour in USD.
    pub cost_per_hour: f64,
}

/// System-wide state needed for invariant checks.
#[derive(Debug, Clone)]
pub struct SystemContext {
    /// Total running tasks across all agents.
    pub total_running_tasks: u32,
    /// Scopes of currently running tasks (list of scope lists).
    pub running_scopes: Vec<Vec<String>>,
    /// Branches used by currently running tasks.
    pub running_branches: Vec<String>,
    /// Remaining budget in USD (None = unlimited).
    pub budget_remaining_usd: Option<f64>,
    /// Number of retries for this task so far.
    pub retry_count: u32,
    /// Current API calls per minute.
    pub calls_per_minute: u32,
}

/// Invariant threshold configuration (mirrors invariants.toml).
#[derive(Debug, Clone)]
pub struct InvariantThresholds {
    /// Maximum total concurrent tasks across all agents.
    pub max_total_concurrent: u32,
    /// Maximum retry attempts.
    pub max_retries: u32,
    /// Maximum API calls per minute.
    pub calls_per_minute: u32,
    /// Maximum failures in 24h before agent is unhealthy.
    pub max_failures_24h: u32,
    /// SLA buffer multiplier (e.g. 1.5).
    pub buffer_multiplier: f64,
}

// ---------------------------------------------------------------------------
// Individual invariant rules
// ---------------------------------------------------------------------------

/// Rule 1: agent_available (Critical)
///
/// Agent must be active AND have available slots.
pub fn agent_available(agent: &AgentContext) -> InvariantResult {
    let has_slots = agent.running_tasks < agent.max_concurrent;
    let is_active = agent.state == AgentState::Active;
    let passed = is_active && has_slots;

    let detail = if passed {
        format!(
            "Agent {} is active with {}/{} slots used",
            agent.agent_id, agent.running_tasks, agent.max_concurrent
        )
    } else if !is_active {
        format!("Agent {} is {} (not active)", agent.agent_id, agent.state)
    } else {
        format!(
            "Agent {} at capacity ({}/{} slots used)",
            agent.agent_id, agent.running_tasks, agent.max_concurrent
        )
    };

    InvariantResult {
        rule: "agent_available".to_string(),
        severity: Severity::Critical,
        passed,
        detail,
    }
}

/// Rule 2: scope_isolation (Critical)
///
/// No file/directory overlap between the task's scope and any running task's scope.
pub fn scope_isolation(task: &TaskInput, system: &SystemContext) -> InvariantResult {
    if task.scope.is_empty() {
        return InvariantResult {
            rule: "scope_isolation".to_string(),
            severity: Severity::Critical,
            passed: true,
            detail: "No scope specified, no conflicts possible".to_string(),
        };
    }

    for running_scope in &system.running_scopes {
        for task_path in &task.scope {
            for running_path in running_scope {
                if paths_overlap(task_path, running_path) {
                    return InvariantResult {
                        rule: "scope_isolation".to_string(),
                        severity: Severity::Critical,
                        passed: false,
                        detail: format!(
                            "Scope conflict: {} overlaps with running task scope {}",
                            task_path, running_path
                        ),
                    };
                }
            }
        }
    }

    InvariantResult {
        rule: "scope_isolation".to_string(),
        severity: Severity::Critical,
        passed: true,
        detail: "No scope conflicts with running tasks".to_string(),
    }
}

/// Rule 3: branch_not_locked (Critical)
///
/// The task's branch must not be in use by any running task.
pub fn branch_not_locked(task: &TaskInput, system: &SystemContext) -> InvariantResult {
    let branch = match &task.branch {
        Some(b) => b,
        None => {
            return InvariantResult {
                rule: "branch_not_locked".to_string(),
                severity: Severity::Critical,
                passed: true,
                detail: "No branch specified, no lock conflict possible".to_string(),
            };
        }
    };

    let locked = system.running_branches.iter().any(|b| b == branch);

    InvariantResult {
        rule: "branch_not_locked".to_string(),
        severity: Severity::Critical,
        passed: !locked,
        detail: if locked {
            format!("Branch {} is locked by a running task", branch)
        } else {
            format!("Branch {} is available", branch)
        },
    }
}

/// Rule 4: concurrency_limit (Critical)
///
/// Total running tasks across all agents must be below the maximum.
pub fn concurrency_limit(
    system: &SystemContext,
    thresholds: &InvariantThresholds,
) -> InvariantResult {
    let passed = system.total_running_tasks < thresholds.max_total_concurrent;

    InvariantResult {
        rule: "concurrency_limit".to_string(),
        severity: Severity::Critical,
        passed,
        detail: if passed {
            format!(
                "Total running tasks {}/{} within limit",
                system.total_running_tasks, thresholds.max_total_concurrent
            )
        } else {
            format!(
                "Total running tasks {}/{} at or above limit",
                system.total_running_tasks, thresholds.max_total_concurrent
            )
        },
    }
}

/// Rule 5: budget_remaining (Warning)
///
/// Estimated cost for this task must not exceed the remaining budget.
pub fn budget_remaining(agent: &AgentContext, system: &SystemContext) -> InvariantResult {
    let budget = match system.budget_remaining_usd {
        Some(b) => b,
        None => {
            return InvariantResult {
                rule: "budget_remaining".to_string(),
                severity: Severity::Warning,
                passed: true,
                detail: "No budget constraint specified".to_string(),
            };
        }
    };

    // Estimate cost: cost_per_hour * (avg_duration_min / 60)
    let estimated_cost = agent.cost_per_hour * (agent.avg_duration_min / 60.0);
    let passed = estimated_cost <= budget;

    InvariantResult {
        rule: "budget_remaining".to_string(),
        severity: Severity::Warning,
        passed,
        detail: if passed {
            format!(
                "Estimated cost ${:.2} within budget ${:.2}",
                estimated_cost, budget
            )
        } else {
            format!(
                "Estimated cost ${:.2} exceeds budget ${:.2}",
                estimated_cost, budget
            )
        },
    }
}

/// Rule 6: retry_limit (Warning)
///
/// Retry count must be below the maximum.
pub fn retry_limit(system: &SystemContext, thresholds: &InvariantThresholds) -> InvariantResult {
    let passed = system.retry_count < thresholds.max_retries;

    InvariantResult {
        rule: "retry_limit".to_string(),
        severity: Severity::Warning,
        passed,
        detail: if passed {
            format!(
                "Retry count {}/{} within limit",
                system.retry_count, thresholds.max_retries
            )
        } else {
            format!(
                "Retry count {}/{} at or above limit",
                system.retry_count, thresholds.max_retries
            )
        },
    }
}

/// Rule 7: rate_limit (Warning)
///
/// Current API calls per minute must be below the limit.
pub fn rate_limit(system: &SystemContext, thresholds: &InvariantThresholds) -> InvariantResult {
    let passed = system.calls_per_minute < thresholds.calls_per_minute;

    InvariantResult {
        rule: "rate_limit".to_string(),
        severity: Severity::Warning,
        passed,
        detail: if passed {
            format!(
                "API calls {}/{} per minute within limit",
                system.calls_per_minute, thresholds.calls_per_minute
            )
        } else {
            format!(
                "API calls {}/{} per minute at or above limit",
                system.calls_per_minute, thresholds.calls_per_minute
            )
        },
    }
}

/// Rule 8: agent_health (Warning)
///
/// Agent's failure count in the last 24 hours must be below threshold.
pub fn agent_health(agent: &AgentContext, thresholds: &InvariantThresholds) -> InvariantResult {
    let passed = agent.failures_24h < thresholds.max_failures_24h;

    InvariantResult {
        rule: "agent_health".to_string(),
        severity: Severity::Warning,
        passed,
        detail: if passed {
            format!(
                "Agent {} has {}/{} failures in 24h",
                agent.agent_id, agent.failures_24h, thresholds.max_failures_24h
            )
        } else {
            format!(
                "Agent {} has {}/{} failures in 24h (unhealthy)",
                agent.agent_id, agent.failures_24h, thresholds.max_failures_24h
            )
        },
    }
}

/// Rule 9: task_compatible (Warning)
///
/// Agent must support the task's language AND task type.
pub fn task_compatible(task: &TaskInput, agent: &AgentContext) -> InvariantResult {
    let task_type_str = task.task_type.to_string();
    let lang_str = task.language.to_string();

    let supports_type = agent.supports_types.contains(&task_type_str);
    let supports_lang = agent.supports_languages.contains(&lang_str);
    let passed = supports_type && supports_lang;

    let detail = if passed {
        format!(
            "Agent {} supports {} + {}",
            agent.agent_id, task_type_str, lang_str
        )
    } else if !supports_type && !supports_lang {
        format!(
            "Agent {} does not support type {} or language {}",
            agent.agent_id, task_type_str, lang_str
        )
    } else if !supports_type {
        format!(
            "Agent {} does not support type {}",
            agent.agent_id, task_type_str
        )
    } else {
        format!(
            "Agent {} does not support language {}",
            agent.agent_id, lang_str
        )
    };

    InvariantResult {
        rule: "task_compatible".to_string(),
        severity: Severity::Warning,
        passed,
        detail,
    }
}

/// Rule 10: sla_feasible (Warning)
///
/// Estimated duration (with buffer) must fit within the SLA deadline.
pub fn sla_feasible(
    task: &TaskInput,
    agent: &AgentContext,
    thresholds: &InvariantThresholds,
) -> InvariantResult {
    let sla = match task.sla_minutes {
        Some(s) => s as f64,
        None => {
            return InvariantResult {
                rule: "sla_feasible".to_string(),
                severity: Severity::Warning,
                passed: true,
                detail: "No SLA deadline specified".to_string(),
            };
        }
    };

    let buffered_duration = agent.avg_duration_min * thresholds.buffer_multiplier;
    let passed = buffered_duration <= sla;

    InvariantResult {
        rule: "sla_feasible".to_string(),
        severity: Severity::Warning,
        passed,
        detail: if passed {
            format!(
                "Estimated {:.1}min (with {:.1}x buffer) within SLA {:.0}min",
                buffered_duration, thresholds.buffer_multiplier, sla
            )
        } else {
            format!(
                "Estimated {:.1}min (with {:.1}x buffer) exceeds SLA {:.0}min",
                buffered_duration, thresholds.buffer_multiplier, sla
            )
        },
    }
}

// ---------------------------------------------------------------------------
// Aggregate checker
// ---------------------------------------------------------------------------

/// Run all 10 invariant checks and return the results.
///
/// Always returns exactly 10 results, one per rule, in the canonical order:
/// 1. agent_available
/// 2. scope_isolation
/// 3. branch_not_locked
/// 4. concurrency_limit
/// 5. budget_remaining
/// 6. retry_limit
/// 7. rate_limit
/// 8. agent_health
/// 9. task_compatible
/// 10. sla_feasible
pub fn check_all_invariants(
    task: &TaskInput,
    agent: &AgentContext,
    system: &SystemContext,
    thresholds: &InvariantThresholds,
) -> Vec<InvariantResult> {
    vec![
        agent_available(agent),
        scope_isolation(task, system),
        branch_not_locked(task, system),
        concurrency_limit(system, thresholds),
        budget_remaining(agent, system),
        retry_limit(system, thresholds),
        rate_limit(system, thresholds),
        agent_health(agent, thresholds),
        task_compatible(task, agent),
        sla_feasible(task, agent, thresholds),
    ]
}

/// Returns `true` if any critical invariant failed.
pub fn has_critical_failure(results: &[InvariantResult]) -> bool {
    results
        .iter()
        .any(|r| r.severity == Severity::Critical && !r.passed)
}

// ---------------------------------------------------------------------------
// Path overlap helper
// ---------------------------------------------------------------------------

/// Two paths overlap if one is a prefix of the other or they are equal.
///
/// Directory containment is checked by normalizing paths with trailing slashes.
fn paths_overlap(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let a_dir = if a.ends_with('/') {
        a.to_string()
    } else {
        format!("{a}/")
    };
    let b_dir = if b.ends_with('/') {
        b.to_string()
    } else {
        format!("{b}/")
    };
    a_dir.starts_with(&b_dir) || b_dir.starts_with(&a_dir)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    // -- Test helpers --

    fn active_agent() -> AgentContext {
        AgentContext {
            agent_id: "claude_code".to_string(),
            state: AgentState::Active,
            running_tasks: 0,
            max_concurrent: 2,
            supports_languages: vec![
                "python".to_string(),
                "rust".to_string(),
                "typescript".to_string(),
            ],
            supports_types: vec![
                "feature".to_string(),
                "bugfix".to_string(),
                "refactor".to_string(),
                "docs".to_string(),
                "review".to_string(),
            ],
            failures_24h: 0,
            avg_duration_min: 18.0,
            cost_per_hour: 0.30,
        }
    }

    fn default_task() -> TaskInput {
        TaskInput {
            task_type: TaskType::Feature,
            language: Language::Rust,
            complexity: Complexity::Moderate,
            priority: Priority::Normal,
            scope: vec!["src/main.rs".to_string()],
            branch: Some("feature/new-thing".to_string()),
            estimated_tokens: Some(50000),
            has_dependencies: false,
            requires_internet: false,
            sla_minutes: Some(120),
            description: Some("Implement new feature".to_string()),
        }
    }

    fn empty_system() -> SystemContext {
        SystemContext {
            total_running_tasks: 0,
            running_scopes: vec![],
            running_branches: vec![],
            budget_remaining_usd: Some(10.0),
            retry_count: 0,
            calls_per_minute: 0,
        }
    }

    fn default_thresholds() -> InvariantThresholds {
        InvariantThresholds {
            max_total_concurrent: 5,
            max_retries: 3,
            calls_per_minute: 60,
            max_failures_24h: 5,
            buffer_multiplier: 1.5,
        }
    }

    // =====================================================================
    // UT-04: scope_isolation overlap
    // =====================================================================

    #[test]
    fn scope_isolation_exact_file_overlap() {
        let task = default_task(); // scope: ["src/main.rs"]
        let system = SystemContext {
            running_scopes: vec![vec!["src/main.rs".to_string()]],
            ..empty_system()
        };

        let result = scope_isolation(&task, &system);
        assert!(!result.passed);
        assert_eq!(result.severity, Severity::Critical);
        assert!(result.detail.contains("src/main.rs"));
    }

    // =====================================================================
    // UT-05: scope_isolation no overlap
    // =====================================================================

    #[test]
    fn scope_isolation_no_overlap() {
        let task = default_task(); // scope: ["src/main.rs"]
        let system = SystemContext {
            running_scopes: vec![vec!["tests/test_main.rs".to_string()]],
            ..empty_system()
        };

        let result = scope_isolation(&task, &system);
        assert!(result.passed);
        assert_eq!(result.severity, Severity::Critical);
    }

    // =====================================================================
    // UT-06: scope_isolation directory contains file
    // =====================================================================

    #[test]
    fn scope_isolation_directory_contains_file() {
        let task = default_task(); // scope: ["src/main.rs"]
        let system = SystemContext {
            running_scopes: vec![vec!["src/".to_string()]],
            ..empty_system()
        };

        let result = scope_isolation(&task, &system);
        assert!(!result.passed);
        assert_eq!(result.severity, Severity::Critical);
        assert!(result.detail.contains("src/main.rs"));
        assert!(result.detail.contains("src/"));
    }

    #[test]
    fn scope_isolation_file_inside_running_directory() {
        // Reverse: task has directory, running has file inside it
        let mut task = default_task();
        task.scope = vec!["src/".to_string()];
        let system = SystemContext {
            running_scopes: vec![vec!["src/lib.rs".to_string()]],
            ..empty_system()
        };

        let result = scope_isolation(&task, &system);
        assert!(!result.passed);
    }

    #[test]
    fn scope_isolation_empty_task_scope() {
        let mut task = default_task();
        task.scope = vec![];
        let system = SystemContext {
            running_scopes: vec![vec!["src/".to_string()]],
            ..empty_system()
        };

        let result = scope_isolation(&task, &system);
        assert!(result.passed);
    }

    #[test]
    fn scope_isolation_similar_prefix_no_overlap() {
        let mut task = default_task();
        task.scope = vec!["src/".to_string()];
        let system = SystemContext {
            running_scopes: vec![vec!["src_old/file.rs".to_string()]],
            ..empty_system()
        };

        let result = scope_isolation(&task, &system);
        assert!(result.passed);
    }

    // =====================================================================
    // UT-07: concurrency at limit
    // =====================================================================

    #[test]
    fn concurrency_at_limit() {
        let system = SystemContext {
            total_running_tasks: 5,
            ..empty_system()
        };
        let thresholds = default_thresholds(); // max_total_concurrent = 5

        let result = concurrency_limit(&system, &thresholds);
        assert!(!result.passed);
        assert_eq!(result.severity, Severity::Critical);
    }

    #[test]
    fn concurrency_above_limit() {
        let system = SystemContext {
            total_running_tasks: 7,
            ..empty_system()
        };
        let thresholds = default_thresholds();

        let result = concurrency_limit(&system, &thresholds);
        assert!(!result.passed);
    }

    // =====================================================================
    // UT-08: concurrency below limit
    // =====================================================================

    #[test]
    fn concurrency_below_limit() {
        let system = SystemContext {
            total_running_tasks: 3,
            ..empty_system()
        };
        let thresholds = default_thresholds(); // max_total_concurrent = 5

        let result = concurrency_limit(&system, &thresholds);
        assert!(result.passed);
        assert_eq!(result.severity, Severity::Critical);
    }

    #[test]
    fn concurrency_zero_running() {
        let system = empty_system();
        let thresholds = default_thresholds();

        let result = concurrency_limit(&system, &thresholds);
        assert!(result.passed);
    }

    // =====================================================================
    // UT-09: budget exceeded
    // =====================================================================

    #[test]
    fn budget_exceeded() {
        let agent = AgentContext {
            cost_per_hour: 10.0,
            avg_duration_min: 60.0, // cost = 10.0 * (60/60) = $10.00
            ..active_agent()
        };
        let system = SystemContext {
            budget_remaining_usd: Some(5.0), // only $5 left
            ..empty_system()
        };

        let result = budget_remaining(&agent, &system);
        assert!(!result.passed);
        assert_eq!(result.severity, Severity::Warning);
    }

    // =====================================================================
    // UT-10: budget ok
    // =====================================================================

    #[test]
    fn budget_ok() {
        let agent = active_agent(); // cost_per_hour=0.30, avg_duration_min=18
        let system = SystemContext {
            budget_remaining_usd: Some(10.0),
            ..empty_system()
        };

        // estimated = 0.30 * (18/60) = $0.09
        let result = budget_remaining(&agent, &system);
        assert!(result.passed);
        assert_eq!(result.severity, Severity::Warning);
    }

    #[test]
    fn budget_no_constraint() {
        let agent = active_agent();
        let system = SystemContext {
            budget_remaining_usd: None,
            ..empty_system()
        };

        let result = budget_remaining(&agent, &system);
        assert!(result.passed);
    }

    // =====================================================================
    // UT-11: branch locked
    // =====================================================================

    #[test]
    fn branch_locked() {
        let task = default_task(); // branch: "feature/new-thing"
        let system = SystemContext {
            running_branches: vec!["feature/new-thing".to_string()],
            ..empty_system()
        };

        let result = branch_not_locked(&task, &system);
        assert!(!result.passed);
        assert_eq!(result.severity, Severity::Critical);
        assert!(result.detail.contains("feature/new-thing"));
    }

    #[test]
    fn branch_not_locked_passes() {
        let task = default_task(); // branch: "feature/new-thing"
        let system = SystemContext {
            running_branches: vec!["feature/other".to_string()],
            ..empty_system()
        };

        let result = branch_not_locked(&task, &system);
        assert!(result.passed);
    }

    #[test]
    fn branch_none_passes() {
        let mut task = default_task();
        task.branch = None;
        let system = SystemContext {
            running_branches: vec!["main".to_string()],
            ..empty_system()
        };

        let result = branch_not_locked(&task, &system);
        assert!(result.passed);
    }

    // =====================================================================
    // UT-12: agent health failures
    // =====================================================================

    #[test]
    fn agent_health_too_many_failures() {
        let agent = AgentContext {
            failures_24h: 5, // at threshold
            ..active_agent()
        };
        let thresholds = default_thresholds(); // max_failures_24h = 5

        let result = agent_health(&agent, &thresholds);
        assert!(!result.passed);
        assert_eq!(result.severity, Severity::Warning);
    }

    #[test]
    fn agent_health_above_threshold() {
        let agent = AgentContext {
            failures_24h: 10,
            ..active_agent()
        };
        let thresholds = default_thresholds();

        let result = agent_health(&agent, &thresholds);
        assert!(!result.passed);
    }

    #[test]
    fn agent_health_ok() {
        let agent = AgentContext {
            failures_24h: 2,
            ..active_agent()
        };
        let thresholds = default_thresholds();

        let result = agent_health(&agent, &thresholds);
        assert!(result.passed);
    }

    #[test]
    fn agent_health_zero_failures() {
        let agent = active_agent(); // failures_24h = 0
        let thresholds = default_thresholds();

        let result = agent_health(&agent, &thresholds);
        assert!(result.passed);
    }

    // =====================================================================
    // agent_available tests
    // =====================================================================

    #[test]
    fn agent_available_active_with_slots() {
        let agent = active_agent();
        let result = agent_available(&agent);
        assert!(result.passed);
        assert_eq!(result.severity, Severity::Critical);
    }

    #[test]
    fn agent_available_at_capacity() {
        let agent = AgentContext {
            running_tasks: 2,
            max_concurrent: 2,
            ..active_agent()
        };
        let result = agent_available(&agent);
        assert!(!result.passed);
    }

    #[test]
    fn agent_available_inactive() {
        let agent = AgentContext {
            state: AgentState::Inactive,
            ..active_agent()
        };
        let result = agent_available(&agent);
        assert!(!result.passed);
        assert!(result.detail.contains("inactive"));
    }

    #[test]
    fn agent_available_failed_state() {
        let agent = AgentContext {
            state: AgentState::Failed,
            ..active_agent()
        };
        let result = agent_available(&agent);
        assert!(!result.passed);
    }

    // =====================================================================
    // task_compatible tests
    // =====================================================================

    #[test]
    fn task_compatible_supported() {
        let task = default_task(); // Feature + Rust
        let agent = active_agent(); // supports feature + rust
        let result = task_compatible(&task, &agent);
        assert!(result.passed);
        assert_eq!(result.severity, Severity::Warning);
    }

    #[test]
    fn task_compatible_unsupported_type() {
        let mut task = default_task();
        task.task_type = TaskType::Research; // not in agent's types
        let agent = active_agent();
        let result = task_compatible(&task, &agent);
        assert!(!result.passed);
        assert!(result.detail.contains("type"));
    }

    #[test]
    fn task_compatible_unsupported_language() {
        let mut task = default_task();
        task.language = Language::Go; // not in agent's languages
        let agent = active_agent();
        let result = task_compatible(&task, &agent);
        assert!(!result.passed);
        assert!(result.detail.contains("language"));
    }

    #[test]
    fn task_compatible_both_unsupported() {
        let mut task = default_task();
        task.task_type = TaskType::Research;
        task.language = Language::Go;
        let agent = active_agent();
        let result = task_compatible(&task, &agent);
        assert!(!result.passed);
    }

    // =====================================================================
    // retry_limit tests
    // =====================================================================

    #[test]
    fn retry_limit_within() {
        let system = SystemContext {
            retry_count: 1,
            ..empty_system()
        };
        let thresholds = default_thresholds();
        let result = retry_limit(&system, &thresholds);
        assert!(result.passed);
    }

    #[test]
    fn retry_limit_at_max() {
        let system = SystemContext {
            retry_count: 3, // at max_retries
            ..empty_system()
        };
        let thresholds = default_thresholds();
        let result = retry_limit(&system, &thresholds);
        assert!(!result.passed);
    }

    // =====================================================================
    // rate_limit tests
    // =====================================================================

    #[test]
    fn rate_limit_within() {
        let system = SystemContext {
            calls_per_minute: 30,
            ..empty_system()
        };
        let thresholds = default_thresholds();
        let result = rate_limit(&system, &thresholds);
        assert!(result.passed);
    }

    #[test]
    fn rate_limit_at_max() {
        let system = SystemContext {
            calls_per_minute: 60, // at limit
            ..empty_system()
        };
        let thresholds = default_thresholds();
        let result = rate_limit(&system, &thresholds);
        assert!(!result.passed);
    }

    // =====================================================================
    // sla_feasible tests
    // =====================================================================

    #[test]
    fn sla_feasible_within() {
        let task = default_task(); // sla_minutes: 120
        let agent = active_agent(); // avg_duration_min: 18
        let thresholds = default_thresholds(); // buffer: 1.5
                                               // buffered = 18 * 1.5 = 27.0 <= 120
        let result = sla_feasible(&task, &agent, &thresholds);
        assert!(result.passed);
    }

    #[test]
    fn sla_feasible_exceeded() {
        let mut task = default_task();
        task.sla_minutes = Some(10); // tight SLA
        let agent = AgentContext {
            avg_duration_min: 10.0,
            ..active_agent()
        };
        let thresholds = default_thresholds(); // buffer: 1.5
                                               // buffered = 10 * 1.5 = 15.0 > 10
        let result = sla_feasible(&task, &agent, &thresholds);
        assert!(!result.passed);
    }

    #[test]
    fn sla_feasible_no_sla() {
        let mut task = default_task();
        task.sla_minutes = None;
        let agent = active_agent();
        let thresholds = default_thresholds();
        let result = sla_feasible(&task, &agent, &thresholds);
        assert!(result.passed);
    }

    // =====================================================================
    // check_all_invariants — returns 10 results
    // =====================================================================

    #[test]
    fn check_all_returns_10_results() {
        let task = default_task();
        let agent = active_agent();
        let system = empty_system();
        let thresholds = default_thresholds();

        let results = check_all_invariants(&task, &agent, &system, &thresholds);
        assert_eq!(results.len(), 10);

        // Verify canonical ordering
        assert_eq!(results[0].rule, "agent_available");
        assert_eq!(results[1].rule, "scope_isolation");
        assert_eq!(results[2].rule, "branch_not_locked");
        assert_eq!(results[3].rule, "concurrency_limit");
        assert_eq!(results[4].rule, "budget_remaining");
        assert_eq!(results[5].rule, "retry_limit");
        assert_eq!(results[6].rule, "rate_limit");
        assert_eq!(results[7].rule, "agent_health");
        assert_eq!(results[8].rule, "task_compatible");
        assert_eq!(results[9].rule, "sla_feasible");
    }

    #[test]
    fn check_all_all_pass_happy_path() {
        let task = default_task();
        let agent = active_agent();
        let system = empty_system();
        let thresholds = default_thresholds();

        let results = check_all_invariants(&task, &agent, &system, &thresholds);
        assert!(results.iter().all(|r| r.passed));
        assert!(!has_critical_failure(&results));
    }

    #[test]
    fn check_all_critical_failure_detected() {
        let task = default_task();
        let agent = AgentContext {
            state: AgentState::Inactive,
            ..active_agent()
        };
        let system = empty_system();
        let thresholds = default_thresholds();

        let results = check_all_invariants(&task, &agent, &system, &thresholds);
        assert_eq!(results.len(), 10);
        assert!(has_critical_failure(&results));
        assert!(!results[0].passed); // agent_available failed
    }

    #[test]
    fn check_all_warning_does_not_count_as_critical() {
        let mut task = default_task();
        task.task_type = TaskType::Research; // not supported by agent
        let agent = active_agent();
        let system = empty_system();
        let thresholds = default_thresholds();

        let results = check_all_invariants(&task, &agent, &system, &thresholds);
        assert_eq!(results.len(), 10);
        // task_compatible fails with Warning severity
        assert!(!results[8].passed);
        assert_eq!(results[8].severity, Severity::Warning);
        // But no critical failure
        assert!(!has_critical_failure(&results));
    }

    // =====================================================================
    // has_critical_failure tests
    // =====================================================================

    #[test]
    fn has_critical_failure_empty() {
        assert!(!has_critical_failure(&[]));
    }

    #[test]
    fn has_critical_failure_only_warnings() {
        let results = vec![InvariantResult {
            rule: "test".to_string(),
            severity: Severity::Warning,
            passed: false,
            detail: "warning failure".to_string(),
        }];
        assert!(!has_critical_failure(&results));
    }

    #[test]
    fn has_critical_failure_critical_pass() {
        let results = vec![InvariantResult {
            rule: "test".to_string(),
            severity: Severity::Critical,
            passed: true,
            detail: "ok".to_string(),
        }];
        assert!(!has_critical_failure(&results));
    }

    // =====================================================================
    // Performance test: all 10 checks under 1ms
    // =====================================================================

    #[test]
    fn check_all_invariants_under_1ms() {
        let task = default_task();
        let agent = active_agent();
        let system = SystemContext {
            total_running_tasks: 3,
            running_scopes: vec![
                vec!["tests/".to_string()],
                vec!["docs/".to_string()],
                vec!["benches/".to_string()],
            ],
            running_branches: vec!["fix/bug-1".to_string(), "fix/bug-2".to_string()],
            budget_remaining_usd: Some(8.50),
            retry_count: 1,
            calls_per_minute: 30,
        };
        let thresholds = default_thresholds();

        let start = std::time::Instant::now();
        let results = check_all_invariants(&task, &agent, &system, &thresholds);
        let elapsed = start.elapsed();

        assert_eq!(results.len(), 10);
        assert!(
            elapsed.as_micros() < 1000,
            "Invariant checks took {}us, expected < 1000us (1ms)",
            elapsed.as_micros()
        );
    }

    // =====================================================================
    // Path overlap helper tests
    // =====================================================================

    #[test]
    fn paths_overlap_exact() {
        assert!(paths_overlap("src/main.rs", "src/main.rs"));
    }

    #[test]
    fn paths_overlap_dir_contains_file() {
        assert!(paths_overlap("src/", "src/main.rs"));
        assert!(paths_overlap("src/main.rs", "src/"));
    }

    #[test]
    fn paths_overlap_nested() {
        assert!(paths_overlap("src/", "src/core/lib.rs"));
    }

    #[test]
    fn paths_no_overlap_different() {
        assert!(!paths_overlap("src/main.rs", "tests/test.rs"));
        assert!(!paths_overlap("src/", "tests/"));
    }

    #[test]
    fn paths_no_overlap_similar_prefix() {
        assert!(!paths_overlap("src/", "src_old/file.rs"));
    }
}
