//! Database module — manages SQLite connections and schema migrations.

use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::error::CoreError;

/// Thread-safe wrapper around a SQLite connection.
///
/// On construction the connection is configured with production PRAGMAs
/// and all pending schema migrations are applied automatically.
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
    path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredDocumentChunk {
    pub content: String,
    pub start_offset: i64,
    pub end_offset: i64,
    pub metadata_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredDocumentMetadata {
    pub mime_type: String,
    pub metadata_json: String,
}

impl Database {
    #[cfg(test)]
    pub(crate) fn execute_batch_for_test(&self, sql: &str) -> Result<(), CoreError> {
        self.conn().execute_batch(sql)?;
        Ok(())
    }

    /// Open a file-backed database with WAL mode, PRAGMAs, and auto-migration.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, CoreError> {
        let mut conn = Connection::open(path.as_ref())?;
        Self::configure_connection(&conn)?;
        crate::migrations::run_migrations(&conn)?;
        if let Err(error) =
            crate::settings_schema_v2::migrate_legacy_agent_configs_on_open(&mut conn)
        {
            tracing::error!(
                error = %error,
                "Settings Schema V2 migration failed; keeping the V1 schema active"
            );
        }
        if let Err(error) = crate::capability_registry::migrate_registry_on_open(&mut conn) {
            disable_registry_reads(&conn, &error);
            tracing::error!(
                error = %error,
                "Capability Registry import failed; keeping legacy runtime reads available"
            );
        }
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: Some(path.as_ref().to_path_buf()),
        })
    }

    /// Open an in-memory database for testing.
    pub fn open_memory() -> Result<Self, CoreError> {
        let mut conn = Connection::open_in_memory()?;
        Self::configure_connection(&conn)?;
        crate::migrations::run_migrations(&conn)?;
        if let Err(error) =
            crate::settings_schema_v2::migrate_legacy_agent_configs_on_open(&mut conn)
        {
            tracing::error!(
                error = %error,
                "Settings Schema V2 migration failed; keeping the V1 schema active"
            );
        }
        if let Err(error) = crate::capability_registry::migrate_registry_on_open(&mut conn) {
            disable_registry_reads(&conn, &error);
            tracing::error!(
                error = %error,
                "Capability Registry import failed; keeping legacy runtime reads available"
            );
        }
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: None,
        })
    }

    /// Build the connection used by the bounded read lane. File-backed
    /// databases receive a separate query-only WAL reader; in-memory tests
    /// share the original connection because independent `:memory:` handles do
    /// not share state.
    pub(crate) fn read_only_lane(&self) -> Result<Self, CoreError> {
        let Some(path) = self.path.as_ref() else {
            return Ok(self.clone());
        };
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        for pragma in [
            "PRAGMA busy_timeout = 5000",
            "PRAGMA foreign_keys = ON",
            "PRAGMA query_only = ON",
            "PRAGMA cache_size = -32000",
            "PRAGMA mmap_size = 268435456",
        ] {
            let _ = conn.prepare(pragma)?.query([])?;
        }
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: Some(path.clone()),
        })
    }

    /// Get a reference to the connection (locked).
    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        match self.conn.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::error!(
                    "Database mutex was poisoned by an earlier panic; recovering inner connection"
                );
                poisoned.into_inner()
            }
        }
    }

    /// Return the file path of the database, if file-backed.
    pub fn db_path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn list_document_chunks_by_path(
        &self,
        file_path: &str,
    ) -> Result<Vec<StoredDocumentChunk>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT c.content, c.start_offset, c.end_offset, c.metadata_json
             FROM chunks c
             JOIN documents d ON d.id = c.document_id
             WHERE d.path = ?1
             ORDER BY c.chunk_index",
        )?;
        let rows = stmt.query_map(rusqlite::params![file_path], |row| {
            Ok(StoredDocumentChunk {
                content: row.get(0)?,
                start_offset: row.get(1)?,
                end_offset: row.get(2)?,
                metadata_json: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(CoreError::Database)
    }

    pub fn get_document_storage_metadata_by_path(
        &self,
        file_path: &str,
    ) -> Result<StoredDocumentMetadata, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT mime_type, metadata FROM documents WHERE path = ?1",
            rusqlite::params![file_path],
            |row| {
                Ok(StoredDocumentMetadata {
                    mime_type: row.get(0)?,
                    metadata_json: row.get(1)?,
                })
            },
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Document for path {file_path}"))
            }
            other => CoreError::Database(other),
        })
    }

    pub fn update_workflow_scheduler_event_created_at(
        &self,
        event_id: &str,
        created_at: &str,
    ) -> Result<(), CoreError> {
        let conn = self.conn();
        let updated = conn.execute(
            "UPDATE workflow_automation_scheduler_events SET created_at = ?2 WHERE id = ?1",
            rusqlite::params![event_id, created_at],
        )?;
        if updated == 0 {
            return Err(CoreError::NotFound(format!(
                "Workflow scheduler event {event_id}"
            )));
        }
        Ok(())
    }

    fn configure_connection(conn: &Connection) -> Result<(), CoreError> {
        // Use prepare + query for each PRAGMA individually.
        // Some PRAGMAs return result rows (journal_mode, journal_size_limit)
        // while others don't — query() handles both cases gracefully.
        for pragma in [
            "PRAGMA journal_mode = WAL",
            "PRAGMA busy_timeout = 5000",
            "PRAGMA foreign_keys = ON",
            "PRAGMA synchronous = NORMAL",
            "PRAGMA cache_size = -64000",
            "PRAGMA temp_store = MEMORY",
            "PRAGMA mmap_size = 268435456",
            "PRAGMA journal_size_limit = 67108864",
        ] {
            let _ = conn.prepare(pragma)?.query([])?;
        }
        Ok(())
    }
}

fn disable_registry_reads(conn: &Connection, _error: &CoreError) {
    let parity = serde_json::json!({
        "status": "blocked",
        "reasonCodes": ["registry_import_failed"],
        "errorCategory": "import_error",
    });
    if let Err(update_error) = conn.execute(
        "UPDATE registry_activation_state
         SET read_mode = 'legacy', parity_status = 'blocked', parity_json = ?1,
             rolled_back_at = datetime('now'), updated_at = datetime('now')",
        [parity.to_string()],
    ) {
        tracing::error!(
            error = %update_error,
            "Failed to force Capability Registry reads back to legacy mode"
        );
    }
}

