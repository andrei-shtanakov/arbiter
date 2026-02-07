//! Core data types for the Arbiter policy engine.
//!
//! All types here are pure data — no I/O, no database, no network.
//! They are shared across arbiter-core and arbiter-mcp.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Action decided by the routing engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentAction {
    /// Task assigned to an agent.
    Assign,
    /// Task rejected — no agent can handle it safely.
    Reject,
    /// Primary agent failed invariants; assigned to a fallback agent.
    Fallback,
}

/// Lifecycle state of a coding agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Agent is available and accepting tasks.
    Active,
    /// Agent is disabled / not accepting tasks.
    Inactive,
    /// Agent is at max concurrency — no available slots.
    Busy,
    /// Agent exceeded failure threshold — requires manual recovery.
    Failed,
}

/// Severity level for invariant rule results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Severity {
    /// Blocks assignment and triggers cascade fallback.
    Critical,
    /// Logged but does not block assignment.
    Warning,
}

/// Task type classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    Feature,
    Bugfix,
    Refactor,
    Test,
    Docs,
    Review,
    Research,
}

/// Programming language classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Python,
    Rust,
    Typescript,
    Go,
    Mixed,
    Other,
}

/// Task complexity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Complexity {
    Trivial,
    Simple,
    Moderate,
    Complex,
    Critical,
}

/// Task priority level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    Normal,
    High,
    Urgent,
}

// ---------------------------------------------------------------------------
// Ordinal encoding for feature vector construction
// ---------------------------------------------------------------------------

impl TaskType {
    /// Ordinal encoding for the 22-dim feature vector (index 0).
    pub fn as_ordinal(&self) -> f64 {
        match self {
            Self::Feature => 0.0,
            Self::Bugfix => 1.0,
            Self::Refactor => 2.0,
            Self::Test => 3.0,
            Self::Docs => 4.0,
            Self::Review => 5.0,
            Self::Research => 6.0,
        }
    }
}

impl Language {
    /// Ordinal encoding for the 22-dim feature vector (index 1).
    pub fn as_ordinal(&self) -> f64 {
        match self {
            Self::Python => 0.0,
            Self::Rust => 1.0,
            Self::Typescript => 2.0,
            Self::Go => 3.0,
            Self::Mixed => 4.0,
            Self::Other => 5.0,
        }
    }
}

impl Complexity {
    /// Ordinal encoding for the 22-dim feature vector (index 2).
    pub fn as_ordinal(&self) -> f64 {
        match self {
            Self::Trivial => 0.0,
            Self::Simple => 1.0,
            Self::Moderate => 2.0,
            Self::Complex => 3.0,
            Self::Critical => 4.0,
        }
    }
}

impl Priority {
    /// Ordinal encoding for the 22-dim feature vector (index 3).
    pub fn as_ordinal(&self) -> f64 {
        match self {
            Self::Low => 0.0,
            Self::Normal => 1.0,
            Self::High => 2.0,
            Self::Urgent => 3.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// Result of a single invariant rule check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvariantResult {
    /// Rule identifier (e.g. "agent_available", "scope_isolation").
    pub rule: String,
    /// Whether a failure blocks assignment or is just a warning.
    pub severity: Severity,
    /// Whether the rule passed.
    pub passed: bool,
    /// Human-readable detail about the check outcome.
    pub detail: String,
}

/// Description of a task to be routed to an agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskInput {
    /// Task type classification.
    #[serde(rename = "type")]
    pub task_type: TaskType,
    /// Primary programming language.
    pub language: Language,
    /// Complexity assessment.
    pub complexity: Complexity,
    /// Priority level.
    pub priority: Priority,
    /// File/directory paths affected by this task.
    #[serde(default)]
    pub scope: Vec<String>,
    /// Git branch name for this task.
    #[serde(default)]
    pub branch: Option<String>,
    /// Estimated token usage.
    #[serde(default)]
    pub estimated_tokens: Option<u64>,
    /// Whether the task depends on other tasks.
    #[serde(default)]
    pub has_dependencies: bool,
    /// Whether the task requires internet access.
    #[serde(default)]
    pub requires_internet: bool,
    /// SLA deadline in minutes.
    #[serde(default)]
    pub sla_minutes: Option<u32>,
    /// Human-readable task description.
    #[serde(default)]
    pub description: Option<String>,
}

/// Constraints provided by the orchestrator alongside a routing request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Constraints {
    /// Prefer this agent if it scores well.
    #[serde(default)]
    pub preferred_agent: Option<String>,
    /// Never assign to these agents.
    #[serde(default)]
    pub excluded_agents: Vec<String>,
    /// Remaining budget in USD.
    #[serde(default)]
    pub budget_remaining_usd: Option<f64>,
    /// Number of tasks waiting in queue.
    #[serde(default)]
    pub total_pending_tasks: Option<u32>,
    /// Tasks currently being executed by agents.
    #[serde(default)]
    pub running_tasks: Vec<RunningTask>,
}

