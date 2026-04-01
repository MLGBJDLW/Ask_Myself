/// Schema migration runner for ask-core.
///
/// Uses a single consolidated schema for fresh installs, with support
/// for future incremental migrations (v017+). Tracks applied migrations
/// in a `_migrations` table.
use rusqlite::Connection;
use rusqlite::Error as SqlError;

use crate::error::CoreError;

/// Consolidated schema covering v001–v016 for fresh installs.
const V_INITIAL_CONSOLIDATED: &str = include_str!("v_initial_consolidated.sql");

/// Names of the original v001–v016 migrations (now consolidated).
const MIGRATION_NAMES: &[&str] = &[
    "v001_core_tables",
    "v002_fts5",
    "v003_playbooks",
    "v004_embeddings_feedback",
    "v005_privacy_config",
    "v006_embedder_config",
    "v007_conversations",
    "v008_agent_config_context_window",
    "v009_conversation_sources",
    "v010_agent_config_reasoning",
    "v011_message_thinking",
    "v012_agent_config_max_iterations",
    "v013_document_metadata",
    "v014_agent_config_summarization",
    "v015_answer_cache",
];

/// Future incremental migrations (v017+). Add new entries here.
/// Each entry is `(name, sql)`.
const FUTURE_MIGRATIONS: &[(&str, &str)] = &[
    (
        "v016_ocr_config",
        "CREATE TABLE IF NOT EXISTS ocr_config (
          key TEXT PRIMARY KEY NOT NULL,
          value TEXT NOT NULL,
          updated_at TEXT NOT NULL DEFAULT (datetime('now'))
      );",
    ),
    (
        "v017_conversation_checkpoints",
        "CREATE TABLE IF NOT EXISTS conversation_checkpoints (
          id TEXT PRIMARY KEY,
          conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
          label TEXT NOT NULL DEFAULT '',
          message_count INTEGER NOT NULL,
          estimated_tokens INTEGER NOT NULL DEFAULT 0,
          created_at TEXT NOT NULL DEFAULT (datetime('now'))
      );
      CREATE TABLE IF NOT EXISTS archived_messages (
          id TEXT PRIMARY KEY,
          checkpoint_id TEXT NOT NULL REFERENCES conversation_checkpoints(id) ON DELETE CASCADE,
          conversation_id TEXT NOT NULL,
          role TEXT NOT NULL,
          content TEXT NOT NULL DEFAULT '',
          tool_call_id TEXT,
          tool_calls_json TEXT,
          token_count INTEGER NOT NULL DEFAULT 0,
          original_sort_order INTEGER NOT NULL DEFAULT 0,
          created_at TEXT NOT NULL DEFAULT (datetime('now'))
      );",
    ),
    (
        "v018_user_memories",
        "CREATE TABLE IF NOT EXISTS user_memories (
          id TEXT PRIMARY KEY NOT NULL,
          content TEXT NOT NULL,
          created_at TEXT NOT NULL DEFAULT (datetime('now')),
          updated_at TEXT NOT NULL DEFAULT (datetime('now'))
      );",
    ),
    (
        "v019_skills",
        "CREATE TABLE IF NOT EXISTS skills (
          id TEXT PRIMARY KEY NOT NULL,
          name TEXT NOT NULL,
          content TEXT NOT NULL,
          enabled INTEGER NOT NULL DEFAULT 1,
          created_at TEXT NOT NULL DEFAULT (datetime('now')),
          updated_at TEXT NOT NULL DEFAULT (datetime('now'))
      );",
    ),
    (
        "v020_mcp_servers",
        "CREATE TABLE IF NOT EXISTS mcp_servers (
          id TEXT PRIMARY KEY NOT NULL,
          name TEXT NOT NULL,
          transport TEXT NOT NULL DEFAULT 'stdio',
          command TEXT,
          args TEXT,
          url TEXT,
          env_json TEXT,
          headers_json TEXT,
          enabled INTEGER NOT NULL DEFAULT 1,
          created_at TEXT NOT NULL DEFAULT (datetime('now')),
          updated_at TEXT NOT NULL DEFAULT (datetime('now'))
      );",
    ),
    (
        "v021_message_artifacts",
        "ALTER TABLE messages ADD COLUMN artifacts_json TEXT;
      ALTER TABLE archived_messages ADD COLUMN artifacts_json TEXT;",
    ),
    (
        "v022_subagent_allowed_tools",
        "ALTER TABLE agent_configs ADD COLUMN subagent_allowed_tools_json TEXT;",
    ),
    (
        "v023_subagent_budget_controls",
        "ALTER TABLE agent_configs ADD COLUMN subagent_max_parallel INTEGER;
      ALTER TABLE agent_configs ADD COLUMN subagent_max_calls_per_turn INTEGER;
      ALTER TABLE agent_configs ADD COLUMN subagent_token_budget INTEGER;",
    ),
    (
        "v024_video_config",
        "CREATE TABLE IF NOT EXISTS video_config (
          key TEXT PRIMARY KEY,
          value TEXT NOT NULL
      );",
    ),
    (
        "v025_builtin_mcp",
        "ALTER TABLE mcp_servers ADD COLUMN builtin_id TEXT;
        INSERT OR IGNORE INTO mcp_servers (id, name, transport, command, args, url, env_json, headers_json, enabled, builtin_id)
        VALUES (
            'builtin-open-websearch',
            'Web Search',
            'streamable_http',
            'npx',
            '[\"open-websearch@latest\"]',
            NULL,
            '{\"DEFAULT_SEARCH_ENGINE\":\"bing\"}',
            NULL,
            0,
            'open-websearch'
        );",
    ),
    (
        "v026_timeout_settings",
        "ALTER TABLE agent_configs ADD COLUMN tool_timeout_secs INTEGER DEFAULT NULL;
        ALTER TABLE agent_configs ADD COLUMN agent_timeout_secs INTEGER DEFAULT NULL;",
    ),
    (
        "v027_agent_traces",
        "CREATE TABLE IF NOT EXISTS agent_traces (
            id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL,
            started_at TEXT NOT NULL,
            finished_at TEXT,
            model_id TEXT NOT NULL,
            total_iterations INTEGER NOT NULL DEFAULT 0,
            total_tool_calls INTEGER NOT NULL DEFAULT 0,
            total_input_tokens INTEGER NOT NULL DEFAULT 0,
            total_output_tokens INTEGER NOT NULL DEFAULT 0,
            peak_context_usage_pct REAL NOT NULL DEFAULT 0.0,
            tools_offered INTEGER NOT NULL DEFAULT 0,
            cache_hit INTEGER NOT NULL DEFAULT 0,
            outcome TEXT NOT NULL DEFAULT 'success',
            error_message TEXT,
            trace_json TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_agent_traces_conversation ON agent_traces(conversation_id);
        CREATE INDEX IF NOT EXISTS idx_agent_traces_created ON agent_traces(created_at);",
    ),
    (
        "v028_conversation_messages_fts",
        "CREATE VIRTUAL TABLE IF NOT EXISTS fts_messages USING fts5(
            content,
            conversation_id UNINDEXED,
            message_id UNINDEXED,
            role UNINDEXED,
            tokenize='unicode61 remove_diacritics 2'
        );
        INSERT OR IGNORE INTO fts_messages(content, conversation_id, message_id, role)
            SELECT content, conversation_id, id, role FROM messages
            WHERE role IN ('user', 'assistant') AND content != '';
        CREATE TRIGGER IF NOT EXISTS messages_fts_ai AFTER INSERT ON messages
        WHEN new.role IN ('user', 'assistant') AND new.content != ''
        BEGIN
            INSERT INTO fts_messages(content, conversation_id, message_id, role)
            VALUES (new.content, new.conversation_id, new.id, new.role);
        END;
        CREATE TRIGGER IF NOT EXISTS messages_fts_ad AFTER DELETE ON messages BEGIN
            DELETE FROM fts_messages WHERE message_id = old.id;
        END;
        CREATE TRIGGER IF NOT EXISTS messages_fts_au AFTER UPDATE OF content ON messages BEGIN
            DELETE FROM fts_messages WHERE message_id = old.id;
            INSERT INTO fts_messages(content, conversation_id, message_id, role)
            VALUES (new.content, new.conversation_id, new.id, new.role);
        END;",
    ),
    (
        "v029_user_memories_source",
        "ALTER TABLE user_memories ADD COLUMN source TEXT NOT NULL DEFAULT 'manual';",
    ),
    (
        "v030_subagent_allowed_skills",
        "ALTER TABLE agent_configs ADD COLUMN subagent_allowed_skill_ids_json TEXT;",
    ),
    (
        "v031_conversation_collection_context",
        "ALTER TABLE conversations ADD COLUMN collection_context_json TEXT;",
    ),
    (
        "v032_conversation_turns",
        "CREATE TABLE IF NOT EXISTS conversation_turns (
            id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
            user_message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
            assistant_message_id TEXT REFERENCES messages(id) ON DELETE SET NULL,
            status TEXT NOT NULL DEFAULT 'running',
            route_kind TEXT,
            trace_json TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            finished_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_conversation_turns_conversation
            ON conversation_turns(conversation_id, created_at);",
    ),
    (
        "v033_default_skills",
        r#"INSERT OR IGNORE INTO skills (id, name, content, enabled)
        VALUES (
            'builtin-visual-explanations',
            'Visual Explanations',
            'When a workflow, architecture, state transition, hierarchy, timeline, or comparison would be easier to understand visually, prefer a compact Mermaid code block in the final reply. Use Mermaid only when it genuinely clarifies. Favor flowcharts for workflows, sequence diagrams for request or tool exchanges, state diagrams for lifecycle changes, and graph layouts for dependencies. Keep diagrams accurate, small, and readable, then summarize the takeaway in prose under the diagram.',
            1
        );
        INSERT OR IGNORE INTO skills (id, name, content, enabled)
        VALUES (
            'builtin-office-document-design',
            'Office Document Design Director',
            'When creating DOCX, XLSX, or PPTX files, decide the design brief before using tools: audience, tone, information hierarchy, and visual style. Then generate the file deliberately instead of dumping raw text. For DOCX, use cover details, section rhythm, callouts, and tables when useful. For XLSX, create a clear title band or summary area, freeze important rows, use formulas for derived metrics, and separate presentation from raw data. For PPTX, storyboard the deck, keep one message per slide, and use section or comparison layouts where they improve clarity. If the user leaves design choices open, choose polished professional defaults.',
            1
        );"#,
    ),
];

