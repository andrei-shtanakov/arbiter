# ТЗ: Arbiter — Policy Engine MCP Server (MVP)

**Версия:** 1.1
**Дата:** 2026-02-07
**Основа:** mcp-policy-engine-design.md v1.0
**Scope:** Phase 1 (MVP) — минимальный работающий продукт
**Репозиторий:** `arbiter`

---

## 1. Цель и scope

### 1.1 Цель

Создать MCP-сервер на Rust (`arbiter`), который принимает от Agent Orchestrator
описание кодинг-задачи и возвращает решение: какому агенту (Claude Code, Codex CLI,
Aider) отдать задачу, с какими параметрами, и почему.

### 1.2 Что входит в MVP

- MCP-сервер со stdio transport (JSON-RPC 2.0)
- 2 обязательных tool: `route_task`, `report_outcome`
- 1 информационный tool: `get_agent_status`
- Decision Tree inference (переиспользование из `arbiter-core`)
- 10 invariant rules с cascade fallback
- Agent registry с 3 агентами
- SQLite persistence для stats и decision log
- Expert-policy bootstrap tree
- Python MCP client для Orchestrator
- Smoke / integration / benchmark тесты

### 1.3 Что НЕ входит в MVP

- HTTP SSE transport (Phase 2)
- `evaluate_strategy` tool (Phase 3)
- Hot reload дерева (Phase 2)
- Retraining pipeline (Phase 2, только logging в MVP)
- Dashboard / TUI (Phase 4)
- Docker Compose deployment (Phase 4)
- ONNX backend (уже есть в PoC, но не критичен для MVP)

### 1.4 Определения

| Термин | Значение |
|---|---|
| Arbiter | Rust MCP-сервер, принимающий решения о маршрутизации задач |
| Orchestrator | Python daemon, управляющий задачами, зависимостями, git, запуском агентов |
| Agent | Внешний кодинг-инструмент (Claude Code, Codex CLI, Aider) |
| Decision Tree | Обученная sklearn/XGBoost модель, экспортированная в JSON |
| Invariant | Правило безопасности, проверяемое перед выполнением решения |
| Expert Policy | Начальный набор правил, кодирующий экспертные знания |

---

## 2. Контракт с существующим AI-OS PoC

AI-OS PoC (`ai-os-poc/`) содержит работающий Decision Tree inference, Invariant Layer
и Model Registry на Rust. Arbiter переиспользует core-логику, адаптируя её под
контекст кодинг-агентов вместо ML-моделей.

### 2.1 Что переиспользуется из `arbiter-core` напрямую

| Модуль | Файл | Что берём | Адаптация |
|---|---|---|---|
| Decision Tree inference | `policy/decision_tree.rs` | Парсинг sklearn JSON, traversal, decision path | Без изменений — подключаем как dependency |
| Policy Engine wrapper | `policy/engine.rs` | DT + ONNX fallback | Расширяем: добавляем multi-agent evaluation |
| Metrics | `metrics.rs` | Atomic counters, Prometheus | Без изменений |

### 2.2 Что адаптируется

| Модуль | Файл | Текущее состояние (AI-OS PoC) | Что меняем (Arbiter) |
|---|---|---|---|
| FeatureVector | `types.rs` | 26-dim для ML-моделей (GPU, VRAM, queue) | Новый 22-dim вектор для кодинг-агентов (task_type, language, complexity, agent_stats) |
| PolicyAction | `types.rs` | Enum: RouteToModel, ScaleUp, ScaleDown, Reject, Fallback | Новый enum: Assign(agent_id), Reject(reason), Fallback(agent_id, reason) |
| Invariant rules | `invariant/rules.rs` | 7 правил для ML-инфраструктуры (GPU capacity, VRAM) | 10 новых правил для агент-оркестрации (scope isolation, branch lock, concurrency) |
| Registry | `registry/lifecycle.rs` | 8-state FSM для ML-моделей, in-memory | 4-state FSM для агентов (inactive, active, busy, failed) + SQLite persistence |

### 2.3 Что пишется с нуля

| Компонент | Описание |
|---|---|
| `arbiter-mcp` crate | MCP server binary, stdio transport, JSON-RPC handler |
| MCP protocol layer | `initialize`, `tools/list`, `tools/call` handlers |
| `route_task` tool | Feature extraction → DT inference → invariant check → response |
| `report_outcome` tool | Outcome recording, stats update |
| `get_agent_status` tool | Agent registry query |
| SQLite layer | Schema, migrations, CRUD для outcomes/stats/decisions |
| Feature builder | Raw task JSON → 22-dim numeric vector |
| Agent config loader | TOML parser для agents.toml, invariants.toml |
| Bootstrap tree trainer | Python script: expert rules → sklearn tree → JSON export |
| Python MCP client | `ArbiterClient` class для Orchestrator |

