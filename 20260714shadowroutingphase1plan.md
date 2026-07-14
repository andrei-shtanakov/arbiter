# Shadow Routing Phase 1 — candidate-policy shadow evaluation + decision log + offline eval

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

> **Reconciled against live code 2026-07-14.** Key anchors verified: `ranked` pipeline and
> `apply_benchmark_rerank` wiring at `route_task.rs:401-424`; invariant cascade consumes
> `&ranked` from `route_task.rs:438`; `log_decision` at `route_task.rs:631` builds
> `DecisionRecord` (`db.rs:63`); `migrate()` at `db.rs:191` is v1-only
> (`SCHEMA_V1` + `INSERT OR IGNORE version 1`); the live tree is held as
> `Arc<RwLock<Option<DecisionTree>>>` (`server.rs:231`) and loaded in `main.rs:207`;
> the file watcher (`main.rs:302`) hot-reloads config + live tree only.

**Goal:** Every `route_task` call additionally evaluates a **candidate ("shadow") policy** —
an alternative tree and/or an alternative `ARBITER_BENCH_WEIGHT` — through the *same* ranking
pipeline (DT inference → preferred boost → bench re-rank), records the shadow's top-1 pick
next to the live decision in SQLite, and **never** influences the live route. An offline
script then reports agreement rate and joins disagreements with `outcomes`. This is the
counterfactual dataset that lets any future policy change (retrained tree, new bench weight,
TS-ranker) be measured against production traffic *before* it takes traffic — the
LiteLLM-style "traffic mirroring" pattern, at task granularity.