/// Ensures the internal `_migrations` tracking table exists.
fn ensure_migrations_table(conn: &Connection) -> Result<(), CoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;
    Ok(())
}

fn is_idempotent_schema_error(err: &SqlError) -> bool {
    matches!(
        err,
        SqlError::SqliteFailure(_, Some(msg))
            if msg.to_ascii_lowercase().contains("duplicate column name")
    )
}

/// Runs all pending migrations against the given connection.
///
/// - Fresh DB (empty `_migrations`): runs the consolidated schema and
///   records all `MIGRATION_NAMES` plus any `FUTURE_MIGRATIONS`.
/// - Existing DB: verifies consolidated names are present (marks any
///   missing ones as applied), then applies any un-applied future
///   migrations.
pub fn run_migrations(conn: &Connection) -> Result<(), CoreError> {
    ensure_migrations_table(conn)?;

    let migration_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM _migrations", [], |row| row.get(0))?;

    if migration_count == 0 {
        // Fresh install: apply consolidated schema.
        tracing::info!("Fresh install detected – applying consolidated schema…");
        conn.execute_batch(V_INITIAL_CONSOLIDATED)?;
        for name in MIGRATION_NAMES {
            conn.execute("INSERT INTO _migrations (name) VALUES (?1)", [name])?;
        }
    } else {
        // Existing DB: ensure all consolidated names are recorded.
        for name in MIGRATION_NAMES {
            let already_applied: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM _migrations WHERE name = ?1)",
                [name],
                |row| row.get(0),
            )?;
            if !already_applied {
                tracing::warn!(
                    "Migration '{name}' not in _migrations table but DB exists; marking as applied."
                );
                conn.execute("INSERT INTO _migrations (name) VALUES (?1)", [name])?;
            }
        }
    }

    // Apply any future incremental migrations.
    for (name, sql) in FUTURE_MIGRATIONS {
        let already_applied: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM _migrations WHERE name = ?1)",
            [name],
            |row| row.get(0),
        )?;

        // Always execute — uses IF NOT EXISTS, safe to re-run.
        // Self-heals databases where name was recorded without SQL running.
        if let Err(err) = conn.execute_batch(sql) {
            if !is_idempotent_schema_error(&err) {
                return Err(err.into());
            }
        }

        if !already_applied {
            tracing::info!("Applying migration '{name}'…");
            conn.execute("INSERT INTO _migrations (name) VALUES (?1)", [name])?;
        }
    }

    Ok(())
}