### 2.4 Cargo workspace layout

```
arbiter/                             # Repository root
├── Cargo.toml                       # Workspace members: arbiter-core, arbiter-mcp,
│                                    #   arbiter-server, arbiter-cli
├── arbiter-core/                    # Shared library (evolved from aios-core)
│   └── src/
│       ├── types.rs                 # MODIFY: add AgentFeatureVector, AgentAction
│       ├── policy/
│       │   ├── decision_tree.rs     # REUSE as-is
│       │   ├── onnx.rs             # REUSE as-is
│       │   └── engine.rs           # MODIFY: add evaluate_for_agents()
│       ├── invariant/
│       │   └── rules.rs            # MODIFY: add 10 agent-specific rules
│       ├── registry/
│       │   └── lifecycle.rs        # MODIFY: add AgentState FSM
│       └── metrics.rs              # REUSE as-is
│
├── arbiter-mcp/                     # NEW — MCP Server binary
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs                  # Entry: parse args, init, run stdio loop
│       ├── server.rs                # MCP protocol: JSON-RPC dispatch
│       ├── tools/
│       │   ├── mod.rs
│       │   ├── route_task.rs
│       │   ├── report_outcome.rs
│       │   └── agent_status.rs
│       ├── features.rs              # Task JSON → FeatureVector
│       ├── agents.rs                # Agent registry + stats (backed by SQLite)
│       ├── db.rs                    # SQLite schema, migrations, queries
│       └── config.rs                # TOML config loader
│
├── arbiter-server/                  # HTTP API (Axum) — from AI-OS PoC, untouched in MVP
├── arbiter-cli/                     # CLI testing — from AI-OS PoC
├── config/                          # NEW
│   ├── agents.toml
│   └── invariants.toml
├── models/
│   ├── demo_tree.json              # Existing (AI-OS PoC demo)
│   └── agent_policy_tree.json      # NEW — bootstrap tree for agents
├── scripts/
│   ├── export_sklearn_tree.py      # Existing
│   └── bootstrap_agent_tree.py     # NEW — expert policy → tree
└── orchestrator/                    # NEW — Python client + integration
    ├── arbiter_client.py            # MCP client wrapper
    └── tests/
        └── test_arbiter_integration.py
```

---

## 3. Схема данных (SQLite)

### 3.1 Файл БД

Путь: задаётся через `--db <path>` (по умолчанию `./arbiter.db`).

### 3.2 Таблицы

