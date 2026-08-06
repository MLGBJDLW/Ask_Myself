//! Structured user-input request rendered as interactive question cards.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::error::CoreError;
use crate::interaction::{
    normalize_questions, CreateInteractionRequest, InteractionKind, InteractionQuestion,
    INTERACTION_PROTOCOL_VERSION,
};

use super::{Tool, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/request_user_input.json");
pub struct RequestUserInputTool;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestArgs {
    questions: Vec<InteractionQuestion>,
    #[serde(default)]
    kind: Option<InteractionKind>,
}

#[async_trait]
impl Tool for RequestUserInputTool {
    fn name(&self) -> &str {
        "request_user_input"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn is_concurrency_safe(&self, _args: &serde_json::Value) -> bool {
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
            turn_id,
            ..
        } = context;
        let args: RequestArgs = serde_json::from_str(arguments).map_err(|error| {
            CoreError::InvalidInput(format!("Invalid request_user_input arguments: {error}"))
        })?;
        let questions = normalize_questions(&args.questions)?;
        let kind = match args.kind.unwrap_or(InteractionKind::UserInput) {
            kind @ (InteractionKind::UserInput | InteractionKind::HighRiskConfirmation) => kind,
            _ => {
                return Err(CoreError::InvalidInput(
                    "request_user_input kind must be user_input or high_risk_confirmation"
                        .to_string(),
                ));
            }
        };
        let conversation_id = conversation_id.ok_or_else(|| {
            CoreError::InvalidInput(
                "request_user_input requires an active conversation".to_string(),
            )
        })?;
        let turn_id = turn_id.ok_or_else(|| {
            CoreError::InvalidInput("request_user_input requires an active turn".to_string())
        })?;
        let title = if questions.len() == 1 {
            questions[0].header.clone()
        } else {
            "Input required".to_string()
        };
        let created = db.create_interaction_request(&CreateInteractionRequest {
            conversation_id: conversation_id.to_string(),
            turn_id: turn_id.to_string(),
            tool_call_id: Some(call_id.to_string()),
            idempotency_key: format!("request_user_input:{call_id}"),
            kind,
            title,
            description: None,
            questions,
            required: true,
            expires_at: None,
        })?;
        let request = created.request;
        if request.status.is_active() {
            db.suspend_agent_turn_for_interaction(&request.interaction_id)?;
        }
        let mut artifact_request = serde_json::to_value(&request)?;
        if let Some(object) = artifact_request.as_object_mut() {
            object.remove("resumeToken");
        }
        let legacy_status = if request.status.is_active() {
            "pending"
        } else {
            "answered"
        };
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content:
                "The questions are displayed to the user. Stop now and wait for their next message."
                    .into(),
            is_error: false,
            artifacts: Some(serde_json::json!({
                "kind": "questionRequest",
                "version": 2,
                "interactionProtocolVersion": INTERACTION_PROTOCOL_VERSION,
                "interactionId": request.interaction_id,
                "callId": call_id,
                "status": legacy_status,
                "questions": request.questions,
                "interactionRequest": artifact_request,
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::{ConversationMessage, CreateConversationInput};
    use crate::db::Database;
    use crate::llm::Role;
    use uuid::Uuid;

    fn runtime_fixture() -> (Database, String, String) {
        let db = Database::open_memory().unwrap();
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-5".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let message = ConversationMessage {
            id: Uuid::new_v4().to_string(),
            conversation_id: conversation.id.clone(),
            role: Role::User,
            content: "Choose a scope".into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 3,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&message).unwrap();
        let turn = db
            .create_conversation_turn(&conversation.id, &message.id, None)
            .unwrap();
        db.create_agent_task_run(
            &conversation.id,
            &turn.id,
            &message.id,
            "Choose a scope",
            Some("openai"),
            Some("gpt-5"),
        )
        .unwrap();
        (db, conversation.id, turn.id)
    }

    #[tokio::test]
    async fn returns_a_typed_question_request_artifact() {
        let (db, conversation_id, turn_id) = runtime_fixture();
        let result = RequestUserInputTool
            .execute(
                crate::tools::ToolExecutionContext::new("call-1", r#"{"questions":[{"id":"scope","header":"Scope","question":"Which scope?","type":"single_choice","options":[{"label":"App (Recommended)","description":"This app only."},{"label":"Repo","description":"The full repository."}]}]}"#, &db, &[])
                    .with_conversation_id(Some(&conversation_id))
                    .with_turn_id(Some(&turn_id)),
            )
            .await
            .unwrap();
        let artifacts = result.artifacts.unwrap();
        assert_eq!(artifacts["kind"], "questionRequest");
        assert_eq!(artifacts["version"], 2);
        assert!(artifacts["interactionId"].as_str().is_some());
        assert!(artifacts.get("resumeToken").is_none());
        assert!(artifacts["interactionRequest"].get("resumeToken").is_none());
        assert_eq!(
            db.list_interaction_requests(Some(&conversation_id), false)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            db.get_agent_task_run_by_turn(&turn_id)
                .unwrap()
                .unwrap()
                .status,
            "awaiting_user_input"
        );
        assert_eq!(
            db.get_conversation_turn(&turn_id).unwrap().status,
            "awaiting_user_input"
        );
    }

    #[tokio::test]
    async fn supports_six_question_high_risk_wizards() {
        let (db, conversation_id, turn_id) = runtime_fixture();
        let arguments = serde_json::json!({
            "kind": "high_risk_confirmation",
            "questions": (1..=6).map(|index| serde_json::json!({
                "id": format!("question_{index}"),
                "header": format!("Question {index}"),
                "question": format!("Confirm item {index}?"),
                "type": "confirm"
            })).collect::<Vec<_>>()
        })
        .to_string();
        let result = RequestUserInputTool
            .execute(
                crate::tools::ToolExecutionContext::new("call-high-risk", &arguments, &db, &[])
                    .with_conversation_id(Some(&conversation_id))
                    .with_turn_id(Some(&turn_id)),
            )
            .await
            .unwrap();

        let artifacts = result.artifacts.unwrap();
        assert_eq!(
            artifacts["interactionRequest"]["kind"],
            "high_risk_confirmation"
        );
        let requests = db
            .list_interaction_requests(Some(&conversation_id), false)
            .unwrap();
        assert_eq!(requests[0].kind, InteractionKind::HighRiskConfirmation);
        assert_eq!(requests[0].questions.len(), 6);
    }
}
