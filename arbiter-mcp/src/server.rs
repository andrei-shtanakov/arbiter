//! MCP server: JSON-RPC 2.0 dispatch over stdio.
//!
//! Reads one JSON object per line from stdin, dispatches to handlers,
//! and writes JSON responses to stdout. All logging goes to stderr.

use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, error, info, warn};

use arbiter_core::policy::decision_tree::DecisionTree;
use arbiter_core::types::{Constraints, TaskInput};

use crate::agents::AgentRegistry;
use crate::config::ArbiterConfig;
use crate::db::Database;
use crate::metrics::Metrics;
use crate::tools::{agent_status, report_outcome, route_task};

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 types
// ---------------------------------------------------------------------------

/// Incoming JSON-RPC 2.0 message (request or notification).
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

/// Outgoing JSON-RPC 2.0 response.
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

// Standard JSON-RPC error codes
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;

/// Known task type values accepted by the routing engine.
const KNOWN_TASK_TYPES: &[&str] = &[
    "feature", "bugfix", "refactor", "test", "docs", "review", "research",
];

/// Known language values accepted by the routing engine.
const KNOWN_LANGUAGES: &[&str] = &["python", "rust", "typescript", "go", "mixed", "other"];

// ---------------------------------------------------------------------------
// Tool schemas
// ---------------------------------------------------------------------------

/// Build the tool schemas for `tools/list`.
fn tool_schemas() -> Value {
    serde_json::json!({
        "tools": [
            {
                "name": "route_task",
                "description": "Route a coding task to the best agent based on decision tree inference, invariant checks, and agent capabilities.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "Unique task identifier" },
                        "task": {
                            "type": "object",
                            "properties": {
                                "type": { "type": "string", "enum": ["feature", "bugfix", "refactor", "test", "docs", "review", "research"] },
                                "language": { "type": "string", "enum": ["python", "rust", "typescript", "go", "mixed", "other"] },
                                "complexity": { "type": "string", "enum": ["trivial", "simple", "moderate", "complex", "critical"] },
                                "priority": { "type": "string", "enum": ["low", "normal", "high", "urgent"] },
                                "scope": { "type": "array", "items": { "type": "string" } },
                                "branch": { "type": "string" },
                                "estimated_tokens": { "type": "integer" },
                                "has_dependencies": { "type": "boolean" },
                                "requires_internet": { "type": "boolean" },
                                "sla_minutes": { "type": "integer" },
                                "description": { "type": "string" }
                            },
                            "required": ["type", "language", "complexity", "priority"]
                        },
                        "constraints": {
                            "type": "object",
                            "properties": {
                                "preferred_agent": { "type": "string" },
                                "excluded_agents": { "type": "array", "items": { "type": "string" } },
                                "budget_remaining_usd": { "type": "number" },
                                "total_pending_tasks": { "type": "integer" },
                                "running_tasks": { "type": "array" },
                                "retry_count": { "type": "integer", "description": "Number of retries for this task so far" },
                                "calls_per_minute": { "type": "integer", "description": "Current API calls per minute" }
                            }
                        }
                    },
                    "required": ["task_id", "task"]
                }
            },
            {
                "name": "report_outcome",
                "description": "Report the outcome of a task execution to update agent statistics and detect health issues.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "Task identifier from route_task" },
                        "agent_id": { "type": "string", "description": "Agent that executed the task" },
                        "status": { "type": "string", "enum": ["success", "failure", "timeout", "cancelled"] },
                        "duration_min": { "type": "number" },
                        "tokens_used": { "type": "integer" },
                        "cost_usd": { "type": "number" },
                        "exit_code": { "type": "integer" },
                        "files_changed": { "type": "integer" },
                        "tests_passed": { "type": "boolean" },
                        "validation_passed": { "type": "boolean" },
                        "error_summary": { "type": "string" },
                        "retry_count": { "type": "integer" }
                    },
                    "required": ["task_id", "agent_id", "status"]
                }
            },
            {
                "name": "get_agent_status",
                "description": "Query agent capabilities, current load, and performance history.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "agent_id": { "type": "string", "description": "Agent ID to query. Omit to get all agents." }
                    }
                }
            },
            {
                "name": "get_metrics",
                "description": "Get current server metrics: decision counts, latency stats, fallback and reject rates.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            }
        ]
    })
}

