# Capability v1 — rationale

Contracts-roadmap phase 5 (RD-006): **promote, don't build**. The capability
plane has existed since R1 as the routing hard-filter's input; this contract
names it so the Capability/Authority split is visible on both sides
(`contracts/authority/` is the other half). Design:
`docs/2026-07-12-authority-split-design.md`.

## What this is

One agent's **capability declaration** — "CAN this agent take this task":

- `agent_id` — the canonical `harness@model` identity
  (`AgentEntry::agent_id()` in `arbiter-core/src/catalog/mod.rs`), byte-equal
  to the keys of `agent_stats` and `benchmark_runs`. Bare legacy names
  (`aider`) remain valid.
- `supports_types` / `supports_languages` / `max_concurrent` — exactly what
  the hard filter consumes (`route_task.rs`, step 3): an agent missing the
  task's type or language, or out of slots, is silently not a candidate.

## What this deliberately is NOT

- **Not competence evidence.** Success rates (`agent_stats`) and benchmark
  scores (`benchmark_runs`, `report_benchmark` since R-06b) are the *evidence*
  tiers of capability; they share the `agent_id` key but have their own
  storage and lifecycle. This schema covers the declaration only.
- **Not authority.** Capability filtering is silent (a non-candidate is not a
  refusal); authority denial is a first-class audited outcome — see
  `contracts/authority/rationale.md`.
- **Not the catalog.** Enrollment/lifecycle (`tested`/`routable`, model
  status) live in the vendored agents-catalog; `config/agents.toml` holds the
  hand-authored policy fields this contract projects.

## Source of truth

`config/agents.toml` sections (`AgentConfig` in `arbiter-mcp/src/config.rs`)
keyed by agent id. The contract object is the projection
`{agent_id} ∪ {supports_types, supports_languages, max_concurrent}`.

## Contract tests

`arbiter-mcp/tests/promoted_contracts.rs` builds the projection from a live
`AgentConfig` and validates it against this schema, plus every fixture under
`fixtures/`.

## Consumers

- Maestro (routing calls; interprets a missing candidate as capability, a
  `metadata.authority.denied` entry as authority).
- proctor (future runtime admission — reads the same declaration shape).
