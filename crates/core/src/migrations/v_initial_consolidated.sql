-- Consolidated initial schema (v001–v016).
-- Used only for fresh installs (empty _migrations table).
-- Produces the exact same final schema as running all 16 migrations sequentially.

-- Sources table
CREATE TABLE IF NOT EXISTS sources (
    id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL DEFAULT 'local_folder',
    root_path TEXT NOT NULL UNIQUE,
    include_globs TEXT NOT NULL DEFAULT '[]',
    exclude_globs TEXT NOT NULL DEFAULT '[]',
    watch_enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Documents table (v001 + v013 metadata column folded in)
CREATE TABLE IF NOT EXISTS documents (
    id TEXT PRIMARY KEY NOT NULL,
    source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    title TEXT,
    mime_type TEXT NOT NULL DEFAULT 'text/plain',
    file_size INTEGER NOT NULL DEFAULT 0,
    modified_at TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    indexed_at TEXT NOT NULL DEFAULT (datetime('now')),
    metadata TEXT NOT NULL DEFAULT '{}',
    UNIQUE(source_id, path)
);

CREATE INDEX IF NOT EXISTS idx_documents_source ON documents(source_id);
CREATE INDEX IF NOT EXISTS idx_documents_path ON documents(path);
CREATE INDEX IF NOT EXISTS idx_documents_modified ON documents(modified_at);

-- Chunks table
CREATE TABLE IF NOT EXISTS chunks (
    id TEXT PRIMARY KEY NOT NULL,
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL,
    kind TEXT NOT NULL DEFAULT 'text',
    content TEXT NOT NULL,
    start_offset INTEGER NOT NULL,
    end_offset INTEGER NOT NULL,
    line_start INTEGER NOT NULL,
    line_end INTEGER NOT NULL,
    content_hash TEXT NOT NULL,
    metadata_json TEXT DEFAULT '{}',
    UNIQUE(document_id, chunk_index)
);

CREATE INDEX IF NOT EXISTS idx_chunks_document ON chunks(document_id);
CREATE INDEX IF NOT EXISTS idx_chunks_hash ON chunks(content_hash);

-- FTS5 full-text search index on chunks
CREATE VIRTUAL TABLE IF NOT EXISTS fts_chunks USING fts5(
    content,
    content='chunks',
    content_rowid='rowid',
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
    INSERT INTO fts_chunks(rowid, content) VALUES (new.rowid, new.content);
END;

CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
    INSERT INTO fts_chunks(fts_chunks, rowid, content) VALUES ('delete', old.rowid, old.content);
END;

CREATE TRIGGER IF NOT EXISTS chunks_au AFTER UPDATE ON chunks BEGIN
    INSERT INTO fts_chunks(fts_chunks, rowid, content) VALUES ('delete', old.rowid, old.content);
    INSERT INTO fts_chunks(rowid, content) VALUES (new.rowid, new.content);
END;

-- Playbooks
CREATE TABLE IF NOT EXISTS playbooks (
    id TEXT PRIMARY KEY NOT NULL,
    title TEXT NOT NULL,
    body_md TEXT NOT NULL DEFAULT '',
    goal TEXT DEFAULT '',
    prerequisites TEXT DEFAULT '',
    notes TEXT DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_playbooks_title ON playbooks(title);

-- Playbook citations
CREATE TABLE IF NOT EXISTS playbook_citations (
    id TEXT PRIMARY KEY NOT NULL,
    playbook_id TEXT NOT NULL REFERENCES playbooks(id) ON DELETE CASCADE,
    chunk_id TEXT NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    sort_order INTEGER NOT NULL DEFAULT 0,
    annotation TEXT DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(playbook_id, chunk_id)
);

CREATE INDEX IF NOT EXISTS idx_pb_citations_playbook ON playbook_citations(playbook_id);

-- Query logs
CREATE TABLE IF NOT EXISTS query_logs (
    id TEXT PRIMARY KEY NOT NULL,
    query_text TEXT NOT NULL,
    filters_json TEXT DEFAULT '{}',
    result_count INTEGER NOT NULL DEFAULT 0,
    clicked_chunk_ids TEXT DEFAULT '[]',
    pinned_chunk_ids TEXT DEFAULT '[]',
    excluded_chunk_ids TEXT DEFAULT '[]',
    duration_ms INTEGER DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_query_logs_created ON query_logs(created_at);

-- Embeddings table
CREATE TABLE IF NOT EXISTS embeddings (
    id TEXT PRIMARY KEY,
    chunk_id TEXT NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    model TEXT NOT NULL DEFAULT 'tfidf-v1',
    vector BLOB NOT NULL,
    dimensions INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(chunk_id, model)
);

CREATE INDEX IF NOT EXISTS idx_embeddings_chunk ON embeddings(chunk_id);
CREATE INDEX IF NOT EXISTS idx_embeddings_model ON embeddings(model);

-- Feedback table
CREATE TABLE IF NOT EXISTS feedback (
    id TEXT PRIMARY KEY,
    chunk_id TEXT NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    query_text TEXT NOT NULL,
    action TEXT NOT NULL CHECK(action IN ('upvote', 'downvote', 'pin')),
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_feedback_chunk ON feedback(chunk_id);
CREATE INDEX IF NOT EXISTS idx_feedback_query ON feedback(query_text);

-- Privacy configuration
CREATE TABLE IF NOT EXISTS privacy_config (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- User-authored long-term memory notes
CREATE TABLE IF NOT EXISTS user_memories (
    id TEXT PRIMARY KEY NOT NULL,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Embedder configuration
CREATE TABLE IF NOT EXISTS embedder_config (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT OR IGNORE INTO embedder_config (key, value) VALUES ('provider', 'local');
INSERT OR IGNORE INTO embedder_config (key, value) VALUES ('api_key', '');
INSERT OR IGNORE INTO embedder_config (key, value) VALUES ('api_base_url', 'https://api.openai.com/v1');
INSERT OR IGNORE INTO embedder_config (key, value) VALUES ('api_model', 'text-embedding-3-small');
INSERT OR IGNORE INTO embedder_config (key, value) VALUES ('model_path', '');
INSERT OR IGNORE INTO embedder_config (key, value) VALUES ('vector_dimensions', '384');

-- Conversations (v007 base)
CREATE TABLE IF NOT EXISTS conversations (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL DEFAULT '',
    provider    TEXT NOT NULL,
    model       TEXT NOT NULL,
    system_prompt TEXT NOT NULL DEFAULT '',
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Messages (v007 base + v011 thinking column folded in)
CREATE TABLE IF NOT EXISTS messages (
    id              TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role            TEXT NOT NULL,
    content         TEXT NOT NULL DEFAULT '',
    tool_call_id    TEXT,
    tool_calls_json TEXT,
    artifacts_json  TEXT,
    token_count     INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    sort_order      INTEGER NOT NULL DEFAULT 0,
    thinking        TEXT
);

CREATE INDEX IF NOT EXISTS idx_messages_conversation
    ON messages(conversation_id, sort_order);

-- Agent configs (v007 base + v008/v010/v012/v014 columns folded in)
CREATE TABLE IF NOT EXISTS agent_configs (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    provider    TEXT NOT NULL,
    api_key     TEXT NOT NULL DEFAULT '',
    base_url    TEXT,
    model       TEXT NOT NULL,
    temperature REAL DEFAULT 0.3,
    max_tokens  INTEGER DEFAULT 4096,
    is_default  INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    context_window          INTEGER DEFAULT NULL,
    reasoning_enabled       BOOLEAN DEFAULT NULL,
    thinking_budget         INTEGER DEFAULT NULL,
    reasoning_effort        TEXT DEFAULT NULL,
    max_iterations          INTEGER,
    summarization_model     TEXT DEFAULT NULL,
    summarization_provider  TEXT DEFAULT NULL
);

-- Conversation sources
CREATE TABLE IF NOT EXISTS conversation_sources (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    created_at DATETIME DEFAULT (datetime('now')),
    PRIMARY KEY (conversation_id, source_id)
);

-- Answer cache
CREATE TABLE IF NOT EXISTS answer_cache (
    id TEXT PRIMARY KEY,
    query_hash TEXT NOT NULL,
    query_text TEXT NOT NULL,
    answer_text TEXT NOT NULL,
    citations TEXT NOT NULL DEFAULT '[]',
    source_filter TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    hit_count INTEGER NOT NULL DEFAULT 0,
    UNIQUE(query_hash, source_filter)
);

CREATE INDEX IF NOT EXISTS idx_answer_cache_hash ON answer_cache(query_hash);

-- OCR configuration
CREATE TABLE IF NOT EXISTS ocr_config (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Conversation checkpoints (v017)
CREATE TABLE IF NOT EXISTS conversation_checkpoints (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    label TEXT NOT NULL DEFAULT '',
    message_count INTEGER NOT NULL,
    estimated_tokens INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Archived messages tied to checkpoints (v017)
CREATE TABLE IF NOT EXISTS archived_messages (
    id TEXT PRIMARY KEY,
    checkpoint_id TEXT NOT NULL REFERENCES conversation_checkpoints(id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL DEFAULT '',
    tool_call_id TEXT,
    tool_calls_json TEXT,
    artifacts_json TEXT,
    token_count INTEGER NOT NULL DEFAULT 0,
    original_sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
