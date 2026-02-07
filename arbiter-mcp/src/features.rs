//! Feature vector builder: converts task + agent + system state into
//! the 22-dimensional float vector consumed by Decision Tree inference.
//!
//! The builder is pure computation — no I/O, no SQLite dependency.
//! All data is passed in via the [`AgentInfo`] and [`SystemState`] structs,
//! which callers populate from config + database queries.

use arbiter_core::types::{Constraints, TaskInput};

use crate::config::AgentConfig;

// ---------------------------------------------------------------------------
// Feature vector dimension
// ---------------------------------------------------------------------------

/// Number of features in the vector (matches sklearn tree n_features=22).
pub const FEATURE_DIM: usize = 22;

// ---------------------------------------------------------------------------
// Caps and defaults
// ---------------------------------------------------------------------------

const DEFAULT_SCOPE_SIZE: f64 = 1.0;
const MAX_SCOPE_SIZE: f64 = 100.0;

const DEFAULT_ESTIMATED_TOKENS: f64 = 50.0; // 50K tokens
const MAX_ESTIMATED_TOKENS: f64 = 200.0;

const DEFAULT_SLA_MINUTES: f64 = 120.0;
const MAX_SLA_MINUTES: f64 = 480.0;

const DEFAULT_SUCCESS_RATE: f64 = 0.5;
const DEFAULT_AVG_DURATION_MIN: f64 = 15.0;
const DEFAULT_AVG_COST_USD: f64 = 0.10;

const DEFAULT_BUDGET_REMAINING: f64 = 10.0;

const MAX_AVAILABLE_SLOTS: f64 = 10.0;
const MAX_RUNNING_TASKS: f64 = 10.0;
const MAX_RECENT_FAILURES: f64 = 50.0;
const MAX_AVG_DURATION: f64 = 480.0;
const MAX_AVG_COST: f64 = 100.0;
const MAX_TOTAL_RUNNING: f64 = 20.0;
const MAX_TOTAL_PENDING: f64 = 100.0;
const MAX_BUDGET: f64 = 1000.0;
const MAX_SCOPE_CONFLICTS: f64 = 10.0;

// ---------------------------------------------------------------------------
// Input structs
// ---------------------------------------------------------------------------

/// Agent information combining config and runtime stats.
///
/// Callers construct this from [`AgentConfig`] (static) plus SQLite
/// queries (dynamic). All stats fields are optional and default to
/// conservative priors when absent (e.g. new agent with no history).
#[derive(Debug, Clone)]
pub struct AgentInfo {
    /// Agent identifier (e.g. "claude_code").
    pub agent_id: String,
    /// Static agent config from agents.toml.
    pub config: AgentConfig,
    /// Number of tasks currently running on this agent.
    pub running_tasks: u32,
    /// Lifetime success rate (successful / total). `None` → 0.5.
    pub success_rate: Option<f64>,
    /// Average task duration in minutes. `None` → config or 15.0.
    pub avg_duration_min: Option<f64>,
    /// Average cost per task in USD. `None` → config or 0.10.
    pub avg_cost_usd: Option<f64>,
    /// Number of failures in the last 24 hours.
    pub recent_failures: u32,
}

/// System-wide state for the feature vector.
///
/// Constructed from [`Constraints`] plus aggregate SQLite queries.
#[derive(Debug, Clone)]
pub struct SystemState {
    /// Orchestrator constraints (budget, pending tasks, running tasks).
    pub constraints: Constraints,
    /// Sum of running_tasks across all agents.
    pub total_running_tasks: u32,
    /// Current UTC hour (0–23).
    pub time_of_day_hour: u32,
}

// ---------------------------------------------------------------------------
// Feature vector builder
// ---------------------------------------------------------------------------

