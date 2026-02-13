CREATE TABLE IF NOT EXISTS conversation_sources (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    created_at DATETIME DEFAULT (datetime('now')),
    PRIMARY KEY (conversation_id, source_id)
);
