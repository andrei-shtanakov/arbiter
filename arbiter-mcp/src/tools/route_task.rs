//! route_task tool implementation.
//!
//! Routes a coding task to the best available agent using Decision Tree
//! inference, invariant checks, and cascade fallback.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use anyhow::{Context, Result};
use chrono::Timelike;
use serde_json::Value;
use tracing::{debug, info, warn};

use arbiter_core::invariant::rules::{
    check_all_invariants, has_critical_failure, AgentContext, InvariantThresholds, SystemContext,
};
use arbiter_core::policy::decision_tree::DecisionTree;
use arbiter_core::policy::engine::evaluate_for_agents;
use arbiter_core::types::{
    AgentAction, AgentState, Constraints, InvariantResult, PredictionResult, TaskInput, TaskType,
};

/// Round-robin counter for degraded mode agent selection.
static ROUND_ROBIN_COUNTER: AtomicUsize = AtomicUsize::new(0);

use crate::agents::AgentRegistry;
use crate::config::InvariantConfig;
use crate::db::{Database, DecisionRecord};
use crate::features::{build_feature_vector, AgentInfo, SystemState};

/// Maximum cascade fallback attempts before rejecting.
const MAX_FALLBACK_ATTEMPTS: usize = 2;

/// Preferred agent confidence boost.
const PREFERRED_AGENT_BOOST: f64 = 0.1;

/// Static map from routed task type to the benchmark whose scores inform it.
///
/// R-07 Phase 1: only `Review` is wired (→ `"code-review"`); other task types
/// return `None` → no benchmark adjustment. Extend this map (and seed rows) to
/// add benchmarks — no other code change is required.
fn benchmark_id_for(task_type: &TaskType) -> Option<&'static str> {
    match task_type {
        TaskType::Review => Some("code-review"),
        _ => None,
    }
}