/// Build a 22-dimensional feature vector for one (task, agent) pair.
///
/// This is called once per candidate agent during `route_task`. The
/// resulting vector is fed into [`DecisionTree::predict`] for scoring.
///
/// # Feature layout
///
/// | Idx | Name                        | Source        |
/// |-----|-----------------------------|---------------|
/// | 0   | task_type                   | Task          |
/// | 1   | language                    | Task          |
/// | 2   | complexity                  | Task          |
/// | 3   | priority                    | Task          |
/// | 4   | scope_size                  | Task          |
/// | 5   | estimated_tokens            | Task          |
/// | 6   | has_dependencies            | Task          |
/// | 7   | requires_internet           | Task          |
/// | 8   | sla_minutes                 | Task          |
/// | 9   | agent_success_rate          | Agent stats   |
/// | 10  | agent_available_slots       | Agent runtime |
/// | 11  | agent_running_tasks         | Agent runtime |
/// | 12  | agent_avg_duration_min      | Agent stats   |
/// | 13  | agent_avg_cost_usd          | Agent stats   |
/// | 14  | agent_recent_failures       | Agent stats   |
/// | 15  | agent_supports_task_type    | Agent config  |
/// | 16  | agent_supports_language     | Agent config  |
/// | 17  | total_running_tasks         | System        |
/// | 18  | total_pending_tasks         | System        |
/// | 19  | budget_remaining_usd        | System        |
/// | 20  | time_of_day_hour            | System        |
/// | 21  | concurrent_scope_conflicts  | System        |
pub fn build_feature_vector(
    task: &TaskInput,
    agent: &AgentInfo,
    system: &SystemState,
) -> [f64; FEATURE_DIM] {
    let mut v = [0.0f64; FEATURE_DIM];

    // -- Task features (indices 0–8) --

    // [0] task_type: ordinal 0–6
    v[0] = task.task_type.as_ordinal();

    // [1] language: ordinal 0–5
    v[1] = task.language.as_ordinal();

    // [2] complexity: ordinal 0–4
    v[2] = task.complexity.as_ordinal();

    // [3] priority: ordinal 0–3
    v[3] = task.priority.as_ordinal();

    // [4] scope_size: count of paths, capped at 100
    let scope_size = if task.scope.is_empty() {
        DEFAULT_SCOPE_SIZE
    } else {
        (task.scope.len() as f64).min(MAX_SCOPE_SIZE)
    };
    v[4] = scope_size;

    // [5] estimated_tokens: raw / 1000, capped at 200
    let tokens = task
        .estimated_tokens
        .map(|t| (t as f64 / 1000.0).min(MAX_ESTIMATED_TOKENS))
        .unwrap_or(DEFAULT_ESTIMATED_TOKENS);
    v[5] = tokens;

    // [6] has_dependencies: boolean → 0.0 / 1.0
    v[6] = if task.has_dependencies { 1.0 } else { 0.0 };

    // [7] requires_internet: boolean → 0.0 / 1.0
    v[7] = if task.requires_internet { 1.0 } else { 0.0 };

    // [8] sla_minutes: capped at 480
    let sla = task
        .sla_minutes
        .map(|m| (m as f64).min(MAX_SLA_MINUTES))
        .unwrap_or(DEFAULT_SLA_MINUTES);
    v[8] = sla;

    // -- Agent features (indices 9–16) --

    // [9] agent_success_rate: [0.0, 1.0], default 0.5
    v[9] = agent
        .success_rate
        .unwrap_or(DEFAULT_SUCCESS_RATE)
        .clamp(0.0, 1.0);

    // [10] agent_available_slots: max_concurrent - running_tasks
    let available = (agent.config.max_concurrent as f64) - (agent.running_tasks as f64);
    v[10] = available.clamp(0.0, MAX_AVAILABLE_SLOTS);

    // [11] agent_running_tasks
    v[11] = (agent.running_tasks as f64).min(MAX_RUNNING_TASKS);

    // [12] agent_avg_duration_min
    v[12] = agent
        .avg_duration_min
        .unwrap_or(DEFAULT_AVG_DURATION_MIN)
        .clamp(0.0, MAX_AVG_DURATION);

    // [13] agent_avg_cost_usd
    v[13] = agent
        .avg_cost_usd
        .unwrap_or(DEFAULT_AVG_COST_USD)
        .clamp(0.0, MAX_AVG_COST);

    // [14] agent_recent_failures
    v[14] = (agent.recent_failures as f64).min(MAX_RECENT_FAILURES);

    // [15] agent_supports_task_type: 1.0 if supported
    let task_type_str = task.task_type.to_string();
    v[15] = if agent.config.supports_types.contains(&task_type_str) {
        1.0
    } else {
        0.0
    };

    // [16] agent_supports_language: 1.0 if supported
    let lang_str = task.language.to_string();
    v[16] = if agent.config.supports_languages.contains(&lang_str) {
        1.0
    } else {
        0.0
    };

    // -- System features (indices 17–21) --

    // [17] total_running_tasks
    v[17] = (system.total_running_tasks as f64).min(MAX_TOTAL_RUNNING);

    // [18] total_pending_tasks
    let pending = system.constraints.total_pending_tasks.unwrap_or(0);
    v[18] = (pending as f64).min(MAX_TOTAL_PENDING);

    // [19] budget_remaining_usd
    let budget = system
        .constraints
        .budget_remaining_usd
        .unwrap_or(DEFAULT_BUDGET_REMAINING);
    v[19] = budget.clamp(0.0, MAX_BUDGET);

    // [20] time_of_day_hour: 0–23
    v[20] = (system.time_of_day_hour as f64).clamp(0.0, 23.0);

    // [21] concurrent_scope_conflicts
    let conflicts = count_scope_conflicts(&task.scope, &system.constraints);
    v[21] = (conflicts as f64).min(MAX_SCOPE_CONFLICTS);

    v
}

