# TODO — arbiter

> Роль в экосистеме: MCP policy engine / router. Интеграция с Maestro shipped — обе стороны на v0.2.0+
> Обновлено **2026-07-26**. Предыдущая ревизия — снапшот от 2026-04-25, в котором все
> пункты были закрыты; накопленная с мая работа (R-07, каталог, гейты, authority-плоскость,
> shadow routing) в него не попадала.
> Стратегический контекст: `../prograph-vault/authored/notes/ecosystem-roadmap.md`

## Правила ведения

- Пункты уровня команды. Микрошаги реализации живут в `docs/plans/` и файлах планов, сюда не переносятся.
- Синтаксис чекбоксов `- [ ]` / `- [x]` обязателен — это машинно-читаемый маркер «осталось сделать».
- После выполнения — `[x]` + хеш коммита, затем пункт переезжает в «Закрыто».
- Опциональные инлайн-теги в хвосте строки чекбокса (формат — plan-fields v2):
  `@owner:<principal>`, `@blocked_by:<reference>`,
  `@trigger:"<проверяемое условие>"`, `@id:<node-id>`.
  Канонический principal: `github:<login>`, `github-team:<org>/<team>`,
  `repo:<manifest-key>` или `TBD`; bare handle/role — только legacy-синтаксис.
  Каноническая ссылка блокера: `todo://<repo>/<id>`; `<repo>#<slug>` —
  переходный legacy-синтаксис. `node-id` соответствует
  `[a-z0-9][a-z0-9._-]{0,63}` и уникален внутри репозитория.
  Отсутствие тега означает «неизвестно» — придумывать триггер там, где его нет, не надо.
- **Контрактная заморозка**: DTO для Maestro (`861534e`) заморожен. Любое изменение API,
  описанного в E2E smoke test, требует согласования с Maestro и bump версии MCP API.

---

## Активные задачи

### Shadow routing — Phase 2

