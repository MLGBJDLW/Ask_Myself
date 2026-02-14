//! Tool system — trait, registry, and built-in tools for the agent framework.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::CoreError;
use crate::llm::ToolDefinition;

// ---------------------------------------------------------------------------
// Shared tool-definition helper (parsed from JSON once via OnceLock)
// ---------------------------------------------------------------------------

/// Cached tool definition loaded from a JSON file at compile time.
pub(crate) struct ToolDef {
    pub description: String,
    pub parameters: serde_json::Value,
}

impl ToolDef {
    /// Parse a tool-definition JSON blob (`include_str!` output) exactly once.
    pub fn from_json<'a>(lock: &'a OnceLock<ToolDef>, json_str: &str) -> &'a ToolDef {
        lock.get_or_init(|| {
            let v: serde_json::Value =
                serde_json::from_str(json_str).expect("invalid tool definition JSON");
            ToolDef {
                description: v["description"]
                    .as_str()
                    .expect("tool JSON missing 'description'")
                    .to_string(),
                parameters: v["parameters"].clone(),
            }
        })
    }
}

pub mod chunk_context_tool;
pub mod fetch_url_tool;
pub mod file_tool;
pub mod list_dir_tool;
pub mod list_documents_tool;
pub mod list_sources_tool;
pub mod playbook_tool;
pub mod search_tool;
pub mod summarize_tool;
pub mod write_note_tool;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Result returned by a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub call_id: String,
    pub content: String,
    pub is_error: bool,
    pub artifacts: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Tool trait
// ---------------------------------------------------------------------------

/// A tool that can be invoked by the agent during a conversation.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Machine-readable name used in LLM tool-call requests.
    fn name(&self) -> &str;

    /// Human-readable description shown to the LLM.
    fn description(&self) -> &str;

    /// JSON Schema describing the parameters the tool accepts.
    fn parameters_schema(&self) -> serde_json::Value;

    /// Build a [`ToolDefinition`] suitable for an LLM completion request.
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
        }
    }

    /// Execute the tool with the given JSON-encoded arguments.
    ///
    /// `source_scope` restricts results to the given source IDs when non-empty
    /// (used for per-conversation source scoping).
    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError>;
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// A collection of tools available to the agent.
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Register a tool.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Return [`ToolDefinition`]s for every registered tool.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|t| t.definition()).collect()
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|t| t.as_ref())
    }

    /// Execute a tool by name, returning an error if the tool is not found.
    pub async fn execute(
        &self,
        name: &str,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let tool = self
            .get(name)
            .ok_or_else(|| CoreError::InvalidInput(format!("Unknown tool: {name}")))?;
        tool.execute(call_id, arguments, db, source_scope).await
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Default registry builder
// ---------------------------------------------------------------------------

/// Build the default tool registry with all built-in tools.
pub fn default_tool_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(search_tool::SearchTool));
    registry.register(Box::new(playbook_tool::PlaybookTool));
    registry.register(Box::new(file_tool::FileTool));
    registry.register(Box::new(summarize_tool::RetrieveEvidenceTool));
    registry.register(Box::new(list_sources_tool::ListSourcesTool));
    registry.register(Box::new(list_documents_tool::ListDocumentsTool));
    registry.register(Box::new(list_dir_tool::ListDirTool));
    registry.register(Box::new(chunk_context_tool::ChunkContextTool));
    registry.register(Box::new(fetch_url_tool::FetchUrlTool));
    registry.register(Box::new(write_note_tool::WriteNoteTool));
    registry
}
