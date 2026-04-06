"""Evaluation framework comparing DT vs round-robin vs always-claude.

Generates a synthetic benchmark suite of tasks with expert-defined
correct answers, then measures accuracy and cost for three strategies:
  1. Decision Tree (DT) — loads trained tree JSON and runs inference
  2. Round-Robin (RR) — cycles through agents
  3. Always-Claude (AB) — always picks "claude_code"

Usage:
    uv run python scripts/eval_tree.py
    uv run python scripts/eval_tree.py --tree models/custom.json --tasks 100
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from pathlib import Path

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

AGENTS: list[str] = ["claude_code", "codex_cli", "aider"]

AGENT_COSTS: dict[str, float] = {
    "claude_code": 0.30,
    "codex_cli": 0.20,
    "aider": 0.10,
}

# Feature indices (22-dim vector)
F_TASK_TYPE = 0
F_LANGUAGE = 1
F_COMPLEXITY = 2
F_PRIORITY = 3
F_SCOPE_SIZE = 4
F_EST_TOKENS = 5
F_HAS_DEPS = 6
F_REQ_INTERNET = 7
F_SLA_MINUTES = 8
F_SUCCESS_RATE = 9
F_AVAIL_SLOTS = 10
F_RUNNING_TASKS = 11
F_AVG_DURATION = 12
F_AVG_COST = 13
F_RECENT_FAILURES = 14
F_SUPPORTS_TYPE = 15
F_SUPPORTS_LANG = 16
F_TOTAL_RUNNING = 17
F_TOTAL_PENDING = 18
F_BUDGET_REMAINING = 19
F_TIME_OF_DAY = 20
F_SCOPE_CONFLICTS = 21

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


# ---------------------------------------------------------------------------
# Expert rules (ground truth for evaluation)
# ---------------------------------------------------------------------------


@dataclass
class ExpertRule:
    """Maps (task_type, language, complexity) to the correct agent."""

    task_type: int
    language: int
    complexity: int
    agent: str
    avg_duration: float
    avg_cost: float


# 20 expert rules covering the main routing patterns from bootstrap
EXPERT_RULES: list[ExpertRule] = [
    # Rule group 1: complex/critical + Rust -> claude_code
    ExpertRule(TASK_FEATURE, LANG_RUST, COMP_COMPLEX, "claude_code", 15.0, 0.15),
    ExpertRule(TASK_BUGFIX, LANG_RUST, COMP_CRITICAL, "claude_code", 15.0, 0.15),
    ExpertRule(TASK_REFACTOR, LANG_RUST, COMP_COMPLEX, "claude_code", 15.0, 0.15),
    # Rule group 2: complex/critical + Python -> claude_code
    ExpertRule(TASK_FEATURE, LANG_PYTHON, COMP_COMPLEX, "claude_code", 15.0, 0.15),
    ExpertRule(TASK_BUGFIX, LANG_PYTHON, COMP_CRITICAL, "claude_code", 15.0, 0.15),
    # Rule group 3: docs/review/research -> claude_code
    ExpertRule(TASK_DOCS, LANG_PYTHON, COMP_MODERATE, "claude_code", 15.0, 0.15),
    ExpertRule(TASK_REVIEW, LANG_RUST, COMP_SIMPLE, "claude_code", 15.0, 0.15),
    ExpertRule(TASK_RESEARCH, LANG_TYPESCRIPT, COMP_TRIVIAL, "claude_code", 15.0, 0.15),
    # Rule group 4: trivial/simple + bugfix -> aider
    ExpertRule(TASK_BUGFIX, LANG_PYTHON, COMP_TRIVIAL, "aider", 8.0, 0.05),
    ExpertRule(TASK_BUGFIX, LANG_TYPESCRIPT, COMP_SIMPLE, "aider", 8.0, 0.05),
    ExpertRule(TASK_BUGFIX, LANG_RUST, COMP_TRIVIAL, "aider", 8.0, 0.05),
    # Rule group 5: trivial/simple + refactor -> aider
    ExpertRule(TASK_REFACTOR, LANG_PYTHON, COMP_SIMPLE, "aider", 6.0, 0.04),
    ExpertRule(TASK_REFACTOR, LANG_TYPESCRIPT, COMP_TRIVIAL, "aider", 6.0, 0.04),
    # Rule group 6: TypeScript + feature -> codex_cli
    ExpertRule(TASK_FEATURE, LANG_TYPESCRIPT, COMP_SIMPLE, "codex_cli", 12.0, 0.12),
    ExpertRule(TASK_FEATURE, LANG_TYPESCRIPT, COMP_MODERATE, "codex_cli", 12.0, 0.12),
    # Rule group 7: Go language -> codex_cli
    ExpertRule(TASK_FEATURE, LANG_GO, COMP_SIMPLE, "codex_cli", 10.0, 0.10),
    ExpertRule(TASK_BUGFIX, LANG_GO, COMP_MODERATE, "codex_cli", 10.0, 0.10),
    # Rule group 8: moderate + Python -> codex_cli
    ExpertRule(TASK_FEATURE, LANG_PYTHON, COMP_MODERATE, "codex_cli", 12.0, 0.10),
    # Rule group 9: test + simple/moderate -> aider
    ExpertRule(TASK_TEST, LANG_PYTHON, COMP_SIMPLE, "aider", 7.0, 0.05),
    ExpertRule(TASK_TEST, LANG_RUST, COMP_MODERATE, "aider", 7.0, 0.05),
]


# ---------------------------------------------------------------------------
# Benchmark task generation
# ---------------------------------------------------------------------------


@dataclass
class BenchmarkTask:
    """A benchmark task with features and expected correct agent."""

    features: list[float]
    expected_agent: str


def _make_features(
    rule: ExpertRule,
    rng_vals: dict[str, float],
) -> list[float]:
    """Build a 22-dim feature vector from a rule + random fill values."""
    features = [0.0] * 22
    features[F_TASK_TYPE] = float(rule.task_type)
    features[F_LANGUAGE] = float(rule.language)
    features[F_COMPLEXITY] = float(rule.complexity)
    features[F_PRIORITY] = rng_vals["priority"]
    features[F_SCOPE_SIZE] = rng_vals["scope_size"]
    features[F_EST_TOKENS] = rng_vals["est_tokens"]
    features[F_HAS_DEPS] = rng_vals["has_deps"]
    features[F_REQ_INTERNET] = (
        1.0
        if rule.task_type in (TASK_DOCS, TASK_REVIEW, TASK_RESEARCH)
        else rng_vals["req_internet"]
    )
    features[F_SLA_MINUTES] = rng_vals["sla_minutes"]
    features[F_SUCCESS_RATE] = rng_vals["success_rate"]
    features[F_AVAIL_SLOTS] = rng_vals["avail_slots"]
    features[F_RUNNING_TASKS] = rng_vals["running_tasks"]
    features[F_AVG_DURATION] = rule.avg_duration
    features[F_AVG_COST] = rule.avg_cost
    features[F_RECENT_FAILURES] = rng_vals["recent_failures"]
    features[F_SUPPORTS_TYPE] = 1.0
    features[F_SUPPORTS_LANG] = 1.0
    features[F_TOTAL_RUNNING] = rng_vals["total_running"]
    features[F_TOTAL_PENDING] = rng_vals["total_pending"]
    features[F_BUDGET_REMAINING] = rng_vals["budget_remaining"]
    features[F_TIME_OF_DAY] = rng_vals["time_of_day"]
    features[F_SCOPE_CONFLICTS] = rng_vals["scope_conflicts"]
    return features


def generate_benchmark(
    n_tasks: int,
    seed: int,
) -> list[BenchmarkTask]:
    """Generate n_tasks benchmark tasks cycling through expert rules."""
    import random

    rng = random.Random(seed)
    tasks: list[BenchmarkTask] = []

    for i in range(n_tasks):
        rule = EXPERT_RULES[i % len(EXPERT_RULES)]
        rng_vals = {
            "priority": float(rng.randint(0, 3)),
            "scope_size": rng.uniform(1.0, 20.0),
            "est_tokens": rng.uniform(10.0, 120.0),
            "has_deps": float(rng.randint(0, 1)),
            "req_internet": float(rng.randint(0, 1)),
            "sla_minutes": rng.uniform(30.0, 360.0),
            "success_rate": rng.uniform(0.7, 0.99),
            "avail_slots": float(rng.randint(1, 5)),
            "running_tasks": float(rng.randint(0, 3)),
            "recent_failures": float(rng.randint(0, 3)),
            "total_running": float(rng.randint(0, 5)),
            "total_pending": float(rng.randint(0, 10)),
            "budget_remaining": rng.uniform(5.0, 50.0),
            "time_of_day": float(rng.randint(8, 20)),
            "scope_conflicts": float(rng.randint(0, 2)),
        }
        features = _make_features(rule, rng_vals)
        tasks.append(BenchmarkTask(features=features, expected_agent=rule.agent))

    return tasks


# ---------------------------------------------------------------------------
# Tree inference
# ---------------------------------------------------------------------------


def load_tree(path: str) -> dict:
    """Load decision tree JSON from file."""
    with open(path) as f:
        return json.load(f)


def predict_tree(tree: dict, features: list[float]) -> str:
    """Run DT inference: traverse nodes, return predicted agent name."""
    nodes = tree["nodes"]
    class_names = tree["class_names"]
    idx = 0

    while True:
        node = nodes[idx]
        if node["feature"] == -1:
            # Leaf node: return class with highest value
            values = node["value"]
            best_class = values.index(max(values))
            return class_names[best_class]
        if features[node["feature"]] <= node["threshold"]:
            idx = node["left"]
        else:
            idx = node["right"]


# ---------------------------------------------------------------------------
# Strategies
# ---------------------------------------------------------------------------


def strategy_decision_tree(
    tree: dict,
    tasks: list[BenchmarkTask],
) -> list[str]:
    """DT strategy: use tree inference for each task."""
    return [predict_tree(tree, t.features) for t in tasks]


def strategy_round_robin(tasks: list[BenchmarkTask]) -> list[str]:
    """Round-robin strategy: cycle through agents."""
    return [AGENTS[i % len(AGENTS)] for i in range(len(tasks))]


def strategy_always_claude(tasks: list[BenchmarkTask]) -> list[str]:
    """Always-claude strategy: always pick claude_code."""
    return ["claude_code"] * len(tasks)


# ---------------------------------------------------------------------------
# Evaluation
# ---------------------------------------------------------------------------


@dataclass
class StrategyResult:
    """Evaluation result for a single strategy."""

    name: str
    predictions: list[str]
    correct: int
    total: int
    accuracy: float
    avg_cost: float
    total_cost: float


def evaluate_strategy(
    name: str,
    predictions: list[str],
    tasks: list[BenchmarkTask],
) -> StrategyResult:
    """Compute accuracy and cost metrics for a strategy."""
    correct = sum(1 for p, t in zip(predictions, tasks) if p == t.expected_agent)
    total = len(tasks)
    accuracy = correct / total if total > 0 else 0.0
    costs = [AGENT_COSTS[p] for p in predictions]
    avg_cost = sum(costs) / len(costs) if costs else 0.0
    total_cost = sum(costs)
    return StrategyResult(
        name=name,
        predictions=predictions,
        correct=correct,
        total=total,
        accuracy=accuracy,
        avg_cost=avg_cost,
        total_cost=total_cost,
    )


def print_results(results: list[StrategyResult]) -> None:
    """Print the comparison table."""
    width = 70
    print("=" * width)
    print("EVALUATION RESULTS")
    print("=" * width)
    header = (
        f"{'Strategy':<20} {'Accuracy':>10} {'Correct':>9}"
        f" {'Avg Cost':>10} {'Total Cost':>12}"
    )
    print(header)
    print("-" * width)
    for r in results:
        line = (
            f"{r.name:<20} {r.accuracy:>9.1%}"
            f" {r.correct:>5}/{r.total:<3}"
            f" {'${:.2f}'.format(r.avg_cost):>10}"
            f" {'${:.2f}'.format(r.total_cost):>12}"
        )
        print(line)
    print("=" * width)


def main() -> None:
    """CLI entry point."""
    parser = argparse.ArgumentParser(
        description="Evaluate DT vs round-robin vs always-claude"
    )
    parser.add_argument(
        "--tree",
        default="models/agent_policy_tree.json",
        help="Path to decision tree JSON (default: models/agent_policy_tree.json)",
    )
    parser.add_argument(
        "--seed",
        type=int,
        default=42,
        help="Random seed (default: 42)",
    )
    parser.add_argument(
        "--tasks",
        type=int,
        default=50,
        help="Number of benchmark tasks (default: 50)",
    )
    args = parser.parse_args()

    # Load tree
    tree_path = Path(args.tree)
    if not tree_path.exists():
        print(f"ERROR: Tree file not found: {tree_path}", file=sys.stderr)
        sys.exit(1)
    tree = load_tree(str(tree_path))

    # Generate benchmark
    tasks = generate_benchmark(args.tasks, args.seed)

    # Run strategies
    dt_preds = strategy_decision_tree(tree, tasks)
    rr_preds = strategy_round_robin(tasks)
    ac_preds = strategy_always_claude(tasks)

    # Evaluate
    dt_result = evaluate_strategy("decision_tree", dt_preds, tasks)
    rr_result = evaluate_strategy("round_robin", rr_preds, tasks)
    ac_result = evaluate_strategy("always_claude", ac_preds, tasks)

    results = [dt_result, rr_result, ac_result]
    print()
    print_results(results)

    # Comparison
    diff_pp = (dt_result.accuracy - rr_result.accuracy) * 100
    print(f"\nDT vs Round-Robin: {diff_pp:+.1f}pp")

    if dt_result.accuracy > rr_result.accuracy:
        print("\nPASS: Decision tree beats round-robin.")
        sys.exit(0)
    else:
        print(
            "\nFAIL: Decision tree does NOT beat round-robin.",
            file=sys.stderr,
        )
        sys.exit(1)


if __name__ == "__main__":
    main()
