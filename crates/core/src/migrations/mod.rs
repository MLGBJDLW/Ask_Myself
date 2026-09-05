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
        "ALTER TABLE mcp_servers ADD COLUMN builtin_id TEXT;",
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
            evidence_snippet TEXT NOT NULL DEFAULT '',
            confidence REAL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT OR IGNORE INTO entity_links_new (
            id, source_entity_id, target_entity_id, relation_type,
            strength, evidence_doc_id, created_at
        )
        SELECT
            id, source_entity_id, target_entity_id, relation_type,
            strength, evidence_doc_id, created_at
        FROM entity_links;
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
When creating DOCX, XLSX, or PPTX files via Python Office packages.

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
            SET content = REPLACE(REPLACE(REPLACE(REPLACE(content,
                    'generate_pptx', 'Python Office packages'),
                    'generate_docx', 'Python Office packages'),
                    'generate_xlsx', 'Python Office packages'),
                    'ppt_generate', 'Python Office packages'),
                updated_at = datetime('now')
            WHERE id = 'builtin-office-document-design'
              AND (content LIKE '%generate_pptx%'
                   OR content LIKE '%generate_docx%'
                   OR content LIKE '%generate_xlsx%'
                   OR content LIKE '%ppt_generate%');",
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
    (
        "v049_agent_self_evolution",
        "CREATE TABLE IF NOT EXISTS agent_procedural_memories (
            id TEXT PRIMARY KEY NOT NULL,
            title TEXT NOT NULL,
            content TEXT NOT NULL,
            tags_json TEXT NOT NULL DEFAULT '[]',
            source TEXT NOT NULL DEFAULT 'agent',
            confidence REAL NOT NULL DEFAULT 0.7,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_agent_procedural_memories_updated
            ON agent_procedural_memories(updated_at);

        CREATE VIRTUAL TABLE IF NOT EXISTS fts_agent_procedural_memories USING fts5(
            title,
            content,
            tags,
            memory_id UNINDEXED,
            tokenize='unicode61 remove_diacritics 2'
        );
        CREATE TRIGGER IF NOT EXISTS agent_procedural_memories_fts_ai
        AFTER INSERT ON agent_procedural_memories BEGIN
            INSERT INTO fts_agent_procedural_memories(title, content, tags, memory_id)
            VALUES (new.title, new.content, new.tags_json, new.id);
        END;
        CREATE TRIGGER IF NOT EXISTS agent_procedural_memories_fts_ad
        AFTER DELETE ON agent_procedural_memories BEGIN
            DELETE FROM fts_agent_procedural_memories WHERE memory_id = old.id;
        END;
        CREATE TRIGGER IF NOT EXISTS agent_procedural_memories_fts_au
        AFTER UPDATE ON agent_procedural_memories BEGIN
            DELETE FROM fts_agent_procedural_memories WHERE memory_id = old.id;
            INSERT INTO fts_agent_procedural_memories(title, content, tags, memory_id)
            VALUES (new.title, new.content, new.tags_json, new.id);
        END;

        CREATE TABLE IF NOT EXISTS skill_change_proposals (
            id TEXT PRIMARY KEY NOT NULL,
            action TEXT NOT NULL CHECK(action IN ('create', 'patch')),
            skill_id TEXT,
            name TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            content TEXT NOT NULL,
            resource_bundle_json TEXT,
            rationale TEXT NOT NULL DEFAULT '',
            warnings_json TEXT NOT NULL DEFAULT '[]',
            status TEXT NOT NULL DEFAULT 'pending'
                CHECK(status IN ('pending', 'applied', 'rejected')),
            conversation_id TEXT REFERENCES conversations(id) ON DELETE SET NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            applied_at TEXT,
            rejected_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_skill_change_proposals_status
            ON skill_change_proposals(status, created_at);

        CREATE TABLE IF NOT EXISTS agent_evolution_events (
            id TEXT PRIMARY KEY NOT NULL,
            kind TEXT NOT NULL,
            severity TEXT NOT NULL DEFAULT 'info',
            summary TEXT NOT NULL,
            conversation_id TEXT REFERENCES conversations(id) ON DELETE SET NULL,
            trace_id TEXT,
            metadata_json TEXT NOT NULL DEFAULT '{}',
            status TEXT NOT NULL DEFAULT 'open',
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_agent_evolution_events_status
            ON agent_evolution_events(status, created_at);
        CREATE INDEX IF NOT EXISTS idx_agent_evolution_events_trace
            ON agent_evolution_events(trace_id);",
    ),
    (
        "v050_agent_task_runs",
        "CREATE TABLE IF NOT EXISTS agent_task_runs (
            id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
            turn_id TEXT NOT NULL REFERENCES conversation_turns(id) ON DELETE CASCADE,
            user_message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
            status TEXT NOT NULL DEFAULT 'queued',
            phase TEXT NOT NULL DEFAULT 'queued',
            title TEXT NOT NULL DEFAULT '',
            route_kind TEXT,
            summary TEXT,
            error_message TEXT,
            provider TEXT,
            model TEXT,
            plan_json TEXT,
            artifacts_json TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            started_at TEXT,
            finished_at TEXT
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_task_runs_turn
            ON agent_task_runs(turn_id);
        CREATE INDEX IF NOT EXISTS idx_agent_task_runs_conversation
            ON agent_task_runs(conversation_id, created_at);

        CREATE TABLE IF NOT EXISTS agent_task_run_events (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL REFERENCES agent_task_runs(id) ON DELETE CASCADE,
            event_type TEXT NOT NULL,
            label TEXT NOT NULL DEFAULT '',
            status TEXT,
            payload_json TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_agent_task_run_events_run
            ON agent_task_run_events(run_id, created_at);",
    ),
    (
        "v051_file_checkpoints",
        "CREATE TABLE IF NOT EXISTS file_checkpoints (
            id TEXT PRIMARY KEY,
            conversation_id TEXT REFERENCES conversations(id) ON DELETE SET NULL,
            tool_call_id TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            operation TEXT NOT NULL,
            path TEXT NOT NULL,
            absolute_path TEXT NOT NULL,
            existed_before INTEGER NOT NULL DEFAULT 0,
            content_before BLOB,
            bytes_before INTEGER NOT NULL DEFAULT 0,
            hash_before TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_file_checkpoints_conversation
            ON file_checkpoints(conversation_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_file_checkpoints_path
            ON file_checkpoints(absolute_path, created_at);",
    ),
    (
        "v052_playwright_browser_connector",
        "INSERT OR IGNORE INTO mcp_servers (id, name, transport, command, args, url, env_json, headers_json, enabled, builtin_id)
        VALUES (
            'builtin-playwright-browser',
            'Browser Automation',
            'streamable_http',
            'npx',
            '[\"-y\",\"@playwright/mcp@latest\",\"--port\",\"${PORT}\"]',
            NULL,
            NULL,
            NULL,
            0,
            'playwright-browser'
        );",
    ),
    (
        "v053_fix_playwright_browser_transport",
        "UPDATE mcp_servers
         SET transport = 'streamable_http',
             url = NULL,
             updated_at = datetime('now')
         WHERE id = 'builtin-playwright-browser'
           AND builtin_id = 'playwright-browser'
           AND transport != 'streamable_http';",
    ),
    (
        "v054_project_memories",
        "CREATE TABLE IF NOT EXISTS project_memories (
            id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            kind TEXT NOT NULL DEFAULT 'note',
            title TEXT NOT NULL DEFAULT '',
            content TEXT NOT NULL,
            source TEXT NOT NULL DEFAULT 'manual',
            pinned INTEGER NOT NULL DEFAULT 0,
            archived INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_project_memories_project
            ON project_memories(project_id, archived, pinned, updated_at);
        CREATE INDEX IF NOT EXISTS idx_project_memories_kind
            ON project_memories(project_id, kind);",
    ),
    (
        "v055_custom_personas",
        "CREATE TABLE IF NOT EXISTS personas (
            id TEXT PRIMARY KEY NOT NULL,
            name TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            instructions TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            default_skill_ids_json TEXT NOT NULL DEFAULT '[]',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_personas_enabled
            ON personas(enabled, created_at);
        ALTER TABLE conversations ADD COLUMN persona_id TEXT DEFAULT NULL;",
    ),
    (
        "v056_agent_subtask_runs",
        "CREATE TABLE IF NOT EXISTS agent_subtask_runs (
            id TEXT PRIMARY KEY,
            parent_run_id TEXT NOT NULL REFERENCES agent_task_runs(id) ON DELETE CASCADE,
            label TEXT NOT NULL DEFAULT '',
            role TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'queued',
            phase TEXT NOT NULL DEFAULT 'queued',
            input_json TEXT,
            output_json TEXT,
            error_message TEXT,
            token_budget INTEGER,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            started_at TEXT,
            finished_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_agent_subtask_runs_parent
            ON agent_subtask_runs(parent_run_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_agent_subtask_runs_status
            ON agent_subtask_runs(status, created_at);",
    ),
    (
        "v057_project_memory_lifecycle",
        "ALTER TABLE project_memories ADD COLUMN confidence REAL NOT NULL DEFAULT 0.75;
        ALTER TABLE project_memories ADD COLUMN expires_at TEXT DEFAULT NULL;
        ALTER TABLE project_memories ADD COLUMN conflict_status TEXT NOT NULL DEFAULT 'clear';
        CREATE INDEX IF NOT EXISTS idx_project_memories_lifecycle
            ON project_memories(project_id, archived, expires_at, conflict_status);",
    ),
    (
        "v058_agent_task_artifact_versions",
        "CREATE TABLE IF NOT EXISTS agent_task_artifacts (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL REFERENCES agent_task_runs(id) ON DELETE CASCADE,
            kind TEXT NOT NULL DEFAULT 'artifact',
            title TEXT NOT NULL DEFAULT '',
            summary TEXT,
            content TEXT NOT NULL DEFAULT '',
            paths_json TEXT NOT NULL DEFAULT '[]',
            payload_json TEXT,
            source TEXT NOT NULL DEFAULT 'manual',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_agent_task_artifacts_run
            ON agent_task_artifacts(run_id, updated_at);
        CREATE INDEX IF NOT EXISTS idx_agent_task_artifacts_kind
            ON agent_task_artifacts(run_id, kind);

        CREATE TABLE IF NOT EXISTS agent_task_artifact_versions (
            id TEXT PRIMARY KEY,
            artifact_id TEXT NOT NULL REFERENCES agent_task_artifacts(id) ON DELETE CASCADE,
            version INTEGER NOT NULL,
            title TEXT NOT NULL DEFAULT '',
            summary TEXT,
            content TEXT NOT NULL DEFAULT '',
            paths_json TEXT NOT NULL DEFAULT '[]',
            payload_json TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(artifact_id, version)
        );
        CREATE INDEX IF NOT EXISTS idx_agent_task_artifact_versions_artifact
            ON agent_task_artifact_versions(artifact_id, version DESC);",
    ),
    (
        "v059_agent_config_image_generation_model",
        "ALTER TABLE agent_configs ADD COLUMN image_generation_model TEXT DEFAULT NULL;",
    ),
    (
        "v060_tool_permission_policies",
        "CREATE TABLE IF NOT EXISTS tool_permission_policies (
            permission_key TEXT PRIMARY KEY NOT NULL,
            tool_name TEXT NOT NULL,
            target_kind TEXT NOT NULL,
            target_value TEXT NOT NULL,
            decision TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_tool_permission_policies_tool
            ON tool_permission_policies(tool_name, target_kind, target_value);",
    ),
    (
        "v061_skill_proposal_evidence",
        "ALTER TABLE skill_change_proposals ADD COLUMN source TEXT NOT NULL DEFAULT 'manual';
        ALTER TABLE skill_change_proposals ADD COLUMN confidence REAL NOT NULL DEFAULT 0.7;
        ALTER TABLE skill_change_proposals ADD COLUMN evidence_json TEXT NOT NULL DEFAULT '[]';
        CREATE INDEX IF NOT EXISTS idx_skill_change_proposals_source
            ON skill_change_proposals(source, created_at);",
    ),
    (
        "v062_remove_builtin_open_websearch",
        "DELETE FROM mcp_servers
         WHERE id = 'builtin-open-websearch'
           AND builtin_id = 'open-websearch';",
    ),
    (
        "v063_workflow_automation_resumability_governance",
        "CREATE TABLE IF NOT EXISTS workflow_automations (
            id TEXT PRIMARY KEY NOT NULL,
            name TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            workflow_template_id TEXT NOT NULL,
            prompt TEXT NOT NULL,
            trigger_json TEXT NOT NULL,
            trigger_kind TEXT NOT NULL,
            source_scope_json TEXT NOT NULL DEFAULT '[]',
            approval_policy_json TEXT NOT NULL DEFAULT '{}',
            enabled INTEGER NOT NULL DEFAULT 1,
            status TEXT NOT NULL DEFAULT 'ready',
            last_run_at TEXT DEFAULT NULL,
            next_run_at TEXT DEFAULT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_workflow_automations_due
            ON workflow_automations(enabled, trigger_kind, next_run_at);
        CREATE INDEX IF NOT EXISTS idx_workflow_automations_template
            ON workflow_automations(workflow_template_id, updated_at);

        CREATE TABLE IF NOT EXISTS workflow_automation_runs (
            id TEXT PRIMARY KEY NOT NULL,
            automation_id TEXT NOT NULL REFERENCES workflow_automations(id) ON DELETE CASCADE,
            task_run_id TEXT REFERENCES agent_task_runs(id) ON DELETE SET NULL,
            status TEXT NOT NULL DEFAULT 'queued',
            summary TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            finished_at TEXT DEFAULT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_workflow_automation_runs_automation
            ON workflow_automation_runs(automation_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_workflow_automation_runs_task
            ON workflow_automation_runs(task_run_id);

        CREATE TABLE IF NOT EXISTS task_resume_checkpoints (
            id TEXT PRIMARY KEY NOT NULL,
            run_id TEXT NOT NULL REFERENCES agent_task_runs(id) ON DELETE CASCADE,
            reason TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL,
            phase TEXT NOT NULL,
            state_json TEXT NOT NULL,
            resume_prompt TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_task_resume_checkpoints_run
            ON task_resume_checkpoints(run_id, created_at);

        CREATE TABLE IF NOT EXISTS skill_usage_events (
            id TEXT PRIMARY KEY NOT NULL,
            skill_id TEXT NOT NULL,
            conversation_id TEXT REFERENCES conversations(id) ON DELETE SET NULL,
            task_run_id TEXT REFERENCES agent_task_runs(id) ON DELETE SET NULL,
            outcome TEXT NOT NULL,
            evidence_json TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_skill_usage_events_skill
            ON skill_usage_events(skill_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_skill_usage_events_task
            ON skill_usage_events(task_run_id);

        CREATE TABLE IF NOT EXISTS memory_injection_events (
            id TEXT PRIMARY KEY NOT NULL,
            memory_id TEXT NOT NULL,
            conversation_id TEXT REFERENCES conversations(id) ON DELETE SET NULL,
            turn_id TEXT REFERENCES conversation_turns(id) ON DELETE SET NULL,
            query TEXT NOT NULL DEFAULT '',
            reason TEXT NOT NULL DEFAULT '',
            score REAL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_memory_injection_events_memory
            ON memory_injection_events(memory_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_memory_injection_events_turn
            ON memory_injection_events(turn_id);

        CREATE TABLE IF NOT EXISTS browser_evidence_captures (
            id TEXT PRIMARY KEY NOT NULL,
            url TEXT NOT NULL,
            final_url TEXT NOT NULL,
            title TEXT NOT NULL DEFAULT '',
            excerpt TEXT NOT NULL DEFAULT '',
            method TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_browser_evidence_captures_url
            ON browser_evidence_captures(final_url, created_at);",
    ),
    (
        "v064_backfill_document_entities_from_first_seen_doc",
        "INSERT OR IGNORE INTO document_entities (document_id, entity_id, relevance, context_snippet)
         SELECT e.first_seen_doc, e.id, 1.0, COALESCE(NULLIF(e.description, ''), e.name)
         FROM entities e
         JOIN documents d ON d.id = e.first_seen_doc
         WHERE e.first_seen_doc IS NOT NULL
           AND TRIM(e.first_seen_doc) <> '';",
    ),
    (
        "v065_agent_run_events",
        "CREATE TABLE IF NOT EXISTS agent_run_events (
            run_id TEXT NOT NULL,
            turn_id TEXT NOT NULL,
            event_seq INTEGER NOT NULL,
            version INTEGER NOT NULL,
            kind TEXT NOT NULL,
            phase TEXT NOT NULL,
            label TEXT NOT NULL DEFAULT '',
            status TEXT,
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (run_id, event_seq)
        );
        CREATE INDEX IF NOT EXISTS idx_agent_run_events_run
            ON agent_run_events(run_id, event_seq);
        CREATE INDEX IF NOT EXISTS idx_agent_run_events_turn
            ON agent_run_events(turn_id, event_seq);
        CREATE INDEX IF NOT EXISTS idx_agent_run_events_kind
            ON agent_run_events(kind, created_at);",
    ),
    (
        "v066_agent_trajectories",
        "CREATE TABLE IF NOT EXISTS agent_trajectories (
            trajectory_id TEXT PRIMARY KEY NOT NULL,
            schema_version INTEGER NOT NULL,
            source_kind TEXT NOT NULL,
            source_run_id TEXT,
            user_input_summary TEXT NOT NULL DEFAULT '',
            outcome TEXT,
            event_count INTEGER NOT NULL DEFAULT 0,
            tool_call_count INTEGER NOT NULL DEFAULT 0,
            approval_count INTEGER NOT NULL DEFAULT 0,
            task_run_count INTEGER NOT NULL DEFAULT 0,
            redaction_profile TEXT NOT NULL,
            trajectory_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_agent_trajectories_source
            ON agent_trajectories(source_kind, source_run_id);
        CREATE INDEX IF NOT EXISTS idx_agent_trajectories_created
            ON agent_trajectories(created_at);
        CREATE INDEX IF NOT EXISTS idx_agent_trajectories_outcome
            ON agent_trajectories(outcome, created_at);",
    ),
    (
        "v067_package_host_state",
        "CREATE TABLE IF NOT EXISTS package_host_state (
            package_id TEXT PRIMARY KEY NOT NULL,
            lifecycle_state TEXT NOT NULL,
            health_state TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_package_host_state_lifecycle
            ON package_host_state(lifecycle_state, health_state);",
    ),
    (
        "v068_workflow_automation_scheduler_events",
        "CREATE TABLE IF NOT EXISTS workflow_automation_scheduler_events (
            id TEXT PRIMARY KEY NOT NULL,
            automation_id TEXT REFERENCES workflow_automations(id) ON DELETE CASCADE,
            run_id TEXT REFERENCES workflow_automation_runs(id) ON DELETE SET NULL,
            event_type TEXT NOT NULL,
            status TEXT,
            summary TEXT NOT NULL DEFAULT '',
            payload_json TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_workflow_scheduler_events_automation
            ON workflow_automation_scheduler_events(automation_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_workflow_scheduler_events_run
            ON workflow_automation_scheduler_events(run_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_workflow_scheduler_events_type
            ON workflow_automation_scheduler_events(event_type, created_at);",
    ),
    (
        "v069_dreaming_review_artifacts",
        "CREATE TABLE IF NOT EXISTS dream_runs (
            id TEXT PRIMARY KEY,
            trigger_kind TEXT NOT NULL,
            scope_json TEXT NOT NULL,
            status TEXT NOT NULL,
            phase TEXT,
            summary TEXT,
            stats_json TEXT NOT NULL DEFAULT '{}',
            error TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            started_at TEXT,
            finished_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_dream_runs_status
            ON dream_runs(status, created_at);
        CREATE INDEX IF NOT EXISTS idx_dream_runs_trigger
            ON dream_runs(trigger_kind, created_at);

        CREATE TABLE IF NOT EXISTS dream_run_events (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL REFERENCES dream_runs(id) ON DELETE CASCADE,
            event_type TEXT NOT NULL,
            status TEXT,
            summary TEXT,
            payload_json TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_dream_run_events_run
            ON dream_run_events(run_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_dream_run_events_type
            ON dream_run_events(event_type, created_at);

        CREATE TABLE IF NOT EXISTS dream_artifacts (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL REFERENCES dream_runs(id) ON DELETE CASCADE,
            kind TEXT NOT NULL,
            status TEXT NOT NULL,
            title TEXT NOT NULL,
            summary TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            evidence_json TEXT NOT NULL,
            application_json TEXT NOT NULL DEFAULT '{}',
            confidence REAL NOT NULL,
            review_required INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            applied_at TEXT,
            rejected_at TEXT,
            undone_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_dream_artifacts_run
            ON dream_artifacts(run_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_dream_artifacts_status
            ON dream_artifacts(status, created_at);
        CREATE INDEX IF NOT EXISTS idx_dream_artifacts_kind
            ON dream_artifacts(kind, status, created_at);",
    ),
    (
        "v070_entity_aliases",
        "CREATE TABLE IF NOT EXISTS entity_aliases (
            entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
            alias TEXT NOT NULL,
            normalized_alias TEXT NOT NULL,
            entity_type TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY(entity_id, normalized_alias)
        );
        CREATE INDEX IF NOT EXISTS idx_entity_aliases_lookup
            ON entity_aliases(normalized_alias, entity_type);
        INSERT OR IGNORE INTO entity_aliases (entity_id, alias, normalized_alias, entity_type)
        SELECT id, name, lower(trim(name)), entity_type
        FROM entities
        WHERE trim(name) <> '';",
    ),
    (
        "v071_entity_link_evidence_snippet",
        "ALTER TABLE entity_links ADD COLUMN evidence_snippet TEXT NOT NULL DEFAULT '';",
    ),
    (
        "v072_entity_link_confidence",
        "ALTER TABLE entity_links ADD COLUMN confidence REAL;",
    ),
    (
        "v073_conversation_archiving",
        "ALTER TABLE conversations ADD COLUMN archived_at TEXT;
        CREATE INDEX IF NOT EXISTS idx_conversations_archived_at
            ON conversations(archived_at);",
    ),
    (
        "v074_conversation_goals",
        "CREATE TABLE IF NOT EXISTS conversation_goals (
            conversation_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
            id TEXT NOT NULL,
            objective TEXT NOT NULL,
            status TEXT NOT NULL CHECK(status IN ('active', 'blocked', 'complete')),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            completed_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_conversation_goals_status
            ON conversation_goals(status, updated_at);",
    ),
    (
        "v075_agent_turn_idempotency",
        "ALTER TABLE agent_task_runs ADD COLUMN idempotency_key TEXT;
        CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_task_runs_idempotency
            ON agent_task_runs(conversation_id, idempotency_key)
            WHERE idempotency_key IS NOT NULL;",
    ),
    (
        "v076_agent_run_event_presentation",
        "ALTER TABLE agent_run_events ADD COLUMN visibility TEXT NOT NULL DEFAULT 'user';
        ALTER TABLE agent_run_events ADD COLUMN persistence TEXT NOT NULL DEFAULT 'durable';
        ALTER TABLE agent_run_events ADD COLUMN display_kind TEXT NOT NULL DEFAULT 'status';
        ALTER TABLE agent_run_events ADD COLUMN importance TEXT NOT NULL DEFAULT 'normal';
        UPDATE agent_run_events
        SET visibility = CASE kind
                WHEN 'usageUpdated' THEN 'developer'
                ELSE 'user'
            END,
            display_kind = CASE kind
                WHEN 'outputDelta' THEN 'output'
                WHEN 'thinking' THEN 'reasoning'
                WHEN 'planUpdated' THEN 'plan'
                WHEN 'toolPreparing' THEN 'tool'
                WHEN 'toolStarted' THEN 'tool'
                WHEN 'toolProgress' THEN 'tool'
                WHEN 'toolCompleted' THEN 'tool'
                WHEN 'approvalRequested' THEN 'approval'
                WHEN 'approvalResolved' THEN 'approval'
                WHEN 'streamReset' THEN 'recovery'
                WHEN 'recoveryAttempt' THEN 'recovery'
                WHEN 'usageUpdated' THEN 'usage'
                WHEN 'autoCompacted' THEN 'compaction'
                WHEN 'done' THEN 'completion'
                WHEN 'error' THEN 'error'
                ELSE 'status'
            END,
            importance = CASE kind
                WHEN 'approvalRequested' THEN 'high'
                WHEN 'approvalResolved' THEN 'high'
                WHEN 'streamReset' THEN 'high'
                WHEN 'recoveryAttempt' THEN 'high'
                WHEN 'done' THEN 'high'
                WHEN 'error' THEN 'high'
                WHEN 'usageUpdated' THEN 'low'
                ELSE 'normal'
            END;",
    ),
    (
        "v077_backfill_agent_run_events_from_task_events",
        "INSERT INTO agent_run_events (
            run_id, turn_id, event_seq, version, kind, phase,
            visibility, persistence, display_kind, importance,
            label, status, payload_json, created_at
         )
         SELECT
            COALESCE(NULLIF(json_extract(e.payload_json, '$.agentRun.runId'), ''), e.run_id),
            COALESCE(NULLIF(json_extract(e.payload_json, '$.agentRun.turnId'), ''), r.turn_id),
            CAST(json_extract(e.payload_json, '$.agentRun.eventSeq') AS INTEGER),
            COALESCE(CAST(json_extract(e.payload_json, '$.agentRun.version') AS INTEGER), 2),
            json_extract(e.payload_json, '$.agentRun.kind'),
            json_extract(e.payload_json, '$.agentRun.phase'),
            COALESCE(NULLIF(json_extract(e.payload_json, '$.agentRun.visibility'), ''), 'user'),
            COALESCE(NULLIF(json_extract(e.payload_json, '$.agentRun.persistence'), ''), 'durable'),
            COALESCE(NULLIF(json_extract(e.payload_json, '$.agentRun.displayKind'), ''),
                CASE json_extract(e.payload_json, '$.agentRun.kind')
                    WHEN 'outputDelta' THEN 'output'
                    WHEN 'thinking' THEN 'reasoning'
                    WHEN 'planUpdated' THEN 'plan'
                    WHEN 'toolPreparing' THEN 'tool'
                    WHEN 'toolStarted' THEN 'tool'
                    WHEN 'toolProgress' THEN 'tool'
                    WHEN 'toolCompleted' THEN 'tool'
                    WHEN 'approvalRequested' THEN 'approval'
                    WHEN 'approvalResolved' THEN 'approval'
                    WHEN 'streamReset' THEN 'recovery'
                    WHEN 'recoveryAttempt' THEN 'recovery'
                    WHEN 'usageUpdated' THEN 'usage'
                    WHEN 'autoCompacted' THEN 'compaction'
                    WHEN 'done' THEN 'completion'
                    WHEN 'error' THEN 'error'
                    ELSE 'status'
                END),
            COALESCE(NULLIF(json_extract(e.payload_json, '$.agentRun.importance'), ''),
                CASE json_extract(e.payload_json, '$.agentRun.kind')
                    WHEN 'approvalRequested' THEN 'high'
                    WHEN 'approvalResolved' THEN 'high'
                    WHEN 'streamReset' THEN 'high'
                    WHEN 'recoveryAttempt' THEN 'high'
                    WHEN 'done' THEN 'high'
                    WHEN 'error' THEN 'high'
                    WHEN 'usageUpdated' THEN 'low'
                    ELSE 'normal'
                END),
            COALESCE(json_extract(e.payload_json, '$.agentRun.label'), e.label, ''),
            COALESCE(json_extract(e.payload_json, '$.agentRun.status'), e.status),
            COALESCE(json_extract(e.payload_json, '$.agentRun.payload'), '{}'),
            COALESCE(json_extract(e.payload_json, '$.agentRun.createdAt'), e.created_at)
         FROM agent_task_run_events e
         JOIN agent_task_runs r ON r.id = e.run_id
         WHERE json_valid(e.payload_json)
           AND json_type(e.payload_json, '$.agentRun') = 'object'
           AND CAST(json_extract(e.payload_json, '$.agentRun.eventSeq') AS INTEGER) > 0
           AND NULLIF(json_extract(e.payload_json, '$.agentRun.kind'), '') IS NOT NULL
           AND NULLIF(json_extract(e.payload_json, '$.agentRun.phase'), '') IS NOT NULL
           AND COALESCE(NULLIF(json_extract(e.payload_json, '$.agentRun.persistence'), ''), 'durable') = 'durable'
         ON CONFLICT(run_id, event_seq) DO NOTHING;",
    ),
    (
        "v078_migrate_tool_approval_policies",
        "INSERT OR IGNORE INTO tool_permission_policies (
            permission_key, tool_name, target_kind, target_value, decision, created_at
         )
         SELECT
            tool_name || '|tool|*', tool_name, 'tool', '*', decision, created_at
         FROM tool_approval_policies;
         DROP TABLE tool_approval_policies;",
    ),
    (
        "v079_migrate_qwen_payg_provider_ids",
        "UPDATE conversations
         SET provider = 'alibaba_model_studio'
         WHERE lower(trim(provider)) = 'qwen'
           AND EXISTS (
             SELECT 1 FROM agent_configs config
             WHERE lower(trim(config.provider)) = 'qwen'
               AND lower(trim(config.model)) = lower(trim(conversations.model))
               AND lower(COALESCE(config.base_url, '')) NOT LIKE '%token-plan.%'
               AND (
                 lower(COALESCE(config.base_url, '')) LIKE '%dashscope%'
                 OR lower(COALESCE(config.base_url, '')) LIKE '%maas.aliyuncs.com%'
               )
           );
         UPDATE agent_task_runs
         SET provider = 'alibaba_model_studio'
         WHERE lower(trim(COALESCE(provider, ''))) = 'qwen'
           AND EXISTS (
             SELECT 1 FROM agent_configs config
             WHERE lower(trim(config.provider)) = 'qwen'
               AND lower(trim(config.model)) = lower(trim(COALESCE(agent_task_runs.model, '')))
               AND lower(COALESCE(config.base_url, '')) NOT LIKE '%token-plan.%'
               AND (
                 lower(COALESCE(config.base_url, '')) LIKE '%dashscope%'
                 OR lower(COALESCE(config.base_url, '')) LIKE '%maas.aliyuncs.com%'
               )
           );
         UPDATE agent_configs
         SET summarization_provider = 'alibaba_model_studio'
         WHERE lower(trim(provider)) = 'qwen'
           AND lower(trim(COALESCE(summarization_provider, ''))) = 'qwen'
           AND lower(COALESCE(base_url, '')) NOT LIKE '%token-plan.%'
           AND (
             lower(COALESCE(base_url, '')) LIKE '%dashscope%'
             OR lower(COALESCE(base_url, '')) LIKE '%maas.aliyuncs.com%'
           );
         UPDATE agent_configs
         SET provider = 'alibaba_model_studio'
         WHERE lower(trim(provider)) = 'qwen'
           AND lower(COALESCE(base_url, '')) NOT LIKE '%token-plan.%'
           AND (
             lower(COALESCE(base_url, '')) LIKE '%dashscope%'
             OR lower(COALESCE(base_url, '')) LIKE '%maas.aliyuncs.com%'
           );",
    ),
    (
        "v080_canonical_ai_usage_accounting",
        "CREATE TABLE IF NOT EXISTS ai_usage_records (
            id TEXT PRIMARY KEY NOT NULL,
            invocation_id TEXT NOT NULL UNIQUE,
            occurred_at TEXT NOT NULL DEFAULT (datetime('now')),
            provider_id TEXT NOT NULL DEFAULT 'unknown',
            provider_type TEXT NOT NULL DEFAULT 'unknown',
            model_id TEXT NOT NULL DEFAULT 'unknown',
            raw_model_id TEXT,
            modality TEXT NOT NULL DEFAULT 'language_model',
            operation_kind TEXT NOT NULL DEFAULT 'agent_main',
            conversation_id TEXT,
            turn_id TEXT,
            run_id TEXT,
            subtask_run_id TEXT,
            project_id TEXT,
            prompt_tokens INTEGER NOT NULL DEFAULT 0 CHECK(prompt_tokens >= 0),
            completion_tokens INTEGER NOT NULL DEFAULT 0 CHECK(completion_tokens >= 0),
            thinking_tokens INTEGER NOT NULL DEFAULT 0 CHECK(thinking_tokens >= 0),
            total_tokens INTEGER NOT NULL DEFAULT 0 CHECK(total_tokens >= 0),
            cache_read_tokens INTEGER NOT NULL DEFAULT 0 CHECK(cache_read_tokens >= 0),
            cache_miss_tokens INTEGER NOT NULL DEFAULT 0 CHECK(cache_miss_tokens >= 0),
            cache_creation_tokens INTEGER NOT NULL DEFAULT 0 CHECK(cache_creation_tokens >= 0),
            usage_source TEXT NOT NULL DEFAULT 'provider',
            request_status TEXT NOT NULL DEFAULT 'success',
            latency_ms INTEGER,
            estimated_cost_micros INTEGER,
            currency TEXT,
            pricing_version TEXT,
            provider_raw_json TEXT NOT NULL DEFAULT '{}'
        );
        CREATE INDEX IF NOT EXISTS idx_ai_usage_occurred_at
            ON ai_usage_records(occurred_at);
        CREATE INDEX IF NOT EXISTS idx_ai_usage_provider_model
            ON ai_usage_records(provider_id, model_id, occurred_at);
        CREATE INDEX IF NOT EXISTS idx_ai_usage_operation
            ON ai_usage_records(operation_kind, occurred_at);
        CREATE INDEX IF NOT EXISTS idx_ai_usage_run
            ON ai_usage_records(run_id);

        CREATE TABLE IF NOT EXISTS ai_model_pricing (
            id TEXT PRIMARY KEY NOT NULL,
            provider_id TEXT NOT NULL,
            model_id TEXT NOT NULL,
            effective_from TEXT NOT NULL,
            effective_to TEXT,
            input_micros_per_million INTEGER,
            output_micros_per_million INTEGER,
            cache_read_micros_per_million INTEGER,
            cache_write_micros_per_million INTEGER,
            currency TEXT NOT NULL DEFAULT 'USD',
            pricing_version TEXT NOT NULL,
            UNIQUE(provider_id, model_id, effective_from)
        );
        CREATE INDEX IF NOT EXISTS idx_ai_model_pricing_lookup
            ON ai_model_pricing(provider_id, model_id, effective_from, effective_to);

        INSERT OR IGNORE INTO ai_usage_records (
            id, invocation_id, occurred_at, provider_id, provider_type,
            model_id, raw_model_id, modality, operation_kind,
            conversation_id, turn_id, run_id,
            prompt_tokens, completion_tokens, thinking_tokens, total_tokens,
            cache_read_tokens, cache_miss_tokens, cache_creation_tokens,
            usage_source, request_status, provider_raw_json
        )
        SELECT
            'legacy-' || r.id,
            'legacy-run:' || r.id,
            COALESCE(e.created_at, r.finished_at, r.created_at),
            COALESCE(NULLIF(r.provider, ''), 'unknown'),
            COALESCE(NULLIF(r.provider, ''), 'unknown'),
            COALESCE(NULLIF(r.model, ''), 'unknown'),
            r.model,
            'language_model',
            'legacy_unclassified',
            r.conversation_id,
            r.turn_id,
            r.id,
            MAX(0, COALESCE(json_extract(e.payload_json, '$.usageTotal.promptTokens'), 0)),
            MAX(0, COALESCE(json_extract(e.payload_json, '$.usageTotal.completionTokens'), 0)),
            MAX(0, COALESCE(json_extract(e.payload_json, '$.usageTotal.thinkingTokens'), 0)),
            MAX(
                COALESCE(json_extract(e.payload_json, '$.usageTotal.totalTokens'), 0),
                COALESCE(json_extract(e.payload_json, '$.usageTotal.promptTokens'), 0)
                    + COALESCE(json_extract(e.payload_json, '$.usageTotal.completionTokens'), 0)
            ),
            MAX(0, COALESCE(json_extract(e.payload_json, '$.usageTotal.cacheReadTokens'), 0)),
            MAX(0, COALESCE(json_extract(e.payload_json, '$.usageTotal.cacheMissTokens'), 0)),
            MAX(0, COALESCE(json_extract(e.payload_json, '$.usageTotal.cacheCreationTokens'), 0)),
            'legacy_migrated',
            CASE WHEN r.status = 'failed' THEN 'error' ELSE 'success' END,
            COALESCE(json_extract(e.payload_json, '$.usageTotal'), '{}')
        FROM agent_task_runs r
        JOIN agent_run_events e ON e.run_id = r.id
        WHERE NOT EXISTS (
            SELECT 1 FROM _migrations
            WHERE name = 'v080_canonical_ai_usage_accounting'
        )
          AND e.event_seq = (
            SELECT MAX(latest.event_seq)
            FROM agent_run_events latest
            WHERE latest.run_id = r.id
              AND latest.kind IN ('usageUpdated', 'done')
              AND json_valid(latest.payload_json)
              AND json_type(latest.payload_json, '$.usageTotal') = 'object'
        );

        INSERT OR IGNORE INTO ai_usage_records (
            id, invocation_id, occurred_at, provider_id, provider_type,
            model_id, raw_model_id, modality, operation_kind,
            conversation_id, turn_id, run_id, subtask_run_id,
            prompt_tokens, completion_tokens, thinking_tokens, total_tokens,
            cache_read_tokens, cache_miss_tokens, cache_creation_tokens,
            usage_source, request_status, provider_raw_json
        )
        SELECT
            'legacy-subtask-' || s.id,
            'legacy-subtask:' || s.id,
            COALESCE(s.finished_at, s.updated_at, s.created_at),
            COALESCE(NULLIF(r.provider, ''), 'unknown'),
            COALESCE(NULLIF(r.provider, ''), 'unknown'),
            COALESCE(NULLIF(r.model, ''), 'unknown'),
            r.model,
            'language_model',
            'subagent',
            r.conversation_id,
            r.turn_id,
            r.id,
            s.id,
            MAX(0, COALESCE(
                json_extract(s.output_json, '$.run.usageTotal.promptTokens'),
                json_extract(s.output_json, '$.usageTotal.promptTokens'), 0
            )),
            MAX(0, COALESCE(
                json_extract(s.output_json, '$.run.usageTotal.completionTokens'),
                json_extract(s.output_json, '$.usageTotal.completionTokens'), 0
            )),
            MAX(0, COALESCE(
                json_extract(s.output_json, '$.run.usageTotal.thinkingTokens'),
                json_extract(s.output_json, '$.usageTotal.thinkingTokens'), 0
            )),
            MAX(
                COALESCE(
                    json_extract(s.output_json, '$.run.usageTotal.totalTokens'),
                    json_extract(s.output_json, '$.usageTotal.totalTokens'), 0
                ),
                COALESCE(
                    json_extract(s.output_json, '$.run.usageTotal.promptTokens'),
                    json_extract(s.output_json, '$.usageTotal.promptTokens'), 0
                ) + COALESCE(
                    json_extract(s.output_json, '$.run.usageTotal.completionTokens'),
                    json_extract(s.output_json, '$.usageTotal.completionTokens'), 0
                )
            ),
            MAX(0, COALESCE(
                json_extract(s.output_json, '$.run.usageTotal.cacheReadTokens'),
                json_extract(s.output_json, '$.usageTotal.cacheReadTokens'), 0
            )),
            MAX(0, COALESCE(
                json_extract(s.output_json, '$.run.usageTotal.cacheMissTokens'),
                json_extract(s.output_json, '$.usageTotal.cacheMissTokens'), 0
            )),
            MAX(0, COALESCE(
                json_extract(s.output_json, '$.run.usageTotal.cacheCreationTokens'),
                json_extract(s.output_json, '$.usageTotal.cacheCreationTokens'), 0
            )),
            'legacy_migrated',
            CASE WHEN s.status = 'failed' THEN 'error' ELSE 'success' END,
            COALESCE(
                json_extract(s.output_json, '$.run.usageTotal'),
                json_extract(s.output_json, '$.usageTotal'), '{}'
            )
        FROM agent_subtask_runs s
        JOIN agent_task_runs r ON r.id = s.parent_run_id
        WHERE NOT EXISTS (
            SELECT 1 FROM _migrations
            WHERE name = 'v080_canonical_ai_usage_accounting'
        )
          AND json_valid(s.output_json)
          AND (
            json_type(s.output_json, '$.run.usageTotal') = 'object'
            OR json_type(s.output_json, '$.usageTotal') = 'object'
          );",
    ),
    (
        "v081_activity_runtime",
        "CREATE TABLE IF NOT EXISTS activity_records (
            activity_id TEXT PRIMARY KEY NOT NULL,
            state TEXT NOT NULL,
            conversation_id TEXT,
            task_run_id TEXT,
            updated_at TEXT NOT NULL,
            record_json TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_activity_records_conversation
            ON activity_records(conversation_id, updated_at);
        CREATE INDEX IF NOT EXISTS idx_activity_records_task_run
            ON activity_records(task_run_id, updated_at);
        CREATE INDEX IF NOT EXISTS idx_activity_records_state
            ON activity_records(state, updated_at);
        CREATE TABLE IF NOT EXISTS activity_events (
            activity_id TEXT NOT NULL REFERENCES activity_records(activity_id) ON DELETE CASCADE,
            seq INTEGER NOT NULL CHECK(seq > 0),
            kind TEXT NOT NULL,
            timestamp TEXT NOT NULL,
            event_json TEXT NOT NULL,
            PRIMARY KEY(activity_id, seq)
        );
        CREATE INDEX IF NOT EXISTS idx_activity_events_timestamp
            ON activity_events(timestamp);",
    ),
    (
        "v082_controller_event_visibility",
        "UPDATE agent_run_events
         SET visibility = 'developer', importance = 'low'
         WHERE kind = 'planUpdated'
            OR (kind = 'status' AND phase IN ('routing', 'planning'));",
    ),
    (
        "v083_model_catalog_endpoint_identity",
        "ALTER TABLE agent_configs ADD COLUMN provider_endpoint_id TEXT;",
    ),
    (
        "v084_model_catalog_model_identity",
        "ALTER TABLE agent_configs ADD COLUMN model_id TEXT;",
    ),
    (
        "v085_model_catalog_identity_backfill",
        "UPDATE agent_configs
         SET model_id = COALESCE(model_id, model),
             provider_endpoint_id = COALESCE(provider_endpoint_id, CASE
                 WHEN rtrim(lower(COALESCE(base_url, '')), '/') = 'https://api.openai.com/v1' THEN 'text:openai'
                 WHEN rtrim(lower(COALESCE(base_url, '')), '/') = 'https://openrouter.ai/api/v1' THEN 'text:openrouter'
                 WHEN rtrim(lower(COALESCE(base_url, '')), '/') = 'https://api.anthropic.com/v1' THEN 'text:anthropic'
                 WHEN rtrim(lower(COALESCE(base_url, '')), '/') = 'https://generativelanguage.googleapis.com/v1beta' THEN 'text:google'
                 WHEN rtrim(lower(COALESCE(base_url, '')), '/') = 'https://api.deepseek.com' THEN 'text:deepseek'
                 WHEN rtrim(lower(COALESCE(base_url, '')), '/') = 'https://api.x.ai/v1' THEN 'text:xai'
                 WHEN rtrim(lower(COALESCE(base_url, '')), '/') = 'https://api.minimax.io/v1' THEN 'text:minimax'
                 WHEN rtrim(lower(COALESCE(base_url, '')), '/') = 'https://api.mistral.ai/v1' THEN 'text:mistral'
                 WHEN rtrim(lower(COALESCE(base_url, '')), '/') = 'http://localhost:11434' THEN 'text:ollama'
                 WHEN rtrim(lower(COALESCE(base_url, '')), '/') = 'http://localhost:1234/v1' THEN 'text:lmstudio'
                 WHEN rtrim(lower(COALESCE(base_url, '')), '/') = 'https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1' THEN 'text:qwen-token-plan-cn'
                 WHEN rtrim(lower(COALESCE(base_url, '')), '/') = 'https://dashscope-intl.aliyuncs.com/compatible-mode/v1' THEN 'text:qwen-cloud-intl'
                 WHEN rtrim(lower(COALESCE(base_url, '')), '/') = 'https://dashscope.aliyuncs.com/compatible-mode/v1' THEN 'text:alibaba-model-studio'
                 WHEN lower(provider) = 'open_ai' AND COALESCE(base_url, '') = '' THEN 'text:openai'
                 WHEN lower(provider) = 'deep_seek' THEN 'text:deepseek'
                 WHEN lower(provider) = 'lm_studio' THEN 'text:lmstudio'
                 ELSE NULL
             END)
         WHERE model_id IS NULL OR provider_endpoint_id IS NULL;",
    ),
    (
        "v086_delegation_limits_v2",
        "ALTER TABLE agent_configs ADD COLUMN delegation_limits_v2_json TEXT;",
    ),
    (
        "v087_task_center_summary_indexes",
        "CREATE INDEX IF NOT EXISTS idx_agent_task_runs_recency
             ON agent_task_runs(updated_at DESC, created_at DESC, id DESC);
         CREATE INDEX IF NOT EXISTS idx_agent_task_runs_status_recency
             ON agent_task_runs(status, updated_at DESC, created_at DESC, id DESC);",
    ),
    (
        "v088_ai_usage_runtime_observability",
        "ALTER TABLE ai_usage_records ADD COLUMN time_to_first_token_ms INTEGER;
         ALTER TABLE ai_usage_records ADD COLUMN upstream_provider_id TEXT;
         ALTER TABLE ai_usage_records ADD COLUMN cache_outcome_reason TEXT;",
    ),
    (
        "v089_non_destructive_context_compactions",
        "ALTER TABLE conversations ADD COLUMN active_context_compaction_id TEXT;",
    ),
    (
        "v090_context_compaction_checkpoints",
        "CREATE TABLE IF NOT EXISTS context_compactions (
             id TEXT PRIMARY KEY NOT NULL,
             operation_id TEXT NOT NULL UNIQUE,
             conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
             idempotency_key TEXT NOT NULL,
             snapshot_high_watermark INTEGER NOT NULL,
             snapshot_hash TEXT NOT NULL,
             summary TEXT NOT NULL,
             retained_tail_json TEXT NOT NULL,
             retained_start_sort_order INTEGER NOT NULL,
             tokens_before INTEGER NOT NULL,
             tokens_after INTEGER NOT NULL,
             provider TEXT NOT NULL,
             model TEXT NOT NULL,
             usage_json TEXT,
             status TEXT NOT NULL CHECK(status IN ('completed', 'invalidated')),
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(conversation_id, idempotency_key)
         );
         CREATE INDEX IF NOT EXISTS idx_context_compactions_conversation
             ON context_compactions(conversation_id, created_at DESC);
         CREATE INDEX IF NOT EXISTS idx_context_compactions_snapshot
             ON context_compactions(conversation_id, snapshot_high_watermark);",
    ),
    (
        "v091_interaction_requests",
        "CREATE TABLE IF NOT EXISTS interaction_queue_clock (
             sequence INTEGER PRIMARY KEY AUTOINCREMENT,
             created_at TEXT NOT NULL DEFAULT (datetime('now'))
         );

         CREATE TABLE IF NOT EXISTS interaction_requests (
             id TEXT PRIMARY KEY NOT NULL,
             conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
             turn_id TEXT NOT NULL REFERENCES conversation_turns(id) ON DELETE CASCADE,
             run_id TEXT NOT NULL REFERENCES agent_task_runs(id) ON DELETE CASCADE,
             tool_call_id TEXT,
             idempotency_key TEXT NOT NULL,
             kind TEXT NOT NULL CHECK(kind IN (
                 'user_input',
                 'approval',
                 'high_risk_confirmation',
                 'credential_request',
                 'conflict_resolution'
             )),
             title TEXT NOT NULL,
             description TEXT,
             questions_json TEXT NOT NULL,
             required INTEGER NOT NULL DEFAULT 1 CHECK(required IN (0, 1)),
             status TEXT NOT NULL CHECK(status IN (
                 'pending',
                 'presented',
                 'partially_answered',
                 'submitted',
                 'acknowledged',
                 'cancelled',
                 'expired',
                 'superseded',
                 'failed'
             )),
             risk_priority INTEGER NOT NULL DEFAULT 0,
             queue_sequence INTEGER NOT NULL UNIQUE,
             resume_token TEXT NOT NULL UNIQUE,
             expires_at TEXT,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(conversation_id, turn_id, idempotency_key)
         );
         CREATE INDEX IF NOT EXISTS idx_interaction_requests_queue
             ON interaction_requests(status, risk_priority DESC, queue_sequence);
         CREATE INDEX IF NOT EXISTS idx_interaction_requests_conversation
             ON interaction_requests(conversation_id, status, risk_priority DESC, queue_sequence);

         CREATE TABLE IF NOT EXISTS interaction_events (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             interaction_id TEXT NOT NULL REFERENCES interaction_requests(id) ON DELETE CASCADE,
             from_status TEXT,
             to_status TEXT NOT NULL,
             reason TEXT,
             created_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE INDEX IF NOT EXISTS idx_interaction_events_request
             ON interaction_events(interaction_id, id);

         CREATE TABLE IF NOT EXISTS interaction_responses (
             id TEXT PRIMARY KEY NOT NULL,
             interaction_id TEXT NOT NULL UNIQUE REFERENCES interaction_requests(id) ON DELETE CASCADE,
             answers_json TEXT NOT NULL,
             submitted_at TEXT NOT NULL DEFAULT (datetime('now'))
         );",
    ),
    (
        "v092_interaction_response_launch_key",
        "ALTER TABLE interaction_responses ADD COLUMN launch_idempotency_key TEXT;",
    ),
    (
        "v093_interaction_response_message",
        "ALTER TABLE interaction_responses ADD COLUMN response_message_id TEXT REFERENCES messages(id);",
    ),
    (
        "v094_interaction_response_resume_indexes",
        "CREATE INDEX IF NOT EXISTS idx_interaction_responses_launch_key
             ON interaction_responses(launch_idempotency_key)
             WHERE launch_idempotency_key IS NOT NULL;
         CREATE UNIQUE INDEX IF NOT EXISTS idx_interaction_responses_message
             ON interaction_responses(response_message_id)
             WHERE response_message_id IS NOT NULL;",
    ),
    (
        "v095_settings_schema_v2",
        "CREATE TABLE IF NOT EXISTS settings_profiles_v2 (
             id TEXT PRIMARY KEY NOT NULL,
             schema_version INTEGER NOT NULL CHECK (schema_version = 2),
             revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
             scope_kind TEXT NOT NULL CHECK (
                 scope_kind IN ('application', 'workspace', 'agent', 'task')
             ),
             scope_id TEXT NOT NULL DEFAULT '',
             name TEXT NOT NULL,
             preset_id TEXT,
             preset_version INTEGER,
             preset_hash TEXT,
             document_json TEXT NOT NULL CHECK (json_valid(document_json)),
             managed_by TEXT,
             legacy_source_id TEXT,
             source_fingerprint TEXT,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(scope_kind, scope_id)
         );
         CREATE INDEX IF NOT EXISTS idx_settings_profiles_v2_scope
             ON settings_profiles_v2(scope_kind, scope_id);
         CREATE INDEX IF NOT EXISTS idx_settings_profiles_v2_legacy_source
             ON settings_profiles_v2(managed_by, legacy_source_id)
             WHERE legacy_source_id IS NOT NULL;

         CREATE TABLE IF NOT EXISTS settings_schema_state (
             singleton_id INTEGER PRIMARY KEY NOT NULL DEFAULT 1
                 CHECK (singleton_id = 1),
             active_version INTEGER NOT NULL CHECK (active_version IN (1, 2)),
             migration_id TEXT,
             activated_at TEXT,
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         INSERT OR IGNORE INTO settings_schema_state (
             singleton_id, active_version
         ) VALUES (1, 1);

         CREATE TABLE IF NOT EXISTS settings_schema_migration_journal (
             id TEXT PRIMARY KEY NOT NULL,
             migration_run_id TEXT NOT NULL,
             migration_key TEXT NOT NULL,
             source_kind TEXT NOT NULL,
             source_id TEXT NOT NULL,
             source_fingerprint TEXT NOT NULL,
             target_profile_id TEXT NOT NULL,
             status TEXT NOT NULL CHECK (
                 status IN ('applied', 'rolled_back', 'superseded')
             ),
             source_snapshot_ciphertext TEXT NOT NULL,
             source_hash TEXT NOT NULL,
             target_hash TEXT NOT NULL,
             round_trip_verified INTEGER NOT NULL DEFAULT 0
                 CHECK (round_trip_verified IN (0, 1)),
             applied_at TEXT NOT NULL DEFAULT (datetime('now')),
             rolled_back_at TEXT
         );
         CREATE INDEX IF NOT EXISTS idx_settings_schema_migration_source
             ON settings_schema_migration_journal(
                 migration_key, source_kind, source_id, applied_at
             );",
    ),
    (
        "v096_connection_model_capability_registry",
        "CREATE TABLE IF NOT EXISTS provider_connections (
             id TEXT PRIMARY KEY NOT NULL,
             schema_version INTEGER NOT NULL CHECK (schema_version = 1),
             revision INTEGER NOT NULL CHECK (revision > 0),
             provider_id TEXT NOT NULL,
             adapter_provider_id TEXT NOT NULL,
             endpoint_id TEXT NOT NULL,
             base_url TEXT NOT NULL,
             endpoint_fingerprint TEXT NOT NULL,
             credential_ref TEXT,
             enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
             health_status TEXT NOT NULL DEFAULT 'unknown' CHECK (
                 health_status IN ('unknown', 'configured', 'missing', 'invalid', 'expired')
             ),
             source_kind TEXT NOT NULL,
             source_id TEXT NOT NULL,
             source_revision INTEGER NOT NULL,
             source_fingerprint TEXT NOT NULL,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(endpoint_fingerprint, credential_ref)
         );
         CREATE INDEX IF NOT EXISTS idx_provider_connections_provider
             ON provider_connections(provider_id, enabled);

         CREATE TABLE IF NOT EXISTS model_definitions (
             id TEXT PRIMARY KEY NOT NULL,
             schema_version INTEGER NOT NULL CHECK (schema_version = 2),
             provider_id TEXT NOT NULL,
             canonical_model_id TEXT NOT NULL,
             descriptor_json TEXT NOT NULL CHECK (json_valid(descriptor_json)),
             descriptor_hash TEXT NOT NULL,
             source TEXT NOT NULL,
             revision INTEGER NOT NULL CHECK (revision > 0),
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(provider_id, canonical_model_id)
         );

         CREATE TABLE IF NOT EXISTS model_targets (
             id TEXT PRIMARY KEY NOT NULL,
             connection_id TEXT NOT NULL REFERENCES provider_connections(id),
             model_definition_id TEXT REFERENCES model_definitions(id),
             upstream_model_id TEXT NOT NULL,
             availability TEXT NOT NULL DEFAULT 'unknown' CHECK (
                 availability IN ('unknown', 'unavailable', 'discoverable', 'callable', 'product_ready')
             ),
             revision INTEGER NOT NULL CHECK (revision > 0),
             source_kind TEXT NOT NULL,
             source_id TEXT NOT NULL,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(connection_id, upstream_model_id)
         );
         CREATE INDEX IF NOT EXISTS idx_model_targets_connection
             ON model_targets(connection_id, availability);

         CREATE TABLE IF NOT EXISTS capability_bindings (
             id TEXT PRIMARY KEY NOT NULL,
             capability_id TEXT NOT NULL,
             scope_kind TEXT NOT NULL CHECK (
                 scope_kind IN ('application', 'workspace', 'agent', 'task')
             ),
             scope_id TEXT NOT NULL DEFAULT '',
             revision INTEGER NOT NULL CHECK (revision > 0),
             primary_target_id TEXT REFERENCES model_targets(id),
             fallback_target_ids_json TEXT NOT NULL DEFAULT '[]'
                 CHECK (json_valid(fallback_target_ids_json)),
             fallback_mode TEXT NOT NULL DEFAULT 'disabled' CHECK (
                 fallback_mode IN ('disabled', 'ask', 'automatic')
             ),
             route_json TEXT NOT NULL CHECK (json_valid(route_json)),
             route_hash TEXT NOT NULL,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(capability_id, scope_kind, scope_id)
         );
         CREATE INDEX IF NOT EXISTS idx_capability_bindings_scope
             ON capability_bindings(scope_kind, scope_id, capability_id);

         CREATE TABLE IF NOT EXISTS model_catalog_snapshots (
             id TEXT PRIMARY KEY NOT NULL,
             source_id TEXT NOT NULL,
             connection_id TEXT REFERENCES provider_connections(id),
             connection_revision INTEGER,
             schema_version INTEGER NOT NULL,
             content_hash TEXT NOT NULL,
             model_count INTEGER NOT NULL CHECK (model_count >= 0),
             validation_status TEXT NOT NULL CHECK (
                 validation_status IN ('valid', 'rejected')
             ),
             snapshot_json TEXT NOT NULL CHECK (json_valid(snapshot_json)),
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(source_id, content_hash, connection_id)
         );

         CREATE TABLE IF NOT EXISTS registry_activation_state (
             capability_id TEXT NOT NULL,
             scope_kind TEXT NOT NULL CHECK (
                 scope_kind IN ('application', 'workspace', 'agent', 'task')
             ),
             scope_id TEXT NOT NULL DEFAULT '',
             read_mode TEXT NOT NULL DEFAULT 'legacy' CHECK (
                 read_mode IN ('legacy', 'registry')
             ),
             registry_revision INTEGER NOT NULL DEFAULT 0,
             parity_status TEXT NOT NULL DEFAULT 'pending' CHECK (
                 parity_status IN ('pending', 'matched', 'mismatched', 'blocked')
             ),
             parity_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(parity_json)),
             activated_at TEXT,
             rolled_back_at TEXT,
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             PRIMARY KEY(capability_id, scope_kind, scope_id)
         );

         CREATE TABLE IF NOT EXISTS agent_task_registry_snapshots (
             run_id TEXT NOT NULL REFERENCES agent_task_runs(id) ON DELETE CASCADE,
             capability_id TEXT NOT NULL,
             schema_version INTEGER NOT NULL CHECK (schema_version = 1),
             snapshot_hash TEXT NOT NULL,
             snapshot_json TEXT NOT NULL CHECK (json_valid(snapshot_json)),
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             PRIMARY KEY(run_id, capability_id)
         );",
    ),
    (
        "v097_vision_observation_cache",
        "CREATE TABLE IF NOT EXISTS vision_observation_cache (
             attachment_hash TEXT NOT NULL CHECK (length(attachment_hash) = 64),
             profile_hash TEXT NOT NULL CHECK (length(profile_hash) = 64),
             schema_version INTEGER NOT NULL CHECK (schema_version = 1),
             observation_json TEXT NOT NULL CHECK (json_valid(observation_json)),
             created_at_epoch INTEGER NOT NULL,
             expires_at_epoch INTEGER NOT NULL CHECK (expires_at_epoch > created_at_epoch),
             last_accessed_at_epoch INTEGER NOT NULL,
             PRIMARY KEY(attachment_hash, profile_hash)
         );
         CREATE INDEX IF NOT EXISTS idx_vision_observation_cache_expiry
             ON vision_observation_cache(expires_at_epoch);",
    ),
    (
        "v098_media_job_runtime_and_asset_lineage",
        "CREATE TABLE IF NOT EXISTS media_jobs (
             id TEXT PRIMARY KEY,
             idempotency_key TEXT NOT NULL UNIQUE,
             request_fingerprint_sha256 TEXT NOT NULL CHECK (length(request_fingerprint_sha256) = 64),
             project_id TEXT,
             conversation_id TEXT,
             provider_id TEXT NOT NULL,
             provider_source TEXT NOT NULL,
             model_id TEXT NOT NULL,
             api_version TEXT,
             operation TEXT NOT NULL CHECK (operation IN (
                 'text_to_video', 'image_to_video', 'video_to_video', 'extend',
                 'edit', 'first_last_frame', 'motion_transfer', 'lip_sync',
                 'upscale', 'audio_generation'
             )),
             input_asset_ids_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(input_asset_ids_json)),
             state TEXT NOT NULL CHECK (state IN (
                 'draft', 'validating', 'uploading_assets', 'submitting', 'queued',
                 'running', 'post_processing', 'completed', 'failed', 'cancelled',
                 'expired', 'provider_unknown'
             )),
             revision INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0),
             raw_parameters_json TEXT NOT NULL CHECK (json_valid(raw_parameters_json)),
             normalized_parameters_json TEXT NOT NULL CHECK (json_valid(normalized_parameters_json)),
             provider_extras_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(provider_extras_json)),
             observation_mode TEXT NOT NULL CHECK (observation_mode IN ('polling', 'webhook', 'hybrid')),
             current_attempt_id TEXT,
             current_provider_task_id TEXT,
             retry_count INTEGER NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
             max_attempts INTEGER NOT NULL DEFAULT 3 CHECK (max_attempts BETWEEN 1 AND 10),
             estimated_cost_micros INTEGER CHECK (estimated_cost_micros >= 0),
             final_cost_micros INTEGER CHECK (final_cost_micros >= 0),
             currency TEXT,
             data_region TEXT,
             remote_retention_expires_at TEXT,
             cancellation_requested_at TEXT,
             cancellation_reason TEXT,
             allow_cross_provider_fallback INTEGER NOT NULL DEFAULT 0 CHECK (allow_cross_provider_fallback IN (0, 1)),
             watermark_present INTEGER CHECK (watermark_present IN (0, 1)),
             provenance_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(provenance_json)),
             last_provider_observed_at TEXT,
             created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             completed_at TEXT,
             expires_at TEXT
         );
         CREATE INDEX IF NOT EXISTS idx_media_jobs_recovery
             ON media_jobs(state, updated_at);
         CREATE INDEX IF NOT EXISTS idx_media_jobs_project_created
             ON media_jobs(project_id, created_at DESC);

         CREATE TABLE IF NOT EXISTS media_job_attempts (
             id TEXT PRIMARY KEY,
             job_id TEXT NOT NULL REFERENCES media_jobs(id) ON DELETE CASCADE,
             attempt_number INTEGER NOT NULL CHECK (attempt_number >= 1),
             idempotency_key TEXT NOT NULL,
             provider_id TEXT NOT NULL,
             provider_source TEXT NOT NULL,
             model_id TEXT NOT NULL,
             api_version TEXT,
             data_region TEXT,
             remote_retention_expires_at TEXT,
             cross_provider_fallback_authorized INTEGER NOT NULL DEFAULT 0 CHECK (cross_provider_fallback_authorized IN (0, 1)),
             provider_task_id TEXT,
             state TEXT NOT NULL CHECK (state IN (
                 'created', 'submitting', 'accepted', 'observing', 'succeeded',
                 'failed', 'cancelled', 'expired', 'provider_unknown'
             )),
             error_json TEXT CHECK (error_json IS NULL OR json_valid(error_json)),
             retry_classification TEXT,
             next_eligible_at TEXT,
             cancellation_requested_at TEXT,
             cancellation_result_json TEXT CHECK (cancellation_result_json IS NULL OR json_valid(cancellation_result_json)),
             remote_deletion_requested_at TEXT,
             remote_deletion_status TEXT NOT NULL DEFAULT 'not_requested' CHECK (remote_deletion_status IN (
                 'not_requested', 'requested', 'confirmed', 'unsupported', 'failed'
             )),
             remote_deletion_completed_at TEXT,
             remote_deletion_error_json TEXT CHECK (remote_deletion_error_json IS NULL OR json_valid(remote_deletion_error_json)),
             submitted_at TEXT,
             last_observed_at TEXT,
             completed_at TEXT,
             created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             UNIQUE(job_id, attempt_number),
             UNIQUE(job_id, idempotency_key)
         );
         CREATE UNIQUE INDEX IF NOT EXISTS idx_media_job_attempt_provider_task
             ON media_job_attempts(provider_source, provider_task_id)
             WHERE provider_task_id IS NOT NULL;
         CREATE INDEX IF NOT EXISTS idx_media_job_attempt_remote_deletion
             ON media_job_attempts(remote_deletion_status, updated_at)
             WHERE remote_deletion_status = 'requested';

         CREATE TABLE IF NOT EXISTS media_assets (
             id TEXT PRIMARY KEY CHECK (length(id) = 64),
             content_hash_sha256 TEXT NOT NULL UNIQUE CHECK (length(content_hash_sha256) = 64),
             content_verified_at TEXT NOT NULL,
             media_type TEXT NOT NULL,
             byte_length INTEGER NOT NULL CHECK (byte_length > 0),
             storage_kind TEXT NOT NULL CHECK (storage_kind IN ('managed_local', 'provider_remote', 'external')),
             storage_key TEXT NOT NULL,
             width INTEGER CHECK (width IS NULL OR width > 0),
             height INTEGER CHECK (height IS NULL OR height > 0),
             duration_ms INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
             local_state TEXT NOT NULL DEFAULT 'available' CHECK (local_state IN (
                 'available', 'deletion_requested', 'deleted'
             )),
             local_deletion_requested_at TEXT,
             local_deletion_completed_at TEXT,
             created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
         );

         CREATE TABLE IF NOT EXISTS media_asset_relations (
             id TEXT PRIMARY KEY,
             job_id TEXT NOT NULL REFERENCES media_jobs(id) ON DELETE CASCADE,
             attempt_id TEXT NOT NULL REFERENCES media_job_attempts(id) ON DELETE RESTRICT,
             asset_id TEXT NOT NULL REFERENCES media_assets(id) ON DELETE RESTRICT,
             parent_asset_id TEXT REFERENCES media_assets(id) ON DELETE RESTRICT,
             relation_type TEXT NOT NULL CHECK (relation_type IN (
                 'input', 'output', 'derived_from', 'variant_of', 'extends',
                 'edits', 'audio_track', 'export'
             )),
             ordinal INTEGER NOT NULL DEFAULT 0 CHECK (ordinal >= 0),
             local_retention_policy TEXT NOT NULL DEFAULT 'retain_until_deleted' CHECK (
                 local_retention_policy IN ('retain_until_deleted', 'delete_after_expiry')
             ),
             local_retention_expires_at TEXT,
             metadata_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata_json)),
             created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             CHECK (parent_asset_id IS NULL OR parent_asset_id <> asset_id),
             CHECK (
                 (local_retention_policy = 'retain_until_deleted' AND local_retention_expires_at IS NULL)
                 OR (local_retention_policy = 'delete_after_expiry' AND local_retention_expires_at IS NOT NULL)
             )
         );
         CREATE INDEX IF NOT EXISTS idx_media_asset_relations_job
             ON media_asset_relations(job_id, relation_type, ordinal);
         CREATE INDEX IF NOT EXISTS idx_media_asset_relations_parent
             ON media_asset_relations(parent_asset_id);

         CREATE TABLE IF NOT EXISTS media_provider_events (
             id TEXT PRIMARY KEY,
             job_id TEXT NOT NULL REFERENCES media_jobs(id) ON DELETE CASCADE,
             attempt_id TEXT NOT NULL REFERENCES media_job_attempts(id) ON DELETE CASCADE,
             sequence INTEGER NOT NULL CHECK (sequence >= 1),
             provider_id TEXT NOT NULL,
             event_source TEXT NOT NULL,
             deduplication_key TEXT NOT NULL,
             event_kind TEXT NOT NULL,
             payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
             provider_created_at TEXT,
             observed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             UNIQUE(event_source, deduplication_key),
             UNIQUE(job_id, sequence)
         );
         CREATE INDEX IF NOT EXISTS idx_media_provider_events_job
             ON media_provider_events(job_id, observed_at);

         CREATE TABLE IF NOT EXISTS media_exports (
             id TEXT PRIMARY KEY,
             job_id TEXT REFERENCES media_jobs(id) ON DELETE SET NULL,
             asset_id TEXT REFERENCES media_assets(id) ON DELETE RESTRICT,
             state TEXT NOT NULL CHECK (state IN ('draft', 'rendering', 'completed', 'failed', 'cancelled')),
             format TEXT NOT NULL,
             settings_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(settings_json)),
             output_storage_key TEXT,
             error_json TEXT CHECK (error_json IS NULL OR json_valid(error_json)),
             created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             completed_at TEXT
         );",
    ),
    (
        "v099_context_compaction_source_fence",
        "ALTER TABLE context_compactions ADD COLUMN source_message_ids_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(source_message_ids_json));",
    ),
    (
        "v100_context_compaction_source_start",
        "ALTER TABLE context_compactions ADD COLUMN source_start_sort_order INTEGER NOT NULL DEFAULT 0;",
    ),
    (
        "v101_context_compaction_source_boundary",
        "ALTER TABLE context_compactions ADD COLUMN source_boundary_sort_order INTEGER NOT NULL DEFAULT 0;",
    ),
    (
        "v102_context_compaction_source_digest",
        "ALTER TABLE context_compactions ADD COLUMN source_digest TEXT NOT NULL DEFAULT '';",
    ),
    (
        "v103_context_compaction_generation",
        "ALTER TABLE context_compactions ADD COLUMN checkpoint_generation INTEGER NOT NULL DEFAULT 1 CHECK (checkpoint_generation > 0);",
    ),
    (
        "v104_context_compaction_generation_index",
        "WITH ranked AS (
             SELECT id,
                    ROW_NUMBER() OVER (
                        PARTITION BY conversation_id
                        ORDER BY created_at ASC, id ASC
                    ) AS generation
             FROM context_compactions
         )
         UPDATE context_compactions
         SET checkpoint_generation = (
             SELECT generation FROM ranked WHERE ranked.id = context_compactions.id
         );
         CREATE UNIQUE INDEX IF NOT EXISTS idx_context_compactions_generation
             ON context_compactions(conversation_id, checkpoint_generation);",
    ),
    (
        "v105_project_workspace_runtime",
        "CREATE TABLE IF NOT EXISTS conversation_episodes (
             id TEXT PRIMARY KEY,
             project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
             conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
             turn_id TEXT NOT NULL,
             run_id TEXT NOT NULL,
             summary TEXT NOT NULL,
             evidence_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(evidence_json)),
             created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             UNIQUE(project_id, turn_id)
         );
         CREATE INDEX IF NOT EXISTS idx_conversation_episodes_project_recent
             ON conversation_episodes(project_id, created_at DESC);
         CREATE INDEX IF NOT EXISTS idx_conversation_episodes_conversation
             ON conversation_episodes(conversation_id, created_at DESC);

         CREATE TABLE IF NOT EXISTS project_events (
             id TEXT PRIMARY KEY,
             project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
             conversation_id TEXT REFERENCES conversations(id) ON DELETE SET NULL,
             turn_id TEXT,
             event_type TEXT NOT NULL,
             title TEXT NOT NULL,
             summary TEXT NOT NULL DEFAULT '',
             provenance_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(provenance_json)),
             confidence REAL NOT NULL DEFAULT 1.0 CHECK (confidence >= 0.0 AND confidence <= 1.0),
             review_state TEXT NOT NULL DEFAULT 'observed' CHECK (
                 review_state IN ('observed', 'needs_review', 'accepted', 'rejected')
             ),
             valid_from TEXT,
             valid_to TEXT,
             created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             UNIQUE(project_id, event_type, turn_id)
         );
         CREATE INDEX IF NOT EXISTS idx_project_events_project_recent
             ON project_events(project_id, created_at DESC);
         CREATE INDEX IF NOT EXISTS idx_project_events_review
             ON project_events(project_id, review_state, created_at DESC);",
    ),
    (
        "v106_event_claim_graph",
        "CREATE TABLE IF NOT EXISTS knowledge_events (
             id TEXT PRIMARY KEY,
             project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
             event_kind TEXT NOT NULL,
             title TEXT NOT NULL,
             description TEXT NOT NULL DEFAULT '',
             confidence REAL NOT NULL DEFAULT 0.75 CHECK (confidence >= 0.0 AND confidence <= 1.0),
             review_state TEXT NOT NULL DEFAULT 'needs_review' CHECK (
                 review_state IN ('needs_review', 'accepted', 'rejected')
             ),
             valid_from TEXT,
             valid_to TEXT,
             provenance_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(provenance_json)),
             created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
         );
         CREATE INDEX IF NOT EXISTS idx_knowledge_events_project_time
             ON knowledge_events(project_id, valid_from, created_at DESC);

         CREATE TABLE IF NOT EXISTS knowledge_claims (
             id TEXT PRIMARY KEY,
             project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
             subject TEXT NOT NULL,
             predicate TEXT NOT NULL,
             object TEXT NOT NULL,
             claim_status TEXT NOT NULL DEFAULT 'active' CHECK (
                 claim_status IN ('active', 'contested', 'superseded')
             ),
             review_state TEXT NOT NULL DEFAULT 'needs_review' CHECK (
                 review_state IN ('needs_review', 'accepted', 'rejected')
             ),
             confidence REAL NOT NULL DEFAULT 0.75 CHECK (confidence >= 0.0 AND confidence <= 1.0),
             valid_from TEXT,
             valid_to TEXT,
             provenance_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(provenance_json)),
             created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
         );
         CREATE INDEX IF NOT EXISTS idx_knowledge_claims_project_review
             ON knowledge_claims(project_id, review_state, claim_status, updated_at DESC);
         CREATE INDEX IF NOT EXISTS idx_knowledge_claims_subject
             ON knowledge_claims(project_id, subject, predicate);

         CREATE TABLE IF NOT EXISTS knowledge_edges (
             id TEXT PRIMARY KEY,
             project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
             source_kind TEXT NOT NULL CHECK (source_kind IN ('claim', 'event', 'entity')),
             source_id TEXT NOT NULL,
             target_kind TEXT NOT NULL CHECK (target_kind IN ('claim', 'event', 'entity')),
             target_id TEXT NOT NULL,
             relation_type TEXT NOT NULL CHECK (
                 relation_type IN ('supports', 'opposes', 'supersedes', 'causes', 'precedes', 'mentions')
             ),
             confidence REAL NOT NULL DEFAULT 0.75 CHECK (confidence >= 0.0 AND confidence <= 1.0),
             review_state TEXT NOT NULL DEFAULT 'needs_review' CHECK (
                 review_state IN ('needs_review', 'accepted', 'rejected')
             ),
             provenance_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(provenance_json)),
             created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
         );
         CREATE INDEX IF NOT EXISTS idx_knowledge_edges_source
             ON knowledge_edges(project_id, source_kind, source_id);
         CREATE INDEX IF NOT EXISTS idx_knowledge_edges_target
             ON knowledge_edges(project_id, target_kind, target_id);

         CREATE TABLE IF NOT EXISTS knowledge_evidence (
             id TEXT PRIMARY KEY,
             project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
             claim_id TEXT REFERENCES knowledge_claims(id) ON DELETE CASCADE,
             event_id TEXT REFERENCES knowledge_events(id) ON DELETE CASCADE,
             source_type TEXT NOT NULL,
             source_ref TEXT NOT NULL,
             excerpt TEXT NOT NULL DEFAULT '',
             locator_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(locator_json)),
             created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             CHECK ((claim_id IS NOT NULL) <> (event_id IS NOT NULL))
         );
         CREATE INDEX IF NOT EXISTS idx_knowledge_evidence_claim
             ON knowledge_evidence(project_id, claim_id);
         CREATE INDEX IF NOT EXISTS idx_knowledge_evidence_event
             ON knowledge_evidence(project_id, event_id);

         CREATE TABLE IF NOT EXISTS graph_versions (
             id TEXT PRIMARY KEY,
             project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
             version INTEGER NOT NULL CHECK (version > 0),
             change_summary TEXT NOT NULL,
             provenance_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(provenance_json)),
             created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             UNIQUE(project_id, version)
         );

         CREATE TABLE IF NOT EXISTS entity_merge_candidates (
             id TEXT PRIMARY KEY,
             project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
             left_entity_id TEXT NOT NULL,
             right_entity_id TEXT NOT NULL,
             score REAL NOT NULL CHECK (score >= 0.0 AND score <= 1.0),
             review_state TEXT NOT NULL DEFAULT 'needs_review' CHECK (
                 review_state IN ('needs_review', 'accepted', 'rejected')
             ),
             evidence_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(evidence_json)),
             created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             UNIQUE(project_id, left_entity_id, right_entity_id),
             CHECK (left_entity_id <> right_entity_id)
         );",
    ),
    (
        "v107_project_workspace_items",
        "CREATE TABLE IF NOT EXISTS project_workspace_items (
             id TEXT PRIMARY KEY,
             project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
             conversation_id TEXT REFERENCES conversations(id) ON DELETE SET NULL,
             turn_id TEXT,
             run_id TEXT,
             item_kind TEXT NOT NULL CHECK (
                 item_kind IN ('decision', 'constraint', 'task', 'artifact', 'open_question', 'source')
             ),
             item_status TEXT NOT NULL DEFAULT 'active' CHECK (
                 item_status IN ('active', 'open', 'completed', 'superseded')
             ),
             title TEXT NOT NULL,
             summary TEXT NOT NULL DEFAULT '',
             evidence_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(evidence_json)),
             provenance_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(provenance_json)),
             review_state TEXT NOT NULL DEFAULT 'observed' CHECK (
                 review_state IN ('observed', 'needs_review', 'accepted', 'rejected')
             ),
             created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             UNIQUE(project_id, item_kind, turn_id, title)
         );
         CREATE INDEX IF NOT EXISTS idx_project_workspace_items_project_kind
             ON project_workspace_items(project_id, item_kind, item_status, updated_at DESC);
         CREATE INDEX IF NOT EXISTS idx_project_workspace_items_conversation
             ON project_workspace_items(conversation_id, updated_at DESC);",
    ),
    (
        "v108_conversation_system_prompt_origin",
        "ALTER TABLE conversations ADD COLUMN system_prompt_origin TEXT NOT NULL DEFAULT 'legacy';
         UPDATE conversations
         SET system_prompt_origin = CASE
             WHEN trim(system_prompt) = '' THEN 'none'
             WHEN project_id IS NULL THEN 'user'
             WHEN trim(system_prompt) = trim(COALESCE((
                 SELECT projects.system_prompt FROM projects WHERE projects.id = conversations.project_id
             ), '')) THEN 'project_snapshot'
             ELSE 'legacy_ambiguous'
         END;",
    ),
    (
        "v109_turn_launch_project",
        "ALTER TABLE conversation_turns ADD COLUMN launch_project_id TEXT REFERENCES projects(id) ON DELETE SET NULL;",
    ),
    (
        "v110_turn_launch_project_backfill",
        "UPDATE conversation_turns
         SET launch_project_id = (
             SELECT conversations.project_id
             FROM conversations
             WHERE conversations.id = conversation_turns.conversation_id
         )
         WHERE launch_project_id IS NULL;
         CREATE INDEX IF NOT EXISTS idx_conversation_turns_launch_project
             ON conversation_turns(launch_project_id, created_at DESC);",
    ),
    (
        "v111_remove_legacy_reasoning_sentinel",
        "UPDATE messages
         SET thinking = NULL
         WHERE thinking = '[reasoning content unavailable in local history]';",
    ),
    (
        "v112_provider_turn_envelopes",
        "CREATE TABLE IF NOT EXISTS provider_turn_envelopes (
             turn_item_id TEXT PRIMARY KEY,
             sample_id TEXT NOT NULL UNIQUE,
             scope_id TEXT NOT NULL,
             conversation_id TEXT REFERENCES conversations(id) ON DELETE SET NULL,
             conversation_turn_id TEXT REFERENCES conversation_turns(id) ON DELETE SET NULL,
             run_id TEXT,
             subtask_run_id TEXT,
             message_id TEXT UNIQUE REFERENCES messages(id) ON DELETE CASCADE,
             provider_endpoint_id TEXT NOT NULL,
             provider_family TEXT NOT NULL,
             api_style TEXT NOT NULL,
             model_id TEXT NOT NULL,
             reasoning_profile_id TEXT NOT NULL,
             reasoning_profile_version INTEGER NOT NULL CHECK (reasoning_profile_version > 0),
             replay_policy TEXT NOT NULL,
             visible_content TEXT NOT NULL DEFAULT '',
             provider_items_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(provider_items_json)),
             replay_payload_json TEXT NOT NULL CHECK (json_valid(replay_payload_json)),
             tool_calls_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(tool_calls_json)),
             capture_status TEXT NOT NULL CHECK (capture_status IN (
                 'captured', 'notRequested', 'notRequired', 'omittedByProvider',
                 'missingFromLegacyHistory', 'interrupted', 'truncated', 'redacted'
             )),
             request_id TEXT,
             response_id TEXT,
             raw_response_digest TEXT NOT NULL,
             created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
         );
         CREATE INDEX IF NOT EXISTS idx_provider_turn_envelopes_conversation
             ON provider_turn_envelopes(conversation_id, created_at);
         CREATE INDEX IF NOT EXISTS idx_provider_turn_envelopes_subtask
             ON provider_turn_envelopes(subtask_run_id, created_at);
         CREATE TABLE IF NOT EXISTS provider_turn_legacy_boundaries (
             message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
             reason TEXT NOT NULL DEFAULT 'provider_turn_envelope_missing',
             created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
         );
         INSERT OR IGNORE INTO provider_turn_legacy_boundaries (message_id)
         SELECT id
         FROM messages
         WHERE role = 'assistant'
           AND tool_calls_json IS NOT NULL
           AND trim(tool_calls_json) NOT IN ('', '[]')
           AND (
               artifacts_json IS NULL
               OR json_extract(artifacts_json, '$.providerTurnEnvelope') IS NULL
           );
         UPDATE messages
         SET artifacts_json = CASE
             WHEN artifacts_json IS NULL THEN json_object(
                 'providerReplayBoundary', json_object(
                     'reason', 'provider_turn_envelope_missing',
                     'version', 1
                 )
             )
             WHEN json_type(artifacts_json) = 'object' THEN json_set(
                 artifacts_json,
                 '$.providerReplayBoundary', json_object(
                     'reason', 'provider_turn_envelope_missing',
                     'version', 1
                 )
             )
             ELSE json_object(
                 'kind', 'assistantArtifacts',
                 'version', 2,
                 'legacyArtifacts', json(artifacts_json),
                 'providerReplayBoundary', json_object(
                     'reason', 'provider_turn_envelope_missing',
                     'version', 1
                 )
             )
         END
         WHERE id IN (SELECT message_id FROM provider_turn_legacy_boundaries);",
    ),
    (
        "v113_normalize_agent_run_event_sequences",
        // Historical ledgers may contain intentional gaps where ephemeral
        // heartbeats were never persisted. Authoritative replay handles those
        // gaps without rewriting user history during application startup.
        "UPDATE _migrations SET name = name WHERE 0;",
    ),
    (
        "v114_bound_agent_run_event_storage",
        // The table primary key already indexes (run_id, event_seq). Keep
        // deletion ownership aligned with agent_task_runs without rewriting
        // existing event history during application startup.
        "DROP INDEX IF EXISTS idx_agent_run_events_run;
         CREATE TRIGGER IF NOT EXISTS delete_agent_run_events_after_task_run
         AFTER DELETE ON agent_task_runs
         BEGIN
             DELETE FROM agent_run_events WHERE run_id = OLD.id;
         END;",
    ),
    (
        "v115_task_checkpoint_launch_key",
        "ALTER TABLE task_resume_checkpoints ADD COLUMN launch_idempotency_key TEXT;",
    ),
    (
        "v116_task_checkpoint_response_message",
        "ALTER TABLE task_resume_checkpoints ADD COLUMN response_message_id TEXT REFERENCES messages(id);",
    ),
    (
        "v117_task_checkpoint_resume_indexes",
        "CREATE INDEX IF NOT EXISTS idx_task_resume_checkpoints_launch_key
             ON task_resume_checkpoints(launch_idempotency_key)
             WHERE launch_idempotency_key IS NOT NULL;
         CREATE UNIQUE INDEX IF NOT EXISTS idx_task_resume_checkpoints_response_message
             ON task_resume_checkpoints(response_message_id)
             WHERE response_message_id IS NOT NULL;",
    ),
    (
        "v118_retire_playwright_browser_connector",
        "DELETE FROM mcp_servers
         WHERE id = 'builtin-playwright-browser'
            OR builtin_id = 'playwright-browser';",
    ),
    (
        "v119_durable_workflow_schedules",
        "CREATE TABLE IF NOT EXISTS workflow_automation_schedule_configs (
             automation_id TEXT PRIMARY KEY NOT NULL
                 REFERENCES workflow_automations(id) ON DELETE CASCADE,
             config_json TEXT NOT NULL,
             revision INTEGER NOT NULL DEFAULT 1,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE IF NOT EXISTS workflow_automation_occurrences (
             id TEXT PRIMARY KEY NOT NULL,
             automation_id TEXT NOT NULL
                 REFERENCES workflow_automations(id) ON DELETE CASCADE,
             definition_revision INTEGER NOT NULL,
             scheduled_for TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'planned',
             attempt_count INTEGER NOT NULL DEFAULT 0,
             retry_at TEXT,
             last_error TEXT,
             lease_token TEXT,
             lease_expires_at TEXT,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(automation_id, definition_revision, scheduled_for)
         );
         CREATE INDEX IF NOT EXISTS idx_workflow_occurrences_due
             ON workflow_automation_occurrences(status, retry_at, lease_expires_at);
         ALTER TABLE workflow_automation_runs ADD COLUMN occurrence_id TEXT
             REFERENCES workflow_automation_occurrences(id) ON DELETE SET NULL;
         ALTER TABLE workflow_automation_runs ADD COLUMN scheduled_for TEXT;
         ALTER TABLE workflow_automation_runs ADD COLUMN definition_revision INTEGER NOT NULL DEFAULT 1;
         ALTER TABLE workflow_automation_runs ADD COLUMN attempt INTEGER NOT NULL DEFAULT 1;
         CREATE UNIQUE INDEX IF NOT EXISTS idx_workflow_automation_run_attempt
             ON workflow_automation_runs(occurrence_id, attempt)
             WHERE occurrence_id IS NOT NULL;",
    ),
    (
        "v120_classify_legacy_workflow_schedules",
        "WITH legacy AS (
             SELECT id, trim(json_extract(trigger_json, '$.cron')) AS cron
             FROM workflow_automations
             WHERE trigger_kind = 'schedule'
         ), safe_daily AS (
             SELECT id
             FROM legacy
             WHERE (
                 cron GLOB '[0-9] [0-9] [*] [*] [*]'
                 OR cron GLOB '[0-9][0-9] [0-9] [*] [*] [*]'
                 OR cron GLOB '[0-9] [0-9][0-9] [*] [*] [*]'
                 OR cron GLOB '[0-9][0-9] [0-9][0-9] [*] [*] [*]'
             )
             AND CAST(substr(cron, 1, instr(cron, ' ') - 1) AS INTEGER)
                 BETWEEN 0 AND 59
             AND CAST(
                 substr(
                     substr(cron, instr(cron, ' ') + 1),
                     1,
                     instr(substr(cron, instr(cron, ' ') + 1), ' ') - 1
                 ) AS INTEGER
             ) BETWEEN 0 AND 23
         )
         INSERT OR IGNORE INTO workflow_automation_schedule_configs
             (automation_id, config_json, revision)
         SELECT id,
                '{\"version\":2,\"timezone\":\"UTC\",\"misfirePolicy\":\"run_latest\",\"misfireGraceSeconds\":300,\"overlapPolicy\":\"skip\",\"executionPolicy\":{\"powerMode\":\"standard\",\"orchestrationProfile\":\"balanced\",\"collaborationMode\":\"direct\"},\"legacyNeedsReview\":false}',
                1
         FROM safe_daily;
         INSERT OR IGNORE INTO workflow_automation_schedule_configs
             (automation_id, config_json, revision)
         SELECT id,
                '{\"version\":1,\"timezone\":\"UTC\",\"misfirePolicy\":\"run_latest\",\"misfireGraceSeconds\":300,\"overlapPolicy\":\"skip\",\"executionPolicy\":{\"powerMode\":\"standard\",\"orchestrationProfile\":\"balanced\",\"collaborationMode\":\"direct\"},\"legacyNeedsReview\":true}',
                1
         FROM workflow_automations
         WHERE trigger_kind = 'schedule';
         UPDATE workflow_automations
         SET enabled = 0,
             status = 'needs_review',
             next_run_at = NULL,
             updated_at = datetime('now')
         WHERE trigger_kind = 'schedule'
           AND id IN (
               SELECT automation_id
               FROM workflow_automation_schedule_configs
               WHERE json_extract(config_json, '$.legacyNeedsReview') = 1
            );",
    ),
    (
        "v121_workflow_definition_revisions",
        "CREATE TABLE IF NOT EXISTS workflow_automation_definition_revisions (
             automation_id TEXT NOT NULL
                 REFERENCES workflow_automations(id) ON DELETE CASCADE,
             revision INTEGER NOT NULL,
             name TEXT NOT NULL,
             description TEXT NOT NULL DEFAULT '',
             workflow_template_id TEXT NOT NULL,
             prompt TEXT NOT NULL,
             trigger_json TEXT NOT NULL,
             trigger_kind TEXT NOT NULL,
             source_scope_json TEXT NOT NULL DEFAULT '[]',
             approval_policy_json TEXT NOT NULL DEFAULT '{}',
             schedule_config_json TEXT NOT NULL,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             PRIMARY KEY (automation_id, revision)
         );
         CREATE TABLE IF NOT EXISTS workflow_automation_occurrence_approvals (
             occurrence_id TEXT PRIMARY KEY NOT NULL
                 REFERENCES workflow_automation_occurrences(id) ON DELETE CASCADE,
             state TEXT NOT NULL DEFAULT 'pending'
                 CHECK (state IN ('not_required', 'pending', 'approved', 'denied')),
             requested_at TEXT,
             resolved_at TEXT,
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE IF NOT EXISTS workflow_automation_occurrence_origins (
             occurrence_id TEXT PRIMARY KEY NOT NULL
                 REFERENCES workflow_automation_occurrences(id) ON DELETE CASCADE,
             origin TEXT NOT NULL DEFAULT 'schedule'
                 CHECK (origin IN ('schedule', 'manual_run_now')),
             resume_next_run_at TEXT,
             created_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         INSERT OR IGNORE INTO workflow_automation_definition_revisions
             (automation_id, revision, name, description, workflow_template_id,
              prompt, trigger_json, trigger_kind, source_scope_json,
              approval_policy_json, schedule_config_json)
         SELECT a.id, c.revision, a.name, a.description, a.workflow_template_id,
                a.prompt, a.trigger_json, a.trigger_kind, a.source_scope_json,
                a.approval_policy_json, c.config_json
         FROM workflow_automations a
         JOIN workflow_automation_schedule_configs c ON c.automation_id = a.id;
         INSERT OR IGNORE INTO workflow_automation_occurrence_approvals
             (occurrence_id, state)
         SELECT o.id,
                CASE
                    WHEN COALESCE(json_extract(a.approval_policy_json, '$.requireBeforeRun'), 1) = 1
                    THEN 'pending'
                    ELSE 'not_required'
                END
         FROM workflow_automation_occurrences o
         JOIN workflow_automations a ON a.id = o.automation_id;
         INSERT OR IGNORE INTO workflow_automation_occurrence_origins
             (occurrence_id, origin, resume_next_run_at)
         SELECT id, 'schedule', NULL
         FROM workflow_automation_occurrences;
         UPDATE workflow_automation_runs
         SET status = CASE
             WHEN status IN ('ready', 'draft') THEN 'draft'
             WHEN status IN ('pending', 'queued') THEN 'queued'
             WHEN status IN ('initializing', 'in_progress', 'running') THEN 'running'
             WHEN status IN ('cached', 'done', 'completed') THEN 'completed'
             WHEN status IN ('error', 'failed') THEN 'failed'
             WHEN status IN ('canceled', 'cancelled') THEN 'cancelled'
             WHEN status IN ('timeout', 'timed_out') THEN 'timed_out'
             ELSE status
         END;
         UPDATE workflow_automation_occurrences
         SET status = CASE
             WHEN status IN ('pending', 'queued') THEN 'queued'
             WHEN status IN ('initializing', 'in_progress', 'running') THEN 'running'
             WHEN status IN ('cached', 'done', 'completed') THEN 'completed'
             WHEN status IN ('error', 'failed') THEN 'failed'
             WHEN status IN ('canceled', 'cancelled') THEN 'cancelled'
             WHEN status IN ('timeout', 'timed_out') THEN 'timed_out'
             ELSE status
         END;
         UPDATE workflow_automation_occurrences
         SET status = 'cancelled',
             last_error = 'missing_definition_snapshot_migration',
             retry_at = NULL,
             lease_token = NULL,
             lease_expires_at = NULL,
             updated_at = datetime('now')
         WHERE status IN ('planned', 'claimed', 'retry_wait', 'waiting_approval', 'queued')
           AND NOT EXISTS (
               SELECT 1 FROM workflow_automation_definition_revisions d
               WHERE d.automation_id = workflow_automation_occurrences.automation_id
                 AND d.revision = workflow_automation_occurrences.definition_revision
           );
         UPDATE workflow_automation_runs
         SET status = 'cancelled',
             summary = COALESCE(summary,
                                'Definition snapshot unavailable after schedule migration'),
             finished_at = COALESCE(finished_at, datetime('now'))
         WHERE status IN ('queued', 'waiting_approval')
           AND NOT EXISTS (
               SELECT 1 FROM workflow_automation_definition_revisions d
               WHERE d.automation_id = workflow_automation_runs.automation_id
                 AND d.revision = workflow_automation_runs.definition_revision
           );
         CREATE TRIGGER IF NOT EXISTS validate_workflow_run_status_insert
         BEFORE INSERT ON workflow_automation_runs
         WHEN NEW.status NOT IN ('draft', 'queued', 'running', 'waiting_approval',
                                 'paused', 'resuming', 'completed', 'failed',
                                 'cancelled', 'timed_out', 'disabled', 'cancelling')
         BEGIN
             SELECT RAISE(ABORT, 'invalid workflow run status');
         END;
         CREATE TRIGGER IF NOT EXISTS validate_workflow_run_status_update
         BEFORE UPDATE OF status ON workflow_automation_runs
         WHEN NEW.status NOT IN ('draft', 'queued', 'running', 'waiting_approval',
                                 'paused', 'resuming', 'completed', 'failed',
                                 'cancelled', 'timed_out', 'disabled', 'cancelling')
         BEGIN
             SELECT RAISE(ABORT, 'invalid workflow run status');
         END;
         CREATE TRIGGER IF NOT EXISTS validate_workflow_occurrence_status_insert
         BEFORE INSERT ON workflow_automation_occurrences
         WHEN NEW.status NOT IN ('planned', 'claimed', 'retry_wait', 'waiting_approval',
                                 'queued', 'running', 'paused', 'resuming', 'completed',
                                 'skipped', 'failed', 'cancelled', 'timed_out', 'disabled',
                                 'cancelling')
         BEGIN
             SELECT RAISE(ABORT, 'invalid workflow occurrence status');
         END;
         CREATE TRIGGER IF NOT EXISTS validate_workflow_occurrence_status_update
         BEFORE UPDATE OF status ON workflow_automation_occurrences
         WHEN NEW.status NOT IN ('planned', 'claimed', 'retry_wait', 'waiting_approval',
                                 'queued', 'running', 'paused', 'resuming', 'completed',
                                 'skipped', 'failed', 'cancelled', 'timed_out', 'disabled',
                                 'cancelling')
         BEGIN
             SELECT RAISE(ABORT, 'invalid workflow occurrence status');
         END;",
    ),
    (
        "v122_workspace_isolation_ownership",
        "CREATE TABLE IF NOT EXISTS workspace_isolation_ownership (
             id TEXT PRIMARY KEY NOT NULL,
             owner_turn_id TEXT REFERENCES conversation_turns(id) ON DELETE SET NULL,
             original_repo_root TEXT NOT NULL,
             worktree_root TEXT NOT NULL UNIQUE,
             isolated_source_root TEXT NOT NULL,
             source_id TEXT,
             state TEXT NOT NULL DEFAULT 'preparing'
                 CHECK (state IN ('preparing', 'active')),
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE INDEX IF NOT EXISTS idx_workspace_isolation_owner
             ON workspace_isolation_ownership(owner_turn_id, state);",
    ),
    (
        "v123_scrub_legacy_gemini_query_credentials",
        "UPDATE agent_task_runs
         SET error_message = 'Gemini request failed; legacy credential-bearing URL detail was redacted.'
         WHERE lower(COALESCE(error_message, '')) LIKE '%?key=%'
            OR lower(COALESCE(error_message, '')) LIKE '%&key=%';
         UPDATE agent_traces
         SET error_message = CASE
                 WHEN error_message IS NULL THEN NULL
                 ELSE 'Provider error detail redacted because it contained a credential-bearing URL.'
             END,
             trace_json = '{\"redacted\":true,\"reason\":\"credential_bearing_url\"}'
         WHERE lower(COALESCE(error_message, '')) LIKE '%?key=%'
            OR lower(trace_json) LIKE '%?key=%'
            OR lower(COALESCE(error_message, '')) LIKE '%&key=%'
            OR lower(trace_json) LIKE '%&key=%';
         UPDATE conversation_turns
         SET trace_json = NULL
         WHERE lower(COALESCE(trace_json, '')) LIKE '%?key=%'
            OR lower(COALESCE(trace_json, '')) LIKE '%&key=%';
         UPDATE agent_task_run_events
         SET label = 'Provider error detail redacted',
             payload_json = '{\"redacted\":true,\"reason\":\"credential_bearing_url\"}'
         WHERE lower(label) LIKE '%?key=%'
            OR lower(COALESCE(payload_json, '')) LIKE '%?key=%'
            OR lower(label) LIKE '%&key=%'
            OR lower(COALESCE(payload_json, '')) LIKE '%&key=%';
         UPDATE agent_run_events
         SET label = 'Provider error detail redacted',
             payload_json = '{\"redacted\":true,\"reason\":\"credential_bearing_url\"}'
         WHERE lower(label) LIKE '%?key=%'
            OR lower(payload_json) LIKE '%?key=%'
            OR lower(label) LIKE '%&key=%'
            OR lower(payload_json) LIKE '%&key=%';
         UPDATE workflow_automation_runs
         SET summary = 'Provider error detail redacted because it contained a credential-bearing URL.'
         WHERE lower(COALESCE(summary, '')) LIKE '%?key=%'
            OR lower(COALESCE(summary, '')) LIKE '%&key=%';
         DELETE FROM agent_trajectories
         WHERE lower(trajectory_json) LIKE '%?key=%'
            OR lower(trajectory_json) LIKE '%&key=%';
         UPDATE ai_usage_records
         SET provider_raw_json = '{\"redacted\":true,\"reason\":\"credential_bearing_diagnostic\"}'
         WHERE lower(provider_raw_json) LIKE '%?key=%'
            OR lower(provider_raw_json) LIKE '%&key=%'
            OR lower(provider_raw_json) LIKE '%api_key=%'
            OR lower(provider_raw_json) LIKE '%apikey=%'
            OR lower(provider_raw_json) LIKE '%access_token=%'
            OR lower(provider_raw_json) LIKE '%x-amz-signature=%'
            OR provider_raw_json GLOB '*AIza????????????????*';",
    ),
    (
        "v124_provider_streaming_config",
        "ALTER TABLE agent_configs ADD COLUMN provider_streaming_json TEXT;",
    ),
    (
        "v125_compact_legacy_tool_preview_events",
        "WITH ranked_previews AS (
             SELECT rowid AS event_rowid,
                    ROW_NUMBER() OVER (
                        PARTITION BY run_id,
                            CASE
                              WHEN json_valid(payload_json)
                              THEN COALESCE(json_extract(payload_json, '$.run.callId'), label)
                              ELSE label
                            END
                        ORDER BY event_seq DESC
                    ) AS preview_rank
             FROM agent_run_events
             WHERE kind = 'toolProgress' AND status = 'preparing'
         )
         DELETE FROM agent_run_events
         WHERE rowid IN (
             SELECT event_rowid FROM ranked_previews WHERE preview_rank > 1
         );
         DELETE FROM agent_run_events
         WHERE kind = 'toolProgress'
           AND status = 'preparing'
           AND run_id IN (
               SELECT id FROM agent_task_runs
               WHERE status IN ('completed', 'cancelled', 'failed', 'timed_out')
         );",
    ),
    (
        "v126_skill_canonical_names",
        "ALTER TABLE skills ADD COLUMN canonical_name TEXT;
         CREATE UNIQUE INDEX IF NOT EXISTS idx_skills_canonical_name
             ON skills(canonical_name COLLATE NOCASE)
             WHERE canonical_name IS NOT NULL AND canonical_name != '';",
    ),
    ("v127_remove_unsafe_answer_cache", "DROP TABLE IF EXISTS answer_cache;"),
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

