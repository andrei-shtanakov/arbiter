# Shadow Routing Phase 1 — candidate-policy shadow evaluation + decision log + offline eval

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

> **Reconciled against live code 2026-07-14** (+ review pass R1 same day). Key anchors
> verified: `ranked` pipeline and `apply_benchmark_rerank` wiring at
> `route_task.rs:401-424`; invariant cascade consumes `&ranked` from `route_task.rs:438`;
> `log_decision` at `route_task.rs:631` builds `DecisionRecord` (`db.rs:63`) — 5 call
> sites (`:250/:331/:364/:495/:568`); `migrate()` at `db.rs:191` is v1-only
> (`SCHEMA_V1` + `INSERT OR IGNORE version 1`); the live tree is held as
> `Arc<RwLock<Option<DecisionTree>>>` (`server.rs:231`) and loaded in `main.rs:207`;
> the file watcher (`main.rs:302`) hot-reloads config + live tree only.
>
> **Review-pass fixes baked in (do not regress):**
> - **RR1 (S1):** `round_robin_ranking` mutates the global `ROUND_ROBIN_COUNTER`
>   (`fetch_add`, `route_task.rs:595`). The shadow must therefore NEVER call it — when no
>   distinct shadow tree is set, the shadow base is a **clone of the live post-boost
>   ranking** (`PredictionResult` derives `Clone`, `arbiter-core/types.rs:252`). This also
>   removes the duplicate DT inference in the common "same tree, different weight" case.
> - **RR2:** the full MCP response can never be byte-for-byte equal across calls —
>   `inference_us` is a live timer (`route_task.rs:449`) and `decision_id` is an
>   autoincrement rowid. The DTO-freeze test compares JSON **after stripping those two
>   volatile fields** (and only those two).
> - **RR3:** `Database.conn` is private — integration tests read `shadow_json` via the
>   `get_decision_shadow_json` helper added in Task 1.

**Goal:** Every `route_task` call additionally evaluates a **candidate ("shadow") policy** —
an alternative tree and/or an alternative `ARBITER_BENCH_WEIGHT` — through the *same* ranking
pipeline (DT inference → preferred boost → bench re-rank), records the shadow's top-1 pick
next to the live decision in SQLite, and **never** influences the live route. An offline
script then reports agreement rate and joins disagreements with `outcomes`. This is the
counterfactual dataset that lets any future policy change (retrained tree, new bench weight,
TS-ranker) be measured against production traffic *before* it takes traffic — the
LiteLLM-style "traffic mirroring" pattern, at task granularity.

**Architecture:** Shadow is a read-only branch off the existing flow. The shadow base is
captured/computed **after Step 6 (preferred boost) and before Step 6b** (`route_task.rs:416/418`):
when a distinct shadow tree is set, the shadow re-ranks the *same* `feature_vectors` with it
and re-applies the boost; when only the weight differs, the shadow base is a **clone of the
live post-boost `ranked`** (RR1 — never re-run the base ranker: in degraded mode it would
advance the global round-robin counter and change the NEXT live route). Then
`apply_benchmark_rerank` runs on the shadow base with the shadow weight, and the shadow
top-1 is serialized into a new nullable `decisions.shadow_json` column. Invariants are NOT
run for the shadow (they depend on the live choice's side effects and would double-count
state); the recorded live comparison point is therefore the **pre-invariant top-1**
(`ranked[0]`), stored explicitly in the JSON — implementers MUST NOT "improve" this to
compare against `chosen_agent` (fallback rows would poison the agreement metric; the eval
script separates them by `action` instead).

**Why not in the DTO (R3 carried over from R-07):** the Maestro DTO (`861534e`) is frozen.
Shadow data lives ONLY in SQLite (+ a `route.shadow` tracing line). `RouteResult` keeps the
same response contract; Task 3 asserts the DTO-freeze by comparing normalized JSON after
stripping only `inference_us` and `decision_id`.

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

- [x] **Step 1: Write the failing tests** (append to `db.rs` `#[cfg(test)] mod tests`)

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

- [x] **Step 2: Run to verify failure** — `cargo test -p arbiter-mcp migrate_v2` → FAIL (no column / no field).

- [x] **Step 3: Implement**

1. Add `pub shadow_json: Option<String>` to `DecisionRecord` (`db.rs:63`).
2. Extend `insert_decision` (`db.rs:213`) column list + params with `shadow_json`.
3. In `migrate()`: after the v1 block, read `MAX(version)` from `schema_version`
   (COALESCE to 1); if `< 2`, run inside one transaction:
   `ALTER TABLE decisions ADD COLUMN shadow_json TEXT;` +
   `INSERT OR IGNORE INTO schema_version (version) VALUES (2);`.