// ---------------------------------------------------------------------------
// MCP Server
// ---------------------------------------------------------------------------

/// MCP server that reads JSON-RPC 2.0 from stdin and writes to stdout.
pub struct McpServer<'a> {
    config: ArbiterConfig,
    initialized: bool,
    db: &'a Database,
    tree: Option<&'a DecisionTree>,
    registry: AgentRegistry<'a>,
    metrics: &'a Metrics,
    shutdown: Arc<AtomicBool>,
}

impl<'a> McpServer<'a> {
    /// Create a new MCP server with the given configuration, database,
    /// decision tree, agent registry, metrics collector, and shutdown
    /// flag.
    pub fn new(
        config: ArbiterConfig,
        db: &'a Database,
        tree: Option<&'a DecisionTree>,
        registry: AgentRegistry<'a>,
        metrics: &'a Metrics,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            config,
            initialized: false,
            db,
            tree,
            registry,
            metrics,
            shutdown,
        }
    }

    /// Run the stdio loop: read lines from stdin, dispatch, write responses.
    ///
    /// Returns `Ok(())` on graceful shutdown (stdin EOF).
    pub fn run(&mut self) -> Result<()> {
        let stdin = io::stdin();
        let stdout = io::stdout();
        let mut stdout = stdout.lock();

        info!("MCP server ready, reading from stdin");

        for line in stdin.lock().lines() {
            if self.shutdown.load(AtomicOrdering::Relaxed) {
                info!("shutdown signal received, stopping");
                break;
            }
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    error!("stdin read error: {e}");
                    break;
                }
            };

            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }

            debug!("recv: {line}");

            let request: JsonRpcRequest = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(e) => {
                    warn!("invalid JSON-RPC: {e}");
                    let resp = JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id: None,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32700,
                            message: format!("Parse error: {e}"),
                            data: None,
                        }),
                    };
                    write_response(&mut stdout, &resp)?;
                    continue;
                }
            };

            if let Some(resp) = self.dispatch(&request) {
                debug!("send: {:?}", resp.id);
                write_response(&mut stdout, &resp)?;
            }
        }

        info!("stdin EOF, shutting down");
        Ok(())
    }

    /// Dispatch a JSON-RPC request to the appropriate handler.
    ///
    /// Returns `None` for notifications (no `id` field).
    pub fn dispatch(&mut self, req: &JsonRpcRequest) -> Option<JsonRpcResponse> {
        match req.method.as_str() {
            "initialize" => Some(self.handle_initialize(req)),
            "notifications/initialized" | "initialized" => {
                self.handle_initialized();
                // Notification — no response if no id
                if req.id.is_some() {
                    Some(JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id: req.id.clone(),
                        result: Some(Value::Null),
                        error: None,
                    })
                } else {
                    None
                }
            }
            "tools/list" => Some(self.handle_tools_list(req)),
            "tools/call" => Some(self.handle_tools_call(req)),
            "ping" => {
                debug!("ping");
                Some(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id.clone(),
                    result: Some(serde_json::json!({})),
                    error: None,
                })
            }
            _ => {
                warn!("unknown method: {}", req.method);
                Some(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id.clone(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: METHOD_NOT_FOUND,
                        message: format!("Method not found: {}", req.method),
                        data: None,
                    }),
                })
            }
        }
    }

    /// Handle `initialize` — return server capabilities.
    fn handle_initialize(&mut self, req: &JsonRpcRequest) -> JsonRpcResponse {
        info!("initialize handshake");
        self.initialized = true;
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id.clone(),
            result: Some(serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "arbiter",
                    "version": env!("CARGO_PKG_VERSION")
                }
            })),
            error: None,
        }
    }

    /// Handle `initialized` notification — log acknowledgement.
    fn handle_initialized(&mut self) {
        info!("client acknowledged initialization");
    }

    /// Handle `tools/list` — return 3 tool schemas.
    fn handle_tools_list(&self, req: &JsonRpcRequest) -> JsonRpcResponse {
        debug!("tools/list");
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id.clone(),
            result: Some(tool_schemas()),
            error: None,
        }
    }

    /// Handle `tools/call` — dispatch to the named tool.
    fn handle_tools_call(&self, req: &JsonRpcRequest) -> JsonRpcResponse {
        let params = match &req.params {
            Some(p) => p,
            None => {
                return JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id.clone(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: INVALID_PARAMS,
                        message: "Missing params".to_string(),
                        data: None,
                    }),
                };
            }
        };

        let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");

        match tool_name {
            "route_task" => {
                let arguments = params.get("arguments");
                self.handle_route_task(req, arguments)
            }
            "report_outcome" => {
                let arguments = params.get("arguments");
                self.handle_report_outcome(req, arguments)
            }
            "get_agent_status" => {
                let arguments = params.get("arguments");
                self.handle_get_agent_status(req, arguments)
            }
            "get_metrics" => self.handle_get_metrics(req),
            _ => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id.clone(),
                result: None,
                error: Some(JsonRpcError {
                    code: INVALID_PARAMS,
                    message: format!("Unknown tool: {tool_name}"),
                    data: None,
                }),
            },
        }
    }

    // -- Tool handlers --

    /// Handle route_task: validate input, run routing logic, return decision.
    fn handle_route_task(
        &self,
        req: &JsonRpcRequest,
        arguments: Option<&Value>,
    ) -> JsonRpcResponse {
        debug!("route_task called");
        let args = match arguments {
            Some(a) => a,
            None => {
                return JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id.clone(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: INVALID_PARAMS,
                        message: "Missing arguments for route_task".to_string(),
                        data: None,
                    }),
                };
            }
        };

        // Validate required fields
        let task_id = match args.get("task_id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => {
                return JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id.clone(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: INVALID_PARAMS,
                        message: "Missing required field: task_id".to_string(),
                        data: None,
                    }),
                };
            }
        };

        let task_value = match args.get("task") {
            Some(t) => t,
            None => {
                return JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id.clone(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: INVALID_PARAMS,
                        message: "Missing required field: task".to_string(),
                        data: None,
                    }),
                };
            }
        };

        // Pre-validate task_type and language, substituting defaults for unknowns
        let mut task_value = task_value.clone();
        let mut input_warnings: Vec<String> = Vec::new();

        if let Some(obj) = task_value.as_object_mut() {
            if let Some(type_val) = obj.get("type").and_then(|v| v.as_str()) {
                if !KNOWN_TASK_TYPES.contains(&type_val) {
                    warn!(
                        unknown_type = type_val,
                        "unknown task_type, defaulting to 'feature'"
                    );
                    input_warnings.push(format!(
                        "Unknown task_type '{}', defaulting to 'feature'",
                        type_val
                    ));
                    obj.insert("type".to_string(), Value::String("feature".to_string()));
                }
            }
            if let Some(lang_val) = obj.get("language").and_then(|v| v.as_str()) {
                if !KNOWN_LANGUAGES.contains(&lang_val) {
                    warn!(
                        unknown_language = lang_val,
                        "unknown language, defaulting to 'other'"
                    );
                    input_warnings.push(format!(
                        "Unknown language '{}', defaulting to 'other'",
                        lang_val
                    ));
                    obj.insert("language".to_string(), Value::String("other".to_string()));
                }
            }
        }

        // Parse task input
        let task: TaskInput = match serde_json::from_value(task_value) {
            Ok(t) => t,
            Err(e) => {
                return JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id.clone(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: INVALID_PARAMS,
                        message: format!("Invalid task: {e}"),
                        data: None,
                    }),
                };
            }
        };

        // Parse constraints (optional, defaults to empty)
        let constraints: Constraints = args
            .get("constraints")
            .map(|c| serde_json::from_value(c.clone()))
            .transpose()
            .unwrap_or(None)
            .unwrap_or(Constraints {
                preferred_agent: None,
                excluded_agents: vec![],
                budget_remaining_usd: None,
                total_pending_tasks: None,
                running_tasks: vec![],
                retry_count: None,
                calls_per_minute: None,
            });

        // Execute route_task logic
        match route_task::execute(
            task_id,
            &task,
            &constraints,
            self.tree,
            &self.registry,
            self.db,
            &self.config.invariants,
            self.metrics,
        ) {
            Ok(mut result) => {
                // Merge any input warnings into the result
                result.warnings.extend(input_warnings);
                let response_json = route_task::result_to_json(&result);
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id.clone(),
                    result: Some(serde_json::json!({
                        "content": [{
                            "type": "text",
                            "text": response_json.to_string()
                        }]
                    })),
                    error: None,
                }
            }
            Err(e) => {
                error!("route_task failed: {e:#}");
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id.clone(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!("Internal error: {e}"),
                        data: None,
                    }),
                }
            }
        }
    }

    /// Handle report_outcome: validate input, record outcome, update stats.
    fn handle_report_outcome(
        &self,
        req: &JsonRpcRequest,
        arguments: Option<&Value>,
    ) -> JsonRpcResponse {
        debug!("report_outcome called");
        let args = match arguments {
            Some(a) => a,
            None => {
                return JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id.clone(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: INVALID_PARAMS,
                        message: "Missing arguments for report_outcome".to_string(),
                        data: None,
                    }),
                };
            }
        };

        // Validate required fields
        if args.get("task_id").and_then(|v| v.as_str()).is_none() {
            return JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id.clone(),
                result: None,
                error: Some(JsonRpcError {
                    code: INVALID_PARAMS,
                    message: "Missing required field: task_id".to_string(),
                    data: None,
                }),
            };
        }
        if args.get("agent_id").and_then(|v| v.as_str()).is_none() {
            return JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id.clone(),
                result: None,
                error: Some(JsonRpcError {
                    code: INVALID_PARAMS,
                    message: "Missing required field: agent_id".to_string(),
                    data: None,
                }),
            };
        }
        if args.get("status").and_then(|v| v.as_str()).is_none() {
            return JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id.clone(),
                result: None,
                error: Some(JsonRpcError {
                    code: INVALID_PARAMS,
                    message: "Missing required field: status".to_string(),
                    data: None,
                }),
            };
        }

        match report_outcome::execute(args, self.db, &self.config) {
            Ok(result) => {
                let response_json = report_outcome::result_to_json(&result);
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id.clone(),
                    result: Some(serde_json::json!({
                        "content": [{
                            "type": "text",
                            "text": response_json.to_string()
                        }]
                    })),
                    error: None,
                }
            }
            Err(e) => {
                error!("report_outcome failed: {e:#}");
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id.clone(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!("{e}"),
                        data: None,
                    }),
                }
            }
        }
    }

    /// Handle get_agent_status: query agent capabilities, load, and stats.
    fn handle_get_agent_status(
        &self,
        req: &JsonRpcRequest,
        arguments: Option<&Value>,
    ) -> JsonRpcResponse {
        debug!("get_agent_status called");

        let args = arguments.cloned().unwrap_or(serde_json::json!({}));
        let max_failures = self.config.invariants.agent_health.max_failures_24h;

        match agent_status::execute(&args, self.db, &self.registry, max_failures) {
            Ok(result) => {
                let response_json = agent_status::result_to_json(&result);
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id.clone(),
                    result: Some(serde_json::json!({
                        "content": [{
                            "type": "text",
                            "text": response_json.to_string()
                        }]
                    })),
                    error: None,
                }
            }
            Err(e) => {
                error!("get_agent_status failed: {e:#}");
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id.clone(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32000,
                        message: format!("{e}"),
                        data: None,
                    }),
                }
            }
        }
    }

    /// Handle get_metrics: return server metrics snapshot.
    fn handle_get_metrics(&self, req: &JsonRpcRequest) -> JsonRpcResponse {
        debug!("get_metrics called");
        let response_json = crate::tools::get_metrics::execute(self.metrics);
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id.clone(),
            result: Some(serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": response_json.to_string()
                }]
            })),
            error: None,
        }
    }
}

