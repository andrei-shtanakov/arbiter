# R6 — Retrain Pipeline + Eval

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the learning loop — Arbiter can retrain its decision tree from real outcome data, evaluate tree quality against baselines, and report feature importance.

**Architecture:** The bootstrap script gains `--from-db` to extract training data from SQLite outcomes+decisions. A new `scripts/eval_tree.py` evaluates DT vs round-robin vs always-claude on a synthetic benchmark suite. Feature importance is reported inline during training. Criterion replaces custom timing in arbiter-cli for statistically rigorous benchmarks.

**Tech Stack:** Python (scikit-learn, sqlite3, numpy), Rust (criterion for benchmarks)

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Modify | `scripts/bootstrap_agent_tree.py` | Add `--from-db`, feature importance report |
| Create | `scripts/eval_tree.py` | A/B evaluation: DT vs round-robin vs always-best |
| Create | `arbiter-cli/benches/routing.rs` | Criterion benchmarks replacing custom timing |
| Modify | `arbiter-cli/Cargo.toml` | Add criterion dev-dep |
| Modify | `Cargo.toml` (workspace) | Add criterion to workspace deps |

---

### Task 1: `--from-db` Data Extraction

**Files:**
- Modify: `scripts/bootstrap_agent_tree.py`

Add ability to read training data from the Arbiter SQLite database. Each decision+outcome pair becomes a training example: the feature vector is stored in `decisions.feature_vector` (JSON array of 22 floats), and the label is derived from `outcomes.status` + `decisions.chosen_agent`.

- [ ] **Step 1: Add `--from-db` CLI argument**

In `main()`, add to the argument parser after `--seed`:

```python
    parser.add_argument(
        "--from-db",
        default=None,
        help="Path to arbiter.db — extract training data from outcomes",
    )
```

- [ ] **Step 2: Add `extract_from_db` function**

Add this function before `train_and_export`:

```python
import sqlite3


def extract_from_db(
    db_path: str,
) -> tuple[list[list[float]], list[int]]:
    """Extract training data from Arbiter SQLite database.

    Joins decisions with outcomes to get (feature_vector, agent_label) pairs.
    Only successful outcomes are used as positive training signal.
    Failed outcomes are excluded (the tree should NOT learn to repeat failures).

    Returns (X, y) where X is list of 22-dim feature vectors
    and y is list of agent class indices.
    """
    conn = sqlite3.connect(db_path)
    cursor = conn.execute(
        """
        SELECT d.feature_vector, d.chosen_agent, o.status
        FROM decisions d
        JOIN outcomes o ON o.decision_id = d.id
        WHERE o.status = 'success'
        ORDER BY d.id
        """
    )

    examples: list[list[float]] = []
    labels: list[int] = []
    skipped = 0

    for row in cursor:
        feature_json, agent_id, _status = row

        # Parse feature vector
        try:
            features = json.loads(feature_json)
        except (json.JSONDecodeError, TypeError):
            skipped += 1
            continue

        if len(features) != 22:
            skipped += 1
            continue

        # Map agent to class index
        if agent_id not in AGENT_IDX:
            skipped += 1
            continue

        examples.append(features)
        labels.append(AGENT_IDX[agent_id])

    conn.close()

    if skipped > 0:
        print(f"  Skipped {skipped} rows (parse errors or unknown agents)")

    return examples, labels
```

- [ ] **Step 3: Update `train_and_export` to accept optional DB data**

Change the signature to:

```python
def train_and_export(
    output_path: str = "models/agent_policy_tree.json",
    seed: int = 42,
    from_db: str | None = None,
) -> dict:
```

After `X_raw, y_raw = generate_expert_examples(rng)`, add:

```python
    # Merge DB data if available
    if from_db:
        print(f"\nExtracting training data from {from_db}")
        db_X, db_y = extract_from_db(from_db)
        print(f"  Extracted {len(db_X)} examples from database")
        if db_X:
            X_raw.extend(db_X)
            y_raw.extend(db_y)
            print(
                f"  Total training data: {len(X_raw)} examples "
                f"({len(db_X)} from DB + {len(X_raw) - len(db_X)} from expert rules)"
            )
```

- [ ] **Step 4: Update main() to pass --from-db**

```python
    stats = train_and_export(args.output, args.seed, args.from_db)
```

- [ ] **Step 5: Run the script without --from-db (regression test)**