```sql
-- Версионирование схемы
CREATE TABLE IF NOT EXISTS schema_version (
    version     INTEGER PRIMARY KEY,
    applied_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Зарегистрированные агенты и их текущее состояние
CREATE TABLE IF NOT EXISTS agents (
    id                TEXT PRIMARY KEY,           -- "claude_code", "codex_cli", "aider"
    display_name      TEXT NOT NULL,
    state             TEXT NOT NULL DEFAULT 'active'
                      CHECK (state IN ('active', 'inactive', 'busy', 'failed')),
    max_concurrent    INTEGER NOT NULL DEFAULT 2,
    running_tasks     INTEGER NOT NULL DEFAULT 0,
    config_json       TEXT NOT NULL,              -- serialized agent config from TOML
    updated_at        TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Агрегированная статистика (обновляется при каждом report_outcome)
CREATE TABLE IF NOT EXISTS agent_stats (
    agent_id          TEXT NOT NULL,
    task_type         TEXT NOT NULL,              -- "feature", "bugfix", etc.
    language          TEXT NOT NULL,              -- "python", "rust", etc.
    total_tasks       INTEGER NOT NULL DEFAULT 0,
    successful_tasks  INTEGER NOT NULL DEFAULT 0,
    failed_tasks      INTEGER NOT NULL DEFAULT 0,
    total_duration_min REAL NOT NULL DEFAULT 0.0,
    total_cost_usd    REAL NOT NULL DEFAULT 0.0,
    total_tokens      INTEGER NOT NULL DEFAULT 0,
    last_failure_at   TEXT,
    updated_at        TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (agent_id, task_type, language),
    FOREIGN KEY (agent_id) REFERENCES agents(id)
);

-- Каждое принятое решение (аудит + данные для retraining)
CREATE TABLE IF NOT EXISTS decisions (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id           TEXT NOT NULL,
    timestamp         TEXT NOT NULL DEFAULT (datetime('now')),
    -- Input
    task_json         TEXT NOT NULL,               -- полный JSON задачи от Orchestrator
    feature_vector    TEXT NOT NULL,               -- JSON array of 22 floats
    constraints_json  TEXT,                        -- constraints от Orchestrator
    -- Decision
    chosen_agent      TEXT NOT NULL,
    action            TEXT NOT NULL CHECK (action IN ('assign', 'reject', 'fallback')),
    confidence        REAL NOT NULL,
    decision_path     TEXT NOT NULL,               -- JSON array of strings
    fallback_agent    TEXT,
    fallback_reason   TEXT,
    -- Invariant results
    invariants_json   TEXT NOT NULL,               -- JSON array of check results
    invariants_passed INTEGER NOT NULL,            -- count of passed
    invariants_failed INTEGER NOT NULL,            -- count of failed
    -- Timing
    inference_us      INTEGER NOT NULL             -- tree inference time in microseconds
);

-- Результаты выполнения задач (feedback loop)
CREATE TABLE IF NOT EXISTS outcomes (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id           TEXT NOT NULL,
    decision_id       INTEGER NOT NULL,
    agent_id          TEXT NOT NULL,
    timestamp         TEXT NOT NULL DEFAULT (datetime('now')),
    -- Result
    status            TEXT NOT NULL CHECK (status IN ('success', 'failure', 'timeout', 'cancelled')),
    duration_min      REAL,
    tokens_used       INTEGER,
    cost_usd          REAL,
    exit_code         INTEGER,
    files_changed     INTEGER,
    tests_passed      INTEGER,                     -- 0 or 1
    validation_passed INTEGER,                     -- 0 or 1
    error_summary     TEXT,
    retry_count       INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (decision_id) REFERENCES decisions(id),
    FOREIGN KEY (agent_id) REFERENCES agents(id)
);

-- Индексы для быстрых запросов
CREATE INDEX IF NOT EXISTS idx_decisions_task ON decisions(task_id);
CREATE INDEX IF NOT EXISTS idx_decisions_agent ON decisions(chosen_agent);
CREATE INDEX IF NOT EXISTS idx_decisions_ts ON decisions(timestamp);
CREATE INDEX IF NOT EXISTS idx_outcomes_task ON outcomes(task_id);
CREATE INDEX IF NOT EXISTS idx_outcomes_agent ON outcomes(agent_id);
CREATE INDEX IF NOT EXISTS idx_outcomes_status ON outcomes(status);
CREATE INDEX IF NOT EXISTS idx_outcomes_ts ON outcomes(timestamp);
```

### 3.3 Миграции

При запуске `arbiter` проверяет `schema_version`. Если таблица отсутствует или
версия < текущей, применяет миграции последовательно. MVP использует одну миграцию
(v1 — создание всех таблиц).

---

## 4. Спецификация компонентов

### 4.1 MCP Server (`arbiter-mcp/src/server.rs`)

**Протокол:** JSON-RPC 2.0 over stdio (stdin/stdout), по одному JSON-объекту на строку.

**Lifecycle:**

1. Orchestrator запускает `arbiter` как subprocess
2. Orchestrator отправляет `initialize` → сервер отвечает capabilities
3. Orchestrator отправляет `initialized` notification
4. Далее — `tools/list` и `tools/call` по необходимости
5. При завершении Orchestrator закрывает stdin → сервер завершается gracefully

**Обязательные MCP methods:**

| Method | Direction | Описание |
|---|---|---|
| `initialize` | client → server | Handshake, exchange capabilities |
| `initialized` | client → server | Notification: handshake complete |
| `tools/list` | client → server | Return list of 3 tools with schemas |
| `tools/call` | client → server | Execute a tool |

**Server capabilities response:**

```json
{
  "capabilities": {
    "tools": {}
  },
  "serverInfo": {
    "name": "arbiter",
    "version": "0.1.0"
  }
}
```

**Acceptance criteria:**

- AC-4.1.1: Сервер запускается за < 500ms, включая загрузку дерева и SQLite init
- AC-4.1.2: Сервер корректно обрабатывает `initialize` → `initialized` → `tools/list`
- AC-4.1.3: Сервер возвращает JSON-RPC error (-32601) для неизвестных methods
- AC-4.1.4: Сервер возвращает JSON-RPC error (-32602) для невалидных params
- AC-4.1.5: Сервер корректно завершается при EOF на stdin (exit code 0)
- AC-4.1.6: Все сообщения в stderr (логи), никогда в stdout (protocol only)

### 4.2 Tool: `route_task` (`arbiter-mcp/src/tools/route_task.rs`)

**Input/Output:** см. design document, секция 2.1.

**Алгоритм:**

