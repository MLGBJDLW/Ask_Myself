//! AgentMemoryTool - procedural memory for reusable agent workflows.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;
use crate::evolution::CreateAgentProceduralMemoryInput;

use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/manage_agent_memory.json");

pub struct AgentMemoryTool;

#[derive(Debug, Deserialize)]
struct AgentMemoryArgs {
    action: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    confidence: Option<f32>,
}

fn missing(field: &str, action: &str) -> CoreError {
    CoreError::InvalidInput(format!(
        "{field} is required for manage_agent_memory action '{action}'"
    ))
}

#[async_trait]
impl Tool for AgentMemoryTool {
    fn name(&self) -> &str {
        "manage_agent_memory"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::Core, ToolCategory::Knowledge]
    }

    fn requires_confirmation(&self, args: &serde_json::Value) -> bool {
        args.get("action")
            .and_then(|v| v.as_str())
            .is_some_and(|action| action == "delete")
    }

    fn confirmation_message(&self, args: &serde_json::Value) -> Option<String> {
        let action = args.get("action")?.as_str()?;
        if action != "delete" {
            return None;
        }
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("<missing>");
        Some(format!("Delete agent procedural memory {id}."))
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: AgentMemoryArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid manage_agent_memory arguments: {e}"))
        })?;
        let action = args.action.trim();

        match action {
            "record" => {
                let memory =
                    db.create_agent_procedural_memory(&CreateAgentProceduralMemoryInput {
                        title: args.title.ok_or_else(|| missing("title", action))?,
                        content: args.content.ok_or_else(|| missing("content", action))?,
                        tags: args.tags,
                        source: Some("agent_tool".to_string()),
                        confidence: args.confidence,
                    })?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("Agent procedural memory recorded: {}", memory.id),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "agentProceduralMemory",
                        "memory": memory
                    })),
                })
            }
            "search" => {
                let query = args.query.ok_or_else(|| missing("query", action))?;
                let memories =
                    db.search_agent_procedural_memories(&query, args.limit.unwrap_or(5))?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("Found {} procedural memory item(s).", memories.len()),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "agentProceduralMemoryList",
                        "memories": memories
                    })),
                })
            }
            "list" => {
                let memories = db.list_agent_procedural_memories(args.limit.unwrap_or(10))?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("Found {} procedural memory item(s).", memories.len()),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "agentProceduralMemoryList",
                        "memories": memories
                    })),
                })
            }
            "delete" => {
                let id = args.id.ok_or_else(|| missing("id", action))?;
                db.delete_agent_procedural_memory(&id)?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("Deleted agent procedural memory: {id}"),
                    is_error: false,
                    artifacts: None,
                })
            }
            other => Err(CoreError::InvalidInput(format!(
                "Unknown manage_agent_memory action '{other}'"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn record_then_search_memory() {
        let db = Database::open_memory().unwrap();
        let tool = AgentMemoryTool;
        let record = serde_json::json!({
            "action": "record",
            "title": "FTS fallback",
            "content": "When an FTS table is absent, fall back to LIKE search.",
            "tags": ["sqlite", "search"],
            "confidence": 0.8
        });
        let result = tool
            .execute("call-1", &record.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(!result.is_error);

        let search = serde_json::json!({
            "action": "search",
            "query": "sqlite fts",
            "limit": 5
        });
        let result = tool
            .execute("call-2", &search.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("1"));
    }
}
