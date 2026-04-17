# TODO — arbiter (план от 2026-04-16)

> Роль в экосистеме: MCP policy engine / router. Сторона arbiter для R-03 **готова** (DTO + E2E smoke в `861534e`) — ждём Maestro.
> Стратегический контекст: `../_cowork_output/roadmap/ecosystem-roadmap.md`
> Актуальный статус: `../_cowork_output/status/2026-04-10-status.md`

## Текущее состояние
- ✅ R1–R4 собственного roadmap закрыты (p99 ≤5ms, typed errors, metrics, golden-tests)
- ✅ Typed DTOs + E2E smoke test для Maestro integration (`861534e`)
- ✅ MCP API расширен: `route_task`, `report_outcome`, `get_metrics`, `get_budget_status`
- ✅ **CI** (GitHub Actions: Rust stable/beta + Python ruff/pytest, `fe4c033`)
- ✅ **Maestro R-01/R-02/R-03 закрыты на их стороне** (release v0.2.0, merged PR #13) — наш DTO-контракт `861534e` вендорнут ими без изменений, интеграция технически разблокирована

## Правила ведения
- После каждой выполненной задачи проставь `[x]` и добавь хеш коммита
- **Контрактная заморозка**: DTO для Maestro (`861534e`) — заморожен. Любое изменение API, описанного в E2E smoke test, требует согласования с Maestro и bump версии MCP API

---

## Активные задачи

### R-10: CI/CD (effort S) — частично закрыт

- [x] **GitHub Actions**: `cargo test` + `cargo clippy` + `ruff` (`.github/workflows/ci.yml`, `fe4c033`)
  - Rust matrix: stable, beta — fmt-check + clippy `-D warnings` + `cargo test --all-targets`
  - Python: `ruff format --check` + `ruff check` + `pytest orchestrator/tests/` через uv
  - Trigger: push/PR на `master`, `main`
  - Дополнительно: попутно вычищены 4 clippy-варнинга в тестовом коде (`cede423`)
  - Примечание: `pyrefly` из пункта плана не добавлен (не сконфигурирован в проекте) — при необходимости вынести в отдельную задачу
- [ ] **arbiter-mcp binary как CI artifact** — приоритет #1
  - Требование из Maestro TODO.md (follow-up R-10): Maestro нужен готовый бинарь для R-05 (интеграционные тесты с реальным subprocess) и pending manual acceptance tests (authoritative mode + kill arbiter)
  - План: добавить job `release-binary` в `ci.yml` — `cargo build --release --bin arbiter-mcp` + упаковать с `config/` (agents.toml + invariants.toml) и `models/agent_policy_tree.json` в `arbiter-mcp-linux-x64.tar.gz`, upload через `actions/upload-artifact@v4`, retention 30 дней
  - Платформа: только linux-x64 на старте (macOS/windows при запросе от Maestro)
  - Trigger: только `push` на master/main (не PR — не засорять storage), после прохождения rust+python jobs

### R-13: Нормализация guardrails с ATP (effort M) — приоритет #2

- [ ] **Анализ overlap между invariants**
  - arbiter: 10 invariants в `arbiter-core/src/invariant/`
  - ATP: 3 правила в `../atp-platform/atp/evaluators/guardrails.py` ("inspired by arbiter")
  - Задокументировать семантический маппинг: какие правила одинаковые, какие расходятся
  - Решить: извлечь shared-типы (Rust JSON Schema → ATP Python) или выровнять naming

---

## Ждём от Maestro (НЕ делаем здесь, но отслеживаем)

- [x] **R-01**: rename `codex` → `codex_cli` — сделано Maestro в `8fd0b51`
- [x] **R-02**: `task_type`/`language`/`complexity` в TaskConfig — сделано Maestro в `8a3cba8`
- [x] **R-03**: MCP-клиент — сделано Maestro, ветка `feat/r-03-arbiter-client` merged в `166198a`, release v0.2.0 (`e4f0a9f`)
- [ ] **R-05**: интеграционные тесты с реальным subprocess — заблокировано нашим R-10 artifact-сабтаском (см. выше)

**Текущий статус**: интеграция технически разблокирована. Maestro вендорнул `arbiter_client.py` от commit `861534e`, наш DTO-контракт заморожен. Любой bump API = coordinated release.

---

## НЕ делаем до стабилизации R-03

- ❌ Shared type library (R-14, XL) — сначала пусть Maestro встанет на наши DTO
- ❌ Дальнейшее расширение MCP API — зафиксировать то, что уже есть
- ❌ ECO-3 eval-driven routing (R-07) — зависит от R-06b (ATP SDK в Maestro)
