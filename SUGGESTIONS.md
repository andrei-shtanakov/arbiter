# Предложения по доработке проекта arbiter

## 1. Добавление новых агентов

### Текущее состояние

Архитектура на 90% agent-agnostic — routing, invariant rules, feature vector (22-dim) не зависят от количества агентов. Всё работает через `HashMap<String, AgentConfig>`, не через enum.

### Что нужно для добавления нового агента

| Шаг | Файл | Усилия |
|-----|------|--------|
| Добавить секцию в config | `config/agents.toml` | 1 мин |
| Добавить expert rules | `scripts/bootstrap_agent_tree.py` (AGENTS list + новые правила) | 1-2 часа |
| Переобучить дерево | `uv run python scripts/bootstrap_agent_tree.py` | 1 мин |
| Обновить тесты | integration.rs, main.rs (hardcoded agent lists) | 30 мин |
| Перезапустить сервер | `cargo run --release` | обязательно |

### Переобучение модели

Переобучение **обязательно** при добавлении нового агента. Decision tree имеет `n_classes: 3` и `class_names` зашитые в JSON. Без переобучения дерево не знает о новом агенте.

Процесс быстрый: bootstrap скрипт генерирует ~500 примеров из expert rules, обучает sklearn DecisionTreeClassifier и экспортирует JSON за секунды.

Hot-reload дерева и конфигов сейчас нет (Phase 2) — нужен перезапуск.

---

## 2. Заимствования из других проектов монорепы

### 2.1. Observability из atp-platform (structured metrics)

- **Сейчас:** только stderr логи + SQLite audit
- **Взять:** паттерн Prometheus counters/histograms для `decisions/sec`, `latency_p99`, `fallback_rate`, `agent_success_rate`
- **Почему:** 3 метрики + endpoint `/metrics` — минимальные изменения, огромный рост visibility
- **Не брать:** OTel tracing целиком — overkill для single-process <5ms inference

### 2.2. Dashboard из Maestro (SSE + minimal web UI)

- **Сейчас:** нет UI, только `arbiter-cli bench`
- **Взять:** паттерн SSE event stream + single-page HTML с live decision flow, agent stats, routing heatmap
- **Почему:** Maestro делает это ~200 строками (FastAPI + SSE + vanilla JS)
- **Не брать:** полноценный React dashboard — перегруз

### 2.3. Hot-reload паттерн из openclaw (config watcher)

- **Сейчас:** config и tree загружаются один раз при старте
- **Взять:** file watcher + `Arc<RwLock<Config>>` для agents.toml и tree JSON
- **Почему:** openclaw перезагружает JSON5 config без рестарта — тот же паттерн для TOML + tree swap
- **Не брать:** plugin SDK/marketplace — не нужен для policy engine

### 2.4. Cost tracking из Maestro (per-agent USD)

- **Сейчас:** `cost_per_hour` в конфиге, но нет аккумуляции реальных затрат
- **Взять:** простой accumulator `actual_cost_usd` в outcomes table + budget enforcement в invariant rules
- **Почему:** уже есть `budget_available` invariant — нужно только подключить реальные данные

### 2.5. Provider-aware routing из codebuff → LLM fallback в агентах

- **Проблема:** arbiter маршрутизирует между CLI tools (Claude Code, Codex, Aider), но каждый tool = один LLM provider. Если provider down — agent unavailable.
- **Взять из codebuff:**
  - Awareness о provider health в feature vector: `provider_available: bool`, `provider_latency_p99: f32`
  - Invariant rule `provider_health`: check HTTP endpoint перед routing
  - Fallback в рамках одного агента: Claude Opus → Claude Sonnet → Codex
- **Реализация:** добавить 2-3 фичи в feature vector + новый invariant `provider_health`
- **Объём:** ~40 строк в `features.rs` + `invariant/rules.rs`
- **Не брать:** OpenRouter API routing — arbiter работает с CLI, не с API напрямую
- **Условие:** имеет смысл только если agents начнут вызывать LLM API напрямую (сейчас — только CLI)

### 2.6. Eval-driven tree validation из codebuff → A/B тестирование routing

- **Проблема:** DT обучается на expert rules (~500 примеров). Нет объективной метрики "правильности" routing.
- **Взять из codebuff:**
  - Git commit reconstruction methodology: дать задачу → агент выполняет → AI judge оценивает
  - Сравнить: DT routing vs random routing vs always-best-agent на одном benchmark suite
  - Метрика: `mean_completion_score` при DT routing > random
- **Реализация:**
  - Benchmark suite: 50 задач разной сложности
  - Для каждой: route через DT → execute → judge score
  - Сравнить с baseline (random assignment, always-claude)
- **Объём:** ~200 строк в `scripts/eval_routing.py`
- **Не брать:** полный eval framework — arbiter не coding assistant; достаточно скрипта

---

## 3. Что НЕ брать

| Паттерн | Источник | Причина отказа |
|---------|----------|---------------|
| DAG orchestration | Maestro, hive | arbiter не оркестратор, а router |
| MCP tools ecosystem | hive, klaw.sh | arbiter сам является MCP server |
| Container isolation | nanoclaw | нет execution, только routing decisions |
| Multi-channel support | openclaw, nullclaw | одна точка входа (MCP JSON-RPC) |
| Generator-based agents | codebuff | arbiter — stateless router (<5ms), генераторы для long-running processes |
| Best-of-N sampling | codebuff | DT inference детерминистичен, sampling не нужен |
| Streaming events | codebuff | single decision = one response, не stream |
| SDK / programmatic API | codebuff | MCP JSON-RPC клиент уже достаточен |
| ErrorOr pattern | codebuff | Rust `Result<T,E>` уже решает эту задачу |
| Propose pattern | codebuff | arbiter не модифицирует файлы |
