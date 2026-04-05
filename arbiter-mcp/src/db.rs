//! SQLite persistence layer for Arbiter.
//!
//! Stores decisions, outcomes, agent state, and aggregated stats.
//! Uses WAL mode for concurrent read/write performance and retries
//! with exponential backoff on lock contention.

use std::path::Path;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Retry configuration
// ---------------------------------------------------------------------------

/// Backoff delays for SQLite lock retry (50ms, 100ms, 200ms).
const RETRY_DELAYS_MS: [u64; 3] = [50, 100, 200];

/// Execute a closure with retry-on-lock backoff.
fn with_retry<F, T>(f: F) -> Result<T>
where
    F: Fn() -> Result<T>,
{
    let mut last_err = None;
    for (attempt, &delay_ms) in std::iter::once(&0u64)
        .chain(RETRY_DELAYS_MS.iter())
        .enumerate()
    {
        if attempt > 0 {
            warn!(attempt, delay_ms, "retrying after SQLite lock");
            thread::sleep(Duration::from_millis(delay_ms));
        }
        match f() {
            Ok(val) => return Ok(val),
            Err(e) => {
                if is_lock_error(&e) && attempt < RETRY_DELAYS_MS.len() {
                    last_err = Some(e);
                    continue;
                }
                return Err(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("retry exhausted")))
}

/// Check if an error is a SQLite lock/busy error.
fn is_lock_error(err: &anyhow::Error) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("database is locked") || msg.contains("busy")
}

// ---------------------------------------------------------------------------
// Data types for insert/query
// ---------------------------------------------------------------------------

/// Decision record for insert into the `decisions` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub task_id: String,
    pub task_json: String,
    pub feature_vector: String,
    pub constraints_json: Option<String>,
    pub chosen_agent: String,
    pub action: String,
    pub confidence: f64,
    pub decision_path: String,
    pub fallback_agent: Option<String>,
    pub fallback_reason: Option<String>,
    pub invariants_json: String,
    pub invariants_passed: i32,
    pub invariants_failed: i32,
    pub inference_us: i64,
}

/// Outcome record for insert into the `outcomes` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomeRecord {
    pub task_id: String,
    pub decision_id: Option<i64>,
    pub agent_id: String,
    pub status: String,
    pub duration_min: Option<f64>,
    pub tokens_used: Option<i64>,
    pub cost_usd: Option<f64>,
    pub exit_code: Option<i32>,
    pub files_changed: Option<i32>,
    pub tests_passed: Option<bool>,
    pub validation_passed: Option<bool>,
    pub error_summary: Option<String>,
    pub retry_count: i32,
}

/// Aggregated agent stats across all task types and languages.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentStats {
    pub agent_id: String,
    pub total_tasks: i64,
    pub successful_tasks: i64,
    pub failed_tasks: i64,
    pub success_rate: f64,
    pub avg_duration_min: f64,
    pub avg_cost_usd: f64,
    pub total_tokens: i64,
}

// ---------------------------------------------------------------------------
// Database
// ---------------------------------------------------------------------------