- [ ] Решить по накопленным shadow-данным, нужен ли Phase 2 @owner:github:andrei-shtanakov @trigger:"eval_shadow.py на накопленном shadow_json показывает, что петля используется" @id:shadow-phase-2-decision — кандидаты (все отложены сознательно): hot-reload shadow-дерева через `watcher.rs`, `shadow_match_rate` в `get_metrics`, obs-контрактное событие для shadow-решений, несколько одновременных кандидат-политик, canary-graduation победившей
  - Phase 1 закрыт: `09b7b88` (#53) — `shadow_json` в `decisions`, offline `eval_shadow.py`
  - Источник списка: `20260714shadowroutingphase1plan.md` §«Phase 2 candidates»
  - Гоча при ручной проверке: сырой `route_task` без authority-контекста ведёт себя иначе, чем вызов из Maestro

### ADR-ECO-003b: каталог в рантайме (ADR ратифицирован 2026-07-06)

- [x] Подключить `arbiter_core::catalog` к `arbiter-mcp` @owner:github:andrei-shtanakov @id:arbiter-mcp-catalog-loader — сделано (issue #72 / PP-103, PR #73): `catalog_guard.rs` — сервер при старте валидирует `agents.toml` против user-config каталога (`$ATP_CATALOG` → XDG), fail-loud на невалидной паре (Check 5), warn-and-start при отсутствии каталога
- [x] Provider-swap смоук (acceptance (c) PP-103) @owner:github:andrei-shtanakov @id:approved-pp-103-catalog-last-mile — сделано (issue #72, PR #73): retire X + promote Y только правкой каталога (+scaffold-apply/рестарт) переключает `route_task` X → Y при байтово неизменном потребителе; `orchestrator/tests/test_provider_swap_smoke.py`
- [x] Общий conformance-тест на фикстурах каталога для трёх загрузчиков (Maestro / ATP / arbiter-Rust) @owner:github:andrei-shtanakov @id:catalog-conformance-fixtures — сторона arbiter закрыта (inbox issue #74, PR #75): вендорена пиненая копия SSOT-набора devtools (`arbiter-core/tests/fixtures/catalog-conformance/v1/` + `PIN`, `devtools@2a5c154`), сьют `catalog_conformance.rs` гоняет все `[[case]]`/`[[pathres]]` и сверяет целостность копии с `manifest.json`; локальные покейсовые фикстуры сняты. Owner самого набора остаётся devtools; участие Maestro/ATP — на их стороне
- [x] Пин-бамп conformance-набора каталога до `devtools@2533ff7` @owner:github:andrei-shtanakov @id:catalog-conformance-pin-bump-v1-gaps — сделано (inbox issue #76, PR #77): аддитивное расширение v1 (+`v1-empty-harnesses`, +`v7-unknown-kind`), семантика существующих кейсов не менялась. Загрузчик не правился — arbiter конформен на обоих новых кейсах (пустая map harness'ов → V1 на каждый агент; `kind` валидируется отдельно от `status`); обе фикстуры проверены мутацией

### R-07: benchmark-aware routing — открытые хвосты

Механизм сдан и сознательно сужен до тайбрейкера (decision C, `1fbbdf7` #34): аддитивная
клампованная дельта меняет исход только на почти-равных кандидатах и **не** переворачивает
доминантный (1.0) лист дерева.

- [ ] Развилка по силе связи: на реальном ре-свипе Δ`rank_score` ≈ 0.08 при разумном весе ≈ 0.15 не переворачивает доминантный лист → выбрать (а) более сильную связь `rank_score` с confidence или (б) переобучение дерева на мягкие листья @owner:github:andrei-shtanakov @id:r-07-link-strength-decision
  - Разблокирована: crossover-гейт закрыт 2026-08-16 анализом (`docs/2026-08-16-r07-crossover-gate-analysis.md`)
  - Аргумент из данных бенчмарка №2: усиление веса ничего не даёт на суитах-ничьих — разброс есть только там, где суит реально дифференцирует агентов; сильная форма crossover (переворот ранжирования) на сатурированном суите ненаблюдаема и едет сюда контекстом

---

## Ждём от соседей (не наша работа, отслеживаем)

- [x] **Maestro R-01/R-02/R-03** — закрыты на их стороне (v0.2.0); наш DTO-контракт `861534e` вендорнут без изменений
- [x] **Maestro R-05 contract-level** — 4 subprocess-теста (`f1f7d26`)
- [x] **RD-006 M4 (Maestro)** — `authority_context` едет в `constraints` при вызове `route_task` (`maestro/coordination/routing.py`)
- [x] **RD-006 M2 (steward)** — `profiles/authority.yaml` как SSOT; в arbiter вендорнута пиненая копия (`0cb27c8`, #52)
- [x] **Данные для R-07 №2 и A/B-вью** — прогон второго task_type тремя агентами @owner:repo:atp-platform @id:r-07-second-task-type-data — доставлено 2026-08-16: 3 прогона `req-extraction` в `benchmark_runs` через `report_benchmark`, atp-platform#279 закрыт

Пункты про TTL/retention и GIN-индекс для `benchmark_runs` относятся к **Maestro-side**
таблице и живут в их `TODO.md`; наша SQLite-таблица чистится общим 90-дневным retention.

---

## Закрыто

Хронологически, свежее сверху. Подробности — в PR и docs/.

- **PP-103: последняя миля ADR-ECO-003b** (issue #72, PR #73, 2026-08-17): (1) `arbiter-mcp/src/catalog_guard.rs` — при старте сервер валидирует `agents.toml` против user-config каталога (резолюция D2 `$ATP_CATALOG` → XDG `atp/`): невалидная пара (missing/retired модель, Check 5) → fail-loud exit 1 со списком находок; каталог не сконфигурирован (XDG-слой без файла) → warn + штатный старт; явный `$ATP_CATALOG` на отсутствующий/битый файл → ошибка. Валидация, не замена enrollment-плоскости: bare-id (`[aider]`) вне SSOT → warning. (2) Provider-swap смоук `orchestrator/tests/test_provider_swap_smoke.py`: retire X + promote Y только правкой каталога + штатный операторский шаг (`gen_agents_scaffold.py` → apply → рестарт) переключает `route_task` X → Y; потребитель байтово неизменен; плюс fail-loud тест на retired-ссылке

- **A/B-вью над `benchmark_runs`** (#70, 2026-08-16): подкоманда `ab` в `scripts/check_routable_gate.py` — «агент A vs B на бенчмарке T» для человеческого гейта флипа `routable` (вью, не гейт). Эффективные скоры (семантика `get_benchmark_score`, последний прогон `ts DESC, run_id DESC`), по-задачный дифф по пересечению `task_index` (точное целочисленное сравнение pass rate), legacy/`runs_graded=0` → ungraded, `INCOMPLETE COMPARISON` при разных наборах/truncated. Ограничение v1: suite identity в `benchmark_runs` не хранится — печатается в NOTE. **Все три arbiter-пункта ADR-ECO-003a закрыты** (мёртвые ключи `6ee2f39` #32, routable-гейт `6a1fbb2` #41, вью #70); статус ADR — Proposed, `agent_id` не автобампится (D1); `atp-platform#golden-suite-ab` был снят как blocker ранее
- **R-07 crossover-гейт** (2026-08-16) — закрыт анализом: `docs/2026-08-16-r07-crossover-gate-analysis.md`. Бенчмарк №2 (`req-extraction`, 3 прогона atp-platform) показал: сигнал `rank_score` task-зависим (Δ 0.209 на `code-review` не переносится, ничья 1.0 на `req-extraction`), global bias не утекает, re-rank на ничьей — честный no-op. Оговорка: суит сатурирован, сильная форма crossover ненаблюдаема — контекст уехал в `r-07-link-strength-decision`

- **Governance-гейт и обвязка CI** (07-16…07-19): governance gate в required checks (`06c8cf1`, #57), обновление путей Maestro (`ab17ad2`, #55), CODEOWNERS (`9643b90`), bump `mcp` 1.27.0 → 1.28.1 (`694f5fe`, #56)
- **M3-obs: per-request trace binding** (`e25ffed`, #59): `params._meta.traceparent` из maestro#88 биндится на время dispatch (`obs::bind_request_trace`, thread-local + RAII guard); мусор/отсутствие — молча текущее поведение. Плюс уточнение docs: логи идут в OTel JSONL, не в stderr (`faf9be3`, #58)
- **Shadow routing Phase 1** (`09b7b88`, #53 + план `985a186`, #54): candidate-policy shadow evaluation, `shadow_json` в decision log, offline `eval_shadow.py`, миграция схемы v2; без hot-reload и без shadow-полей в `get_metrics`
- **RD-006 authority plane** (M1 `e5237d6` #50 → hardening `2dc58b9` #51 → M3 `0cb27c8` #52): role/phase-scoped allowlist, fail-closed, вендоренный `authority.toml` + CI-гейт на пиненый SHA; дизайн `docs/2026-07-12-authority-split-design.md`
- **RD-002 promote-контракты** (`0226ad0`, #46 + фиксы `3fee073` #47, `532dade` #48): `contracts/budget/` и `contracts/policy-decision-ref/` — промоушн существующих ответов в контракты, потребители вендорят пиненые копии
- **ADR-ECO-003a D4: routable-flip гейт** (`6a1fbb2`, #41): `scripts/check_routable_gate.py` — диффовые правила A/B + `verify` против `benchmark_runs`; CI-job `routable-gate`; попутно закрыта дыра — workspace-тесты (`pytest tests/`) не гонялись в CI. Дизайн `docs/2026-07-05-routable-gate-design.md`
- **ADR-ECO-003b: загрузчик user-config каталога** (`6a966a0`, #39): `arbiter_core::catalog` (parse 3 плоскостей + validate V1–V7 + `resolve_path` `$ATP_CATALOG` → XDG `atp/`, fail-loud, без bundled-дефолта) и `arbiter-cli catalog path|check|list`. Дизайн `docs/2026-07-05-catalog-loader-design.md`
- **Каталог и agent_id** (`f3c955c` #24, `8b6bfec` #25, `cb71b57` #26, `e1881c7` #27, `c8e190e` #29, `6ee2f39` #32, `4f242a0` #37, `f11bb17` #38): конвенция `<harness>@<model>`, миграция ключей на `sonnet-4-6`/`gpt-5.5`, детерминированный tie-break по `agent_id`, catalog-driven скаффолд `agents.toml`, retired-ключи, промоушн `opencode@glm-5.1`
- **R-07 track B slice A** (`eec1879` #30, тест `3c307ad` #33, скоуп `1fbbdf7` #34): `get_benchmark_score` (task_type-scoped) + `apply_benchmark_rerank` под `ARBITER_BENCH_WEIGHT`, аудит в `pred.path`, метрика вне замороженного DTO
- **R-06b M4** (2026-05-23, #11/#13/#14/#15): 6-й MCP-tool `report_benchmark`, таблица `benchmark_runs` (`run_id` PK, идемпотентность), `protocolVersion` → `"1.1.0"`, workspace v0.2.0, разделение validation (-32602) и runtime (-32000) ошибок
- **observability v1 (Rust)** (`d1a8ecd`, 2026-04-25): `arbiter-core::obs` — OTel Logs JSONL, structlog/ulid-py для Python-клиента, contract-тесты `emit_contract`/`fixtures_contract`
- **arbiter#9** (`d1a8ecd`): `metadata.decision_id` в ответе `route_task`; парный Maestro `e5915f2`
- **R-10 CI/CD** (`fe4c033`, `6efe792`): Rust stable/beta + Python в GitHub Actions, release-бинарь linux-x64 и macos-arm64 как артефакт + tag-triggered Release
- **R-13 нормализация guardrails с ATP** — закрыт анализом: `docs/guardrails-atp-mapping.md`; системы работают в непересекающихся фазах (arbiter = pre-dispatch, ATP = pre-evaluation), shared-типы не извлекаем
- **R1–R4 собственного roadmap** (p99 ≤5ms, typed errors, metrics, golden-tests) + typed DTO и E2E smoke для Maestro (`861534e`)

---

## Не делаем

- ❌ Shared type library (R-14, XL) — 15 строк структур на двух языках с разными циклами релизов
- ❌ Дальнейшее расширение MCP API без запроса потребителя — 6 инструментов заморожены
- ❌ Автоматический бамп `agent_id` (ADR-ECO-003a D1) — это join-ключ между бенчмарками ATP и роутингом, а не версия зависимости
- ❌ Discovery моделей внутри arbiter — владелец devtools (ADR-ECO-003a D5); роутинг не должен опрашивать провайдеров
