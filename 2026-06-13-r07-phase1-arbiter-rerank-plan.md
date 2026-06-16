# R-07 Phase 1 — arbiter reader + benchmark re-rank + A/B Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

> **Reconciled against live code 2026-06-13** (review pass). Fixes applied vs the
> first draft: helper now operates on the real `Vec<(String, PredictionResult)>`
> (not `Vec<(String, f64)>`); audit line is written into `prediction.path` (there is
> **no `result` struct in scope** at the re-rank site); all test seeds go through
> `insert_benchmark_run` / `BenchmarkRunInput` (raw partial `INSERT`s violate 6
> NOT NULL columns); Task 4 gate language softened (one benchmark proves the
> *mechanism*, not task-dependence — see D5 note).

**Goal:** Make benchmark scores influence routing — a **task_type-scoped** reader (`get_benchmark_score`) + a benchmark re-rank step in `route_task` keyed by `task.task_type → benchmark_id`, gated by `ARBITER_BENCH_WEIGHT`. Then a 0-vs-0.15 A/B that proves **the read→rerank loop moves the route from real per-agent benchmark data** on the one wired benchmark. **Task-dependence (crossover) is NOT provable from a single benchmark** — it is validated incrementally as benchmark #2+ land (see Task 4 / D5).

**Architecture:** `benchmark_runs` already exists (migration + `idx_benchmark_runs_agent_bench_ts` on `(agent_id, benchmark_id, ts DESC)`) and `report_benchmark` already INSERTs rows. This plan adds the *read* side: a scoped score lookup, a static `TaskType → benchmark_id` map (`Review → "code-review"`), and an additive re-rank adjustment centered on 0.5 so a 0.5 score is neutral and an empty table is a no-op. The eval data is produced by the separate atp-platform plan (3 spawners on code-review).

**Tech Stack:** Rust, rusqlite, the existing `arbiter-mcp` crate (`db.rs`, `tools/route_task.rs`), cargo test.

**Why task_type-scoped (R1):** an unscoped score would leak a code-review score into Docs/Bugfix routing — invalid. The reader MUST filter by `benchmark_id`, derived from `task.task_type`.

**Roadmap note (one benchmark is the start, not the end):** Phase 1 wires exactly one
mapping (`Review → "code-review"`) to prove the vertical slice end-to-end on real data.
Subsequent benchmarks (bugfix, refactor, test, …) are added by extending `benchmark_id_for`
and seeding their rows — no new mechanism. The **crossover validity test** (agent A wins
task_type X, agent B wins task_type Y → proves routing is task-dependent, not a global
ranking) becomes runnable the moment the **second** benchmark lands; it is specified in
Task 4 Step 4 as the deferred gate.

**Scope guard (NOT here):** EWMA/multi-run aggregation, tree retrain, CI smoke, `TaskType` enum changes, the `report_benchmark` contract, a second benchmark mapping. Just: reader + re-rank + A/B on the one wired benchmark.

---

## File Structure

- `arbiter-mcp/src/db.rs` — add `get_benchmark_score(agent_id, benchmark_id) -> Result<Option<f64>>`.
- `arbiter-mcp/src/tools/route_task.rs` — add `benchmark_id_for(task_type)` map + `apply_benchmark_rerank` helper + `ARBITER_BENCH_WEIGHT` read, wired after the preferred-agent boost.
- `arbiter-mcp/tests/integration.rs` — A/B test (weight 0.0 vs 0.15) + scoping test.

---

## Task 1: task_type-scoped benchmark reader

**Files:**
- Modify: `arbiter-mcp/src/db.rs` (near `count_benchmark_runs`, ~line 793)