Run: `cd /Users/Andrei_Shtanakov/labs/all_ai_orchestrators/arbiter && uv run python scripts/bootstrap_agent_tree.py`
Expected: Same output as before — expert rules only, CV > 90%, exits 0.

- [ ] **Step 6: Commit**

```bash
git add scripts/bootstrap_agent_tree.py
git commit -m "feat: add --from-db to bootstrap script for training on real outcomes"
```

---

### Task 2: Feature Importance Report

**Files:**
- Modify: `scripts/bootstrap_agent_tree.py`

Add feature importance output after training the tree.

- [ ] **Step 1: Add feature importance reporting**

In `train_and_export()`, after the confusion matrix printout and before the "Validate constraints" section, add:

```python
    # Feature importance report
    importances = clf.feature_importances_
    sorted_idx = np.argsort(importances)[::-1]

    print("\nFeature importance (top 10):")
    for rank, idx in enumerate(sorted_idx[:10], 1):
        name = FEATURE_NAMES[idx] if idx < len(FEATURE_NAMES) else f"feature[{idx}]"
        print(f"  {rank:2d}. {name:30s} {importances[idx]:.4f}")

    # Features with zero importance
    zero_features = [
        FEATURE_NAMES[i]
        for i in range(len(FEATURE_NAMES))
        if importances[i] == 0.0
    ]
    if zero_features:
        print(f"\n  Zero-importance features ({len(zero_features)}): "
              f"{', '.join(zero_features)}")
```

- [ ] **Step 2: Add importance to return dict**

```python
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
```

- [ ] **Step 3: Run the script**

Run: `cd /Users/Andrei_Shtanakov/labs/all_ai_orchestrators/arbiter && uv run python scripts/bootstrap_agent_tree.py`
Expected: Output includes "Feature importance (top 10)" section. Exits 0.

- [ ] **Step 4: Commit**

```bash
git add scripts/bootstrap_agent_tree.py
git commit -m "feat: add feature importance report to bootstrap tree training"
```

---

### Task 3: Eval Framework

**Files:**
- Create: `scripts/eval_tree.py`

A/B comparison of three routing strategies on a synthetic benchmark suite.

- [ ] **Step 1: Create `scripts/eval_tree.py`**