const DURABLE_WORKFLOW_SCHEDULES_MIGRATION: &str = "v119_durable_workflow_schedules";
const WORKFLOW_DEFINITION_REVISIONS_MIGRATION: &str = "v121_workflow_definition_revisions";

fn workflow_run_columns(conn: &Connection) -> Result<Vec<String>, CoreError> {
    let mut stmt = conn.prepare("PRAGMA table_info(workflow_automation_runs)")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let mut columns = Vec::new();
    for row in rows {
        columns.push(row?);
    }
    Ok(columns)
}

/// Applies and verifies v119 one restart-safe schema component at a time.
///
/// An older `execute_batch` could stop on the first duplicate column and then
/// mark the whole migration applied, leaving later columns or indexes absent.
/// This repair runs even when the migration marker already exists.
fn ensure_durable_workflow_schedule_schema(conn: &Connection) -> Result<(), CoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workflow_automation_schedule_configs (
             automation_id TEXT PRIMARY KEY NOT NULL
                 REFERENCES workflow_automations(id) ON DELETE CASCADE,
             config_json TEXT NOT NULL,
             revision INTEGER NOT NULL DEFAULT 1,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE IF NOT EXISTS workflow_automation_occurrences (
             id TEXT PRIMARY KEY NOT NULL,
             automation_id TEXT NOT NULL
                 REFERENCES workflow_automations(id) ON DELETE CASCADE,
             definition_revision INTEGER NOT NULL,
             scheduled_for TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'planned',
             attempt_count INTEGER NOT NULL DEFAULT 0,
             retry_at TEXT,
             last_error TEXT,
             lease_token TEXT,
             lease_expires_at TEXT,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             UNIQUE(automation_id, definition_revision, scheduled_for)
         );
         CREATE INDEX IF NOT EXISTS idx_workflow_occurrences_due
             ON workflow_automation_occurrences(status, retry_at, lease_expires_at);",
    )?;
    let mut columns = workflow_run_columns(conn)?;
    for (column, sql) in [
        (
            "occurrence_id",
            "ALTER TABLE workflow_automation_runs ADD COLUMN occurrence_id TEXT
                 REFERENCES workflow_automation_occurrences(id) ON DELETE SET NULL",
        ),
        (
            "scheduled_for",
            "ALTER TABLE workflow_automation_runs ADD COLUMN scheduled_for TEXT",
        ),
        (
            "definition_revision",
            "ALTER TABLE workflow_automation_runs ADD COLUMN definition_revision INTEGER NOT NULL DEFAULT 1",
        ),
        (
            "attempt",
            "ALTER TABLE workflow_automation_runs ADD COLUMN attempt INTEGER NOT NULL DEFAULT 1",
        ),
    ] {
        if !columns.iter().any(|existing| existing == column) {
            conn.execute_batch(sql)?;
            columns.push(column.to_string());
        }
    }
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_workflow_automation_run_attempt
             ON workflow_automation_runs(occurrence_id, attempt)
             WHERE occurrence_id IS NOT NULL;",
    )?;

    let required_columns = [
        "occurrence_id",
        "scheduled_for",
        "definition_revision",
        "attempt",
    ];
    let columns = workflow_run_columns(conn)?;
    if required_columns
        .iter()
        .any(|required| !columns.iter().any(|column| column == required))
    {
        return Err(CoreError::Internal(
            "Durable workflow schedule migration failed its column postcondition".into(),
        ));
    }
    let attempt_index_exists: bool = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM sqlite_master
             WHERE type = 'index' AND name = 'idx_workflow_automation_run_attempt'
         )",
        [],
        |row| row.get(0),
    )?;
    if !attempt_index_exists {
        return Err(CoreError::Internal(
            "Durable workflow schedule migration failed its index postcondition".into(),
        ));
    }
    Ok(())
}

