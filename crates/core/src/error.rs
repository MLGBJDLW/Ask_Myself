//! Core error types for the nexa-core crate.

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

    #[error("Stream interrupted: {0}")]
    StreamIncomplete(String),

    #[error("Operation cancelled: {0}")]
    Cancelled(String),

    /// The current turn intentionally yielded after creating a durable
    /// interaction request. This is a non-terminal control-flow outcome: the
    /// host must keep the original turn/run resumable instead of reporting a
    /// failure or synthesizing an assistant answer.
    #[error("Agent is awaiting user input for interaction {interaction_id}")]
    AwaitingUserInput { interaction_id: String },

    #[error("MCP error: {0}")]
    Mcp(String),

    /// An MCP failure that means the underlying connection can no longer be
    /// trusted (for example, a timeout, closed stream, or transient HTTP 5xx).
    /// Keeping this distinct from JSON-RPC/application errors prevents a tool
    /// mistake from needlessly restarting a healthy stateful MCP server.
    #[error("MCP transport error: {0}")]
    McpTransport(String),
}