4. Fix the two struct-literal construction sites in tests
   (`tests/integration.rs:176`, `tests/promoted_contracts.rs:105`) with
   `shadow_json: None` — grep `DecisionRecord {` for any others.
5. **RR3 — public read helper** (integration tests cannot touch the private
   `self.conn`; this is also what `eval_shadow.py` mirrors in SQL):

```rust
/// Latest decision's shadow_json for a task (test/eval surface; NULL when the
/// shadow was inactive or degraded). Ordered by rowid: same-task decisions can
/// share a datetime('now') timestamp.
pub fn get_decision_shadow_json(&self, task_id: &str) -> Result<Option<String>> {
    self.conn
        .query_row(
            "SELECT shadow_json FROM decisions WHERE task_id = ?1 \
             ORDER BY id DESC LIMIT 1",
            params![task_id],
            |r| r.get(0),
        )
        .optional()
        .context("Failed to read shadow_json")
        .map(Option::flatten)
}
```

   Add a unit test next to the Step 1 tests: insert with `Some(...)` → helper returns
   it; insert with `None` → helper returns `None`; unknown task_id → `None`.

> Pre-existing on-disk v1 DBs take the ALTER on next startup; fresh DBs get v1+v2 in
> sequence. `data retention` purge and all existing readers are unaffected (column is
> nullable, never selected by them).

- [x] **Step 4: Run** `cargo test -p arbiter-mcp` → PASS, no regressions.

- [x] **Step 5: Commit**

```bash
git add arbiter-mcp/src/db.rs arbiter-mcp/tests/
git commit -m "feat(db): schema v2 — nullable decisions.shadow_json (shadow routing P1)"
```

---

## Task 2: shadow config plumbing — CLI flag, startup load, server state

**Files:**
- Modify: `arbiter-mcp/src/main.rs`, `arbiter-mcp/src/server.rs`

- [x] **Step 1: Add `--shadow-tree <PATH>` to the arg parser** (next to `--tree`), default: none.

- [x] **Step 2: Load the shadow tree in `main.rs`** (after the live tree block at `:207`),
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

- [x] **Step 3: Thread it through `McpServer`** — add field + `new()` param (`server.rs:228/241`),
  pass `self.shadow_tree.as_ref().as_ref()` into `route_task::execute` at `server.rs:635`
  as a new `shadow_tree: Option<&DecisionTree>` argument (fn already carries
  `#[allow(clippy::too_many_arguments)]`, `route_task.rs:206`). Update every other
  `route_task::execute(` call site (integration tests) with `None`.

- [x] **Step 4: Shadow bench weight via env**, mirroring the live pattern
  (`ARBITER_BENCH_WEIGHT`, `route_task.rs:420`): `ARBITER_SHADOW_BENCH_WEIGHT`, parsed the
  same way, **defaulting to the live weight** when unset. Reading happens inside
  `execute` in Task 3 (env is read ONLY there; `compute_shadow` takes the weight as a
  plain parameter — keeps unit tests env-free, RR-env note below) — nothing else to do
  here beyond documenting the contract:
  - shadow is ACTIVE iff `shadow_tree.is_some() || env(ARBITER_SHADOW_BENCH_WEIGHT).is_some()`;
  - weight-only mode: shadow = live post-boost ranking (cloned) + shadow weight;
  - **tree-only mode with live weight 0.0 is INTENTIONAL**: the shadow re-rank is then a
    no-op (`apply_benchmark_rerank` early-returns at `weight <= 0.0`) and the run is a
    *pure tree-vs-tree comparison* — document this in the README paragraph so it is not
    mistaken for a bug during debugging.

- [x] **Step 5: Run** `cargo test -p arbiter-mcp && cargo clippy --workspace -- -D warnings` → PASS.

- [x] **Step 6: Commit**

```bash
git add arbiter-mcp/src/main.rs arbiter-mcp/src/server.rs arbiter-mcp/tests/
git commit -m "feat(mcp): --shadow-tree flag + shadow state plumbing (shadow routing P1)"
```

---

## Task 3: `compute_shadow` + wiring + DTO-freeze test

**Files:**
- Modify: `arbiter-mcp/src/tools/route_task.rs`, `arbiter-mcp/tests/integration.rs`

