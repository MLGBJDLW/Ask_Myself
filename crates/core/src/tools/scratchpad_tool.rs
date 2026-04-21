//! UpdateScratchpadTool — mutates the per-conversation agent scratchpad.
//!
//! The scratchpad is rendered into the system prompt on every turn, giving
//! the agent a durable notebook scoped to the current conversation.

use std::sync::OnceLock;

use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;

use crate::agent::scratchpad::MAX_SCRATCHPAD_CHARS;
use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/update_scratchpad.json");

pub struct UpdateScratchpadTool;

#[derive(Deserialize)]
struct UpdateScratchpadArgs {
    action: String,
    #[serde(default)]
    content: Option<String>,
}

fn error_result(call_id: &str, message: impl Into<String>) -> ToolResult {
    ToolResult {
        call_id: call_id.to_string(),
        content: message.into(),
        is_error: true,
        artifacts: None,
    }
}

#[async_trait]
impl Tool for UpdateScratchpadTool {
    fn name(&self) -> &str {
        "update_scratchpad"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::Core]
    }

    async fn execute(
        &self,
        call_id: &str,
        _arguments: &str,
        _db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        // The scratchpad is conversation-scoped; this entry point is unused.
        Ok(error_result(
            call_id,
            "update_scratchpad requires a conversation context.",
        ))
    }

    async fn execute_with_context(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        _source_scope: &[String],
        conversation_id: Option<&str>,
    ) -> Result<ToolResult, CoreError> {
        let Some(conversation_id) = conversation_id else {
            return Ok(error_result(
                call_id,
                "update_scratchpad can only be used inside a conversation.",
            ));
        };

        let args: UpdateScratchpadArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid update_scratchpad arguments: {e}"))
        })?;

        let action = args.action.to_lowercase();
        match action.as_str() {
            "append" => {
                let content = args.content.unwrap_or_default();
                if content.trim().is_empty() {
                    return Ok(error_result(
                        call_id,
                        "'content' must be non-empty for action 'append'.",
                    ));
                }
                let existing = db
                    .get_agent_scratchpad(conversation_id)?
                    .map(|s| s.content)
                    .unwrap_or_default();
                let stamp = Utc::now().format("%Y-%m-%d %H:%M UTC");
                let new_body = if existing.trim().is_empty() {
                    format!("{stamp}\n{}", content.trim_end())
                } else {
                    format!(
                        "{}\n\n---\n{stamp}\n{}",
                        existing.trim_end(),
                        content.trim_end()
                    )
                };
                db.upsert_agent_scratchpad(conversation_id, &new_body)?;
                echo_state(db, call_id, conversation_id, "append")
            }
            "replace" => {
                let content = args.content.unwrap_or_default();
                if content.is_empty() {
                    return Ok(error_result(
                        call_id,
                        "'content' must be non-empty for action 'replace'. Use 'clear' to empty the scratchpad.",
                    ));
                }
                db.upsert_agent_scratchpad(conversation_id, &content)?;
                echo_state(db, call_id, conversation_id, "replace")
            }
            "clear" => {
                db.clear_agent_scratchpad(conversation_id)?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: "Scratchpad cleared. 0 / 4000 chars used.".to_string(),
                    is_error: false,
                    artifacts: None,
                })
            }
            other => Ok(error_result(
                call_id,
                format!("Unknown action '{other}'. Use 'append', 'replace', or 'clear'."),
            )),
        }
    }
}

/// Read back the scratchpad and build a human-readable confirmation.
fn echo_state(
    db: &Database,
    call_id: &str,
    conversation_id: &str,
    action: &str,
) -> Result<ToolResult, CoreError> {
    let stored = db
        .get_agent_scratchpad(conversation_id)?
        .map(|s| s.content)
        .unwrap_or_default();
    let used = stored.chars().count();
    let byte_len = stored.len();
    let text = format!(
        "Scratchpad updated ({action}). {used} chars / {MAX_SCRATCHPAD_CHARS} cap (bytes: {byte_len})."
    );
    Ok(ToolResult {
        call_id: call_id.to_string(),
        content: text,
        is_error: false,
        artifacts: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn append_writes_and_cap_reports() {
        let db = Database::open_memory().unwrap();
        let tool = UpdateScratchpadTool;
        let args = serde_json::json!({ "action": "append", "content": "first note" });
        let res = tool
            .execute_with_context("c", &args.to_string(), &db, &[], Some("conv-1"))
            .await
            .unwrap();
        assert!(!res.is_error);
        let stored = db.get_agent_scratchpad("conv-1").unwrap().unwrap();
        assert!(stored.content.contains("first note"));
    }

    #[tokio::test]
    async fn replace_overwrites_prior_content() {
        let db = Database::open_memory().unwrap();
        let tool = UpdateScratchpadTool;
        db.upsert_agent_scratchpad("conv-1", "old").unwrap();
        let args = serde_json::json!({ "action": "replace", "content": "brand new" });
        tool.execute_with_context("c", &args.to_string(), &db, &[], Some("conv-1"))
            .await
            .unwrap();
        let stored = db.get_agent_scratchpad("conv-1").unwrap().unwrap();
        assert_eq!(stored.content, "brand new");
    }

    #[tokio::test]
    async fn clear_removes_row() {
        let db = Database::open_memory().unwrap();
        let tool = UpdateScratchpadTool;
        db.upsert_agent_scratchpad("conv-1", "x").unwrap();
        let args = serde_json::json!({ "action": "clear" });
        tool.execute_with_context("c", &args.to_string(), &db, &[], Some("conv-1"))
            .await
            .unwrap();
        assert!(db.get_agent_scratchpad("conv-1").unwrap().is_none());
    }

    #[tokio::test]
    async fn errors_without_conversation_id() {
        let db = Database::open_memory().unwrap();
        let tool = UpdateScratchpadTool;
        let args = serde_json::json!({ "action": "append", "content": "hi" });
        let res = tool
            .execute_with_context("c", &args.to_string(), &db, &[], None)
            .await
            .unwrap();
        assert!(res.is_error);
    }

    #[tokio::test]
    async fn unknown_action_is_error() {
        let db = Database::open_memory().unwrap();
        let tool = UpdateScratchpadTool;
        let args = serde_json::json!({ "action": "zap" });
        let res = tool
            .execute_with_context("c", &args.to_string(), &db, &[], Some("conv-1"))
            .await
            .unwrap();
        assert!(res.is_error);
    }
}