```
1. Validate input JSON against schema
2. Load current agent states from registry
3. Filter agents by hard constraints:
   a. agent supports task_type
   b. agent supports language
   c. agent has available slots (running_tasks < max_concurrent)
   d. agent not in excluded_agents
4. For each candidate agent:
   a. Build 22-dim feature vector (task features + agent stats + system state)
   b. Run Decision Tree inference → score
5. Rank agents by score, select top
6. Run 10 invariant checks against selected agent
7. If critical invariant fails:
   a. Try next-best agent (up to max_fallback_attempts)
   b. If all fail → return action="reject"
8. Log decision to SQLite (decisions table)
9. Increment running_tasks for chosen agent
10. Return decision with full audit trail
```

**Acceptance criteria:**

- AC-4.2.1: С 3 активными агентами и пустой историей, `route_task` возвращает решение за < 5ms
- AC-4.2.2: Если preferred_agent указан и доступен, он выбирается (при прочих равных confidence +0.1 boost)
- AC-4.2.3: Если все агенты excluded → action="reject", reasoning содержит причину
- AC-4.2.4: Если выбранный агент имеет scope conflict → fallback на следующего
- AC-4.2.5: Decision записывается в SQLite с полным feature vector и decision path
- AC-4.2.6: Response всегда содержит invariant_checks с результатами всех 10 правил
- AC-4.2.7: При невалидном input JSON → JSON-RPC error с описанием проблемы
- AC-4.2.8: Confidence ∈ [0.0, 1.0], decision_path — непустой массив строк
- AC-4.2.9: running_tasks инкрементируется при action="assign" или action="fallback"

### 4.3 Tool: `report_outcome` (`arbiter-mcp/src/tools/report_outcome.rs`)

**Input/Output:** см. design document, секция 2.2.

**Алгоритм:**

```
1. Validate input JSON
2. Find corresponding decision in decisions table by task_id
3. Insert outcome into outcomes table
4. Update agent_stats:
   a. Increment total_tasks
   b. Increment successful_tasks or failed_tasks
   c. Add duration, cost, tokens to running totals
   d. If failure → update last_failure_at
5. Update agents.running_tasks (decrement by 1)
6. Check if agent should transition to 'failed' state:
   a. If failures_last_24h > threshold → state='failed'
7. Return updated stats + retrain_suggested flag
```

**Acceptance criteria:**

- AC-4.3.1: После report_outcome, agent_stats отражает новые данные
- AC-4.3.2: Если task_id не найден в decisions → warning в response, но outcome записывается
- AC-4.3.3: running_tasks корректно декрементируется (никогда < 0)
- AC-4.3.4: При > 5 failures за 24h → retrain_suggested=true
- AC-4.3.5: Все поля outcome опциональны кроме status
- AC-4.3.6: Дублирующий report_outcome для того же task_id → записывается (idempotency через перезапись)

### 4.4 Tool: `get_agent_status` (`arbiter-mcp/src/tools/agent_status.rs`)

**Input/Output:** см. design document, секция 2.3.

**Acceptance criteria:**

- AC-4.4.1: Без параметров → возвращает все 3 агента
- AC-4.4.2: С agent_id → возвращает одного агента или error "agent not found"
- AC-4.4.3: Performance stats рассчитываются из agent_stats (не hardcoded)
- AC-4.4.4: by_language и by_type группировки корректны при пустой истории (пустые объекты)

### 4.5 Feature Builder (`arbiter-mcp/src/features.rs`)

Трансформация сырого JSON задачи + agent stats → 22-dim float vector.

**Encoding:**

| Feature | Encoding | Range |
|---|---|---|
| task_type | ordinal: feature=0, bugfix=1, refactor=2, test=3, docs=4, review=5, research=6 | [0, 6] |
| language | ordinal: python=0, rust=1, typescript=2, go=3, mixed=4, other=5 | [0, 5] |
| complexity | ordinal: trivial=0, simple=1, moderate=2, complex=3, critical=4 | [0, 4] |
| priority | ordinal: low=0, normal=1, high=2, urgent=3 | [0, 3] |
| scope_size | raw count, capped at 100 | [0, 100] |
| estimated_tokens | raw / 1000, capped at 200 | [0.0, 200.0] |
| has_dependencies | boolean | {0, 1} |
| requires_internet | boolean | {0, 1} |
| sla_minutes | raw, capped at 480 (8h) | [0, 480] |
| agent_success_rate | float from agent_stats (for this type+lang combo) | [0.0, 1.0] |
| agent_available_slots | max_concurrent - running_tasks | [0, 10] |
| agent_running_tasks | raw | [0, 10] |
| agent_avg_duration_min | from agent_stats | [0.0, 480.0] |
| agent_avg_cost_usd | from agent_stats | [0.0, 100.0] |
| agent_recent_failures | count from outcomes, last 24h | [0, 50] |
| agent_supports_task_type | boolean | {0, 1} |
| agent_supports_language | boolean | {0, 1} |
| total_running_tasks | sum across all agents | [0, 20] |
| total_pending_tasks | from Orchestrator (passed in context) | [0, 100] |
| budget_remaining_usd | from constraints or default | [0.0, 1000.0] |
| time_of_day_hour | current hour UTC | [0, 23] |
| concurrent_scope_conflicts | count of running tasks with overlapping scope | [0, 10] |

