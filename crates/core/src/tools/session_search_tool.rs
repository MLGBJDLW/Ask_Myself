//! SessionSearchTool - agent-facing cross-conversation search.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/search_sessions.json");

pub struct SessionSearchTool;

#[derive(Debug, Deserialize)]
struct SessionSearchArgs {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl Tool for SessionSearchTool {
    fn name(&self) -> &str {
        "search_sessions"
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

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: SessionSearchArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid search_sessions arguments: {e}"))
        })?;
        let results = db.search_conversations(&args.query, args.limit.unwrap_or(8))?;
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: format!("Found {} past session message(s).", results.len()),
            is_error: false,
            artifacts: Some(serde_json::json!({
                "kind": "sessionSearchResults",
                "query": args.query,
                "results": results
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::memory::estimate_tokens;
    use crate::conversation::{ConversationMessage, CreateConversationInput};
    use crate::llm::Role;

    #[tokio::test]
    async fn searches_past_messages() {
        let db = Database::open_memory().unwrap();
        let conv = db
            .create_conversation(&CreateConversationInput {
                provider: "local".to_string(),
                model: "test".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
            })
            .unwrap();
        db.add_message(&ConversationMessage {
            id: "m1".to_string(),
            conversation_id: conv.id,
            role: Role::User,
            content: "Remember that harness dry-run should report blocked readiness.".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: estimate_tokens("harness dry-run"),
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        })
        .unwrap();

        let tool = SessionSearchTool;
        let args = serde_json::json!({
            "query": "harness dry-run",
            "limit": 5
        });
        let result = tool
            .execute("call-1", &args.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("1"));
    }
}
