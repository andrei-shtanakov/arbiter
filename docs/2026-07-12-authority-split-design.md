# Дизайн: Split Capability / Authority — role/phase-scoped allowlist (RD-006)

**Дата:** 2026-07-12 · **Статус:** Draft v1 (дизайн-фаза; код только после аппрува)
**Основание:** contracts-roadmap RD-006 (`prograph-vault/authored/roadmaps/contracts-v1.yaml`,
phase 5, deps RD-004 ✅ verified); консолидированный роадмап
(`prograph-vault/authored/notes/2026-07-11-ai-dark-factory-consolidated-roadmap.md`,
таблица владельцев: `Authority` = steward + proctor, consumers arbiter/Maestro/dispatcher;
`Capability` = arbiter, «есть (routing)»).
**Прецеденты:** RD-002 «promote, don't build» (`contracts/budget/`,
`contracts/policy-decision-ref/`); вендоринг agents-catalog (SSOT в atp-platform,
байт-идентичная копия + pinned-SHA CI-check); WS-006 gates (steward risk-model +
Maestro guards, fail-closed, opt-in).

## 0. Граница (честная формулировка)

Сегодня arbiter — чистый **capability**-движок: «справится ли агент» решается в три
яруса (hard-фильтр `supports_types/languages/slots` → DT-скоринг → benchmark re-rank),
всё ключуется `harness@model`. Плоскости **authority** («разрешено ли агенту в этом
контексте») не существует нигде: grep по role/phase/authority/allowlist в
`arbiter-core/src` и `arbiter-mcp/src` даёт ноль. ATP purpose-gating — транспортный
кузен (гейтит целый сервер по purpose токена), не то же самое.

RD-006 = ввести плоскость MAY, не смешав её с CAN:

| Плоскость | Вопрос | Владелец решения | Владелец данных |
|---|---|---|---|
| Capability (CAN) | справится ли | **arbiter** (routing, как есть) | arbiter (`config/agents.toml`, stats, benchmarks) |
| Authority (MAY) | разрешено ли в этом role×phase | **arbiter** (enforcement на route-time) | **steward** (governance-данные, PR-review) — arbiter вендорит пиненую копию |

Route-time authority-отказ — **first-class audited decision**, а не «кандидат тихо
исчез» (это ключевое отличие от capability-фильтра) и не invariant-veto (то —
safety per-candidate после ранжирования).

## 1. Объём

**Делаем (v1):**
- Контракт `contracts/authority/` (enforcement-контракт arbiter: wire-вход +
  audit-выход + семантика матчинга) — schema.json + rationale.md + fixtures/ +
  live-output assertion в `arbiter-mcp/tests/promoted_contracts.rs`.
- Контракт `contracts/capability/` — **промоушн существующего**: именуем то, что
  routing уже потребляет (agent_id `harness@model`, supports_*, слоты,
  stats/benchmark-ключи). Без изменений поведения.
- Policy-данные: SSOT `steward/profiles/authority.yaml` (steward-side PR) →
  вендоринг в `config/authority.toml` (arbiter) (пиненая копия, hot-reload по
  паттерну `invariants.toml`/`watcher.rs`, pinned-SHA CI-check по паттерну
  agents-catalog).
- Enforcement: новая стадия в `route_task::execute` **после** capability
  hard-фильтра (шаг 3), **до** feature-vectors/скоринга.
- Wire: `constraints.authority_context` (opt-in).
- Conformance-проверки authority-файла в CI (см. §4).

**НЕ делаем (v2+, осознанно):**
- Tools-allowlist (OpenOPC `toolset`) — нужна поддержка harness-уровня / proctor
  (runtime admission из роадмапа). Отдельная фаза.
- Escalation `timeout→default + approval_context` (OpenOPC) — после первых живых
  прогонов v1.
- Explicit deny-правила — v1 это чистый allowlist, default deny.
- Произвольные glob'ы в паттернах агентов (см. §4).
- proctor-интеграция (runtime admission) — когда proctor выйдет из паузы.

## 2. Словарь: role и phase — две оси, обе в v1

- **role** — функция агентского запуска: `decompose | implement | review | benchmark`.
  НЕ человеческие approval-роли (@architects, @qa — это CODEOWNERS-словарь steward,
  другая плоскость).
- **phase** — coarse-enum точки lifecycle, НЕ все внутренние состояния:
  `authoring | execution | merge | pr`. Проекция из Maestro WorkstreamStatus:
  DECOMPOSING→authoring, RUNNING→execution, MERGING→merge, PR_CREATED→pr.

Сегодня в Maestro-пайплайне оси почти вырождены (в execution бежит implement) —
это временно: как только рядом с implementer в той же фазе появится
reviewer/validator, одна ось перестанет хватать. Держим обе; вырожденность v1 —
свойство данных, не схемы.

## 3. Wire: `constraints.authority_context`

```json
{
  "constraints": {
    "authority_context": {
      "role": "implement",
      "phase": "execution"
    }
  }
}
```

- Живёт в **constraints**, не в task: это execution-контекст, не capability-фича.
  Инвариант: authority_context **не попадает** в 22-dim feature vector и в
  training-семантику DT — политика не должна протекать в обучение.
- Opt-in двухслойный (по образцу Maestro `gates:`):
  1. нет `config/authority.toml` → фича выключена, поведение как сейчас;
  2. файл есть, но запрос без `authority_context` → политика файла решает:
     `unknown_context: deny` (fail-closed default) | `allow` (миграционный режим).