**Default values** (когда данные отсутствуют):

| Feature | Default | Reason |
|---|---|---|
| agent_success_rate | 0.5 | Neutral prior for new agents |
| agent_avg_duration_min | 15.0 | Conservative estimate |
| agent_avg_cost_usd | 0.10 | Mid-range estimate |
| estimated_tokens | 50.0 (= 50K tokens) | Typical task |
| budget_remaining_usd | config default (10.0) | From invariants.toml |
| scope_size | 1 | Minimum |

**Acceptance criteria:**

- AC-4.5.1: Для одного task + 3 агентов → строится ровно 3 вектора по 22 элемента
- AC-4.5.2: Все значения в указанных ranges (capping корректен)
- AC-4.5.3: При отсутствии optional полей → default values
- AC-4.5.4: Feature builder работает без SQLite (для unit-тестов, с mock stats)

### 4.6 Invariant Layer (`arbiter-core/src/invariant/rules.rs` — расширение)

10 правил. Каждое правило — функция `(action, system_state) → InvariantResult`.

```rust
pub struct InvariantResult {
    pub rule: String,          // rule name
    pub severity: Severity,    // Critical | Warning
    pub passed: bool,
    pub detail: String,        // human-readable explanation
}

pub enum Severity {
    Critical,  // blocks action, triggers fallback
    Warning,   // logged, action proceeds
}
```

**Правила:**

| # | Rule ID | Severity | Input | Logic | Failure message |
|---|---|---|---|---|---|
| 1 | `agent_available` | Critical | agent state + slots | agent.state == "active" AND running < max_concurrent | "Agent {id} unavailable: state={state}, slots={available}" |
| 2 | `scope_isolation` | Critical | task scope + running tasks' scopes | intersection(task.scope, running_scopes) == ∅ | "Scope conflict: {files} shared with task {other_id}" |
| 3 | `branch_not_locked` | Critical | task branch + running tasks' branches | task.branch ∉ running_branches | "Branch {branch} locked by task {other_id}" |
| 4 | `concurrency_limit` | Critical | total running | total_running < max_total_concurrent | "Concurrency limit: {running}/{max}" |
| 5 | `budget_remaining` | Warning | estimated cost + remaining | estimated_cost ≤ budget_remaining | "Budget: need ${cost}, have ${remaining}" |
| 6 | `retry_limit` | Warning | task retry count | retry_count < max_retries | "Retry limit: attempt {n}/{max}" |
| 7 | `rate_limit` | Warning | API calls this minute | calls < rate_limit_per_minute | "Rate limit: {calls}/{limit} calls/min" |
| 8 | `agent_health` | Warning | recent failures | failures_24h < max_failures_per_agent_24h | "Agent {id}: {n} failures in 24h (limit: {max})" |
| 9 | `task_compatible` | Warning | agent capabilities | agent supports language AND task_type | "Agent {id} doesn't support {lang}/{type}" |
| 10 | `sla_feasible` | Warning | estimated duration × buffer | estimated_duration × sla_buffer ≤ sla_minutes | "SLA risk: est {est}min × {buf} > {sla}min" |

**Cascade fallback при Critical violation:**

```
1. Выбранный агент не прошёл critical check
2. Берём следующего по score из DT ranking
3. Прогоняем invariants
4. Если прошёл → assign с fallback_reason
5. Если не прошёл → повторяем (до max_fallback_attempts=2)
6. Если все провалились → action="reject"
```

**Acceptance criteria:**

- AC-4.6.1: Critical violation → action="fallback" или "reject", никогда "assign"
- AC-4.6.2: Warning violation → action="assign", warning в invariant_checks
- AC-4.6.3: Scope isolation проверяет пересечение на уровне файлов/директорий
- AC-4.6.4: Branch lock проверяет exact match branch name
- AC-4.6.5: Invariant check отрабатывает за < 1ms (все правила вместе)
- AC-4.6.6: Все 10 правил всегда выполняются и возвращаются в response (даже passed=true)

### 4.7 Agent Registry (`arbiter-mcp/src/agents.rs`)

**State FSM:**

```
  ┌──────────┐    register     ┌──────────┐
  │          │ ──────────────► │          │
  │ (absent) │                 │  active  │ ◄─── recover
  │          │                 │          │ ────►─┐
  └──────────┘                 └────┬─────┘       │
                                    │             │
                              spawn │        fail │
                                    ▼             │
                               ┌──────────┐       │
                               │   busy   │ ──────┘
                               │          │   (running=max OR
                               └────┬─────┘    health check fail)
                                    │
                                    │ all tasks complete
                                    ▼
                               back to active
```

