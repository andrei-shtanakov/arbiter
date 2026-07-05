# Дизайн: гейт routable-PR на benchmark-эвиденс (ADR-ECO-003a D4)

**Дата:** 2026-07-05 · **Статус:** Draft (на ревью)
**Основание:** ADR-ECO-003a (`../_cowork_output/decisions/2026-07-02-adr-eco-003a-model-discovery-adoption.md` — dev-only sibling-workspace, в клоне отсутствует), arbiter-действие: «Gate merge: routable-PR (Plane 2) блокируется до наличия `rank_score` на golden-suite» (D4: объединяет human-gate и data-gate).
**Прецедент:** промоушн `opencode@glm-5.1` (PR #38 ↔ atp-platform#223) — гейт пройден вручную, эвиденс в commit message и комментарии каталога (rank 0.915 vs 0.777/0.705, golden `071f25d`, runs=3).

## 1. Объём

**Делаем:**
- `scripts/check_routable_gate.py` — stdlib-only (Python 3.12: `tomllib`, `sqlite3`,
  `argparse`), два режима: `gate` (диффовый, для CI) и `verify` (сверка с
  `arbiter.db`, локально).
- CI-job `routable-gate` в `.github/workflows/ci.yml` (только `pull_request`).
- pytest-тесты в `tests/`.

**НЕ делаем:**
- Бэкфил `bench`-полей для существующих routable-пар — это правка SSOT-канона
  (atp-platform) + ре-вендоринг; кросс-репный follow-up.
- Зеркалирование конвенции `bench`-полей в SSOT-каноне — тоже follow-up в
  atp-platform (без него канонный флип без полей приедет вендорингом и упрётся
  в наш гейт — что и является желаемым поведением: гейт заставит добавить
  эвиденс в канон).
- A/B-вью над `benchmark_runs` — третий пункт 003a, отдельная задача.
- Изменения Rust-кода — не требуются (Rust-загрузчик уже игнорирует
  незнакомые поля, `bench`-блок его не ломает).

## 2. Триггер гейта

Гейт анализирует пару версий `config/agents-catalog.toml` (base vs head) и
срабатывает на **каждую** `[[agents]]`-запись, которая:

1. существовала в base с `routable = false` (или без поля — default false) и
   имеет `routable = true` в head («флип»), либо
2. отсутствовала в base и добавлена в head с `routable = true` («новая routable-пара»).

Ключ сопоставления записей — `agent_id = "{harness}@{model}"`.

**Не гейтится:**
- `harnesses.*.routable` — флаг «может ли harness роутиться в принципе»; в
  роутинг вводит только флип пары (Plane 3).
- `true → false` (снятие с роутинга) и удаление записей.
- Существующие routable-пары без изменений — **grandfathered** (три текущие:
  `claude_code@claude-sonnet-4-6`, `codex_cli@gpt-5.5`, `opencode@glm-5.1`).

**Почему диффовый, а не инвентаризационный:** каталог обязан оставаться
байт-идентичным SSOT-канону atp-platform (conformance-check) — локальный
ретрофит полей невозможен; флипы материализуются здесь vendor-PR'ом (как #38 ↔
atp#223), и диффовый гейт стоит ровно на этой точке.

## 3. Схема эвиденса

Флипнутая/новая routable-запись обязана нести инлайн-таблицу `bench`:

```toml
[[agents]]
harness  = "opencode"
model    = "glm-5.1"
tested   = true
routable = true
bench    = { benchmark = "code-review", suite = "071f25d", rank_score = 0.915, runs = 3, date = "2026-07-03" }
```

| Ключ | Тип | Обязателен | Валидация в `gate` |
|---|---|---|---|
| `benchmark` | string | да | непустой; это `benchmark_id` из `benchmark_runs` (task-type-скоуп, напр. `code-review`) |
| `suite` | string | да | непустой; пин golden-suite (sha `SUITE.lock`, atp#215) — декларативный, в db не хранится |
| `rank_score` | float | да | в `[0, 1]` |
| `date` | string | да | ISO `YYYY-MM-DD` |
| `runs` | int | нет | ≥ 1, если задан |
| `notes` | string | нет | свободный текст (напр. «vs codex 0.777 / claude 0.705») |

Нарушение любого правила = невалидный эвиденс = красный PR (строгая валидация:
полупустая декларация хуже её отсутствия — гейт существует ради проверяемости).

Сравнение с инкумбентами в схему **не** вшивается (никаких `baselines`) — это
работа A/B-вью; проза сравнения — в `notes`.

Все три загрузчика (Rust-загрузчик arbiter — проверено; ATP/Maestro — по
конвенции 003b) игнорируют незнакомые поля, поэтому `bench`-блок схему каталога
не ломает.

## 4. Скрипт: `scripts/check_routable_gate.py`

Stdlib-only, без внешних зависимостей (CI ставит только Python).

### Режим `gate` (CI, декларативный)

```
uv run python scripts/check_routable_gate.py gate \
    --base-file /tmp/base-catalog.toml \
    --head-file config/agents-catalog.toml
```

- Парсит обе версии (`tomllib`), находит флипы/новые routable-пары (§2).
- Для каждой требует валидный `bench`-блок (§3).
- Вывод: список нарушений в stdout (`GATE FAIL <agent_id>: <причина>`), либо
  `GATE OK: no routable flips` / `GATE OK: N flip(s) with valid evidence`.
- Exit: 0 — нарушений нет; 1 — есть нарушения; 2 — не смог распарсить входы
  (битый TOML, нет файла).
- База недоступна и не нужна: цифры в этом режиме **не** сверяются.

### Режим `verify` (локальный, data-gate)

```
uv run python scripts/check_routable_gate.py verify \
    --db arbiter.db [--eps 0.05] [--catalog config/agents-catalog.toml]
```

- Для каждой routable-пары каталога **с** `bench`-блоком:
  - в `benchmark_runs` существует ≥1 строка `(agent_id, benchmark_id = bench.benchmark)`
    (запрос по последнему `ts`);
  - `|последний score − bench.rank_score| ≤ eps` (default **0.05** — скор
    агрегирован по прогонам, межпрогонная дисперсия реальна).
- Routable-пары **без** `bench`-блока (grandfathered) → warning в stdout,
  **не** ошибка (иначе verify бесполезен до SSOT-бэкфила).
- Расхождение печатает фактический score из db рядом с заявленным.
- Exit: 0 — все аннотированные пары сверены (warnings допустимы); 1 — есть
  отсутствующие строки или расхождение > eps; 2 — нет db/каталога, битый TOML.
- `suite` в db не хранится (`benchmark_runs` несёт только `benchmark_id`) —
  suite-пин остаётся декларативным; его подлинность проверяется на стороне
  atp-platform (SUITE.lock), не здесь.

## 5. CI-job

Новый job в `.github/workflows/ci.yml`:

```yaml
  routable-gate:
    name: Routable-flip gate (ADR-003a D4)
    runs-on: ubuntu-latest
    if: github.event_name == 'pull_request'
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - name: Set up Python 3.12
        uses: actions/setup-python@v5
        with:
          python-version: "3.12"
      - name: Extract base catalog
        run: git show "origin/${{ github.base_ref }}:config/agents-catalog.toml" > /tmp/base-catalog.toml
      - name: Run gate
        run: python scripts/check_routable_gate.py gate --base-file /tmp/base-catalog.toml --head-file config/agents-catalog.toml
```

- Только `pull_request` (на push в master дифф не определён — гейт стоит на PR).
- Stdlib-only → без uv/зависимостей в этом job'е (голый `python`).
- **Никакой db в CI** — жёсткое разделение: CI = декларативный `gate`,
  данные = локальный `verify`. Артефакты/кэши с базой в Actions — антипаттерн
  (протухшие данные в гейте).

## 6. Тесты

pytest в `tests/test_routable_gate.py` (рядом с существующими workspace-тестами;
скрипт импортируется как модуль, файловые случаи — через `tmp_path`):

**gate:**
- флип false→true с валидным `bench` → OK (exit 0);
- флип без `bench` → FAIL;
- новая запись с `routable=true` без `bench` → FAIL;
- не-флип изменения (правка `tested`, комментариев, добавление non-routable пары) → OK, «no routable flips»;
- `true→false` и удаление записи → OK;
- отсутствие `routable` в base = false (default) → добавление `routable=true` к такой записи = флип;
- битые значения: `rank_score = 1.5`, `date = "03.07.2026"`, пустой `suite`,
  отсутствие обязательного ключа → FAIL с указанием причины;
- битый TOML / отсутствующий файл → exit 2.

**verify:**
- временная sqlite с реальной схемой `benchmark_runs` (колонки как в
  `arbiter-mcp/src/db.rs`: `run_id`, `benchmark_id`, `agent_id`, `ts`, `score`, …);
- аннотированная пара, строка есть, |Δ| ≤ eps → OK;
- строки нет → FAIL;
- |Δ| > eps → FAIL (и фактический score в выводе);
- берётся **последняя** строка по `ts`, не первая попавшаяся;
- grandfathered-пара без `bench` → warning, exit 0;
- нет файла db → exit 2.

## 7. Риски / follow-ups (вне объёма)

- **Конвенция `bench`-полей должна попасть в SSOT-канон** (atp-platform):
  пока её там нет, канонный флип без полей приедет вендорингом и наш гейт его
  заблокирует — это желаемое поведение (заставляет добавить эвиденс в канон),
  но follow-up надо завести, чтобы не удивляться.
- **Бэкфил трёх grandfathered-пар** — SSOT-side PR (данные есть: #38 для
  glm-5.1, re-sweep 2026-07-02 для инкумбентов); после него `verify` покроет
  весь routable-набор.
- **`gate` доверяет декларации** (rank_score мог быть вписан от руки) — по
  построению: данные в git не живут (003a D4). Честность цифр закрывает
  локальный `verify` + human-gate владельца.
- Скрипт парсит каталог собственным `tomllib`-путём, а не Rust-загрузчиком —
  двух реализаций парсинга не избежать (разные языки); контракт минимальный
  (три плоскости + `bench`), дрейф ловится conformance-фикстурами (003b
  follow-up).