/// Count how many running tasks have overlapping scope with the given task.
///
/// Two scopes overlap if:
/// - Either path is a prefix of the other (directory containment), or
/// - The paths are identical.
fn count_scope_conflicts(task_scope: &[String], constraints: &Constraints) -> u32 {
    if task_scope.is_empty() {
        return 0;
    }

    let mut conflicts = 0u32;
    for running in &constraints.running_tasks {
        if scopes_overlap(task_scope, &running.scope) {
            conflicts += 1;
        }
    }
    conflicts
}

/// Check if two scope lists have any overlapping paths.
fn scopes_overlap(a: &[String], b: &[String]) -> bool {
    for pa in a {
        for pb in b {
            if paths_overlap(pa, pb) {
                return true;
            }
        }
    }
    false
}

/// Two paths overlap if one is a prefix of the other or they are equal.
fn paths_overlap(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    // Normalize: ensure trailing slash for prefix check
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
    use arbiter_core::types::{
        Complexity, Constraints, Language, Priority, RunningTask, TaskInput, TaskType,
    };

    fn test_agent_config() -> AgentConfig {
        AgentConfig {
            display_name: "Claude Code".to_string(),
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
            max_concurrent: 2,
            cost_per_hour: 0.30,
            avg_duration_min: 18.0,
        }
    }

    fn full_task() -> TaskInput {
        TaskInput {
            task_type: TaskType::Feature,
            language: Language::Rust,
            complexity: Complexity::Complex,
            priority: Priority::High,
            scope: vec!["src/main.rs".to_string(), "src/lib.rs".to_string()],
            branch: Some("feature/new-thing".to_string()),
            estimated_tokens: Some(50000),
            has_dependencies: true,
            requires_internet: false,
            sla_minutes: Some(120),
            description: Some("Implement new feature".to_string()),
        }
    }

    fn minimal_task() -> TaskInput {
        TaskInput {
            task_type: TaskType::Bugfix,
            language: Language::Python,
            complexity: Complexity::Simple,
            priority: Priority::Normal,
            scope: vec![],
            branch: None,
            estimated_tokens: None,
            has_dependencies: false,
            requires_internet: false,
            sla_minutes: None,
            description: None,
        }
    }

    fn full_agent() -> AgentInfo {
        AgentInfo {
            agent_id: "claude_code".to_string(),
            config: test_agent_config(),
            running_tasks: 0,
            success_rate: Some(0.85),
            avg_duration_min: Some(18.0),
            avg_cost_usd: Some(0.30),
            recent_failures: 0,
        }
    }

    fn new_agent() -> AgentInfo {
        AgentInfo {
            agent_id: "new_agent".to_string(),
            config: test_agent_config(),
            running_tasks: 0,
            success_rate: None,
            avg_duration_min: None,
            avg_cost_usd: None,
            recent_failures: 0,
        }
    }

    fn default_system() -> SystemState {
        SystemState {
            constraints: Constraints {
                preferred_agent: None,
                excluded_agents: vec![],
                budget_remaining_usd: Some(8.50),
                total_pending_tasks: Some(3),
                running_tasks: vec![],
            },
            total_running_tasks: 1,
            time_of_day_hour: 14,
        }
    }

    // -- UT-01: Full task → 22-dim vector --

    #[test]
    fn full_task_produces_22_dim_vector() {
        let task = full_task();
        let agent = full_agent();
        let system = default_system();

        let v = build_feature_vector(&task, &agent, &system);

        assert_eq!(v.len(), FEATURE_DIM);

        // task_type: Feature → 0.0
        assert_eq!(v[0], 0.0);
        // language: Rust → 1.0
        assert_eq!(v[1], 1.0);
        // complexity: Complex → 3.0
        assert_eq!(v[2], 3.0);
        // priority: High → 2.0
        assert_eq!(v[3], 2.0);
        // scope_size: 2 files
        assert_eq!(v[4], 2.0);
        // estimated_tokens: 50000 / 1000 = 50.0
        assert_eq!(v[5], 50.0);
        // has_dependencies: true → 1.0
        assert_eq!(v[6], 1.0);
        // requires_internet: false → 0.0
        assert_eq!(v[7], 0.0);
        // sla_minutes: 120
        assert_eq!(v[8], 120.0);
        // agent_success_rate: 0.85
        assert_eq!(v[9], 0.85);
        // agent_available_slots: 2 - 0 = 2
        assert_eq!(v[10], 2.0);
        // agent_running_tasks: 0
        assert_eq!(v[11], 0.0);
        // agent_avg_duration_min: 18.0
        assert_eq!(v[12], 18.0);
        // agent_avg_cost_usd: 0.30
        assert_eq!(v[13], 0.30);
        // agent_recent_failures: 0
        assert_eq!(v[14], 0.0);
        // agent_supports_task_type: "feature" in types → 1.0
        assert_eq!(v[15], 1.0);
        // agent_supports_language: "rust" in langs → 1.0
        assert_eq!(v[16], 1.0);
        // total_running_tasks: 1
        assert_eq!(v[17], 1.0);
        // total_pending_tasks: 3
        assert_eq!(v[18], 3.0);
        // budget_remaining_usd: 8.50
        assert_eq!(v[19], 8.50);
        // time_of_day_hour: 14
        assert_eq!(v[20], 14.0);
        // concurrent_scope_conflicts: 0 (no running tasks)
        assert_eq!(v[21], 0.0);
    }

    // -- UT-02: Minimal task → defaults --

    #[test]
    fn minimal_task_uses_defaults() {
        let task = minimal_task();
        let agent = new_agent();
        let system = SystemState {
            constraints: Constraints {
                preferred_agent: None,
                excluded_agents: vec![],
                budget_remaining_usd: None,
                total_pending_tasks: None,
                running_tasks: vec![],
            },
            total_running_tasks: 0,
            time_of_day_hour: 0,
        };

        let v = build_feature_vector(&task, &agent, &system);

        assert_eq!(v.len(), FEATURE_DIM);

        // task_type: Bugfix → 1.0
        assert_eq!(v[0], 1.0);
        // language: Python → 0.0
        assert_eq!(v[1], 0.0);
        // complexity: Simple → 1.0
        assert_eq!(v[2], 1.0);
        // priority: Normal → 1.0
        assert_eq!(v[3], 1.0);
        // scope_size: empty → default 1.0
        assert_eq!(v[4], DEFAULT_SCOPE_SIZE);
        // estimated_tokens: None → default 50.0
        assert_eq!(v[5], DEFAULT_ESTIMATED_TOKENS);
        // has_dependencies: false → 0.0
        assert_eq!(v[6], 0.0);
        // requires_internet: false → 0.0
        assert_eq!(v[7], 0.0);
        // sla_minutes: None → default 120.0
        assert_eq!(v[8], DEFAULT_SLA_MINUTES);
        // agent_success_rate: None → default 0.5
        assert_eq!(v[9], DEFAULT_SUCCESS_RATE);
        // agent_available_slots: 2 - 0 = 2
        assert_eq!(v[10], 2.0);
        // agent_running_tasks: 0
        assert_eq!(v[11], 0.0);
        // agent_avg_duration_min: None → default 15.0
        assert_eq!(v[12], DEFAULT_AVG_DURATION_MIN);
        // agent_avg_cost_usd: None → default 0.10
        assert_eq!(v[13], DEFAULT_AVG_COST_USD);
        // agent_recent_failures: 0
        assert_eq!(v[14], 0.0);
        // agent_supports_task_type: "bugfix" in types → 1.0
        assert_eq!(v[15], 1.0);
        // agent_supports_language: "python" in langs → 1.0
        assert_eq!(v[16], 1.0);
        // total_running_tasks: 0
        assert_eq!(v[17], 0.0);
        // total_pending_tasks: None → 0
        assert_eq!(v[18], 0.0);
        // budget_remaining_usd: None → default 10.0
        assert_eq!(v[19], DEFAULT_BUDGET_REMAINING);
        // time_of_day_hour: 0
        assert_eq!(v[20], 0.0);
        // concurrent_scope_conflicts: 0
        assert_eq!(v[21], 0.0);
    }

    // -- UT-03: Unknown type / unsupported task --

    #[test]
    fn unsupported_task_type_encodes_zero() {
        let task = TaskInput {
            task_type: TaskType::Research, // not in agent's supports_types
            language: Language::Rust,
            complexity: Complexity::Simple,
            priority: Priority::Normal,
            scope: vec![],
            branch: None,
            estimated_tokens: None,
            has_dependencies: false,
            requires_internet: false,
            sla_minutes: None,
            description: None,
        };
        let agent = full_agent();
        let system = default_system();

        let v = build_feature_vector(&task, &agent, &system);

        // task_type: Research → 6.0
        assert_eq!(v[0], 6.0);
        // agent doesn't support "research" → 0.0
        assert_eq!(v[15], 0.0);
    }

    #[test]
    fn unsupported_language_encodes_zero() {
        let task = TaskInput {
            task_type: TaskType::Feature,
            language: Language::Go, // not in agent's supports_languages
            complexity: Complexity::Simple,
            priority: Priority::Normal,
            scope: vec![],
            branch: None,
            estimated_tokens: None,
            has_dependencies: false,
            requires_internet: false,
            sla_minutes: None,
            description: None,
        };
        let agent = full_agent();
        let system = default_system();

        let v = build_feature_vector(&task, &agent, &system);

        // language: Go → 3.0
        assert_eq!(v[1], 3.0);
        // agent doesn't support "go" → 0.0
        assert_eq!(v[16], 0.0);
    }

    // -- Value range tests --

    #[test]
    fn all_values_in_documented_ranges() {
        let task = full_task();
        let agent = full_agent();
        let system = default_system();

        let v = build_feature_vector(&task, &agent, &system);

        assert!((0.0..=6.0).contains(&v[0]), "task_type out of range");
        assert!((0.0..=5.0).contains(&v[1]), "language out of range");
        assert!((0.0..=4.0).contains(&v[2]), "complexity out of range");
        assert!((0.0..=3.0).contains(&v[3]), "priority out of range");
        assert!(
            (0.0..=MAX_SCOPE_SIZE).contains(&v[4]),
            "scope_size out of range"
        );
        assert!(
            (0.0..=MAX_ESTIMATED_TOKENS).contains(&v[5]),
            "estimated_tokens out of range"
        );
        assert!(v[6] == 0.0 || v[6] == 1.0, "has_dependencies not boolean");
        assert!(v[7] == 0.0 || v[7] == 1.0, "requires_internet not boolean");
        assert!(
            (0.0..=MAX_SLA_MINUTES).contains(&v[8]),
            "sla_minutes out of range"
        );
        assert!((0.0..=1.0).contains(&v[9]), "success_rate out of range");
        assert!(
            (0.0..=MAX_AVAILABLE_SLOTS).contains(&v[10]),
            "available_slots out of range"
        );
        assert!(
            (0.0..=MAX_RUNNING_TASKS).contains(&v[11]),
            "running_tasks out of range"
        );
        assert!(
            (0.0..=MAX_AVG_DURATION).contains(&v[12]),
            "avg_duration out of range"
        );
        assert!(
            (0.0..=MAX_AVG_COST).contains(&v[13]),
            "avg_cost out of range"
        );
        assert!(
            (0.0..=MAX_RECENT_FAILURES).contains(&v[14]),
            "recent_failures out of range"
        );
        assert!(
            v[15] == 0.0 || v[15] == 1.0,
            "supports_task_type not boolean"
        );
        assert!(
            v[16] == 0.0 || v[16] == 1.0,
            "supports_language not boolean"
        );
        assert!(
            (0.0..=MAX_TOTAL_RUNNING).contains(&v[17]),
            "total_running out of range"
        );
        assert!(
            (0.0..=MAX_TOTAL_PENDING).contains(&v[18]),
            "total_pending out of range"
        );
        assert!((0.0..=MAX_BUDGET).contains(&v[19]), "budget out of range");
        assert!((0.0..=23.0).contains(&v[20]), "time_of_day out of range");
        assert!(
            (0.0..=MAX_SCOPE_CONFLICTS).contains(&v[21]),
            "scope_conflicts out of range"
        );
    }

    // -- Capping tests --

    #[test]
    fn scope_size_capped_at_100() {
        let mut task = full_task();
        task.scope = (0..150).map(|i| format!("file_{i}.rs")).collect();
        let agent = full_agent();
        let system = default_system();

        let v = build_feature_vector(&task, &agent, &system);
        assert_eq!(v[4], MAX_SCOPE_SIZE);
    }

    #[test]
    fn estimated_tokens_capped_at_200() {
        let mut task = full_task();
        task.estimated_tokens = Some(500_000); // 500K → 500.0 uncapped
        let agent = full_agent();
        let system = default_system();

        let v = build_feature_vector(&task, &agent, &system);
        assert_eq!(v[5], MAX_ESTIMATED_TOKENS);
    }

    #[test]
    fn sla_minutes_capped_at_480() {
        let mut task = full_task();
        task.sla_minutes = Some(1000);
        let agent = full_agent();
        let system = default_system();

        let v = build_feature_vector(&task, &agent, &system);
        assert_eq!(v[8], MAX_SLA_MINUTES);
    }

    #[test]
    fn success_rate_clamped_to_0_1() {
        let mut agent = full_agent();
        agent.success_rate = Some(1.5);
        let task = full_task();
        let system = default_system();

        let v = build_feature_vector(&task, &agent, &system);
        assert_eq!(v[9], 1.0);

        agent.success_rate = Some(-0.5);
        let v = build_feature_vector(&task, &agent, &system);
        assert_eq!(v[9], 0.0);
    }

    // -- Scope conflict tests --

    #[test]
    fn scope_conflict_exact_match() {
        let task = TaskInput {
            task_type: TaskType::Feature,
            language: Language::Rust,
            complexity: Complexity::Simple,
            priority: Priority::Normal,
            scope: vec!["src/main.rs".to_string()],
            branch: None,
            estimated_tokens: None,
            has_dependencies: false,
            requires_internet: false,
            sla_minutes: None,
            description: None,
        };
        let agent = full_agent();
        let system = SystemState {
            constraints: Constraints {
                preferred_agent: None,
                excluded_agents: vec![],
                budget_remaining_usd: Some(10.0),
                total_pending_tasks: Some(0),
                running_tasks: vec![RunningTask {
                    task_id: "task-42".to_string(),
                    agent_id: "codex_cli".to_string(),
                    scope: vec!["src/main.rs".to_string()],
                    branch: None,
                }],
            },
            total_running_tasks: 1,
            time_of_day_hour: 14,
        };

        let v = build_feature_vector(&task, &agent, &system);
        assert_eq!(v[21], 1.0);
    }

    #[test]
    fn scope_conflict_directory_containment() {
        let task = TaskInput {
            task_type: TaskType::Feature,
            language: Language::Rust,
            complexity: Complexity::Simple,
            priority: Priority::Normal,
            scope: vec!["src/main.rs".to_string()],
            branch: None,
            estimated_tokens: None,
            has_dependencies: false,
            requires_internet: false,
            sla_minutes: None,
            description: None,
        };
        let agent = full_agent();
        let system = SystemState {
            constraints: Constraints {
                preferred_agent: None,
                excluded_agents: vec![],
                budget_remaining_usd: Some(10.0),
                total_pending_tasks: Some(0),
                running_tasks: vec![RunningTask {
                    task_id: "task-43".to_string(),
                    agent_id: "aider".to_string(),
                    scope: vec!["src/".to_string()],
                    branch: None,
                }],
            },
            total_running_tasks: 1,
            time_of_day_hour: 14,
        };

        let v = build_feature_vector(&task, &agent, &system);
        assert_eq!(v[21], 1.0);
    }

    #[test]
    fn scope_no_conflict() {
        let task = TaskInput {
            task_type: TaskType::Feature,
            language: Language::Rust,
            complexity: Complexity::Simple,
            priority: Priority::Normal,
            scope: vec!["tests/test_main.rs".to_string()],
            branch: None,
            estimated_tokens: None,
            has_dependencies: false,
            requires_internet: false,
            sla_minutes: None,
            description: None,
        };
        let agent = full_agent();
        let system = SystemState {
            constraints: Constraints {
                preferred_agent: None,
                excluded_agents: vec![],
                budget_remaining_usd: Some(10.0),
                total_pending_tasks: Some(0),
                running_tasks: vec![RunningTask {
                    task_id: "task-43".to_string(),
                    agent_id: "aider".to_string(),
                    scope: vec!["src/".to_string()],
                    branch: None,
                }],
            },
            total_running_tasks: 1,
            time_of_day_hour: 14,
        };

        let v = build_feature_vector(&task, &agent, &system);
        assert_eq!(v[21], 0.0);
    }

    #[test]
    fn scope_conflicts_capped_at_10() {
        let task = TaskInput {
            task_type: TaskType::Feature,
            language: Language::Rust,
            complexity: Complexity::Simple,
            priority: Priority::Normal,
            scope: vec!["src/".to_string()],
            branch: None,
            estimated_tokens: None,
            has_dependencies: false,
            requires_internet: false,
            sla_minutes: None,
            description: None,
        };
        let agent = full_agent();

        // 15 running tasks all touching src/
        let running: Vec<RunningTask> = (0..15)
            .map(|i| RunningTask {
                task_id: format!("task-{i}"),
                agent_id: "aider".to_string(),
                scope: vec!["src/lib.rs".to_string()],
                branch: None,
            })
            .collect();

        let system = SystemState {
            constraints: Constraints {
                preferred_agent: None,
                excluded_agents: vec![],
                budget_remaining_usd: Some(10.0),
                total_pending_tasks: Some(0),
                running_tasks: running,
            },
            total_running_tasks: 15,
            time_of_day_hour: 14,
        };

        let v = build_feature_vector(&task, &agent, &system);
        assert_eq!(v[21], MAX_SCOPE_CONFLICTS);
    }

    // -- Agent busy / at capacity --

    #[test]
    fn agent_at_capacity_has_zero_slots() {
        let mut agent = full_agent();
        agent.running_tasks = 2; // max_concurrent = 2
        let task = full_task();
        let system = default_system();

        let v = build_feature_vector(&task, &agent, &system);
        assert_eq!(v[10], 0.0); // available_slots
        assert_eq!(v[11], 2.0); // running_tasks
    }

    #[test]
    fn agent_over_capacity_clamps_to_zero_slots() {
        let mut agent = full_agent();
        agent.running_tasks = 5; // > max_concurrent of 2
        let task = full_task();
        let system = default_system();

        let v = build_feature_vector(&task, &agent, &system);
        assert_eq!(v[10], 0.0); // available_slots can't be negative
    }

    // -- All ordinal encodings --

    #[test]
    fn all_task_type_ordinals() {
        let types = [
            (TaskType::Feature, 0.0),
            (TaskType::Bugfix, 1.0),
            (TaskType::Refactor, 2.0),
            (TaskType::Test, 3.0),
            (TaskType::Docs, 4.0),
            (TaskType::Review, 5.0),
            (TaskType::Research, 6.0),
        ];
        for (tt, expected) in types {
            let task = TaskInput {
                task_type: tt,
                language: Language::Python,
                complexity: Complexity::Simple,
                priority: Priority::Normal,
                scope: vec![],
                branch: None,
                estimated_tokens: None,
                has_dependencies: false,
                requires_internet: false,
                sla_minutes: None,
                description: None,
            };
            let v = build_feature_vector(&task, &full_agent(), &default_system());
            assert_eq!(
                v[0], expected,
                "task_type {tt:?} should encode as {expected}"
            );
        }
    }

    #[test]
    fn all_language_ordinals() {
        let langs = [
            (Language::Python, 0.0),
            (Language::Rust, 1.0),
            (Language::Typescript, 2.0),
            (Language::Go, 3.0),
            (Language::Mixed, 4.0),
            (Language::Other, 5.0),
        ];
        for (lang, expected) in langs {
            let task = TaskInput {
                task_type: TaskType::Feature,
                language: lang,
                complexity: Complexity::Simple,
                priority: Priority::Normal,
                scope: vec![],
                branch: None,
                estimated_tokens: None,
                has_dependencies: false,
                requires_internet: false,
                sla_minutes: None,
                description: None,
            };
            let v = build_feature_vector(&task, &full_agent(), &default_system());
            assert_eq!(
                v[1], expected,
                "language {lang:?} should encode as {expected}"
            );
        }
    }

    #[test]
    fn all_complexity_ordinals() {
        let comps = [
            (Complexity::Trivial, 0.0),
            (Complexity::Simple, 1.0),
            (Complexity::Moderate, 2.0),
            (Complexity::Complex, 3.0),
            (Complexity::Critical, 4.0),
        ];
        for (c, expected) in comps {
            let task = TaskInput {
                task_type: TaskType::Feature,
                language: Language::Python,
                complexity: c,
                priority: Priority::Normal,
                scope: vec![],
                branch: None,
                estimated_tokens: None,
                has_dependencies: false,
                requires_internet: false,
                sla_minutes: None,
                description: None,
            };
            let v = build_feature_vector(&task, &full_agent(), &default_system());
            assert_eq!(
                v[2], expected,
                "complexity {c:?} should encode as {expected}"
            );
        }
    }

    #[test]
    fn all_priority_ordinals() {
        let prios = [
            (Priority::Low, 0.0),
            (Priority::Normal, 1.0),
            (Priority::High, 2.0),
            (Priority::Urgent, 3.0),
        ];
        for (p, expected) in prios {
            let task = TaskInput {
                task_type: TaskType::Feature,
                language: Language::Python,
                complexity: Complexity::Simple,
                priority: p,
                scope: vec![],
                branch: None,
                estimated_tokens: None,
                has_dependencies: false,
                requires_internet: false,
                sla_minutes: None,
                description: None,
            };
            let v = build_feature_vector(&task, &full_agent(), &default_system());
            assert_eq!(v[3], expected, "priority {p:?} should encode as {expected}");
        }
    }

    // -- Boolean feature tests --

    #[test]
    fn boolean_features_encode_correctly() {
        let mut task = minimal_task();
        let agent = full_agent();
        let system = default_system();

        // Both false
        let v = build_feature_vector(&task, &agent, &system);
        assert_eq!(v[6], 0.0);
        assert_eq!(v[7], 0.0);

        // Both true
        task.has_dependencies = true;
        task.requires_internet = true;
        let v = build_feature_vector(&task, &agent, &system);
        assert_eq!(v[6], 1.0);
        assert_eq!(v[7], 1.0);
    }

    // -- Multiple agents produce different vectors --

    #[test]
    fn different_agents_produce_different_vectors() {
        let task = full_task();
        let system = default_system();

        let agent1 = AgentInfo {
            agent_id: "claude_code".to_string(),
            config: test_agent_config(),
            running_tasks: 0,
            success_rate: Some(0.9),
            avg_duration_min: Some(18.0),
            avg_cost_usd: Some(0.30),
            recent_failures: 0,
        };

        let agent2 = AgentInfo {
            agent_id: "aider".to_string(),
            config: AgentConfig {
                display_name: "Aider".to_string(),
                supports_languages: vec!["python".to_string()],
                supports_types: vec!["bugfix".to_string()],
                max_concurrent: 5,
                cost_per_hour: 0.10,
                avg_duration_min: 8.0,
            },
            running_tasks: 2,
            success_rate: Some(0.7),
            avg_duration_min: Some(8.0),
            avg_cost_usd: Some(0.05),
            recent_failures: 1,
        };

        let v1 = build_feature_vector(&task, &agent1, &system);
        let v2 = build_feature_vector(&task, &agent2, &system);

        // Task features (0-8) should be identical
        for i in 0..9 {
            assert_eq!(
                v1[i], v2[i],
                "task feature {i} should be same for both agents"
            );
        }

        // Agent features (9-16) should differ
        assert_ne!(v1[9], v2[9]); // success_rate
        assert_ne!(v1[10], v2[10]); // available_slots
        assert_ne!(v1[11], v2[11]); // running_tasks
        assert_ne!(v1[15], v2[15]); // supports_task_type (aider: no "feature")
        assert_ne!(v1[16], v2[16]); // supports_language (aider: no "rust")
    }

    // -- Path overlap logic --

    #[test]
    fn paths_overlap_exact_match() {
        assert!(paths_overlap("src/main.rs", "src/main.rs"));
    }

    #[test]
    fn paths_overlap_directory_contains_file() {
        assert!(paths_overlap("src/", "src/main.rs"));
        assert!(paths_overlap("src/main.rs", "src/"));
    }

    #[test]
    fn paths_overlap_nested_directories() {
        assert!(paths_overlap("src/", "src/core/lib.rs"));
        assert!(paths_overlap("src/core/lib.rs", "src/"));
    }

    #[test]
    fn paths_no_overlap_different_dirs() {
        assert!(!paths_overlap("src/main.rs", "tests/test.rs"));
        assert!(!paths_overlap("src/", "tests/"));
    }

    #[test]
    fn paths_no_overlap_similar_prefix() {
        // "src_old/" is not a child of "src/"
        assert!(!paths_overlap("src/", "src_old/file.rs"));
    }

    // -- Edge case: empty scope means no conflicts --

    #[test]
    fn empty_task_scope_no_conflicts() {
        let task = minimal_task(); // empty scope
        let agent = full_agent();
        let system = SystemState {
            constraints: Constraints {
                preferred_agent: None,
                excluded_agents: vec![],
                budget_remaining_usd: Some(10.0),
                total_pending_tasks: Some(0),
                running_tasks: vec![RunningTask {
                    task_id: "task-1".to_string(),
                    agent_id: "aider".to_string(),
                    scope: vec!["src/".to_string()],
                    branch: None,
                }],
            },
            total_running_tasks: 1,
            time_of_day_hour: 14,
        };

        let v = build_feature_vector(&task, &agent, &system);
        assert_eq!(v[21], 0.0);
    }
}