В MVP: state управляется через running_tasks count:
- `running_tasks == 0` → active
- `0 < running_tasks < max_concurrent` → active (has capacity)
- `running_tasks == max_concurrent` → busy (no capacity)
- `failures_24h > threshold` → failed (manual recovery)

**Acceptance criteria:**

- AC-4.7.1: При запуске — загрузка агентов из agents.toml, запись в SQLite если не существуют
- AC-4.7.2: Stats запрашиваются агрегацией из agent_stats таблицы
- AC-4.7.3: running_tasks инкрементируется при route_task(assign), декрементируется при report_outcome
- AC-4.7.4: Невозможно уйти в running_tasks < 0

### 4.8 Config Loader (`arbiter-mcp/src/config.rs`)

**Файлы:**

- `config/agents.toml` — определения агентов (см. design document, секция 6.2)
- `config/invariants.toml` — пороги правил (см. design document, секция 6.3)

**Acceptance criteria:**

- AC-4.8.1: Отсутствующий config file → сервер не запускается, stderr: "Config not found: {path}"
- AC-4.8.2: Невалидный TOML → сервер не запускается, stderr: parse error с line number
- AC-4.8.3: Неизвестные поля в TOML → игнорируются с warning в stderr
- AC-4.8.4: Отсутствующие обязательные поля → error с описанием какое поле пропущено

---

## 5. Error Handling

### 5.1 Категории ошибок

| Категория | Примеры | Поведение |
|---|---|---|
| **Startup fatal** | Дерево не загружается, SQLite не открывается, config невалиден | Сервер не запускается, exit code 1, stderr описывает проблему |
| **Protocol error** | Невалидный JSON-RPC, неизвестный method | JSON-RPC error response, сервер продолжает работу |
| **Tool input error** | Невалидные параметры tool | JSON-RPC error -32602 с описанием проблемы |
| **Runtime recoverable** | SQLite write fails (disk full), agent stats inconsistent | Tool возвращает результат с warning, stderr log |
| **Runtime degraded** | Дерево не может принять решение (все фичи default) | Fallback на hardcoded round-robin, warning в response |

### 5.2 Конкретные сценарии

| Сценарий | Поведение |
|---|---|
| Arbiter не запустился (crash при старте) | Orchestrator ловит subprocess exit, логирует stderr, переходит в fallback: round-robin assignment без policy |
| Arbiter упал во время работы | Orchestrator ловит broken pipe, перезапускает сервер, pending route_task → retry через 1s |
| SQLite locked (concurrent access) | Retry с backoff (50ms, 100ms, 200ms), max 3 attempts. Если все failed → tool возвращает результат без записи в DB, warning в response |
| Дерево не загрузилось, но config ОК | Сервер запускается в degraded mode, route_task использует hardcoded rules (round-robin по capable agents), warning в каждом response |
| Все агенты state=failed | route_task возвращает action="reject", reasoning="All agents unhealthy" |
| Unknown task_type или language | Используются defaults (task_type=0, language=5="other"), warning в response |
| report_outcome для неизвестного task_id | Outcome записывается, decision_id=NULL, warning "No matching decision found" |
| stdin EOF (Orchestrator shutdown) | Сервер flush'ит SQLite WAL, closes cleanly, exit code 0 |

### 5.3 Orchestrator fallback mode

Если Arbiter недоступен, Orchestrator переключается на встроенный round-robin:

```python
class FallbackScheduler:
    """Used when Arbiter is unavailable"""
    AGENT_ORDER = ["claude_code", "codex_cli", "aider"]

    def __init__(self):
        self._index = 0

    def route(self, task: dict) -> str:
        agent = self.AGENT_ORDER[self._index % len(self.AGENT_ORDER)]
        self._index += 1
        return agent
```

---

## 6. Тестовые сценарии

### 6.1 Unit Tests (Rust, `cargo test`)