/// A task currently being executed by an agent (used for scope/branch conflict checks).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunningTask {
    /// Unique task identifier.
    pub task_id: String,
    /// Agent executing this task.
    pub agent_id: String,
    /// File/directory paths this task is working on.
    #[serde(default)]
    pub scope: Vec<String>,
    /// Git branch this task is using.
    #[serde(default)]
    pub branch: Option<String>,
}

/// Result of Decision Tree inference for a single agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PredictionResult {
    /// Agent class index from the decision tree.
    pub class: usize,
    /// Confidence score in [0, 1].
    pub confidence: f64,
    /// Decision path through the tree (for audit trail).
    pub path: Vec<String>,
}

// ---------------------------------------------------------------------------
// Display implementations
// ---------------------------------------------------------------------------

impl std::fmt::Display for AgentAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Assign => write!(f, "assign"),
            Self::Reject => write!(f, "reject"),
            Self::Fallback => write!(f, "fallback"),
        }
    }
}

impl std::fmt::Display for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Inactive => write!(f, "inactive"),
            Self::Busy => write!(f, "busy"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Critical => write!(f, "Critical"),
            Self::Warning => write!(f, "Warning"),
        }
    }
}

impl std::fmt::Display for TaskType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Feature => write!(f, "feature"),
            Self::Bugfix => write!(f, "bugfix"),
            Self::Refactor => write!(f, "refactor"),
            Self::Test => write!(f, "test"),
            Self::Docs => write!(f, "docs"),
            Self::Review => write!(f, "review"),
            Self::Research => write!(f, "research"),
        }
    }
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Python => write!(f, "python"),
            Self::Rust => write!(f, "rust"),
            Self::Typescript => write!(f, "typescript"),
            Self::Go => write!(f, "go"),
            Self::Mixed => write!(f, "mixed"),
            Self::Other => write!(f, "other"),
        }
    }
}

