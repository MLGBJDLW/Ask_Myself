//! ProjectMemoryTool - active-project durable memory management.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;
use crate::project_memory::{
    rank_project_memories_for_query, CreateProjectMemoryInput, ProjectMemory,
    UpdateProjectMemoryInput,
};

use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/manage_project_memory.json");

pub struct ProjectMemoryTool;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectMemoryArgs {
    action: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    pinned: Option<bool>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    confidence: Option<f32>,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    conflict_status: Option<String>,
}

fn missing(field: &str, action: &str) -> CoreError {
    CoreError::InvalidInput(format!(
        "{field} is required for manage_project_memory action '{action}'"
    ))
}

fn active_project_id(db: &Database, conversation_id: Option<&str>) -> Result<String, CoreError> {
    let conversation_id = conversation_id.ok_or_else(|| {
        CoreError::InvalidInput(
            "manage_project_memory requires an active conversation context.".to_string(),
        )
    })?;
    let conversation = db.get_conversation(conversation_id)?;
    conversation.project_id.ok_or_else(|| {
        CoreError::InvalidInput(
            "manage_project_memory requires the current conversation to belong to a Project."
                .to_string(),
        )
    })
}

fn ensure_memory_in_project(
    db: &Database,
    id: &str,
    project_id: &str,
) -> Result<ProjectMemory, CoreError> {
    let memory = db.get_project_memory(id)?;
    if memory.project_id != project_id {
        return Err(CoreError::InvalidInput(
            "Project memory belongs to a different Project.".to_string(),
        ));
    }
    Ok(memory)
}

fn clamp_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(10).clamp(1, 50)
}