| ID | Компонент | Сценарий | Expected |
|---|---|---|---|
| UT-01 | Feature builder | Полный task JSON → 22-dim vector | Все 22 значения в корректных ranges |
| UT-02 | Feature builder | Минимальный task (только required fields) → vector | Default values для optional fields |
| UT-03 | Feature builder | task_type="unknown" → vector | task_type encoded as 6 (other), warning logged |
| UT-04 | Invariant: scope_isolation | Task scope=["src/main.rs"], running=["src/main.rs"] | passed=false, detail contains "src/main.rs" |
| UT-05 | Invariant: scope_isolation | Task scope=["src/lib.rs"], running=["src/main.rs"] | passed=true |
| UT-06 | Invariant: scope_isolation | Task scope=["src/"], running=["src/main.rs"] | passed=false (directory contains file) |
| UT-07 | Invariant: concurrency | 5 running, limit 5 | passed=false |
| UT-08 | Invariant: concurrency | 4 running, limit 5 | passed=true |
| UT-09 | Invariant: budget | cost=0.50, remaining=0.30 | passed=false, detail shows amounts |
| UT-10 | Invariant: budget | cost=0.50, remaining=1.00 | passed=true |
| UT-11 | Invariant: branch_lock | branch="feature/x", running has "feature/x" | passed=false |
| UT-12 | Invariant: agent_health | 6 failures in 24h, threshold=5 | passed=false |
| UT-13 | Registry | Increment running_tasks from 0 → 1 | state still "active" |
| UT-14 | Registry | Increment running_tasks to max_concurrent | state "busy" |
| UT-15 | Registry | Decrement running_tasks below 0 → clamped | running_tasks=0, no panic |
| UT-16 | DB | Insert decision + query by task_id | Record found, all fields match |
| UT-17 | DB | Insert outcome + verify agent_stats update | Stats reflect new outcome |
| UT-18 | DB | Concurrent writes (2 threads) | No corruption, WAL handles contention |
| UT-19 | Config | Valid agents.toml → parsed config | 3 agents with all fields |
| UT-20 | Config | Missing required field → error | Error message names the field |
| UT-21 | DT | Bootstrap tree + known input → deterministic output | Same input always produces same agent choice |
| UT-22 | DT | All agents filtered out → empty candidates | Graceful: returns reject action |

### 6.2 Integration Tests (Rust, `cargo test --test integration`)

| ID | Сценарий | Описание | Expected |
|---|---|---|---|
| IT-01 | Happy path | route_task → assign → report_outcome(success) | Decision logged, stats updated, success_rate reflects |
| IT-02 | Fallback | route_task, primary agent has scope conflict | Fallback agent assigned, fallback_reason populated |
| IT-03 | All rejected | route_task, all agents excluded | action="reject", all invariant_checks present |
| IT-04 | Cold start | route_task with zero history | Decision made using bootstrap tree defaults |
| IT-05 | Stats accumulation | 10× (route_task + report_outcome) | agent_stats correctly accumulated |
| IT-06 | Agent failure | 6× report_outcome(failure) for same agent in 24h | agent_health invariant fails, agent deprioritized |
| IT-07 | Concurrent routing | 3× route_task simultaneously (async) | No race conditions, running_tasks consistent |

### 6.3 MCP Protocol Tests (Python, `pytest`)

| ID | Сценарий | Описание | Expected |
|---|---|---|---|
| PT-01 | Handshake | initialize → initialized → tools/list | 3 tools returned with correct schemas |
| PT-02 | Route simple | tools/call route_task with minimal task | Valid response with decision |
| PT-03 | Route + Report | Full cycle: route → report success | Stats updated, second route reflects history |
| PT-04 | Invalid params | tools/call with missing required field | JSON-RPC error -32602 |
| PT-05 | Unknown tool | tools/call with name="nonexistent" | JSON-RPC error -32601 |
| PT-06 | Server crash recovery | Kill server mid-operation, restart | Orchestrator reconnects, state preserved in SQLite |
| PT-07 | Large batch | 100× route_task sequentially | All succeed, total time < 2s |

### 6.4 Benchmark Tests (Rust, `cargo run --bin arbiter-cli`)

| ID | Metric | Target | Measurement |
|---|---|---|---|
| BT-01 | route_task throughput | > 10,000 decisions/sec | 10K route_task calls (in-process, no MCP overhead) |
| BT-02 | route_task e2e latency | < 5ms p99 | Over MCP stdio, including serialization |
| BT-03 | report_outcome latency | < 10ms p99 | Including SQLite write |
| BT-04 | Memory usage | < 50MB RSS | With loaded tree + 10K decisions in DB |
| BT-05 | SQLite size after 10K decisions | < 10MB | With full audit trail |

---

## 7. Bootstrap Tree

### 7.1 Expert Rules

Минимальный набор правил для холодного старта (расширяемый):

| # | Conditions | Agent | Rationale |
|---|---|---|---|
| 1 | complexity ∈ {complex, critical} AND language=rust | claude_code | Best Rust performance |
| 2 | complexity ∈ {complex, critical} AND language=python | claude_code | Best for complex tasks |
| 3 | type=docs OR type=review OR type=research | claude_code | Needs internet + tools |
| 4 | complexity ∈ {trivial, simple} AND type=bugfix | aider | Fast & cheap for simple fixes |
| 5 | complexity ∈ {trivial, simple} AND type=refactor | aider | Fast & cheap for refactors |
| 6 | language=typescript AND type=feature | codex_cli | Strong TS performance |
| 7 | language=go | codex_cli | Better Go support |
| 8 | complexity=moderate AND language=python | codex_cli | Good balance cost/quality |
| 9 | type=test AND complexity ≤ moderate | aider | Test writing is routine |
| 10 | DEFAULT (all others) | claude_code | Safest fallback |

