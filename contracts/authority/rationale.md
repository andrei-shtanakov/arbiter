# Authority v1 — rationale

Contracts-roadmap phase 5 (RD-006). Design:
`docs/2026-07-12-authority-split-design.md`.

## What this is

The **authority plane** ("MAY this agent act here"), split from the existing
**capability plane** ("CAN this agent do the task", see
`contracts/capability/`). This contract names two shapes:

1. **Wire input** — `constraints.authority_context` (`definitions.authority_context`):
   the agent-run `role` and coarse lifecycle `phase`, both closed enums.
   It lives in constraints, not in the task, and never enters the 22-dim
   feature vector or DT training semantics.
2. **Audit output** — the top-level object, attached to the routing decision
   at `metadata.authority` whenever the authority feature is enabled:
   `policy_sha` (sha256 of the loaded policy file), the context echoed back
   (null when absent), and the `denied` list with per-agent reasons.

## Semantics (normative)

- Pure **allowlist, default deny**; no explicit deny rules in v1.
- Agent patterns are exactly two forms: exact `harness@model` or `harness@*`.
  Arbitrary globs are rejected at policy load.
- Enforcement order: capability hard-filter → **authority filter** → scoring →
  benchmark re-rank → invariants. The audit therefore only ever names
  capability-eligible candidates.
- Zero authorized candidates → `action: reject` with
  `reasoning: authority_no_authorized_candidates` — a deterministic policy
  denial (not HOLD), logged to the decisions table like any other decision
  (PolicyDecisionRef unchanged).
- A request without `authority_context` while the feature is enabled is
  decided by the policy's `unknown_context` (`deny` fail-closed default;
  `allow` is a migration mode).
- Feature off (no `config/authority.toml`) → no `metadata.authority` key at
  all; existing consumers and golden fixtures are untouched.

## Source of truth

- Engine: `arbiter-core/src/authority.rs` (pure; policy injected).
- Loading: `arbiter-mcp/src/config.rs::load_authority` — reads
  `config/authority.toml`, computes `policy_sha`, validates closed
  vocabularies and pattern forms; an invalid file is a hard config error
  (installing a broken allowlist silently would be fail-open). Hot-reload
  via the existing `watcher.rs` path.
- Enforcement: `arbiter-mcp/src/tools/route_task.rs` (step 3b).
- Policy data SSOT: **steward** `profiles/authority.yaml` (governance,
  PR-reviewed); `config/authority.toml` here is a pinned vendored copy
  (agents-catalog pattern). Conformance:
  `scripts/check_authority_conformance.py`.

## Contract tests

`arbiter-mcp/tests/promoted_contracts.rs` validates the LIVE
`metadata.authority` of a real `route_task` execution against this schema,
plus every fixture under `fixtures/`.

## Consumers

- Maestro (supplies `authority_context` from workstream role/phase — handoff M4
  of the design; consumes the audit for gate verdict records).
- dispatcher (read-side: surfaces authority denials in work-items drill-down).
