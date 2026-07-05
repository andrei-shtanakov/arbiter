# Дизайн: Rust-загрузчик user-config каталога (ADR-ECO-003b)

**Дата:** 2026-07-05 · **Статус:** Draft (на ревью)
**Основание:** ADR-ECO-003b (`../_cowork_output/decisions/2026-07-02-adr-eco-003b-catalog-distribution.md`), рекомендация «arbiter (Rust): загрузчик того же TOML из user-config (`$ATP_CATALOG`/XDG); без bundled-дефолта».

## 1. Объём

**Делаем:** загрузчик каталога (резолюция пути + парсинг + валидация) как чистый
модуль `arbiter_core::catalog` + CLI-поверхность `arbiter-cli catalog
path|check|list`.

**НЕ делаем (в этой итерации):**
- Интеграцию в arbiter-mcp (валидация agents.toml против каталога при старте) —
  следующий шаг, отдельная задача.
- `models init/discover/update` — это ATP CLI (ADR-003b D3), не arbiter.
- Кросс-языковой conformance-тест трёх загрузчиков — вне объёма; фикстуры
  кладём так, чтобы их можно было расшарить позже.

Runtime arbiter-mcp не меняется: сервер стартует на `config/agents.toml`, как раньше.

## 2. Резолюция пути (ADR-003b D2)

Приоритет сверху вниз:

1. `$ATP_CATALOG` — явный путь к файлу. Если переменная задана, fallback ниже
   **не** происходит: файла нет → ошибка `catalog file not found: <path> (from $ATP_CATALOG)`.
2. `$XDG_CONFIG_HOME/atp/agents-catalog.toml` (если `XDG_CONFIG_HOME` задан и непуст).
3. `~/.config/atp/agents-catalog.toml`.

Слои 2–3 — это один XDG-слой с дефолтом: выбор между ними определяется только
тем, задана ли `XDG_CONFIG_HOME` (по XDG-спеке), **не** существованием файла.
Резолюция даёт ровно один кандидатный путь; файла по нему нет → fail-loud.

Ни одного слоя нет / файл не существует → **fail-loud**:

```
model catalog not configured: set $ATP_CATALOG or create ~/.config/atp/agents-catalog.toml
```

Никакого скрытого дефолта. `config/agents-catalog.toml` в репе — dev-SSOT-вендор
для scaffold-генерации, runtime/CLI его **не читает** (граница из CLAUDE.md:
shipped-код не резолвит dev-ресурсы).

Канонический XDG-namespace: **`atp/`** (решение этого дизайна; согласуется с
именем `$ATP_CATALOG`; закрывает открытый пункт ADR-003b «согласовать
канонический XDG-путь»).

## 3. Схема (три плоскости ADR-ECO-003)

```rust
// arbiter-core/src/catalog/mod.rs (типы; serde, unknown fields игнорируются)
pub struct Catalog {
    pub models: BTreeMap<String, ModelEntry>,     // Плоскость 1
    pub harnesses: BTreeMap<String, HarnessEntry>, // Плоскость 2
    pub agents: Vec<AgentEntry>,                   // Плоскость 3
}

pub struct ModelEntry {
    pub vendor: String,
    pub status: ModelStatus,        // active | deprecated | retired
    pub aliases: Vec<String>,       // default []
}

pub struct HarnessEntry {
    pub kind: HarnessKind,          // cli | api-baseline | local
    pub shim: String,
    pub model_env: Option<String>,
    pub model_flag: Option<String>,
    pub routable: bool,             // default false
}

pub struct AgentEntry {
    pub harness: String,
    pub model: String,
    pub tested: bool,               // default false
    pub routable: bool,             // default false
}
```

- `agent_id = "{harness}@{model}"` — производное, метод `AgentEntry::agent_id()`.
- Неизвестные поля/секции **не** ломают парсинг (forward-compat: schema растёт
  на стороне SSOT, старый загрузчик не должен падать).
- Незнакомое значение enum (`status`, `kind`) — **degrade-with-warning**, не
  ошибка: парсится в fallback-вариант `Other(String)` (custom Deserialize),
  валидация даёт warning V7. Обоснование: три загрузчика (ATP/Maestro/arbiter)
  релизятся независимо; новое значение `status` в SSOT (напр. `preview`) не
  должно валить весь каталог у не обновлённого arbiter — иначе forward-compat
  декларируется, но не выполняется. Семантика консервативная: `Other`-статус
  трактуется как «не active» (не даёт прав, которых мы не понимаем), `Other`-kind
  информационен и ни на что не влияет.

## 4. Валидация (после парсинга)

