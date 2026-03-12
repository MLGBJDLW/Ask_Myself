//! Core error types for the ask-core crate.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum CoreError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Embedding error: {0}")]
    Embedding(String),

    #[error("LLM error: {0}")]
    Llm(String),

    #[error("LLM rate limited, retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },

    #[error("LLM context window exceeded: {0} tokens > {1} max")]
    ContextOverflow(u32, u32),

    #[error("Agent error: {0}")]
    Agent(String),

    #[error("OCR error: {0}")]
    Ocr(String),

    #[error("Video error: {0}")]
    Video(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("LLM transient error (retriable): {0}")]
    TransientLlm(String),

    #[error("Stream ended without completion marker — response may be truncated")]
    StreamIncomplete,

    #[error("MCP error: {0}")]
    Mcp(String),
}
