# Bind `params._meta.traceparent` per Request — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When Maestro sends `params._meta.traceparent` on a `tools/call` (shipped in maestro#88), arbiter binds that W3C trace context for the duration of handling the request, so `route.decision` / `outcome.recorded` / all request-scoped records carry the caller's `TraceId` and correlate cross-process.

**Architecture:** A thread-local per-request override in `arbiter-core/src/obs.rs` (`REQUEST_TRACE` + RAII `RequestTraceGuard`), consulted by `on_new_span`/`on_event` where they currently fall back to the process-wide `ROOT` context. `server.rs::handle_tools_call` extracts `_meta.traceparent` and holds the guard while dispatching. The server processes one request at a time on one thread, so a thread-local is exact. Absent/malformed traceparent → silent fallback to today's behavior (handoff contract: not an error).

**Handoff source:** `prograph-vault/authored/notes/2026-07-19-arbiter-meta-traceparent-handoff.md` — sender guarantees: strict `00-<32hex>-<16hex>-01`, all-zero ids never sent, `_meta` omitted when no context.

## Global Constraints

- Rust: `cargo fmt`, `cargo clippy --workspace`, `cargo test --workspace` must pass.
- obs.rs is arbiter's own implementation of the ecosystem observability v1 contract — the JSONL schema must NOT change (only where trace context is sourced from).
- Test-file-per-binary convention in `arbiter-core/tests/` (global tracing subscriber; see header of `emit_contract.rs`).
- `parse_traceparent` (obs.rs:575) already validates format and rejects all-zero ids — reuse, do not duplicate.
- Branch `feat/meta-traceparent-binding`; PR-only; human merges.

## File Structure

| File | Role |
|---|---|
| `arbiter-core/src/obs.rs` | Modify: `REQUEST_TRACE` thread-local, `bind_request_trace()` + `RequestTraceGuard`, override in `on_new_span`/`on_event` fallback branches |
| `arbiter-core/tests/request_trace.rs` | **Create**: own test binary — bind → records carry caller trace; drop → root trace returns; malformed → None |
| `arbiter-mcp/src/server.rs` | Modify: extract + bind in `handle_tools_call` (`:457`) |
| `TODO.md` | Record the closed item |

### Task 1: obs.rs override + contract test

- [ ] Step 1: failing test `arbiter-core/tests/request_trace.rs` (own binary): init_logging into tempdir WITHOUT `TRACEPARENT` env (random root trace); `bind_request_trace("00-<T>-<S>-01")` → emit plain event + spanned event; drop guard; emit post-drop event. Assert: bound records have `TraceId == T`, span record has `parent_span_id == S`; post-drop record's TraceId != T; `bind_request_trace("garbage")` returns `None`.
- [ ] Step 2: `cargo test -p arbiter-core --test request_trace` → compile FAIL (no `bind_request_trace`).
- [ ] Step 3: implement in obs.rs: thread-local + guard + `request_trace()` accessor; in `on_new_span` root-fallback branch (`:216`) and `on_event` root-fallback branch (`:330-331`) consult `request_trace()` before `ROOT`.
- [ ] Step 4: test passes; `cargo test -p arbiter-core` all green.
- [ ] Step 5: commit.

### Task 2: server.rs wiring

- [ ] Step 1: in `handle_tools_call` after the `params` match (`server.rs:472`), bind:
  `let _trace_guard = params.get("_meta").and_then(|m| m.get("traceparent")).and_then(|v| v.as_str()).and_then(arbiter_core::obs::bind_request_trace);`
- [ ] Step 2: server unit test: `tools/call` request JSON with `_meta.traceparent` still dispatches successfully (reuse existing test helpers in `server.rs` tests mod).
- [ ] Step 3: `cargo test --workspace`, `cargo clippy --workspace`, `cargo fmt` — green; commit.

### Task 3: TODO + PR

- [ ] TODO.md entry; push; `gh pr create`; PR body links maestro#88 + handoff note; Copilot review tracking.

## Self-Review
- Contract schema untouched (only trace sourcing). Sender never transmits zero ids, and `parse_traceparent` rejects them anyway (defense in depth).
- Thread-local is correct for the sequential stdio server; if the server ever goes concurrent, the guard moves into per-task context — noted in the guard's doc comment.
- Out of scope: child_env() override during requests (arbiter doesn't spawn children per request), Maestro pin bump (only needed when Maestro wants to RELY on correlation).