impl Database {
    /// Get all chunks as `(chunk_id, content)` pairs.
    pub fn get_all_chunks(&self) -> Result<Vec<(String, String)>, crate::error::CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id, content FROM chunks ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get all chunks belonging to documents of a given source.
    pub fn get_chunks_for_source(
        &self,
        source_id: &str,
    ) -> Result<Vec<(String, String)>, crate::error::CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT c.id, c.content FROM chunks c
             JOIN documents d ON c.document_id = d.id
             WHERE d.source_id = ?1
             ORDER BY c.id",
        )?;
        let rows = stmt.query_map(rusqlite::params![source_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Count chunks belonging to a source.
    pub fn count_chunks_for_source(
        &self,
        source_id: &str,
    ) -> Result<usize, crate::error::CoreError> {
        let conn = self.conn();
        let count = conn.query_row(
            "SELECT COUNT(*) FROM chunks c
             JOIN documents d ON c.document_id = d.id
             WHERE d.source_id = ?1",
            rusqlite::params![source_id],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count.max(0) as usize)
    }

    /// Count all chunks without materializing their content.
    pub fn count_all_chunks(&self) -> Result<usize, crate::error::CoreError> {
        let conn = self.conn();
        let count = conn.query_row("SELECT COUNT(*) FROM chunks", [], |row| {
            row.get::<_, i64>(0)
        })?;
        Ok(count.max(0) as usize)
    }

    /// Count source chunks that do not yet have an embedding for `model`.
    pub fn count_chunks_without_embeddings_for_source(
        &self,
        source_id: &str,
        model: &str,
    ) -> Result<usize, crate::error::CoreError> {
        let conn = self.conn();
        let count = conn.query_row(
            "SELECT COUNT(*) FROM chunks c
             JOIN documents d ON c.document_id = d.id
             LEFT JOIN embeddings e ON c.id = e.chunk_id AND e.model = ?2
             WHERE d.source_id = ?1 AND e.chunk_id IS NULL",
            rusqlite::params![source_id, model],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count.max(0) as usize)
    }

    /// Fetch one bounded source-scoped page of chunks missing embeddings.
    ///
    /// Callers persist each page before fetching the next one, so repeatedly
    /// reading the first page is stable without retaining a growing cursor or
    /// the full corpus in memory.
    pub fn get_chunks_without_embeddings_for_source_batch(
        &self,
        source_id: &str,
        model: &str,
        limit: usize,
    ) -> Result<Vec<(String, String)>, crate::error::CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT c.id, c.content FROM chunks c
             JOIN documents d ON c.document_id = d.id
             LEFT JOIN embeddings e ON c.id = e.chunk_id AND e.model = ?2
             WHERE d.source_id = ?1 AND e.chunk_id IS NULL
             ORDER BY c.id
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![source_id, model, limit.max(1) as i64],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Fetch one bounded global page of chunks missing embeddings.
    pub fn get_chunks_without_embeddings_batch(
        &self,
        model: &str,
        limit: usize,
    ) -> Result<Vec<(String, String)>, crate::error::CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT c.id, c.content FROM chunks c
             LEFT JOIN embeddings e ON c.id = e.chunk_id AND e.model = ?1
             WHERE e.chunk_id IS NULL
             ORDER BY c.id
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![model, limit.max(1) as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Delete all embeddings for a given model.
    pub fn delete_all_embeddings(&self, model: &str) -> Result<usize, crate::error::CoreError> {
        let conn = self.conn();
        let count = conn.execute(
            "DELETE FROM embeddings WHERE model = ?1",
            rusqlite::params![model],
        )?;
        Ok(count)
    }
}

// ---------------------------------------------------------------------------
// Scan error tracking
// ---------------------------------------------------------------------------

impl Database {
    /// Record or update a scan error for a file.
    ///
    /// On conflict (same source+path), increments `error_count`, updates
    /// `last_failed_at` and `error_message`.
    pub fn upsert_scan_error(
        &self,
        source_id: &str,
        path: &str,
        error_message: &str,
    ) -> Result<(), crate::error::CoreError> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO scan_errors (source_id, path, error_message)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(source_id, path) DO UPDATE SET
                error_count = error_count + 1,
                error_message = excluded.error_message,
                last_failed_at = datetime('now')",
            rusqlite::params![source_id, path, error_message],
        )?;
        Ok(())
    }

    /// Delete all scan errors for a source (retry all).
    pub fn clear_scan_errors(&self, source_id: &str) -> Result<usize, crate::error::CoreError> {
        let conn = self.conn();
        let count = conn.execute(
            "DELETE FROM scan_errors WHERE source_id = ?1",
            rusqlite::params![source_id],
        )?;
        Ok(count)
    }

    /// Delete a single scan error (file recovered or manual reset).
    pub fn clear_scan_error(
        &self,
        source_id: &str,
        path: &str,
    ) -> Result<bool, crate::error::CoreError> {
        let conn = self.conn();
        let count = conn.execute(
            "DELETE FROM scan_errors WHERE source_id = ?1 AND path = ?2",
            rusqlite::params![source_id, path],
        )?;
        Ok(count > 0)
    }

    /// List all scan errors for a source.
    pub fn get_scan_errors(
        &self,
        source_id: &str,
    ) -> Result<Vec<crate::models::ScanError>, crate::error::CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT source_id, path, error_message, error_count, first_failed_at, last_failed_at
             FROM scan_errors WHERE source_id = ?1 ORDER BY last_failed_at DESC",
        )?;
        let rows = stmt.query_map(rusqlite::params![source_id], |row| {
            Ok(crate::models::ScanError {
                source_id: row.get(0)?,
                path: row.get(1)?,
                error_message: row.get(2)?,
                error_count: row.get(3)?,
                first_failed_at: row.get(4)?,
                last_failed_at: row.get(5)?,
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Check whether a file should be retried.
    ///
    /// Returns `true` (should retry) if:
    /// - No error record exists, OR
    /// - `error_count` < 3, OR
    /// - `last_failed_at` is older than 24 hours.
    pub fn should_retry_scan(
        &self,
        source_id: &str,
        path: &str,
    ) -> Result<bool, crate::error::CoreError> {
        let conn = self.conn();
        let result: Option<(i64, String)> = conn
            .query_row(
                "SELECT error_count, last_failed_at FROM scan_errors
                 WHERE source_id = ?1 AND path = ?2",
                rusqlite::params![source_id, path],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        match result {
            None => Ok(true), // no error record → retry
            Some((count, last_failed)) => {
                if count < 3 {
                    return Ok(true);
                }
                // Parse last_failed_at and check if older than 24 hours.
                if let Ok(dt) =
                    chrono::NaiveDateTime::parse_from_str(&last_failed, "%Y-%m-%d %H:%M:%S")
                {
                    let age = chrono::Utc::now().naive_utc() - dt;
                    if age > chrono::Duration::hours(24) {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Durable agent run event persistence
// ---------------------------------------------------------------------------

impl Database {
    pub(crate) fn agent_run_event_head(&self, run_id: &str) -> Result<(u64, bool), CoreError> {
        let conn = self.conn();
        Self::agent_run_event_head_on_connection(&conn, run_id)
    }

    pub(crate) fn agent_run_event_head_on_connection(
        connection: &rusqlite::Connection,
        run_id: &str,
    ) -> Result<(u64, bool), CoreError> {
        let (sequence, closed): (i64, bool) = connection.query_row(
            "SELECT COALESCE(MAX(event_seq), 0),
                    (EXISTS(
                       SELECT 1
                       FROM agent_run_events terminal
                       WHERE terminal.run_id = ?1
                         AND (
                           terminal.kind = 'error'
                           OR (
                             terminal.kind = 'done'
                             AND COALESCE(terminal.status, '') <> 'paused'
                           )
                         )
                     )
                     OR EXISTS(
                       SELECT 1
                       FROM agent_task_runs task
                       WHERE task.id = ?1
                         AND task.status IN ('completed', 'failed', 'timed_out', 'cancelled')
                     ))
             FROM agent_run_events
             WHERE run_id = ?1",
            rusqlite::params![run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let sequence = u64::try_from(sequence).map_err(|_| {
            CoreError::Internal(format!(
                "Agent run {run_id} has an invalid negative event sequence"
            ))
        })?;
        Ok((sequence, closed))
    }

    /// Persist one durable agent run event after validating the protocol contract.
    pub fn save_agent_run_event(
        &self,
        event: &crate::agent_run::AgentRunEvent,
    ) -> Result<(), CoreError> {
        event
            .validate_durable_contract()
            .map_err(|err| CoreError::InvalidInput(format!("invalid agent run event: {err}")))?;
        let event_seq = agent_event_seq_to_i64(event.event_seq)?;
        let payload_json = serde_json::to_string(&event.payload)?;

        let conn = self.conn();
        conn.execute(
            "INSERT INTO agent_run_events
             (run_id, turn_id, event_seq, version, kind, phase, visibility, persistence,
              display_kind, importance, label, status, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                event.run_id,
                event.turn_id,
                event_seq,
                event.version as i64,
                event.kind.as_str(),
                event.phase.as_str(),
                event.visibility.as_str(),
                event.persistence.as_str(),
                event.display_kind.as_str(),
                event.importance.as_str(),
                event.label,
                event.status,
                payload_json,
            ],
        )?;
        Ok(())
    }

    /// Persist a batch of durable agent run events in one transaction.
    pub fn save_agent_run_events(
        &self,
        events: &[crate::agent_run::AgentRunEvent],
    ) -> Result<(), CoreError> {
        if events.is_empty() {
            return Ok(());
        }

        let mut conn = self.conn();
        let tx = conn.transaction()?;
        insert_agent_run_events(&tx, events)?;
        tx.commit()?;
        Ok(())
    }

    /// Read durable agent run events for a run in event sequence order.
    pub fn list_agent_run_events(
        &self,
        run_id: &str,
    ) -> Result<Vec<crate::agent_run::AgentRunEvent>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT version, run_id, turn_id, event_seq, kind, phase, visibility, persistence,
                    display_kind, importance, label, status, payload_json, created_at
             FROM agent_run_events
             WHERE run_id = ?1
             ORDER BY event_seq ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![run_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, String>(12)?,
                row.get::<_, String>(13)?,
            ))
        })?;

        let mut events = Vec::new();
        for row in rows {
            let (
                version,
                run_id,
                turn_id,
                event_seq,
                kind,
                phase,
                visibility,
                persistence,
                display_kind,
                importance,
                label,
                status,
                payload_json,
                created_at,
            ) = row?;
            let event = crate::agent_run::AgentRunEvent {
                version: u16::try_from(version).map_err(|_| {
                    CoreError::Internal(format!(
                        "stored agent run event has invalid version {version}"
                    ))
                })?,
                run_id,
                turn_id,
                event_seq: u64::try_from(event_seq).map_err(|_| {
                    CoreError::Internal(format!(
                        "stored agent run event has invalid sequence {event_seq}"
                    ))
                })?,
                kind: crate::agent_run::AgentRunEventKind::from_wire(&kind).ok_or_else(|| {
                    CoreError::Internal(format!("stored agent run event has unknown kind '{kind}'"))
                })?,
                phase: crate::agent_run::AgentRunPhase::from_wire(&phase).ok_or_else(|| {
                    CoreError::Internal(format!(
                        "stored agent run event has unknown phase '{phase}'"
                    ))
                })?,
                visibility: crate::agent_run::AgentRunEventVisibility::from_wire(&visibility)
                    .ok_or_else(|| {
                        CoreError::Internal(format!(
                            "stored agent run event has unknown visibility '{visibility}'"
                        ))
                    })?,
                persistence: crate::agent_run::AgentRunEventPersistence::from_wire(&persistence)
                    .ok_or_else(|| {
                        CoreError::Internal(format!(
                            "stored agent run event has unknown persistence '{persistence}'"
                        ))
                    })?,
                display_kind: crate::agent_run::AgentRunDisplayKind::from_wire(&display_kind)
                    .ok_or_else(|| {
                        CoreError::Internal(format!(
                            "stored agent run event has unknown display kind '{display_kind}'"
                        ))
                    })?,
                importance: crate::agent_run::AgentRunEventImportance::from_wire(&importance)
                    .ok_or_else(|| {
                        CoreError::Internal(format!(
                            "stored agent run event has unknown importance '{importance}'"
                        ))
                    })?,
                label,
                status,
                payload: serde_json::from_str(&payload_json)?,
                created_at: Some(created_at),
            };
            event.validate_durable_contract().map_err(|err| {
                CoreError::Internal(format!("stored agent run event violates contract: {err}"))
            })?;
            events.push(event);
        }

        Ok(events)
    }
}

pub(crate) fn insert_agent_run_events(
    connection: &rusqlite::Connection,
    events: &[crate::agent_run::AgentRunEvent],
) -> Result<(), CoreError> {
    if events.is_empty() {
        return Ok(());
    }

    let mut prepared_events = Vec::with_capacity(events.len());
    for event in events {
        event
            .validate_durable_contract()
            .map_err(|err| CoreError::InvalidInput(format!("invalid agent run event: {err}")))?;
        prepared_events.push((
            event,
            agent_event_seq_to_i64(event.event_seq)?,
            serde_json::to_string(&event.payload)?,
        ));
    }

    let mut statement = connection.prepare(
        "INSERT INTO agent_run_events
         (run_id, turn_id, event_seq, version, kind, phase, visibility, persistence,
          display_kind, importance, label, status, payload_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
    )?;
    for (event, event_seq, payload_json) in prepared_events {
        statement.execute(rusqlite::params![
            event.run_id,
            event.turn_id,
            event_seq,
            event.version as i64,
            event.kind.as_str(),
            event.phase.as_str(),
            event.visibility.as_str(),
            event.persistence.as_str(),
            event.display_kind.as_str(),
            event.importance.as_str(),
            event.label,
            event.status,
            payload_json,
        ])?;
    }
    Ok(())
}

fn agent_event_seq_to_i64(event_seq: u64) -> Result<i64, CoreError> {
    i64::try_from(event_seq).map_err(|_| {
        CoreError::InvalidInput(format!(
            "agent run event_seq {event_seq} exceeds SQLite integer range"
        ))
    })
}

// ---------------------------------------------------------------------------
// Trajectory persistence
// ---------------------------------------------------------------------------

impl Database {
    /// Save or import a versioned trajectory fixture with indexed summary fields.
    pub fn save_agent_trajectory(
        &self,
        trajectory: &crate::trajectory::Trajectory,
    ) -> Result<crate::trajectory::TrajectoryStoreSummary, CoreError> {
        let mut trajectory = trajectory.clone();
        validate_trajectory_for_storage(&mut trajectory)?;
        let (source_kind, source_run_id) =
            crate::trajectory::trajectory_source_identity(&trajectory.trajectory_id);
        let redaction_profile = trajectory_redaction_profile_wire(trajectory.sanitization.profile)?;
        let trajectory_json = serde_json::to_string(&trajectory)?;
        let event_count = usize_to_i64(trajectory.metrics.event_count, "event_count")?;
        let tool_call_count = usize_to_i64(trajectory.metrics.tool_call_count, "tool_call_count")?;
        let approval_count = usize_to_i64(trajectory.metrics.approval_count, "approval_count")?;
        let task_run_count = usize_to_i64(trajectory.metrics.task_run_count, "task_run_count")?;

        let conn = self.conn();
        conn.execute(
            "INSERT OR REPLACE INTO agent_trajectories
             (trajectory_id, schema_version, source_kind, source_run_id, user_input_summary,
              outcome, event_count, tool_call_count, approval_count, task_run_count,
              redaction_profile, trajectory_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, datetime('now'))",
            rusqlite::params![
                &trajectory.trajectory_id,
                trajectory.schema_version as i64,
                &source_kind,
                &source_run_id,
                &trajectory.user_input_summary,
                &trajectory.outcome,
                event_count,
                tool_call_count,
                approval_count,
                task_run_count,
                &redaction_profile,
                &trajectory_json,
                &trajectory.created_at,
            ],
        )?;
        drop(conn);
        self.get_agent_trajectory_summary(&trajectory.trajectory_id)
    }

    pub fn load_agent_trajectory(
        &self,
        trajectory_id: &str,
    ) -> Result<crate::trajectory::Trajectory, CoreError> {
        let conn = self.conn();
        let trajectory_json = conn
            .query_row(
                "SELECT trajectory_json FROM agent_trajectories WHERE trajectory_id = ?1",
                rusqlite::params![trajectory_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => {
                    CoreError::NotFound(format!("Trajectory {trajectory_id}"))
                }
                other => CoreError::Database(other),
            })?;
        let mut trajectory: crate::trajectory::Trajectory = serde_json::from_str(&trajectory_json)?;
        validate_trajectory_for_storage(&mut trajectory)?;
        Ok(trajectory)
    }

    pub fn get_agent_trajectory_summary(
        &self,
        trajectory_id: &str,
    ) -> Result<crate::trajectory::TrajectoryStoreSummary, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT trajectory_id, schema_version, source_kind, source_run_id,
                    user_input_summary, outcome, event_count, tool_call_count,
                    approval_count, task_run_count, redaction_profile, created_at, updated_at
             FROM agent_trajectories
             WHERE trajectory_id = ?1",
            rusqlite::params![trajectory_id],
            trajectory_summary_from_row,
        )
        .map_err(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Trajectory {trajectory_id}"))
            }
            other => CoreError::Database(other),
        })
    }

    pub fn list_agent_trajectory_summaries(
        &self,
        limit: usize,
    ) -> Result<Vec<crate::trajectory::TrajectoryStoreSummary>, CoreError> {
        let bounded_limit = i64::try_from(limit.clamp(1, 500)).unwrap_or(500);
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT trajectory_id, schema_version, source_kind, source_run_id,
                    user_input_summary, outcome, event_count, tool_call_count,
                    approval_count, task_run_count, redaction_profile, created_at, updated_at
             FROM agent_trajectories
             ORDER BY datetime(created_at) DESC, datetime(updated_at) DESC, trajectory_id DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![bounded_limit],
            trajectory_summary_from_row,
        )?;
        let mut summaries = Vec::new();
        for row in rows {
            summaries.push(row?);
        }
        Ok(summaries)
    }
}

#[async_trait::async_trait]
impl crate::trajectory::TrajectoryStore for Database {
    async fn save_trajectory(
        &self,
        trajectory: crate::trajectory::Trajectory,
    ) -> Result<(), CoreError> {
        self.save_agent_trajectory(&trajectory).map(|_| ())
    }

    async fn load_trajectory(
        &self,
        trajectory_id: &str,
    ) -> Result<crate::trajectory::Trajectory, CoreError> {
        self.load_agent_trajectory(trajectory_id)
    }

    async fn list_trajectory_summaries(
        &self,
        limit: usize,
    ) -> Result<Vec<crate::trajectory::TrajectoryStoreSummary>, CoreError> {
        self.list_agent_trajectory_summaries(limit)
    }
}

fn validate_trajectory_for_storage(
    trajectory: &mut crate::trajectory::Trajectory,
) -> Result<(), CoreError> {
    if trajectory.trajectory_id.trim().is_empty() {
        return Err(CoreError::InvalidInput(
            "trajectory_id must not be empty".to_string(),
        ));
    }
    if trajectory.created_at.trim().is_empty() {
        return Err(CoreError::InvalidInput(
            "trajectory created_at must not be empty".to_string(),
        ));
    }
    if trajectory.schema_version != crate::trajectory::TRAJECTORY_SCHEMA_VERSION {
        return Err(CoreError::InvalidInput(format!(
            "unsupported trajectory schema version {}",
            trajectory.schema_version
        )));
    }
    trajectory.refresh_metrics();
    trajectory
        .validate_run_events()
        .map_err(|err| CoreError::InvalidInput(format!("invalid trajectory run event: {err}")))?;
    Ok(())
}

fn trajectory_redaction_profile_wire(
    profile: crate::trajectory::TrajectoryRedactionProfile,
) -> Result<String, CoreError> {
    let value = serde_json::to_value(profile)?;
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| CoreError::Internal("serialize trajectory redaction profile".to_string()))
}

fn trajectory_redaction_profile_from_wire(
    value: &str,
) -> Result<crate::trajectory::TrajectoryRedactionProfile, CoreError> {
    serde_json::from_value(serde_json::Value::String(value.to_string())).map_err(CoreError::from)
}

fn trajectory_summary_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<crate::trajectory::TrajectoryStoreSummary> {
    let redaction_profile: String = row.get(10)?;
    let redaction_profile =
        trajectory_redaction_profile_from_wire(&redaction_profile).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                10,
                rusqlite::types::Type::Text,
                Box::new(err),
            )
        })?;
    Ok(crate::trajectory::TrajectoryStoreSummary {
        trajectory_id: row.get(0)?,
        schema_version: u16::try_from(row.get::<_, i64>(1)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Integer,
                Box::new(err),
            )
        })?,
        source_kind: row.get(2)?,
        source_run_id: row.get(3)?,
        user_input_summary: row.get(4)?,
        outcome: row.get(5)?,
        event_count: i64_to_usize(row.get(6)?, 6)?,
        tool_call_count: i64_to_usize(row.get(7)?, 7)?,
        approval_count: i64_to_usize(row.get(8)?, 8)?,
        task_run_count: i64_to_usize(row.get(9)?, 9)?,
        redaction_profile,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

fn usize_to_i64(value: usize, field: &str) -> Result<i64, CoreError> {
    i64::try_from(value)
        .map_err(|_| CoreError::InvalidInput(format!("trajectory {field} exceeds SQLite integer")))
}

fn i64_to_usize(value: i64, column: usize) -> rusqlite::Result<usize> {
    usize::try_from(value).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Integer,
            Box::new(err),
        )
    })
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    // ── helpers ──────────────────────────────────────────────────────────

    fn new_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    #[test]
    fn agent_trace_errors_are_redacted_before_insert() {
        let db = Database::open_memory().unwrap();
        let mut trace = crate::trace::AgentTrace::begin(
            "redacted-conversation",
            "test",
            "gemini-test",
            128_000,
        );
        trace.finish(
            crate::trace::TraceOutcome::Error,
            Some(
                "request https://example.test/generate?key=AIza0123456789abcdefghijklmnopqrst"
                    .to_string(),
            ),
        );
        db.save_agent_trace(&trace).unwrap();

        let stored: (Option<String>, String) = db
            .conn()
            .query_row(
                "SELECT error_message, trace_json FROM agent_traces WHERE id = ?1",
                [&trace.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let encoded = format!("{} {}", stored.0.unwrap_or_default(), stored.1);
        assert!(!encoded.contains("AIza"));
        assert!(!encoded.to_ascii_lowercase().contains("?key="));
        assert!(encoded.contains("REDACTED"));
    }

    fn insert_source(conn: &Connection) -> String {
        let id = new_id();
        conn.execute(
            "INSERT INTO sources (id, kind, root_path) VALUES (?1, 'local_folder', ?2)",
            rusqlite::params![&id, format!("/tmp/src-{}", &id[..8])],
        )
        .expect("insert source");
        id
    }

    fn insert_document(conn: &Connection, source_id: &str) -> String {
        let id = new_id();
        conn.execute(
            "INSERT INTO documents (id, source_id, path, title, mime_type, file_size, modified_at, content_hash)
             VALUES (?1, ?2, ?3, 'Test Doc', 'text/plain', 1234, datetime('now'), 'hash123')",
            rusqlite::params![&id, source_id, format!("/tmp/doc-{}.md", &id[..8])],
        )
        .expect("insert document");
        id
    }

    fn insert_chunk(conn: &Connection, document_id: &str, content: &str) -> String {
        let id = new_id();
        conn.execute(
            "INSERT INTO chunks (id, document_id, chunk_index, kind, content, start_offset, end_offset, line_start, line_end, content_hash)
             VALUES (?1, ?2, 0, 'text', ?3, 0, ?4, 1, 10, 'chunkhash')",
            rusqlite::params![&id, document_id, content, content.len() as i64],
        )
        .expect("insert chunk");
        id
    }

    // ── tests ───────────────────────────────────────────────────────────

    #[test]
    fn test_database_new_memory() {
        let db = Database::open_memory().expect("open_memory should succeed");
        let conn = db.conn();

        let tables: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };

        for expected in &[
            "sources",
            "documents",
            "chunks",
            "fts_chunks",
            "playbooks",
            "playbook_citations",
            "query_logs",
            "_migrations",
        ] {
            assert!(
                tables.contains(&expected.to_string()),
                "table '{}' should exist, got: {:?}",
                expected,
                tables
            );
        }
    }

    #[test]
    fn test_database_migrations_idempotent() {
        let _db1 = Database::open_memory().expect("first open_memory should succeed");
        let _db2 = Database::open_memory().expect("second open_memory should succeed");
    }

    #[test]
    fn test_conn_recovers_from_poisoned_mutex() {
        let db = Database::open_memory().expect("open_memory should succeed");
        let db_clone = db.clone();

        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = db_clone.conn.lock().expect("lock should succeed");
            panic!("poison the mutex");
        }));

        let conn = db.conn();
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
                [],
                |row| row.get(0),
            )
            .expect("query after poison recovery should succeed");

        assert!(table_count > 0);
    }

    #[test]
    fn test_sources_crud() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        // Create
        let id = insert_source(&conn);

        // Read
        let kind: String = conn
            .query_row("SELECT kind FROM sources WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(kind, "local_folder");

        // Update
        conn.execute("UPDATE sources SET kind = 'remote' WHERE id = ?1", [&id])
            .unwrap();
        let kind: String = conn
            .query_row("SELECT kind FROM sources WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(kind, "remote");

        // Delete
        conn.execute("DELETE FROM sources WHERE id = ?1", [&id])
            .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sources WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_documents_crud() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        let source_id = insert_source(&conn);
        let doc_id = insert_document(&conn, &source_id);

        // Read
        let title: String = conn
            .query_row(
                "SELECT title FROM documents WHERE id = ?1",
                [&doc_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(title, "Test Doc");

        // Update
        conn.execute(
            "UPDATE documents SET title = 'Updated Doc' WHERE id = ?1",
            [&doc_id],
        )
        .unwrap();
        let title: String = conn
            .query_row(
                "SELECT title FROM documents WHERE id = ?1",
                [&doc_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(title, "Updated Doc");

        // Delete
        conn.execute("DELETE FROM documents WHERE id = ?1", [&doc_id])
            .unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE id = ?1",
                [&doc_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_chunks_crud() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        let source_id = insert_source(&conn);
        let doc_id = insert_document(&conn, &source_id);
        let chunk_id = insert_chunk(&conn, &doc_id, "chunk body text");

        // Read & verify offsets
        let (content, start, end): (String, i64, i64) = conn
            .query_row(
                "SELECT content, start_offset, end_offset FROM chunks WHERE id = ?1",
                [&chunk_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(content, "chunk body text");
        assert_eq!(start, 0);
        assert_eq!(end, "chunk body text".len() as i64);
    }

    #[test]
    fn test_fts5_insert_and_search() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        let source_id = insert_source(&conn);
        let doc_id = insert_document(&conn, &source_id);
        insert_chunk(
            &conn,
            &doc_id,
            "the quick brown fox jumps over the lazy dog",
        );

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts_chunks WHERE fts_chunks MATCH 'quick'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "FTS should find the inserted chunk");
    }

    #[test]
    fn test_fts5_auto_sync_on_delete() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        let source_id = insert_source(&conn);
        let doc_id = insert_document(&conn, &source_id);
        let chunk_id = insert_chunk(&conn, &doc_id, "unique_sentinel_word_alpha");

        // Verify FTS has it
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts_chunks WHERE fts_chunks MATCH 'unique_sentinel_word_alpha'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // Delete chunk
        conn.execute("DELETE FROM chunks WHERE id = ?1", [&chunk_id])
            .unwrap();

        // FTS should no longer find it
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts_chunks WHERE fts_chunks MATCH 'unique_sentinel_word_alpha'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "FTS should auto-remove on chunk delete");
    }

    #[test]
    fn test_fts5_auto_sync_on_update() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        let source_id = insert_source(&conn);
        let doc_id = insert_document(&conn, &source_id);
        let chunk_id = insert_chunk(&conn, &doc_id, "original_sentinel_beta");

        // Update content
        conn.execute(
            "UPDATE chunks SET content = 'replacement_sentinel_gamma' WHERE id = ?1",
            [&chunk_id],
        )
        .unwrap();

        // Old content gone
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts_chunks WHERE fts_chunks MATCH 'original_sentinel_beta'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "FTS should not find old content after update");

        // New content present
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts_chunks WHERE fts_chunks MATCH 'replacement_sentinel_gamma'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "FTS should find new content after update");
    }

    #[test]
    fn test_playbooks_crud() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        let id = new_id();

        // Create
        conn.execute(
            "INSERT INTO playbooks (id, title, body_md) VALUES (?1, 'My Playbook', '# Hello')",
            [&id],
        )
        .unwrap();

        // Read
        let title: String = conn
            .query_row("SELECT title FROM playbooks WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(title, "My Playbook");

        // Update
        conn.execute(
            "UPDATE playbooks SET title = 'Renamed Playbook' WHERE id = ?1",
            [&id],
        )
        .unwrap();
        let title: String = conn
            .query_row("SELECT title FROM playbooks WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(title, "Renamed Playbook");

        // Delete
        conn.execute("DELETE FROM playbooks WHERE id = ?1", [&id])
            .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM playbooks WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_playbook_citations_crud() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        let source_id = insert_source(&conn);
        let doc_id = insert_document(&conn, &source_id);
        let chunk_id = insert_chunk(&conn, &doc_id, "cited chunk content");

        let playbook_id = new_id();
        conn.execute(
            "INSERT INTO playbooks (id, title, body_md) VALUES (?1, 'Citation PB', '')",
            [&playbook_id],
        )
        .unwrap();

        let citation_id = new_id();
        conn.execute(
            "INSERT INTO playbook_citations (id, playbook_id, chunk_id, sort_order, annotation)
             VALUES (?1, ?2, ?3, 1, 'important note')",
            rusqlite::params![&citation_id, &playbook_id, &chunk_id],
        )
        .unwrap();

        // Read back
        let annotation: String = conn
            .query_row(
                "SELECT annotation FROM playbook_citations WHERE id = ?1",
                [&citation_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(annotation, "important note");
    }

    #[test]
    fn test_cascade_delete_source() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        let source_id = insert_source(&conn);
        let doc_id = insert_document(&conn, &source_id);
        insert_chunk(&conn, &doc_id, "cascade test content");

        // Delete source — should cascade to documents and chunks
        conn.execute("DELETE FROM sources WHERE id = ?1", [&source_id])
            .unwrap();

        let doc_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE source_id = ?1",
                [&source_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(doc_count, 0, "documents should be cascade-deleted");

        let chunk_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE document_id = ?1",
                [&doc_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(chunk_count, 0, "chunks should be cascade-deleted");
    }

    #[test]
    fn test_cascade_delete_document() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        let source_id = insert_source(&conn);
        let doc_id = insert_document(&conn, &source_id);
        insert_chunk(&conn, &doc_id, "document cascade chunk");

        // Delete document — should cascade to chunks
        conn.execute("DELETE FROM documents WHERE id = ?1", [&doc_id])
            .unwrap();

        let chunk_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE document_id = ?1",
                [&doc_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(chunk_count, 0, "chunks should be cascade-deleted");
    }

    #[test]
    fn test_query_logs_insert() {
        let db = Database::open_memory().unwrap();
        let conn = db.conn();

        let id = new_id();
        conn.execute(
            "INSERT INTO query_logs (id, query_text, result_count, duration_ms)
             VALUES (?1, 'how to deploy?', 5, 42)",
            [&id],
        )
        .unwrap();

        let (query_text, result_count, duration): (String, i64, i64) = conn
            .query_row(
                "SELECT query_text, result_count, duration_ms FROM query_logs WHERE id = ?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();

        assert_eq!(query_text, "how to deploy?");
        assert_eq!(result_count, 5);
        assert_eq!(duration, 42);
    }

    #[test]
    fn test_agent_run_events_round_trip_in_sequence_order() {
        let db = Database::open_memory().unwrap();
        let finished = crate::agent_run::AgentRunEvent::terminal_error(
            "run-db-1",
            Some("turn-db-1"),
            2,
            "Agent execution timed out.",
            "timed_out",
            Some(&serde_json::json!({ "reason": "timeout" })),
        );
        let routed = crate::agent_run::AgentRunEvent::status_update(
            "run-db-1",
            Some("turn-db-1"),
            1,
            crate::agent_run::AgentRunPhase::Routing,
            "Route selected: Direct",
            Some("running"),
            None,
        )
        .with_presentation(
            crate::agent_run::AgentRunEventVisibility::Internal,
            crate::agent_run::AgentRunDisplayKind::Status,
            crate::agent_run::AgentRunEventImportance::Low,
        );

        db.save_agent_run_events(&[finished, routed])
            .expect("save events");

        let events = db
            .list_agent_run_events("run-db-1")
            .expect("list saved events");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_seq, 1);
        assert_eq!(events[0].kind, crate::agent_run::AgentRunEventKind::Status);
        assert_eq!(events[0].payload["content"], "Route selected: Direct");
        assert_eq!(
            events[0].visibility,
            crate::agent_run::AgentRunEventVisibility::Internal
        );
        assert_eq!(
            events[0].importance,
            crate::agent_run::AgentRunEventImportance::Low
        );
        assert_eq!(events[1].event_seq, 2);
        assert_eq!(events[1].kind, crate::agent_run::AgentRunEventKind::Error);
        assert_eq!(events[1].status.as_deref(), Some("timed_out"));
        assert_eq!(events[1].payload["reason"], "timeout");
    }

    #[test]
    fn test_agent_run_events_reject_invalid_event_before_persisting_batch() {
        let db = Database::open_memory().unwrap();
        let valid = crate::agent_run::AgentRunEvent::status_update(
            "run-db-2",
            Some("turn-db-2"),
            1,
            crate::agent_run::AgentRunPhase::Routing,
            "Route selected: Direct",
            Some("running"),
            None,
        );
        let invalid = crate::agent_run::AgentRunEvent::status_update(
            "run-db-2",
            Some("turn-db-2"),
            0,
            crate::agent_run::AgentRunPhase::Routing,
            "Invalid sequence",
            Some("running"),
            None,
        );

        let err = db
            .save_agent_run_events(&[valid, invalid])
            .expect_err("invalid event should be rejected");

        assert!(matches!(err, CoreError::InvalidInput(_)));
        assert!(
            db.list_agent_run_events("run-db-2").unwrap().is_empty(),
            "batch validation should reject before writing any event"
        );
    }

    #[test]
    fn test_agent_trajectory_store_round_trip_and_summary() {
        let db = Database::open_memory().unwrap();
        let mut trajectory = crate::trajectory::Trajectory::new(
            "agent_task_run:run-store-1",
            "2026-06-03T00:00:00Z",
            crate::runtime::AgentSessionConfig::default(),
        );
        trajectory.user_input_summary = "Summarize the stored trajectory.".to_string();
        trajectory.outcome = Some("success".to_string());
        trajectory
            .run_events
            .push(crate::agent_run::AgentRunEvent::status_update(
                "run-store-1",
                Some("turn-store-1"),
                1,
                crate::agent_run::AgentRunPhase::Routing,
                "Route selected: Direct",
                Some("running"),
                None,
            ));
        trajectory
            .tool_calls
            .push(serde_json::json!({ "toolName": "search" }));
        trajectory
            .approvals
            .push(serde_json::json!({ "id": "approval-1" }));

        let summary = db.save_agent_trajectory(&trajectory).unwrap();

        assert_eq!(summary.trajectory_id, "agent_task_run:run-store-1");
        assert_eq!(summary.source_kind, "agent_task_run");
        assert_eq!(summary.source_run_id.as_deref(), Some("run-store-1"));
        assert_eq!(summary.event_count, 1);
        assert_eq!(summary.tool_call_count, 1);
        assert_eq!(summary.approval_count, 1);
        assert_eq!(
            summary.redaction_profile,
            crate::trajectory::TrajectoryRedactionProfile::FullLocalPrivate
        );

        let loaded = db
            .load_agent_trajectory("agent_task_run:run-store-1")
            .unwrap();
        assert_eq!(loaded.trajectory_id, trajectory.trajectory_id);
        assert_eq!(loaded.metrics.event_count, 1);
        assert_eq!(loaded.metrics.tool_call_count, 1);

        let listed = db.list_agent_trajectory_summaries(10).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].trajectory_id, "agent_task_run:run-store-1");
    }

    #[test]
    fn trace_summary_buckets_cache_by_request_kind_and_compaction() {
        let db = Database::open_memory().unwrap();
        let mut trace =
            crate::trace::AgentTrace::begin("conv-cache", "hello", "deepseek-chat", 64_000);
        trace.add_step(crate::trace::TraceStep {
            iteration: 0,
            request_kind: "mainAgentStep".to_string(),
            tool_name: None,
            tool_duration_ms: None,
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: Some(80),
            cache_miss_tokens: Some(20),
            cache_creation_tokens: Some(20),
            context_usage_pct: 10.0,
            was_compacted: false,
        });
        trace.add_step(crate::trace::TraceStep {
            iteration: 1,
            request_kind: "subagentWorker".to_string(),
            tool_name: None,
            tool_duration_ms: None,
            input_tokens: 200,
            output_tokens: 30,
            cache_read_tokens: Some(25),
            cache_miss_tokens: Some(75),
            cache_creation_tokens: Some(75),
            context_usage_pct: 20.0,
            was_compacted: true,
        });
        trace.finish(crate::trace::TraceOutcome::Success, None);
        db.save_agent_trace(&trace).unwrap();

        let summary = db.get_trace_summary().unwrap();
        let main = summary
            .cache_buckets
            .iter()
            .find(|bucket| bucket.request_kind == "mainAgentStep" && !bucket.was_compacted)
            .expect("main non-compacted bucket");
        assert_eq!(main.step_count, 1);
        assert_eq!(main.cache_read_tokens, 80);
        assert_eq!(main.cache_miss_tokens, 20);
        assert_eq!(main.cache_creation_tokens, 20);
        assert!((main.hit_rate - 0.8).abs() < f64::EPSILON);

        let subagent = summary
            .cache_buckets
            .iter()
            .find(|bucket| bucket.request_kind == "subagentWorker" && bucket.was_compacted)
            .expect("subagent compacted bucket");
        assert_eq!(subagent.step_count, 1);
        assert_eq!(subagent.cache_read_tokens, 25);
        assert_eq!(subagent.cache_miss_tokens, 75);
        assert_eq!(subagent.cache_creation_tokens, 75);
        assert!((subagent.hit_rate - 0.25).abs() < f64::EPSILON);
    }
}

// ---------------------------------------------------------------------------
// Agent trace persistence
// ---------------------------------------------------------------------------

impl Database {
    /// Persist a completed agent trace.
    pub fn save_agent_trace(&self, trace: &crate::trace::AgentTrace) -> Result<(), CoreError> {
        let trace_value = serde_json::to_value(trace)
            .map_err(|e| CoreError::Internal(format!("serialize agent trace: {e}")))?;
        let trace_json = serde_json::to_string(&crate::sensitive_data::sanitize_json_strings(
            &trace_value,
            None,
        ))
        .map_err(|e| CoreError::Internal(format!("serialize sanitized agent trace: {e}")))?;
        let error_message = trace
            .error_message
            .as_deref()
            .map(|message| crate::sensitive_data::sanitize_diagnostic(message, None));
        let conn = self.conn();
        conn.execute(
            "INSERT OR REPLACE INTO agent_traces
             (id, conversation_id, started_at, finished_at, model_id,
              total_iterations, total_tool_calls, total_input_tokens, total_output_tokens,
              peak_context_usage_pct, tools_offered, cache_hit, outcome, error_message, trace_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            rusqlite::params![
                trace.id,
                trace.conversation_id,
                trace.started_at.to_rfc3339(),
                trace.finished_at.map(|t| t.to_rfc3339()),
                trace.model_id,
                trace.total_iterations,
                trace.total_tool_calls,
                trace.total_input_tokens as i64,
                trace.total_output_tokens as i64,
                trace.peak_context_usage_pct,
                trace.tools_offered,
                trace.cache_hit as i32,
                trace.outcome.to_string(),
                error_message,
                trace_json,
            ],
        )?;
        Ok(())
    }

    /// Retrieve all traces for a conversation.
    pub fn get_agent_traces(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<crate::trace::AgentTrace>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT trace_json FROM agent_traces WHERE conversation_id = ?1 ORDER BY created_at",
        )?;
        let rows = stmt.query_map(rusqlite::params![conversation_id], |row| {
            row.get::<_, String>(0)
        })?;
        let mut traces = Vec::new();
        for row in rows {
            let json = row?;
            let trace: crate::trace::AgentTrace = serde_json::from_str(&json)
                .map_err(|e| CoreError::Internal(format!("deserialize agent trace: {e}")))?;
            traces.push(trace);
        }
        Ok(traces)
    }

    /// Retrieve the most recent traces across all conversations.
    pub fn get_recent_traces(
        &self,
        limit: usize,
    ) -> Result<Vec<crate::trace::AgentTrace>, CoreError> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT trace_json FROM agent_traces ORDER BY created_at DESC LIMIT ?1")?;
        let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
            row.get::<_, String>(0)
        })?;
        let mut traces = Vec::new();
        for row in rows {
            let json = row?;
            let trace: crate::trace::AgentTrace = serde_json::from_str(&json)
                .map_err(|e| CoreError::Internal(format!("deserialize agent trace: {e}")))?;
            traces.push(trace);
        }
        Ok(traces)
    }

    /// Compute aggregated analytics across all agent traces.
    pub fn get_trace_summary(&self) -> Result<crate::trace::TraceSummary, CoreError> {
        let conn = self.conn();

        // Aggregate numeric stats in one query.
        let mut stmt = conn.prepare(
            "SELECT
                COUNT(*) AS total_sessions,
                COALESCE(SUM(total_tool_calls), 0) AS total_tool_calls,
                COALESCE(SUM(total_input_tokens), 0) AS total_input_tokens,
                COALESCE(SUM(total_output_tokens), 0) AS total_output_tokens,
                COALESCE(AVG(total_iterations), 0) AS avg_iterations,
                COALESCE(AVG(total_tool_calls), 0) AS avg_tools,
                COALESCE(AVG(peak_context_usage_pct), 0) AS avg_context,
                COALESCE(SUM(CASE WHEN outcome = 'success' THEN 1 ELSE 0 END), 0) AS success_count,
                COALESCE(SUM(CASE WHEN cache_hit = 1 THEN 1 ELSE 0 END), 0) AS cache_hit_count,
                COALESCE(SUM(CASE WHEN started_at >= datetime('now', '-7 days') THEN 1 ELSE 0 END), 0) AS sessions_7d,
                COALESCE(SUM(CASE WHEN started_at >= datetime('now', '-7 days') THEN total_input_tokens + total_output_tokens ELSE 0 END), 0) AS tokens_7d
             FROM agent_traces",
        )?;

        let (
            total_sessions,
            total_tool_calls,
            total_input_tokens,
            total_output_tokens,
            avg_iterations,
            avg_tools,
            avg_context,
            success_count,
            cache_hit_count,
            sessions_7d,
            tokens_7d,
        ) = stmt.query_row([], |row| {
            Ok((
                row.get::<_, i64>(0)? as u64,
                row.get::<_, i64>(1)? as u64,
                row.get::<_, i64>(2)? as u64,
                row.get::<_, i64>(3)? as u64,
                row.get::<_, f64>(4)?,
                row.get::<_, f64>(5)?,
                row.get::<_, f64>(6)?,
                row.get::<_, i64>(7)? as u64,
                row.get::<_, i64>(8)? as u64,
                row.get::<_, i64>(9)? as u64,
                row.get::<_, i64>(10)? as u64,
            ))
        })?;

        let success_rate = if total_sessions > 0 {
            success_count as f64 / total_sessions as f64
        } else {
            0.0
        };
        let cache_hit_rate = if total_sessions > 0 {
            cache_hit_count as f64 / total_sessions as f64
        } else {
            0.0
        };

        // Top tools: extract from trace_json steps (limit scan to 200 most recent).
        let mut tool_counts: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        let mut cache_buckets: std::collections::BTreeMap<
            (String, bool),
            crate::trace::TraceCacheBucket,
        > = std::collections::BTreeMap::new();
        let mut stmt2 =
            conn.prepare("SELECT trace_json FROM agent_traces ORDER BY created_at DESC LIMIT 200")?;
        let rows2 = stmt2.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows2 {
            let json = row?;
            if let Ok(trace) = serde_json::from_str::<crate::trace::AgentTrace>(&json) {
                for step in &trace.steps {
                    if let Some(ref name) = step.tool_name {
                        *tool_counts.entry(name.clone()).or_insert(0) += 1;
                    }
                    let key = (step.request_kind.clone(), step.was_compacted);
                    let bucket = cache_buckets.entry(key).or_insert_with(|| {
                        crate::trace::TraceCacheBucket {
                            request_kind: step.request_kind.clone(),
                            was_compacted: step.was_compacted,
                            step_count: 0,
                            cache_read_tokens: 0,
                            cache_miss_tokens: 0,
                            cache_creation_tokens: 0,
                            hit_rate: 0.0,
                        }
                    });
                    bucket.step_count += 1;
                    bucket.cache_read_tokens += step.cache_read_tokens.unwrap_or(0);
                    bucket.cache_miss_tokens += step.cache_miss_tokens.unwrap_or(0);
                    bucket.cache_creation_tokens += step.cache_creation_tokens.unwrap_or(0);
                }
            }
        }
        let mut top_tools: Vec<(String, u64)> = tool_counts.into_iter().collect();
        top_tools.sort_by_key(|tool| std::cmp::Reverse(tool.1));
        top_tools.truncate(10);
        let mut cache_buckets: Vec<crate::trace::TraceCacheBucket> =
            cache_buckets.into_values().collect();
        for bucket in &mut cache_buckets {
            let denominator = bucket.cache_read_tokens + bucket.cache_miss_tokens;
            bucket.hit_rate = if denominator == 0 {
                0.0
            } else {
                bucket.cache_read_tokens as f64 / denominator as f64
            };
        }

        Ok(crate::trace::TraceSummary {
            total_sessions,
            total_tool_calls,
            total_input_tokens,
            total_output_tokens,
            avg_iterations_per_session: avg_iterations,
            avg_tools_per_session: avg_tools,
            avg_context_usage_pct: avg_context,
            success_rate,
            cache_hit_rate,
            top_tools,
            cache_buckets,
            sessions_last_7_days: sessions_7d,
            tokens_last_7_days: tokens_7d,
        })
    }
}
