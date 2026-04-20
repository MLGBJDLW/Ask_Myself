//! McpTool — adapter that bridges an MCP server tool to the local `Tool` trait.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::db::Database;
use crate::error::CoreError;
use crate::mcp::client::McpClient;
use crate::mcp::McpToolInfo;

use super::{Tool, ToolCategory, ToolResult};

/// Wraps an MCP tool so it implements the local `Tool` trait.
pub struct McpTool {
    info: McpToolInfo,
    registry_name: String,
    description: String,
    client: Arc<Mutex<McpClient>>,
    _server_id: String,
}

impl McpTool {
    pub fn new(
        info: McpToolInfo,
        client: Arc<Mutex<McpClient>>,
        server_id: String,
        registry_name: String,
        server_name: String,
    ) -> Self {
        let description = match info.description.as_deref() {
            Some(text) if !text.trim().is_empty() => {
                format!("MCP server '{server_name}': {}", text.trim())
            }
            _ => format!("MCP server '{server_name}' tool '{}'", info.name),
        };
        Self {
            info,
            registry_name,
            description,
            client,
            _server_id: server_id,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.registry_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::Mcp]
    }

    fn parameters_schema(&self) -> Value {
        self.info.input_schema.clone()
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        _db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: Value =
            serde_json::from_str(arguments).unwrap_or(Value::Object(Default::default()));
        let mut client = self.client.lock().await;
        match client.call_tool(&self.info.name, args).await {
            Ok(result) => Ok(ToolResult {
                call_id: call_id.to_string(),
                content: result,
                is_error: false,
                artifacts: None,
            }),
            Err(e) => Ok(ToolResult {
                call_id: call_id.to_string(),
                content: format!("MCP tool error: {e}"),
                is_error: true,
                artifacts: None,
            }),
        }
    }
}