```python
"""Evaluate decision tree routing against baseline strategies.

Compares three strategies on a 50-task benchmark suite:
1. Decision Tree (DT) — the trained tree
2. Round-Robin (RR) — cycle through agents
3. Always-Best (AB) — always pick the agent with highest success rate

Usage:
    uv run python scripts/eval_tree.py
    uv run python scripts/eval_tree.py --tree models/custom.json
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import numpy as np

# ---------------------------------------------------------------------------
# Constants (must match bootstrap_agent_tree.py)
# ---------------------------------------------------------------------------

AGENTS: list[str] = ["claude_code", "codex_cli", "aider"]

FEATURE_NAMES: list[str] = [
    "task_type", "language", "complexity", "priority",
    "scope_size", "estimated_tokens", "has_dependencies",
    "requires_internet", "sla_minutes",
    "agent_success_rate", "agent_available_slots",
    "agent_running_tasks", "agent_avg_duration_min",
    "agent_avg_cost_usd", "agent_recent_failures",
    "agent_supports_task_type", "agent_supports_language",
    "total_running_tasks", "total_pending_tasks",
    "budget_remaining_usd", "time_of_day_hour",
    "concurrent_scope_conflicts",
]

# Agent cost per hour (from config/agents.toml)
AGENT_COSTS: dict[str, float] = {
    "claude_code": 0.30,
    "codex_cli": 0.20,
    "aider": 0.10,
}

# Expert-defined "correct" agent for each scenario
# (task_type_ordinal, language_ordinal, complexity_ordinal) -> best agent
EXPERT_RULES: dict[tuple[int, int, int], str] = {
    # Complex Rust -> claude_code
    (0, 1, 3): "claude_code",
    (0, 1, 4): "claude_code",
    (1, 1, 3): "claude_code",
    # Simple Python bugfix -> aider
    (1, 0, 0): "aider",
    (1, 0, 1): "aider",
    # TypeScript feature -> codex_cli
    (0, 2, 0): "codex_cli",
    (0, 2, 1): "codex_cli",
    (0, 2, 2): "codex_cli",
    # Go anything -> codex_cli
    (0, 3, 1): "codex_cli",
    (1, 3, 2): "codex_cli",
    # Moderate Python -> codex_cli
    (0, 0, 2): "codex_cli",
    (1, 0, 2): "codex_cli",
    # Simple refactor -> aider
    (2, 0, 0): "aider",
    (2, 0, 1): "aider",
    (2, 2, 0): "aider",
    # Test writing -> aider
    (3, 0, 1): "aider",
    (3, 0, 2): "aider",
    # Docs/review -> claude_code
    (4, 0, 2): "claude_code",
    (5, 1, 2): "claude_code",
    (6, 0, 1): "claude_code",
}


# ---------------------------------------------------------------------------
# Tree loading (same format as bootstrap_agent_tree.py export)
# ---------------------------------------------------------------------------

def load_tree(path: str) -> dict:
    """Load a decision tree JSON from disk."""
    return json.loads(Path(path).read_text())


def predict_tree(tree: dict, features: list[float]) -> int:
    """Run inference on the tree. Returns predicted class index."""
    nodes = tree["nodes"]
    idx = 0
    while True:
        node = nodes[idx]
        if node["feature"] < 0:
            # Leaf node
            values = node["value"]
            return int(np.argmax(values))
        feat_val = features[node["feature"]]
        if feat_val <= node["threshold"]:
            idx = node["left"]
        else:
            idx = node["right"]


# ---------------------------------------------------------------------------
# Benchmark suite
# ---------------------------------------------------------------------------

def generate_benchmark_suite(
    rng: np.random.Generator,
    n: int = 50,
) -> list[tuple[list[float], str]]:
    """Generate n benchmark tasks with expert-assigned correct agents.

    Returns list of (feature_vector, correct_agent).
    """
    suite: list[tuple[list[float], str]] = []
    keys = list(EXPERT_RULES.keys())

    for i in range(n):
        key = keys[i % len(keys)]
        task_type, language, complexity = key
        correct_agent = EXPERT_RULES[key]

        features = [0.0] * 22
        features[0] = float(task_type)
        features[1] = float(language)
        features[2] = float(complexity)
        features[3] = float(rng.integers(0, 4))  # priority
        features[4] = float(rng.integers(1, 20))  # scope_size
        features[5] = float(rng.integers(10, 100))  # estimated_tokens
        features[6] = float(rng.integers(0, 2))  # has_dependencies
        features[7] = 0.0  # requires_internet
        features[8] = float(rng.integers(30, 240))  # sla_minutes
        features[9] = 0.85  # agent_success_rate
        features[10] = 2.0  # agent_available_slots
        features[11] = 0.0  # agent_running_tasks
        features[12] = 15.0  # agent_avg_duration_min
        features[13] = 0.15  # agent_avg_cost_usd
        features[14] = 0.0  # agent_recent_failures
        features[15] = 1.0  # agent_supports_task_type
        features[16] = 1.0  # agent_supports_language
        features[17] = 1.0  # total_running_tasks
        features[18] = 3.0  # total_pending_tasks
        features[19] = 8.0  # budget_remaining_usd
        features[20] = float(rng.integers(8, 20))  # time_of_day_hour
        features[21] = 0.0  # concurrent_scope_conflicts

        suite.append((features, correct_agent))

    return suite


# ---------------------------------------------------------------------------
# Strategies
# ---------------------------------------------------------------------------

def strategy_dt(
    tree: dict, features: list[float],
) -> str:
    """Decision Tree strategy."""
    class_idx = predict_tree(tree, features)
    return AGENTS[class_idx]


def strategy_round_robin(
    _features: list[float], counter: int,
) -> str:
    """Round-robin strategy."""
    return AGENTS[counter % len(AGENTS)]


def strategy_always_best(
    _features: list[float],
) -> str:
    """Always pick claude_code (assumed best)."""
    return "claude_code"


# ---------------------------------------------------------------------------
# Evaluation
# ---------------------------------------------------------------------------

def evaluate(
    tree_path: str,
    seed: int = 42,
    n_tasks: int = 50,
) -> dict:
    """Run evaluation and return results."""
    rng = np.random.default_rng(seed)
    tree = load_tree(tree_path)
    suite = generate_benchmark_suite(rng, n_tasks)

    results: dict[str, dict] = {}

    for name, strategy_fn in [
        ("decision_tree", lambda f, i: strategy_dt(tree, f)),
        ("round_robin", lambda f, i: strategy_round_robin(f, i)),
        ("always_claude", lambda f, i: strategy_always_best(f)),
    ]:
        correct = 0
        total_cost = 0.0

        for i, (features, expected_agent) in enumerate(suite):
            predicted = strategy_fn(features, i)
            if predicted == expected_agent:
                correct += 1
            total_cost += AGENT_COSTS.get(predicted, 0.20)

        accuracy = correct / len(suite) if suite else 0.0
        results[name] = {
            "accuracy": accuracy,
            "correct": correct,
            "total": len(suite),
            "total_cost": total_cost,
            "avg_cost": total_cost / len(suite) if suite else 0.0,
        }

    return results


def print_results(results: dict) -> None:
    """Pretty-print evaluation results."""
    print("\n" + "=" * 70)
    print("EVALUATION RESULTS")
    print("=" * 70)
    print(
        f"{'Strategy':<20} {'Accuracy':>10} {'Correct':>10} "
        f"{'Avg Cost':>10} {'Total Cost':>12}"
    )
    print("-" * 70)
    for name, r in results.items():
        print(
            f"{name:<20} {r['accuracy']:>9.1%} "
            f"{r['correct']:>7}/{r['total']:<3} "
            f"${r['avg_cost']:>8.2f} "
            f"${r['total_cost']:>10.2f}"
        )
    print("=" * 70)

    # DT should beat random
    dt_acc = results["decision_tree"]["accuracy"]
    rr_acc = results["round_robin"]["accuracy"]
    print(
        f"\nDT vs Round-Robin: "
        f"{'+' if dt_acc > rr_acc else ''}"
        f"{(dt_acc - rr_acc) * 100:.1f}pp"
    )


def main() -> None:
    """CLI entry point."""
    parser = argparse.ArgumentParser(
        description="Evaluate Arbiter decision tree against baselines"
    )
    parser.add_argument(
        "--tree",
        default="models/agent_policy_tree.json",
        help="Path to decision tree JSON",
    )
    parser.add_argument(
        "--seed",
        type=int,
        default=42,
        help="Random seed",
    )
    parser.add_argument(
        "--tasks",
        type=int,
        default=50,
        help="Number of benchmark tasks",
    )
    args = parser.parse_args()

    results = evaluate(args.tree, args.seed, args.tasks)
    print_results(results)

    # DT must beat round-robin
    if results["decision_tree"]["accuracy"] <= results["round_robin"]["accuracy"]:
        print(
            "\nFAIL: Decision tree does not beat round-robin!",
            file=sys.stderr,
        )
        sys.exit(1)

    print("\nPASS: Decision tree beats round-robin.")


if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Run the eval script**

Run: `cd /Users/Andrei_Shtanakov/labs/all_ai_orchestrators/arbiter && uv run python scripts/eval_tree.py`
Expected: Table showing DT accuracy > RR accuracy. Exits 0.

- [ ] **Step 3: Commit**

```bash
git add scripts/eval_tree.py
git commit -m "feat: add eval framework comparing DT vs round-robin vs always-best"
```

---

### Task 4: Criterion Benchmarks

**Files:**
- Create: `arbiter-cli/benches/routing.rs`
- Modify: `arbiter-cli/Cargo.toml`
- Modify: `Cargo.toml` (workspace)

Replace custom timing in arbiter-cli with criterion for statistically rigorous benchmarks.

- [ ] **Step 1: Add criterion dependency**

In workspace `Cargo.toml` `[workspace.dependencies]`:
```toml
criterion = { version = "0.5", features = ["html_reports"] }
```

In `arbiter-cli/Cargo.toml`, add:
```toml
[dev-dependencies]
criterion = { workspace = true }