### 7.2 Генерация

`scripts/bootstrap_agent_tree.py`:

1. Разворачивает 10 правил в ~500 обучающих примеров с вариациями
2. Добавляет шум в agent features (success_rate, duration) для robustness
3. Тренирует `DecisionTreeClassifier(max_depth=7, min_samples_leaf=10)`
4. Экспортирует в Arbiter JSON формат (совместимый с `arbiter-core::policy::decision_tree`)
5. Выводит accuracy, tree stats, confusion matrix

**Acceptance criteria:**

- AC-7.1: Bootstrap tree имеет accuracy > 95% на training data (expert rules)
- AC-7.2: Экспортированный JSON загружается в Rust без ошибок
- AC-7.3: Tree depth ≤ 7, node count ≤ 127

---

## 8. CLI Arguments

```
arbiter — Coding Agent Policy Engine (MCP Server)

USAGE:
    arbiter [OPTIONS]

OPTIONS:
    --tree <PATH>       Path to decision tree JSON [default: models/agent_policy_tree.json]
    --config <DIR>      Path to config directory [default: config/]
    --db <PATH>         Path to SQLite database [default: arbiter.db]
    --log-level <LEVEL> Log level: trace|debug|info|warn|error [default: info]
    --version           Print version
    --help              Print help
```

---

## 9. Зависимости

### 9.1 Rust crates (arbiter-mcp)

| Crate | Version | Purpose |
|---|---|---|
| `serde` + `serde_json` | 1.x | JSON serialization |
| `tokio` | 1.x | Async runtime (для stdin/stdout) |
| `rusqlite` | 0.31+ | SQLite (bundled feature) |
| `toml` | 0.8+ | Config parsing |
| `tracing` + `tracing-subscriber` | 0.1 / 0.3 | Structured logging (stderr) |
| `chrono` | 0.4+ | Timestamps |
| `arbiter-core` | workspace | DT inference, invariants, metrics |

Не добавляем MCP SDK — реализуем протокол вручную (он простой: JSON-RPC 2.0 over stdio,
3 метода). Это убирает тяжёлую dependency и даёт полный контроль.

### 9.2 Python (orchestrator/)

| Package | Purpose |
|---|---|
| `asyncio` | Subprocess management |
| `json` | MCP protocol |
| `sqlite3` | Orchestrator's own DB (не Arbiter DB) |
| `pytest` + `pytest-asyncio` | MCP protocol tests |

### 9.3 Python (scripts/)

| Package | Purpose |
|---|---|
| `scikit-learn` | Tree training |
| `numpy` | Data generation |
| `json` | Export |

---

## 10. Deployment

### 10.1 Claude Desktop / Claude Code интеграция

```json
{
  "mcpServers": {
    "arbiter": {
      "command": "/path/to/arbiter",
      "args": [
        "--tree", "/path/to/models/agent_policy_tree.json",
        "--config", "/path/to/config/",
        "--db", "/path/to/arbiter.db"
      ]
    }
  }
}
```

### 10.2 Orchestrator daemon интеграция

```python
from arbiter_client import ArbiterClient

client = ArbiterClient(
    binary_path="/path/to/arbiter",
    tree_path="models/agent_policy_tree.json",
    config_dir="config/",
    db_path="arbiter.db"
)
await client.start()
decision = await client.route_task(task_id="task-1", task={...})
```

---

## 11. Definition of Done

MVP считается завершённым когда:

- [ ] `cargo build --release` компилируется без warnings
- [ ] `cargo test` — все 22 unit tests pass
- [ ] `cargo test --test integration` — все 7 integration tests pass
- [ ] `pytest orchestrator/tests/` — все 7 MCP protocol tests pass
- [ ] `arbiter --help` выводит usage
- [ ] `arbiter` запускается, принимает MCP handshake, отвечает на tools/list
- [ ] `route_task` возвращает корректное решение для каждого из 10 expert rules
- [ ] `report_outcome` записывает в SQLite и обновляет stats
- [ ] `get_agent_status` возвращает корректную статистику после серии route+report
- [ ] Benchmark: > 10K decisions/sec in-process, < 5ms e2e over stdio
- [ ] Bootstrap tree генерируется, экспортируется, загружается в Rust
- [ ] README.md с quick start, architecture, примерами использования
- [ ] Код ревью: нет unsafe, нет unwrap() в production paths, все errors handled