## 4. Policy-данные: `authority.yaml` (SSOT в steward)

```yaml
version: 1
unknown_context: deny        # запрос без authority_context при включённой фиче
rules:
  - {role: implement, phase: execution, agents: ["claude_code@*", "codex_cli@gpt-5.5"]}
  - {role: review,    phase: execution, agents: ["claude_code@claude-opus-4-8"]}
  - {role: decompose, phase: authoring, agents: ["claude_code@*"]}
  - {role: benchmark, phase: execution, agents: ["opencode@*", "claude_code@*"]}
```

- **Матчинг агентов:** ровно две формы — точный `harness@model` и `harness@*`
  (весь харнесс). Произвольных glob'ов нет: `harness@*` нужен, чтобы каждый
  model-catalog refresh не ломал authority-файл, а большего v1 не требуется.
- **Default deny:** роль×фаза без правила → пустой allowlist → deny. Explicit
  deny-правил нет.
- **Conformance (CI, сторона arbiter при вендоринге + сторона steward при PR):**
  - каждый `harness` существует в agents-catalog;
  - каждый паттерн матчит ≥1 **routable** агента;
  - паттерн не матчит retired-модели;
  - роли/фазы — из закрытых словарей §2.
- Вендоринг: `profiles/authority.yaml` (steward) → `config/authority.toml` (arbiter)
  байт-эквивалентной трансформацией, `AUTHORITY_PINNED_SHA` в CI (паттерн
  agents-catalog). Hot-reload через существующий `watcher.rs`.

## 5. Route-pipeline: порядок и семантика отказа

Порядок стадий: **capability hard-filter → authority filter → DT-скоринг →
benchmark re-rank → invariants**. Authority после capability — иначе аудит шумит
по агентам, которые задачу всё равно не умеют; до скоринга — неавторизованные
кандидаты не попадают ни в feature vectors, ни в ranking-логи.

**Все кандидаты запрещены = `REJECT`, не `HOLD`.** Это детерминированный policy
denial, сам он не рассосётся; HOLD — для «может стать можно позже».

Reason-код: `authority_no_authorized_candidates`. Audit-payload в decision
(и в `metadata` ответа):

```json
{
  "authority": {
    "policy_sha": "sha256:…",
    "role": "implement",
    "phase": "execution",
    "denied": [
      {"agent_id": "opencode@glm-5.1", "reason": "no rule for (implement, execution) matches"}
    ]
  }
}
```

- `policy_sha` — sha256 вендоренного authority-файла (провенанс, как
  `risk_model_version` в WS-006).
- Блок пишется в decisions-таблицу как обычное решение (PolicyDecisionRef
  работает без изменений) — отказ аудируем и коррелируем.

## 6. Контракты

- **`contracts/authority/`** (arbiter): schema — вход `authority_context`
  (role/phase из закрытых enum'ов) + audit-блок ответа (§5) + семантика
  матчинга (§4). Fixtures: `allowed.json`, `denied.json`. Live-output
  assertion в `promoted_contracts.rs` — реальный вызов route_task с
  authority-контекстом против скомпилированной схемы.
- **`contracts/capability/`** (arbiter, промоушн): именует существующее —
  agent_id convention (`harness@model`), `supports_types`/`supports_languages`/
  `max_concurrent` как capability-декларация, stats/benchmark-ключи как
  competence-эвиденс. Rationale фиксирует: capability-фильтр остаётся молчаливым
  (это не отказ, а несоответствие), authority — аудируемым.

## 7. Milestones

- **M0 (этот док):** ревью дизайна; решения по OQ ниже.
- **M1 (arbiter):** `authority.rs` (загрузка+валидация+матчинг, unit-тесты
  pass/deny по конвенции invariant-rules) + стадия в route_task + audit-payload +
  `contracts/authority/` + live-output тест + hot-reload + conformance-скрипт CI.
- **M2 (steward):** `profiles/authority.yaml` SSOT + conformance на PR
  (переиспользует каталог из atp-platform как read-only reference).
- **M3 (вендоринг):** копия в arbiter + `AUTHORITY_PINNED_SHA` CI-check.
- **M4 (Maestro, handoff):** `authority_context` в route_task-вызовах
  (role/phase из workstream-контекста). Малый PR по образцу сегодняшних.
- **M5 (roadmap):** evidence_rules RD-006 в contracts-v1.yaml после фиксации
  путей (honesty rule — как RD-004).
- **`contracts/capability/`** — параллельно M1, независим.

## 8. Open Questions

- **OQ-1:** формат вендоренной копии — точный YAML→TOML маппинг или хранить YAML
  и в arbiter (`config/` сегодня TOML-only)? Предложение: TOML с детерминированной
  трансформацией + байт-проверка через каноникализацию, как удобнее CI.
- **OQ-2:** `unknown_context: allow` как миграционный режим — нужен ли вообще,
  или включение фичи сразу требует authority_context от всех продовых вызовов?
  Предложение: оставить в схеме, задеплоить с `deny`.
- **OQ-3:** словарь `phase` — хватает ли четырёх (`authoring|execution|merge|pr`),
  и кто владелец enum'а (contracts/authority схема = arbiter, но семантика фаз —
  Maestro/steward)? Предложение: enum в контракте arbiter, маппинг из
  WorkstreamStatus — в доке Maestro-handoff.