/// Total number of all migration names (consolidated + future).
#[cfg(test)]
fn total_migration_count() -> usize {
    MIGRATION_NAMES.len() + FUTURE_MIGRATIONS.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migrations_run_successfully() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("migrations should succeed");

        // Verify all expected tables exist
        let tables: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };

        assert!(tables.contains(&"sources".to_string()));
        assert!(tables.contains(&"documents".to_string()));
        assert!(tables.contains(&"chunks".to_string()));
        assert!(tables.contains(&"playbooks".to_string()));
        assert!(tables.contains(&"playbook_citations".to_string()));
        assert!(tables.contains(&"query_logs".to_string()));
        assert!(tables.contains(&"embeddings".to_string()));
        assert!(tables.contains(&"feedback".to_string()));
        assert!(tables.contains(&"_migrations".to_string()));
    }

    #[test]
    fn test_migrations_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("first run should succeed");
        run_migrations(&conn).expect("second run should also succeed");

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM _migrations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count,
            total_migration_count() as i64,
            "should have exactly {} migration records",
            total_migration_count()
        );
    }

    #[test]
    fn test_default_skills_seeded() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("migrations should succeed");

        let mut stmt = conn
            .prepare("SELECT id, enabled FROM skills ORDER BY id")
            .unwrap();
        let rows: Vec<(String, i32)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .filter_map(|row| row.ok())
            .collect();

        assert!(rows.iter().any(|(id, enabled)| id == "builtin-office-document-design" && *enabled == 1));
        assert!(rows.iter().any(|(id, enabled)| id == "builtin-visual-explanations" && *enabled == 1));
    }
}