/// Write a JSON-RPC response as a single line to the writer.
fn write_response(writer: &mut impl Write, resp: &JsonRpcResponse) -> Result<()> {
    let json = serde_json::to_string(resp)?;
    writeln!(writer, "{json}")?;
    writer.flush()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AgentConfig, AgentHealthConfig, ArbiterConfig, BudgetConfig, ConcurrencyConfig,
        InvariantConfig, RateLimitConfig, RetriesConfig, SlaConfig,
    };
    use std::collections::HashMap;

    fn test_config() -> ArbiterConfig {
        let mut agents = HashMap::new();
        agents.insert(
            "claude_code".to_string(),
            AgentConfig {
                display_name: "Claude Code".to_string(),
                supports_languages: vec!["python".to_string(), "rust".to_string()],
                supports_types: vec!["feature".to_string(), "bugfix".to_string()],
                max_concurrent: 2,
                cost_per_hour: 0.30,
                avg_duration_min: 18.0,
            },
        );
        ArbiterConfig {
            agents,
            invariants: InvariantConfig {
                budget: BudgetConfig {
                    threshold_usd: 10.0,
                },
                retries: RetriesConfig { max_retries: 3 },
                rate_limit: RateLimitConfig {
                    calls_per_minute: 60,
                },
                agent_health: AgentHealthConfig {
                    max_failures_24h: 5,
                },
                concurrency: ConcurrencyConfig {
                    max_total_concurrent: 5,
                },
                sla: SlaConfig {
                    buffer_multiplier: 1.5,
                },
            },
        }
    }

    fn test_tree_json() -> String {
        serde_json::json!({
            "n_features": 22,
            "n_classes": 3,
            "class_names": ["claude_code", "codex_cli", "aider"],
            "feature_names": [
                "task_type", "language", "complexity", "priority",
                "scope_size", "estimated_tokens", "has_dependencies",
                "requires_internet", "sla_minutes",
                "agent_success_rate", "agent_available_slots",
                "agent_running_tasks", "agent_avg_duration_min",
                "agent_avg_cost_usd", "agent_recent_failures",
                "agent_supports_task_type", "agent_supports_language",
                "total_running_tasks", "total_pending_tasks",
                "budget_remaining_usd", "time_of_day_hour",
                "concurrent_scope_conflicts"
            ],
            "nodes": [
                {"feature": 12, "threshold": 12.9, "left": 1, "right": 2,
                 "value": [10.0, 10.0, 10.0]},
                {"feature": -1, "threshold": 0.0, "left": -1, "right": -1,
                 "value": [2.0, 5.0, 8.0]},
                {"feature": -1, "threshold": 0.0, "left": -1, "right": -1,
                 "value": [8.0, 1.0, 1.0]}
            ]
        })
        .to_string()
    }

    fn setup_server() -> (Database, DecisionTree, ArbiterConfig) {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let tree = DecisionTree::from_json(&test_tree_json()).unwrap();
        let config = test_config();
        (db, tree, config)
    }

    fn dispatch(server: &mut McpServer, json: &str) -> Option<JsonRpcResponse> {
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        server.dispatch(&req)
    }

    #[test]
    fn handle_initialize_returns_capabilities() {
        let (db, tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, Some(&tree), registry, &metrics, shutdown);
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        )
        .unwrap();

        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["capabilities"]["tools"], serde_json::json!({}));
        assert_eq!(result["serverInfo"]["name"], "arbiter");
        assert!(server.initialized);
    }

    #[test]
    fn handle_initialized_notification_no_id() {
        let (db, tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, Some(&tree), registry, &metrics, shutdown);
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        );
        // Notification without id -> no response
        assert!(resp.is_none());
    }

    #[test]
    fn handle_initialized_with_id() {
        let (db, tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, Some(&tree), registry, &metrics, shutdown);
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","id":2,"method":"initialized"}"#,
        )
        .unwrap();
        assert!(resp.error.is_none());
    }

    #[test]
    fn handle_tools_list_returns_4_tools() {
        let (db, tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, Some(&tree), registry, &metrics, shutdown);
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
        )
        .unwrap();

        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 4);

        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"route_task"));
        assert!(names.contains(&"report_outcome"));
        assert!(names.contains(&"get_agent_status"));
        assert!(names.contains(&"get_metrics"));
    }

    #[test]
    fn handle_ping_returns_empty_object() {
        let (db, tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, Some(&tree), registry, &metrics, shutdown);
        let resp = dispatch(&mut server, r#"{"jsonrpc":"2.0","id":99,"method":"ping"}"#).unwrap();

        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap(), serde_json::json!({}));
        assert_eq!(resp.id, Some(serde_json::json!(99)));
    }

    #[test]
    fn handle_unknown_method_returns_32601() {
        let (db, tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, Some(&tree), registry, &metrics, shutdown);
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","id":4,"method":"nonexistent"}"#,
        )
        .unwrap();

        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("nonexistent"));
    }

    #[test]
    fn tools_call_missing_params_returns_32602() {
        let (db, tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, Some(&tree), registry, &metrics, shutdown);
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call"}"#,
        )
        .unwrap();

        let err = resp.error.unwrap();
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("Missing params"));
    }

    #[test]
    fn tools_call_unknown_tool_returns_32602() {
        let (db, tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, Some(&tree), registry, &metrics, shutdown);
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"bad_tool"}}"#,
        )
        .unwrap();

        let err = resp.error.unwrap();
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("bad_tool"));
    }

    #[test]
    fn route_task_missing_task_id_returns_32602() {
        let (db, tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, Some(&tree), registry, &metrics, shutdown);
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"route_task","arguments":{"task":{}}}}"#,
        )
        .unwrap();

        let err = resp.error.unwrap();
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("task_id"));
    }

    #[test]
    fn route_task_missing_task_returns_32602() {
        let (db, tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, Some(&tree), registry, &metrics, shutdown);
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"route_task","arguments":{"task_id":"t1"}}}"#,
        )
        .unwrap();

        let err = resp.error.unwrap();
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("task"));
    }

    #[test]
    fn route_task_returns_decision() {
        let (db, tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, Some(&tree), registry, &metrics, shutdown);
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"route_task","arguments":{"task_id":"t1","task":{"type":"bugfix","language":"python","complexity":"simple","priority":"normal"}}}}"#,
        )
        .unwrap();

        assert!(resp.error.is_none(), "got error: {:?}", resp.error);
        let result = resp.result.unwrap();
        let content = &result["content"][0]["text"];
        let decision: serde_json::Value = serde_json::from_str(content.as_str().unwrap()).unwrap();
        assert_eq!(decision["task_id"], "t1");
        assert!(
            decision["action"] == "assign" || decision["action"] == "fallback",
            "expected assign or fallback, got {}",
            decision["action"]
        );
    }

    #[test]
    fn report_outcome_missing_fields_returns_32602() {
        let (db, tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, Some(&tree), registry, &metrics, shutdown);
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"report_outcome","arguments":{"task_id":"t1"}}}"#,
        )
        .unwrap();

        let err = resp.error.unwrap();
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("agent_id"));
    }

    #[test]
    fn report_outcome_returns_result() {
        let (db, tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, Some(&tree), registry, &metrics, shutdown);
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"report_outcome","arguments":{"task_id":"t1","agent_id":"claude_code","status":"success"}}}"#,
        )
        .unwrap();

        assert!(resp.error.is_none(), "got error: {:?}", resp.error);
        let result = resp.result.unwrap();
        let content = &result["content"][0]["text"];
        let outcome: serde_json::Value = serde_json::from_str(content.as_str().unwrap()).unwrap();
        assert_eq!(outcome["task_id"], "t1");
        assert_eq!(outcome["recorded"], true);
        assert_eq!(outcome["retrain_suggested"], false);
        // Unknown task_id should produce a warning
        assert!(outcome["warnings"].as_array().unwrap().len() > 0);
    }

    #[test]
    fn get_agent_status_stub_returns_agents() {
        let (db, tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, Some(&tree), registry, &metrics, shutdown);
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"get_agent_status","arguments":{}}}"#,
        )
        .unwrap();

        assert!(resp.error.is_none());
        assert!(resp.result.is_some());
    }

    #[test]
    fn route_task_no_arguments_returns_32602() {
        let (db, tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, Some(&tree), registry, &metrics, shutdown);
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","id":13,"method":"tools/call","params":{"name":"route_task"}}"#,
        )
        .unwrap();

        let err = resp.error.unwrap();
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("Missing arguments"));
    }

    #[test]
    fn route_task_unknown_task_type_defaults_to_feature() {
        let (db, tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, Some(&tree), registry, &metrics, shutdown);
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","id":20,"method":"tools/call","params":{"name":"route_task","arguments":{"task_id":"unknown-type","task":{"type":"magic_spell","language":"python","complexity":"simple","priority":"normal"}}}}"#,
        )
        .unwrap();

        assert!(resp.error.is_none(), "got error: {:?}", resp.error);
        let result = resp.result.unwrap();
        let content = &result["content"][0]["text"];
        let decision: serde_json::Value = serde_json::from_str(content.as_str().unwrap()).unwrap();

        // Should have a warning about the unknown task_type
        let warnings = decision["warnings"].as_array().unwrap();
        assert!(
            warnings.iter().any(|w| {
                let s = w.as_str().unwrap_or("");
                s.contains("magic_spell") && s.contains("feature")
            }),
            "should warn about unknown type defaulting to feature: {:?}",
            warnings
        );
    }

    #[test]
    fn route_task_unknown_language_defaults_to_other() {
        let (db, tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, Some(&tree), registry, &metrics, shutdown);
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","id":21,"method":"tools/call","params":{"name":"route_task","arguments":{"task_id":"unknown-lang","task":{"type":"bugfix","language":"cobol","complexity":"simple","priority":"normal"}}}}"#,
        )
        .unwrap();

        assert!(resp.error.is_none(), "got error: {:?}", resp.error);
        let result = resp.result.unwrap();
        let content = &result["content"][0]["text"];
        let decision: serde_json::Value = serde_json::from_str(content.as_str().unwrap()).unwrap();

        // Should have a warning about the unknown language
        let warnings = decision["warnings"].as_array().unwrap();
        assert!(
            warnings.iter().any(|w| {
                let s = w.as_str().unwrap_or("");
                s.contains("cobol") && s.contains("other")
            }),
            "should warn about unknown language defaulting to other: {:?}",
            warnings
        );
    }

    #[test]
    fn route_task_degraded_mode_no_tree() {
        let (db, _tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, None, registry, &metrics, shutdown); // No tree
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","id":22,"method":"tools/call","params":{"name":"route_task","arguments":{"task_id":"no-tree","task":{"type":"bugfix","language":"python","complexity":"simple","priority":"normal"}}}}"#,
        )
        .unwrap();

        assert!(resp.error.is_none(), "got error: {:?}", resp.error);
        let result = resp.result.unwrap();
        let content = &result["content"][0]["text"];
        let decision: serde_json::Value = serde_json::from_str(content.as_str().unwrap()).unwrap();

        // Should route successfully (not error)
        assert!(
            decision["action"] == "assign" || decision["action"] == "fallback",
            "degraded mode should still route, got {}",
            decision["action"]
        );

        // Should have round-robin warning
        let warnings = decision["warnings"].as_array().unwrap();
        assert!(
            warnings
                .iter()
                .any(|w| { w.as_str().unwrap_or("").contains("round-robin") }),
            "should warn about round-robin: {:?}",
            warnings
        );
    }

    #[test]
    fn response_ids_match_request() {
        let (db, tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, Some(&tree), registry, &metrics, shutdown);

        // Numeric id
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","id":42,"method":"initialize"}"#,
        )
        .unwrap();
        assert_eq!(resp.id, Some(serde_json::json!(42)));

        // String id
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","id":"abc","method":"tools/list"}"#,
        )
        .unwrap();
        assert_eq!(resp.id, Some(serde_json::json!("abc")));
    }

    #[test]
    fn report_outcome_error_uses_server_error_code() {
        let (db, tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, Some(&tree), registry, &metrics, shutdown);

        // Send report_outcome with invalid status (should fail with business logic error)
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"report_outcome","arguments":{"task_id":"test-task","agent_id":"claude-code","status":"invalid_status"}}}"#,
        )
        .unwrap();

        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("status") || err.message.contains("invalid"));
    }

    #[test]
    fn get_agent_status_error_uses_server_error_code() {
        let (db, tree, config) = setup_server();
        let metrics = Metrics::new();
        let registry = AgentRegistry::new(&db, &config.agents).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut server = McpServer::new(config, &db, Some(&tree), registry, &metrics, shutdown);

        // Send get_agent_status with unknown agent_id (should fail with business logic error)
        let resp = dispatch(
            &mut server,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_agent_status","arguments":{"agent_id":"unknown-agent-12345"}}}"#,
        )
        .unwrap();

        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("agent not found"));
    }
}
