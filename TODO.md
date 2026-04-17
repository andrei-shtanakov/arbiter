# TODO — arbiter (план от 2026-04-16)

> Роль в экосистеме: MCP policy engine / router. Сторона arbiter для R-03 **готова** (DTO + E2E smoke в `861534e`) — ждём Maestro.
> Стратегический контекст: `../_cowork_output/roadmap/ecosystem-roadmap.md`
> Актуальный статус: `../_cowork_output/status/2026-04-10-status.md`

## Текущее состояние
- ✅ R1–R4 собственного roadmap закрыты (p99 ≤5ms, typed errors, metrics, golden-tests)
- ✅ Typed DTOs + E2E smoke test для Maestro integration (`861534e`)
- ✅ MCP API расширен: `route_task`, `report_outcome`, `get_metrics`, `get_budget_status`
- ✅ **CI** (GitHub Actions: Rust stable/beta + Python ruff/pytest, `fe4c033`)
- ⏳ Ожидаем: Maestro R-01/R-02/R-03

## Правила ведения
- После каждой выполненной задачи проставь `[x]` и добавь хеш коммита
- **Контрактная заморозка**: DTO для Maestro (`861534e`) — заморожен. Любое изменение API, описанного в E2E smoke test, требует согласования с Maestro и bump версии MCP API

---

## Активные задачи

### R-10: CI/CD (effort S) — ✅ закрыт `fe4c033`

- [x] **GitHub Actions**: `cargo test` + `cargo clippy` + `ruff` (`.github/workflows/ci.yml`, `fe4c033`)
  - Rust matrix: stable, beta — fmt-check + clippy `-D warnings` + `cargo test --all-targets`
  - Python: `ruff format --check` + `ruff check` + `pytest orchestrator/tests/` через uv
  - Trigger: push/PR на `master`, `main`
  - Дополнительно: попутно вычищены 4 clippy-варнинга в тестовом коде (`cede423`)
  - Примечание: `pyrefly` из пункта плана не добавлен (не сконфигурирован в проекте) — при необходимости вынести в отдельную задачу

### R-13: Нормализация guardrails с ATP (effort M) — приоритет #1

- [ ] **Анализ overlap между invariants**
  - arbiter: 10 invariants в `arbiter-core/src/invariant/`
  - ATP: 3 правила в `../atp-platform/atp/evaluators/guardrails.py` ("inspired by arbiter")
  - Задокументировать семантический маппинг: какие правила одинаковые, какие расходятся
  - Решить: извлечь shared-типы (Rust JSON Schema → ATP Python) или выровнять naming

---

## Ждём от Maestro (НЕ делаем здесь, но отслеживаем)

- **R-01**: rename `codex` → `codex_cli` на Maestro-стороне — после этого наш `config/agents.toml` валиден
- **R-02**: Maestro добавит `task_type`/`language`/`complexity` в TaskConfig
- **R-03**: Maestro реализует MCP-клиент
- **R-05**: интеграционные тесты — мы предоставляем mock-endpoints и E2E harness

**Действие с нашей стороны**: когда Maestro начнёт R-03, быть готовыми помочь с отладкой. E2E smoke test (`861534e`) — reference implementation.

---

## НЕ делаем до стабилизации R-03

- ❌ Shared type library (R-14, XL) — сначала пусть Maestro встанет на наши DTO
- ❌ Дальнейшее расширение MCP API — зафиксировать то, что уже есть
- ❌ ECO-3 eval-driven routing (R-07) — зависит от R-06b (ATP SDK в Maestro)