The live comparison point is `ranked[0]` **after** Step 6b (`route_task.rs:424`) and
**before** the invariant cascade (`route_task.rs:438`) — NOT `chosen_agent` (see the
Architecture note). The shadow replays only what differs: with a distinct shadow tree it
re-ranks the already-built `feature_vectors` (`route_task.rs:380`, no second feature
build); with weight-only shadow it starts from a **clone of the live post-boost ranking**.
**RR1 hard rule: `compute_shadow` must not call `round_robin_ranking`** — that function
advances the global `ROUND_ROBIN_COUNTER` (`route_task.rs:595`) and a shadow call would
shift the next live degraded-mode route (S1 violation). The clone-base design makes the
degraded case work for free.

- [x] **Step 1: Write the failing helper test** (append to `route_task.rs` tests; reuse
  `seed_bench` from the R-07 tests in this file)

```rust
#[test]
fn compute_shadow_reranks_cloned_base_and_flags_disagreement() {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    seed_bench(&db, "s1", "claude_code", "code-review", 0.90);
    seed_bench(&db, "s2", "aider", "code-review", 0.20);

    let mk = |conf: f64| PredictionResult { class: 0, confidence: conf, path: vec![] };
    // Weight-only shadow: base = clone of the live post-boost ranking (aider leads).
    let live_post_boost = vec![
        ("aider".to_string(), mk(0.55)),
        ("claude_code".to_string(), mk(0.50)),
    ];
    let shadow = compute_shadow(
        ShadowBase::ClonedLive(live_post_boost.clone()),
        &TaskType::Review,
        &db,
        0.15,     // shadow bench weight
        "aider",  // live pre-invariant top-1
    );
    let s = shadow.expect("shadow must produce a value");
    // claude_code: 0.50 + (0.90-0.5)*0.15 = 0.56 ; aider: 0.55 + (0.20-0.5)*0.15 = 0.505
    assert_eq!(s.agent, "claude_code");
    assert!(!s.agrees_with_live);
    assert_eq!(s.live_top1, "aider");
    assert_eq!(s.tree, "live");
}
```

- [x] **Step 2: Run to verify failure** — `compute_shadow` / `ShadowBase` not found.

- [x] **Step 3: Implement** (module scope, near `apply_benchmark_rerank`)

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

/// Base ranking the shadow re-ranks. Never produced by round_robin_ranking (RR1:
/// global counter side effect). PredictionResult is Clone (types.rs:252).
enum ShadowBase {
    /// Distinct shadow tree: fresh inference over the live feature_vectors,
    /// preferred boost re-applied by the caller-side constructor (Step 4).
    ShadowTree(Vec<(String, PredictionResult)>),
    /// Weight-only shadow (or degraded live mode): clone of the live post-boost
    /// ranking, taken BEFORE the live Step 6b mutates it in place.
    ClonedLive(Vec<(String, PredictionResult)>),
}

/// Re-rank the shadow base with the shadow bench weight. Returns None on ANY
/// internal error (S1): shadow must never fail or alter the live route.
/// Invariants are deliberately NOT replayed (pre-invariant top-1 comparison).
fn compute_shadow(
    base: ShadowBase,
    task_type: &TaskType,
    db: &Database,
    shadow_bench_weight: f64,
    live_top1: &str,
) -> Option<ShadowDecision> { /* rerank; wrap fallible parts; warn! on error */ }
```

Implementation notes (confirm local names before editing):
1. Unpack `base` into `(tree_label, mut shadow_ranked)` — `"shadow"` / `"live"`.
2. Bench re-rank: `apply_benchmark_rerank(&mut shadow_ranked, task_type, db, shadow_bench_weight)`
   — on `Err`, `warn!` and return `None` (S1: do NOT propagate with `?` to the caller).
3. Build `ShadowDecision` from `shadow_ranked.first()?`;
   `agrees_with_live = agent == live_top1`.
4. Constructing `ShadowBase::ShadowTree` happens at the call site (Step 4):
   `evaluate_for_agents(shadow_dt, &feature_vectors)` + the preferred boost block copied
   verbatim from `route_task.rs:402-416`. Extract that boost into a small
   `apply_preferred_boost(&mut ranked, &constraints.preferred_agent)` helper and call it
   from BOTH the live path and the shadow constructor, so the two cannot drift.

- [x] **Step 4: Wire into `execute`** — TWO insertion points, both around Step 6b:

(a) **Before** Step 6b (between `:416` and `:418`), capture the shadow base — the live
Step 6b mutates `ranked` in place, so the clone must happen first:

```rust
// Step 6b-pre: capture the shadow base (RR1: never via round_robin_ranking).
let shadow_bench_weight = std::env::var("ARBITER_SHADOW_BENCH_WEIGHT")
    .ok()
    .and_then(|v| v.parse::<f64>().ok());
