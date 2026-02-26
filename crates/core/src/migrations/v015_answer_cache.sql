-- Answer cache: stores recent LLM answers to avoid redundant ReAct loops.

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
