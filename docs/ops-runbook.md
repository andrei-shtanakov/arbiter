# Arbiter Operations Runbook

## Startup

```bash
# Build
cargo build --release

# Run with defaults
cargo run --release --bin arbiter-mcp

# Run with custom paths
cargo run --release --bin arbiter-mcp -- \
  --config config/ \
  --tree models/agent_policy_tree.json \
  --db arbiter.db \
  --log-level info
```

The server starts on stdio (JSON-RPC 2.0). On startup it:
1. Loads config from `config/agents.toml` and `config/invariants.toml`
2. Loads decision tree from `models/agent_policy_tree.json` (runs in round-robin mode if unavailable)
3. Opens SQLite database (creates if missing, runs migrations)
4. Purges records older than 90 days
5. Resets orphaned `running_tasks` counters (crash recovery)
6. Starts file watcher for hot-reloading config and tree

## Claude Desktop Setup

1. Copy `config/claude_desktop_config.json` to Claude Desktop settings
2. Replace `__ARBITER_DIR__` with the absolute path to the Arbiter project
3. Restart Claude Desktop

## Monitoring

### get_metrics
Returns decision counters, fallback rate, and latency statistics.

### get_budget_status
Returns total spend, remaining budget, and per-agent cost breakdown.

### get_agent_status
Returns per-agent state, capabilities, and performance history.

### Logs
Logs are structured OTel JSONL, one file per process:
`$ORCHESTRA_LOG_DIR/arbiter-<pid>.jsonl` (default `./logs/<pipeline_id>/`,
pipeline id from `ORCHESTRA_PIPELINE_ID` or a generated ULID). Level comes
from `--log-level` or `ORCHESTRA_LOG_LEVEL`; use `--log-level debug` for
verbose output. stderr carries only fatal pre-init errors and serves as a
fallback if log initialization fails.

Key log events (the `event` attribute in each JSONL record):
- `route.decision` — every routing decision with agent, confidence, latency
- `outcome.recorded` — every outcome with agent and status
- `arbiter.retention_purged` — retention cleanup on startup
- `arbiter.running_tasks_reset` — startup crash-recovery of stuck counters
- `mcp.ready` / `mcp.shutdown_requested` / `mcp.stdin_eof` — server lifecycle
- hot-reload messages from the file watcher are plain-text bodies (e.g.
  "decision tree reloaded successfully") without an `event` attribute

## Troubleshooting

### Server won't start
- Check config syntax: fatal startup errors (including parse errors) are printed to stderr before logging is initialized
- Check tree JSON: valid JSON with `n_features`, `n_classes`, `nodes` arrays
- Check DB permissions: Arbiter needs read/write to the DB path

### All tasks rejected
- Check `get_agent_status` — are agents in `failed` state?
- Check `get_metrics` — high `reject_rate`?
- Check invariant thresholds in `config/invariants.toml`
- Running tasks may be stuck: restart resets counters

### Performance degraded
- Check `get_metrics` latency stats
- Check DB size: `ls -la arbiter.db`
- Purge runs on startup; for immediate purge, restart the server

### Hot reload not working
- Check the JSONL log for watcher errors ("file watcher error" entries)
- Only `.toml` files in config dir and the exact tree JSON path are watched
- Invalid config/tree files are rejected (old state preserved)

## Retraining

```bash
# Retrain from expert rules only
uv run python scripts/bootstrap_agent_tree.py

# Retrain including real outcome data
uv run python scripts/bootstrap_agent_tree.py --from-db arbiter.db

# Evaluate tree quality
uv run python scripts/eval_tree.py
```

The tree file is hot-reloaded — no restart needed after retraining.

## Database Backup

```bash
# SQLite backup (safe while server is running, WAL mode)
sqlite3 arbiter.db ".backup arbiter-backup.db"

# Or simply copy (stop server first for consistency)
cp arbiter.db arbiter-backup.db
```

## Configuration Reference

### agents.toml
| Field | Type | Description |
|-------|------|-------------|
| display_name | string | Human-readable name |
| supports_languages | string[] | Languages the agent handles |
| supports_types | string[] | Task types the agent handles |
| max_concurrent | int | Max parallel tasks |
| cost_per_hour | float | Cost estimate (USD/hour) |
| avg_duration_min | float | Average task duration |

### invariants.toml
| Section | Field | Description |
|---------|-------|-------------|
| budget | threshold_usd | Budget limit for cost estimates |
| retries | max_retries | Max retry attempts per task |
| rate_limit | calls_per_minute | API rate limit |
| agent_health | max_failures_24h | Failure threshold for agent health |
| concurrency | max_total_concurrent | Max total running tasks |
| sla | buffer_multiplier | SLA duration buffer (e.g., 1.5x) |