let shadow_active = shadow_tree.is_some() || shadow_bench_weight.is_some();
let shadow_base: Option<ShadowBase> = match (shadow_active, shadow_tree) {
    (false, _) => None,
    (true, Some(sdt)) => {
        let mut sr = evaluate_for_agents(sdt, &feature_vectors);
        apply_preferred_boost(&mut sr, &constraints.preferred_agent);
        Some(ShadowBase::ShadowTree(sr))
    }
    (true, None) => Some(ShadowBase::ClonedLive(ranked.clone())),
};
```

(b) **After** Step 6b (between `:424` and `:426`), compute + log:

```rust
// Step 6c: shadow policy evaluation (read-only; never affects the live route).
let shadow: Option<ShadowDecision> = match (shadow_base, ranked.first()) {
    (Some(base), Some((live_top1, _))) => compute_shadow(
        base, &task.task_type, db,
        shadow_bench_weight.unwrap_or(bench_weight), live_top1,
    ),
    _ => None,
};
if let Some(ref s) = shadow {
    info!(event = "route.shadow", task_id, shadow_agent = %s.agent,
          agrees = s.agrees_with_live, "shadow decision");
}
```

> Env is read ONLY here (mirrors the `ARBITER_BENCH_WEIGHT` read at `:420-423`);
> `compute_shadow` and the unit tests stay env-free.

Then extend `log_decision` (`route_task.rs:631`) with a `shadow: Option<&ShadowDecision>`
parameter serialized into `DecisionRecord.shadow_json`. The assign-path call site
(`:495`) passes `shadow.as_ref()`; the three earlier terminal reject paths (`:250`,
`:331`, `:364`) and the post-cascade reject path pass `None` (grep `log_decision(` —
5 sites total; shadow is only meaningful once ranking happened).

- [x] **Step 5: Integration tests** (append to `arbiter-mcp/tests/integration.rs`;
  read `shadow_json` via `db.get_decision_shadow_json(task_id)` from Task 1 — RR3)

1. `shadow_json_written_on_assign_and_null_without_shadow` — route twice (with/without
   `ARBITER_SHADOW_BENCH_WEIGHT` set); assert shadow_json NOT NULL / NULL respectively,
   and that the JSON parses with the six expected keys.
2. `shadow_never_changes_live_route_or_dto` — serialize the full `route_task` MCP
   response with shadow off and with shadow on (same seed data, weight chosen so the
   shadow DISAGREES): responses must be **identical after stripping exactly two volatile
   fields** — `inference_us` (live timer, `route_task.rs:449`) and the `decision_id`
   metadata (autoincrement rowid) (RR2; a naive byte-for-byte compare can NEVER pass).
   Normalize by parsing both to `serde_json::Value`, removing those two keys, and
   asserting equality — everything else, including `reasoning` and `decision_path`,
   must match exactly (R3 + S1 in one assertion).
3. `shadow_error_degrades_to_null` — e.g. shadow weight set + a task_type with a mapped
   benchmark but a dropped `benchmark_runs` table (or a closed DB handle if simpler):
   route succeeds, `shadow_json` is NULL.
4. `shadow_does_not_advance_round_robin_in_degraded_mode` — the RR1 regression test:
   live tree = None (degraded round-robin), shadow weight set. Route the same task
   twice with shadow ON and, in a control run, twice with shadow OFF: the SEQUENCE of
   live `chosen_agent`s must be identical in both runs (a shadow-side counter increment
   would shift the second pick). This is the test the original design would have failed.
   **RR4 — config MUST have exactly 2 candidate agents:** the global
   `ROUND_ROBIN_COUNTER` is never reset between runs, so the control run starts 2
   ahead of the first and the sequences only align when `2 % n == 0` — with 3
   candidates the test false-fails on correct code. Keep this test under the same
   serial guard as the env tests: the counter is shared by the whole test binary,
   and any parallel degraded-mode route would shift it mid-sequence.

> Env vars in parallel tests: `std::env::set_var` bleeds across threads (precedent:
> set/remove pair at `integration.rs:279` for `ARBITER_BENCH_WEIGHT`). Only these e2e
> tests touch env — follow that same set/remove discipline and keep them serial
> (`--test-threads=1` guard or a shared mutex, whichever the R-07 test already uses);
> all unit tests take the weight as a parameter and stay env-free.

- [x] **Step 6: Run** `cargo test -p arbiter-mcp && cargo clippy --workspace -- -D warnings && cargo fmt --all` → PASS.

- [x] **Step 7: Commit**

```bash
git add arbiter-mcp/src/tools/route_task.rs arbiter-mcp/tests/integration.rs
git commit -m "feat(route): shadow policy evaluation logged to decisions.shadow_json (P1)"
```

---

## Task 4: offline eval script `scripts/eval_shadow.py`

**Files:**
- Create: `scripts/eval_shadow.py`, `tests/test_eval_shadow.py`

- [x] **Step 1: Write the failing pytest** (`tests/test_eval_shadow.py`): build a temp
  SQLite DB with the v2 schema (reuse the pattern from existing workspace tests), insert
  6 decisions (4 with shadow: 3 agree / 1 disagree; 2 without) + outcomes for the
  disagreement (`status='failure'`), run `eval_shadow.report(db_path)`, assert:
  `coverage == 4/6`, `agreement_rate == 0.75`, the disagreement row carries
  `task_type`, `live_agent`, `shadow_agent`, `live_outcome == "failure"`.

- [x] **Step 2: Implement** — stdlib only (`sqlite3`, `json`, `argparse`), consistent with
  `check_routable_gate.py` conventions. Output (stdout, text + `--json` flag):
  - coverage: decisions with `shadow_json` / total (window `--since`, default all);
  - agreement rate overall and per `task_type` (extracted from `task_json`);
  - disagreement table: task_id, task_type, live agent (+`action`), shadow agent,
    joined `outcomes.status` for the live agent;
  - one-sided-counterfactual caveat printed in the footer (this script measures blast
    radius, not shadow quality — see plan header).
  - `action != 'assign'` rows are reported separately (fallback distorts the live-top1
    comparison; the stored `live_top1` key keeps them analyzable).

- [x] **Step 3: Run** `uv run pytest tests/test_eval_shadow.py` → PASS; `ruff format --check && ruff check` clean.

- [x] **Step 4: Commit**

```bash
git add scripts/eval_shadow.py tests/test_eval_shadow.py
git commit -m "feat(scripts): eval_shadow.py — shadow/live agreement report (P1)"
```

---

## Task 5: end-to-end smoke + Definition of Done

- [x] **Step 1: Self-shadow sanity run** — start `arbiter-mcp` with
  `--shadow-tree models/agent_policy_tree.json` (shadow == live), route ≥20 mixed tasks
  via the Python client, run `eval_shadow.py`: agreement MUST be 100%. Any disagreement
  = pipeline replay bug (this is the strongest correctness check in the plan).
- [x] **Step 2: Divergent-shadow run** — same, but with `ARBITER_SHADOW_BENCH_WEIGHT=0.15`
  and seeded `benchmark_runs` (reuse R-07 seed helpers): eval shows ≥1 disagreement on
  Review tasks, 100% agreement elsewhere (scoping, mirrors R-07 R1).
- [x] **Step 3: DoD checklist**
  - [x] `cargo test` / `cargo clippy -D warnings` / `cargo fmt --check` green; `uv run pytest tests/ orchestrator/tests/` green
  - [x] MCP response identical with shadow on/off modulo `inference_us` + `decision_id` (Task 3 test 2, RR2)
  - [x] Degraded-mode round-robin sequence unaffected by shadow (Task 3 test 4, RR1)
  - [x] v2 migration applies cleanly to an existing v1 `arbiter.db` (Task 1 test) and on-disk smoke
  - [x] p99 route latency target (<5ms e2e) still met: `cargo run --release --bin arbiter-cli -- bench`
  - [x] README: `--shadow-tree` + `ARBITER_SHADOW_BENCH_WEIGHT` documented (options table + a "Shadow routing" paragraph); CLAUDE.md tool list untouched (no new MCP tool)
- [x] **Step 4: PR** — branch `feat/shadow-routing-p1`, `gh pr create`, iterate on Copilot review per repo git workflow. **No merge** (user merges).

---

## Phase 2 candidates (explicitly deferred)

Hot-reload of the shadow tree via `watcher.rs`; `shadow_match_rate` in `get_metrics`;
obs contract event for shadow decisions; multiple simultaneous shadows (vector of
candidate policies); interleaving/canary graduation of a winning shadow policy — each
only after Phase 1 data shows the loop is used.
