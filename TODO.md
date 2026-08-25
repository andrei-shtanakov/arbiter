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

- [ ] Решить по накопленным shadow-данным, нужен ли Phase 2 @owner:github:andrei-shtanakov @trigger:"eval_shadow.py на накопленном shadow_json показывает, что петля используется" @id:shadow-phase-2-decision — кандидаты (все отложены сознательно): hot-reload shadow-дерева через `watcher.rs`, `shadow_match_rate` в `get_metrics`, obs-контрактное событие для shadow-решений, несколько одновременных кандидат-политик, canary-graduation победившей @epic:eco.routing
  - Phase 1 закрыт: `09b7b88` (#53) — `shadow_json` в `decisions`, offline `eval_shadow.py`
  - Источник списка: `20260714shadowroutingphase1plan.md` §«Phase 2 candidates»
  - Гоча при ручной проверке: сырой `route_task` без authority-контекста ведёт себя иначе, чем вызов из Maestro

### ADR-ECO-003b: каталог в рантайме (ADR ратифицирован 2026-07-06)

- [x] Подключить `arbiter_core::catalog` к `arbiter-mcp` @owner:github:andrei-shtanakov @id:arbiter-mcp-catalog-loader — сделано (issue #72 / PP-103, PR #73): `catalog_guard.rs` — сервер при старте валидирует `agents.toml` против user-config каталога (`$ATP_CATALOG` → XDG), fail-loud на невалидной паре (Check 5), warn-and-start при отсутствии каталога
- [x] Provider-swap смоук (acceptance (c) PP-103) @owner:github:andrei-shtanakov @id:approved-pp-103-catalog-last-mile — сделано (issue #72, PR #73): retire X + promote Y только правкой каталога (+scaffold-apply/рестарт) переключает `route_task` X → Y при байтово неизменном потребителе; `orchestrator/tests/test_provider_swap_smoke.py`
- [x] Общий conformance-тест на фикстурах каталога для трёх загрузчиков (Maestro / ATP / arbiter-Rust) @owner:github:andrei-shtanakov @id:catalog-conformance-fixtures — сторона arbiter закрыта (inbox issue #74, PR #75): вендорена пиненая копия SSOT-набора devtools (`arbiter-core/tests/fixtures/catalog-conformance/v1/` + `PIN`, `devtools@2a5c154`), сьют `catalog_conformance.rs` гоняет все `[[case]]`/`[[pathres]]` и сверяет целостность копии с `manifest.json`; локальные покейсовые фикстуры сняты. Owner самого набора остаётся devtools; участие Maestro/ATP — на их стороне
- [x] Пин-бамп conformance-набора каталога до `devtools@2533ff7` @owner:github:andrei-shtanakov @id:catalog-conformance-pin-bump-v1-gaps — сделано (inbox issue #76, PR #77): аддитивное расширение v1 (+`v1-empty-harnesses`, +`v7-unknown-kind`), семантика существующих кейсов не менялась. Загрузчик не правился — arbiter конформен на обоих новых кейсах (пустая map harness'ов → V1 на каждый агент; `kind` валидируется отдельно от `status`); обе фикстуры проверены мутацией

### `benchmark_runs`: владение и предпосылки (решено по inbox #78)

- [x] Решить владельца трёх пунктов про `benchmark_runs` (GIN-индекс, нормализация, TTL/retention) @owner:github:andrei-shtanakov @id:benchmark-runs-prereqs-ownership — решено (inbox issue #78, PR #79): **владелец — arbiter**, таблица наша и единственная релевантная. Maestro удаляет три своих watch-пункта (`r-07-prereq-gin-index`, `r-07-prereq-normalize`, `r-07-prereq-retention`); формулировки GIN/`jsonb` под SQLite неприменимы и переписаны ниже
- [ ] Retention для `benchmark_runs`: политика **«последние N прогонов на пару (agent_id, benchmark_id)»**, не «старше X дней» @owner:github:andrei-shtanakov @trigger:"benchmark_runs > 10k строк ИЛИ файл БД заметно растёт из-за per_task" @id:benchmark-runs-retention — сейчас таблица не чистится ничем: `purge_older_than` покрывает только `outcomes`/`decisions`. Возрастной purge здесь был бы **активно вреден**: `get_benchmark_score` берёт последний **пригодный** прогон по паре, поэтому удаление по возрасту способно стереть единственный пригодный прогон агента и молча выключить re-rank для него. Уточнение по итогам #82: пригодный прогон может лежать за сколь угодно длинным хвостом удержанных (не-качественных), поэтому политика обязана считать **пригодные прогоны**, а не строки. Не срочно: в таблице ~16 строк (13 свипа + 3 `req-extraction`) @epic:eco.routing

### Контракт `report_benchmark-v1`: единицы и семантика скора (inbox #81, #82)

- [x] Единица `score` в `report_benchmark-v1` не зафиксирована — два продюсера кладут разные величины @owner:github:andrei-shtanakov @id:benchmark-score-unit-mismatch — inbox issue #81 (from maestro). atp-platform шлёт долю [0..1], maestro — ATP `total_score` в процентах [0..100]; `get_benchmark_score` молча `.clamp(0.0, 1.0)` превращает любой процент > 1% в идеальную `1.0`. Канон — **доля [0..1]**; сделано: валидация на ингесте (`-32602` с именем канонической единицы) + отказ вместо верхнего клампа на чтении, `minimum`/`maximum` в схеме, нижний кламп сохранён (`rank_score` законно уходит чуть ниже нуля)
- [x] Принять `score_semantics` и учитывать `quality_signal` при ре-ранке @owner:github:andrei-shtanakov @id:benchmark-score-semantics — inbox issue #82 (from maestro). ATP-контракт v1 отдаёт `score_semantics` (`kind`/`quality_signal`/`coverage`), где `quality_signal: false` означает completion-rate, а не качество. Сегодня репорт никогда не инертен: любое число становится входом маршрутизации. Сделано: поле принимается и хранится дословно (миграция v3, колонка `benchmark_runs.score_semantics`), `get_benchmark_score` идёт от новейшего прогона к старому и берёт первый пригодный — не-качественный прогон хранится, но в тайбрейкер не идёт и **не маскирует** качественный прогон постарше. Отсутствие блока = legacy-продюсер и остаётся пригодным (иначе выключился бы весь текущий датасет R-07). `payload_version` не бампался: схема v1 структурно терпит поле, а бамп сломал бы локстеп — arbiter отвергает незнакомые версии. Maestro может смягчать свой гейт до «отправлять с меткой»

### Детерминированный routing-эвал каталога (inbox #84)

- [x] agents-toml-deterministic-routing-eval @owner:github:andrei-shtanakov @id:agents-toml-deterministic-routing-eval — сделано (`ebd2d5e`, PR #85; follow-up по ревью Copilot: валидация `schema_version` + фейл ратчета на пустом наборе позитивов): бесплатный CI-эвал `config/agents.toml` без LLM (inbox issue #84, from ai-repos-research): TF-IDF-матчинг тестовых запросов по описательной поверхности агентов (display_name + supports_types + supports_languages), метрика trigger rank-1 rate с ратчетом (`--min-rank1`), детектор коллизий описаний (warn/error по косинусу); фикстуры кейсов пиненым набором `[[case]]`, у негативного кейса объявлен owner-агент, обязанный обойти проверяемого. Второй независимый сигнал к benchmark-петле R-07, не замена. Образец: agent-skills `scripts/run-evals.js`

### R-07: benchmark-aware routing — открытые хвосты

Механизм сдан и сознательно сужен до тайбрейкера (decision C, `1fbbdf7` #34): аддитивная
клампованная дельта меняет исход только на почти-равных кандидатах и **не** переворачивает
доминантный (1.0) лист дерева.

- [ ] Развилка по силе связи: на реальном ре-свипе Δ`rank_score` ≈ 0.08 при разумном весе ≈ 0.15 не переворачивает доминантный лист → выбрать (а) более сильную связь `rank_score` с confidence или (б) переобучение дерева на мягкие листья @owner:github:andrei-shtanakov @id:r-07-link-strength-decision @epic:eco.routing
  - Разблокирована: crossover-гейт закрыт 2026-08-16 анализом (`docs/2026-08-16-r07-crossover-gate-analysis.md`)
  - Аргумент из данных бенчмарка №2: усиление веса ничего не даёт на суитах-ничьих — разброс есть только там, где суит реально дифференцирует агентов; сильная форма crossover (переворот ранжирования) на сатурированном суите ненаблюдаема и едет сюда контекстом

---

## Ждём от соседей (не наша работа, отслеживаем)

- [x] **Maestro R-01/R-02/R-03** — закрыты на их стороне (v0.2.0); наш DTO-контракт `861534e` вендорнут без изменений
- [x] **Maestro R-05 contract-level** — 4 subprocess-теста (`f1f7d26`)
- [x] **RD-006 M4 (Maestro)** — `authority_context` едет в `constraints` при вызове `route_task` (`maestro/coordination/routing.py`)
- [x] **RD-006 M2 (steward)** — `profiles/authority.yaml` как SSOT; в arbiter вендорнута пиненая копия (`0cb27c8`, #52)
- [x] **Данные для R-07 №2 и A/B-вью** — прогон второго task_type тремя агентами @owner:repo:atp-platform @id:r-07-second-task-type-data — доставлено 2026-08-16: 3 прогона `req-extraction` в `benchmark_runs` через `report_benchmark`, atp-platform#279 закрыт

~~Пункты про TTL/retention и GIN-индекс для `benchmark_runs` относятся к Maestro-side
таблице и живут в их `TODO.md`; наша SQLite-таблица чистится общим 90-дневным
retention.~~ **Приписка была неверна дважды** (разобрано по inbox #78, 2026-08-18):
таблица `benchmark_runs` — наша SQLite (`arbiter-spec.md` §3.2), у Maestro такой
таблицы нет вовсе (их упоминания — указатель `correlation.py` и тест, читающий
НАШУ БД); и `purge_older_than` чистит только `outcomes` и `decisions`, а
`benchmark_runs` не трогает. Владелец всех трёх пунктов — arbiter; см. раздел
«benchmark_runs» ниже.

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
- ❌ GIN-индекс по `benchmark_runs.per_task` (бывш. maestro `@id:r-07-prereq-gin-index`) — GIN/`jsonb` это PostgreSQL, у нас SQLite и плана миграции нет. Триггер («R-07 начинает писать SQL-фильтры по `per_task`») **не сработал**: `per_task` нигде не фильтруется в SQL — колонка читается целиком и парсится в приложении (`check_routable_gate.py`), а единственный горячий запрос `(agent_id, benchmark_id, ts DESC)` уже покрыт `idx_benchmark_runs_agent_bench_ts`. Появится реальный SQL-спрос — ответ в SQLite это generated column + индекс либо дочерняя таблица, но не GIN
- ❌ Нормализация `benchmark_task_results` (бывш. maestro `@id:r-07-prereq-normalize`) — тот же триггер и то же состояние, что у GIN: формального запроса к `per_task` нет. Решается вместе с ним, если спрос появится
- ❌ Discovery моделей внутри arbiter — владелец devtools (ADR-ECO-003a D5); роутинг не должен опрашивать провайдеров

## codex-review: потребитель кита steward (принят 2026-08-25)

- [ ] PR-B: caller-workflow гейта codex-review (по образцу пилота spec-runner:
      механика из base, потолки, generated-декларация, экономный триггер по
      драфту/лейблу) + лейбл `codex-review` + секрет `CODEX_REVIEW_API_KEY`
      (кладёт владелец в настройки репо) — после мержа PR-A
      @owner:github:andrei-shtanakov @id:codex-review-caller

  PR-A (этот): кит завендорен — `scripts/review/` (5 POSIX-скриптов) +
  `.github/codex/review-schema.json`, PIN @ steward `e4c43cc`;
  copy-integrity — джоба `review-kit-integrity` в ci.yml, чекер из base
  (на первом PR — бутстрап-notice); upstream-drift — вахта
  `review-kit-drift.yml` (не PR-гейт); `review-prompt.md` — данные репо,
  вне integrity; generated-декларация — `.gitattributes`. Ре-вендор —
  рецепт в комментарии PIN; дисциплина раундов гейта — спека steward §13;
  умолчание итераций — экономный цикл (local.sh → драфт → один платный
  прогон, см. Git workflow в CLAUDE.md).