**Architecture:** Shadow is a read-only branch off the existing flow. After Step 6b
(`apply_benchmark_rerank`, `route_task.rs:424`) the live `ranked` is final (pre-invariants).
At that point we re-rank the *same* `feature_vectors` with the shadow tree (or the live tree
if only the weight differs), apply the same preferred boost, apply `apply_benchmark_rerank`
with the shadow weight, and serialize the shadow top-1 into a new nullable
`decisions.shadow_json` column. Invariants are NOT run for the shadow (they depend on the
live choice's side effects and would double-count state); the recorded live comparison point
is therefore the **pre-invariant top-1** (`ranked[0]`), stored explicitly in the JSON.

**Why not in the DTO (R3 carried over from R-07):** the Maestro DTO (`861534e`) is frozen.
Shadow data lives ONLY in SQLite (+ a `route.shadow` tracing line). `RouteResult` and the
MCP response are byte-for-byte unchanged — this is asserted by a test in Task 3.

**Failure isolation invariant (S1):** a shadow failure (bad tree file, DB read error,
poisoned lock) must NEVER fail or change the live route. Every shadow error degrades to
`shadow_json = NULL` + a `warn!`. Startup with an unloadable `--shadow-tree` warns and
disables shadow — it does not exit (mirrors the live tree's degraded-mode philosophy).

**Tech Stack:** Rust (`arbiter-mcp`: `db.rs`, `tools/route_task.rs`, `main.rs`, `server.rs`),
rusqlite migration v2, Python stdlib eval script, cargo test + pytest.

**Scope guard (NOT here):**
- NO hot reload of the shadow tree (watcher untouched; shadow tree loads once at startup — Phase 2).
- NO shadow fields in `get_metrics` (agreement rate is computed offline by the script — Phase 2 if the script proves insufficient).
- NO obs JSONL contract change (the obs log-schema contract tests stay untouched; shadow emits a plain `tracing` event, not a new contract event type).
- NO interleaving/canary (shadow never takes traffic; graduating a shadow policy to live is a separate decision, by hand, after data).
- NO changes to `apply_benchmark_rerank`, the DT format, `n_features`, or the `report_*` tools.

**What Phase 1 does and does not prove (honest gate):** agreement rate + disagreement
volume quantify the *blast radius* of switching policies on real traffic. Joining
disagreements with `outcomes` shows how the live agent fared *where the shadow would have
routed elsewhere* — a one-sided counterfactual (we never observe the shadow agent's outcome).
Directional quality claims still require benchmark evidence (R-07 track) or a later
interleave. Phase 1's deliverable is the measurement loop, not a verdict.

---

## File Structure

- `arbiter-mcp/src/db.rs` — migration v2 (`ALTER TABLE decisions ADD COLUMN shadow_json TEXT`), `DecisionRecord.shadow_json`, version-gated `migrate()`.
- `arbiter-mcp/src/tools/route_task.rs` — `ShadowPolicy` struct + `compute_shadow` helper + wiring after Step 6b + `log_decision` signature extension.
- `arbiter-mcp/src/main.rs` — `--shadow-tree <PATH>` CLI arg, shadow tree load (warn-and-disable on error), pass into `McpServer`.
- `arbiter-mcp/src/server.rs` — hold `shadow_tree: Arc<Option<DecisionTree>>`, thread into `route_task::execute`.
- `arbiter-mcp/tests/integration.rs` — shadow-vs-live integration tests + DTO-freeze assertion.
- `scripts/eval_shadow.py` — offline agreement/disagreement report.
- `tests/test_eval_shadow.py` — workspace-level pytest for the script.

---

## Task 1: migration v2 — nullable `shadow_json` on `decisions`

`migrate()` (`db.rs:191`) currently applies `SCHEMA_V1` and `INSERT OR IGNORE ... VALUES (1)`.
`ALTER TABLE ... ADD COLUMN` is NOT idempotent in SQLite, so v2 must be gated on the
recorded version, inside a transaction with its version bump.

**Files:**
- Modify: `arbiter-mcp/src/db.rs`

- [ ] **Step 1: Write the failing tests** (append to `db.rs` `#[cfg(test)] mod tests`)

```rust
#[test]
fn migrate_v2_adds_shadow_json_and_is_idempotent() {
    let db = setup_db(); // runs migrate() once
    db.migrate().unwrap(); // second run must not fail (ALTER is version-gated)

    let cols: Vec<String> = db
        .conn
        .prepare("SELECT name FROM pragma_table_info('decisions')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert!(cols.contains(&"shadow_json".to_string()));

    let version: i32 = db
        .conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 2);
}

#[test]
fn insert_decision_persists_shadow_json() {
    let db = setup_db();
    let mut d = sample_decision_record("t-shadow"); // reuse/extract the existing test builder
    d.shadow_json = Some(r#"{"agent":"aider","agrees_with_live":false}"#.to_string());
    let id = db.insert_decision(&d).unwrap();
    let stored: Option<String> = db
        .conn
        .query_row("SELECT shadow_json FROM decisions WHERE id = ?1", params![id], |r| r.get(0))
        .unwrap();
    assert!(stored.unwrap().contains("aider"));
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p arbiter-mcp migrate_v2` → FAIL (no column / no field).

- [ ] **Step 3: Implement**

1. Add `pub shadow_json: Option<String>` to `DecisionRecord` (`db.rs:63`).
2. Extend `insert_decision` (`db.rs:213`) column list + params with `shadow_json`.
3. In `migrate()`: after the v1 block, read `MAX(version)` from `schema_version`
   (COALESCE to 1); if `< 2`, run inside one transaction:
   `ALTER TABLE decisions ADD COLUMN shadow_json TEXT;` +
   `INSERT OR IGNORE INTO schema_version (version) VALUES (2);`.
4. Fix the two struct-literal construction sites in tests
   (`tests/integration.rs:176`, `tests/promoted_contracts.rs:105`) with
   `shadow_json: None` — grep `DecisionRecord {` for any others.

> Pre-existing on-disk v1 DBs take the ALTER on next startup; fresh DBs get v1+v2 in
> sequence. `data retention` purge and all existing readers are unaffected (column is
> nullable, never selected by them).

- [ ] **Step 4: Run** `cargo test -p arbiter-mcp` → PASS, no regressions.

- [ ] **Step 5: Commit**

```bash
git add arbiter-mcp/src/db.rs arbiter-mcp/tests/
git commit -m "feat(db): schema v2 — nullable decisions.shadow_json (shadow routing P1)"
```

---

## Task 2: shadow config plumbing — CLI flag, startup load, server state

**Files:**
- Modify: `arbiter-mcp/src/main.rs`, `arbiter-mcp/src/server.rs`

- [ ] **Step 1: Add `--shadow-tree <PATH>` to the arg parser** (next to `--tree`), default: none.

- [ ] **Step 2: Load the shadow tree in `main.rs`** (after the live tree block at `:207`),
  same `DecisionTree::from_json` path, but with **warn-and-disable** semantics (S1):

```rust
// Shadow tree (optional). Load failure disables shadow — never fatal (S1).
let shadow_tree: Arc<Option<DecisionTree>> = Arc::new(match &args.shadow_tree {
    None => None,
    Some(path) => match std::fs::read_to_string(path)
        .map_err(anyhow::Error::from)
        .and_then(|json| DecisionTree::from_json(&json).map_err(Into::into))
    {
        Ok(t) => {
            info!(event = "arbiter.shadow_tree_loaded", path = %path.display(), "shadow tree loaded");
            Some(t)
        }
        Err(e) => {
            eprintln!("WARNING: shadow tree failed to load: {e:#}. Shadow routing disabled.");
            None
        }
    },
});
```

> Plain `Arc<Option<..>>`, NOT `Arc<RwLock<..>>` — no hot reload in Phase 1 (scope guard),
> so no lock and no poisoning surface on the hot path.

- [ ] **Step 3: Thread it through `McpServer`** — add field + `new()` param (`server.rs:228/241`),
  pass `self.shadow_tree.as_ref().as_ref()` into `route_task::execute` at `server.rs:635`
  as a new `shadow_tree: Option<&DecisionTree>` argument (fn already carries
  `#[allow(clippy::too_many_arguments)]`, `route_task.rs:206`). Update every other
  `route_task::execute(` call site (integration tests) with `None`.

- [ ] **Step 4: Shadow bench weight via env**, mirroring the live pattern
  (`ARBITER_BENCH_WEIGHT`, `route_task.rs:420`): `ARBITER_SHADOW_BENCH_WEIGHT`, parsed the
  same way, **defaulting to the live weight** when unset. Reading happens inside
  `execute` in Task 3 — nothing to do here beyond documenting the contract:
  shadow is ACTIVE iff `shadow_tree.is_some() || env(ARBITER_SHADOW_BENCH_WEIGHT).is_some()`.
  When only the weight is set, the shadow uses the LIVE tree with the shadow weight.

- [ ] **Step 5: Run** `cargo test -p arbiter-mcp && cargo clippy --workspace -- -D warnings` → PASS.

- [ ] **Step 6: Commit**

```bash
git add arbiter-mcp/src/main.rs arbiter-mcp/src/server.rs arbiter-mcp/tests/
git commit -m "feat(mcp): --shadow-tree flag + shadow state plumbing (shadow routing P1)"
```

---

## Task 3: `compute_shadow` + wiring + DTO-freeze test

**Files:**
- Modify: `arbiter-mcp/src/tools/route_task.rs`, `arbiter-mcp/tests/integration.rs`

The live comparison point is `ranked[0]` **after** Step 6b (`route_task.rs:424`) and
**before** the invariant cascade (`route_task.rs:438`). The shadow replays the pipeline
on the already-built `feature_vectors` (`route_task.rs:380`) — no second feature build.

- [ ] **Step 1: Write the failing helper test** (append to `route_task.rs` tests; reuse
  `seed_bench` from the R-07 tests in this file)

```rust
#[test]
fn compute_shadow_reranks_with_shadow_weight_and_flags_disagreement() {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    seed_bench(&db, "s1", "claude_code", "code-review", 0.90);
    seed_bench(&db, "s2", "aider", "code-review", 0.20);

    let fvs: Vec<(String, [f64; 22])> = vec![
        ("aider".to_string(), [0.0; 22]),
        ("claude_code".to_string(), [0.0; 22]),
    ];
    // Live top-1 is aider; shadow (weight 0.15, live tree = None -> round-robin base)
    // must promote claude_code on its code-review score and flag the disagreement.
    let shadow = compute_shadow(
        None,                 // shadow tree (None -> fall back to live tree, here also None)
        None,                 // live tree
        &fvs,
        &make_candidates(),   // reuse/extract the existing candidates test builder
        &TaskType::Review,
        &None,                // preferred_agent
        &db,
        0.15,                 // shadow bench weight
        "aider",              // live pre-invariant top-1
    );
    let s = shadow.expect("shadow must produce a value");
    assert_eq!(s.agent, "claude_code");
    assert!(!s.agrees_with_live);
    assert_eq!(s.live_top1, "aider");
}
```

- [ ] **Step 2: Run to verify failure** — `compute_shadow` not found.

- [ ] **Step 3: Implement** (module scope, near `apply_benchmark_rerank`)

```rust
/// Shadow decision snapshot persisted into decisions.shadow_json. NOT part of the
/// frozen Maestro DTO (R3) — SQLite + tracing only.
#[derive(Debug, serde::Serialize)]
struct ShadowDecision {
    agent: String,
    confidence: f64,
    tree: &'static str,        // "shadow" | "live"
    bench_weight: f64,
    live_top1: String,
    agrees_with_live: bool,
}

/// Replay the ranking pipeline (DT -> preferred boost -> bench re-rank) under the
/// shadow policy. Returns None when shadow is inactive or on ANY internal error (S1):
/// shadow must never fail or alter the live route. Runs on the already-built
/// feature_vectors; invariants are deliberately NOT replayed (pre-invariant top-1).
#[allow(clippy::too_many_arguments)]
fn compute_shadow(
    shadow_tree: Option<&DecisionTree>,
    live_tree: Option<&DecisionTree>,
    feature_vectors: &[(String, [f64; 22])],
    candidates: &[AgentInfo],
    task_type: &TaskType,
    preferred_agent: &Option<String>,
    db: &Database,
    shadow_bench_weight: f64,
    live_top1: &str,
) -> Option<ShadowDecision> { /* mirror steps 5-6b; wrap fallible parts; warn! on error */ }
```

Implementation notes (confirm local names before editing):
1. Base ranking: `evaluate_for_agents(shadow_tree.or(live_tree)?, feature_vectors)`;
   when BOTH trees are `None` (degraded live mode), use `round_robin_ranking(candidates)`
   so shadow-weight-only experiments still work in degraded mode.
2. Preferred boost: copy the live block (`route_task.rs:402-416`) verbatim.
3. Bench re-rank: `apply_benchmark_rerank(&mut shadow_ranked, task_type, db, shadow_bench_weight)`
   — on `Err`, `warn!` and return `None` (S1: do NOT propagate with `?` to the caller).
4. Build `ShadowDecision` from `shadow_ranked.first()?`; `tree` = `"shadow"` iff a
   distinct shadow tree was used; `agrees_with_live = agent == live_top1`.

- [ ] **Step 4: Wire into `execute`** — insert between Step 6b (`:424`) and Step 7 (`:427`):

```rust
// Step 6c: shadow policy evaluation (read-only; never affects the live route).
let shadow_bench_weight = std::env::var("ARBITER_SHADOW_BENCH_WEIGHT")
    .ok()
    .and_then(|v| v.parse::<f64>().ok());
let shadow_active = shadow_tree.is_some() || shadow_bench_weight.is_some();
let shadow: Option<ShadowDecision> = if shadow_active {
    ranked.first().and_then(|(live_top1, _)| {
        compute_shadow(
            shadow_tree, tree, &feature_vectors, &candidates, &task.task_type,
            &constraints.preferred_agent, db,
            shadow_bench_weight.unwrap_or(bench_weight), live_top1,
        )
    })
} else {
    None
};
if let Some(ref s) = shadow {
    info!(event = "route.shadow", task_id, shadow_agent = %s.agent,
          agrees = s.agrees_with_live, "shadow decision");
}
```

Then extend `log_decision` (`route_task.rs:631`) with a `shadow: Option<&ShadowDecision>`
parameter serialized into `DecisionRecord.shadow_json`. The assign-path call site
(`:495`) passes `shadow.as_ref()`; the three earlier terminal reject paths (`:250`,
`:331`, `:364`) and the post-cascade reject path pass `None` (grep `log_decision(` —
5 sites total; shadow is only meaningful once ranking happened).

- [ ] **Step 5: Integration tests** (append to `arbiter-mcp/tests/integration.rs`)

1. `shadow_json_written_on_assign_and_null_without_shadow` — route twice (with/without
   `ARBITER_SHADOW_BENCH_WEIGHT` set); assert `shadow_json` NOT NULL / NULL respectively,
   and that the JSON parses with the six expected keys.
2. `shadow_never_changes_live_route_or_dto` — serialize the full `route_task` MCP
   response with shadow off and with shadow on (same seed data, weight chosen so the
   shadow DISAGREES): responses must be **byte-for-byte identical** (R3 + S1 in one
   assertion; mirrors the R-07 "weight 0.0 => identical" test style).
3. `shadow_error_degrades_to_null` — e.g. shadow weight set + a task_type with a mapped
   benchmark but a dropped `benchmark_runs` table (or a closed DB handle if simpler):
   route succeeds, `shadow_json` is NULL.

> Env vars in parallel tests: `std::env::set_var` bleeds across threads. Follow the
> existing R-07 A/B test's isolation approach in this file (serial guard or explicit
> weight parameter) — confirm before writing.

- [ ] **Step 6: Run** `cargo test -p arbiter-mcp && cargo clippy --workspace -- -D warnings && cargo fmt --all` → PASS.

- [ ] **Step 7: Commit**

```bash
git add arbiter-mcp/src/tools/route_task.rs arbiter-mcp/tests/integration.rs
git commit -m "feat(route): shadow policy evaluation logged to decisions.shadow_json (P1)"
```

---

## Task 4: offline eval script `scripts/eval_shadow.py`

**Files:**
- Create: `scripts/eval_shadow.py`, `tests/test_eval_shadow.py`

- [ ] **Step 1: Write the failing pytest** (`tests/test_eval_shadow.py`): build a temp
  SQLite DB with the v2 schema (reuse the pattern from existing workspace tests), insert
  6 decisions (4 with shadow: 3 agree / 1 disagree; 2 without) + outcomes for the
  disagreement (`status='failure'`), run `eval_shadow.report(db_path)`, assert:
  `coverage == 4/6`, `agreement_rate == 0.75`, the disagreement row carries
  `task_type`, `live_agent`, `shadow_agent`, `live_outcome == "failure"`.

- [ ] **Step 2: Implement** — stdlib only (`sqlite3`, `json`, `argparse`), consistent with
  `check_routable_gate.py` conventions. Output (stdout, text + `--json` flag):
  - coverage: decisions with `shadow_json` / total (window `--since`, default all);
  - agreement rate overall and per `task_type` (extracted from `task_json`);
  - disagreement table: task_id, task_type, live agent (+`action`), shadow agent,
    joined `outcomes.status` for the live agent;
  - one-sided-counterfactual caveat printed in the footer (this script measures blast
    radius, not shadow quality — see plan header).
  - `action != 'assign'` rows are reported separately (fallback distorts the live-top1
    comparison; the stored `live_top1` key keeps them analyzable).

- [ ] **Step 3: Run** `uv run pytest tests/test_eval_shadow.py` → PASS; `ruff format --check && ruff check` clean.

- [ ] **Step 4: Commit**

```bash
git add scripts/eval_shadow.py tests/test_eval_shadow.py
git commit -m "feat(scripts): eval_shadow.py — shadow/live agreement report (P1)"
```

---

## Task 5: end-to-end smoke + Definition of Done

- [ ] **Step 1: Self-shadow sanity run** — start `arbiter-mcp` with
  `--shadow-tree models/agent_policy_tree.json` (shadow == live), route ≥20 mixed tasks
  via the Python client, run `eval_shadow.py`: agreement MUST be 100%. Any disagreement
  = pipeline replay bug (this is the strongest correctness check in the plan).
- [ ] **Step 2: Divergent-shadow run** — same, but with `ARBITER_SHADOW_BENCH_WEIGHT=0.15`
  and seeded `benchmark_runs` (reuse R-07 seed helpers): eval shows ≥1 disagreement on
  Review tasks, 100% agreement elsewhere (scoping, mirrors R-07 R1).
- [ ] **Step 3: DoD checklist**
  - [ ] `cargo test` / `cargo clippy -D warnings` / `cargo fmt --check` green; `uv run pytest tests/ orchestrator/tests/` green
  - [ ] MCP response byte-identical with shadow on/off (Task 3 test)
  - [ ] v2 migration applies cleanly to an existing v1 `arbiter.db` (Task 1 test) and on-disk smoke
  - [ ] p99 route latency target (<5ms e2e) still met: `cargo run --release --bin arbiter-cli -- bench`
  - [ ] README: `--shadow-tree` + `ARBITER_SHADOW_BENCH_WEIGHT` documented (options table + a "Shadow routing" paragraph); CLAUDE.md tool list untouched (no new MCP tool)
- [ ] **Step 4: PR** — branch `feat/shadow-routing-p1`, `gh pr create`, iterate on Copilot review per repo git workflow. **No merge** (user merges).

---

## Phase 2 candidates (explicitly deferred)

Hot-reload of the shadow tree via `watcher.rs`; `shadow_match_rate` in `get_metrics`;
obs contract event for shadow decisions; multiple simultaneous shadows (vector of
candidate policies); interleaving/canary graduation of a winning shadow policy — each
only after Phase 1 data shows the loop is used.
