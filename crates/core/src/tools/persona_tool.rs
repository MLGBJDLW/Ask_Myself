//! PersonaTool - active conversation persona switching.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;
use crate::persona::{enabled_persona_by_id, list_personas, PersonaProfile};

use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/manage_persona.json");

pub struct PersonaTool;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersonaArgs {
    action: String,
    #[serde(default)]
    persona_id: Option<String>,
}

fn missing(field: &str, action: &str) -> CoreError {
    CoreError::InvalidInput(format!(
        "{field} is required for manage_persona action '{action}'"
    ))
}

fn format_persona(persona: &PersonaProfile) -> String {
    let source = if persona.builtin { "builtin" } else { "user" };
    let skills = if persona.default_skill_ids.is_empty() {
        "none".to_string()
    } else {
        persona.default_skill_ids.join(", ")
    };
    format!(
        "- {} ({source}): {} | default skills: {}",
        persona.id, persona.description, skills
    )
}

fn active_conversation_id(conversation_id: Option<&str>) -> Result<&str, CoreError> {
    conversation_id.ok_or_else(|| {
        CoreError::InvalidInput(
            "manage_persona requires an active conversation context.".to_string(),
        )
    })
}

#[async_trait]
impl Tool for PersonaTool {
    fn name(&self) -> &str {
        "manage_persona"
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

    fn is_read_only(&self, args: &serde_json::Value) -> bool {
        args.get("action")
            .and_then(|v| v.as_str())
            .is_some_and(|action| matches!(action, "list" | "current"))
    }

    fn is_concurrency_safe(&self, args: &serde_json::Value) -> bool {
        self.is_read_only(args)
    }

    fn resource_keys(&self, args: &serde_json::Value) -> Vec<String> {
        match args.get("action").and_then(|v| v.as_str()) {
            Some("list" | "current") => Vec::new(),
            Some("switch") => vec!["conversation:persona".to_string()],
            _ => vec!["conversation:persona".to_string()],
        }
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        self.execute_with_context(call_id, arguments, db, source_scope, None)
            .await
    }

    async fn execute_with_context(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        _source_scope: &[String],
        conversation_id: Option<&str>,
    ) -> Result<ToolResult, CoreError> {
        let args: PersonaArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid manage_persona arguments: {e}"))
        })?;
        let action = args.action.trim();

        match action {
            "list" => {
                let personas = list_personas(db)?;
                let enabled: Vec<PersonaProfile> = personas
                    .into_iter()
                    .filter(|persona| persona.enabled)
                    .collect();
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: enabled
                        .iter()
                        .map(format_persona)
                        .collect::<Vec<_>>()
                        .join("\n"),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "personaList",
                        "personas": enabled
                    })),
                })
            }
            "current" => {
                let conversation_id = active_conversation_id(conversation_id)?;
                let conversation = db.get_conversation(conversation_id)?;
                let persona_id = conversation.persona_id.as_deref().unwrap_or("default");
                let persona = enabled_persona_by_id(db, persona_id)?.ok_or_else(|| {
                    CoreError::InvalidInput(format!("Persona '{persona_id}' is not enabled"))
                })?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("Current conversation persona: {}", format_persona(&persona)),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "persona",
                        "persona": persona
                    })),
                })
            }
            "switch" => {
                let conversation_id = active_conversation_id(conversation_id)?;
                let persona_id = args
                    .persona_id
                    .ok_or_else(|| missing("personaId", action))?;
                let persona = enabled_persona_by_id(db, &persona_id)?.ok_or_else(|| {
                    CoreError::InvalidInput(format!("Persona '{persona_id}' is not enabled"))
                })?;
                db.update_conversation_persona(
                    conversation_id,
                    if persona.id == "default" {
                        None
                    } else {
                        Some(persona.id.as_str())
                    },
                )?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!(
                        "Conversation persona switched to {}. The new persona will apply to the next model turn.",
                        persona.id
                    ),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "personaSwitch",
                        "persona": persona,
                        "applies": "next_turn"
                    })),
                })
            }
            other => Err(CoreError::InvalidInput(format!(
                "Unknown manage_persona action '{other}'"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::CreateConversationInput;

    #[tokio::test]
    async fn switches_conversation_persona_for_future_turns() {
        let db = Database::open_memory().unwrap();
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "mock".to_string(),
                model: "mock".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let tool = PersonaTool;
        let args = serde_json::json!({
            "action": "switch",
            "personaId": "novelist"
        });

        let result = tool
            .execute_with_context(
                "call-1",
                &args.to_string(),
                &db,
                &[],
                Some(&conversation.id),
            )
            .await
            .unwrap();
        assert!(!result.is_error);

        let updated = db.get_conversation(&conversation.id).unwrap();
        assert_eq!(updated.persona_id.as_deref(), Some("novelist"));
    }

    #[tokio::test]
    async fn errors_when_switching_without_conversation_context() {
        let db = Database::open_memory().unwrap();
        let tool = PersonaTool;
        let args = serde_json::json!({
            "action": "switch",
            "personaId": "novelist"
        });

        let err = tool
            .execute_with_context("call-1", &args.to_string(), &db, &[], None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("active conversation context"));
    }
}
