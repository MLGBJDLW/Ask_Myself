/// Schema migration runner for nexa-core.
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
    (
        "v034_knowledge_compile",
        "CREATE TABLE IF NOT EXISTS document_summaries (
            id TEXT PRIMARY KEY NOT NULL,
            document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            summary TEXT NOT NULL,
            key_points TEXT NOT NULL DEFAULT '[]',
            tags TEXT NOT NULL DEFAULT '[]',
            model_used TEXT NOT NULL DEFAULT '',
            compiled_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_doc_summaries_doc ON document_summaries(document_id);

        CREATE TABLE IF NOT EXISTS entities (
            id TEXT PRIMARY KEY NOT NULL,
            name TEXT NOT NULL,
            entity_type TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            first_seen_doc TEXT REFERENCES documents(id) ON DELETE SET NULL,
            mention_count INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_entities_name_type ON entities(name, entity_type);
        CREATE INDEX IF NOT EXISTS idx_entities_type ON entities(entity_type);

        CREATE TABLE IF NOT EXISTS document_entities (
            document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            relevance REAL NOT NULL DEFAULT 1.0,
            context_snippet TEXT NOT NULL DEFAULT '',
            PRIMARY KEY(document_id, entity_id)
        );

        CREATE TABLE IF NOT EXISTS entity_links (
            id TEXT PRIMARY KEY NOT NULL,
            source_entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            target_entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            relation_type TEXT NOT NULL,
            strength REAL NOT NULL DEFAULT 1.0,
            evidence_doc_id TEXT REFERENCES documents(id) ON DELETE SET NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_entity_links_unique ON entity_links(source_entity_id, target_entity_id, relation_type);
        CREATE INDEX IF NOT EXISTS idx_entity_links_source ON entity_links(source_entity_id);
        CREATE INDEX IF NOT EXISTS idx_entity_links_target ON entity_links(target_entity_id);

        CREATE TABLE IF NOT EXISTS health_checks (
            id TEXT PRIMARY KEY NOT NULL,
            check_type TEXT NOT NULL,
            severity TEXT NOT NULL DEFAULT 'info',
            target_doc_id TEXT REFERENCES documents(id) ON DELETE CASCADE,
            target_entity_id TEXT REFERENCES entities(id) ON DELETE CASCADE,
            description TEXT NOT NULL,
            suggestion TEXT NOT NULL DEFAULT '',
            resolved INTEGER NOT NULL DEFAULT 0,
            checked_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_health_checks_type ON health_checks(check_type);
        CREATE INDEX IF NOT EXISTS idx_health_checks_resolved ON health_checks(resolved);",
    ),
    (
        "v035_scan_errors",
        "CREATE TABLE IF NOT EXISTS scan_errors (
            source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
            path TEXT NOT NULL,
            error_message TEXT NOT NULL,
            error_count INTEGER NOT NULL DEFAULT 1,
            first_failed_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_failed_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (source_id, path)
        );",
    ),
    (
        "v036_fix_knowledge_column_types",
        "-- document_summaries: document_id INTEGER → TEXT
        CREATE TABLE IF NOT EXISTS document_summaries_new (
            id TEXT PRIMARY KEY NOT NULL,
            document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            summary TEXT NOT NULL,
            key_points TEXT NOT NULL DEFAULT '[]',
            tags TEXT NOT NULL DEFAULT '[]',
            model_used TEXT NOT NULL DEFAULT '',
            compiled_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT OR IGNORE INTO document_summaries_new SELECT * FROM document_summaries;
        DROP TABLE IF EXISTS document_summaries;
        ALTER TABLE document_summaries_new RENAME TO document_summaries;
        CREATE UNIQUE INDEX IF NOT EXISTS idx_doc_summaries_doc ON document_summaries(document_id);

        -- entities: first_seen_doc INTEGER → TEXT
        CREATE TABLE IF NOT EXISTS entities_new (
            id TEXT PRIMARY KEY NOT NULL,
            name TEXT NOT NULL,
            entity_type TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            first_seen_doc TEXT REFERENCES documents(id) ON DELETE SET NULL,
            mention_count INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT OR IGNORE INTO entities_new SELECT * FROM entities;
        DROP TABLE IF EXISTS entities;
        ALTER TABLE entities_new RENAME TO entities;
        CREATE UNIQUE INDEX IF NOT EXISTS idx_entities_name_type ON entities(name, entity_type);
        CREATE INDEX IF NOT EXISTS idx_entities_type ON entities(entity_type);

        -- document_entities: document_id INTEGER → TEXT
        CREATE TABLE IF NOT EXISTS document_entities_new (
            document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            relevance REAL NOT NULL DEFAULT 1.0,
            context_snippet TEXT NOT NULL DEFAULT '',
            PRIMARY KEY(document_id, entity_id)
        );
        INSERT OR IGNORE INTO document_entities_new SELECT * FROM document_entities;
        DROP TABLE IF EXISTS document_entities;
        ALTER TABLE document_entities_new RENAME TO document_entities;

        -- entity_links: evidence_doc_id INTEGER → TEXT
        CREATE TABLE IF NOT EXISTS entity_links_new (
            id TEXT PRIMARY KEY NOT NULL,
            source_entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            target_entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            relation_type TEXT NOT NULL,
            strength REAL NOT NULL DEFAULT 1.0,
            evidence_doc_id TEXT REFERENCES documents(id) ON DELETE SET NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT OR IGNORE INTO entity_links_new SELECT * FROM entity_links;
        DROP TABLE IF EXISTS entity_links;
        ALTER TABLE entity_links_new RENAME TO entity_links;
        CREATE UNIQUE INDEX IF NOT EXISTS idx_entity_links_unique ON entity_links(source_entity_id, target_entity_id, relation_type);
        CREATE INDEX IF NOT EXISTS idx_entity_links_source ON entity_links(source_entity_id);
        CREATE INDEX IF NOT EXISTS idx_entity_links_target ON entity_links(target_entity_id);

        -- health_checks: target_doc_id INTEGER → TEXT
        CREATE TABLE IF NOT EXISTS health_checks_new (
            id TEXT PRIMARY KEY NOT NULL,
            check_type TEXT NOT NULL,
            severity TEXT NOT NULL DEFAULT 'info',
            target_doc_id TEXT REFERENCES documents(id) ON DELETE CASCADE,
            target_entity_id TEXT REFERENCES entities(id) ON DELETE CASCADE,
            description TEXT NOT NULL,
            suggestion TEXT NOT NULL DEFAULT '',
            resolved INTEGER NOT NULL DEFAULT 0,
            checked_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT OR IGNORE INTO health_checks_new SELECT * FROM health_checks;
        DROP TABLE IF EXISTS health_checks;
        ALTER TABLE health_checks_new RENAME TO health_checks;
        CREATE INDEX IF NOT EXISTS idx_health_checks_type ON health_checks(check_type);
        CREATE INDEX IF NOT EXISTS idx_health_checks_resolved ON health_checks(resolved);",
    ),
    (
        "v037_upgrade_default_skills",
        r#"UPDATE skills SET content = '## Trigger
When the answer involves: workflows, processes, state transitions, hierarchies, dependencies, timelines, comparisons, or data flows.

## Rules
1. ALWAYS include a Mermaid diagram when the trigger conditions match
2. Choose the right diagram type:
   - Workflows/processes → flowchart
   - Request/response flows → sequence diagram
   - Lifecycle/state changes → state diagram
   - Hierarchies/dependencies → graph TD/LR
   - Timelines → gantt
   - Comparisons → use a table instead of Mermaid
3. Keep diagrams under 15 nodes. Split complex diagrams into multiple smaller ones
4. Every diagram MUST have a 1-sentence takeaway below it
5. Use descriptive node labels, not single letters (A, B, C)

## Format
```mermaid
[diagram]
```
**Takeaway:** [one sentence explaining the key insight]

## Example
User asks: "How does the login flow work?"

BAD (no visual):
> The user submits credentials, the server validates them, creates a session, and returns a token.

GOOD:
```mermaid
sequenceDiagram
    User->>Server: POST /login (credentials)
    Server->>DB: Validate credentials
    DB-->>Server: User record
    Server->>Server: Create JWT token
    Server-->>User: 200 OK + token
```
**Takeaway:** Login is a 3-hop flow (client → server → DB) with JWT token returned on success.', updated_at = datetime('now') WHERE id = 'builtin-visual-explanations';

        UPDATE skills SET content = '## Trigger
When creating DOCX, XLSX, or PPTX files via generate_docx/generate_xlsx/ppt_generate tools.

## Rules

### DOCX — Professional Documents
1. ALWAYS include: theme colors, title font, body font
2. Start with a cover page (title, subtitle, date/author note)
3. Use section rhythm: heading → 1-2 paragraphs → callout or table → next section
4. Insert callout boxes for key takeaways (tone: info for facts, warning for risks, success for wins)
5. Tables: use for any data with 3+ items. Always include header row
6. Bullet lists: max 7 items per list. Prefer grouped bullets with sub-headings

### XLSX — Data Workbooks
1. Sheet 1 = Summary dashboard (title banner, KPIs, key metrics)
2. Sheet 2+ = Detail data (raw data, calculations)
3. ALWAYS add charts when showing trends, comparisons, or distributions
4. Use formulas for derived values — never hardcode calculated numbers
5. Freeze header rows. Enable auto-filter. Set column widths explicitly
6. Use color coding: green for positive, red for negative, blue for neutral

### PPTX — Presentations
1. Max 6 bullets per slide. One message per slide
2. Storyboard: Title slide → Agenda → Content (3-7 slides) → Summary → Q&A
3. Use section divider slides between major topics
4. Comparison layout for pros/cons, before/after, option A vs B
5. Every data claim needs a source citation on the slide
6. Speaker notes: include detailed talking points (2-3 sentences per slide)

## Common Rules (All Formats)
- Choose colors that match the topic: blue for corporate, green for nature/health, orange for energy/startup
- Never use default black-and-white. Always set a theme
- Information hierarchy: most important info first, details second
- If user doesn''t specify design, use professional blue theme: primary #2B579A, accent #217346', updated_at = datetime('now') WHERE id = 'builtin-office-document-design';

        INSERT OR IGNORE INTO skills (id, name, content, enabled)
        VALUES (
            'builtin-evidence-first',
            'Evidence-First Answers',

            '## Trigger
Every answer that uses knowledge base search results.

## Rules
1. ALWAYS cite sources: "According to [Document Title] (path/to/file)..."
2. When multiple sources exist:
   - If they AGREE: synthesize into one answer, cite all sources
   - If they CONFLICT: present both views explicitly, note the contradiction
   - If only ONE source: clearly state the answer comes from a single source
3. Confidence levels:
   - HIGH: 3+ sources agree → state confidently
   - MEDIUM: 1-2 sources → note limited evidence
   - LOW: no direct source, inferring → explicitly say "Based on inference, not direct knowledge base evidence"
4. Never fabricate information not in the search results
5. If the knowledge base has NO relevant results, say so clearly — don''t guess

## Format
📚 **Sources:** [Document1], [Document2]
[Answer with inline citations]

💡 **Confidence:** HIGH/MEDIUM/LOW — [reason]',
            1
        );"#,
    ),
    (
        "v038_projects",
        "CREATE TABLE IF NOT EXISTS projects (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            icon        TEXT NOT NULL DEFAULT '',
            color       TEXT NOT NULL DEFAULT '',
            system_prompt TEXT NOT NULL DEFAULT '',
            source_scope_json TEXT,
            archived    INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_projects_name ON projects(name);
        CREATE INDEX IF NOT EXISTS idx_projects_archived ON projects(archived);",
    ),
    (
        "v039_conversation_project",
        "ALTER TABLE conversations ADD COLUMN project_id TEXT REFERENCES projects(id) ON DELETE SET NULL;
        CREATE INDEX IF NOT EXISTS idx_conversations_project ON conversations(project_id);",
    ),
    (
        "v040_message_image_attachments",
        "ALTER TABLE messages ADD COLUMN image_attachments_json TEXT;",
    ),
    (
        "v041_refresh_office_skill_tools",
        "UPDATE skills
            SET content = REPLACE(content, 'generate_pptx', 'ppt_generate'),
                updated_at = datetime('now')
            WHERE id = 'builtin-office-document-design'
              AND content LIKE '%generate_pptx%';",
    ),
    (
        "v042_conversation_title_is_auto",
        "ALTER TABLE conversations ADD COLUMN title_is_auto INTEGER NOT NULL DEFAULT 1;",
    ),
    (
        "v043_agent_scratchpad",
        "CREATE TABLE IF NOT EXISTS agent_scratchpad (
            conversation_id TEXT PRIMARY KEY,
            content TEXT NOT NULL DEFAULT '',
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_agent_scratchpad_updated ON agent_scratchpad(updated_at);",
    ),
    (
        "v044_message_feedback_and_learned_successes",
        "CREATE TABLE IF NOT EXISTS message_feedback (
            id TEXT PRIMARY KEY,
            message_id TEXT NOT NULL,
            conversation_id TEXT NOT NULL,
            rating INTEGER NOT NULL,
            note TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE,
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            UNIQUE(message_id)
        );
        CREATE INDEX IF NOT EXISTS idx_message_feedback_conv ON message_feedback(conversation_id);
        CREATE INDEX IF NOT EXISTS idx_message_feedback_rating ON message_feedback(rating, created_at);

        CREATE TABLE IF NOT EXISTS learned_successes (
            id TEXT PRIMARY KEY,
            user_query TEXT NOT NULL,
            response_summary TEXT NOT NULL,
            source_message_id TEXT NOT NULL,
            query_embedding BLOB,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (source_message_id) REFERENCES messages(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_learned_successes_created ON learned_successes(created_at);",
    ),
    (
        "v045_skill_description_and_bundled_builtins",
        "ALTER TABLE skills ADD COLUMN description TEXT NOT NULL DEFAULT '';
        DELETE FROM skills WHERE id IN (
            'builtin-visual-explanations',
            'builtin-office-document-design',
            'builtin-evidence-first'
        );",
    ),
    (
        "v046_tool_approval_policies",
        "CREATE TABLE IF NOT EXISTS tool_approval_policies (
            tool_name TEXT PRIMARY KEY NOT NULL,
            decision TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    ),
    (
        "v047_skill_resource_bundles",
        "ALTER TABLE skills ADD COLUMN resource_bundle_json TEXT DEFAULT NULL;",
    ),
    (
        "v048_remove_legacy_builtin_skill_rows",
        "DELETE FROM skills WHERE id LIKE 'builtin-%';",
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

        // v045 removes legacy built-in rows from DB; built-ins now live on
        // the filesystem (see crates/core/assets/skills/). The DB should
        // only contain user-created skills.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM skills WHERE id LIKE 'builtin-%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "legacy built-in skills should be removed");

        // `description` column must exist after v045.
        let has_desc: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('skills') WHERE name = 'description'",
                [],
                |row| row.get::<_, i64>(0).map(|n| n > 0),
            )
            .unwrap();
        assert!(has_desc, "skills.description column should exist");

        let has_resource_bundle: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('skills') WHERE name = 'resource_bundle_json'",
                [],
                |row| row.get::<_, i64>(0).map(|n| n > 0),
            )
            .unwrap();
        assert!(
            has_resource_bundle,
            "skills.resource_bundle_json column should exist"
        );
    }
}