- [ ] **Step 1: Write the failing test** (append to `db.rs`'s `#[cfg(test)] mod tests`)

`benchmark_runs` has 6 NOT NULL columns beyond the score; seed through the existing
`insert_benchmark_run` / `BenchmarkRunInput` (both in-module here) — a partial raw
`INSERT` fails the constraint, not the assertion.

```rust
#[test]
fn get_benchmark_score_returns_latest_scoped_by_benchmark() {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();

    // All three rows for the same agent; only benchmark_id / ts / score vary.
    let mk = |run_id: &'static str, bench: &'static str, score: f64, ts: &'static str| {
        BenchmarkRunInput {
            run_id,
            payload_version: "1.0.0",
            benchmark_id: bench,
            agent_id: "claude_code",
            ts,
            score,
            score_components: "{}",
            total_tokens: None,
            total_cost_usd: None,
            duration_seconds: 0.0,
            per_task: "[]",
            per_task_total_count: 0,
            per_task_truncated: 0,
        }
    };
    db.insert_benchmark_run(&mk("r1", "code-review", 0.40, "2026-06-10T00:00:00Z")).unwrap();
    db.insert_benchmark_run(&mk("r2", "code-review", 0.80, "2026-06-13T00:00:00Z")).unwrap();
    db.insert_benchmark_run(&mk("r3", "docs", 0.10, "2026-06-13T00:00:00Z")).unwrap();

    // latest ts wins within a benchmark
    assert_eq!(db.get_benchmark_score("claude_code", "code-review").unwrap(), Some(0.80));
    // scoping: a different benchmark_id must not leak
    assert_eq!(db.get_benchmark_score("claude_code", "docs").unwrap(), Some(0.10));
    // unknown agent -> None
    assert_eq!(db.get_benchmark_score("aider", "code-review").unwrap(), None);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p arbiter-mcp get_benchmark_score_returns_latest -- --nocapture`
Expected: FAIL — method `get_benchmark_score` not found.

- [ ] **Step 3: Implement the reader** (in `impl Database`, after `count_benchmark_runs`)

```rust
/// Latest benchmark score for an agent on a specific benchmark, clamped to
/// [0,1]. task_type-scoped via `benchmark_id` (R1): a score MUST NOT leak across
/// benchmarks. Uses idx_benchmark_runs_agent_bench_ts.
pub fn get_benchmark_score(
    &self,
    agent_id: &str,
    benchmark_id: &str,
) -> Result<Option<f64>> {
    let row = self
        .conn
        .query_row(
            "SELECT score FROM benchmark_runs \
             WHERE agent_id = ?1 AND benchmark_id = ?2 \
             ORDER BY ts DESC LIMIT 1",
            rusqlite::params![agent_id, benchmark_id],
            |r| r.get::<_, f64>(0),
        )
        .optional()
        .context("Failed to read benchmark score")?;
    Ok(row.map(|s| s.clamp(0.0, 1.0)))
}
```

> Confirmed against live code: the connection field is `self.conn` and
> `use rusqlite::OptionalExtension;` is already in scope in `db.rs` (other
> `.optional()` calls use it). `BenchmarkRunInput` is defined at `db.rs:115`,
> `insert_benchmark_run` at `db.rs:757`.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p arbiter-mcp get_benchmark_score_returns_latest`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add arbiter-mcp/src/db.rs
git commit -m "feat(db): task_type-scoped get_benchmark_score reader (R-07)"
```

---

## Task 2: benchmark re-rank step in route_task

**Files:**
- Modify: `arbiter-mcp/src/tools/route_task.rs`

The live ranking value is `ranked: Vec<(String, PredictionResult)>` (where
`PredictionResult { class, confidence, path }`, arbiter_core types.rs:246). The base
ranker is `evaluate_for_agents` / `round_robin_ranking`, followed by the existing
`preferred_agent` boost + re-sort (route_task.rs:275–290). The benchmark re-rank slots
in **right after** that boost and **before** the invariant cascade loop (route_task.rs:304).
There is **no `result` struct in scope** at that point — the audit line therefore goes
into `pred.path`, which the chosen agent copies into `decision_path` at route_task.rs:348.

- [ ] **Step 1: Add the task_type → benchmark_id map** (module scope, near `PREFERRED_AGENT_BOOST`)

```rust
/// Static map from routed task type to the benchmark whose scores inform it.
/// Phase 1: only Review is wired (→ "code-review"). Others return None → no
/// adjustment. Extend this map (+ seed rows) to add benchmarks — no other change.
fn benchmark_id_for(task_type: &TaskType) -> Option<&'static str> {
    match task_type {
        TaskType::Review => Some("code-review"),
        _ => None,
    }
}
```
> Ensure `use arbiter_core::types::{PredictionResult, TaskType};` (or the crate's
> existing alias) is in scope — `TaskType` is already used in this file.

- [ ] **Step 2: Write the failing helper test** (append to `route_task.rs` tests; add a local `seed_bench` helper that goes through `insert_benchmark_run`)

```rust
fn seed_bench(db: &Database, run_id: &str, agent: &str, bench: &str, score: f64) {
    db.insert_benchmark_run(&crate::db::BenchmarkRunInput {
        run_id,
        payload_version: "1.0.0",
        benchmark_id: bench,
        agent_id: agent,
        ts: "2026-06-13T00:00:00Z",
        score,
        score_components: "{}",
        total_tokens: None,
        total_cost_usd: None,
        duration_seconds: 0.0,
        per_task: "[]",
        per_task_total_count: 0,
        per_task_truncated: 0,
    })
    .unwrap();
}

#[test]
fn benchmark_weight_reranks_review_by_per_agent_score() {
    // Two agents that both support Review; aider leads on base confidence.
    // With weight=0 the base ranker decides; with weight=0.15 claude_code's
    // higher code-review score (0.90 vs 0.20) overtakes it.
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    seed_bench(&db, "a", "claude_code", "code-review", 0.90);
    seed_bench(&db, "b", "aider", "code-review", 0.20);

    let mk = |conf: f64| PredictionResult { class: 0, confidence: conf, path: vec![] };
    let base = || vec![
        ("aider".to_string(), mk(0.55)),
        ("claude_code".to_string(), mk(0.50)),
    ];

    let mut w0 = base();
    apply_benchmark_rerank(&mut w0, &TaskType::Review, &db, 0.0).unwrap();
    assert_eq!(w0[0].0, "aider", "weight 0 leaves base ranking");

    let mut w15 = base();
    apply_benchmark_rerank(&mut w15, &TaskType::Review, &db, 0.15).unwrap();
    // claude_code: 0.50 + (0.90-0.5)*0.15 = 0.56 ; aider: 0.55 + (0.20-0.5)*0.15 = 0.505
    assert_eq!(w15[0].0, "claude_code", "high code-review score promotes claude_code");
    // audit line landed on the adjusted prediction's path
    assert!(w15[0].1.path.iter().any(|s| s.starts_with("bench_adjust[claude_code]")));
}
```

- [ ] **Step 2b: Run it to verify it fails**

Run: `cargo test -p arbiter-mcp benchmark_weight_reranks_review`
Expected: FAIL — `apply_benchmark_rerank` not found.

- [ ] **Step 3: Implement the re-rank helper** (module scope)

```rust
/// Additively adjust each agent's prediction confidence by its benchmark score,
/// centered on 0.5 so a neutral score changes nothing: delta = (score - 0.5)*weight.
/// Appends a `bench_adjust[...]` audit line to that prediction's `path` (the chosen
/// agent later copies `path` into `decision_path` — there is no `result` here yet).
/// No-op when weight <= 0, the task type has no mapped benchmark, or the table is
/// empty (score absent → agent left untouched). Confidence stays clamped to [0,1].
fn apply_benchmark_rerank(
    ranked: &mut [(String, PredictionResult)],
    task_type: &TaskType,
    db: &Database,
    weight: f64,
) -> anyhow::Result<()> {
    if weight <= 0.0 {
        return Ok(());
    }
    let Some(bench) = benchmark_id_for(task_type) else {
        return Ok(());
    };
    for (agent_id, pred) in ranked.iter_mut() {
        if let Some(score) = db.get_benchmark_score(agent_id, bench)? {
            let delta = (score - 0.5) * weight;
            pred.confidence = (pred.confidence + delta).clamp(0.0, 1.0);
            pred.path.push(format!(
                "bench_adjust[{agent_id}]: {bench} score={score:.3} delta={delta:+.3}"
            ));
        }
    }
    ranked.sort_by(|a, b| {
        b.1.confidence
            .partial_cmp(&a.1.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(())
}
```
> Takes `&mut [...]` so it works on the existing `ranked` `Vec` by reference; the
> sort mirrors the preferred-boost re-sort already at route_task.rs:285–289.

- [ ] **Step 4: Wire it into the routing flow**

In `route_task`, immediately **after** the `preferred_agent` boost re-sort block
(route_task.rs ~:290) and **before** the invariant cascade loop (`for (agent_id, prediction) in &ranked`, ~:304):

```rust
// Step 6b: benchmark re-rank (R-07), opt-in via env flag, off by default.
let bench_weight = std::env::var("ARBITER_BENCH_WEIGHT")
    .ok()
    .and_then(|v| v.parse::<f64>().ok())
    .unwrap_or(0.0);
apply_benchmark_rerank(&mut ranked, &task.task_type, db, bench_weight)?;
```
> `ranked` is already declared `let mut ranked = ...` (route_task.rs:263), and `db`,
> `task` are in scope here. No separate `decision_path.push` — the audit line is
> carried by `pred.path` and surfaces via the chosen agent's `prediction.path.clone()`
> at route_task.rs:348. Confirm the local names before editing.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p arbiter-mcp benchmark_weight_reranks_review && cargo test -p arbiter-mcp`
Expected: PASS (new test + no regressions). Then `cargo clippy -p arbiter-mcp -- -D warnings` and `cargo fmt --all`.

- [ ] **Step 6: Commit**

```bash
git add arbiter-mcp/src/tools/route_task.rs
git commit -m "feat(route): benchmark re-rank for Review, gated by ARBITER_BENCH_WEIGHT (R-07)"
```

---

## Task 3: A/B + scoping integration test

**Files:**
- Modify: `arbiter-mcp/tests/integration.rs`

- [ ] **Step 1: Add an end-to-end test: weight changes the Review route; Docs is unaffected**

This proves two things the mechanism MUST guarantee: (a) real per-agent benchmark
data moves the chosen agent on the wired task_type; (b) **scoping** — a task_type with
no mapped benchmark is byte-for-byte unchanged (no leak, R1). It does **not** prove
task-dependence across task_types — that needs benchmark #2 (Task 4 Step 4).

```rust
#[test]
fn ab_benchmark_weight_shifts_review_route_and_scopes_docs() {
    let db = make_db_with_two_review_agents();   // reuse the registry+routing setup
    seed_benchmark(&db, "claude_code", "code-review", 0.90);
    seed_benchmark(&db, "aider", "code-review", 0.20);

    let a = route_review_task(&db, /*weight*/ 0.0);   // baseline
    let b = route_review_task(&db, /*weight*/ 0.15);  // bench-on

    assert_ne!(a.chosen_agent, b.chosen_agent, "weight must change the Review route");
    assert_eq!(b.chosen_agent, "claude_code");

    // scoping: a Docs task (no mapped benchmark) must be UNAFFECTED by the flag
    assert_eq!(
        route_docs_task(&db, 0.15).chosen_agent,
        route_docs_task(&db, 0.0).chosen_agent,
    );
}
```
> Implement `make_db_with_two_review_agents`, `seed_benchmark` (via `insert_benchmark_run`
> / `BenchmarkRunInput` — NOT a raw `INSERT`), and `route_review_task`/`route_docs_task`
> by reusing the agent-registry + routing harness already in `integration.rs` /
> `golden_tests.rs`. Read those first and copy their construction idiom — do not invent a
> new harness. The two Review agents must both list `review` in `supports_types` and share
> a language so they pass the Step-3 filter; pick base confidences close enough that the
> 0.90-vs-0.20 score gap flips the order (the 0.15 weight moves confidence by up to ±0.075).
> `ARBITER_BENCH_WEIGHT` is process-global env state — set/remove it inside each
> `route_*_task` helper (and avoid running these two assertions in parallel threads that
> share the env), or thread the weight in directly if the harness allows.

- [ ] **Step 2: Run it**

Run: `cargo test -p arbiter-mcp ab_benchmark_weight_shifts_review`
Expected: FAIL first (helpers absent), PASS once helpers reuse the existing setup and Task 2 is in.

- [ ] **Step 3: Commit**

```bash
git add arbiter-mcp/tests/integration.rs
git commit -m "test(route): A/B Review route-shift + Docs scoping under ARBITER_BENCH_WEIGHT (R-07)"
```

---

## Task 4: Manual A/B run + honest decision gate

- [ ] **Step 1:** With real eval data in `benchmark_runs` (from the atp-platform plan — 3 spawners scoring `claude_code` / `codex_cli` / `aider` on `code-review`), run routing requests with identical features, `task_type=Review`, candidate agents `{claude_code, codex_cli, aider}`.

- [ ] **Step 2:** Run A (`ARBITER_BENCH_WEIGHT=0.0`) and B (`0.15`). Record chosen agent + `decision_path` (the `bench_adjust[...]` lines) for each.

- [ ] **Step 3 — mechanism gate (what one benchmark CAN prove):**
  - **PASS (mechanism wired):** the Review route changes between A and B, the shift is
    explained by the per-agent `code-review` score gap in `decision_path`, and the gap is
    large enough to matter (note: with weight 0.15 a score gap < ~0.67 moves confidence by
    < `PREFERRED_AGENT_BOOST=0.1`, so report the actual gap and resulting delta, don't just
    assert a flip).
  - **INCONCLUSIVE:** scores cluster so tightly (cf. Phase 0: real ATP gaps were ~0.01) that
    no weight short of absurd moves the route. Then the *signal*, not the mechanism, is the
    problem — record the score distribution and stop; do not crank the weight to force a flip.
  - **Do NOT** declare "benchmark-aware routing validated/no-go" from this single benchmark.
    One benchmark cannot distinguish "task-dependent routing" from "promote the globally
    best-reviewing agent" (R1). This step validates plumbing + signal magnitude only.

- [ ] **Step 4 — crossover gate (deferred to benchmark #2, the real direction test):**
  When the second benchmark lands (extend `benchmark_id_for`, seed its rows), run the
  cross-task A/B: task_type X where agent A has the higher score, task_type Y where agent B
  does. **GO for the direction** iff B promotes A on X *and* B on Y (the route is
  task-dependent, not a global rank). **NO-GO** iff the same agent wins both regardless of
  task_type. This is the gate that actually answers "is benchmark-aware routing worth it";
  Phase 1's single benchmark only gets us to the starting line.

- [ ] **Step 5:** Write results to `_cowork_output/status/2026-06-13-r07-phase1-ab.md` — the
  A/B numbers, the per-agent score gaps, which gate (Step 3 mechanism / Step 4 crossover) was
  exercised, and the verdict scoped to what the available benchmarks can support.

---

## Self-review notes

- **Spec coverage:** thin-slice §3.1 reader → Task 1 (scoped, per R1); §3.2 re-rank (center 0.5, env flag, audit in `pred.path`) → Task 2; §5 A/B + scoping → Task 3; mechanism vs crossover gates → Task 4. The `report_benchmark` write side already exists (`insert_benchmark_run`, db.rs:757) — no work needed.
- **Live-code reconciliation (done, not deferred):** `ranked` is `Vec<(String, PredictionResult)>` → helper retyped + mutates `.confidence`; no `result` at the re-rank site → audit written to `pred.path`; `benchmark_runs` has 6 NOT NULL columns → seeds use `insert_benchmark_run` / `BenchmarkRunInput`; `Database::open_in_memory()` + `migrate()` confirmed (used across `integration.rs`).
- **Dependency:** Task 4 needs real `benchmark_runs` rows from the atp-platform plan. Tasks 1–3 are independently testable with seeded rows.
- **Honest-scope reminder:** one wired benchmark proves the loop, not the thesis. Crossover (Task 4 Step 4) is the direction gate and is intentionally deferred to benchmark #2, per the roadmap — it is specified here so it isn't lost.
- **Remaining read-and-reconcile (flagged):** the integration-test harness helpers (Task 3) and the exact local names at the wiring site (Task 2 Step 4) must be matched to live code — each carries an explicit "read first / confirm before editing" instruction.
```

