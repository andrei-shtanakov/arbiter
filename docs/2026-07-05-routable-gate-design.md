# Дизайн: гейт routable-PR на benchmark-эвиденс (ADR-ECO-003a D4)

**Дата:** 2026-07-05 · **Статус:** Draft v3 (два раунда ревью; v1 — `a1cfba8`, v2 — `0189702`)
**Основание:** ADR-ECO-003a (`../_cowork_output/decisions/2026-07-02-adr-eco-003a-model-discovery-adoption.md` — dev-only sibling-workspace, в клоне отсутствует), arbiter-действие: «Gate merge: routable-PR (Plane 2) блокируется до наличия `rank_score` на golden-suite» (D4).
**Прецедент:** промоушн `opencode@glm-5.1` (PR #38 ↔ atp-platform#223) — гейт пройден вручную, эвиденс в commit message и комментарии каталога (rank 0.915 vs 0.777/0.705, golden `071f25d`, runs=3).

## 0. Уровень гарантии (честная формулировка)

Механизм — **двухслойный**, и слои дают разные гарантии:

- **CI-слой (`gate`) — evidence-declaration gate:** доказывает, что автор PR
  задекларировал полный, машинно-валидный, привязанный к конкретным `run_id`
  эвиденс. Он НЕ доказывает, что данные существуют (db в git/CI не живёт —
  003a D4).
- **Локальный слой (`verify`) — data-gate:** сверяет декларацию с реальными
  строками `benchmark_runs` в `arbiter.db`, воспроизводя runtime-семантику
  скора. Запускается владельцем перед merge (human-gate D4 = владелец,
  data-gate = `verify`).

Полное «D4 в одном CI» недостижимо без внешнего immutable evidence store;
привязка через обязательные `run_ids` (§3) сужает разрыв: декларация ссылается
на стабильные идентификаторы строк в **доверенной локальной БД** (`run_id` PK;
`ON CONFLICT DO NOTHING` даёт идемпотентность ingest'а, но НЕ неизменяемость
строк — это не immutable evidence), и её подделка обнаруживается первым же
запуском `verify`.

## 1. Объём

**Делаем:**
- `scripts/check_routable_gate.py` — stdlib-only (Python 3.12: `tomllib`,
  `sqlite3`, `argparse`), режимы `gate` (CI) и `verify` (локальный data-gate).
- CI-job `routable-gate` в `.github/workflows/ci.yml` (только `pull_request`).
- pytest-тесты в `tests/test_routable_gate.py` + **включение `pytest tests/` в
  CI python-job** (закрывает и pre-existing дыру: 19 существующих
  workspace-тестов сейчас в CI не запускаются вовсе).

**НЕ делаем:**
- Бэкфил `bench`-полей для существующих routable-пар — SSOT-side PR
  (atp-platform) + ре-вендоринг; follow-up.
- Зеркалирование конвенции `bench`-полей в SSOT-каноне — follow-up в
  atp-platform (канонный флип без полей приедет вендорингом и упрётся в наш
  гейт — желаемое поведение: заставляет добавить эвиденс в канон).
- A/B-вью над `benchmark_runs` — третий пункт 003a, отдельная задача.
- Изменения Rust-кода — не требуются (Rust-загрузчик игнорирует незнакомые
  поля, `bench`-блок его не ломает; проверено).
- Внешний immutable evidence store (подписанные summary, artifact registry) —
  за пределами одного репо; §0 фиксирует достижимый уровень без него.

## 2. Триггер гейта

Гейт анализирует пару версий `config/agents-catalog.toml` (base vs head).
Ключ сопоставления записей — `agent_id = "{harness}@{model}"`.
**Дубликаты `agent_id` в base или head → exit 2** (невалидный вход: свёртка
списка в map молча скрыла бы запись; это зеркало правила V4 Rust-валидатора).

**Правило A — промоушн.** Запись, которая:

1. существовала в base с `routable = false` (или без поля — default false) и
   имеет `routable = true` в head («флип»), либо
2. отсутствовала в base и добавлена в head с `routable = true` («новая routable-пара»),

обязана нести `tested = true` и валидный `bench`-блок (§3).

**Правило B — защита аудиторской записи (анти-обход).** `bench` у
routable-записи — долговременная аудиторская запись; последующие PR не могут
её тихо снести или испортить. Для каждой записи с `routable = true` в head:

1. если в base у неё был `bench`, а в head его нет → FAIL («evidence removed»);
2. если `bench` изменён (любое поле) или добавлен к уже-routable записи
   (сценарий SSOT-бэкфила) → новый `bench` проходит **полную**
   schema-валидацию §3 (+`tested = true`);
3. замена `run_ids`/`rank_score` **допустима** — это декларация ре-бенчмарка
   (модель перегнали на suite заново); гейт требует только валидность новой
   декларации, а её правдивость закрывают те же human-gate (ревью PR) +
   локальный `verify`, что и при первичном промоушне. Отдельного механизма
   «повторного подтверждения» не вводим (YAGNI: тот же контур доверия).

Записи с `routable = false` в head правилом B не покрываются: снятая с
роутинга пара может нести или не нести `bench` — он более не является
управляющей записью (история остаётся в git).

**Не гейтится:**
- `harnesses.*.routable` — флаг «может ли harness роутиться в принципе»; в
  роутинг вводит только флип пары (Plane 3).
- `true → false` (снятие с роутинга) и удаление записей.
- Routable-пары, у которых ни `routable`-флаг, ни `bench` не менялись —
  **grandfathered** без `bench` проходят молча (три текущие:
  `claude_code@claude-sonnet-4-6`, `codex_cli@gpt-5.5`, `opencode@glm-5.1`).

**Почему диффовый, а не инвентаризационный:** каталог обязан оставаться
байт-идентичным SSOT-канону atp-platform (conformance-check) — локальный
ретрофит полей невозможен; флипы материализуются здесь vendor-PR'ом (как #38 ↔
atp#223), и диффовый гейт стоит ровно на этой точке.

## 3. Схема эвиденса

Флипнутая/новая routable-запись обязана нести инлайн-таблицу `bench` **и**
`tested = true` (routable-пара не может не быть в ATP-свипе — иначе откуда
бенчмарк; нарушение = gate failure):

```toml
[[agents]]
harness  = "opencode"
model    = "glm-5.1"
tested   = true
routable = true
bench    = { benchmark = "code-review", suite = "071f25d", rank_score = 0.915, date = "2026-07-03", run_ids = ["r-glm-01", "r-glm-02", "r-glm-03"] }
```

| Ключ | Тип | Обязателен | Валидация в `gate` |
|---|---|---|---|
| `benchmark` | string | да | непустой; это `benchmark_id` из `benchmark_runs` (task-type-скоуп, напр. `code-review`) |
| `suite` | string | да | строгий формат digest: `^[0-9a-f]{7,64}$` (lowercase hex, пин `SUITE.lock`, atp#215); произвольная строка не проходит |
| `rank_score` | float | да | `math.isfinite` И в `[0, 1]` (TOML допускает `nan`/`inf` — голая проверка диапазона обходится); `bool` — отвергается (в Python `bool` — подкласс `int`) |
| `date` | string | да | ISO `YYYY-MM-DD`; **не в будущем** — сравнение строго с `datetime.now(timezone.utc).date()`, не с локальной датой процесса |
| `run_ids` | array of string | да | непустой список непустых строк без дубликатов; конкретные `run_id` строк `benchmark_runs`, по которым получен `rank_score` |
| `runs` | int | нет | если задан — обязан равняться `len(run_ids)` |
| `evidence` | string | нет | URI/путь на immutable-артефакт свипа (напр. `_bench_output/...` или URL) — рекомендуется |
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

### Exit-коды (общие для обоих режимов)

- `0` — нарушений нет (warnings допустимы);
- `1` — policy-нарушения (флип без/с невалидным эвиденсом; в verify —
  отсутствующие run_ids, расхождение скора, несвежая дата);
- `2` — невалидный вход/окружение: битый TOML, отсутствующий файл каталога
  или db, дубликаты `agent_id`, отсутствующая таблица `benchmark_runs` или
  несовместимая схема sqlite.

### Режим `gate` (CI, evidence-declaration)

```
python scripts/check_routable_gate.py gate \
    --base-file /tmp/base-catalog.toml \
    --head-file config/agents-catalog.toml
```

- Парсит обе версии (`tomllib`), проверяет отсутствие дубликатов `agent_id`,
  применяет правила A (промоушн) и B (защита `bench`) из §2.
- Вывод: `GATE FAIL <agent_id>: <причина>` на нарушение, либо
  `GATE OK: no gated changes` / `GATE OK: N gated change(s) with valid evidence`.
- База недоступна и не нужна: существование данных в этом режиме **не**
  проверяется (см. §0).

### Режим `verify` (локальный, data-gate)

```
python scripts/check_routable_gate.py verify \
    --db arbiter.db [--eps 0.05] [--catalog config/agents-catalog.toml]
```

`--eps` валидируется: конечное число (`math.isfinite`) и `>= 0`, иначе exit 2.

Для каждой routable-пары каталога **с** `bench`-блоком:

1. **Существование:** каждая `run_id` из `bench.run_ids` присутствует в
   `benchmark_runs` с совпадающими `agent_id` и `benchmark_id = bench.benchmark`.
   Отсутствие/несовпадение → FAIL.
2. **Эффективный скор — воспроизводит runtime-семантику**
   (`get_benchmark_score`, `arbiter-mcp/src/db.rs:817-841`): для строки
   эффективный скор = `score_components.rank_score`, если `score_components` —
   валидный JSON с числовым `rank_score`; иначе fallback на колонку `score`;
   результат клампится в `[0, 1]`. Битый JSON / отсутствующий ключ /
   нечисловой `rank_score` (в т.ч. **JSON-boolean** — зеркало
   `serde_json::Value::as_f64`, который на Bool возвращает None) → fallback,
   не ошибка (как в runtime). Схема объявляет колонку `TEXT NOT NULL`
   (db.rs:943) — NULL-кейс на реальных данных невозможен.
3. **Агрегация определена явно:** заявленный `bench.rank_score` сверяется с
   **арифметическим средним эффективных скоров по строкам `run_ids`**
   (`|mean − заявленный| ≤ eps`, default **0.05**, переопределяется флагом).
   Никакой «последней строки» в критерии гейта нет — набор строк зафиксирован
   декларацией.
4. **Свежесть даты:** `|bench.date − max(ts по run_ids)| ≤ 7 дней` → иначе FAIL
   (дата эвиденса обязана соответствовать реальному времени прогонов).
   Ingest гарантирует RFC3339 в `ts` — невалидный `ts` в строке из `run_ids`
   трактуется как повреждённые данные → **exit 2**, не policy-mismatch.
5. **Информационно (не критерий):** печатается runtime-эффективный скор — по
   последней строке `(agent_id, benchmark_id)` с детерминированной сортировкой
   `ORDER BY ts DESC, run_id DESC` (второй ключ — против недетерминизма при
   равных `ts`) — чтобы владелец видел, что роутинг фактически получит.

Routable-пары **без** `bench`-блока (grandfathered) → warning в stdout,
**не** ошибка (иначе verify бесполезен до SSOT-бэкфила).

Расхождение печатает фактическое среднее и по-строчные эффективные скоры
рядом с заявленным.

`suite` в db не хранится (`benchmark_runs` несёт только `benchmark_id`) —
suite-пин остаётся декларативным (формат-валидация в §3); его подлинность
проверяется на стороне atp-platform (`SUITE.lock`), не здесь.

## 5. CI

### Новый job `routable-gate`

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
        run: git show "${{ github.event.pull_request.base.sha }}:config/agents-catalog.toml" > /tmp/base-catalog.toml
      - name: Run gate
        run: python scripts/check_routable_gate.py gate --base-file /tmp/base-catalog.toml --head-file config/agents-catalog.toml
```

- База диффа — `github.event.pull_request.base.sha` (точный SHA, а не
  `origin/<base_ref>`, который может уехать между checkout и show).
- Только `pull_request` (на push дифф не определён — гейт стоит на PR).
- Stdlib-only → без uv/зависимостей в этом job'е (голый `python`).
- **Никакой db в CI** — жёсткое разделение (§0): CI = декларативный `gate`,
  данные = локальный `verify`. Артефакты/кэши с базой в Actions — антипаттерн
  (протухшие данные в гейте).

### Существующий python-job

Добавить шаг после `pytest (orchestrator)`:

```yaml
      - name: pytest (workspace)
        run: uv run python -m pytest tests/ -v
```

(запускает и новые тесты гейта, и 19 существующих workspace-тестов, которые
до сих пор в CI не гонялись; `Makefile` цель `test-python` — расширить так же).

## 6. Тесты

pytest в `tests/test_routable_gate.py` (скрипт импортируется как модуль,
файловые случаи — через `tmp_path`):

**gate:**
- флип false→true с валидным `bench` (+`tested=true`) → OK (exit 0);
- флип без `bench` → FAIL;
- новая запись с `routable=true` без `bench` → FAIL;
- флип с `bench`, но `tested = false` → FAIL;
- не-флип изменения (правка `tested`, комментариев, добавление non-routable пары) → OK, «no routable flips»;
- `true→false` и удаление записи → OK;
- отсутствие `routable` в base = false (default) → добавление `routable=true` = флип;
- битые значения → FAIL с причиной: `rank_score = 1.5`; `rank_score = nan` и
  `inf` (isfinite); `rank_score = true` (bool-не-число); `date = "03.07.2026"`;
  `date` в будущем; `suite = "foo"` (не hex-digest); пустой `run_ids`; дубликаты
  внутри `run_ids`; `runs` ≠ `len(run_ids)`; отсутствие обязательного ключа;
- **правило B (анти-обход):** удаление `bench` у routable-записи → FAIL;
  изменение любого поля `bench` с невалидным результатом → FAIL; валидное
  изменение (ре-бенчмарк: новые `run_ids`+`rank_score`+`date`) → OK;
  добавление валидного `bench` к уже-routable записи (бэкфил) → OK,
  невалидного → FAIL; удаление `bench` одновременно с `routable → false` → OK;
- дубликаты `agent_id` в base или head → exit 2;
- битый TOML / отсутствующий файл → exit 2.

**verify:**
- временная sqlite с реальной схемой `benchmark_runs` (колонки как в
  `arbiter-mcp/src/db.rs`: `run_id`, `payload_version`, `benchmark_id`,
  `agent_id`, `ts`, `score`, `score_components`, …);
- все run_ids есть, `score_components.rank_score` валиден, |mean − заявка| ≤ eps → OK;
- эффективный скор через fallback (колонка `TEXT NOT NULL` — NULL-кейса на
  реальной схеме не существует, тестируем достижимые): `score_components = "{}"`
  (нет ключа) / битый JSON / `rank_score` не число / **`rank_score` —
  JSON-boolean** (зеркало `as_f64` → None) → берётся `score`;
- один из `run_ids` отсутствует в db → FAIL;
- `run_id` есть, но `agent_id`/`benchmark_id` не совпадают → FAIL;
- |mean − заявка| > eps → FAIL (фактическое среднее в выводе);
- `bench.date` дальше 7 дней от `max(ts)` → FAIL;
- невалидный (не-RFC3339) `ts` в строке из `run_ids` → exit 2 (повреждённые данные);
- `--eps -1` / `--eps nan` → exit 2;
- равные `ts` у строк → информационный runtime-скор детерминирован
  (`ORDER BY ts DESC, run_id DESC`);
- grandfathered-пара без `bench` → warning, exit 0;
- нет файла db → exit 2; db без таблицы `benchmark_runs` → exit 2.

## 7. Риски / follow-ups (вне объёма)

- **Конвенция `bench`-полей должна попасть в SSOT-канон** (atp-platform):
  пока её там нет, канонный флип без полей приедет вендорингом и наш гейт его
  заблокирует — желаемое поведение, но follow-up завести.
- **Бэкфил трёх grandfathered-пар** — SSOT-side PR (данные есть: #38 для
  glm-5.1, re-sweep 2026-07-02 для инкумбентов; `run_ids` — из
  `_bench_output`/ingest-логов); после него `verify` покроет весь routable-набор.
- **Уровень гарантии зафиксирован в §0**: CI-слой доказывает декларацию, не
  данные. Усиление до сквозного data-gate требует внешнего immutable evidence
  store — сознательно отложено.
- Скрипт парсит каталог собственным `tomllib`-путём, а не Rust-загрузчиком —
  двух реализаций парсинга не избежать (разные языки); контракт минимальный
  (три плоскости + `bench`), дрейф ловится conformance-фикстурами (003b
  follow-up).
- Runtime-роутинг использует **последнюю** строку (`get_benchmark_score`), а
  заявка гейта — среднее по `run_ids`: это разные величины по построению;
  verify печатает обе (п. 5 в §4), расхождение между ними — сигнал владельцу,
  не ошибка гейта.