[[bench]]
name = "routing"
harness = false
```

- [ ] **Step 2: Create `arbiter-cli/benches/routing.rs`**

```rust
//! Criterion benchmarks for Arbiter routing.
//!
//! Run: cargo bench -p arbiter-cli

// Database (rusqlite) is Send but not Sync; Arc is used for shared
// ownership, not for cross-thread access.
#![allow(clippy::arc_with_non_send_sync)]

use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, Criterion};

use arbiter_core::policy::decision_tree::DecisionTree;
use arbiter_core::types::*;

use arbiter_mcp::agents::AgentRegistry;
use arbiter_mcp::config::*;
use arbiter_mcp::db::Database;
use arbiter_mcp::metrics::Metrics;
use arbiter_mcp::tools::route_task;

use std::collections::HashMap;

fn load_bootstrap_tree() -> DecisionTree {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = std::path::Path::new(manifest_dir)
        .parent()
        .unwrap()
        .join("models/agent_policy_tree.json");
    let json = std::fs::read_to_string(&path).unwrap();
    DecisionTree::from_json(&json).unwrap()
}

fn bench_agents() -> HashMap<String, AgentConfig> {
    let mut agents = HashMap::new();
    agents.insert(
        "claude_code".to_string(),
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
        "codex_cli".to_string(),
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
            supports_languages: vec![
                "python".to_string(),
                "javascript".to_string(),
            ],
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

fn bench_invariant_config() -> InvariantConfig {
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
            max_total_concurrent: 10,
        },
        sla: SlaConfig {
            buffer_multiplier: 1.5,
        },
    }
}

