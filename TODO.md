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
- [ ] **R-05**: интеграционные тесты с реальным subprocess — заблокировано нашим R-10 artifact-сабтаском (см. выше)

**Текущий статус**: интеграция технически разблокирована. Maestro вендорнул `arbiter_client.py` от commit `861534e`, наш DTO-контракт заморожен. Любой bump API = coordinated release.

---

## НЕ делаем до стабилизации R-03

- ❌ Shared type library (R-14, XL) — сначала пусть Maestro встанет на наши DTO
- ❌ Дальнейшее расширение MCP API — зафиксировать то, что уже есть
- ❌ ECO-3 eval-driven routing (R-07) — зависит от R-06b (ATP SDK в Maestro)
