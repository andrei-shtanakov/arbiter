"""Bootstrap decision tree generator for Arbiter.

Generates a decision tree from expert routing rules by:
1. Defining 10 expert rules for agent selection
2. Expanding them into ~500 training examples with variations
3. Adding noise for robustness
4. Training a DecisionTreeClassifier(max_depth=7)
5. Exporting to Arbiter JSON format

Usage:
    uv run python scripts/bootstrap_agent_tree.py
    uv run python scripts/bootstrap_agent_tree.py --output models/custom.json
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import sys
from pathlib import Path

import numpy as np
from sklearn.metrics import accuracy_score, confusion_matrix
from sklearn.model_selection import cross_val_score
from sklearn.tree import DecisionTreeClassifier

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

FEATURE_NAMES: list[str] = [
    "task_type",  # 0: ordinal [0,6]
    "language",  # 1: ordinal [0,5]
    "complexity",  # 2: ordinal [0,4]
    "priority",  # 3: ordinal [0,3]
    "scope_size",  # 4: [0,100]
    "estimated_tokens",  # 5: [0,200] (tokens/1000)
    "has_dependencies",  # 6: {0,1}
    "requires_internet",  # 7: {0,1}
    "sla_minutes",  # 8: [0,480]
    "agent_success_rate",  # 9: [0,1]
    "agent_available_slots",  # 10: [0,10]
    "agent_running_tasks",  # 11: [0,10]
    "agent_avg_duration_min",  # 12: [0,480]
    "agent_avg_cost_usd",  # 13: [0,100]
    "agent_recent_failures",  # 14: [0,50]
    "agent_supports_task_type",  # 15: {0,1}
    "agent_supports_language",  # 16: {0,1}
    "total_running_tasks",  # 17: [0,20]
    "total_pending_tasks",  # 18: [0,100]
    "budget_remaining_usd",  # 19: [0,1000]
    "time_of_day_hour",  # 20: [0,23]
    "concurrent_scope_conflicts",  # 21: [0,10]
]

# Agent classes
AGENTS: list[str] = ["claude_code", "codex_cli", "aider"]
AGENT_IDX: dict[str, int] = {name: i for i, name in enumerate(AGENTS)}

# Task type ordinals
TASK_FEATURE = 0
TASK_BUGFIX = 1
TASK_REFACTOR = 2
TASK_TEST = 3
TASK_DOCS = 4
TASK_REVIEW = 5
TASK_RESEARCH = 6

# Complexity ordinals
COMP_TRIVIAL = 0
COMP_SIMPLE = 1
COMP_MODERATE = 2
COMP_COMPLEX = 3
COMP_CRITICAL = 4

# Language ordinals
LANG_PYTHON = 0
LANG_RUST = 1
LANG_TYPESCRIPT = 2
LANG_GO = 3
LANG_MIXED = 4
LANG_OTHER = 5

# Priority ordinals
PRI_LOW = 0
PRI_NORMAL = 1
PRI_HIGH = 2
PRI_URGENT = 3


# ---------------------------------------------------------------------------
# Expert rules
# ---------------------------------------------------------------------------


def make_base_features(
    task_type: float = 0.0,
    language: float = 0.0,
    complexity: float = 2.0,
    priority: float = 1.0,
    scope_size: float = 5.0,
    estimated_tokens: float = 50.0,
    has_dependencies: float = 0.0,
    requires_internet: float = 0.0,
    sla_minutes: float = 120.0,
    success_rate: float = 0.85,
    available_slots: float = 2.0,
    running_tasks: float = 0.0,
    avg_duration: float = 15.0,
    avg_cost: float = 0.15,
    recent_failures: float = 0.0,
    supports_type: float = 1.0,
    supports_lang: float = 1.0,
    total_running: float = 1.0,
    total_pending: float = 2.0,
    budget_remaining: float = 8.0,
    time_of_day: float = 14.0,
    scope_conflicts: float = 0.0,
) -> list[float]:
    """Create a 22-dim feature vector from keyword arguments."""
    return [
        task_type,
        language,
        complexity,
        priority,
        scope_size,
        estimated_tokens,
        has_dependencies,
        requires_internet,
        sla_minutes,
        success_rate,
        available_slots,
        running_tasks,
        avg_duration,
        avg_cost,
        recent_failures,
        supports_type,
        supports_lang,
        total_running,
        total_pending,
        budget_remaining,
        time_of_day,
        scope_conflicts,
    ]


def generate_expert_examples(
    rng: np.random.Generator,
) -> tuple[list[list[float]], list[int]]:
    """Generate training examples from 10 expert routing rules.

    Returns (X, y) where X is a list of 22-dim feature vectors
    and y is a list of agent class indices.
    """
    examples: list[list[float]] = []
    labels: list[int] = []

    def add(features: list[float], agent: str, count: int = 1) -> None:
        for _ in range(count):
            examples.append(features[:])
            labels.append(AGENT_IDX[agent])

    # Rule 1: complex/critical + Rust -> claude_code
    for complexity in [COMP_COMPLEX, COMP_CRITICAL]:
        for priority in [PRI_LOW, PRI_NORMAL, PRI_HIGH, PRI_URGENT]:
            for task in [TASK_FEATURE, TASK_BUGFIX, TASK_REFACTOR]:
                for tokens in [80.0, 120.0]:
                    add(
                        make_base_features(
                            task_type=task,
                            language=LANG_RUST,
                            complexity=complexity,
                            priority=priority,
                            estimated_tokens=tokens,
                        ),
                        "claude_code",
                    )

    # Rule 2: complex/critical + Python -> claude_code
    for complexity in [COMP_COMPLEX, COMP_CRITICAL]:
        for priority in [PRI_NORMAL, PRI_HIGH, PRI_URGENT]:
            for task in [TASK_FEATURE, TASK_BUGFIX, TASK_REFACTOR]:
                for tokens in [60.0, 100.0]:
                    add(
                        make_base_features(
                            task_type=task,
                            language=LANG_PYTHON,
                            complexity=complexity,
                            priority=priority,
                            estimated_tokens=tokens,
                        ),
                        "claude_code",
                    )

    # Rule 3: docs/review/research -> claude_code
    for task in [TASK_DOCS, TASK_REVIEW, TASK_RESEARCH]:
        for lang in [
            LANG_PYTHON,
            LANG_RUST,
            LANG_TYPESCRIPT,
            LANG_GO,
            LANG_MIXED,
            LANG_OTHER,
        ]:
            for complexity in range(5):
                add(
                    make_base_features(
                        task_type=task,
                        language=lang,
                        complexity=complexity,
                        requires_internet=1.0,
                    ),
                    "claude_code",
                )

    # Rule 4: trivial/simple + bugfix -> aider
    for complexity in [COMP_TRIVIAL, COMP_SIMPLE]:
        for lang in [LANG_PYTHON, LANG_RUST, LANG_TYPESCRIPT]:
            for priority in [PRI_LOW, PRI_NORMAL, PRI_HIGH]:
                for tokens in [10.0, 25.0, 40.0]:
                    add(
                        make_base_features(
                            task_type=TASK_BUGFIX,
                            language=lang,
                            complexity=complexity,
                            priority=priority,
                            estimated_tokens=tokens,
                            avg_duration=8.0,
                            avg_cost=0.05,
                        ),
                        "aider",
                    )

    # Rule 5: trivial/simple + refactor -> aider
    for complexity in [COMP_TRIVIAL, COMP_SIMPLE]:
        for lang in [LANG_PYTHON, LANG_TYPESCRIPT, LANG_RUST]:
            for priority in [PRI_LOW, PRI_NORMAL, PRI_HIGH]:
                for tokens in [10.0, 25.0]:
                    add(
                        make_base_features(
                            task_type=TASK_REFACTOR,
                            language=lang,
                            complexity=complexity,
                            priority=priority,
                            estimated_tokens=tokens,
                            avg_duration=6.0,
                            avg_cost=0.04,
                        ),
                        "aider",
                    )

    # Rule 6: TypeScript + feature -> codex_cli
    for complexity in [
        COMP_TRIVIAL,
        COMP_SIMPLE,
        COMP_MODERATE,
    ]:
        for priority in [PRI_LOW, PRI_NORMAL, PRI_HIGH, PRI_URGENT]:
            for tokens in [40.0, 80.0]:
                add(
                    make_base_features(
                        task_type=TASK_FEATURE,
                        language=LANG_TYPESCRIPT,
                        complexity=complexity,
                        priority=priority,
                        estimated_tokens=tokens,
                        avg_duration=12.0,
                        avg_cost=0.12,
                    ),
                    "codex_cli",
                )

    # Rule 7: Go language -> codex_cli
    for task in [TASK_FEATURE, TASK_BUGFIX, TASK_REFACTOR, TASK_TEST]:
        for complexity in range(4):
            for priority in [PRI_LOW, PRI_NORMAL, PRI_HIGH]:
                add(
                    make_base_features(
                        task_type=task,
                        language=LANG_GO,
                        complexity=complexity,
                        priority=priority,
                        estimated_tokens=50.0,
                        avg_duration=10.0,
                        avg_cost=0.10,
                    ),
                    "codex_cli",
                )

    # Rule 8: moderate + Python -> codex_cli
    for task in [TASK_FEATURE, TASK_BUGFIX, TASK_REFACTOR]:
        for priority in [PRI_LOW, PRI_NORMAL, PRI_HIGH]:
            for scope in [3.0, 8.0, 15.0]:
                add(
                    make_base_features(
                        task_type=task,
                        language=LANG_PYTHON,
                        complexity=COMP_MODERATE,
                        priority=priority,
                        scope_size=scope,
                        estimated_tokens=40.0,
                        avg_duration=12.0,
                        avg_cost=0.10,
                    ),
                    "codex_cli",
                )

    # Rule 9: test + simple/moderate -> aider
    for complexity in [COMP_SIMPLE, COMP_MODERATE]:
        for lang in [LANG_PYTHON, LANG_TYPESCRIPT, LANG_RUST]:
            for priority in [PRI_LOW, PRI_NORMAL, PRI_HIGH]:
                for tokens in [15.0, 35.0]:
                    add(
                        make_base_features(
                            task_type=TASK_TEST,
                            language=lang,
                            complexity=complexity,
                            priority=priority,
                            estimated_tokens=tokens,
                            avg_duration=7.0,
                            avg_cost=0.05,
                        ),
                        "aider",
                    )

    # Rule 10: DEFAULT -> claude_code (catch-all)
    for task in [
        TASK_FEATURE,
        TASK_BUGFIX,
        TASK_REFACTOR,
        TASK_TEST,
    ]:
        for lang in [LANG_MIXED, LANG_OTHER]:
            for complexity in [
                COMP_TRIVIAL,
                COMP_SIMPLE,
                COMP_MODERATE,
                COMP_COMPLEX,
            ]:
                for priority in [PRI_LOW, PRI_NORMAL, PRI_HIGH]:
                    add(
                        make_base_features(
                            task_type=task,
                            language=lang,
                            complexity=complexity,
                            priority=priority,
                            estimated_tokens=70.0,
                        ),
                        "claude_code",
                    )

    return examples, labels


def inject_noise(
    X: list[list[float]],
    y: list[int],
    rng: np.random.Generator,
    noise_scale: float = 0.1,
) -> tuple[np.ndarray, np.ndarray]:
    """Add Gaussian noise to continuous features for robustness.

    Discrete/boolean features (indices 0-3, 6, 7, 15, 16) are not noised.
    """
    X_arr = np.array(X, dtype=np.float64)
    y_arr = np.array(y, dtype=np.int64)

    # Indices of continuous features to add noise to
    continuous = [4, 5, 8, 9, 10, 11, 12, 13, 14, 17, 18, 19, 20, 21]

    noise = rng.normal(0, noise_scale, (X_arr.shape[0], len(continuous)))

    for i, col in enumerate(continuous):
        X_arr[:, col] += noise[:, i] * np.abs(X_arr[:, col]).clip(min=1.0)

    # Clamp to valid ranges
    X_arr = np.clip(X_arr, 0.0, None)
    # Boolean features stay 0 or 1
    for idx in [6, 7, 15, 16]:
        X_arr[:, idx] = np.round(X_arr[:, idx]).clip(0, 1)

    return X_arr, y_arr


def export_tree_json(
    clf: DecisionTreeClassifier,
    class_names: list[str],
    feature_names: list[str],
) -> dict:
    """Export a trained sklearn DecisionTreeClassifier to Arbiter JSON format.

    Format:
    {
        "n_features": int,
        "n_classes": int,
        "class_names": [...],
        "feature_names": [...],
        "nodes": [
            {
                "feature": int (-1 for leaf),
                "threshold": float,
                "left": int (-1 for none),
                "right": int (-1 for none),
                "value": [float per class]
            },
            ...
        ]
    }
    """
    tree = clf.tree_

    nodes = []
    for i in range(tree.node_count):
        is_leaf = tree.children_left[i] == tree.children_right[i]

        # value shape: (n_nodes, n_classes, 1) for single-output
        class_values = tree.value[i].flatten().tolist()

        nodes.append(
            {
                "feature": int(tree.feature[i]) if not is_leaf else -1,
                "threshold": float(tree.threshold[i]) if not is_leaf else 0.0,
                "left": int(tree.children_left[i]) if not is_leaf else -1,
                "right": int(tree.children_right[i]) if not is_leaf else -1,
                "value": class_values,
            }
        )

    return {
        "n_features": int(tree.n_features),
        "n_classes": int(tree.n_classes[0]),
        "class_names": class_names,
        "feature_names": feature_names,
        "nodes": nodes,
    }


def extract_from_db(
    db_path: str,
) -> tuple[list[list[float]], list[int]]:
    """Extract training data from arbiter.db outcomes.

    Joins decisions with successful outcomes, parses feature vectors
    and maps agent names to class indices.

    Returns (X, y) lists of feature vectors and agent class indices.
    """
    X: list[list[float]] = []
    y: list[int] = []

    conn = sqlite3.connect(db_path)
    try:
        cursor = conn.execute(
            "SELECT d.feature_vector, d.chosen_agent "
            "FROM decisions d "
            "JOIN outcomes ON outcomes.decision_id = d.id "
            "WHERE outcomes.status = 'success'"
        )
        for row in cursor:
            try:
                features = json.loads(row[0])
                agent = row[1]
                if agent not in AGENT_IDX:
                    continue
                if len(features) != 22:
                    continue
                X.append([float(f) for f in features])
                y.append(AGENT_IDX[agent])
            except (json.JSONDecodeError, TypeError, ValueError):
                continue
    finally:
        conn.close()

    return X, y


def train_and_export(
    output_path: str = "models/agent_policy_tree.json",
    seed: int = 42,
    from_db: str | None = None,
) -> dict:
    """Train bootstrap tree from expert rules and export to JSON.

    Returns a dict with training stats.
    """
    rng = np.random.default_rng(seed)

    # Generate expert examples
    X_raw, y_raw = generate_expert_examples(rng)
    print(f"Generated {len(X_raw)} raw training examples")

    if from_db:
        print(f"\nExtracting training data from {from_db}")
        db_X, db_y = extract_from_db(from_db)
        print(f"  Extracted {len(db_X)} examples from database")
        if db_X:
            X_raw.extend(db_X)
            y_raw.extend(db_y)
            print(
                f"  Total: {len(X_raw)} examples"
                f" ({len(db_X)} DB"
                f" + {len(X_raw) - len(db_X)} expert)"
            )

    # Add noise
    X, y = inject_noise(X_raw, y_raw, rng, noise_scale=0.05)
    print(f"After noise injection: {len(X)} examples")

    # Cross-validation on noised data
    cv_clf = DecisionTreeClassifier(
        max_depth=7,
        min_samples_leaf=5,
        random_state=seed,
    )
    cv_scores = cross_val_score(cv_clf, X, y, cv=5, scoring="accuracy")
    cv_mean = cv_scores.mean()
    cv_std = cv_scores.std()
    print(f"\n5-fold CV accuracy: {cv_mean:.4f} (+/- {cv_std:.4f})")
    print(f"  Per-fold: {[f'{s:.4f}' for s in cv_scores]}")

    assert cv_mean > 0.90, (
        f"Cross-validation accuracy {cv_mean:.4f} below 90% threshold"
    )

    # Train decision tree
    clf = DecisionTreeClassifier(
        max_depth=7,
        min_samples_leaf=5,
        random_state=seed,
    )
    clf.fit(X, y)

    # Evaluate
    y_pred = clf.predict(X)
    accuracy = accuracy_score(y, y_pred)
    cm = confusion_matrix(y, y_pred)

    print(f"\nTraining accuracy: {accuracy:.4f}")
    print(f"Tree depth: {clf.get_depth()}")
    print(f"Tree node count: {clf.tree_.node_count}")
    print(f"Tree leaf count: {clf.get_n_leaves()}")

    print("\nConfusion matrix:")
    print(f"{'':>15} ", end="")
    for name in AGENTS:
        print(f"{name:>12}", end="")
    print()
    for i, name in enumerate(AGENTS):
        print(f"{name:>15} ", end="")
        for j in range(len(AGENTS)):
            print(f"{cm[i][j]:>12}", end="")
        print()

    # Feature importance
    importances = clf.feature_importances_
    sorted_idx = np.argsort(importances)[::-1]
    print("\nFeature importance (top 10):")
    for rank, idx in enumerate(sorted_idx[:10], 1):
        name = FEATURE_NAMES[idx] if idx < len(FEATURE_NAMES) else f"feature[{idx}]"
        print(f"  {rank:2d}. {name:30s} {importances[idx]:.4f}")
    zero_features = [
        FEATURE_NAMES[i] for i in range(len(FEATURE_NAMES)) if importances[i] == 0.0
    ]
    if zero_features:
        print(
            f"\n  Zero-importance features"
            f" ({len(zero_features)}):"
            f" {', '.join(zero_features)}"
        )

    # Validate constraints
    assert accuracy > 0.95, f"Accuracy {accuracy:.4f} below 95% threshold"
    assert clf.get_depth() <= 7, f"Tree depth {clf.get_depth()} exceeds max 7"
    assert clf.tree_.node_count <= 127, (
        f"Node count {clf.tree_.node_count} exceeds max 127"
    )

    # Export
    tree_json = export_tree_json(clf, AGENTS, FEATURE_NAMES)

    out = Path(output_path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(tree_json, indent=2) + "\n")
    print(f"\nTree exported to {output_path}")

    return {
        "accuracy": accuracy,
        "cv_mean": cv_mean,
        "cv_std": cv_std,
        "depth": clf.get_depth(),
        "node_count": clf.tree_.node_count,
        "n_examples": len(X),
        "output_path": output_path,
        "feature_importance": {
            FEATURE_NAMES[i]: float(importances[i])
            for i in sorted_idx
            if importances[i] > 0
        },
    }


def main() -> None:
    """CLI entry point."""
    parser = argparse.ArgumentParser(
        description="Generate bootstrap decision tree for Arbiter"
    )
    parser.add_argument(
        "--output",
        default="models/agent_policy_tree.json",
        help="Output path for the tree JSON",
    )
    parser.add_argument(
        "--from-db",
        default=None,
        help="Path to arbiter.db — extract training data from outcomes",
    )
    parser.add_argument(
        "--seed",
        type=int,
        default=42,
        help="Random seed for reproducibility",
    )
    args = parser.parse_args()

    stats = train_and_export(args.output, args.seed, args.from_db)

    if stats["accuracy"] < 0.95:
        print("\nWARNING: accuracy below 95%%!", file=sys.stderr)
        sys.exit(1)

    if stats["cv_mean"] < 0.90:
        print(
            f"\nWARNING: CV accuracy {stats['cv_mean']:.4f} below 90%!",
            file=sys.stderr,
        )
        sys.exit(1)

    print("\nDone.")


if __name__ == "__main__":
    main()
