//! Decision Tree inference engine.
//!
//! Loads a sklearn-exported Decision Tree from JSON and performs
//! deterministic inference, producing a class prediction with confidence
//! and an auditable decision path.

use crate::types::PredictionResult;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// A single node in the decision tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TreeNode {
    /// Index of the feature to split on (-1 for leaf nodes).
    feature: i32,
    /// Threshold value for the split.
    threshold: f64,
    /// Index of the left child (-1 for none).
    left: i32,
    /// Index of the right child (-1 for none).
    right: i32,
    /// Class distribution at this node (samples per class).
    value: Vec<f64>,
}

/// Sklearn-exported Decision Tree JSON format.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TreeJson {
    /// Number of features the tree was trained on.
    n_features: usize,
    /// Number of output classes.
    n_classes: usize,
    /// Class label names.
    class_names: Vec<String>,
    /// Feature name labels.
    #[serde(default)]
    feature_names: Vec<String>,
    /// Flat array of tree nodes.
    nodes: Vec<TreeNode>,
}

/// A trained Decision Tree ready for inference.
#[derive(Debug, Clone)]
pub struct DecisionTree {
    n_features: usize,
    n_classes: usize,
    class_names: Vec<String>,
    feature_names: Vec<String>,
    nodes: Vec<TreeNode>,
}

impl DecisionTree {
    /// Load a Decision Tree from sklearn-exported JSON.
    ///
    /// Expected JSON format:
    /// ```json
    /// {
    ///   "n_features": 22,
    ///   "n_classes": 3,
    ///   "class_names": ["claude_code", "codex_cli", "aider"],
    ///   "feature_names": ["task_type", "language", ...],
    ///   "nodes": [
    ///     {"feature": 2, "threshold": 2.5, "left": 1, "right": 2, "value": [100, 50, 30]},
    ///     ...
    ///   ]
    /// }
    /// ```
    pub fn from_json(json: &str) -> Result<Self> {
        let tree: TreeJson =
            serde_json::from_str(json).context("failed to parse decision tree JSON")?;

        if tree.nodes.is_empty() {
            bail!("decision tree has no nodes");
        }
        if tree.n_classes == 0 {
            bail!("decision tree has zero classes");
        }
        if tree.n_features == 0 {
            bail!("decision tree has zero features");
        }
        if tree.class_names.len() != tree.n_classes {
            bail!(
                "class_names length {} does not match n_classes {}",
                tree.class_names.len(),
                tree.n_classes
            );
        }

        // Validate node structure
        let n = tree.nodes.len() as i32;
        for (i, node) in tree.nodes.iter().enumerate() {
            let is_leaf = node.feature < 0;
            if !is_leaf {
                if node.left < 0 || node.left >= n {
                    bail!("node {i}: invalid left child index {}", node.left);
                }
                if node.right < 0 || node.right >= n {
                    bail!("node {i}: invalid right child index {}", node.right);
                }
                if node.feature as usize >= tree.n_features {
                    bail!(
                        "node {i}: feature index {} >= n_features {}",
                        node.feature,
                        tree.n_features
                    );
                }
            }
            if node.value.len() != tree.n_classes {
                bail!(
                    "node {i}: value length {} does not match n_classes {}",
                    node.value.len(),
                    tree.n_classes
                );
            }
        }

        Ok(Self {
            n_features: tree.n_features,
            n_classes: tree.n_classes,
            class_names: tree.class_names,
            feature_names: tree.feature_names,
            nodes: tree.nodes,
        })
    }

