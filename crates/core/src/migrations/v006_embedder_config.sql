-- Embedder configuration
CREATE TABLE IF NOT EXISTS embedder_config (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Default: use local ONNX embedder
INSERT OR IGNORE INTO embedder_config (key, value) VALUES ('provider', 'local');
INSERT OR IGNORE INTO embedder_config (key, value) VALUES ('api_key', '');
INSERT OR IGNORE INTO embedder_config (key, value) VALUES ('api_base_url', 'https://api.openai.com/v1');
INSERT OR IGNORE INTO embedder_config (key, value) VALUES ('api_model', 'text-embedding-3-small');
INSERT OR IGNORE INTO embedder_config (key, value) VALUES ('model_path', '');
INSERT OR IGNORE INTO embedder_config (key, value) VALUES ('vector_dimensions', '384');