/// SQLite persistence layer.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (or create) a SQLite database at the given path with WAL mode.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open database: {}", path.display()))?;

        // Enable WAL mode for concurrent reads during writes.
        conn.pragma_update(None, "journal_mode", "WAL")
            .context("Failed to set WAL mode")?;

        // Reasonable busy timeout before our manual retry kicks in.
        conn.pragma_update(None, "busy_timeout", 1000)
            .context("Failed to set busy_timeout")?;

        // Enable foreign keys.
        conn.pragma_update(None, "foreign_keys", "ON")
            .context("Failed to enable foreign keys")?;

        // Performance PRAGMAs (safe with WAL mode).
        conn.pragma_update(None, "synchronous", "NORMAL")
            .context("Failed to set synchronous=NORMAL")?;

        conn.pragma_update(None, "cache_size", -8000_i64)
            .context("Failed to set cache_size")?;

        conn.pragma_update(None, "mmap_size", 67108864_i64)
            .context("Failed to set mmap_size")?;

        debug!(path = %path.display(), "database opened");
        Ok(Self { conn })
    }

    /// Open an in-memory database (for testing and benchmarks).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("Failed to open in-memory database")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(Self { conn })
    }

    /// Run schema migration to v1. Creates all 5 tables and 7 indices.
    ///
    /// Idempotent: uses `CREATE TABLE IF NOT EXISTS`.
    pub fn migrate(&self) -> Result<()> {
        self.conn
            .execute_batch(SCHEMA_V1)
            .context("Failed to apply schema v1")?;

        // Record schema version (ignore if already present).
        self.conn
            .execute(
                "INSERT OR IGNORE INTO schema_version (version) VALUES (?1)",
                params![1],
            )
            .context("Failed to record schema version")?;

        debug!("schema v1 applied");
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Decisions
    // -----------------------------------------------------------------------

    /// Insert a routing decision and return its auto-generated ID.
    pub fn insert_decision(&self, d: &DecisionRecord) -> Result<i64> {
        with_retry(|| {
            self.conn
                .execute(
                    "INSERT INTO decisions (
                        task_id, task_json, feature_vector,
                        constraints_json, chosen_agent, action,
                        confidence, decision_path,
                        fallback_agent, fallback_reason,
                        invariants_json, invariants_passed,
                        invariants_failed, inference_us
                    ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6,
                        ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
                    )",
                    params![
                        d.task_id,
                        d.task_json,
                        d.feature_vector,
                        d.constraints_json,
                        d.chosen_agent,
                        d.action,
                        d.confidence,
                        d.decision_path,
                        d.fallback_agent,
                        d.fallback_reason,
                        d.invariants_json,
                        d.invariants_passed,
                        d.invariants_failed,
                        d.inference_us,
                    ],
                )
                .context("Failed to insert decision")?;
            Ok(self.conn.last_insert_rowid())
        })
    }

    /// Find the most recent decision row ID for a task_id.
    pub fn find_decision_id_by_task(&self, task_id: &str) -> Result<Option<i64>> {
        let result: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM decisions WHERE task_id = ?1
                 ORDER BY id DESC LIMIT 1",
                params![task_id],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to query decision id")?;
        Ok(result)
    }

    /// Find a decision record by task_id.
    #[allow(dead_code)] // Used by integration tests.
    pub fn find_decision_by_task(&self, task_id: &str) -> Result<Option<DecisionRecord>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT task_id, task_json, feature_vector,
                    constraints_json, chosen_agent, action,
                    confidence, decision_path,
                    fallback_agent, fallback_reason,
                    invariants_json, invariants_passed,
                    invariants_failed, inference_us
             FROM decisions WHERE task_id = ?1
             ORDER BY id DESC LIMIT 1",
        )?;

        let result = stmt
            .query_row(params![task_id], |row| {
                Ok(DecisionRecord {
                    task_id: row.get(0)?,
                    task_json: row.get(1)?,
                    feature_vector: row.get(2)?,
                    constraints_json: row.get(3)?,
                    chosen_agent: row.get(4)?,
                    action: row.get(5)?,
                    confidence: row.get(6)?,
                    decision_path: row.get(7)?,
                    fallback_agent: row.get(8)?,
                    fallback_reason: row.get(9)?,
                    invariants_json: row.get(10)?,
                    invariants_passed: row.get(11)?,
                    invariants_failed: row.get(12)?,
                    inference_us: row.get(13)?,
                })
            })
            .optional()
            .context("Failed to query decision")?;

        Ok(result)
    }

    // -----------------------------------------------------------------------
    // Outcomes
    // -----------------------------------------------------------------------

    /// Insert a task outcome record.
    pub fn insert_outcome(&self, o: &OutcomeRecord) -> Result<()> {
        with_retry(|| {
            self.conn
                .execute(
                    "INSERT INTO outcomes (
                        task_id, decision_id, agent_id, status,
                        duration_min, tokens_used, cost_usd,
                        exit_code, files_changed,
                        tests_passed, validation_passed,
                        error_summary, retry_count
                    ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                        ?8, ?9, ?10, ?11, ?12, ?13
                    )",
                    params![
                        o.task_id,
                        o.decision_id,
                        o.agent_id,
                        o.status,
                        o.duration_min,
                        o.tokens_used,
                        o.cost_usd,
                        o.exit_code,
                        o.files_changed,
                        o.tests_passed.map(|b| if b { 1 } else { 0 }),
                        o.validation_passed.map(|b| if b { 1 } else { 0 }),
                        o.error_summary,
                        o.retry_count,
                    ],
                )
                .context("Failed to insert outcome")?;
            Ok(())
        })
    }

    // -----------------------------------------------------------------------
    // Agent stats
    // -----------------------------------------------------------------------

    /// Update agent_stats from an outcome (upsert by agent_id + task_type + language).
    ///
    /// Increments counters and running totals.
    pub fn update_agent_stats(
        &self,
        agent_id: &str,
        task_type: &str,
        language: &str,
        outcome: &OutcomeRecord,
    ) -> Result<()> {
        let is_success = outcome.status == "success";
        let is_failure = outcome.status == "failure" || outcome.status == "timeout";

        with_retry(|| {
            self.conn
                .execute(
                    "INSERT INTO agent_stats (
                        agent_id, task_type, language,
                        total_tasks, successful_tasks, failed_tasks,
                        total_duration_min, total_cost_usd, total_tokens,
                        last_failure_at
                    ) VALUES (
                        ?1, ?2, ?3,
                        1,
                        ?4,
                        ?5,
                        ?6,
                        ?7,
                        ?8,
                        CASE WHEN ?5 > 0 THEN datetime('now') ELSE NULL END
                    )
                    ON CONFLICT(agent_id, task_type, language)
                    DO UPDATE SET
                        total_tasks = total_tasks + 1,
                        successful_tasks = successful_tasks + ?4,
                        failed_tasks = failed_tasks + ?5,
                        total_duration_min = total_duration_min + ?6,
                        total_cost_usd = total_cost_usd + ?7,
                        total_tokens = total_tokens + ?8,
                        last_failure_at = CASE
                            WHEN ?5 > 0 THEN datetime('now')
                            ELSE last_failure_at
                        END,
                        updated_at = datetime('now')",
                    params![
                        agent_id,
                        task_type,
                        language,
                        if is_success { 1 } else { 0 },
                        if is_failure { 1 } else { 0 },
                        outcome.duration_min.unwrap_or(0.0),
                        outcome.cost_usd.unwrap_or(0.0),
                        outcome.tokens_used.unwrap_or(0),
                    ],
                )
                .context("Failed to update agent stats")?;
            Ok(())
        })
    }

    /// Get aggregated stats for an agent across all task types and languages.
    pub fn get_agent_stats(&self, agent_id: &str) -> Result<AgentStats> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT
                COALESCE(SUM(total_tasks), 0),
                COALESCE(SUM(successful_tasks), 0),
                COALESCE(SUM(failed_tasks), 0),
                COALESCE(SUM(total_duration_min), 0.0),
                COALESCE(SUM(total_cost_usd), 0.0),
                COALESCE(SUM(total_tokens), 0)
             FROM agent_stats WHERE agent_id = ?1",
        )?;

        let stats = stmt
            .query_row(params![agent_id], |row| {
                let total_tasks: i64 = row.get(0)?;
                let successful_tasks: i64 = row.get(1)?;
                let failed_tasks: i64 = row.get(2)?;
                let total_duration: f64 = row.get(3)?;
                let total_cost: f64 = row.get(4)?;
                let total_tokens: i64 = row.get(5)?;

                let success_rate = if total_tasks > 0 {
                    successful_tasks as f64 / total_tasks as f64
                } else {
                    0.0
                };
                let avg_duration = if total_tasks > 0 {
                    total_duration / total_tasks as f64
                } else {
                    0.0
                };
                let avg_cost = if total_tasks > 0 {
                    total_cost / total_tasks as f64
                } else {
                    0.0
                };

                Ok(AgentStats {
                    agent_id: agent_id.to_string(),
                    total_tasks,
                    successful_tasks,
                    failed_tasks,
                    success_rate,
                    avg_duration_min: avg_duration,
                    avg_cost_usd: avg_cost,
                    total_tokens,
                })
            })
            .context("Failed to get agent stats")?;

        Ok(stats)
    }

    /// Get per-category stats for an agent, grouped by language and by task type.
    ///
    /// Returns `(by_language, by_type)` where each is a vec of
    /// `(category_name, total_tasks, successful_tasks)`.
    #[allow(clippy::type_complexity)]
    pub fn get_agent_stats_by_category(
        &self,
        agent_id: &str,
    ) -> Result<(Vec<(String, i64, i64)>, Vec<(String, i64, i64)>)> {
        // Aggregate by language
        let mut stmt = self.conn.prepare_cached(
            "SELECT language, SUM(total_tasks), SUM(successful_tasks)
             FROM agent_stats WHERE agent_id = ?1
             GROUP BY language",
        )?;
        let by_language = stmt
            .query_map(params![agent_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("Failed to query stats by language")?;

        // Aggregate by task type
        let mut stmt = self.conn.prepare_cached(
            "SELECT task_type, SUM(total_tasks), SUM(successful_tasks)
             FROM agent_stats WHERE agent_id = ?1
             GROUP BY task_type",
        )?;
        let by_type = stmt
            .query_map(params![agent_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("Failed to query stats by type")?;

        Ok((by_language, by_type))
    }

    // -----------------------------------------------------------------------
    // Health monitoring
    // -----------------------------------------------------------------------

    /// Count failures for an agent in the last N hours.
    pub fn get_recent_failures(&self, agent_id: &str, hours: u32) -> Result<u32> {
        let count: u32 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM outcomes
                 WHERE agent_id = ?1
                   AND status IN ('failure', 'timeout')
                   AND timestamp >= datetime('now', ?2)",
                params![agent_id, format!("-{hours} hours")],
                |row| row.get(0),
            )
            .context("Failed to count recent failures")?;
        Ok(count)
    }

    // -----------------------------------------------------------------------
    // Agent running tasks
    // -----------------------------------------------------------------------

    /// Increment running_tasks for an agent.
    pub fn increment_running_tasks(&self, agent_id: &str) -> Result<()> {
        with_retry(|| {
            let rows = self
                .conn
                .execute(
                    "UPDATE agents SET running_tasks = running_tasks + 1,
                                       updated_at = datetime('now')
                     WHERE id = ?1",
                    params![agent_id],
                )
                .context("Failed to increment running_tasks")?;
            if rows == 0 {
                anyhow::bail!("Agent not found: {agent_id}");
            }
            Ok(())
        })
    }

    /// Decrement running_tasks for an agent (floor at 0).
    pub fn decrement_running_tasks(&self, agent_id: &str) -> Result<()> {
        with_retry(|| {
            let rows = self
                .conn
                .execute(
                    "UPDATE agents SET running_tasks = MAX(running_tasks - 1, 0),
                                       updated_at = datetime('now')
                     WHERE id = ?1",
                    params![agent_id],
                )
                .context("Failed to decrement running_tasks")?;
            if rows == 0 {
                anyhow::bail!("Agent not found: {agent_id}");
            }
            Ok(())
        })
    }

    /// Get the running_tasks count for an agent.
    pub fn get_running_tasks(&self, agent_id: &str) -> Result<u32> {
        let count: u32 = self
            .conn
            .query_row(
                "SELECT running_tasks FROM agents WHERE id = ?1",
                params![agent_id],
                |row| row.get(0),
            )
            .context("Failed to get running_tasks")?;
        Ok(count)
    }

    /// Get total running tasks across all agents.
    pub fn get_total_running_tasks(&self) -> Result<u32> {
        let count: u32 = self
            .conn
            .query_row(
                "SELECT COALESCE(SUM(running_tasks), 0) FROM agents",
                [],
                |row| row.get(0),
            )
            .context("Failed to get total running_tasks")?;
        Ok(count)
    }

    // -----------------------------------------------------------------------
    // Agent CRUD (for AgentRegistry)
    // -----------------------------------------------------------------------

    /// Upsert an agent record. Inserts if new, updates config_json if existing.
    pub fn upsert_agent(
        &self,
        agent_id: &str,
        display_name: &str,
        max_concurrent: u32,
        config_json: &str,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO agents (id, display_name, max_concurrent, config_json)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET
                    display_name = ?2,
                    max_concurrent = ?3,
                    config_json = ?4,
                    updated_at = datetime('now')",
                params![agent_id, display_name, max_concurrent, config_json],
            )
            .context("Failed to upsert agent")?;
        Ok(())
    }

    /// Get agent state from the agents table.
    #[allow(dead_code)]
    pub fn get_agent_state(&self, agent_id: &str) -> Result<Option<String>> {
        let result: Option<String> = self
            .conn
            .query_row(
                "SELECT state FROM agents WHERE id = ?1",
                params![agent_id],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to get agent state")?;
        Ok(result)
    }

    /// List all agent IDs.
    #[allow(dead_code)]
    pub fn list_agent_ids(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT id FROM agents")?;
        let ids = stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()
            .context("Failed to list agents")?;
        Ok(ids)
    }

    /// Get a reference to the underlying connection (for testing).
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

// ---------------------------------------------------------------------------
// Schema SQL
// ---------------------------------------------------------------------------

const SCHEMA_V1: &str = "
CREATE TABLE IF NOT EXISTS schema_version (
    version     INTEGER PRIMARY KEY,
    applied_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS agents (
    id                TEXT PRIMARY KEY,
    display_name      TEXT NOT NULL,
    state             TEXT NOT NULL DEFAULT 'active'
                      CHECK (state IN ('active','inactive','busy','failed')),
    max_concurrent    INTEGER NOT NULL DEFAULT 2,
    running_tasks     INTEGER NOT NULL DEFAULT 0,
    config_json       TEXT NOT NULL,
    updated_at        TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS agent_stats (
    agent_id          TEXT NOT NULL,
    task_type         TEXT NOT NULL,
    language          TEXT NOT NULL,
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

CREATE TABLE IF NOT EXISTS decisions (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id           TEXT NOT NULL,
    timestamp         TEXT NOT NULL DEFAULT (datetime('now')),
    task_json         TEXT NOT NULL,
    feature_vector    TEXT NOT NULL,
    constraints_json  TEXT,
    chosen_agent      TEXT NOT NULL,
    action            TEXT NOT NULL CHECK (action IN ('assign','reject','fallback')),
    confidence        REAL NOT NULL,
    decision_path     TEXT NOT NULL,
    fallback_agent    TEXT,
    fallback_reason   TEXT,
    invariants_json   TEXT NOT NULL,
    invariants_passed INTEGER NOT NULL,
    invariants_failed INTEGER NOT NULL,
    inference_us      INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS outcomes (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id           TEXT NOT NULL,
    decision_id       INTEGER,
    agent_id          TEXT NOT NULL,
    timestamp         TEXT NOT NULL DEFAULT (datetime('now')),
    status            TEXT NOT NULL CHECK (status IN ('success','failure','timeout','cancelled')),
    duration_min      REAL,
    tokens_used       INTEGER,
    cost_usd          REAL,
    exit_code         INTEGER,
    files_changed     INTEGER,
    tests_passed      INTEGER,
    validation_passed INTEGER,
    error_summary     TEXT,
    retry_count       INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (decision_id) REFERENCES decisions(id),
    FOREIGN KEY (agent_id) REFERENCES agents(id)
);

-- Indices for efficient querying
CREATE INDEX IF NOT EXISTS idx_decisions_task ON decisions(task_id);
CREATE INDEX IF NOT EXISTS idx_decisions_agent ON decisions(chosen_agent);
CREATE INDEX IF NOT EXISTS idx_decisions_ts ON decisions(timestamp);
CREATE INDEX IF NOT EXISTS idx_outcomes_task ON outcomes(task_id);
CREATE INDEX IF NOT EXISTS idx_outcomes_agent ON outcomes(agent_id);
CREATE INDEX IF NOT EXISTS idx_outcomes_status ON outcomes(status);
CREATE INDEX IF NOT EXISTS idx_outcomes_ts ON outcomes(timestamp);
CREATE INDEX IF NOT EXISTS idx_agent_stats_agent_type ON agent_stats(agent_id, task_type);
";

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db
    }

    fn insert_test_agent(db: &Database, agent_id: &str) {
        db.upsert_agent(
            agent_id,
            "Test Agent",
            2,
            r#"{"display_name":"Test Agent"}"#,
        )
        .unwrap();
    }

    fn sample_decision() -> DecisionRecord {
        DecisionRecord {
            task_id: "task-001".to_string(),
            task_json: r#"{"type":"bugfix","language":"python","complexity":"simple","priority":"normal"}"#.to_string(),
            feature_vector: "[1.0,0.0,1.0,1.0,1.0,50.0,0.0,0.0,120.0,0.5,2.0,0.0,15.0,0.1,0.0,1.0,1.0,0.0,0.0,10.0,14.0,0.0]".to_string(),
            constraints_json: Some(r#"{"budget_remaining_usd":8.5}"#.to_string()),
            chosen_agent: "claude_code".to_string(),
            action: "assign".to_string(),
            confidence: 0.92,
            decision_path: r#"["node 0: feature[2] <= 2.5","leaf: class 0"]"#.to_string(),
            fallback_agent: None,
            fallback_reason: None,
            invariants_json: "[]".to_string(),
            invariants_passed: 10,
            invariants_failed: 0,
            inference_us: 42,
        }
    }

    fn sample_outcome(decision_id: i64) -> OutcomeRecord {
        OutcomeRecord {
            task_id: "task-001".to_string(),
            decision_id: Some(decision_id),
            agent_id: "claude_code".to_string(),
            status: "success".to_string(),
            duration_min: Some(12.5),
            tokens_used: Some(35000),
            cost_usd: Some(0.25),
            exit_code: Some(0),
            files_changed: Some(3),
            tests_passed: Some(true),
            validation_passed: Some(true),
            error_summary: None,
            retry_count: 0,
        }
    }

    // -- Schema migration --

    #[test]
    fn migrate_creates_all_tables() {
        let db = setup_db();
        let tables: Vec<String> = db
            .conn
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type='table' AND name NOT LIKE 'sqlite_%'
                 ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();

        assert!(tables.contains(&"schema_version".to_string()));
        assert!(tables.contains(&"agents".to_string()));
        assert!(tables.contains(&"agent_stats".to_string()));
        assert!(tables.contains(&"decisions".to_string()));
        assert!(tables.contains(&"outcomes".to_string()));
        assert_eq!(tables.len(), 5);
    }

    #[test]
    fn migrate_creates_indices() {
        let db = setup_db();
        let indices: Vec<String> = db
            .conn
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type='index' AND name LIKE 'idx_%'
                 ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();

        assert_eq!(indices.len(), 8);
        assert!(indices.contains(&"idx_decisions_task".to_string()));
        assert!(indices.contains(&"idx_decisions_agent".to_string()));
        assert!(indices.contains(&"idx_decisions_ts".to_string()));
        assert!(indices.contains(&"idx_outcomes_task".to_string()));
        assert!(indices.contains(&"idx_outcomes_agent".to_string()));
        assert!(indices.contains(&"idx_outcomes_status".to_string()));
        assert!(indices.contains(&"idx_outcomes_ts".to_string()));
        assert!(indices.contains(&"idx_agent_stats_agent_type".to_string()));
    }

    #[test]
    fn migrate_is_idempotent() {
        let db = setup_db();
        // Second migration should not fail.
        db.migrate().unwrap();

        let version: i32 = db
            .conn
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn wal_mode_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        db.migrate().unwrap();

        let mode: String = db
            .conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    // -- UT-16: Insert/query decision --

    #[test]
    fn insert_and_query_decision() {
        let db = setup_db();
        insert_test_agent(&db, "claude_code");

        let decision = sample_decision();
        let id = db.insert_decision(&decision).unwrap();
        assert!(id > 0);

        let found = db
            .find_decision_by_task("task-001")
            .unwrap()
            .expect("decision should exist");
        assert_eq!(found.task_id, "task-001");
        assert_eq!(found.chosen_agent, "claude_code");
        assert_eq!(found.action, "assign");
        assert!((found.confidence - 0.92).abs() < f64::EPSILON);
        assert_eq!(found.invariants_passed, 10);
        assert_eq!(found.invariants_failed, 0);
        assert_eq!(found.inference_us, 42);
    }

    #[test]
    fn find_decision_returns_none_for_unknown() {
        let db = setup_db();
        let result = db.find_decision_by_task("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn insert_decision_returns_incrementing_ids() {
        let db = setup_db();
        insert_test_agent(&db, "claude_code");

        let mut decision = sample_decision();
        let id1 = db.insert_decision(&decision).unwrap();
        decision.task_id = "task-002".to_string();
        let id2 = db.insert_decision(&decision).unwrap();
        assert_eq!(id2, id1 + 1);
    }

    // -- UT-17: Insert outcome + stats update --

    #[test]
    fn insert_outcome_and_update_stats() {
        let db = setup_db();
        insert_test_agent(&db, "claude_code");

        let decision = sample_decision();
        let decision_id = db.insert_decision(&decision).unwrap();

        let outcome = sample_outcome(decision_id);
        db.insert_outcome(&outcome).unwrap();

        // Update stats for the outcome.
        db.update_agent_stats("claude_code", "bugfix", "python", &outcome)
            .unwrap();

        let stats = db.get_agent_stats("claude_code").unwrap();
        assert_eq!(stats.total_tasks, 1);
        assert_eq!(stats.successful_tasks, 1);
        assert_eq!(stats.failed_tasks, 0);
        assert!((stats.success_rate - 1.0).abs() < f64::EPSILON);
        assert!((stats.avg_duration_min - 12.5).abs() < f64::EPSILON);
        assert!((stats.avg_cost_usd - 0.25).abs() < f64::EPSILON);
        assert_eq!(stats.total_tokens, 35000);
    }

    #[test]
    fn stats_accumulate_across_outcomes() {
        let db = setup_db();
        insert_test_agent(&db, "claude_code");

        let decision = sample_decision();
        let decision_id = db.insert_decision(&decision).unwrap();

        // Success outcome.
        let outcome1 = OutcomeRecord {
            task_id: "task-001".to_string(),
            decision_id: Some(decision_id),
            agent_id: "claude_code".to_string(),
            status: "success".to_string(),
            duration_min: Some(10.0),
            tokens_used: Some(20000),
            cost_usd: Some(0.20),
            exit_code: Some(0),
            files_changed: Some(2),
            tests_passed: Some(true),
            validation_passed: Some(true),
            error_summary: None,
            retry_count: 0,
        };
        db.insert_outcome(&outcome1).unwrap();
        db.update_agent_stats("claude_code", "bugfix", "python", &outcome1)
            .unwrap();

        // Failure outcome.
        let outcome2 = OutcomeRecord {
            task_id: "task-002".to_string(),
            decision_id: Some(decision_id),
            agent_id: "claude_code".to_string(),
            status: "failure".to_string(),
            duration_min: Some(5.0),
            tokens_used: Some(10000),
            cost_usd: Some(0.10),
            exit_code: Some(1),
            files_changed: Some(0),
            tests_passed: Some(false),
            validation_passed: Some(false),
            error_summary: Some("test failed".to_string()),
            retry_count: 0,
        };
        db.insert_outcome(&outcome2).unwrap();
        db.update_agent_stats("claude_code", "bugfix", "python", &outcome2)
            .unwrap();

        let stats = db.get_agent_stats("claude_code").unwrap();
        assert_eq!(stats.total_tasks, 2);
        assert_eq!(stats.successful_tasks, 1);
        assert_eq!(stats.failed_tasks, 1);
        assert!((stats.success_rate - 0.5).abs() < f64::EPSILON);
        assert!((stats.avg_duration_min - 7.5).abs() < f64::EPSILON);
        assert!((stats.avg_cost_usd - 0.15).abs() < f64::EPSILON);
        assert_eq!(stats.total_tokens, 30000);
    }

    #[test]
    fn stats_default_for_unknown_agent() {
        let db = setup_db();
        let stats = db.get_agent_stats("nonexistent").unwrap();
        assert_eq!(stats.total_tasks, 0);
        assert_eq!(stats.successful_tasks, 0);
        assert!((stats.success_rate - 0.0).abs() < f64::EPSILON);
    }

    // -- UT-18: Concurrent writes --

    #[test]
    fn concurrent_inserts_on_same_db() {
        // Test that multiple sequential writes to the same db work.
        // (True concurrent writes require separate connections, tested
        // with file-based DB below.)
        let db = setup_db();
        insert_test_agent(&db, "claude_code");

        for i in 0..10 {
            let mut d = sample_decision();
            d.task_id = format!("task-{i:03}");
            db.insert_decision(&d).unwrap();
        }

        // Verify all 10 are present.
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM decisions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 10);
    }

    #[test]
    fn concurrent_file_db_writes() {
        use std::sync::{Arc, Barrier};

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("concurrent.db");

        // Set up schema with first connection.
        let db = Database::open(&db_path).unwrap();
        db.migrate().unwrap();
        insert_test_agent(&db, "claude_code");
        drop(db);

        let barrier = Arc::new(Barrier::new(4));
        let mut handles = vec![];

        for thread_id in 0..4 {
            let path = db_path.clone();
            let barrier = barrier.clone();
            handles.push(thread::spawn(move || {
                let db = Database::open(&path).unwrap();
                barrier.wait();
                for i in 0..5 {
                    let mut d = sample_decision();
                    d.task_id = format!("task-t{thread_id}-{i}");
                    db.insert_decision(&d).unwrap();
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // All 20 decisions should be present.
        let db = Database::open(&db_path).unwrap();
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM decisions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 20);
    }

    // -- UT-13/UT-14: Running tasks increment/decrement --

    #[test]
    fn increment_running_tasks() {
        let db = setup_db();
        insert_test_agent(&db, "claude_code");

        assert_eq!(db.get_running_tasks("claude_code").unwrap(), 0);

        db.increment_running_tasks("claude_code").unwrap();
        assert_eq!(db.get_running_tasks("claude_code").unwrap(), 1);

        db.increment_running_tasks("claude_code").unwrap();
        assert_eq!(db.get_running_tasks("claude_code").unwrap(), 2);
    }

    #[test]
    fn decrement_running_tasks() {
        let db = setup_db();
        insert_test_agent(&db, "claude_code");

        db.increment_running_tasks("claude_code").unwrap();
        db.increment_running_tasks("claude_code").unwrap();
        assert_eq!(db.get_running_tasks("claude_code").unwrap(), 2);

        db.decrement_running_tasks("claude_code").unwrap();
        assert_eq!(db.get_running_tasks("claude_code").unwrap(), 1);

        db.decrement_running_tasks("claude_code").unwrap();
        assert_eq!(db.get_running_tasks("claude_code").unwrap(), 0);
    }

    #[test]
    fn decrement_running_tasks_floors_at_zero() {
        let db = setup_db();
        insert_test_agent(&db, "claude_code");

        assert_eq!(db.get_running_tasks("claude_code").unwrap(), 0);
        db.decrement_running_tasks("claude_code").unwrap();
        assert_eq!(db.get_running_tasks("claude_code").unwrap(), 0);
    }

    #[test]
    fn increment_unknown_agent_fails() {
        let db = setup_db();
        let result = db.increment_running_tasks("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn decrement_unknown_agent_fails() {
        let db = setup_db();
        let result = db.decrement_running_tasks("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn total_running_tasks() {
        let db = setup_db();
        insert_test_agent(&db, "agent_a");
        insert_test_agent(&db, "agent_b");

        db.increment_running_tasks("agent_a").unwrap();
        db.increment_running_tasks("agent_b").unwrap();
        db.increment_running_tasks("agent_b").unwrap();

        assert_eq!(db.get_total_running_tasks().unwrap(), 3);
    }

    // -- Recent failures --

    #[test]
    fn recent_failures_count() {
        let db = setup_db();
        insert_test_agent(&db, "claude_code");

        let decision = sample_decision();
        let decision_id = db.insert_decision(&decision).unwrap();

        // Insert a failure outcome.
        let outcome = OutcomeRecord {
            task_id: "task-fail".to_string(),
            decision_id: Some(decision_id),
            agent_id: "claude_code".to_string(),
            status: "failure".to_string(),
            duration_min: Some(5.0),
            tokens_used: None,
            cost_usd: None,
            exit_code: Some(1),
            files_changed: None,
            tests_passed: None,
            validation_passed: None,
            error_summary: Some("crashed".to_string()),
            retry_count: 0,
        };
        db.insert_outcome(&outcome).unwrap();

        // Insert a timeout outcome.
        let timeout = OutcomeRecord {
            task_id: "task-timeout".to_string(),
            decision_id: Some(decision_id),
            agent_id: "claude_code".to_string(),
            status: "timeout".to_string(),
            duration_min: Some(60.0),
            tokens_used: None,
            cost_usd: None,
            exit_code: None,
            files_changed: None,
            tests_passed: None,
            validation_passed: None,
            error_summary: None,
            retry_count: 0,
        };
        db.insert_outcome(&timeout).unwrap();

        // Insert a success outcome (should not count).
        let success = sample_outcome(decision_id);
        db.insert_outcome(&success).unwrap();

        let failures = db.get_recent_failures("claude_code", 24).unwrap();
        assert_eq!(failures, 2);
    }

    #[test]
    fn recent_failures_zero_for_no_failures() {
        let db = setup_db();
        insert_test_agent(&db, "claude_code");
        let failures = db.get_recent_failures("claude_code", 24).unwrap();
        assert_eq!(failures, 0);
    }

    // -- Agent CRUD --

    #[test]
    fn upsert_agent_insert_and_update() {
        let db = setup_db();
        db.upsert_agent("test", "Test v1", 2, r#"{"v":1}"#).unwrap();

        let state = db.get_agent_state("test").unwrap();
        assert_eq!(state, Some("active".to_string()));

        // Update config.
        db.upsert_agent("test", "Test v2", 3, r#"{"v":2}"#).unwrap();

        // State should still be active, running_tasks preserved.
        let state = db.get_agent_state("test").unwrap();
        assert_eq!(state, Some("active".to_string()));
    }

    #[test]
    fn list_agent_ids_returns_all() {
        let db = setup_db();
        insert_test_agent(&db, "agent_a");
        insert_test_agent(&db, "agent_b");
        insert_test_agent(&db, "agent_c");

        let mut ids = db.list_agent_ids().unwrap();
        ids.sort();
        assert_eq!(ids, vec!["agent_a", "agent_b", "agent_c"]);
    }

    #[test]
    fn decision_with_fallback() {
        let db = setup_db();
        insert_test_agent(&db, "claude_code");
        insert_test_agent(&db, "codex_cli");

        let decision = DecisionRecord {
            task_id: "task-fallback".to_string(),
            task_json: "{}".to_string(),
            feature_vector: "[]".to_string(),
            constraints_json: None,
            chosen_agent: "codex_cli".to_string(),
            action: "fallback".to_string(),
            confidence: 0.75,
            decision_path: "[]".to_string(),
            fallback_agent: Some("claude_code".to_string()),
            fallback_reason: Some("agent_available: Critical failure".to_string()),
            invariants_json: "[]".to_string(),
            invariants_passed: 8,
            invariants_failed: 2,
            inference_us: 100,
        };

        let id = db.insert_decision(&decision).unwrap();
        assert!(id > 0);

        let found = db.find_decision_by_task("task-fallback").unwrap().unwrap();
        assert_eq!(found.action, "fallback");
        assert_eq!(found.fallback_agent, Some("claude_code".to_string()));
        assert_eq!(
            found.fallback_reason,
            Some("agent_available: Critical failure".to_string())
        );
    }

    // -- Stats across task types / languages --

    #[test]
    fn stats_aggregate_across_types_and_languages() {
        let db = setup_db();
        insert_test_agent(&db, "claude_code");

        let decision = sample_decision();
        let decision_id = db.insert_decision(&decision).unwrap();

        let outcome1 = OutcomeRecord {
            task_id: "t1".to_string(),
            decision_id: Some(decision_id),
            agent_id: "claude_code".to_string(),
            status: "success".to_string(),
            duration_min: Some(10.0),
            tokens_used: Some(1000),
            cost_usd: Some(0.10),
            exit_code: Some(0),
            files_changed: Some(1),
            tests_passed: Some(true),
            validation_passed: Some(true),
            error_summary: None,
            retry_count: 0,
        };
        db.insert_outcome(&outcome1).unwrap();
        db.update_agent_stats("claude_code", "bugfix", "python", &outcome1)
            .unwrap();

        let outcome2 = OutcomeRecord {
            task_id: "t2".to_string(),
            decision_id: Some(decision_id),
            agent_id: "claude_code".to_string(),
            status: "success".to_string(),
            duration_min: Some(20.0),
            tokens_used: Some(2000),
            cost_usd: Some(0.20),
            exit_code: Some(0),
            files_changed: Some(2),
            tests_passed: Some(true),
            validation_passed: Some(true),
            error_summary: None,
            retry_count: 0,
        };
        db.insert_outcome(&outcome2).unwrap();
        db.update_agent_stats("claude_code", "feature", "rust", &outcome2)
            .unwrap();

        // get_agent_stats should aggregate across both rows.
        let stats = db.get_agent_stats("claude_code").unwrap();
        assert_eq!(stats.total_tasks, 2);
        assert_eq!(stats.successful_tasks, 2);
        assert!((stats.avg_duration_min - 15.0).abs() < f64::EPSILON);
        assert!((stats.avg_cost_usd - 0.15).abs() < f64::EPSILON);
        assert_eq!(stats.total_tokens, 3000);
    }
}