| # | Правило | Севериті |
|---|---------|----------|
| V1 | Каждый `[[agents]].harness` объявлен в `[harnesses.*]` | error |
| V2 | Каждый `[[agents]].model` объявлен в `[models.*]` | error |
| V3 | `[[agents]]`-ссылка на модель со `status="retired"` | error |
| V4 | Дубль `agent_id` среди `[[agents]]` | error |
| V5 | `[[agents]].routable=true` при `harnesses.<h>.routable=false` | error (противоречие плоскостей) |
| V6 | `[[agents]]`-ссылка на модель со `status="deprecated"` | warning (не фатально) |
| V7 | Незнакомое значение `status`/`kind` (распарсено как `Other`) | warning (§3, degrade-with-warning) |

V2+V3 **вместе** зеркалят conformance Check 5 (`devtools/check-agent-id-conformance.py`:
«enrollment references missing/retired models»); там оба случая — failure, здесь
оба — error, трактовка одной и той же строки совпадает.

`validate(&Catalog) -> Vec<Issue>` где `Issue { severity: Error|Warning, code:
"V1".., message }`. Ошибки не прерывают валидацию — собираем все.

## 5. Архитектура (вариант A)

- **`arbiter-core/src/catalog/mod.rs`** — чистая логика, без I/O:
  - типы (§3), `CatalogError` (thiserror);
  - `parse_catalog(toml_text: &str) -> Result<Catalog, CatalogError>`;
  - `validate(&Catalog) -> Vec<Issue>`;
  - `resolve_path(env: impl Fn(&str) -> Option<String>, home: Option<&Path>)
    -> Result<PathBuf, CatalogError>` — env и home инжектируются, функция
    чистая и тестируемая без реального окружения. Проверка существования файла —
    на стороне вызывающего (I/O).
- **`arbiter-cli/src/main.rs`** — сабкоманда `catalog`: читает env/файл,
  вызывает core, печатает результат. Вывод команд в stdout (CLI — не MCP-канал),
  ошибки в stderr.
- Зависимость: `toml` уже объявлен в `[workspace.dependencies]` (Cargo.toml:19);
  единственное действие — `toml = { workspace = true }` в `arbiter-core/Cargo.toml`.
  Правило «no I/O в core» не нарушено — модуль работает со строками и
  инжектированным env.

## 6. CLI

```
arbiter-cli catalog path    # куда резолвится путь (или fail-loud ошибка), exit 0/1
arbiter-cli catalog check   # резолюция + чтение + парсинг + валидация;
                            # печатает issues; exit 0 (ок/warnings) / 1 (ошибки или нет конфига)
arbiter-cli catalog list    # таблица: agent_id | tested | routable | model status;
                            # exit 0/1 как у check
```

Ненулевой exit при отсутствующем конфиге — это и есть fail-loud поверхность
arbiter'а до появления runtime-потребителя.

**Механика диспетча:** расширяем существующий hand-rolled разбор
(`args[1] == "catalog"` → `match args[2]`), по образцу текущего `bench`.
`clap` **не** вводим: три сабкоманды не оправдывают новую зависимость,
стиль репы — минимум deps.

## 7. Тесты

- **Парсинг/валидация** (unit, `arbiter-core`):
  - happy-path: тест читает **вендорную копию `config/agents-catalog.toml`
    напрямую** (через `CARGO_MANIFEST_DIR/../config/...`) — НЕ ручную копию в
    фикстуру. Третьего дрейфующего артефакта не появляется (§8 Риск №1);
    тесты — не shipped-код, читать in-repo dev-вендор им можно. Ассерты — на
    инварианты (парсится, 0 errors, есть routable-агенты), не на точные счётчики.
  - битые случаи — фикстуры в `arbiter-core/tests/fixtures/catalog/`:
    `retired_ref.toml` (V3), `unknown_harness.toml` (V1), `unknown_model.toml` (V2),
    `dup_agent.toml` (V4), `routable_conflict.toml` (V5), `deprecated_ref.toml` (V6),
    `unknown_enum.toml` (V7 — парсится, warning);
  - пустой файл / битый TOML → `CatalogError`;
  - незнакомое поле → парсится без ошибки.
- **Резолюция** (unit): все ветки D2 через инжектированный env
  (`ATP_CATALOG` задан; только XDG; только home; ничего → ошибка с текстом,
  содержащим и `$ATP_CATALOG`, и XDG-путь).
- **CLI** (интеграционный smoke): `catalog check` на фикстуре через
  `$ATP_CATALOG`, проверка exit code — в существующем стиле тестов репы.
- TDD: тесты пишутся до реализации (plan-фаза разобьёт на шаги).

## 8. Риски / открытые вопросы

- **Схема-дрейф трёх загрузчиков** (риск №1 ADR-003b): митигация — фикстуры
  оформлены для будущего шаринга; conformance-тест — отдельная задача в devtools.
- **ADR-003b всё ещё Proposed**: эта реализация — первый исполненный пункт его
  рекомендаций; при ратификации обновить статус ADR ссылкой на этот дизайн.
- **`<eco>`-namespace `atp/`** зафиксирован здесь; если владельцы ATP/Maestro
  выберут иное имя до их реализаций — правка одной константы + тестов.
