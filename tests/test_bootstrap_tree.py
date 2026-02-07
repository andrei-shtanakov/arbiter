"""Tests for the bootstrap decision tree generator and generated model."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import numpy as np

TREE_PATH = Path("models/agent_policy_tree.json")


def test_bootstrap_script_runs() -> None:
    """Verify that bootstrap_agent_tree.py runs without errors."""
    result = subprocess.run(
        [
            "uv",
            "run",
            "python",
            "scripts/bootstrap_agent_tree.py",
            "--output",
            "models/agent_policy_tree.json",
        ],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert result.returncode == 0, f"bootstrap script failed:\n{result.stderr}"


def test_tree_json_exists() -> None:
    """Verify the generated tree JSON file exists."""
    assert TREE_PATH.exists(), (
        f"{TREE_PATH} not found — run bootstrap_agent_tree.py first"
    )


def test_tree_json_valid_format() -> None:
    """Verify the tree JSON has the expected Arbiter format."""
    data = json.loads(TREE_PATH.read_text())

    assert "n_features" in data
    assert "n_classes" in data
    assert "class_names" in data
    assert "nodes" in data
    assert "feature_names" in data

    assert data["n_features"] == 22
    assert data["n_classes"] == 3
    assert data["class_names"] == [
        "claude_code",
        "codex_cli",
        "aider",
    ]
    assert len(data["feature_names"]) == 22


def test_tree_depth_constraint() -> None:
    """Verify tree depth <= 7."""
    data = json.loads(TREE_PATH.read_text())
    nodes = data["nodes"]

    def depth(idx: int) -> int:
        node = nodes[idx]
        if node["feature"] < 0:
            return 0
        return 1 + max(depth(node["left"]), depth(node["right"]))

    d = depth(0)
    assert d <= 7, f"tree depth {d} exceeds max 7"


def test_tree_node_count_constraint() -> None:
    """Verify node count <= 127."""
    data = json.loads(TREE_PATH.read_text())
    n = len(data["nodes"])
    assert n <= 127, f"node count {n} exceeds max 127"


def test_tree_node_structure() -> None:
    """Verify each node has the required fields."""
    data = json.loads(TREE_PATH.read_text())
    for i, node in enumerate(data["nodes"]):
        assert "feature" in node, f"node {i} missing 'feature'"
        assert "threshold" in node, f"node {i} missing 'threshold'"
        assert "left" in node, f"node {i} missing 'left'"
        assert "right" in node, f"node {i} missing 'right'"
        assert "value" in node, f"node {i} missing 'value'"
        assert len(node["value"]) == data["n_classes"], (
            f"node {i} value length mismatch"
        )


def test_bootstrap_accuracy_above_95() -> None:
    """Verify training accuracy is above 95%."""
    from scripts.bootstrap_agent_tree import (
        generate_expert_examples,
        inject_noise,
    )
    from sklearn.metrics import accuracy_score
    from sklearn.tree import DecisionTreeClassifier

    rng = np.random.default_rng(42)
    X_raw, y_raw = generate_expert_examples(rng)
    X, y = inject_noise(X_raw, y_raw, rng, noise_scale=0.05)

    clf = DecisionTreeClassifier(max_depth=7, min_samples_leaf=5, random_state=42)
    clf.fit(X, y)
    y_pred = clf.predict(X)
    accuracy = accuracy_score(y, y_pred)

    assert accuracy > 0.95, f"Training accuracy {accuracy:.4f} below 95% threshold"


def test_bootstrap_generates_enough_examples() -> None:
    """Verify at least ~500 training examples are generated."""
    from scripts.bootstrap_agent_tree import (
        generate_expert_examples,
    )

    rng = np.random.default_rng(42)
    X, y = generate_expert_examples(rng)
    assert len(X) >= 400, f"Only {len(X)} examples generated, expected ~500"


def test_bootstrap_deterministic() -> None:
    """Verify bootstrap tree generation is deterministic."""
    from scripts.bootstrap_agent_tree import (
        generate_expert_examples,
        inject_noise,
    )
    from sklearn.tree import DecisionTreeClassifier

    rng1 = np.random.default_rng(42)
    X1_raw, y1_raw = generate_expert_examples(rng1)
    X1, y1 = inject_noise(X1_raw, y1_raw, rng1, noise_scale=0.05)

    rng2 = np.random.default_rng(42)
    X2_raw, y2_raw = generate_expert_examples(rng2)
    X2, y2 = inject_noise(X2_raw, y2_raw, rng2, noise_scale=0.05)

    np.testing.assert_array_equal(X1, X2)
    np.testing.assert_array_equal(y1, y2)

    clf1 = DecisionTreeClassifier(max_depth=7, min_samples_leaf=5, random_state=42)
    clf2 = DecisionTreeClassifier(max_depth=7, min_samples_leaf=5, random_state=42)
    clf1.fit(X1, y1)
    clf2.fit(X2, y2)

    pred1 = clf1.predict(X1)
    pred2 = clf2.predict(X2)
    np.testing.assert_array_equal(pred1, pred2)