impl std::fmt::Display for Complexity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Trivial => write!(f, "trivial"),
            Self::Simple => write!(f, "simple"),
            Self::Moderate => write!(f, "moderate"),
            Self::Complex => write!(f, "complex"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Normal => write!(f, "normal"),
            Self::High => write!(f, "high"),
            Self::Urgent => write!(f, "urgent"),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Enum serialization round-trip tests --

    #[test]
    fn agent_action_serialize_roundtrip() {
        for action in [
            AgentAction::Assign,
            AgentAction::Reject,
            AgentAction::Fallback,
        ] {
            let json = serde_json::to_string(&action).unwrap();
            let back: AgentAction = serde_json::from_str(&json).unwrap();
            assert_eq!(action, back);
        }
    }

    #[test]
    fn agent_action_variant_strings() {
        assert_eq!(
            serde_json::to_string(&AgentAction::Assign).unwrap(),
            "\"assign\""
        );
        assert_eq!(
            serde_json::to_string(&AgentAction::Reject).unwrap(),
            "\"reject\""
        );
        assert_eq!(
            serde_json::to_string(&AgentAction::Fallback).unwrap(),
            "\"fallback\""
        );
    }

    #[test]
    fn agent_state_serialize_roundtrip() {
        for state in [
            AgentState::Active,
            AgentState::Inactive,
            AgentState::Busy,
            AgentState::Failed,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            let back: AgentState = serde_json::from_str(&json).unwrap();
            assert_eq!(state, back);
        }
    }

    #[test]
    fn agent_state_variant_strings() {
        assert_eq!(
            serde_json::to_string(&AgentState::Active).unwrap(),
            "\"active\""
        );
        assert_eq!(
            serde_json::to_string(&AgentState::Inactive).unwrap(),
            "\"inactive\""
        );
        assert_eq!(
            serde_json::to_string(&AgentState::Busy).unwrap(),
            "\"busy\""
        );
        assert_eq!(
            serde_json::to_string(&AgentState::Failed).unwrap(),
            "\"failed\""
        );
    }

    #[test]
    fn severity_serialize_roundtrip() {
        for severity in [Severity::Critical, Severity::Warning] {
            let json = serde_json::to_string(&severity).unwrap();
            let back: Severity = serde_json::from_str(&json).unwrap();
            assert_eq!(severity, back);
        }
    }

    #[test]
    fn severity_variant_strings() {
        assert_eq!(
            serde_json::to_string(&Severity::Critical).unwrap(),
            "\"Critical\""
        );
        assert_eq!(
            serde_json::to_string(&Severity::Warning).unwrap(),
            "\"Warning\""
        );
    }

    #[test]
    fn task_type_serialize_roundtrip() {
        for tt in [
            TaskType::Feature,
            TaskType::Bugfix,
            TaskType::Refactor,
            TaskType::Test,
            TaskType::Docs,
            TaskType::Review,
            TaskType::Research,
        ] {
            let json = serde_json::to_string(&tt).unwrap();
            let back: TaskType = serde_json::from_str(&json).unwrap();
            assert_eq!(tt, back);
        }
    }

    #[test]
    fn task_type_variant_strings() {
        assert_eq!(
            serde_json::to_string(&TaskType::Feature).unwrap(),
            "\"feature\""
        );
        assert_eq!(
            serde_json::to_string(&TaskType::Bugfix).unwrap(),
            "\"bugfix\""
        );
        assert_eq!(
            serde_json::to_string(&TaskType::Refactor).unwrap(),
            "\"refactor\""
        );
        assert_eq!(serde_json::to_string(&TaskType::Test).unwrap(), "\"test\"");
        assert_eq!(serde_json::to_string(&TaskType::Docs).unwrap(), "\"docs\"");
        assert_eq!(
            serde_json::to_string(&TaskType::Review).unwrap(),
            "\"review\""
        );
        assert_eq!(
            serde_json::to_string(&TaskType::Research).unwrap(),
            "\"research\""
        );
    }

    #[test]
    fn language_serialize_roundtrip() {
        for lang in [
            Language::Python,
            Language::Rust,
            Language::Typescript,
            Language::Go,
            Language::Mixed,
            Language::Other,
        ] {
            let json = serde_json::to_string(&lang).unwrap();
            let back: Language = serde_json::from_str(&json).unwrap();
            assert_eq!(lang, back);
        }
    }

    #[test]
    fn language_variant_strings() {
        assert_eq!(
            serde_json::to_string(&Language::Python).unwrap(),
            "\"python\""
        );
        assert_eq!(serde_json::to_string(&Language::Rust).unwrap(), "\"rust\"");
        assert_eq!(
            serde_json::to_string(&Language::Typescript).unwrap(),
            "\"typescript\""
        );
        assert_eq!(serde_json::to_string(&Language::Go).unwrap(), "\"go\"");
        assert_eq!(
            serde_json::to_string(&Language::Mixed).unwrap(),
            "\"mixed\""
        );
        assert_eq!(
            serde_json::to_string(&Language::Other).unwrap(),
            "\"other\""
        );
    }

    #[test]
    fn complexity_serialize_roundtrip() {
        for c in [
            Complexity::Trivial,
            Complexity::Simple,
            Complexity::Moderate,
            Complexity::Complex,
            Complexity::Critical,
        ] {
            let json = serde_json::to_string(&c).unwrap();
            let back: Complexity = serde_json::from_str(&json).unwrap();
            assert_eq!(c, back);
        }
    }

    #[test]
    fn priority_serialize_roundtrip() {
        for p in [
            Priority::Low,
            Priority::Normal,
            Priority::High,
            Priority::Urgent,
        ] {
            let json = serde_json::to_string(&p).unwrap();
            let back: Priority = serde_json::from_str(&json).unwrap();
            assert_eq!(p, back);
        }
    }

    // -- Struct serialization round-trip tests --

    #[test]
    fn invariant_result_serialize_roundtrip() {
        let result = InvariantResult {
            rule: "agent_available".to_string(),
            severity: Severity::Critical,
            passed: true,
            detail: "Agent has 2 available slots".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: InvariantResult = serde_json::from_str(&json).unwrap();
        assert_eq!(result, back);
    }

    #[test]
    fn task_input_full_serialize_roundtrip() {
        let task = TaskInput {
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
        };
        let json = serde_json::to_string(&task).unwrap();
        let back: TaskInput = serde_json::from_str(&json).unwrap();
        assert_eq!(task, back);
    }

    #[test]
    fn task_input_minimal_deserialize() {
        let json = r#"{
            "type": "bugfix",
            "language": "python",
            "complexity": "simple",
            "priority": "normal"
        }"#;
        let task: TaskInput = serde_json::from_str(json).unwrap();
        assert_eq!(task.task_type, TaskType::Bugfix);
        assert_eq!(task.language, Language::Python);
        assert_eq!(task.complexity, Complexity::Simple);
        assert_eq!(task.priority, Priority::Normal);
        assert!(task.scope.is_empty());
        assert!(task.branch.is_none());
        assert!(task.estimated_tokens.is_none());
        assert!(!task.has_dependencies);
        assert!(!task.requires_internet);
        assert!(task.sla_minutes.is_none());
        assert!(task.description.is_none());
    }

    #[test]
    fn task_input_type_field_renamed() {
        let task = TaskInput {
            task_type: TaskType::Feature,
            language: Language::Rust,
            complexity: Complexity::Trivial,
            priority: Priority::Low,
            scope: vec![],
            branch: None,
            estimated_tokens: None,
            has_dependencies: false,
            requires_internet: false,
            sla_minutes: None,
            description: None,
        };
        let json = serde_json::to_string(&task).unwrap();
        assert!(json.contains("\"type\":\"feature\""));
        assert!(!json.contains("\"task_type\""));
    }

    #[test]
    fn constraints_serialize_roundtrip() {
        let constraints = Constraints {
            preferred_agent: Some("claude_code".to_string()),
            excluded_agents: vec!["aider".to_string()],
            budget_remaining_usd: Some(8.50),
            total_pending_tasks: Some(3),
            running_tasks: vec![RunningTask {
                task_id: "task-42".to_string(),
                agent_id: "codex_cli".to_string(),
                scope: vec!["src/".to_string()],
                branch: Some("fix/bug-42".to_string()),
            }],
        };
        let json = serde_json::to_string(&constraints).unwrap();
        let back: Constraints = serde_json::from_str(&json).unwrap();
        assert_eq!(constraints, back);
    }

    #[test]
    fn constraints_empty_defaults() {
        let json = r#"{}"#;
        let constraints: Constraints = serde_json::from_str(json).unwrap();
        assert!(constraints.preferred_agent.is_none());
        assert!(constraints.excluded_agents.is_empty());
        assert!(constraints.budget_remaining_usd.is_none());
        assert!(constraints.total_pending_tasks.is_none());
        assert!(constraints.running_tasks.is_empty());
    }

    #[test]
    fn running_task_serialize_roundtrip() {
        let rt = RunningTask {
            task_id: "task-1".to_string(),
            agent_id: "claude_code".to_string(),
            scope: vec!["src/main.rs".to_string()],
            branch: Some("main".to_string()),
        };
        let json = serde_json::to_string(&rt).unwrap();
        let back: RunningTask = serde_json::from_str(&json).unwrap();
        assert_eq!(rt, back);
    }

    #[test]
    fn prediction_result_serialize_roundtrip() {
        let pred = PredictionResult {
            class: 0,
            confidence: 0.92,
            path: vec![
                "node 0: feature[2] <= 2.5".to_string(),
                "node 1: feature[0] <= 1.5".to_string(),
                "leaf: class 0".to_string(),
            ],
        };
        let json = serde_json::to_string(&pred).unwrap();
        let back: PredictionResult = serde_json::from_str(&json).unwrap();
        assert_eq!(pred, back);
    }

    // -- Ordinal encoding tests --

    #[test]
    fn task_type_ordinals() {
        assert_eq!(TaskType::Feature.as_ordinal(), 0.0);
        assert_eq!(TaskType::Bugfix.as_ordinal(), 1.0);
        assert_eq!(TaskType::Refactor.as_ordinal(), 2.0);
        assert_eq!(TaskType::Test.as_ordinal(), 3.0);
        assert_eq!(TaskType::Docs.as_ordinal(), 4.0);
        assert_eq!(TaskType::Review.as_ordinal(), 5.0);
        assert_eq!(TaskType::Research.as_ordinal(), 6.0);
    }

    #[test]
    fn language_ordinals() {
        assert_eq!(Language::Python.as_ordinal(), 0.0);
        assert_eq!(Language::Rust.as_ordinal(), 1.0);
        assert_eq!(Language::Typescript.as_ordinal(), 2.0);
        assert_eq!(Language::Go.as_ordinal(), 3.0);
        assert_eq!(Language::Mixed.as_ordinal(), 4.0);
        assert_eq!(Language::Other.as_ordinal(), 5.0);
    }

    #[test]
    fn complexity_ordinals() {
        assert_eq!(Complexity::Trivial.as_ordinal(), 0.0);
        assert_eq!(Complexity::Simple.as_ordinal(), 1.0);
        assert_eq!(Complexity::Moderate.as_ordinal(), 2.0);
        assert_eq!(Complexity::Complex.as_ordinal(), 3.0);
        assert_eq!(Complexity::Critical.as_ordinal(), 4.0);
    }

    #[test]
    fn priority_ordinals() {
        assert_eq!(Priority::Low.as_ordinal(), 0.0);
        assert_eq!(Priority::Normal.as_ordinal(), 1.0);
        assert_eq!(Priority::High.as_ordinal(), 2.0);
        assert_eq!(Priority::Urgent.as_ordinal(), 3.0);
    }

    // -- Display tests --

    #[test]
    fn display_matches_serde() {
        assert_eq!(AgentAction::Assign.to_string(), "assign");
        assert_eq!(AgentAction::Reject.to_string(), "reject");
        assert_eq!(AgentAction::Fallback.to_string(), "fallback");

        assert_eq!(AgentState::Active.to_string(), "active");
        assert_eq!(AgentState::Inactive.to_string(), "inactive");
        assert_eq!(AgentState::Busy.to_string(), "busy");
        assert_eq!(AgentState::Failed.to_string(), "failed");

        assert_eq!(Severity::Critical.to_string(), "Critical");
        assert_eq!(Severity::Warning.to_string(), "Warning");
    }

    // -- JSON structure tests (matching MCP protocol schemas) --

    #[test]
    fn invariant_result_json_structure() {
        let result = InvariantResult {
            rule: "scope_isolation".to_string(),
            severity: Severity::Critical,
            passed: false,
            detail: "Conflict with task-42 on src/".to_string(),
        };
        let value: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&result).unwrap()).unwrap();
        assert_eq!(value["rule"], "scope_isolation");
        assert_eq!(value["severity"], "Critical");
        assert_eq!(value["passed"], false);
        assert!(value["detail"].as_str().unwrap().contains("task-42"));
    }

    #[test]
    fn route_task_input_json_matches_spec() {
        let json = r#"{
            "type": "feature",
            "language": "rust",
            "complexity": "complex",
            "priority": "high",
            "scope": ["arbiter-core/src/types.rs"],
            "branch": "task/task-001",
            "estimated_tokens": 50000,
            "has_dependencies": false,
            "requires_internet": false,
            "sla_minutes": 120,
            "description": "Implement core types"
        }"#;
        let task: TaskInput = serde_json::from_str(json).unwrap();
        assert_eq!(task.task_type, TaskType::Feature);
        assert_eq!(task.language, Language::Rust);
        assert_eq!(task.complexity, Complexity::Complex);
        assert_eq!(task.priority, Priority::High);
        assert_eq!(task.scope.len(), 1);
        assert_eq!(task.branch.as_deref(), Some("task/task-001"));
        assert_eq!(task.estimated_tokens, Some(50000));
        assert_eq!(task.sla_minutes, Some(120));
    }
}
