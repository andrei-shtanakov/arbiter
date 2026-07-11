# Budget v1 — rationale

Contracts-roadmap phase 2 (RD-002): **promote, don't build**. The
`get_budget_status` MCP tool has returned this shape since v0.1; this
contract canonizes it verbatim so consumers (Maestro, dispatcher, proctor)
can vendor a pinned copy instead of coupling to arbiter internals.

## Promoted as-is — including the warts

- **Monetary amounts are decimal strings** (`"10.00"`, exactly two fraction
  digits, produced by `format!("{:.2}")`). A float would be the cleaner
  design, but v1 canonizes the wire format that already exists; changing it
  would silently break every current caller. A future v2 may move to
  numbers — that is a new schema file, not an edit to this one.
- **`remaining_usd` can be negative** (`"-3.50"`) — that, plus
  `over_budget: true`, is how overrun is signalled.
- **No version field in the response.** The tool predates versioned
  contracts; adding a field would change the wire format this promotion is
  meant to freeze. The contract is versioned by schema file path.

## Source of truth

`arbiter-mcp/src/tools/get_budget.rs` (`execute`). Numbers come from the
`outcomes` table (`get_total_cost`, `get_cost_by_agent`); the limit comes
from `invariants.budget.threshold_usd` in `config/invariants.toml`.

## Contract tests

`arbiter-mcp/tests/promoted_contracts.rs` validates the **live tool
output** (empty DB, with spend, over budget) against this schema — not just
golden fixtures — so wire drift fails CI in this repo before any consumer
sees it. Golden fixtures live in `fixtures/`.

## Consumers

Vendor a pinned copy per ecosystem contract policy. Known read-side
consumers today: dispatcher (budget panel), Maestro (advisory budget HOLD).