fn format_memories(memories: &[ProjectMemory]) -> String {
    if memories.is_empty() {
        return "No project memories found.".to_string();
    }
    memories
        .iter()
        .map(|memory| {
            let label = if memory.title.trim().is_empty() {
                memory.kind.clone()
            } else {
                format!("{}: {}", memory.kind, memory.title)
            };
            format!(
                "- {} [{}{}]: {}",
                memory.id,
                label,
                if memory.pinned { ", pinned" } else { "" },
                memory.content.replace('\n', " ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[async_trait]
impl Tool for ProjectMemoryTool {
    fn name(&self) -> &str {
        "manage_project_memory"
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
            .is_some_and(|action| matches!(action, "archive" | "delete"))
    }

    fn is_read_only(&self, args: &serde_json::Value) -> bool {
        args.get("action")
            .and_then(|v| v.as_str())
            .is_some_and(|action| matches!(action, "search" | "list"))
    }

    fn is_concurrency_safe(&self, args: &serde_json::Value) -> bool {
        self.is_read_only(args)
    }

    fn resource_keys(&self, args: &serde_json::Value) -> Vec<String> {
        match args.get("action").and_then(|v| v.as_str()) {
            Some("search" | "list") => Vec::new(),
            Some("update" | "archive" | "delete") => args
                .get("id")
                .and_then(|v| v.as_str())
                .map(|id| vec![format!("project-memory:{id}")])
                .unwrap_or_else(|| vec!["project-memory".to_string()]),
            Some("record") => vec!["project-memory".to_string()],
            _ => vec!["project-memory".to_string()],
        }
    }

    fn confirmation_message(&self, args: &serde_json::Value) -> Option<String> {
        let action = args.get("action")?.as_str()?;
        if !matches!(action, "archive" | "delete") {
            return None;
        }
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("<missing>");
        Some(format!("{action} project memory {id}."))
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
        let args: ProjectMemoryArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid manage_project_memory arguments: {e}"))
        })?;
        let action = args.action.trim();
        let project_id = active_project_id(db, conversation_id)?;

        match action {
            "record" => {
                let content = args.content.ok_or_else(|| missing("content", action))?;
                let memory = db.create_project_memory(
                    &project_id,
                    &CreateProjectMemoryInput {
                        kind: args.kind,
                        title: args.title,
                        content,
                        pinned: args.pinned,
                        source: Some("agent_tool".to_string()),
                        confidence: args.confidence,
                        expires_at: args.expires_at,
                        conflict_status: args.conflict_status,
                    },
                )?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("Project memory recorded: {}", memory.id),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "projectMemory",
                        "memory": memory
                    })),
                })
            }
            "search" => {
                let query = args.query.ok_or_else(|| missing("query", action))?;
                let memories = db.list_project_memories(&project_id)?;
                let ranked = rank_project_memories_for_query(memories, &query);
                let selected: Vec<ProjectMemory> =
                    ranked.into_iter().take(clamp_limit(args.limit)).collect();
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format_memories(&selected),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "projectMemoryList",
                        "projectId": project_id,
                        "memories": selected
                    })),
                })
            }
            "list" => {
                let memories = db.list_project_memories(&project_id)?;
                let selected: Vec<ProjectMemory> =
                    memories.into_iter().take(clamp_limit(args.limit)).collect();
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format_memories(&selected),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "projectMemoryList",
                        "projectId": project_id,
                        "memories": selected
                    })),
                })
            }
            "update" => {
                let id = args.id.ok_or_else(|| missing("id", action))?;
                ensure_memory_in_project(db, &id, &project_id)?;
                let memory = db.update_project_memory(
                    &id,
                    &UpdateProjectMemoryInput {
                        kind: args.kind,
                        title: args.title,
                        content: args.content,
                        pinned: args.pinned,
                        archived: None,
                        confidence: args.confidence,
                        expires_at: args.expires_at.map(Some),
                        conflict_status: args.conflict_status,
                    },
                )?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("Project memory updated: {}", memory.id),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "projectMemory",
                        "memory": memory
                    })),
                })
            }
            "archive" => {
                let id = args.id.ok_or_else(|| missing("id", action))?;
                ensure_memory_in_project(db, &id, &project_id)?;
                let memory = db.update_project_memory(
                    &id,
                    &UpdateProjectMemoryInput {
                        kind: None,
                        title: None,
                        content: None,
                        pinned: None,
                        archived: Some(true),
                        confidence: None,
                        expires_at: None,
                        conflict_status: None,
                    },
                )?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("Project memory archived: {}", memory.id),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "projectMemory",
                        "memory": memory
                    })),
                })
            }
            "delete" => {
                let id = args.id.ok_or_else(|| missing("id", action))?;
                ensure_memory_in_project(db, &id, &project_id)?;
                db.delete_project_memory(&id)?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("Project memory deleted: {id}"),
                    is_error: false,
                    artifacts: None,
                })
            }
            other => Err(CoreError::InvalidInput(format!(
                "Unknown manage_project_memory action '{other}'"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::CreateConversationInput;
    use crate::project::CreateProjectInput;

    #[tokio::test]
    async fn record_and_search_uses_active_conversation_project() {
        let db = Database::open_memory().unwrap();
        let project = db
            .create_project(&CreateProjectInput {
                name: "Novel".to_string(),
                description: None,
                icon: None,
                color: None,
                system_prompt: None,
                source_scope: None,
            })
            .unwrap();
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "mock".to_string(),
                model: "mock".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: Some(project.id.clone()),
                persona_id: None,
            })
            .unwrap();

        let tool = ProjectMemoryTool;
        let record = serde_json::json!({
            "action": "record",
            "kind": "style",
            "title": "Narration",
            "content": "Use close third-person narration.",
            "pinned": true
        });
        let result = tool
            .execute_with_context(
                "call-1",
                &record.to_string(),
                &db,
                &[],
                Some(&conversation.id),
            )
            .await
            .unwrap();
        assert!(!result.is_error);

        let search = serde_json::json!({
            "action": "search",
            "query": "narration",
            "limit": 5
        });
        let result = tool
            .execute_with_context(
                "call-2",
                &search.to_string(),
                &db,
                &[],
                Some(&conversation.id),
            )
            .await
            .unwrap();
        assert!(result.content.contains("close third-person"));
    }

    #[tokio::test]
    async fn errors_without_active_project() {
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

        let tool = ProjectMemoryTool;
        let args = serde_json::json!({ "action": "list" });
        let err = tool
            .execute_with_context(
                "call-1",
                &args.to_string(),
                &db,
                &[],
                Some(&conversation.id),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("belong to a Project"));
    }
}