fn ensure_workflow_definition_revision_schema(
    conn: &Connection,
    migration_sql: &str,
) -> Result<(), CoreError> {
    // v121 is intentionally composed only of CREATE IF NOT EXISTS and INSERT
    // OR IGNORE statements, so replay is safe even if a prior startup stopped
    // between schema creation, backfill, and migration-marker persistence.
    conn.execute_batch(migration_sql)?;
    let missing_snapshots: i64 = conn.query_row(
        "SELECT COUNT(*)
         FROM workflow_automation_schedule_configs c
         LEFT JOIN workflow_automation_definition_revisions d
           ON d.automation_id = c.automation_id AND d.revision = c.revision
         WHERE d.automation_id IS NULL",
        [],
        |row| row.get(0),
    )?;
    let missing_approvals: i64 = conn.query_row(
        "SELECT
             (SELECT COUNT(*)
              FROM workflow_automation_occurrences o
              LEFT JOIN workflow_automation_occurrence_approvals p ON p.occurrence_id = o.id
              WHERE p.occurrence_id IS NULL)
           + (SELECT COUNT(*)
              FROM workflow_automation_occurrences o
              LEFT JOIN workflow_automation_occurrence_origins g ON g.occurrence_id = o.id
              WHERE g.occurrence_id IS NULL)",
        [],
        |row| row.get(0),
    )?;
    let invalid_statuses: i64 = conn.query_row(
        "SELECT
             (SELECT COUNT(*) FROM workflow_automation_runs
              WHERE status NOT IN ('draft', 'queued', 'running', 'waiting_approval',
                                   'paused', 'resuming', 'completed', 'failed',
                                   'cancelled', 'timed_out', 'disabled', 'cancelling'))
           + (SELECT COUNT(*) FROM workflow_automation_occurrences
              WHERE status NOT IN ('planned', 'claimed', 'retry_wait', 'waiting_approval',
                                   'queued', 'running', 'paused', 'resuming', 'completed',
                                   'skipped', 'failed', 'cancelled', 'timed_out', 'disabled',
                                   'cancelling'))",
        [],
        |row| row.get(0),
    )?;
    let active_without_snapshot: i64 = conn.query_row(
        "SELECT
             (SELECT COUNT(*)
              FROM workflow_automation_occurrences o
              LEFT JOIN workflow_automation_definition_revisions d
                ON d.automation_id = o.automation_id AND d.revision = o.definition_revision
              WHERE o.status IN ('planned', 'claimed', 'retry_wait', 'waiting_approval', 'queued')
                AND d.automation_id IS NULL)
           + (SELECT COUNT(*)
              FROM workflow_automation_runs r
              LEFT JOIN workflow_automation_definition_revisions d
                ON d.automation_id = r.automation_id AND d.revision = r.definition_revision
              WHERE r.status IN ('queued', 'waiting_approval')
                AND d.automation_id IS NULL)",
        [],
        |row| row.get(0),
    )?;
    if missing_snapshots != 0
        || missing_approvals != 0
        || invalid_statuses != 0
        || active_without_snapshot != 0
    {
        return Err(CoreError::Internal(
            "Workflow definition revision migration failed its lineage postcondition".into(),
        ));
    }
    Ok(())
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

        if *name == DURABLE_WORKFLOW_SCHEDULES_MIGRATION {
            ensure_durable_workflow_schedule_schema(conn)?;
            if !already_applied {
                conn.execute("INSERT INTO _migrations (name) VALUES (?1)", [name])?;
            }
            continue;
        }

        if *name == WORKFLOW_DEFINITION_REVISIONS_MIGRATION {
            ensure_workflow_definition_revision_schema(conn, sql)?;
            if !already_applied {
                conn.execute("INSERT INTO _migrations (name) VALUES (?1)", [name])?;
            }
            continue;
        }

        if already_applied {
            continue;
        }

        tracing::info!("Applying migration '{name}'…");
        if let Err(err) = conn.execute_batch(sql) {
            if !is_idempotent_schema_error(&err) {
                return Err(err.into());
            }
        }
        conn.execute("INSERT INTO _migrations (name) VALUES (?1)", [name])?;
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
    fn retiring_answer_cache_preserves_conversation_history() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        conn.execute_batch(
            "CREATE TABLE answer_cache (answer_text TEXT);
             INSERT INTO answer_cache VALUES ('stale answer');
             INSERT INTO conversations (id, title) VALUES ('kept', 'User history');
             DELETE FROM _migrations WHERE name = 'v127_remove_unsafe_answer_cache';",
        )
        .unwrap();
        run_migrations(&conn).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'answer_cache'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT title FROM conversations WHERE id = 'kept'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "User history"
        );
        run_migrations(&conn).unwrap();
    }

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
        assert!(tables.contains(&"settings_profiles_v2".to_string()));
        assert!(tables.contains(&"settings_schema_state".to_string()));
        assert!(tables.contains(&"settings_schema_migration_journal".to_string()));
        assert!(tables.contains(&"provider_connections".to_string()));
        assert!(tables.contains(&"model_definitions".to_string()));
        assert!(tables.contains(&"model_targets".to_string()));
        assert!(tables.contains(&"capability_bindings".to_string()));
        assert!(tables.contains(&"model_catalog_snapshots".to_string()));
        assert!(tables.contains(&"registry_activation_state".to_string()));
        assert!(tables.contains(&"agent_task_registry_snapshots".to_string()));
        assert!(tables.contains(&"vision_observation_cache".to_string()));
        assert!(tables.contains(&"media_jobs".to_string()));
        assert!(tables.contains(&"media_job_attempts".to_string()));
        assert!(tables.contains(&"media_assets".to_string()));
        assert!(tables.contains(&"media_asset_relations".to_string()));
        assert!(tables.contains(&"media_provider_events".to_string()));
        assert!(tables.contains(&"media_exports".to_string()));
        assert!(tables.contains(&"query_logs".to_string()));
        assert!(tables.contains(&"embeddings".to_string()));
        assert!(tables.contains(&"feedback".to_string()));
        assert!(tables.contains(&"_migrations".to_string()));
        assert!(tables.contains(&"agent_procedural_memories".to_string()));
        assert!(tables.contains(&"skill_change_proposals".to_string()));
        assert!(tables.contains(&"agent_evolution_events".to_string()));
        assert!(tables.contains(&"agent_task_runs".to_string()));
        assert!(tables.contains(&"agent_task_run_events".to_string()));
        assert!(tables.contains(&"interaction_queue_clock".to_string()));
        assert!(tables.contains(&"interaction_requests".to_string()));
        assert!(tables.contains(&"interaction_events".to_string()));
        assert!(tables.contains(&"interaction_responses".to_string()));
        assert!(tables.contains(&"agent_subtask_runs".to_string()));
        assert!(tables.contains(&"agent_task_artifacts".to_string()));
        assert!(tables.contains(&"agent_task_artifact_versions".to_string()));
        assert!(tables.contains(&"file_checkpoints".to_string()));
        assert!(tables.contains(&"personas".to_string()));
        assert!(tables.contains(&"workflow_automations".to_string()));
        assert!(tables.contains(&"workflow_automation_schedule_configs".to_string()));
        assert!(tables.contains(&"workflow_automation_occurrences".to_string()));
        assert!(tables.contains(&"workflow_automation_definition_revisions".to_string()));
        assert!(tables.contains(&"workflow_automation_occurrence_approvals".to_string()));
        assert!(tables.contains(&"workflow_automation_occurrence_origins".to_string()));
        assert!(tables.contains(&"workflow_automation_runs".to_string()));
        assert!(tables.contains(&"workflow_automation_scheduler_events".to_string()));
        assert!(tables.contains(&"task_resume_checkpoints".to_string()));
        assert!(tables.contains(&"skill_usage_events".to_string()));
        assert!(tables.contains(&"memory_injection_events".to_string()));
        assert!(tables.contains(&"browser_evidence_captures".to_string()));
        assert!(tables.contains(&"agent_run_events".to_string()));
        assert!(tables.contains(&"agent_trajectories".to_string()));
        assert!(tables.contains(&"dream_runs".to_string()));
        assert!(tables.contains(&"dream_run_events".to_string()));
        assert!(tables.contains(&"dream_artifacts".to_string()));
        assert!(tables.contains(&"ai_usage_records".to_string()));
        assert!(tables.contains(&"ai_model_pricing".to_string()));
        assert!(tables.contains(&"conversation_episodes".to_string()));
        assert!(tables.contains(&"project_events".to_string()));
        assert!(tables.contains(&"project_workspace_items".to_string()));
        assert!(tables.contains(&"knowledge_events".to_string()));
        assert!(tables.contains(&"knowledge_claims".to_string()));
        assert!(tables.contains(&"knowledge_edges".to_string()));
        assert!(tables.contains(&"knowledge_evidence".to_string()));
        assert!(tables.contains(&"graph_versions".to_string()));
        assert!(tables.contains(&"entity_merge_candidates".to_string()));
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
    fn legacy_workflow_schedules_only_auto_migrate_when_daily_utc_is_equivalent() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("baseline migrations should succeed");
        for (id, cron) in [
            ("safe-daily", "0 9 * * *"),
            ("unsafe-wildcard", "* * * * *"),
            ("unsafe-weekday", "0 9 * * 1-5"),
        ] {
            conn.execute(
                "INSERT INTO workflow_automations
                     (id, name, description, workflow_template_id, prompt, trigger_json,
                      trigger_kind, source_scope_json, approval_policy_json, enabled,
                      status, next_run_at)
                 VALUES (?1, ?1, '', 'report_brief', 'test', ?2, 'schedule', '[]', '{}',
                         1, 'ready', '2026-09-01T01:00:00Z')",
                rusqlite::params![id, format!(r#"{{"kind":"schedule","cron":"{cron}"}}"#)],
            )
            .unwrap();
        }
        conn.execute(
            "DELETE FROM _migrations WHERE name = 'v120_classify_legacy_workflow_schedules'",
            [],
        )
        .unwrap();

        run_migrations(&conn).expect("v120 migration should classify legacy schedules");

        let load = |id: &str| {
            conn.query_row(
                "SELECT a.enabled, a.status, a.next_run_at,
                        json_extract(c.config_json, '$.version'),
                        json_extract(c.config_json, '$.timezone'),
                        json_extract(c.config_json, '$.legacyNeedsReview')
                 FROM workflow_automations a
                 JOIN workflow_automation_schedule_configs c ON c.automation_id = a.id
                 WHERE a.id = ?1",
                [id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                },
            )
            .unwrap()
        };
        assert_eq!(
            load("safe-daily"),
            (
                1,
                "ready".to_string(),
                Some("2026-09-01T01:00:00Z".to_string()),
                2,
                "UTC".to_string(),
                0,
            )
        );
        for id in ["unsafe-wildcard", "unsafe-weekday"] {
            assert_eq!(
                load(id),
                (0, "needs_review".to_string(), None, 1, "UTC".to_string(), 1)
            );
        }
    }

    #[test]
    fn durable_workflow_schedule_migration_repairs_a_marked_partial_schema() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("baseline migrations should succeed");
        conn.execute_batch(
            "DROP INDEX idx_workflow_automation_run_attempt;
             ALTER TABLE workflow_automation_runs DROP COLUMN scheduled_for;",
        )
        .expect("test should simulate a partially applied v119 schema");
        let marker_exists: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM _migrations
                     WHERE name = 'v119_durable_workflow_schedules'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(marker_exists);

        run_migrations(&conn).expect("v119 postcondition repair should be restart-safe");

        let columns = conn
            .prepare("PRAGMA table_info(workflow_automation_runs)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        for column in [
            "occurrence_id",
            "scheduled_for",
            "definition_revision",
            "attempt",
        ] {
            assert!(columns.iter().any(|item| item == column), "{column}");
        }
        let attempt_index_exists: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM sqlite_master
                     WHERE type = 'index'
                       AND name = 'idx_workflow_automation_run_attempt'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(attempt_index_exists);
    }

    #[test]
    fn legacy_gemini_query_credentials_are_removed_from_durable_diagnostics() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("baseline migrations should succeed");
        conn.execute(
            "DELETE FROM _migrations
             WHERE name = 'v123_scrub_legacy_gemini_query_credentials'",
            [],
        )
        .unwrap();
        let leaked = "https://generativelanguage.googleapis.com/v1beta/models/test:generateContent?key=secret-fixture";
        conn.execute(
            "INSERT INTO agent_traces
                 (id, conversation_id, started_at, model_id, error_message, trace_json)
             VALUES ('trace-secret', 'conversation', datetime('now'), 'gemini-test', ?1, ?2)",
            rusqlite::params![
                format!("request failed: {leaked}"),
                format!("{{\"error\":\"{leaked}\"}}")
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_run_events
                 (run_id, turn_id, event_seq, version, kind, phase, label, payload_json)
             VALUES ('run-secret', 'turn-secret', 1, 2, 'error', 'done', ?1, ?2)",
            rusqlite::params![
                format!("request failed: {leaked}"),
                format!("{{\"message\":\"{leaked}\"}}")
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ai_usage_records
                 (id, invocation_id, provider_raw_json)
             VALUES ('usage-secret', 'invocation-secret', ?1)",
            [format!("{{\"requestUrl\":\"{leaked}\"}}")],
        )
        .unwrap();

        run_migrations(&conn).expect("credential scrub migration should rerun");

        let trace: (Option<String>, String) = conn
            .query_row(
                "SELECT error_message, trace_json FROM agent_traces WHERE id = 'trace-secret'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let event: (String, String) = conn
            .query_row(
                "SELECT label, payload_json FROM agent_run_events
                 WHERE run_id = 'run-secret' AND event_seq = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let usage: String = conn
            .query_row(
                "SELECT provider_raw_json FROM ai_usage_records WHERE id = 'usage-secret'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        for value in [
            trace.0.unwrap_or_default(),
            trace.1,
            event.0,
            event.1,
            usage,
        ] {
            assert!(!value.contains("secret-fixture"));
            assert!(!value.to_ascii_lowercase().contains("?key="));
        }
    }

    #[test]
    fn workflow_definition_revision_migration_repairs_missing_backfill_with_marker_present() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("baseline migrations should succeed");
        conn.execute_batch(
            "INSERT INTO workflow_automations
                 (id, name, description, workflow_template_id, prompt, trigger_json,
                  trigger_kind, source_scope_json, approval_policy_json, enabled, status)
             VALUES ('repair-definition', 'Repair', '', 'report_brief', 'Run.',
                     '{\"kind\":\"schedule\",\"cron\":\"0 9 * * *\"}', 'schedule', '[]',
                     '{\"requireBeforeRun\":true,\"allowedTools\":[],\"riskLevel\":\"medium\"}',
                     1, 'ready');
             INSERT INTO workflow_automation_schedule_configs
                 (automation_id, config_json, revision)
             VALUES ('repair-definition',
                     '{\"version\":2,\"timezone\":\"UTC\",\"misfirePolicy\":\"run_latest\",\"misfireGraceSeconds\":300,\"overlapPolicy\":\"skip\",\"executionPolicy\":{\"powerMode\":\"standard\",\"orchestrationProfile\":\"balanced\",\"collaborationMode\":\"direct\"},\"legacyNeedsReview\":false}',
                     4);
             INSERT INTO workflow_automation_occurrences
                 (id, automation_id, definition_revision, scheduled_for, status)
             VALUES ('repair-occurrence', 'repair-definition', 4, '2099-01-01T09:00:00Z', 'planned');",
        )
        .unwrap();

        run_migrations(&conn).expect("v121 backfill repair should run despite its marker");

        let snapshot_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM workflow_automation_definition_revisions
                               WHERE automation_id = 'repair-definition' AND revision = 4)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let approval_state: String = conn
            .query_row(
                "SELECT state FROM workflow_automation_occurrence_approvals
                 WHERE occurrence_id = 'repair-occurrence'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let origin: String = conn
            .query_row(
                "SELECT origin FROM workflow_automation_occurrence_origins
                 WHERE occurrence_id = 'repair-occurrence'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(snapshot_exists);
        assert_eq!(approval_state, "pending");
        assert_eq!(origin, "schedule");
        conn.execute_batch(
            "INSERT INTO workflow_automation_occurrences
                 (id, automation_id, definition_revision, scheduled_for, status)
             VALUES ('orphan-occurrence', 'repair-definition', 3,
                     '2098-01-01T09:00:00Z', 'planned');
             INSERT INTO workflow_automation_runs
                 (id, automation_id, status, occurrence_id, scheduled_for,
                  definition_revision, attempt)
             VALUES ('orphan-run', 'repair-definition', 'queued', 'orphan-occurrence',
                     '2098-01-01T09:00:00Z', 3, 1);",
        )
        .unwrap();
        run_migrations(&conn).expect("v121 should explicitly cancel orphaned revisions");
        let orphan_states: (String, String) = conn
            .query_row(
                "SELECT o.status, r.status
                 FROM workflow_automation_occurrences o
                 JOIN workflow_automation_runs r ON r.occurrence_id = o.id
                 WHERE o.id = 'orphan-occurrence'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(orphan_states, ("cancelled".into(), "cancelled".into()));
        conn.execute_batch(
            "DROP TRIGGER validate_workflow_run_status_insert;
             DROP TRIGGER validate_workflow_run_status_update;
             DROP TRIGGER validate_workflow_occurrence_status_insert;
             DROP TRIGGER validate_workflow_occurrence_status_update;
             INSERT INTO workflow_automation_occurrences
                 (id, automation_id, definition_revision, scheduled_for, status)
             VALUES ('legacy-alias-occurrence', 'repair-definition', 2,
                     '2097-01-01T09:00:00Z', 'pending');
             INSERT INTO workflow_automation_runs
                 (id, automation_id, status, occurrence_id, scheduled_for,
                  definition_revision, attempt)
             VALUES ('legacy-alias-run', 'repair-definition', 'pending',
                     'legacy-alias-occurrence', '2097-01-01T09:00:00Z', 2, 1);",
        )
        .unwrap();
        run_migrations(&conn).expect("v121 should normalize aliases before orphan cancellation");
        let alias_states: (String, String) = conn
            .query_row(
                "SELECT o.status, r.status
                 FROM workflow_automation_occurrences o
                 JOIN workflow_automation_runs r ON r.occurrence_id = o.id
                 WHERE o.id = 'legacy-alias-occurrence'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(alias_states, ("cancelled".into(), "cancelled".into()));
        assert!(conn
            .execute(
                "INSERT INTO workflow_automation_runs
                     (id, automation_id, status)
                 VALUES ('invalid-status-run', 'repair-definition', 'mystery')",
                [],
            )
            .is_err());
    }

    #[test]
    fn conversation_prompt_origin_migration_fails_closed_for_legacy_project_prompts() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE projects (id TEXT PRIMARY KEY, system_prompt TEXT NOT NULL DEFAULT '');
             CREATE TABLE conversations (
                 id TEXT PRIMARY KEY,
                 project_id TEXT,
                 system_prompt TEXT NOT NULL DEFAULT ''
             );
             INSERT INTO projects (id, system_prompt) VALUES ('project-1', 'Current project prompt');
             INSERT INTO conversations (id, project_id, system_prompt) VALUES
                 ('plain-user', NULL, 'Personal prompt'),
                 ('project-empty', 'project-1', ''),
                 ('project-copy', 'project-1', 'Current project prompt'),
                 ('project-ambiguous', 'project-1', 'Older or custom prompt');",
        )
        .unwrap();
        let migration = FUTURE_MIGRATIONS
            .iter()
            .find(|(name, _)| *name == "v108_conversation_system_prompt_origin")
            .map(|(_, sql)| *sql)
            .unwrap();
        conn.execute_batch(migration).unwrap();

        for (id, expected) in [
            ("plain-user", "user"),
            ("project-empty", "none"),
            ("project-copy", "project_snapshot"),
            ("project-ambiguous", "legacy_ambiguous"),
        ] {
            let origin: String = conn
                .query_row(
                    "SELECT system_prompt_origin FROM conversations WHERE id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(origin, expected, "origin for {id}");
        }
    }

    #[test]
    fn turn_launch_project_migration_backfills_current_project_best_effort() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE projects (id TEXT PRIMARY KEY);
             CREATE TABLE conversations (
                 id TEXT PRIMARY KEY,
                 project_id TEXT REFERENCES projects(id) ON DELETE SET NULL
             );
             CREATE TABLE conversation_turns (
                 id TEXT PRIMARY KEY,
                 conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                 created_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             INSERT INTO projects (id) VALUES ('project-1');
             INSERT INTO conversations (id, project_id) VALUES
                 ('project-conversation', 'project-1'),
                 ('plain-conversation', NULL);
             INSERT INTO conversation_turns (id, conversation_id) VALUES
                 ('project-turn', 'project-conversation'),
                 ('plain-turn', 'plain-conversation');",
        )
        .unwrap();
        let migration = FUTURE_MIGRATIONS
            .iter()
            .find(|(name, _)| *name == "v109_turn_launch_project")
            .map(|(_, sql)| *sql)
            .unwrap();
        conn.execute_batch(migration).unwrap();
        let backfill = FUTURE_MIGRATIONS
            .iter()
            .find(|(name, _)| *name == "v110_turn_launch_project_backfill")
            .map(|(_, sql)| *sql)
            .unwrap();
        conn.execute_batch(backfill).unwrap();

        let project_id: Option<String> = conn
            .query_row(
                "SELECT launch_project_id FROM conversation_turns WHERE id = 'project-turn'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let plain_project_id: Option<String> = conn
            .query_row(
                "SELECT launch_project_id FROM conversation_turns WHERE id = 'plain-turn'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(project_id.as_deref(), Some("project-1"));
        assert_eq!(plain_project_id, None);
    }

    #[test]
    fn turn_launch_project_migration_recovers_after_column_only_partial_apply() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("baseline migrations should succeed");
        conn.execute_batch(
            "INSERT INTO projects (id, name) VALUES ('project-1', 'Project');
             INSERT INTO conversations (id, provider, model, project_id)
                 VALUES ('conversation-1', 'test', 'test', 'project-1');
             INSERT INTO messages (id, conversation_id, role)
                 VALUES ('message-1', 'conversation-1', 'user');
             INSERT INTO conversation_turns
                 (id, conversation_id, launch_project_id, user_message_id)
                 VALUES ('turn-1', 'conversation-1', NULL, 'message-1');
             DROP INDEX idx_conversation_turns_launch_project;
             DELETE FROM _migrations
             WHERE name IN ('v109_turn_launch_project', 'v110_turn_launch_project_backfill');",
        )
        .expect("simulate interruption after the launch project column");

        run_migrations(&conn).expect("partial launch project migration should recover");

        let project_id: Option<String> = conn
            .query_row(
                "SELECT launch_project_id FROM conversation_turns WHERE id = 'turn-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let index_exists: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM sqlite_master
                     WHERE type = 'index' AND name = 'idx_conversation_turns_launch_project'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(project_id.as_deref(), Some("project-1"));
        assert!(index_exists);
    }

    #[test]
    fn interaction_resume_migrations_recover_after_first_column_only() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("baseline migrations should succeed");
        conn.execute_batch(
            "DROP INDEX idx_interaction_responses_message;
             DROP INDEX idx_interaction_responses_launch_key;
             ALTER TABLE interaction_responses DROP COLUMN response_message_id;
             DELETE FROM _migrations
             WHERE name IN (
                 'v092_interaction_response_launch_key',
                 'v093_interaction_response_message',
                 'v094_interaction_response_resume_indexes'
             );",
        )
        .expect("simulate interruption after the first response column");

        run_migrations(&conn).expect("partial interaction resume migration should recover");

        for column in ["launch_idempotency_key", "response_message_id"] {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM pragma_table_info('interaction_responses')
                         WHERE name = ?1
                     )",
                    [column],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists, "interaction_responses.{column} should exist");
        }
        let index_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name IN (
                     'idx_interaction_responses_launch_key',
                     'idx_interaction_responses_message'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(index_count, 2);
    }

    #[test]
    fn task_checkpoint_resume_migrations_recover_after_first_column_only() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("baseline migrations should succeed");
        conn.execute_batch(
            "DROP INDEX idx_task_resume_checkpoints_response_message;
             DROP INDEX idx_task_resume_checkpoints_launch_key;
             ALTER TABLE task_resume_checkpoints DROP COLUMN response_message_id;
             DELETE FROM _migrations
             WHERE name IN (
                 'v115_task_checkpoint_launch_key',
                 'v116_task_checkpoint_response_message',
                 'v117_task_checkpoint_resume_indexes'
             );",
        )
        .expect("simulate interruption after the first checkpoint response column");

        run_migrations(&conn).expect("partial task checkpoint resume migration should recover");

        for column in ["launch_idempotency_key", "response_message_id"] {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM pragma_table_info('task_resume_checkpoints')
                         WHERE name = ?1
                     )",
                    [column],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists, "task_resume_checkpoints.{column} should exist");
        }
        let index_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name IN (
                     'idx_task_resume_checkpoints_launch_key',
                     'idx_task_resume_checkpoints_response_message'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(index_count, 2);
    }

    #[test]
    fn test_applied_future_migrations_are_not_replayed() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("first run should succeed");
        let changes_after_first_run = conn.total_changes();

        run_migrations(&conn).expect("second run should succeed");

        assert_eq!(
            conn.total_changes(),
            changes_after_first_run,
            "a fully migrated database must not be rewritten during startup"
        );
    }

    #[test]
    fn backfills_legacy_subagent_usage_artifacts_once() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("baseline migrations should succeed");
        conn.execute_batch(
            "INSERT INTO conversations (id, provider, model)
             VALUES ('conv-usage-backfill', 'open_ai', 'gpt-test');
             INSERT INTO messages (id, conversation_id, role, content)
             VALUES ('msg-usage-backfill', 'conv-usage-backfill', 'user', 'test');
             INSERT INTO conversation_turns (id, conversation_id, user_message_id)
             VALUES ('turn-usage-backfill', 'conv-usage-backfill', 'msg-usage-backfill');
             INSERT INTO agent_task_runs
                (id, conversation_id, turn_id, user_message_id, provider, model, status, phase)
             VALUES
                ('run-usage-backfill', 'conv-usage-backfill', 'turn-usage-backfill',
                 'msg-usage-backfill', 'open_ai', 'gpt-test', 'completed', 'done');",
        )
        .expect("insert legacy subagent parents");
        let output = serde_json::json!({
            "kind": "subagent_run",
            "run": {
                "usageTotal": {
                    "promptTokens": 120,
                    "completionTokens": 30,
                    "totalTokens": 150,
                    "cacheReadTokens": 80
                }
            }
        });
        conn.execute(
            "INSERT INTO agent_subtask_runs
                (id, parent_run_id, status, phase, output_json)
             VALUES ('subtask-usage-backfill', 'run-usage-backfill',
                     'completed', 'done', ?1)",
            [output.to_string()],
        )
        .expect("insert legacy subagent artifact");
        let run_usage = serde_json::json!({
            "usageTotal": {
                "promptTokens": 200,
                "completionTokens": 50,
                "totalTokens": 250
            }
        });
        conn.execute(
            "INSERT INTO agent_run_events
                (run_id, turn_id, event_seq, version, kind, phase, payload_json)
             VALUES ('run-usage-backfill', 'turn-usage-backfill', 1, 1,
                     'done', 'done', ?1)",
            [run_usage.to_string()],
        )
        .expect("insert legacy cumulative run usage");

        conn.execute(
            "DELETE FROM _migrations
             WHERE name = 'v080_canonical_ai_usage_accounting'",
            [],
        )
        .expect("simulate a database that has not applied the usage migration");

        run_migrations(&conn).expect("usage migration should backfill subagent artifact");
        run_migrations(&conn).expect("subagent usage backfill should be idempotent");

        let stored: (String, String, i64, i64, i64, String) = conn
            .query_row(
                "SELECT operation_kind, subtask_run_id, prompt_tokens,
                        completion_tokens, total_tokens, usage_source
                 FROM ai_usage_records
                 WHERE invocation_id = 'legacy-subtask:subtask-usage-backfill'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("backfilled subagent usage record");
        assert_eq!(stored.0, "subagent");
        assert_eq!(stored.1, "subtask-usage-backfill");
        assert_eq!(stored.2, 120);
        assert_eq!(stored.3, 30);
        assert_eq!(stored.4, 150);
        assert_eq!(stored.5, "legacy_migrated");
        let main_total: i64 = conn
            .query_row(
                "SELECT total_tokens FROM ai_usage_records
                 WHERE invocation_id = 'legacy-run:run-usage-backfill'",
                [],
                |row| row.get(0),
            )
            .expect("backfilled main run usage record");
        assert_eq!(main_total, 250);

        conn.execute("DELETE FROM ai_usage_records", [])
            .expect("simulate resetting usage statistics");
        run_migrations(&conn).expect("applied migrations should remain safely replayable");
        let recreated: i64 = conn
            .query_row("SELECT COUNT(*) FROM ai_usage_records", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            recreated, 0,
            "a completed one-time backfill must not recreate reset usage records"
        );
    }

    #[test]
    fn backfills_canonical_run_events_from_legacy_task_payloads() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("baseline migrations should succeed");

        conn.execute_batch(
            "INSERT INTO conversations (id, provider, model)
             VALUES ('conv-run-backfill', 'open_ai', 'gpt-test');
             INSERT INTO messages (id, conversation_id, role, content)
             VALUES ('msg-run-backfill', 'conv-run-backfill', 'user', 'test');
             INSERT INTO conversation_turns (id, conversation_id, user_message_id)
             VALUES ('turn-run-backfill', 'conv-run-backfill', 'msg-run-backfill');
             INSERT INTO agent_task_runs
                (id, conversation_id, turn_id, user_message_id, status, phase)
             VALUES
                ('run-backfill', 'conv-run-backfill', 'turn-run-backfill',
                 'msg-run-backfill', 'running', 'routing');",
        )
        .expect("insert legacy run parents");

        let legacy_payload = serde_json::json!({
            "agentRun": {
                "version": 2,
                "runId": "run-backfill",
                "turnId": "turn-run-backfill",
                "eventSeq": 7,
                "kind": "status",
                "phase": "routing",
                "visibility": "internal",
                "persistence": "durable",
                "displayKind": "status",
                "importance": "low",
                "label": "Route selected: Direct",
                "status": "running",
                "payload": { "content": "Route selected: Direct" },
                "createdAt": "2026-07-01T00:00:07Z"
            }
        });
        conn.execute(
            "INSERT INTO agent_task_run_events
                (id, run_id, event_type, label, status, payload_json, created_at)
             VALUES ('legacy-event-7', 'run-backfill', 'status',
                     'Route selected: Direct', 'running', ?1,
                     '2026-07-01T00:00:07Z')",
            [legacy_payload.to_string()],
        )
        .expect("insert legacy wrapped run event");

        conn.execute(
            "DELETE FROM _migrations
             WHERE name = 'v077_backfill_agent_run_events_from_task_events'",
            [],
        )
        .expect("simulate pre-v077 database");
        run_migrations(&conn).expect("upgrade should backfill canonical run events");
        run_migrations(&conn).expect("backfill should remain idempotent");

        let stored: (String, i64, String, String, String, String, String) = conn
            .query_row(
                "SELECT run_id, event_seq, turn_id, kind, visibility, importance, payload_json
                 FROM agent_run_events
                 WHERE run_id = 'run-backfill' AND event_seq = 7",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .expect("backfilled canonical run event");
        assert_eq!(stored.0, "run-backfill");
        assert_eq!(stored.1, 7);
        assert_eq!(stored.2, "turn-run-backfill");
        assert_eq!(stored.3, "status");
        assert_eq!(stored.4, "internal");
        assert_eq!(stored.5, "low");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&stored.6).unwrap()["content"],
            "Route selected: Direct"
        );

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_run_events
                 WHERE run_id = 'run-backfill' AND event_seq = 7",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn reclassifies_historical_controller_events_as_developer_only() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("baseline migrations should succeed");
        conn.execute(
            "INSERT INTO agent_run_events
                (run_id, turn_id, event_seq, version, kind, phase, label, payload_json,
                 visibility, persistence, display_kind, importance)
             VALUES ('legacy-controller', 'turn-1', 1, 2, 'status', 'routing',
                     'legacy route', '{}', 'user', 'durable', 'status', 'normal')",
            [],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM _migrations WHERE name = 'v082_controller_event_visibility'",
            [],
        )
        .unwrap();

        run_migrations(&conn).expect("controller visibility migration should succeed");

        let presentation: (String, String) = conn
            .query_row(
                "SELECT visibility, importance FROM agent_run_events
                 WHERE run_id = 'legacy-controller'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(presentation, ("developer".to_string(), "low".to_string()));
    }

    #[test]
    fn migrates_tool_name_approval_policies_to_structured_wildcards() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("baseline migrations should succeed");
        conn.execute_batch(
            "CREATE TABLE tool_approval_policies (
                tool_name TEXT PRIMARY KEY NOT NULL,
                decision TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             INSERT INTO tool_approval_policies (tool_name, decision, created_at)
             VALUES ('create_file', 'never', '2026-07-01T00:00:00Z');
             DELETE FROM _migrations
             WHERE name = 'v078_migrate_tool_approval_policies';",
        )
        .expect("simulate pre-v078 approval storage");

        run_migrations(&conn).expect("upgrade should migrate approval policies");

        let migrated: (String, String, String, String, String) = conn
            .query_row(
                "SELECT permission_key, tool_name, target_kind, target_value, decision
                 FROM tool_permission_policies
                 WHERE permission_key = 'create_file|tool|*'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("structured wildcard policy");
        assert_eq!(
            migrated,
            (
                "create_file|tool|*".to_string(),
                "create_file".to_string(),
                "tool".to_string(),
                "*".to_string(),
                "never".to_string(),
            )
        );

        let legacy_table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'tool_approval_policies'",
                [],
                |row| row.get::<_, i64>(0).map(|count| count > 0),
            )
            .unwrap();
        assert!(!legacy_table_exists);
    }

    #[test]
    fn migrates_persisted_qwen_payg_provider_ids_without_touching_token_plan() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("baseline migrations should succeed");
        conn.execute_batch(
            "INSERT INTO agent_configs
                (id, name, provider, api_key, base_url, model, summarization_provider)
             VALUES
                ('payg', 'Pay as you go', 'qwen', '',
                 'https://dashscope.aliyuncs.com/compatible-mode/v1', 'qwen-plus', 'qwen'),
                ('token-plan', 'Token plan', 'qwen', '',
                 'https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1',
                 'qwen3.8-max-preview', 'qwen');
             DELETE FROM _migrations
             WHERE name = 'v079_migrate_qwen_payg_provider_ids';",
        )
        .expect("simulate pre-v079 provider configuration");

        run_migrations(&conn).expect("upgrade should rewrite persisted provider ids");

        let payg: (String, String) = conn
            .query_row(
                "SELECT provider, summarization_provider FROM agent_configs WHERE id = 'payg'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            payg,
            (
                "alibaba_model_studio".to_string(),
                "alibaba_model_studio".to_string()
            )
        );

        let token_plan: String = conn
            .query_row(
                "SELECT provider FROM agent_configs WHERE id = 'token-plan'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(token_plan, "qwen");
    }

    #[test]
    fn backfills_document_entities_from_first_seen_doc() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("migrations should succeed");

        conn.execute(
            "INSERT INTO sources (id, root_path) VALUES ('source-1', 'C:/knowledge')",
            [],
        )
        .expect("insert source");
        conn.execute(
            "INSERT INTO documents (id, source_id, path, title, mime_type, file_size, modified_at, content_hash)
             VALUES ('doc-1', 'source-1', 'C:/knowledge/chapter.md', 'Chapter', 'text/markdown', 100, datetime('now'), 'hash-doc-1')",
            [],
        )
        .expect("insert document");
        conn.execute(
            "INSERT INTO entities (id, name, entity_type, description, first_seen_doc)
             VALUES ('entity-1', 'Princess', 'person', 'A protagonist', 'doc-1')",
            [],
        )
        .expect("insert entity");

        let before: i64 = conn
            .query_row("SELECT COUNT(*) FROM document_entities", [], |row| {
                row.get(0)
            })
            .expect("before count");
        assert_eq!(before, 0);

        conn.execute(
            "DELETE FROM _migrations
             WHERE name = 'v064_backfill_document_entities_from_first_seen_doc'",
            [],
        )
        .expect("simulate a database that has not applied v064");

        run_migrations(&conn).expect("migrations should backfill");
        run_migrations(&conn).expect("migrations remain idempotent");

        let after: i64 = conn
            .query_row("SELECT COUNT(*) FROM document_entities", [], |row| {
                row.get(0)
            })
            .expect("after count");
        assert_eq!(after, 1);
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

        let has_canonical_name: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('skills') WHERE name = 'canonical_name'",
                [],
                |row| row.get::<_, i64>(0).map(|n| n > 0),
            )
            .unwrap();
        assert!(
            has_canonical_name,
            "skills.canonical_name column should exist"
        );
    }

    #[test]
    fn test_builtin_playwright_connector_is_retired_in_favor_of_native_browser() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("migrations should succeed");

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mcp_servers
                 WHERE id = 'builtin-playwright-browser'
                    OR builtin_id = 'playwright-browser'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(count, 0);
    }

    #[test]
    fn test_builtin_open_websearch_removed() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("migrations should succeed");

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mcp_servers
                 WHERE id = 'builtin-open-websearch'
                    OR builtin_id = 'open-websearch'
                    OR args LIKE '%open-websearch%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 0,
            "open-websearch should not be seeded as built-in MCP"
        );
    }

    #[test]
    fn test_custom_personas_schema() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("migrations should succeed");

        let has_persona_id: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('conversations') WHERE name = 'persona_id'",
                [],
                |row| row.get::<_, i64>(0).map(|n| n > 0),
            )
            .unwrap();
        assert!(has_persona_id, "conversations.persona_id should exist");

        let has_default_skills: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('personas') WHERE name = 'default_skill_ids_json'",
                [],
                |row| row.get::<_, i64>(0).map(|n| n > 0),
            )
            .unwrap();
        assert!(
            has_default_skills,
            "personas.default_skill_ids_json should exist"
        );
    }

    #[test]
    fn test_agent_config_image_generation_model_schema() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("migrations should succeed");

        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('agent_configs') WHERE name = 'image_generation_model'",
                [],
                |row| row.get::<_, i64>(0).map(|n| n > 0),
            )
            .unwrap();
        assert!(exists, "agent_configs.image_generation_model should exist");
    }

    #[test]
    fn test_project_memory_lifecycle_schema() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("migrations should succeed");

        for column in ["confidence", "expires_at", "conflict_status"] {
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('project_memories') WHERE name = ?1",
                    [column],
                    |row| row.get::<_, i64>(0).map(|n| n > 0),
                )
                .unwrap();
            assert!(exists, "project_memories.{column} should exist");
        }
    }

    #[test]
    fn test_agent_task_artifact_schema() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("migrations should succeed");

        for column in [
            "id",
            "run_id",
            "kind",
            "title",
            "summary",
            "content",
            "paths_json",
            "payload_json",
            "source",
            "created_at",
            "updated_at",
        ] {
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('agent_task_artifacts') WHERE name = ?1",
                    [column],
                    |row| row.get::<_, i64>(0).map(|n| n > 0),
                )
                .unwrap();
            assert!(exists, "agent_task_artifacts.{column} should exist");
        }

        for column in [
            "id",
            "artifact_id",
            "version",
            "title",
            "summary",
            "content",
            "paths_json",
            "payload_json",
            "created_at",
        ] {
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('agent_task_artifact_versions') WHERE name = ?1",
                    [column],
                    |row| row.get::<_, i64>(0).map(|n| n > 0),
                )
                .unwrap();
            assert!(exists, "agent_task_artifact_versions.{column} should exist");
        }
    }

    #[test]
    fn test_knowledge_graph_alias_and_evidence_schema() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("migrations should succeed");

        let alias_table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'entity_aliases'",
                [],
                |row| row.get::<_, i64>(0).map(|n| n > 0),
            )
            .unwrap();
        assert!(alias_table_exists, "entity_aliases table should exist");

        for column in ["evidence_snippet", "confidence"] {
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('entity_links') WHERE name = ?1",
                    [column],
                    |row| row.get::<_, i64>(0).map(|n| n > 0),
                )
                .unwrap();
            assert!(exists, "entity_links.{column} should exist");
        }
    }

    #[test]
    fn test_conversation_archiving_schema() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("migrations should succeed");

        let archived_at_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('conversations') WHERE name = 'archived_at'",
                [],
                |row| row.get::<_, i64>(0).map(|n| n > 0),
            )
            .unwrap();
        assert!(archived_at_exists, "conversations.archived_at should exist");
    }

    #[test]
    fn test_conversation_goals_schema() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("migrations should succeed");

        for column in [
            "conversation_id",
            "id",
            "objective",
            "status",
            "created_at",
            "updated_at",
            "completed_at",
        ] {
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('conversation_goals') WHERE name = ?1",
                    [column],
                    |row| row.get::<_, i64>(0).map(|count| count > 0),
                )
                .unwrap();
            assert!(exists, "conversation_goals.{column} should exist");
        }
    }

    #[test]
    fn test_model_catalog_identity_schema() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("migrations should succeed");

        for column in ["provider_endpoint_id", "model_id"] {
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('agent_configs') WHERE name = ?1",
                    [column],
                    |row| row.get::<_, i64>(0).map(|count| count > 0),
                )
                .unwrap();
            assert!(exists, "agent_configs.{column} should exist");
        }

        for migration in [
            "v083_model_catalog_endpoint_identity",
            "v084_model_catalog_model_identity",
            "v085_model_catalog_identity_backfill",
        ] {
            let migration_exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM _migrations WHERE name = ?1",
                    [migration],
                    |row| row.get::<_, i64>(0).map(|count| count > 0),
                )
                .unwrap();
            assert!(migration_exists, "{migration} should be recorded");
        }
    }

    #[test]
    fn provider_streaming_config_column_is_migrated() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("migrations should succeed");
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('agent_configs') WHERE name = 'provider_streaming_json'",
                [],
                |row| row.get::<_, i64>(0).map(|count| count > 0),
            )
            .unwrap();
        assert!(exists);
    }

    #[test]
    fn test_model_catalog_identity_recovers_from_a_partially_added_column() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_migrations_table(&conn).unwrap();
        conn.execute_batch(V_INITIAL_CONSOLIDATED).unwrap();
        for name in MIGRATION_NAMES {
            conn.execute(
                "INSERT OR IGNORE INTO _migrations (name) VALUES (?1)",
                [name],
            )
            .unwrap();
        }
        conn.execute_batch("ALTER TABLE agent_configs ADD COLUMN provider_endpoint_id TEXT;")
            .unwrap();

        run_migrations(&conn).expect("partial model identity migration should recover");

        let model_id_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('agent_configs') WHERE name = 'model_id'",
                [],
                |row| row.get::<_, i64>(0).map(|count| count > 0),
            )
            .unwrap();
        assert!(model_id_exists);
    }

    #[test]
    fn test_non_destructive_context_compaction_schema() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("migrations should succeed");

        let pointer_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('conversations')
                 WHERE name = 'active_context_compaction_id'",
                [],
                |row| row.get::<_, i64>(0).map(|count| count > 0),
            )
            .unwrap();
        assert!(pointer_exists);

        for column in [
            "operation_id",
            "conversation_id",
            "idempotency_key",
            "snapshot_high_watermark",
            "snapshot_hash",
            "summary",
            "retained_tail_json",
            "retained_start_sort_order",
            "status",
            "source_message_ids_json",
            "source_start_sort_order",
            "source_boundary_sort_order",
            "source_digest",
            "checkpoint_generation",
        ] {
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('context_compactions') WHERE name = ?1",
                    [column],
                    |row| row.get::<_, i64>(0).map(|count| count > 0),
                )
                .unwrap();
            assert!(exists, "context_compactions.{column} should exist");
        }
    }

    #[test]
    fn context_compaction_source_fence_migration_backfills_distinct_generations() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("migrations should succeed");
        conn.execute_batch(
            "DROP INDEX idx_context_compactions_generation;
             DELETE FROM _migrations
             WHERE name = 'v104_context_compaction_generation_index';
             INSERT INTO conversations (id, provider, model)
             VALUES ('conv-generation', 'open_ai', 'gpt-test');
             INSERT INTO context_compactions (
                 id, operation_id, conversation_id, idempotency_key,
                 snapshot_high_watermark, snapshot_hash, summary,
                 retained_tail_json, retained_start_sort_order,
                 tokens_before, tokens_after, provider, model, status,
                 checkpoint_generation, created_at
             ) VALUES
                 ('checkpoint-b', 'checkpoint-b', 'conv-generation', 'request-b',
                  4, 'hash-b', 'summary-b', '[]', 3,
                  100, 50, 'test', 'test', 'completed', 1, '2026-01-02 00:00:00'),
                 ('checkpoint-a', 'checkpoint-a', 'conv-generation', 'request-a',
                  2, 'hash-a', 'summary-a', '[]', 1,
                  80, 40, 'test', 'test', 'completed', 1, '2026-01-01 00:00:00');",
        )
        .expect("simulate legacy checkpoints with duplicate default generations");

        run_migrations(&conn).expect("generation migration should backfill before indexing");

        let generations = conn
            .prepare(
                "SELECT id, checkpoint_generation
                 FROM context_compactions
                 WHERE conversation_id = 'conv-generation'
                 ORDER BY checkpoint_generation",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            generations,
            vec![
                ("checkpoint-a".to_string(), 1),
                ("checkpoint-b".to_string(), 2),
            ]
        );
    }

    #[test]
    fn test_context_compaction_migration_recovers_after_column_only_partial_apply() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("migrations should succeed");
        conn.execute_batch(
            "DROP TABLE context_compactions;
             DELETE FROM _migrations
             WHERE name IN (
                 'v089_non_destructive_context_compactions',
                 'v090_context_compaction_checkpoints'
             );",
        )
        .expect("simulate interruption after the pointer column was added");

        run_migrations(&conn).expect("partial compaction migration should recover");

        let table_exists: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM sqlite_master
                     WHERE type = 'table' AND name = 'context_compactions'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(table_exists);
        let applied: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM _migrations
                 WHERE name IN (
                     'v089_non_destructive_context_compactions',
                     'v090_context_compaction_checkpoints'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(applied, 2);
    }

    #[test]
    fn legacy_reasoning_sentinel_is_removed_idempotently() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("baseline migrations should succeed");
        conn.execute(
            "INSERT INTO conversations (id, provider, model) VALUES (?1, ?2, ?3)",
            ("conv-reasoning", "deep_seek", "deepseek-v4"),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (id, conversation_id, role, thinking) VALUES (?1, ?2, ?3, ?4)",
            (
                "message-reasoning",
                "conv-reasoning",
                "assistant",
                crate::llm::reasoning_replay::LEGACY_MISSING_REASONING_SENTINEL,
            ),
        )
        .unwrap();
        conn.execute(
            "DELETE FROM _migrations WHERE name = 'v111_remove_legacy_reasoning_sentinel'",
            [],
        )
        .unwrap();

        run_migrations(&conn).expect("reasoning cleanup should succeed");
        run_migrations(&conn).expect("reasoning cleanup should remain idempotent");

        let thinking: Option<String> = conn
            .query_row(
                "SELECT thinking FROM messages WHERE id = 'message-reasoning'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(thinking, None);
        let applied: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM _migrations WHERE name = 'v111_remove_legacy_reasoning_sentinel'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(applied, 1);
    }

    #[test]
    fn legacy_agent_run_event_gaps_are_preserved_without_rewriting_history() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("baseline migrations should succeed");
        conn.execute_batch(
            "INSERT INTO agent_run_events
                 (run_id, turn_id, event_seq, version, kind, phase, label, payload_json)
             VALUES
                 ('run-gap', 'turn-gap', 1, 2, 'status', 'routing', 'first', '{}'),
                 ('run-gap', 'turn-gap', 4, 2, 'outputDelta', 'responding', 'second', '{\"delta\":\"hello\"}'),
                 ('run-gap', 'turn-gap', 7, 2, 'done', 'done', 'third', '{\"message\":\"done\"}'),
                 ('run-other', 'turn-other', 9, 2, 'status', 'routing', 'only', '{}');
             DELETE FROM _migrations
             WHERE name = 'v113_normalize_agent_run_event_sequences';",
        )
        .expect("simulate a pre-v113 ledger with keepalive gaps");

        run_migrations(&conn).expect("legacy event sequences should remain readable");
        run_migrations(&conn).expect("gap compatibility should remain idempotent");

        let run_gap = conn
            .prepare(
                "SELECT event_seq, label, payload_json
                 FROM agent_run_events
                 WHERE run_id = 'run-gap'
                 ORDER BY event_seq",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            run_gap,
            vec![
                (1, "first".to_string(), "{}".to_string()),
                (4, "second".to_string(), "{\"delta\":\"hello\"}".to_string()),
                (7, "third".to_string(), "{\"message\":\"done\"}".to_string()),
            ]
        );
        let other_sequence: i64 = conn
            .query_row(
                "SELECT event_seq FROM agent_run_events WHERE run_id = 'run-other'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(other_sequence, 9);
        let applied: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM _migrations
                 WHERE name = 'v113_normalize_agent_run_event_sequences'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(applied, 1);
    }

    #[test]
    fn agent_run_event_storage_drops_duplicate_index_and_follows_run_deletion() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("baseline migrations should succeed");
        conn.execute_batch(
            "INSERT INTO conversations (id, provider, model)
             VALUES ('conv-event-owner', 'open_ai', 'gpt-test');
             INSERT INTO messages (id, conversation_id, role, content)
             VALUES ('msg-event-owner', 'conv-event-owner', 'user', 'test');
             INSERT INTO conversation_turns (id, conversation_id, user_message_id)
             VALUES ('turn-event-owner', 'conv-event-owner', 'msg-event-owner');
             INSERT INTO agent_task_runs
                (id, conversation_id, turn_id, user_message_id, status, phase)
             VALUES
                ('run-event-owner', 'conv-event-owner', 'turn-event-owner',
                 'msg-event-owner', 'completed', 'done');
             INSERT INTO agent_run_events
                (run_id, turn_id, event_seq, version, kind, phase, label, payload_json)
             VALUES
                ('run-event-owner', 'turn-event-owner', 1, 2, 'done', 'done',
                 'done', '{\"message\":\"done\"}');",
        )
        .expect("insert owned run event");

        let duplicate_index_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_agent_run_events_run'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(duplicate_index_count, 0);

        conn.execute(
            "DELETE FROM agent_task_runs WHERE id = 'run-event-owner'",
            [],
        )
        .expect("delete task run");
        let remaining_events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_run_events WHERE run_id = 'run-event-owner'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining_events, 0);
    }

    #[test]
    fn legacy_tool_preview_storms_collapse_to_live_snapshots_and_terminal_history() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("baseline migrations should succeed");
        conn.execute_batch(
            "DELETE FROM _migrations
             WHERE name = 'v125_compact_legacy_tool_preview_events';
             INSERT INTO conversations (id, provider, model) VALUES
               ('conv-preview-active', 'deep_seek', 'deepseek-v4-pro'),
               ('conv-preview-done', 'deep_seek', 'deepseek-v4-pro');
             INSERT INTO messages (id, conversation_id, role, content) VALUES
               ('msg-preview-active', 'conv-preview-active', 'user', 'active'),
               ('msg-preview-done', 'conv-preview-done', 'user', 'done');
             INSERT INTO conversation_turns
               (id, conversation_id, user_message_id, status) VALUES
               ('turn-preview-active', 'conv-preview-active', 'msg-preview-active', 'running'),
               ('turn-preview-done', 'conv-preview-done', 'msg-preview-done', 'success');
             INSERT INTO agent_task_runs
               (id, conversation_id, turn_id, user_message_id, status, phase) VALUES
               ('run-preview-active', 'conv-preview-active', 'turn-preview-active',
                'msg-preview-active', 'running', 'tooling'),
               ('run-preview-done', 'conv-preview-done', 'turn-preview-done',
                'msg-preview-done', 'completed', 'done');
             INSERT INTO agent_run_events
               (run_id, turn_id, event_seq, version, kind, phase, label, status, payload_json)
             VALUES
               ('run-preview-active', 'turn-preview-active', 1, 2, 'toolProgress',
                'tooling', 'create_file', 'preparing',
                json_object('run', json_object('callId', 'call-a'))),
               ('run-preview-active', 'turn-preview-active', 2, 2, 'toolProgress',
                'tooling', 'create_file', 'preparing',
                json_object('run', json_object('callId', 'call-a'))),
               ('run-preview-active', 'turn-preview-active', 3, 2, 'toolProgress',
                'tooling', 'edit_file', 'preparing', '{}'),
               ('run-preview-active', 'turn-preview-active', 4, 2, 'toolProgress',
                'tooling', 'edit_file', 'preparing', '{}'),
               ('run-preview-active', 'turn-preview-active', 5, 2, 'toolProgress',
                'tooling', 'create_file', 'preparing',
                json_object('run', json_object('callId', 'call-b'))),
               ('run-preview-active', 'turn-preview-active', 6, 2, 'toolProgress',
                'tooling', 'create_file', 'preparing',
                json_object('run', json_object('callId', 'call-b'))),
               ('run-preview-done', 'turn-preview-done', 1, 2, 'toolProgress',
                'tooling', 'create_file', 'preparing', '{}'),
               ('run-preview-done', 'turn-preview-done', 2, 2, 'done',
                'done', 'completed', 'completed', '{}');",
        )
        .expect("simulate legacy cumulative tool previews");

        run_migrations(&conn).expect("preview compaction migration should succeed");
        run_migrations(&conn).expect("preview compaction should remain idempotent");

        let active = conn
            .prepare(
                "SELECT event_seq, label FROM agent_run_events
                 WHERE run_id = 'run-preview-active' ORDER BY event_seq",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            active,
            vec![
                (2, "create_file".to_string()),
                (4, "edit_file".to_string()),
                (6, "create_file".to_string())
            ]
        );
        let terminal_progress: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_run_events
                 WHERE run_id = 'run-preview-done' AND kind = 'toolProgress'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(terminal_progress, 0);
    }

    #[test]
    fn fresh_agent_configs_do_not_inherit_a_hidden_output_cap() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("migrations should succeed");
        conn.execute(
            "INSERT INTO agent_configs (id, name, provider, model)
             VALUES ('auto-output', 'Auto output', 'deep_seek', 'deepseek-v4-pro')",
            [],
        )
        .unwrap();

        let max_tokens: Option<i64> = conn
            .query_row(
                "SELECT max_tokens FROM agent_configs WHERE id = 'auto-output'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(max_tokens, None);
    }
}