    /// Run inference on a feature vector.
    ///
    /// Returns a `PredictionResult` with the predicted class index,
    /// confidence score, and the decision path through the tree.
    ///
    /// # Errors
    /// Returns an error if the feature vector length does not match
    /// `n_features`, or if any feature value is non-finite (NaN or infinity).
    pub fn predict(&self, features: &[f64]) -> Result<PredictionResult> {
        if features.len() != self.n_features {
            bail!(
                "feature vector length {} does not match tree n_features {}",
                features.len(),
                self.n_features
            );
        }

        if let Some(idx) = features.iter().position(|f| !f.is_finite()) {
            bail!("feature vector contains non-finite value at index {idx}");
        }

        let mut node_idx: usize = 0;
        let mut path = Vec::new();

        loop {
            let node = &self.nodes[node_idx];
            let is_leaf = node.feature < 0;

            if is_leaf {
                // Leaf node: compute prediction
                let total: f64 = node.value.iter().sum();
                let (class, &max_val) = node
                    .value
                    .iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| {
                        a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .context("empty value vector in leaf node")?;

                let confidence = if total > 0.0 { max_val / total } else { 0.0 };

                let class_name = self
                    .class_names
                    .get(class)
                    .cloned()
                    .unwrap_or_else(|| format!("class_{class}"));
                path.push(format!("leaf: {class_name} (confidence={confidence:.3})"));

                return Ok(PredictionResult {
                    class,
                    confidence,
                    path,
                });
            }

            // Internal node: traverse left or right
            let feat_idx = node.feature as usize;
            let feat_val = features[feat_idx];
            let feat_name = self
                .feature_names
                .get(feat_idx)
                .cloned()
                .unwrap_or_else(|| format!("feature[{feat_idx}]"));

            if feat_val <= node.threshold {
                path.push(format!(
                    "node {node_idx}: {feat_name} ({feat_val:.2}) <= {:.2} -> left",
                    node.threshold
                ));
                node_idx = node.left as usize;
            } else {
                path.push(format!(
                    "node {node_idx}: {feat_name} ({feat_val:.2}) > {:.2} -> right",
                    node.threshold
                ));
                node_idx = node.right as usize;
            }
        }
    }

    /// Number of nodes in the tree.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Maximum depth of the tree.
    pub fn depth(&self) -> usize {
        self.compute_depth(0)
    }

    /// Number of classes the tree can predict.
    pub fn n_classes(&self) -> usize {
        self.n_classes
    }

    /// Number of features expected by the tree.
    pub fn n_features(&self) -> usize {
        self.n_features
    }

    /// Class name labels.
    pub fn class_names(&self) -> &[String] {
        &self.class_names
    }

    fn compute_depth(&self, node_idx: usize) -> usize {
        let node = &self.nodes[node_idx];
        if node.feature < 0 {
            return 0;
        }
        let left_depth = self.compute_depth(node.left as usize);
        let right_depth = self.compute_depth(node.right as usize);
        1 + left_depth.max(right_depth)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid tree JSON for testing: single-split tree with 2 classes.
    fn minimal_tree_json() -> String {
        serde_json::json!({
            "n_features": 3,
            "n_classes": 2,
            "class_names": ["cat", "dog"],
            "feature_names": ["size", "weight", "color"],
            "nodes": [
                {"feature": 0, "threshold": 5.0, "left": 1, "right": 2, "value": [10.0, 10.0]},
                {"feature": -1, "threshold": 0.0, "left": -1, "right": -1, "value": [8.0, 2.0]},
                {"feature": -1, "threshold": 0.0, "left": -1, "right": -1, "value": [2.0, 8.0]}
            ]
        })
        .to_string()
    }

    #[test]
    fn from_json_valid() {
        let tree = DecisionTree::from_json(&minimal_tree_json()).unwrap();
        assert_eq!(tree.n_features(), 3);
        assert_eq!(tree.n_classes(), 2);
        assert_eq!(tree.node_count(), 3);
        assert_eq!(tree.depth(), 1);
        assert_eq!(tree.class_names(), &["cat", "dog"]);
    }

    #[test]
    fn from_json_empty_nodes() {
        let json = r#"{"n_features":3,"n_classes":2,"class_names":["a","b"],"nodes":[]}"#;
        let err = DecisionTree::from_json(json).unwrap_err();
        assert!(err.to_string().contains("no nodes"));
    }

    #[test]
    fn from_json_class_count_mismatch() {
        let json = serde_json::json!({
            "n_features": 1,
            "n_classes": 3,
            "class_names": ["a", "b"],
            "nodes": [{"feature": -1, "threshold": 0.0, "left": -1, "right": -1, "value": [1.0, 1.0, 1.0]}]
        })
        .to_string();
        let err = DecisionTree::from_json(&json).unwrap_err();
        assert!(err.to_string().contains("class_names length"));
    }

    #[test]
    fn predict_left_branch() {
        let tree = DecisionTree::from_json(&minimal_tree_json()).unwrap();
        // feature[0] = 3.0 <= 5.0, so go left -> class "cat"
        let result = tree.predict(&[3.0, 0.0, 0.0]).unwrap();
        assert_eq!(result.class, 0);
        assert_eq!(result.confidence, 0.8);
        assert!(result.path.len() == 2);
        assert!(result.path[0].contains("left"));
        assert!(result.path[1].contains("cat"));
    }

    #[test]
    fn predict_right_branch() {
        let tree = DecisionTree::from_json(&minimal_tree_json()).unwrap();
        // feature[0] = 7.0 > 5.0, so go right -> class "dog"
        let result = tree.predict(&[7.0, 0.0, 0.0]).unwrap();
        assert_eq!(result.class, 1);
        assert_eq!(result.confidence, 0.8);
        assert!(result.path[0].contains("right"));
        assert!(result.path[1].contains("dog"));
    }

    #[test]
    fn predict_deterministic() {
        let tree = DecisionTree::from_json(&minimal_tree_json()).unwrap();
        let features = vec![3.0, 1.0, 2.0];

        let r1 = tree.predict(&features).unwrap();
        let r2 = tree.predict(&features).unwrap();
        let r3 = tree.predict(&features).unwrap();

        assert_eq!(r1.class, r2.class);
        assert_eq!(r2.class, r3.class);
        assert_eq!(r1.confidence, r2.confidence);
        assert_eq!(r2.confidence, r3.confidence);
        assert_eq!(r1.path, r2.path);
        assert_eq!(r2.path, r3.path);
    }

    #[test]
    fn predict_threshold_boundary() {
        let tree = DecisionTree::from_json(&minimal_tree_json()).unwrap();
        // feature[0] = 5.0 <= 5.0, so go left
        let result = tree.predict(&[5.0, 0.0, 0.0]).unwrap();
        assert_eq!(result.class, 0); // "cat"
    }

    #[test]
    fn depth_calculation() {
        // Deeper tree: depth=2
        let json = serde_json::json!({
            "n_features": 2,
            "n_classes": 2,
            "class_names": ["a", "b"],
            "feature_names": ["x", "y"],
            "nodes": [
                {"feature": 0, "threshold": 5.0, "left": 1, "right": 4, "value": [10.0, 10.0]},
                {"feature": 1, "threshold": 3.0, "left": 2, "right": 3, "value": [6.0, 4.0]},
                {"feature": -1, "threshold": 0.0, "left": -1, "right": -1, "value": [5.0, 1.0]},
                {"feature": -1, "threshold": 0.0, "left": -1, "right": -1, "value": [1.0, 3.0]},
                {"feature": -1, "threshold": 0.0, "left": -1, "right": -1, "value": [2.0, 8.0]}
            ]
        })
        .to_string();
        let tree = DecisionTree::from_json(&json).unwrap();
        assert_eq!(tree.depth(), 2);
        assert_eq!(tree.node_count(), 5);
    }

    #[test]
    fn predict_wrong_feature_count_returns_error() {
        let tree = DecisionTree::from_json(&minimal_tree_json()).unwrap();
        let err = tree.predict(&[1.0]).unwrap_err();
        assert!(err.to_string().contains("feature vector length"));
    }

    #[test]
    fn predict_nan_feature_returns_error() {
        let tree = DecisionTree::from_json(&minimal_tree_json()).unwrap();
        let err = tree.predict(&[1.0, f64::NAN, 0.0]).unwrap_err();
        assert!(err.to_string().contains("non-finite"));
    }

    #[test]
    fn predict_infinity_feature_returns_error() {
        let tree = DecisionTree::from_json(&minimal_tree_json()).unwrap();
        let err = tree.predict(&[f64::INFINITY, 0.0, 0.0]).unwrap_err();
        assert!(err.to_string().contains("non-finite"));
    }

    #[test]
    fn from_json_invalid_child_index() {
        let json = serde_json::json!({
            "n_features": 1,
            "n_classes": 2,
            "class_names": ["a", "b"],
            "nodes": [
                {"feature": 0, "threshold": 5.0, "left": 1, "right": 99, "value": [1.0, 1.0]},
                {"feature": -1, "threshold": 0.0, "left": -1, "right": -1, "value": [1.0, 0.0]}
            ]
        })
        .to_string();
        let err = DecisionTree::from_json(&json).unwrap_err();
        assert!(err.to_string().contains("invalid right child"));
    }

    #[test]
    fn from_json_without_feature_names() {
        let json = serde_json::json!({
            "n_features": 2,
            "n_classes": 2,
            "class_names": ["a", "b"],
            "nodes": [
                {"feature": -1, "threshold": 0.0, "left": -1, "right": -1, "value": [5.0, 3.0]}
            ]
        })
        .to_string();
        let tree = DecisionTree::from_json(&json).unwrap();
        assert!(tree.feature_names.is_empty());
        // predict still works, just uses fallback names
        let result = tree.predict(&[1.0, 2.0]).unwrap();
        assert_eq!(result.class, 0);
    }

    // -- Bootstrap tree tests (UT-21) --

    fn load_bootstrap_tree() -> DecisionTree {
        // CARGO_MANIFEST_DIR points to the crate root (arbiter-core/),
        // so we go up one level to the workspace root.
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

    #[test]
    fn bootstrap_tree_loads() {
        let tree = load_bootstrap_tree();
        assert_eq!(tree.n_features(), 22);
        assert_eq!(tree.n_classes(), 3);
        assert_eq!(tree.class_names(), &["claude_code", "codex_cli", "aider"]);
    }

    #[test]
    fn bootstrap_tree_depth_constraint() {
        let tree = load_bootstrap_tree();
        assert!(
            tree.depth() <= 7,
            "tree depth {} exceeds max 7",
            tree.depth()
        );
    }

    #[test]
    fn bootstrap_tree_node_count_constraint() {
        let tree = load_bootstrap_tree();
        assert!(
            tree.node_count() <= 127,
            "node count {} exceeds max 127",
            tree.node_count()
        );
    }

    #[test]
    fn bootstrap_tree_deterministic() {
        let tree = load_bootstrap_tree();
        // Complex Rust feature -> should route consistently
        let features = [
            0.0,   // task_type: feature
            1.0,   // language: rust
            3.0,   // complexity: complex
            2.0,   // priority: high
            5.0,   // scope_size
            100.0, // estimated_tokens
            0.0,   // has_dependencies
            0.0,   // requires_internet
            120.0, // sla_minutes
            0.85,  // agent_success_rate
            2.0,   // agent_available_slots
            0.0,   // agent_running_tasks
            18.0,  // agent_avg_duration_min
            0.30,  // agent_avg_cost_usd
            0.0,   // agent_recent_failures
            1.0,   // agent_supports_task_type
            1.0,   // agent_supports_language
            1.0,   // total_running_tasks
            2.0,   // total_pending_tasks
            8.0,   // budget_remaining_usd
            14.0,  // time_of_day_hour
            0.0,   // concurrent_scope_conflicts
        ];

        let r1 = tree.predict(&features).unwrap();
        let r2 = tree.predict(&features).unwrap();
        let r3 = tree.predict(&features).unwrap();

        assert_eq!(r1.class, r2.class);
        assert_eq!(r2.class, r3.class);
        assert_eq!(r1.confidence, r2.confidence);
        assert_eq!(r1.path, r2.path);
    }

    #[test]
    fn bootstrap_tree_has_22_features() {
        let tree = load_bootstrap_tree();
        assert_eq!(tree.n_features(), 22);
        assert_eq!(tree.feature_names.len(), 22);
    }
}
