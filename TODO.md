# TODO — arbiter (план от 2026-04-16, snapshot 2026-04-25)

> Роль в экосистеме: MCP policy engine / router. Интеграция с Maestro shipped — обе стороны на v0.2.0+
> Стратегический контекст: `../prograph-vault/authored/notes/ecosystem-roadmap.md`
> Актуальные статусы: `../prograph-vault/authored/notes/status/2026-04-24-status.md`, `../prograph-vault/authored/notes/status/2026-04-18-status.md`

## Текущее состояние
- ✅ R1–R4 собственного roadmap закрыты (p99 ≤5ms, typed errors, metrics, golden-tests)
- ✅ Typed DTOs + E2E smoke test для Maestro integration (`861534e`)
- ✅ MCP API: `route_task`, `report_outcome`, `get_agent_status`, `get_metrics`, `get_budget_status`, `report_benchmark` (6 tools)
- ✅ **CI** (GitHub Actions: Rust stable/beta + Python ruff/pytest, `fe4c033`)
- ✅ **Maestro R-01/R-02/R-03 закрыты на их стороне** (release v0.2.0, merged PR #13) — наш DTO-контракт `861534e` вендорнут ими без изменений
- ✅ **observability v1 (Rust side)** (commit `d1a8ecd`, 2026-04-25): новый `arbiter-core::obs` модуль — OTel Logs JSONL, structlog/ulid-py для Python orchestrator, 2 contract-теста (`emit_contract`, `fixtures_contract`); structured events в route_task/report_outcome/agent_status
- ✅ **arbiter#9 fixed** (commit `d1a8ecd`, 2026-04-25): `metadata.decision_id` (i64 SQLite rowid) теперь в response `route_task`. Maestro парная коммит `e5915f2`. **Разблокировало R-05 contract-level** (Maestro `f1f7d26` написал 4 subprocess-теста)
- ✅ **R-06b M4 closed** (2026-05-23, PRs #11/#13/#14/#15): 6-й MCP-tool `report_benchmark` для приёма BenchmarkResult от Maestro; новая таблица `benchmark_runs` (run_id PK + idx_benchmark_runs_agent_bench_ts) с `INSERT...ON CONFLICT DO NOTHING` идемпотентностью; `protocolVersion` поднят `"2024-11-05"` → `"1.1.0"` (semver-style, pragmatic deviation от canonical MCP date-string per Maestro design §6); workspace v0.2.0; `ReportBenchmarkError` enum разделяет validation (-32602) и runtime (-32000) ошибки; CI получил pyrefly gate + tag-triggered GitHub Release upload (закрыло issue #8 R-10).

## Правила ведения
- После каждой выполненной задачи проставь `[x]` и добавь хеш коммита
- **Контрактная заморозка**: DTO для Maestro (`861534e`) — заморожен. Любое изменение API, описанного в E2E smoke test, требует согласования с Maestro и bump версии MCP API

---

## Активные задачи

### ADR-ECO-003b: Rust-загрузчик user-config каталога — ✅ закрыт

- [x] `arbiter_core::catalog`: parse (3 плоскости, degrade-with-warning для
  незнакомых enum) + validate V1–V7 (V2+V3 зеркалят conformance Check 5) +
  `resolve_path` ($ATP_CATALOG → XDG `atp/`, fail-loud, без bundled-дефолта)
- [x] `arbiter-cli catalog path|check|list` — CLI-поверхность fail-loud
- [x] Дизайн: `docs/2026-07-05-catalog-loader-design.md` (XDG-namespace `atp/`
  зафиксирован — закрывает открытый пункт ADR-003b)
- Коммит: `6a966a0` (PR #39, squash; ветковый `25b0a94` в истории master не существует)

### ADR-ECO-003a: гейт routable-PR на benchmark-эвиденс — ✅ закрыт

- [x] `scripts/check_routable_gate.py`: `gate` (диффовые правила A/B,
  evidence-declaration) + `verify` (data-gate против `benchmark_runs`,
  runtime-семантика скора, mean по `run_ids`, eps 0.05)
- [x] CI-job `routable-gate` (pull_request, base.sha) + `pytest tests/`
  добавлен в CI python-job (закрыта pre-existing дыра: workspace-тесты не гонялись)
- [x] Дизайн: `docs/2026-07-05-routable-gate-design.md` (v3, два раунда ревью)
- Follow-ups (вне репы): конвенция `bench`-полей в SSOT-канон atp-platform;
  бэкфил трёх grandfathered-пар
- Коммит: `ab6f8b0` (код) — CI-коммит этого PR

### R-10: CI/CD (effort S) — ✅ закрыт

- [x] **GitHub Actions**: `cargo test` + `cargo clippy` + `ruff` (`.github/workflows/ci.yml`, `fe4c033`)
  - Rust matrix: stable, beta — fmt-check + clippy `-D warnings` + `cargo test --all-targets`
  - Python: `ruff format --check` + `ruff check` + `pytest orchestrator/tests/` через uv
  - Trigger: push/PR на `master`, `main`
  - Дополнительно: попутно вычищены 4 clippy-варнинга в тестовом коде (`cede423`)
  - Примечание: `pyrefly` из пункта плана не добавлен (не сконфигурирован в проекте) — при необходимости вынести в отдельную задачу
- [x] **arbiter-mcp binary как CI artifact** (`6efe792`, run `24568162844` green)
  - Job `release-binary` в `ci.yml`: cargo build --release → stage в `dist/` (binary + config/*.toml + models/agent_policy_tree.json) → tar.gz → upload через `actions/upload-artifact@v4`
  - Артефакт: `arbiter-mcp-linux-x64.tar.gz` (~2.26MB) + `build-info.txt` (commit/ref/rustc/timestamp)
  - Retention: 30 дней
  - Platform: linux-x64 + macos-arm64 (extended по запросу user'а); windows/macos-x64 — при необходимости
  - Trigger: `push` на master/main (не PR — экономим storage), после green rust+python
  - Для Maestro: распаковка в cwd совместима с их `ArbiterClientConfig` defaults (`target/release/arbiter-mcp` → в tarball лежит по пути `./arbiter-mcp`, Maestro может либо указать `binary_path` явно, либо мы можем добавить symlink-step если понадобится)

### R-13: Нормализация guardrails с ATP (effort M) — ✅ закрыт анализом

- [x] **Анализ overlap между invariants** → `docs/guardrails-atp-mapping.md`
  - Ключевой вывод: системы работают в **непересекающихся фазах** (arbiter = pre-dispatch, ATP = pre-evaluation). "Inspired by arbiter" — про паттерн, не про правила.
  - Маппинг: 0 правил идентичных, 2 пары с **инверсной** семантикой (`sla_feasible`↔`timeout_not_exceeded`, `budget_remaining`↔`within_budget`), 8 arbiter-only, 1 ATP-only.
  - Рекомендация: **не** извлекать shared-типы (overkill для 15 строк структур на двух языках с разным циклом релизов). Выровнять только описания/докстринги, не имена.
  - Follow-ups (вне arbiter-репы): (a) обновить докстринг `atp/evaluators/guardrails.py` чтобы подчеркнуть post-hoc фазу; (b) добавить ссылку на mapping в `_cowork_output/contracts/contract-analysis.md`. Оба — отдельные мелкие PR в ATP и заметки в shared output.

---

## Ждём от Maestro (НЕ делаем здесь, но отслеживаем)

- [x] **R-01**: rename `codex` → `codex_cli` — сделано Maestro в `8fd0b51`
- [x] **R-02**: `task_type`/`language`/`complexity` в TaskConfig — сделано Maestro в `8a3cba8`
- [x] **R-03**: MCP-клиент — сделано Maestro, ветка `feat/r-03-arbiter-client` merged в `166198a`, release v0.2.0 (`e4f0a9f`)
- [x] **R-05 contract-level** (2026-04-25): Maestro `f1f7d26` написал 4 subprocess-теста (`tests/test_arbiter_real_subprocess.py`); auto-skip без бинарника. **Pending в Maestro:** scheduler-driven e2e + CI-задача с предсобранным arbiter-mcp

**Текущий статус**: интеграция полностью shipped. Maestro вендорнул `arbiter_client.py` от commit `861534e`, наш DTO-контракт заморожен. arbiter#9 закрыт обе стороны (`d1a8ecd` + `e5915f2`). Любой bump API = coordinated release.

---

## НЕ делаем до стабилизации R-03

- ❌ Shared type library (R-14, XL) — сначала пусть Maestro встанет на наши DTO
- ❌ Дальнейшее расширение MCP API — зафиксировать то, что уже есть
- 🟢 **R-07 ECO-3 eval-driven routing — Phase 1 в работе** (план от 2026-06-13). Замыкает read-гэп: `benchmark_runs` писалась (`report_benchmark`/M4), но не читалась никем. Тонкий срез = пост-inference re-rank по бенчмарк-скору (зеркало `PREFERRED_AGENT_BOOST`), без ретрейна дерева/смены `n_features`/миграций.
  - **План (источник истины для кода):** `./2026-06-13-r07-phase1-arbiter-rerank-plan.md` — Tasks 1–4 (TDD, сверены с живым кодом).
  - **Мотивация + ревью:** `./2026-06-13-r07-thin-slice.md` (детали §3 помечены «↳ SUPERSEDED» → план).
  - **Фаза 0 (разведка данных):** `../prograph-vault/authored/notes/status/2026-06-13-r07-phase0-data-recon.md` — готовых ATP-данных нет (`.atp-dashboard.db` пуст, agent_id ≠ claude_code/codex_cli/aider, нет покрытия по task_type). **Путь 1 (seed из готовых результатов) мёртв.**
  - **Источник данных:** atp-platform пишет/гоняет тест-прогон 3 агентов на `code-review` → `report_benchmark` → реальные строки `benchmark_runs` (вход для plan Task 4).
  - **Декомпозиция:** Tasks 1–3 (scoped `get_benchmark_score` + `apply_benchmark_rerank` + A/B-тест со scoping'ом) — на сид-данных, **не ждут Maestro**. Task 4 (живой A/B + гейт) ждёт данные от atp-platform.
  - **Гейты:** один бенчмарк доказывает *механизм* (проводка + магнитуда сигнала), НЕ «направление валидировано»; **crossover-gate** (task-dependence, не global bias — ревью R1) отложен до бенчмарка №2 (plan Task 4 Step 4).
  - **Инварианты ревью, вшитые в план:** R1 task_type-scoped reader · R2 центр на наблюдаемом среднем скоров · R3 метрика в `decision_path`/obs, не в frozen DTO (`861534e`) · R4 audit в `pred.path` (на месте re-rank нет `result`).
