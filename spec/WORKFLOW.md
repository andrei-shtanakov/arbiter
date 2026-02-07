# Task Management Workflow

## Overview

The task management system works directly with the `spec/tasks.md` file:
- Statuses and checklists are updated in markdown
- Change history is logged in `.task-history.log`
- Dependencies are tracked automatically
- **Automatic execution via Claude CLI**

## Quick Start

```bash
# === Manual mode ===
make task-stats           # Statistics
make task-next            # What to do next
make task-start ID=TASK-001
make task-done ID=TASK-001

# === Automatic mode (Claude CLI) ===
make exec                 # Execute next task
make exec-all             # Execute all ready tasks
make exec-mvp             # Execute MVP tasks
make exec-status          # Execution status
```

---

## Automatic Execution (Claude CLI)

### Concept

The executor launches Claude CLI for each task:
1. Reads the specification (requirements.md, design.md)
2. Builds a prompt with task context
3. Claude implements code and tests
4. Validates the result (tests, lint)
5. On success — moves to the next task
6. On failure — retry with limit

### Commands

```bash
# Execute next ready task
python spec/executor.py run

# Execute a specific task
python spec/executor.py run --task=TASK-001

# Execute all ready tasks
python spec/executor.py run --all

# MVP tasks only
python spec/executor.py run --all --milestone=mvp

# Execution status
python spec/executor.py status

# Retry a failed task
python spec/executor.py retry TASK-001

# View logs
python spec/executor.py logs TASK-001

# Reset state
python spec/executor.py reset
```

### Safety Mechanisms

| Mechanism | Default | Description |
|----------|---------|----------|
| max_retries | 3 | Max attempts per task |
| max_consecutive_failures | 2 | Stop after N consecutive failures |
| task_timeout | 30 min | Timeout per task |
| post_done tests | ON | Run tests after completion |

### Logs

Logs are saved in `spec/.executor-logs/`:

```
spec/.executor-logs/
├── TASK-001-20260207-103000.log
├── TASK-001-20260207-103500.log  # retry
└── TASK-003-20260207-110000.log
```

---

## CLI Commands

### Viewing

```bash
python spec/task.py list              # All tasks
python spec/task.py list --status=todo
python spec/task.py list --priority=p0
python spec/task.py show TASK-001
python spec/task.py stats
python spec/task.py next
python spec/task.py graph
```

### Status Management

```bash
python spec/task.py start TASK-001
python spec/task.py done TASK-001
python spec/task.py block TASK-001
python spec/task.py check TASK-001 0   # Toggle checklist item
```

## Workflow

### 1. Choose a Task
```bash
python spec/task.py next
```

### 2. Start Working
```bash
python spec/task.py start TASK-100
```

### 3. Complete
```bash
python spec/task.py done TASK-100
```

### 4. Check Progress
```bash
python spec/task.py stats
```
