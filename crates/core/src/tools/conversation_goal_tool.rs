//! Tools for inspecting and explicitly closing a durable conversation goal.

#[cfg(test)]
use crate::db::Database;

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::conversation::ConversationGoalStatus;
use crate::error::CoreError;

use super::{Tool, ToolDef, ToolResult};

static GET_DEF: OnceLock<ToolDef> = OnceLock::new();
static UPDATE_DEF: OnceLock<ToolDef> = OnceLock::new();
const GET_DEF_JSON: &str = include_str!("../../prompts/tools/get_goal.json");
const UPDATE_DEF_JSON: &str = include_str!("../../prompts/tools/update_goal.json");

pub struct GetGoalTool;
pub struct UpdateGoalTool;

#[derive(Debug, Deserialize)]
struct UpdateGoalArgs {
    status: ConversationGoalStatus,
    #[serde(default)]
    objective: Option<String>,
}

fn missing_conversation_id() -> CoreError {
    CoreError::InvalidInput("Goal tools require an active conversation".into())
}

#[async_trait]
impl Tool for GetGoalTool {
    fn name(&self) -> &str {
        "get_goal"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&GET_DEF, GET_DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&GET_DEF, GET_DEF_JSON)
            .parameters
            .clone()
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments: _arguments,
            db,
            source_scope: _source_scope,
            conversation_id,
            ..
        } = context;
        let conversation_id = conversation_id.ok_or_else(missing_conversation_id)?;
        let goal = db.get_conversation_goal(conversation_id)?;
        let artifacts = match &goal {
            Some(goal) => serde_json::to_value(goal)?,
            None => serde_json::json!({
                "kind": "goalState",
                "status": "none",
                "conversationId": conversation_id,
            }),
        };
        let content = match goal {
            Some(goal) => format!("Goal {}: {}", goal.status.as_str(), goal.objective),
            None => "This conversation has no goal.".to_string(),
        };

        Ok(ToolResult {
            call_id: call_id.to_string(),
            content,
            is_error: false,
            artifacts: Some(serde_json::json!({
                "kind": "goalState",
                "goal": artifacts,
            })),
        })
    }
}

#[async_trait]
impl Tool for UpdateGoalTool {
    fn name(&self) -> &str {
        "update_goal"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&UPDATE_DEF, UPDATE_DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&UPDATE_DEF, UPDATE_DEF_JSON)
            .parameters
            .clone()
    }

    fn is_read_only(&self, _args: &serde_json::Value) -> bool {
        false
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments,
            db,
            source_scope: _source_scope,
            conversation_id,
            ..
        } = context;
        let conversation_id = conversation_id.ok_or_else(missing_conversation_id)?;
        let args: UpdateGoalArgs = serde_json::from_str(arguments).map_err(|error| {
            CoreError::InvalidInput(format!("Invalid update_goal arguments: {error}"))
        })?;
        let objective = args.objective.as_deref().map(str::trim);
        if matches!(objective, Some("")) {
            return Err(CoreError::InvalidInput(
                "update_goal objective cannot be empty when supplied".into(),
            ));
        }
        let goal = db.update_conversation_goal(conversation_id, args.status, objective)?;
        let artifact = serde_json::json!({
            "kind": "goal",
            "id": goal.id,
            "conversationId": goal.conversation_id,
            "objective": goal.objective,
            "status": goal.status.as_str(),
            "createdAt": goal.created_at,
            "updatedAt": goal.updated_at,
            "completedAt": goal.completed_at,
        });

        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: format!("Goal marked {}: {}", goal.status.as_str(), goal.objective),
            is_error: false,
            artifacts: Some(artifact),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::CreateConversationInput;

    #[tokio::test]
    async fn update_goal_requires_explicit_terminal_state() {
        let db = Database::open_memory().unwrap();
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        db.set_conversation_goal(&conversation.id, "Finish the work")
            .unwrap();

        let result = UpdateGoalTool
            .execute(
                crate::tools::ToolExecutionContext::new(
                    "call-1",
                    r#"{"status":"complete"}"#,
                    &db,
                    &[],
                )
                .with_conversation_id(Some(&conversation.id)),
            )
            .await
            .unwrap();
        assert_eq!(result.artifacts.unwrap()["status"], "complete");
        assert_eq!(
            db.get_conversation_goal(&conversation.id)
                .unwrap()
                .unwrap()
                .status,
            ConversationGoalStatus::Complete
        );
    }
}
