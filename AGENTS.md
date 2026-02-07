# Repository Guidelines

## Project Structure & Module Organization
Rust sources live in `arbiter-core/` (policy logic), `arbiter-mcp/` (MCP server), and `arbiter-cli/` (benchmarks). Shared configs sit under `config/` (`agents.toml`, `invariants.toml`), decision trees in `models/`, and automation scripts in `scripts/`. Python orchestration code and UV-managed tooling live in `orchestrator/` and `spec/`, while repo-level integration tests are inside `tests/`.

## Build, Test, and Development Commands
Use the `Makefile` targets to keep workflows consistent:
- `make build` → `cargo build --release` for the full workspace.
- `make test`, `make test-core`, `make test-mcp` to scope Rust test runs; `make test-python` covers `orchestrator/tests`.
- `make run` (release) or `make run-debug -- --log-level=debug` to launch the MCP server; `make bootstrap-tree` regenerates `models/agent_policy_tree.json`.
- `make lint`, `make format`, `make fmt-check` run Clippy and rustfmt with warnings promoted to errors.
Call `uv run python scripts/bootstrap_agent_tree.py` directly when iterative tree work is needed outside Make.

## Coding Style & Naming Conventions
Rust code follows rustfmt defaults (4-space indent, snake_case modules, UpperCamelCase types). Keep traits and structs in their own files under `arbiter-core/src/`, and prefer explicit module exports in `mod.rs`. Python modules use Black-equivalent 4-space indent, snake_case functions, and PascalCase classes; keep UV virtual environment configs untouched. Config files use TOML keys with hyphen-safe names (`max_fail_rate`, etc.). When creating binaries or tests, mirror existing naming: `arbiter-mcp/src/bin/arbiter-mcp.rs`, `tests/<feature>_test.rs`.

## Testing Guidelines
Rust tests rely on `cargo test`; place unit tests in the same module using `#[cfg(test)]` and integration tests in `tests/`. Python uses `pytest` under UV; name files `test_*.py` and target orchestrator behaviors. Keep decision-path coverage high—add regression cases whenever you change configs in `config/` or `models/`, and pair any new invariant with failing and passing fixtures before merging.

## Commit & Pull Request Guidelines
Match the existing history: start messages with a scope (`docs:`, `fix:`, `arbiter-core:`) or a task ID (`TASK-123:`) followed by a short imperative summary. Avoid multi-topic commits; one feature or fix per changeset. PRs should include: purpose summary, relevant `make test*` results, linked task IDs, and screenshots or logs when touching CLI output. Mention config migrations explicitly and attach diffs for generated artifacts such as `agent_policy_tree.json`.

## Security & Configuration Tips
Keep `config/agents.toml` free of credentials—use placeholders and document secrets in private channels. Do not check in real SQLite files; `arbiter.db` should stay local. When testing decision trees, prefer synthetic data under `models/` and redact any logs before sharing externally.