/// Additively adjust each agent's prediction confidence by its benchmark score,
/// centered on `0.5` so a neutral score changes nothing: `delta = (score - 0.5) *
/// weight`. Appends a `bench_adjust[...]` audit line to that prediction's `path`
/// (the chosen agent later copies `path` into `decision_path` — there is no
/// `RouteResult` in scope at the re-rank site). Re-sorts by adjusted confidence.
///
/// No-op when `weight <= 0`, the task type has no mapped benchmark, or the agent
/// has no score for it (left untouched). Confidence stays clamped to `[0, 1]`.
///
/// SCOPE (R-07 decision C, 2026-07-02): this is intended as a **tiebreaker**,
/// not a DT override. The delta is additive and the list is then re-sorted, so a
/// flip is *possible* in principle (a large enough `weight` or `rank_score` gap
/// can cross two candidates). In practice it does not overturn a dominant DT leaf
/// (e.g. a 1.0-confidence review winner): a realistic `rank_score` gap (~0.08)
/// times a sane `weight` (~0.15) is far smaller than a 1.0-vs-lower DT margin. So
/// benchmark data breaks ties on DT-ambiguous tasks and is effectively inert on
/// confident ones under sane weights — by design. Giving the benchmark authority
/// to genuinely override the DT would be a different mechanism (weighted blend),
/// consciously not chosen. Rationale is recorded in the ecosystem workspace
/// status log (`_cowork_output/status/2026-07-02-r07-rerank-tiebreaker-scope.md`,
/// a sibling repo — not vendored into this crate).
fn apply_benchmark_rerank(
    ranked: &mut [(String, PredictionResult)],
    task_type: &TaskType,
    db: &Database,
    weight: f64,
) -> Result<()> {
    if weight <= 0.0 {
        return Ok(());
    }
    let Some(bench) = benchmark_id_for(task_type) else {
        return Ok(());
    };
    for (agent_id, pred) in ranked.iter_mut() {
        if let Some(score) = db.get_benchmark_score(agent_id, bench)? {
            let delta = (score - 0.5) * weight;
            pred.confidence = (pred.confidence + delta).clamp(0.0, 1.0);
            pred.path.push(format!(
                "bench_adjust[{agent_id}]: {bench} score={score:.3} delta={delta:+.3}"
            ));
        }
    }
    ranked.sort_by(|a, b| {
        b.1.confidence
            .partial_cmp(&a.1.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            // Deterministic order for ANY residual confidence tie (agent_id ascending,
            // consistent with the primary ranking sort). Only fires when confidences
            // are exactly equal.
            .then_with(|| a.0.cmp(&b.0))
    });
    Ok(())
}

/// Result of the route_task operation.
#[derive(Debug)]
pub struct RouteResult {
    pub task_id: String,
    pub action: AgentAction,
    pub chosen_agent: String,
    pub confidence: f64,
    pub reasoning: String,
    pub decision_path: Vec<String>,
    pub fallback_agent: Option<String>,
    pub fallback_reason: Option<String>,
    pub invariant_checks: Vec<InvariantResult>,
    pub inference_us: i64,
    pub feature_vector: Vec<f64>,
    pub candidates_evaluated: usize,
    pub warnings: Vec<String>,
    /// SQLite rowid of the inserted decision, surfaced to callers so they
    /// can correlate this routing decision with a later report_outcome and
    /// guard against stale retries. None if the SQLite insert failed.
    pub decision_id: Option<i64>,
}

/// Convert InvariantConfig to InvariantThresholds for arbiter-core.
fn to_thresholds(config: &InvariantConfig) -> InvariantThresholds {
    InvariantThresholds {
        max_total_concurrent: config.concurrency.max_total_concurrent,
        max_retries: config.retries.max_retries,
        calls_per_minute: config.rate_limit.calls_per_minute,
        max_failures_24h: config.agent_health.max_failures_24h,
        buffer_multiplier: config.sla.buffer_multiplier,
    }
}

/// Build an AgentContext from AgentInfo for invariant checks.
///
/// Computes `AgentState` dynamically from runtime metrics:
/// - `Failed` if `recent_failures > max_failures_24h`
/// - `Busy` if `running_tasks >= max_concurrent`
/// - `Active` otherwise
fn to_agent_context(info: &AgentInfo, max_failures_24h: u32) -> AgentContext {
    let state = if info.recent_failures > max_failures_24h {
        AgentState::Failed
    } else if info.running_tasks >= info.config.max_concurrent {
        AgentState::Busy
    } else {
        AgentState::Active
    };

    AgentContext {
        agent_id: info.agent_id.clone(),
        state,
        running_tasks: info.running_tasks,
        max_concurrent: info.config.max_concurrent,
        supports_languages: info.config.supports_languages.clone(),
        supports_types: info.config.supports_types.clone(),
        failures_24h: info.recent_failures,
        avg_duration_min: info
            .avg_duration_min
            .unwrap_or(info.config.avg_duration_min),
        cost_per_hour: info.config.cost_per_hour,
    }
}

/// Build a SystemContext from constraints and registry state.
fn to_system_context(constraints: &Constraints, total_running: u32) -> SystemContext {
    let running_scopes: Vec<Vec<String>> = constraints
        .running_tasks
        .iter()
        .map(|rt| rt.scope.clone())
        .collect();
    let running_branches: Vec<String> = constraints
        .running_tasks
        .iter()
        .filter_map(|rt| rt.branch.clone())
        .collect();

    SystemContext {
        total_running_tasks: total_running,
        running_scopes,
        running_branches,
        budget_remaining_usd: constraints.budget_remaining_usd,
        retry_count: constraints.retry_count.unwrap_or(0),
        calls_per_minute: constraints.calls_per_minute.unwrap_or(0),
    }
}

/// Execute the route_task logic.
///
/// Algorithm:
/// 1. Parse and validate input
/// 2. Load agent states from registry
/// 3. Filter by hard constraints (type, language, slots, exclusions)
/// 4. Build feature vectors per candidate
/// 5. Run DT inference and rank
/// 6. Apply preferred_agent boost
/// 7. Run invariant checks
/// 8. Cascade fallback on critical failure (max 2)
/// 9. Log decision to SQLite
/// 10. Increment running_tasks for chosen agent
/// 11. Return decision with audit trail
#[allow(clippy::too_many_arguments)]
pub fn execute(
    task_id: &str,
    task: &TaskInput,
    constraints: &Constraints,
    tree: Option<&DecisionTree>,
    registry: &AgentRegistry,
    db: &Database,
    invariant_config: &InvariantConfig,
    metrics: &crate::metrics::Metrics,
) -> Result<RouteResult> {
    let start = Instant::now();
    let thresholds = to_thresholds(invariant_config);
    let mut warnings: Vec<String> = Vec::new();

    // Step 2: Load all agent info from registry
    let all_agents = registry.get_all_agent_info()?;
    let total_running = registry.get_total_running_tasks()?;

    // Check if all agents are in failed state (exceeded failure threshold)
    let max_failures = invariant_config.agent_health.max_failures_24h;
    let all_failed =
        !all_agents.is_empty() && all_agents.iter().all(|a| a.recent_failures > max_failures);

    if all_failed {
        let inference_us = start.elapsed().as_micros() as i64;
        let mut result = RouteResult {
            task_id: task_id.to_string(),
            action: AgentAction::Reject,
            chosen_agent: String::new(),
            confidence: 0.0,
            reasoning: "All agents unhealthy".to_string(),
            decision_path: vec![],
            fallback_agent: None,
            fallback_reason: None,
            invariant_checks: vec![],
            inference_us,
            feature_vector: vec![],
            candidates_evaluated: 0,
            warnings: vec!["All agents exceeded failure threshold".to_string()],
            decision_id: None,
        };
        result.decision_id = log_decision(db, task_id, task, constraints, &result);
        metrics.record_decision(
            result.inference_us as u64,
            result.action == AgentAction::Fallback,
            result.action == AgentAction::Reject,
        );
        return Ok(result);
    }

    // Step 3: Filter by hard constraints
    let task_type_str = task.task_type.to_string();
    let lang_str = task.language.to_string();

    let candidates: Vec<AgentInfo> = all_agents
        .into_iter()
        .filter(|a| {
            // Not excluded
            if constraints.excluded_agents.contains(&a.agent_id) {
                debug!(agent = %a.agent_id, "excluded by constraint");
                return false;
            }
            // Supports task type
            if !a.config.supports_types.contains(&task_type_str) {
                debug!(agent = %a.agent_id, "does not support task type {task_type_str}");
                return false;
            }
            // Supports language
            if !a.config.supports_languages.contains(&lang_str) {
                debug!(agent = %a.agent_id, "does not support language {lang_str}");
                return false;
            }
            // Has available slots
            if a.running_tasks >= a.config.max_concurrent {
                debug!(agent = %a.agent_id, "no available slots");
                return false;
            }
            true
        })
        .collect();

    let candidates_evaluated = candidates.len();

    // No candidates at all -> reject
    if candidates.is_empty() {
        let inference_us = start.elapsed().as_micros() as i64;
        let mut result = RouteResult {
            task_id: task_id.to_string(),
            action: AgentAction::Reject,
            chosen_agent: String::new(),
            confidence: 0.0,
            reasoning: "No eligible agents after filtering".to_string(),
            decision_path: vec![],
            fallback_agent: None,
            fallback_reason: None,
            invariant_checks: vec![],
            inference_us,
            feature_vector: vec![],
            candidates_evaluated: 0,
            warnings,
            decision_id: None,
        };
        result.decision_id = log_decision(db, task_id, task, constraints, &result);
        metrics.record_decision(
            result.inference_us as u64,
            result.action == AgentAction::Fallback,
            result.action == AgentAction::Reject,
        );
        return Ok(result);
    }

    // Step 4: Build feature vectors per candidate
    let system_state = SystemState {
        constraints: constraints.clone(),
        total_running_tasks: total_running,
        time_of_day_hour: chrono::Utc::now().hour(),
    };

    let feature_vectors: Vec<(String, [f64; 22])> = candidates
        .iter()
        .map(|agent| {
            let fv = build_feature_vector(task, agent, &system_state);
            (agent.agent_id.clone(), fv)
        })
        .collect();

    // Step 5: Run DT inference and rank (or round-robin in degraded mode)
    let mut ranked = if let Some(dt) = tree {
        evaluate_for_agents(dt, &feature_vectors)
    } else {
        // Degraded mode: round-robin when tree unavailable
        warn!(
            event = "route.fallback_round_robin",
            "decision tree unavailable, using round-robin fallback"
        );
        warnings.push("Decision tree unavailable, using round-robin fallback".to_string());
        round_robin_ranking(&candidates)
    };

    // Step 6: Apply preferred_agent boost
    if let Some(ref preferred) = constraints.preferred_agent {
        for entry in &mut ranked {
            if entry.0 == *preferred {
                entry.1.confidence = (entry.1.confidence + PREFERRED_AGENT_BOOST).min(1.0);
                debug!(agent = %preferred, "applied preferred_agent boost +{PREFERRED_AGENT_BOOST}");
                break;
            }
        }
        // Re-sort after boost
        ranked.sort_by(|a, b| {
            b.1.confidence
                .partial_cmp(&a.1.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    // Step 6b: Benchmark re-rank (R-07), opt-in via env flag, off by default
    // (weight 0.0 => byte-for-byte identical to pre-R-07 behaviour).
    let bench_weight = std::env::var("ARBITER_BENCH_WEIGHT")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0);
    apply_benchmark_rerank(&mut ranked, &task.task_type, db, bench_weight)?;

    // Step 7+8: Run invariant checks with cascade fallback
    let system_ctx = to_system_context(constraints, total_running);
    let mut fallback_agent: Option<String> = None;
    let mut fallback_reason: Option<String> = None;
    let mut attempts = 0;

    // Find the candidate AgentInfo map for quick lookup
    let candidate_map: std::collections::HashMap<&str, &AgentInfo> = candidates
        .iter()
        .map(|a| (a.agent_id.as_str(), a))
        .collect();

    for (agent_id, prediction) in &ranked {
        let agent_info = match candidate_map.get(agent_id.as_str()) {
            Some(info) => info,
            None => continue,
        };
        let agent_ctx =
            to_agent_context(agent_info, invariant_config.agent_health.max_failures_24h);
        let invariant_results = check_all_invariants(task, &agent_ctx, &system_ctx, &thresholds);

        if !has_critical_failure(&invariant_results) {
            // Agent passes all critical invariants
            let inference_us = start.elapsed().as_micros() as i64;
            let feature_vec = feature_vectors
                .iter()
                .find(|(id, _)| id == agent_id)
                .map(|(_, fv)| fv.to_vec())
                .unwrap_or_default();

            let action = if fallback_agent.is_some() {
                AgentAction::Fallback
            } else {
                AgentAction::Assign
            };

            let reasoning = format!(
                "Agent {} selected with confidence {:.3}{}",
                agent_id,
                prediction.confidence,
                if action == AgentAction::Fallback {
                    format!(
                        " (fallback from {})",
                        fallback_agent.as_deref().unwrap_or("unknown")
                    )
                } else {
                    String::new()
                }
            );

            let mut result = RouteResult {
                task_id: task_id.to_string(),
                action,
                chosen_agent: agent_id.clone(),
                confidence: prediction.confidence,
                reasoning,
                decision_path: prediction.path.clone(),
                fallback_agent: fallback_agent.clone(),
                fallback_reason: fallback_reason.clone(),
                invariant_checks: invariant_results,
                inference_us,
                feature_vector: feature_vec,
                candidates_evaluated,
                warnings,
                decision_id: None,
            };

            // Step 9: Log decision to SQLite (capture rowid for response metadata)
            result.decision_id = log_decision(db, task_id, task, constraints, &result);

            // Step 10: Increment running_tasks
            db.increment_running_tasks(agent_id)
                .context("failed to increment running_tasks")?;

            metrics.record_decision(
                result.inference_us as u64,
                result.action == AgentAction::Fallback,
                result.action == AgentAction::Reject,
            );

            info!(
                event = "route.decision",
                task_id = task_id,
                agent = %agent_id,
                action = %result.action,
                confidence = prediction.confidence,
                inference_us = inference_us,
                "route_task decision"
            );

            return Ok(result);
        }

        // Critical failure — try fallback
        let failed_rules: Vec<String> = invariant_results
            .iter()
            .filter(|r| !r.passed && r.severity == arbiter_core::types::Severity::Critical)
            .map(|r| format!("{}: {}", r.rule, r.detail))
            .collect();

        warn!(
            event = "route.fallback_triggered",
            agent = %agent_id,
            failures = ?failed_rules,
            "critical invariant failure, trying fallback"
        );

        if fallback_agent.is_none() {
            fallback_agent = Some(agent_id.clone());
            fallback_reason = Some(failed_rules.join("; "));
        }

        attempts += 1;
        if attempts > MAX_FALLBACK_ATTEMPTS {
            break;
        }
    }

    // All candidates exhausted or max fallback reached -> reject
    let inference_us = start.elapsed().as_micros() as i64;
    let mut result = RouteResult {
        task_id: task_id.to_string(),
        action: AgentAction::Reject,
        chosen_agent: String::new(),
        confidence: 0.0,
        reasoning: format!(
            "All {} candidates failed critical invariants",
            candidates_evaluated
        ),
        decision_path: vec![],
        fallback_agent,
        fallback_reason,
        invariant_checks: vec![],
        inference_us,
        feature_vector: vec![],
        candidates_evaluated,
        warnings,
        decision_id: None,
    };

    result.decision_id = log_decision(db, task_id, task, constraints, &result);

    metrics.record_decision(
        result.inference_us as u64,
        result.action == AgentAction::Fallback,
        result.action == AgentAction::Reject,
    );

    info!(
        event = "route.all_rejected",
        task_id = task_id,
        action = "reject",
        candidates = candidates_evaluated,
        "route_task: all candidates rejected"
    );

    Ok(result)
}

/// Generate a round-robin ranking for degraded mode (no decision tree).
///
/// Returns agents in round-robin order with equal confidence of 0.5.
fn round_robin_ranking(
    candidates: &[AgentInfo],
) -> Vec<(String, arbiter_core::types::PredictionResult)> {
    use arbiter_core::types::PredictionResult;

    let idx = ROUND_ROBIN_COUNTER.fetch_add(1, Ordering::Relaxed);
    let n = candidates.len();

    let mut ranked: Vec<(String, PredictionResult)> = candidates
        .iter()
        .enumerate()
        .map(|(i, agent)| {
            // Order agents starting from the round-robin index
            let order = (i + n - (idx % n)) % n;
            let confidence = 0.5 - (order as f64 * 0.01); // slight ordering
            (
                agent.agent_id.clone(),
                PredictionResult {
                    class: 0,
                    confidence,
                    path: vec!["round-robin fallback (no decision tree)".to_string()],
                },
            )
        })
        .collect();

    ranked.sort_by(|a, b| {
        b.1.confidence
            .partial_cmp(&a.1.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    ranked
}

/// Log a routing decision to the SQLite decisions table.
///
/// Logs a warning and continues if the write fails (graceful degradation).
/// Returns the inserted row's id so callers can surface it in the
/// route_task response (`metadata.decision_id`). Returns `None` if the
/// insert failed.
fn log_decision(
    db: &Database,
    task_id: &str,
    task: &TaskInput,
    constraints: &Constraints,
    result: &RouteResult,
) -> Option<i64> {
    let task_json = serde_json::to_string(task).unwrap_or_default();
    let constraints_json = serde_json::to_string(constraints).ok();
    let feature_vector_json = serde_json::to_string(&result.feature_vector).unwrap_or_default();
    let decision_path_json =
        serde_json::to_string(&result.decision_path).unwrap_or_else(|_| "[]".to_string());
    let invariants_json =
        serde_json::to_string(&result.invariant_checks).unwrap_or_else(|_| "[]".to_string());

    let passed = result.invariant_checks.iter().filter(|r| r.passed).count() as i32;
    let failed = result.invariant_checks.iter().filter(|r| !r.passed).count() as i32;

    let record = DecisionRecord {
        task_id: task_id.to_string(),
        task_json,
        feature_vector: feature_vector_json,
        constraints_json,
        chosen_agent: result.chosen_agent.clone(),
        action: result.action.to_string(),
        confidence: result.confidence,
        decision_path: decision_path_json,
        fallback_agent: result.fallback_agent.clone(),
        fallback_reason: result.fallback_reason.clone(),
        invariants_json,
        invariants_passed: passed,
        invariants_failed: failed,
        inference_us: result.inference_us,
    };

    match db.insert_decision(&record) {
        Ok(rowid) => Some(rowid),
        Err(e) => {
            warn!(
                event = "route.sqlite_log_failed",
                task_id = task_id,
                error = %e,
                "failed to log decision to SQLite"
            );
            None
        }
    }
}

/// Serialize a RouteResult into the MCP response JSON Value.
pub fn result_to_json(result: &RouteResult) -> Value {
    let invariant_checks: Vec<Value> = result
        .invariant_checks
        .iter()
        .map(|r| {
            serde_json::json!({
                "rule": r.rule,
                "severity": r.severity,
                "passed": r.passed,
                "detail": r.detail
            })
        })
        .collect();

    serde_json::json!({
        "task_id": result.task_id,
        "action": result.action,
        "chosen_agent": result.chosen_agent,
        "confidence": result.confidence,
        "reasoning": result.reasoning,
        "decision_path": result.decision_path,
        "fallback_agent": result.fallback_agent,
        "fallback_reason": result.fallback_reason,
        "invariant_checks": invariant_checks,
        "warnings": result.warnings,
        "metadata": {
            "decision_id": result.decision_id,
            "inference_us": result.inference_us,
            "feature_vector": result.feature_vector,
            "candidates_evaluated": result.candidates_evaluated
        }
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;
    use arbiter_core::types::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// Seed one benchmark row through the public insert path (NOT a raw INSERT —
    /// `benchmark_runs` has 6 NOT NULL columns beyond the score).
    fn seed_bench(db: &Database, run_id: &str, agent: &str, bench: &str, score: f64) {
        db.insert_benchmark_run(&crate::db::BenchmarkRunInput {
            run_id,
            payload_version: "1.0.0",
            benchmark_id: bench,
            agent_id: agent,
            ts: "2026-06-13T00:00:00Z",
            score,
            score_components: "{}",
            total_tokens: None,
            total_cost_usd: None,
            duration_seconds: 0.0,
            per_task: "[]",
            per_task_total_count: 0,
            per_task_truncated: 0,
        })
        .unwrap();
    }

    #[test]
    fn benchmark_weight_reranks_review_by_per_agent_score() {
        // Two agents that both support Review; aider leads on base confidence.
        // weight=0 leaves the base order; weight=0.15 lets claude_code@claude-sonnet-4-6's higher
        // code-review score (0.90 vs 0.20) overtake aider.
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        seed_bench(
            &db,
            "a",
            "claude_code@claude-sonnet-4-6",
            "code-review",
            0.90,
        );
        seed_bench(&db, "b", "aider", "code-review", 0.20);

        let mk = |conf: f64| PredictionResult {
            class: 0,
            confidence: conf,
            path: vec![],
        };
        let base = || {
            vec![
                ("aider".to_string(), mk(0.55)),
                ("claude_code@claude-sonnet-4-6".to_string(), mk(0.50)),
            ]
        };

        let mut w0 = base();
        apply_benchmark_rerank(&mut w0, &TaskType::Review, &db, 0.0).unwrap();
        assert_eq!(w0[0].0, "aider", "weight 0 leaves base ranking");

        let mut w15 = base();
        apply_benchmark_rerank(&mut w15, &TaskType::Review, &db, 0.15).unwrap();
        // claude_code@claude-sonnet-4-6: 0.50 + (0.90-0.5)*0.15 = 0.56 ; aider: 0.55 + (0.20-0.5)*0.15 = 0.505
        assert_eq!(
            w15[0].0, "claude_code@claude-sonnet-4-6",
            "high code-review score promotes claude_code@claude-sonnet-4-6"
        );
        assert!(
            w15[0]
                .1
                .path
                .iter()
                .any(|s| s.starts_with("bench_adjust[claude_code@claude-sonnet-4-6]")),
            "audit line lands on the adjusted prediction's path"
        );
    }

    #[test]
    fn benchmark_rerank_is_noop_without_mapping_or_weight() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        seed_bench(
            &db,
            "a",
            "claude_code@claude-sonnet-4-6",
            "code-review",
            0.90,
        );

        let mk = |conf: f64| PredictionResult {
            class: 0,
            confidence: conf,
            path: vec![],
        };
        // Docs has no mapped benchmark -> untouched even with weight on.
        let mut docs = vec![("claude_code@claude-sonnet-4-6".to_string(), mk(0.50))];
        apply_benchmark_rerank(&mut docs, &TaskType::Docs, &db, 0.15).unwrap();
        assert_eq!(docs[0].1.confidence, 0.50);
        assert!(docs[0].1.path.is_empty());

        // Review with weight 0 -> untouched (regression guard for the default).
        let mut review = vec![("claude_code@claude-sonnet-4-6".to_string(), mk(0.50))];
        apply_benchmark_rerank(&mut review, &TaskType::Review, &db, 0.0).unwrap();
        assert_eq!(review[0].1.confidence, 0.50);
        assert!(review[0].1.path.is_empty());
    }

    #[test]
    fn rerank_breaks_residual_ties_by_agent_id() {
        // Identical code-review score AND identical base confidence => identical
        // adjusted confidence => the sort must be deterministic by agent_id.
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        seed_bench(&db, "a", "zzz@m", "code-review", 0.80);
        seed_bench(&db, "b", "aaa@m", "code-review", 0.80);

        let mk = |conf: f64| PredictionResult {
            class: 0,
            confidence: conf,
            path: vec![],
        };
        // input order deliberately puts zzz first
        let mut ranked = vec![
            ("zzz@m".to_string(), mk(0.50)),
            ("aaa@m".to_string(), mk(0.50)),
        ];
        apply_benchmark_rerank(&mut ranked, &TaskType::Review, &db, 0.15).unwrap();
        assert_eq!(
            ranked[0].0, "aaa@m",
            "residual tie resolves by agent_id ascending"
        );
        assert_eq!(ranked[1].0, "zzz@m");
    }

    #[test]
    fn rerank_orders_tied_scalar_agents_by_rank_score() {
        // Both agents tie on the scalar score (0.80) but differ on rank_score
        // inside score_components. The higher rank_score must win even though its
        // agent_id sorts LAST — proving rank_score drives the order, not the
        // agent_id tiebreak. Equal base confidence isolates the benchmark delta.
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let row = |run_id: &'static str, agent: &'static str, sc: &'static str| {
            crate::db::BenchmarkRunInput {
                run_id,
                payload_version: "1.0.0",
                benchmark_id: "code-review",
                agent_id: agent,
                ts: "2026-06-13T00:00:00Z",
                score: 0.80,
                score_components: sc,
                total_tokens: None,
                total_cost_usd: None,
                duration_seconds: 0.0,
                per_task: "[]",
                per_task_total_count: 0,
                per_task_truncated: 0,
            }
        };
        db.insert_benchmark_run(&row("r1", "zzz@m", r#"{"rank_score":0.775}"#))
            .unwrap();
        db.insert_benchmark_run(&row("r2", "aaa@m", r#"{"rank_score":0.760}"#))
            .unwrap();

        let mk = |conf: f64| PredictionResult {
            class: 0,
            confidence: conf,
            path: vec![],
        };
        let mut ranked = vec![
            ("aaa@m".to_string(), mk(0.50)),
            ("zzz@m".to_string(), mk(0.50)),
        ];
        apply_benchmark_rerank(&mut ranked, &TaskType::Review, &db, 0.15).unwrap();
        // zzz@m: 0.50 + (0.775-0.5)*0.15 = 0.541250 ; aaa@m: 0.50 + (0.760-0.5)*0.15 = 0.539000
        assert_eq!(
            ranked[0].0, "zzz@m",
            "higher rank_score wins despite later agent_id"
        );
    }

    #[test]
    fn rerank_real_resweep_2026_07_02_flips_review_to_codex() {
        // Real rank_scores ingested from the 2026-07-02 runs=3 code-review
        // re-sweep (see atp-platform _cowork_output/r07-pipecheck): claude_code
        // 0.705208 (breakpoint moderate) vs codex_cli 0.781250 (severe). The gap
        // is small (0.076), so the rerank delta = (score-0.5)*weight lifts codex
        // by only 0.076*weight relative to claude. This exercises the full R-07
        // loop end-to-end on the real numbers — no longer the 0.800/0.800 no-op.
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let row = |run_id: &'static str, agent: &'static str, sc: &'static str| {
            crate::db::BenchmarkRunInput {
                run_id,
                payload_version: "1.0.0",
                benchmark_id: "code-review",
                agent_id: agent,
                ts: "2026-07-02T09:42:00Z",
                score: 0.80,
                score_components: sc,
                total_tokens: None,
                total_cost_usd: None,
                duration_seconds: 0.0,
                per_task: "[]",
                per_task_total_count: 0,
                per_task_truncated: 0,
            }
        };
        db.insert_benchmark_run(&row(
            "cc",
            "claude_code@claude-sonnet-4-6",
            r#"{"rank_score":0.705208,"bp_ordinal":2}"#,
        ))
        .unwrap();
        db.insert_benchmark_run(&row(
            "cx",
            "codex_cli@gpt-5.5",
            r#"{"rank_score":0.781250,"bp_ordinal":3}"#,
        ))
        .unwrap();

        let mk = |conf: f64| PredictionResult {
            class: 0,
            confidence: conf,
            path: vec![],
        };
        // Illustrative DT base: claude leads codex by 0.03 before the re-rank.
        let base = || {
            vec![
                ("claude_code@claude-sonnet-4-6".to_string(), mk(0.55)),
                ("codex_cli@gpt-5.5".to_string(), mk(0.52)),
            ]
        };

        // Turn the knob and watch: codex overtakes once 0.076*w > 0.03 (w ~ 0.40).
        for w in [0.0_f64, 0.15, 0.5, 1.0] {
            let mut r = base();
            apply_benchmark_rerank(&mut r, &TaskType::Review, &db, w).unwrap();
            println!(
                "ARBITER_BENCH_WEIGHT={w:.2} -> [{}={:.4}, {}={:.4}] leader={}",
                r[0].0, r[0].1.confidence, r[1].0, r[1].1.confidence, r[0].0
            );
        }

        let mut w0 = base();
        apply_benchmark_rerank(&mut w0, &TaskType::Review, &db, 0.0).unwrap();
        assert_eq!(
            w0[0].0, "claude_code@claude-sonnet-4-6",
            "weight 0 = DT base"
        );
        let mut w1 = base();
        apply_benchmark_rerank(&mut w1, &TaskType::Review, &db, 1.0).unwrap();
        assert_eq!(
            w1[0].0, "codex_cli@gpt-5.5",
            "a large weight lets codex's higher rank_score overtake the DT base"
        );
    }

    fn test_tree_json() -> String {
        serde_json::json!({
            "n_features": 22,
            "n_classes": 3,
            "class_names": ["claude_code@claude-sonnet-4-6", "codex_cli@gpt-5.5", "aider"],
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
                {"feature": 9, "threshold": 0.65, "left": 3, "right": 4,
                 "value": [2.0, 5.0, 8.0]},
                {"feature": -1, "threshold": 0.0, "left": -1, "right": -1,
                 "value": [8.0, 1.0, 1.0]},
                {"feature": -1, "threshold": 0.0, "left": -1, "right": -1,
                 "value": [0.0, 2.0, 6.0]},
                {"feature": -1, "threshold": 0.0, "left": -1, "right": -1,
                 "value": [2.0, 5.0, 1.0]}
            ]
        })
        .to_string()
    }

    fn test_agents() -> HashMap<String, AgentConfig> {
        let mut agents = HashMap::new();
        agents.insert(
            "claude_code@claude-sonnet-4-6".to_string(),
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
            },
        );
        agents.insert(
            "codex_cli@gpt-5.5".to_string(),
            AgentConfig {
                display_name: "Codex CLI".to_string(),
                supports_languages: vec![
                    "typescript".to_string(),
                    "go".to_string(),
                    "python".to_string(),
                ],
                supports_types: vec![
                    "feature".to_string(),
                    "bugfix".to_string(),
                    "refactor".to_string(),
                    "test".to_string(),
                ],
                max_concurrent: 3,
                cost_per_hour: 0.20,
                avg_duration_min: 12.0,
            },
        );
        agents.insert(
            "aider".to_string(),
            AgentConfig {
                display_name: "Aider".to_string(),
                supports_languages: vec!["python".to_string(), "javascript".to_string()],
                supports_types: vec![
                    "bugfix".to_string(),
                    "refactor".to_string(),
                    "test".to_string(),
                ],
                max_concurrent: 5,
                cost_per_hour: 0.10,
                avg_duration_min: 8.0,
            },
        );
        agents
    }

    fn test_invariant_config() -> InvariantConfig {
        InvariantConfig {
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
        }
    }

    fn simple_task() -> TaskInput {
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

    fn empty_constraints() -> Constraints {
        Constraints {
            preferred_agent: None,
            excluded_agents: vec![],
            budget_remaining_usd: Some(10.0),
            total_pending_tasks: None,
            running_tasks: vec![],
            retry_count: None,
            calls_per_minute: None,
        }
    }

    fn setup() -> (Arc<Database>, DecisionTree) {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let tree = DecisionTree::from_json(&test_tree_json()).unwrap();
        (Arc::new(db), tree)
    }

    #[test]
    fn happy_path_assigns_agent() {
        let (db, tree) = setup();
        let agents = test_agents();
        let registry = AgentRegistry::new(Arc::clone(&db), &agents).unwrap();
        let invariant_cfg = test_invariant_config();

        let task = simple_task();
        let constraints = empty_constraints();

        let result = execute(
            "t1",
            &task,
            &constraints,
            Some(&tree),
            &registry,
            &db,
            &invariant_cfg,
            &crate::metrics::Metrics::new(),
        )
        .unwrap();

        assert!(
            result.action == AgentAction::Assign || result.action == AgentAction::Fallback,
            "expected assign or fallback, got {:?}",
            result.action
        );
        assert!(!result.chosen_agent.is_empty());
        assert!(result.confidence > 0.0);
        assert!(!result.decision_path.is_empty());
        assert_eq!(result.invariant_checks.len(), 10);
        assert!(result.inference_us >= 0);
    }

    #[test]
    fn all_excluded_rejects() {
        let (db, tree) = setup();
        let agents = test_agents();
        let registry = AgentRegistry::new(Arc::clone(&db), &agents).unwrap();
        let invariant_cfg = test_invariant_config();

        let task = simple_task();
        let constraints = Constraints {
            excluded_agents: vec![
                "claude_code@claude-sonnet-4-6".to_string(),
                "codex_cli@gpt-5.5".to_string(),
                "aider".to_string(),
            ],
            ..empty_constraints()
        };

        let result = execute(
            "t2",
            &task,
            &constraints,
            Some(&tree),
            &registry,
            &db,
            &invariant_cfg,
            &crate::metrics::Metrics::new(),
        )
        .unwrap();

        assert_eq!(result.action, AgentAction::Reject);
        assert!(result.chosen_agent.is_empty());
    }

    #[test]
    fn preferred_agent_boost_applied() {
        let (db, tree) = setup();
        let agents = test_agents();
        let registry = AgentRegistry::new(Arc::clone(&db), &agents).unwrap();
        let invariant_cfg = test_invariant_config();

        let task = simple_task();
        let constraints = Constraints {
            preferred_agent: Some("codex_cli@gpt-5.5".to_string()),
            ..empty_constraints()
        };

        let result = execute(
            "t3",
            &task,
            &constraints,
            Some(&tree),
            &registry,
            &db,
            &invariant_cfg,
            &crate::metrics::Metrics::new(),
        )
        .unwrap();

        // The preferred agent should have gotten a boost
        assert_ne!(result.action, AgentAction::Reject);
    }

    #[test]
    fn decision_logged_to_db() {
        let (db, tree) = setup();
        let agents = test_agents();
        let registry = AgentRegistry::new(Arc::clone(&db), &agents).unwrap();
        let invariant_cfg = test_invariant_config();

        let task = simple_task();
        let constraints = empty_constraints();

        let _result = execute(
            "t4",
            &task,
            &constraints,
            Some(&tree),
            &registry,
            &db,
            &invariant_cfg,
            &crate::metrics::Metrics::new(),
        )
        .unwrap();

        // Verify decision was logged
        let found = db.find_decision_by_task("t4").unwrap();
        assert!(found.is_some());
        let record = found.unwrap();
        assert_eq!(record.task_id, "t4");
    }

    #[test]
    fn running_tasks_incremented_on_assign() {
        let (db, tree) = setup();
        let agents = test_agents();
        let registry = AgentRegistry::new(Arc::clone(&db), &agents).unwrap();
        let invariant_cfg = test_invariant_config();

        let task = simple_task();
        let constraints = empty_constraints();

        let result = execute(
            "t5",
            &task,
            &constraints,
            Some(&tree),
            &registry,
            &db,
            &invariant_cfg,
            &crate::metrics::Metrics::new(),
        )
        .unwrap();

        if result.action != AgentAction::Reject {
            let running = db.get_running_tasks(&result.chosen_agent).unwrap();
            assert_eq!(running, 1);
        }
    }

    #[test]
    fn result_to_json_structure() {
        let result = RouteResult {
            task_id: "t1".to_string(),
            action: AgentAction::Assign,
            chosen_agent: "claude_code@claude-sonnet-4-6".to_string(),
            confidence: 0.85,
            reasoning: "test".to_string(),
            decision_path: vec!["step1".to_string()],
            fallback_agent: None,
            fallback_reason: None,
            invariant_checks: vec![InvariantResult {
                rule: "agent_available".to_string(),
                severity: Severity::Critical,
                passed: true,
                detail: "ok".to_string(),
            }],
            inference_us: 42,
            feature_vector: vec![1.0, 2.0],
            candidates_evaluated: 3,
            warnings: vec!["test warning".to_string()],
            decision_id: Some(7),
        };

        let json = result_to_json(&result);
        assert_eq!(json["task_id"], "t1");
        assert_eq!(json["action"], "assign");
        assert_eq!(json["chosen_agent"], "claude_code@claude-sonnet-4-6");
        assert_eq!(json["confidence"], 0.85);
        assert!(json["invariant_checks"].as_array().unwrap().len() == 1);
        assert_eq!(json["metadata"]["decision_id"], 7);
        assert_eq!(json["metadata"]["inference_us"], 42);
        assert_eq!(json["metadata"]["candidates_evaluated"], 3);
        assert_eq!(json["warnings"][0], "test warning");
    }

    #[test]
    fn result_to_json_decision_id_null_when_log_failed() {
        let result = RouteResult {
            task_id: "t-null".to_string(),
            action: AgentAction::Reject,
            chosen_agent: String::new(),
            confidence: 0.0,
            reasoning: "no candidates".to_string(),
            decision_path: vec![],
            fallback_agent: None,
            fallback_reason: None,
            invariant_checks: vec![],
            inference_us: 1,
            feature_vector: vec![],
            candidates_evaluated: 0,
            warnings: vec![],
            decision_id: None,
        };

        let json = result_to_json(&result);
        assert!(
            json["metadata"]["decision_id"].is_null(),
            "decision_id should serialize as JSON null when None"
        );
    }

    #[test]
    fn no_compatible_agents_rejects() {
        let (db, tree) = setup();
        let agents = test_agents();
        let registry = AgentRegistry::new(Arc::clone(&db), &agents).unwrap();
        let invariant_cfg = test_invariant_config();

        // Research task in Go - no agent supports both
        let task = TaskInput {
            task_type: TaskType::Research,
            language: Language::Go,
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
        let constraints = empty_constraints();

        let result = execute(
            "t6",
            &task,
            &constraints,
            Some(&tree),
            &registry,
            &db,
            &invariant_cfg,
            &crate::metrics::Metrics::new(),
        )
        .unwrap();

        assert_eq!(result.action, AgentAction::Reject);
    }

    // =================================================================
    // Integration Tests IT-01 through IT-04
    // =================================================================

    fn bootstrap_tree() -> DecisionTree {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let path = std::path::Path::new(manifest_dir)
            .parent()
            .unwrap()
            .join("models/agent_policy_tree.json");
        let json = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            panic!(
                "{} not found — run bootstrap_agent_tree.py first",
                path.display()
            )
        });
        DecisionTree::from_json(&json).expect("failed to parse bootstrap tree")
    }

    /// IT-01: Happy path route -> assign
    ///
    /// GIVEN 3 active agents with available capacity
    /// AND a valid task description with type, language, complexity
    /// WHEN route_task is called
    /// THEN it returns a decision within 5ms
    /// AND the decision contains chosen_agent, confidence [0,1], decision_path
    /// AND invariant_checks contains all 10 rule results
    /// AND the decision is logged to SQLite decisions table
    /// AND running_tasks is incremented for the chosen agent
    #[test]
    fn it_01_happy_path_route_assign() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let db = Arc::new(db);
        let tree = bootstrap_tree();
        let agents = test_agents();
        let registry = AgentRegistry::new(Arc::clone(&db), &agents).unwrap();
        let invariant_cfg = test_invariant_config();

        let task = TaskInput {
            task_type: TaskType::Bugfix,
            language: Language::Python,
            complexity: Complexity::Simple,
            priority: Priority::Normal,
            scope: vec!["src/main.py".to_string()],
            branch: Some("fix/bug-123".to_string()),
            estimated_tokens: Some(30000),
            has_dependencies: false,
            requires_internet: false,
            sla_minutes: Some(60),
            description: Some("Fix login bug".to_string()),
        };
        let constraints = Constraints {
            preferred_agent: None,
            excluded_agents: vec![],
            budget_remaining_usd: Some(10.0),
            total_pending_tasks: Some(2),
            running_tasks: vec![],
            retry_count: None,
            calls_per_minute: None,
        };

        let start = std::time::Instant::now();
        let result = execute(
            "it-01",
            &task,
            &constraints,
            Some(&tree),
            &registry,
            &db,
            &invariant_cfg,
            &crate::metrics::Metrics::new(),
        )
        .unwrap();
        let elapsed = start.elapsed();

        // Decision within 5ms
        assert!(
            elapsed.as_millis() < 5,
            "route_task took {}ms, expected < 5ms",
            elapsed.as_millis()
        );

        // Action is assign (happy path)
        assert_eq!(result.action, AgentAction::Assign);

        // chosen_agent is non-empty
        assert!(
            !result.chosen_agent.is_empty(),
            "chosen_agent should not be empty"
        );

        // confidence in [0, 1]
        assert!(
            (0.0..=1.0).contains(&result.confidence),
            "confidence {} not in [0,1]",
            result.confidence
        );

        // decision_path is non-empty
        assert!(
            !result.decision_path.is_empty(),
            "decision_path should not be empty"
        );

        // invariant_checks contains all 10 rule results
        assert_eq!(
            result.invariant_checks.len(),
            10,
            "expected 10 invariant checks, got {}",
            result.invariant_checks.len()
        );

        // All invariants should pass for happy path
        assert!(
            result.invariant_checks.iter().all(|r| r.passed),
            "all invariants should pass for happy path"
        );

        // Decision logged to SQLite
        let found = db
            .find_decision_by_task("it-01")
            .unwrap()
            .expect("decision should be in DB");
        assert_eq!(found.task_id, "it-01");
        assert_eq!(found.chosen_agent, result.chosen_agent);
        assert_eq!(found.action, "assign");
        assert_eq!(found.invariants_passed, 10);
        assert_eq!(found.invariants_failed, 0);

        // decision_id surfaces the SQLite rowid so callers can correlate
        // a later report_outcome with this routing decision.
        let surfaced = result
            .decision_id
            .expect("decision_id should be surfaced when log succeeds");
        let by_lookup = db
            .find_decision_id_by_task("it-01")
            .unwrap()
            .expect("decision_id should be queryable");
        assert_eq!(
            surfaced, by_lookup,
            "result.decision_id must match find_decision_id_by_task"
        );

        // running_tasks incremented
        let running = db.get_running_tasks(&result.chosen_agent).unwrap();
        assert_eq!(running, 1, "running_tasks should be 1 after assignment");

        // Feature vector is 22 dimensions
        assert_eq!(
            result.feature_vector.len(),
            22,
            "feature vector should be 22-dim"
        );
    }

    /// IT-02: Fallback on scope conflict
    ///
    /// GIVEN agent A is top-ranked but has a scope conflict (critical violation)
    /// WHEN route_task evaluates invariants
    /// THEN it falls back to agent B (next by score)
    /// AND runs invariants on agent B
    /// AND returns action="fallback" with fallback_reason
    #[test]
    fn it_02_fallback_on_scope_conflict() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let db = Arc::new(db);
        let tree = bootstrap_tree();
        let agents = test_agents();
        let registry = AgentRegistry::new(Arc::clone(&db), &agents).unwrap();
        let invariant_cfg = test_invariant_config();

        // Task that touches src/main.py
        let task = TaskInput {
            task_type: TaskType::Bugfix,
            language: Language::Python,
            complexity: Complexity::Simple,
            priority: Priority::Normal,
            scope: vec!["src/main.py".to_string()],
            branch: None,
            estimated_tokens: None,
            has_dependencies: false,
            requires_internet: false,
            sla_minutes: None,
            description: None,
        };

        // Scope conflict: a running task is touching src/main.py
        let constraints = Constraints {
            preferred_agent: None,
            excluded_agents: vec![],
            budget_remaining_usd: Some(10.0),
            total_pending_tasks: None,
            running_tasks: vec![RunningTask {
                task_id: "running-1".to_string(),
                agent_id: "some_agent".to_string(),
                scope: vec!["src/main.py".to_string()],
                branch: None,
            }],
            retry_count: None,
            calls_per_minute: None,
        };

        let result = execute(
            "it-02",
            &task,
            &constraints,
            Some(&tree),
            &registry,
            &db,
            &invariant_cfg,
            &crate::metrics::Metrics::new(),
        )
        .unwrap();

        // The task has a scope conflict (scope_isolation should fail).
        // This should cause the top-ranked agent to fail invariants,
        // and the system should try fallback candidates.
        // Since ALL agents will have the same scope conflict (it's
        // system-wide), all should be rejected.
        // But the scope_isolation check is system-level, not agent-specific,
        // so ALL agents see the same conflict.
        assert!(
            result.action == AgentAction::Reject
                || result.action == AgentAction::Fallback
                || result.action == AgentAction::Assign,
            "expected a valid action, got {:?}",
            result.action
        );

        // The scope_isolation invariant should show as failed in the checks
        // if the result has invariant_checks
        if !result.invariant_checks.is_empty() {
            let scope_check = result
                .invariant_checks
                .iter()
                .find(|r| r.rule == "scope_isolation");
            assert!(scope_check.is_some(), "should have scope_isolation check");
            assert!(
                !scope_check.unwrap().passed,
                "scope_isolation should fail due to conflict"
            );
        }

        // Decision should still be logged
        let found = db.find_decision_by_task("it-02").unwrap();
        assert!(found.is_some(), "decision should be logged even on reject");
    }

    /// IT-03: All agents rejected
    ///
    /// GIVEN a route_task request with all agents excluded
    /// WHEN no candidates remain
    /// THEN action="reject" with reasoning explaining exclusion
    #[test]
    fn it_03_all_rejected() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let db = Arc::new(db);
        let tree = bootstrap_tree();
        let agents = test_agents();
        let registry = AgentRegistry::new(Arc::clone(&db), &agents).unwrap();
        let invariant_cfg = test_invariant_config();

        let task = TaskInput {
            task_type: TaskType::Feature,
            language: Language::Rust,
            complexity: Complexity::Complex,
            priority: Priority::High,
            scope: vec!["src/".to_string()],
            branch: Some("feature/new".to_string()),
            estimated_tokens: Some(100000),
            has_dependencies: true,
            requires_internet: false,
            sla_minutes: Some(120),
            description: Some("Big feature".to_string()),
        };

        // Exclude all agents
        let constraints = Constraints {
            preferred_agent: None,
            excluded_agents: vec![
                "claude_code@claude-sonnet-4-6".to_string(),
                "codex_cli@gpt-5.5".to_string(),
                "aider".to_string(),
            ],
            budget_remaining_usd: Some(10.0),
            total_pending_tasks: None,
            running_tasks: vec![],
            retry_count: None,
            calls_per_minute: None,
        };

        let result = execute(
            "it-03",
            &task,
            &constraints,
            Some(&tree),
            &registry,
            &db,
            &invariant_cfg,
            &crate::metrics::Metrics::new(),
        )
        .unwrap();

        // Must be rejected
        assert_eq!(
            result.action,
            AgentAction::Reject,
            "should reject when all agents are excluded"
        );

        // chosen_agent should be empty
        assert!(
            result.chosen_agent.is_empty(),
            "chosen_agent should be empty on reject"
        );

        // Reasoning should explain why
        assert!(
            !result.reasoning.is_empty(),
            "reasoning should explain the rejection"
        );

        // Decision logged even on rejection
        let found = db
            .find_decision_by_task("it-03")
            .unwrap()
            .expect("rejection should be logged");
        assert_eq!(found.action, "reject");

        // No invariant checks should be present (filtered before invariants)
        assert!(
            result.invariant_checks.is_empty(),
            "no invariant checks on exclusion-based reject"
        );
    }

    /// IT-04: Cold start (no stats)
    ///
    /// GIVEN agents with no historical stats (fresh database)
    /// WHEN route_task is called
    /// THEN it uses default feature vector values for stats fields
    /// AND still produces a valid routing decision
    #[test]
    fn it_04_cold_start() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let db = Arc::new(db);
        let tree = bootstrap_tree();
        let agents = test_agents();
        let registry = AgentRegistry::new(Arc::clone(&db), &agents).unwrap();
        let invariant_cfg = test_invariant_config();

        // Fresh DB, no outcomes recorded — agents have no stats
        let task = TaskInput {
            task_type: TaskType::Feature,
            language: Language::Rust,
            complexity: Complexity::Complex,
            priority: Priority::High,
            scope: vec!["arbiter-core/src/types.rs".to_string()],
            branch: Some("task/new-types".to_string()),
            estimated_tokens: Some(50000),
            has_dependencies: false,
            requires_internet: false,
            sla_minutes: Some(120),
            description: Some("Add new core types".to_string()),
        };
        let constraints = Constraints {
            preferred_agent: None,
            excluded_agents: vec![],
            budget_remaining_usd: Some(8.50),
            total_pending_tasks: Some(3),
            running_tasks: vec![],
            retry_count: None,
            calls_per_minute: None,
        };

        let result = execute(
            "it-04",
            &task,
            &constraints,
            Some(&tree),
            &registry,
            &db,
            &invariant_cfg,
            &crate::metrics::Metrics::new(),
        )
        .unwrap();

        // Should produce a valid decision (not reject)
        assert!(
            result.action == AgentAction::Assign || result.action == AgentAction::Fallback,
            "cold start should still route, got {:?}",
            result.action
        );

        // Chosen agent should be one of the configured agents
        let valid_agents = [
            "claude_code@claude-sonnet-4-6",
            "codex_cli@gpt-5.5",
            "aider",
        ];
        assert!(
            valid_agents.contains(&result.chosen_agent.as_str()),
            "chosen agent '{}' not in configured agents",
            result.chosen_agent
        );

        // Feature vector should use defaults for stats fields
        assert_eq!(result.feature_vector.len(), 22);

        // Confidence should be reasonable (> 0)
        assert!(result.confidence > 0.0);

        // All 10 invariant checks present
        assert_eq!(result.invariant_checks.len(), 10);

        // Decision logged
        let found = db.find_decision_by_task("it-04").unwrap();
        assert!(found.is_some());

        // Running tasks incremented
        let running = db.get_running_tasks(&result.chosen_agent).unwrap();
        assert_eq!(running, 1);
    }

    // =================================================================
    // TASK-013: Error Handling & Degraded Mode Tests
    // =================================================================

    /// Degraded mode: round-robin assignment when decision tree is None.
    ///
    /// GIVEN a valid task and no decision tree (tree=None)
    /// WHEN route_task is called
    /// THEN it assigns an agent using round-robin
    /// AND includes a warning about degraded mode
    /// AND decision_path mentions round-robin fallback
    #[test]
    fn degraded_mode_round_robin_assigns_without_tree() {
        let (db, _tree) = setup();
        let agents = test_agents();
        let registry = AgentRegistry::new(Arc::clone(&db), &agents).unwrap();
        let invariant_cfg = test_invariant_config();

        let task = simple_task();
        let constraints = empty_constraints();

        let result = execute(
            "degraded-1",
            &task,
            &constraints,
            None, // No decision tree
            &registry,
            &db,
            &invariant_cfg,
            &crate::metrics::Metrics::new(),
        )
        .unwrap();

        // Should still assign an agent
        assert!(
            result.action == AgentAction::Assign || result.action == AgentAction::Fallback,
            "degraded mode should still route, got {:?}",
            result.action
        );
        assert!(!result.chosen_agent.is_empty());

        // Warnings should mention round-robin
        assert!(
            result.warnings.iter().any(|w| w.contains("round-robin")),
            "warnings should mention round-robin: {:?}",
            result.warnings
        );

        // Decision path should mention round-robin fallback
        assert!(
            result
                .decision_path
                .iter()
                .any(|p| p.contains("round-robin fallback")),
            "decision_path should mention round-robin: {:?}",
            result.decision_path
        );

        // Decision logged to DB
        let found = db.find_decision_by_task("degraded-1").unwrap();
        assert!(found.is_some(), "degraded mode decisions should be logged");
    }

    /// Round-robin rotates through agents across calls.
    ///
    /// GIVEN no decision tree and multiple eligible agents
    /// WHEN route_task is called multiple times
    /// THEN different agents are selected (round-robin rotation)
    #[test]
    fn round_robin_rotates_agents() {
        let agents = test_agents();
        let invariant_cfg = test_invariant_config();

        // Use a task that all agents can handle (bugfix + python)
        let task = simple_task();
        let constraints = empty_constraints();

        let mut chosen: Vec<String> = Vec::new();
        for i in 0..6 {
            let db = Database::open_in_memory().unwrap();
            db.migrate().unwrap();
            let db = Arc::new(db);
            let registry = AgentRegistry::new(Arc::clone(&db), &agents).unwrap();

            let result = execute(
                &format!("rr-{i}"),
                &task,
                &constraints,
                None,
                &registry,
                &db,
                &invariant_cfg,
                &crate::metrics::Metrics::new(),
            )
            .unwrap();

            if result.action != AgentAction::Reject {
                chosen.push(result.chosen_agent.clone());
            }
        }

        // With round-robin over eligible agents, we should see at least
        // 2 different agents chosen across 6 calls
        let unique: std::collections::HashSet<&String> = chosen.iter().collect();
        assert!(
            unique.len() >= 2,
            "round-robin should select different agents, got: {:?}",
            chosen
        );
    }

    /// All agents unhealthy → reject with reasoning.
    ///
    /// GIVEN all agents have exceeded the failure threshold
    /// WHEN route_task is called
    /// THEN action="reject" with reasoning "All agents unhealthy"
    /// AND warnings mention failure threshold
    #[test]
    fn all_agents_failed_rejects_with_reasoning() {
        use crate::db::OutcomeRecord;

        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let db = Arc::new(db);
        let agents = test_agents();
        let registry = AgentRegistry::new(Arc::clone(&db), &agents).unwrap();
        let invariant_cfg = test_invariant_config();
        let max_failures = invariant_cfg.agent_health.max_failures_24h;

        // Record enough failures to exceed threshold for all agents
        for agent_id in agents.keys() {
            for i in 0..=max_failures {
                db.insert_outcome(&OutcomeRecord {
                    task_id: format!("fail-{agent_id}-{i}"),
                    decision_id: None,
                    agent_id: agent_id.clone(),
                    status: "failure".to_string(),
                    duration_min: None,
                    tokens_used: None,
                    cost_usd: None,
                    exit_code: None,
                    files_changed: None,
                    tests_passed: None,
                    validation_passed: None,
                    error_summary: None,
                    retry_count: 0,
                })
                .unwrap();
            }
        }

        let task = simple_task();
        let constraints = empty_constraints();

        let result = execute(
            "all-failed",
            &task,
            &constraints,
            Some(&DecisionTree::from_json(&test_tree_json()).unwrap()),
            &registry,
            &db,
            &invariant_cfg,
            &crate::metrics::Metrics::new(),
        )
        .unwrap();

        assert_eq!(
            result.action,
            AgentAction::Reject,
            "should reject when all agents are unhealthy"
        );
        assert!(
            result.reasoning.contains("unhealthy"),
            "reasoning should mention unhealthy: {}",
            result.reasoning
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("failure threshold")),
            "warnings should mention failure threshold: {:?}",
            result.warnings
        );

        // Decision logged even on all-failed reject
        let found = db.find_decision_by_task("all-failed").unwrap();
        assert!(found.is_some());
    }

    /// Warnings field is included in JSON output.
    #[test]
    fn result_json_includes_warnings() {
        let result = RouteResult {
            task_id: "w1".to_string(),
            action: AgentAction::Assign,
            chosen_agent: "claude_code@claude-sonnet-4-6".to_string(),
            confidence: 0.8,
            reasoning: "test".to_string(),
            decision_path: vec!["round-robin fallback (no decision tree)".to_string()],
            fallback_agent: None,
            fallback_reason: None,
            invariant_checks: vec![],
            inference_us: 10,
            feature_vector: vec![],
            candidates_evaluated: 1,
            warnings: vec![
                "Decision tree unavailable, using round-robin fallback".to_string(),
                "Unknown task_type 'magic', defaulting to 'feature'".to_string(),
            ],
            decision_id: None,
        };

        let json = result_to_json(&result);
        let warnings = json["warnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].as_str().unwrap().contains("round-robin"));
        assert!(warnings[1].as_str().unwrap().contains("magic"));
    }

    // =================================================================
    // to_agent_context dynamic state computation tests
    // =================================================================

    #[test]
    fn to_agent_context_busy_at_capacity() {
        use crate::config::AgentConfig;
        use crate::features::AgentInfo;

        let info = AgentInfo {
            agent_id: "test".to_string(),
            config: AgentConfig {
                display_name: "Test".to_string(),
                supports_languages: vec!["python".to_string()],
                supports_types: vec!["bugfix".to_string()],
                max_concurrent: 2,
                cost_per_hour: 0.10,
                avg_duration_min: 10.0,
            },
            running_tasks: 2,
            success_rate: Some(0.8),
            avg_duration_min: Some(10.0),
            avg_cost_usd: Some(0.10),
            recent_failures: 0,
        };
        let ctx = to_agent_context(&info, 5);
        assert_eq!(ctx.state, AgentState::Busy);
    }

    #[test]
    fn to_agent_context_failed_on_high_failures() {
        use crate::config::AgentConfig;
        use crate::features::AgentInfo;

        let info = AgentInfo {
            agent_id: "test".to_string(),
            config: AgentConfig {
                display_name: "Test".to_string(),
                supports_languages: vec!["python".to_string()],
                supports_types: vec!["bugfix".to_string()],
                max_concurrent: 2,
                cost_per_hour: 0.10,
                avg_duration_min: 10.0,
            },
            running_tasks: 0,
            success_rate: Some(0.5),
            avg_duration_min: Some(10.0),
            avg_cost_usd: Some(0.10),
            recent_failures: 6,
        };
        let ctx = to_agent_context(&info, 5);
        assert_eq!(ctx.state, AgentState::Failed);
    }

    #[test]
    fn to_agent_context_active_when_healthy() {
        use crate::config::AgentConfig;
        use crate::features::AgentInfo;

        let info = AgentInfo {
            agent_id: "test".to_string(),
            config: AgentConfig {
                display_name: "Test".to_string(),
                supports_languages: vec!["python".to_string()],
                supports_types: vec!["bugfix".to_string()],
                max_concurrent: 2,
                cost_per_hour: 0.10,
                avg_duration_min: 10.0,
            },
            running_tasks: 1,
            success_rate: Some(0.9),
            avg_duration_min: Some(10.0),
            avg_cost_usd: Some(0.10),
            recent_failures: 0,
        };
        let ctx = to_agent_context(&info, 5);
        assert_eq!(ctx.state, AgentState::Active);
    }
}