fn bench_route_task(c: &mut Criterion) {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    let db = Arc::new(db);
    let tree = load_bootstrap_tree();
    let agents = bench_agents();
    let registry = AgentRegistry::new(Arc::clone(&db), &agents).unwrap();
    let invariant_cfg = bench_invariant_config();
    let metrics = Metrics::new();

    let task = TaskInput {
        task_type: TaskType::Bugfix,
        language: Language::Python,
        complexity: Complexity::Moderate,
        priority: Priority::Normal,
        scope: vec!["src/main.py".to_string()],
        branch: Some("fix/test".to_string()),
        estimated_tokens: Some(30000),
        has_dependencies: false,
        requires_internet: false,
        sla_minutes: Some(60),
        description: Some("Fix benchmark task".to_string()),
    };

    let constraints = Constraints {
        preferred_agent: None,
        excluded_agents: vec![],
        budget_remaining_usd: Some(100.0),
        total_pending_tasks: Some(5),
        running_tasks: vec![],
        retry_count: None,
        calls_per_minute: None,
    };

    let mut counter = 0u64;

    c.bench_function("route_task", |b| {
        b.iter(|| {
            counter += 1;
            let task_id = format!("bench-{counter}");
            let result = route_task::execute(
                black_box(&task_id),
                black_box(&task),
                black_box(&constraints),
                Some(&tree),
                &registry,
                &db,
                &invariant_cfg,
                &metrics,
            )
            .unwrap();
            // Reset running tasks to prevent slot exhaustion
            let _ = db.decrement_running_tasks(&result.chosen_agent);
            result
        });
    });
}

fn bench_dt_predict(c: &mut Criterion) {
    let tree = load_bootstrap_tree();

    let features: [f64; 22] = [
        1.0, 0.0, 2.0, 1.0, 5.0, 50.0, 0.0, 0.0, 120.0,
        0.85, 2.0, 0.0, 15.0, 0.15, 0.0, 1.0, 1.0,
        1.0, 3.0, 8.0, 14.0, 0.0,
    ];

    c.bench_function("dt_predict", |b| {
        b.iter(|| tree.predict(black_box(&features)).unwrap());
    });
}

criterion_group!(benches, bench_route_task, bench_dt_predict);
criterion_main!(benches);
```

- [ ] **Step 3: Run the benchmarks**

Run: `cd /Users/Andrei_Shtanakov/labs/all_ai_orchestrators/arbiter && cargo bench -p arbiter-cli`
Expected: Criterion output with statistical analysis for `route_task` and `dt_predict`.

- [ ] **Step 4: Run all tests to verify nothing broke**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add arbiter-cli/benches/routing.rs arbiter-cli/Cargo.toml Cargo.toml
git commit -m "feat: add criterion benchmarks for route_task and dt_predict"
```

---

## Exit Criteria Checklist

- [ ] `cargo test --workspace` — all tests pass
- [ ] `cargo clippy --workspace -- -D warnings` — clean
- [ ] `uv run python scripts/bootstrap_agent_tree.py` — exits 0 with feature importance
- [ ] `uv run python scripts/bootstrap_agent_tree.py --from-db arbiter.db` — works (or gracefully handles empty DB)
- [ ] `uv run python scripts/eval_tree.py` — DT beats round-robin on benchmark suite
- [ ] `cargo bench -p arbiter-cli` — criterion benchmarks run with statistics
- [ ] Feature importance report shows top 10 features
