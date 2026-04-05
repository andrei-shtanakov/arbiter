//! Policy engine: multi-agent evaluation using the Decision Tree.

use crate::policy::decision_tree::DecisionTree;
use crate::types::PredictionResult;
use tracing::warn;

/// Evaluate multiple agents against the decision tree and return
/// results ranked by confidence (highest first).
///
/// Each entry in `feature_vectors` is an `(agent_id, features)` pair.
/// The returned vector is sorted by descending confidence score.
///
/// If `feature_vectors` is empty, returns an empty vector (the caller
/// should treat this as a reject — no candidates available).
///
/// Failed predictions (e.g. NaN features) are silently filtered out
/// with a warning log.
pub fn evaluate_for_agents(
    tree: &DecisionTree,
    feature_vectors: &[(String, [f64; 22])],
) -> Vec<(String, PredictionResult)> {
    let mut results: Vec<(String, PredictionResult)> = feature_vectors
        .iter()
        .filter_map(|(agent_id, features)| {
            match tree.predict(features) {
                Ok(prediction) => Some((agent_id.clone(), prediction)),
                Err(e) => {
                    warn!(
                        agent = %agent_id,
                        error = %e,
                        "prediction failed, skipping agent"
                    );
                    None
                }
            }
        })
        .collect();

    // Sort by confidence descending (stable sort preserves order for equal confidence)
    results.sort_by(|a, b| {
        b.1.confidence
            .partial_cmp(&a.1.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a 3-class tree (claude_code=0, codex_cli=1, aider=2) that
    /// routes based on complexity (feature[2]) and task_type (feature[0]).
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
                // node 0: split on complexity (idx=2)
                {"feature": 2, "threshold": 2.5, "left": 1, "right": 2,
                 "value": [10.0, 10.0, 10.0]},
                // node 1 (low complexity): aider
                {"feature": -1, "threshold": 0.0, "left": -1, "right": -1,
                 "value": [1.0, 2.0, 7.0]},
                // node 2 (high complexity): claude_code
                {"feature": -1, "threshold": 0.0, "left": -1, "right": -1,
                 "value": [8.0, 1.0, 1.0]}
            ]
        })
        .to_string()
    }

    fn make_features(complexity: f64) -> [f64; 22] {
        let mut f = [0.0f64; 22];
        f[2] = complexity; // complexity
        f[9] = 0.8; // success_rate
        f[10] = 2.0; // available_slots
        f[15] = 1.0; // supports_task_type
        f[16] = 1.0; // supports_language
        f[19] = 10.0; // budget_remaining
        f
    }

    #[test]
    fn evaluate_ranks_by_confidence() {
        let tree = DecisionTree::from_json(&test_tree_json()).unwrap();

        // Two agents: one with low-complexity features, one with high
        let vectors = vec![
            ("claude_code".to_string(), make_features(3.0)), // complex
            ("aider".to_string(), make_features(1.0)),       // simple
        ];

        let results = evaluate_for_agents(&tree, &vectors);
        assert_eq!(results.len(), 2);
        // Both should have confidence 0.8 (8/10 or 7/10)
        // claude_code: complexity 3.0 > 2.5 -> right -> class 0 (8/10=0.8)
        // aider: complexity 1.0 <= 2.5 -> left -> class 2 (7/10=0.7)
        assert_eq!(results[0].0, "claude_code");
        assert!(results[0].1.confidence >= results[1].1.confidence);
    }

    #[test]
    fn evaluate_empty_candidates_returns_empty() {
        let tree = DecisionTree::from_json(&test_tree_json()).unwrap();
        let results = evaluate_for_agents(&tree, &[]);
        assert!(results.is_empty());
    }

    #[test]
    fn evaluate_single_candidate() {
        let tree = DecisionTree::from_json(&test_tree_json()).unwrap();
        let vectors = vec![("codex_cli".to_string(), make_features(1.0))];
        let results = evaluate_for_agents(&tree, &vectors);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "codex_cli");
    }

    #[test]
    fn evaluate_filters_out_failed_predictions() {
        let tree = DecisionTree::from_json(&test_tree_json()).unwrap();

        // One valid, one with wrong-length features (will fail predict)
        // We can't easily pass wrong-length since it's [f64; 22], so use NaN instead
        let mut nan_features = make_features(3.0);
        nan_features[0] = f64::NAN;

        let vectors = vec![
            ("good_agent".to_string(), make_features(3.0)),
            ("bad_agent".to_string(), nan_features),
        ];

        let results = evaluate_for_agents(&tree, &vectors);
        assert_eq!(results.len(), 1, "bad agent should be filtered out");
        assert_eq!(results[0].0, "good_agent");
    }

    #[test]
    fn evaluate_deterministic() {
        let tree = DecisionTree::from_json(&test_tree_json()).unwrap();
        let vectors = vec![
            ("claude_code".to_string(), make_features(3.0)),
            ("codex_cli".to_string(), make_features(2.0)),
            ("aider".to_string(), make_features(1.0)),
        ];

        let r1 = evaluate_for_agents(&tree, &vectors);
        let r2 = evaluate_for_agents(&tree, &vectors);

        for (a, b) in r1.iter().zip(r2.iter()) {
            assert_eq!(a.0, b.0);
            assert_eq!(a.1.class, b.1.class);
            assert_eq!(a.1.confidence, b.1.confidence);
        }
    }
}
