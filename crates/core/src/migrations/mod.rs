/// Schema migration runner for ask-core.
///
/// Embeds SQL migration files and applies them in order,
/// tracking applied migrations in a `_migrations` table.

use rusqlite::Connection;

use crate::error::CoreError;

const V001_CORE_TABLES: &str = include_str!("v001_core_tables.sql");
const V002_FTS5: &str = include_str!("v002_fts5.sql");
const V003_PLAYBOOKS: &str = include_str!("v003_playbooks.sql");
const V004_EMBEDDINGS_FEEDBACK: &str = include_str!("v004_embeddings_feedback.sql");
const V005_PRIVACY_CONFIG: &str = include_str!("v005_privacy_config.sql");
const V006_EMBEDDER_CONFIG: &str = include_str!("v006_embedder_config.sql");
const V007_CONVERSATIONS: &str = include_str!("v007_conversations.sql");
const V008_AGENT_CONFIG_CONTEXT_WINDOW: &str = include_str!("v008_agent_config_context_window.sql");
const V009_CONVERSATION_SOURCES: &str = include_str!("v009_conversation_sources.sql");

/// Ordered list of migrations to apply.
const MIGRATIONS: &[(&str, &str)] = &[
    ("v001_core_tables", V001_CORE_TABLES),
    ("v002_fts5", V002_FTS5),
    ("v003_playbooks", V003_PLAYBOOKS),
    ("v004_embeddings_feedback", V004_EMBEDDINGS_FEEDBACK),
    ("v005_privacy_config", V005_PRIVACY_CONFIG),
    ("v006_embedder_config", V006_EMBEDDER_CONFIG),
    ("v007_conversations", V007_CONVERSATIONS),
    ("v008_agent_config_context_window", V008_AGENT_CONFIG_CONTEXT_WINDOW),
    ("v009_conversation_sources", V009_CONVERSATION_SOURCES),
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

/// Runs all pending migrations against the given connection.
///
/// Migrations are applied inside individual transactions so that a
/// failure in one migration does not silently leave the database in a
/// half-migrated state.
pub fn run_migrations(conn: &Connection) -> Result<(), CoreError> {
    ensure_migrations_table(conn)?;

    for (name, sql) in MIGRATIONS {
        let already_applied: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM _migrations WHERE name = ?1)",
            [name],
            |row| row.get(0),
        )?;

        if already_applied {
            log::debug!("Migration '{name}' already applied, skipping.");
            continue;
        }

        log::info!("Applying migration '{name}'…");
        conn.execute_batch(sql)?;
        conn.execute(
            "INSERT INTO _migrations (name) VALUES (?1)",
            [name],
        )?;
    }

    Ok(())
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
        assert_eq!(count, MIGRATIONS.len() as i64, "should have exactly {} migration records", MIGRATIONS.len());
    }
}
