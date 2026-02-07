# Arbiter — Coding Agent Policy Engine
# Commands for development and task management

.PHONY: help build test lint format clean
.PHONY: task-list task-stats task-next task-graph

# === Setup ===

help:  ## Show help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

# === Build & Test (Rust) ===

build:  ## Build the project
	cargo build --release

check:  ## Check without building
	cargo check --workspace

test:  ## Run all Rust tests
	cargo test

test-integration:  ## Integration tests only
	cargo test --test integration

test-core:  ## Tests for arbiter-core
	cargo test -p arbiter-core

test-mcp:  ## Tests for arbiter-mcp
	cargo test -p arbiter-mcp

# === Code Quality ===

lint:  ## Lint (clippy)
	cargo clippy --workspace -- -D warnings

format:  ## Format code
	cargo fmt --all

fmt-check:  ## Check formatting
	cargo fmt --all -- --check

ci: fmt-check lint test  ## CI pipeline

# === Python ===

test-python:  ## Python protocol tests
	uv run pytest orchestrator/tests/ -v

bootstrap-tree:  ## Generate bootstrap decision tree
	uv run python scripts/bootstrap_agent_tree.py

# === Run ===

run:  ## Run MCP server
	cargo run --release --bin arbiter-mcp

run-debug:  ## Run with debug logs
	cargo run --bin arbiter-mcp -- --log-level=debug

# === Task Management ===

task-list:  ## List all tasks
	@uv run python spec/task.py list

task-todo:  ## List TODO tasks
	@uv run python spec/task.py list --status=todo

task-progress:  ## Tasks in progress
	@uv run python spec/task.py list --status=in_progress

task-stats:  ## Task statistics
	@uv run python spec/task.py stats

task-next:  ## Next tasks to work on
	@uv run python spec/task.py next

task-graph:  ## Dependency graph
	@uv run python spec/task.py graph

task-p0:  ## P0 tasks only
	@uv run python spec/task.py list --priority=p0

task-mvp:  ## MVP tasks
	@uv run python spec/task.py list --milestone=mvp

# === Task Workflow ===

task-start:  ## Start a task (make task-start ID=TASK-001)
	@uv run python spec/task.py start "$(ID)"

task-done:  ## Complete a task (make task-done ID=TASK-001)
	@uv run python spec/task.py done "$(ID)"

task-show:  ## Show a task (make task-show ID=TASK-001)
	@uv run python spec/task.py show "$(ID)"

# === Auto Execution (Claude CLI) ===

exec:  ## Execute next task via Claude
	@uv run python spec/executor.py run

exec-task:  ## Execute specific task (make exec-task ID=TASK-001)
	@uv run python spec/executor.py run --task="$(ID)"

exec-all:  ## Execute all ready tasks
	@uv run python spec/executor.py run --all

exec-mvp:  ## Execute MVP tasks
	@uv run python spec/executor.py run --all --milestone=mvp

exec-status:  ## Execution status
	@uv run python spec/executor.py status

exec-retry:  ## Retry a task (make exec-retry ID=TASK-001)
	@uv run python spec/executor.py retry "$(ID)"

exec-logs:  ## Task logs (make exec-logs ID=TASK-001)
	@uv run python spec/executor.py logs "$(ID)"

exec-reset:  ## Reset executor state
	@uv run python spec/executor.py reset

# === Clean ===

clean:  ## Clean artifacts
	cargo clean
	rm -rf spec/.executor-logs/ spec/.executor-state.json
	rm -rf spec/.task-history.log

# === Default ===

.DEFAULT_GOAL := help
