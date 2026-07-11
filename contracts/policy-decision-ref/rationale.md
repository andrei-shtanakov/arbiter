# PolicyDecisionRef v1 — rationale

Contracts-roadmap phase 2 (RD-002): **promote, don't build**. `decision_id`
has existed since R-03 as the decisions-table rowid, surfaced to callers in
the `route_task` response (`metadata.decision_id`) and referenced by the
`outcomes` table FK. This contract names the portable reference shape so
consumers can persist and exchange it.

## Provenance of `decision_id`

1. **Minted** by arbiter on every `route_task` call: SQLite rowid of the
   inserted `decisions` row (`log_decision` in
   `arbiter-mcp/src/tools/route_task.rs`). `null` in the response only if
   the SQLite insert failed — consumers must treat it as optional at the
   wire level, but a PolicyDecisionRef record requires it (no id → nothing
   to reference).
2. **Surfaced** at `metadata.decision_id` of the route_task response.
3. **Correlated**: `outcomes.decision_id` FK; `report_outcome` resolves the
   id from `task_id` when the caller does not pass one
   (`find_decision_id_by_task`).

## Relation to WorkCorrelation v1

`task_id` here is the same key as `WorkCorrelation.work_item_id`
(Maestro-minted, flows verbatim through `route_task`). A consumer holding a
PolicyDecisionRef can therefore join decisions to work-item chains without
any extra mapping.

**`action` is a decision vocabulary** (`assign | reject | fallback`), not a
work-item lifecycle status. It is deliberately NOT part of WorkCorrelation's
status projections — do not map it onto the common enum.

## What v1 deliberately is not

- Not the full decision record (feature vector, decision path, invariant
  checks stay arbiter-internal; consumers needing them read the response
  directly).
- Not an event/transport mandate: how a consumer obtains the ref (response
  metadata, DB read-side, future emitter) is out of scope.

## Contract tests

`arbiter-mcp/tests/promoted_contracts.rs` builds a ref from a **live
`route_task` response JSON** and validates it against this schema, plus
golden fixtures in `fixtures/` (assign and reject variants).

## Consumers

Maestro (retry gating on `report_outcome` delivery, decision correlation),
dispatcher (decision drill-down), steward/atp later per roadmap phases 4–6.
