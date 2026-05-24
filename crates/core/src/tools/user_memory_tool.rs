//! UserMemoryTool - cross-session user memory management.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;
use crate::personalization::UserMemory;

use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/manage_user_memory.json");

pub struct UserMemoryTool;

#[derive(Debug, Deserialize)]
struct UserMemoryArgs {
    action: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

fn missing(field: &str, action: &str) -> CoreError {
    CoreError::InvalidInput(format!(
        "{field} is required for manage_user_memory action '{action}'"
    ))
}

fn clamp_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(10).clamp(1, 50)
}

fn extract_terms(query: &str) -> Vec<String> {
    let mut terms: Vec<String> = query
        .to_ascii_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|term| term.chars().count() >= 2)
        .map(ToString::to_string)
        .collect();

    let cjk: Vec<char> = query
        .chars()
        .filter(|ch| matches!(ch, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}'))
        .collect();
    for size in [2usize, 3usize] {
        if cjk.len() >= size {
            for window in cjk.windows(size) {
                terms.push(window.iter().collect());
            }
        }
    }
    terms.sort();
    terms.dedup();
    terms
}

fn score_memory(memory: &UserMemory, query: &str, terms: &[String]) -> i32 {
    let content = memory.content.to_ascii_lowercase();
    let query = query.to_ascii_lowercase();
    let mut score = 0;
    if !query.trim().is_empty() && content.contains(query.trim()) {
        score += 60;
    }
    for term in terms {
        if content.contains(term) {
            score += 18;
        }
    }
    score
}

fn rank_memories(mut memories: Vec<UserMemory>, query: &str) -> Vec<UserMemory> {
    let terms = extract_terms(query);
    memories.sort_by(|a, b| {
        score_memory(b, query, &terms)
            .cmp(&score_memory(a, query, &terms))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| b.created_at.cmp(&a.created_at))
    });
    memories
}

fn format_memories(memories: &[UserMemory]) -> String {
    if memories.is_empty() {
        return "No user memories found.".to_string();
    }
    memories
        .iter()
        .map(|memory| format!("- {}: {}", memory.id, memory.content.replace('\n', " ")))
        .collect::<Vec<_>>()
        .join("\n")
}

#[async_trait]
impl Tool for UserMemoryTool {
    fn name(&self) -> &str {
        "manage_user_memory"
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
            Some("update" | "delete") => args
                .get("id")
                .and_then(|v| v.as_str())
                .map(|id| vec![format!("user-memory:{id}")])
                .unwrap_or_else(|| vec!["user-memory".to_string()]),
            Some("record") => vec!["user-memory".to_string()],
            _ => vec!["user-memory".to_string()],
        }
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
        Some(format!("Delete user memory {id}."))
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: UserMemoryArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid manage_user_memory arguments: {e}"))
        })?;
        let action = args.action.trim();

        match action {
            "record" => {
                let content = args.content.ok_or_else(|| missing("content", action))?;
                let memory = db.create_user_memory(&content)?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("User memory recorded: {}", memory.id),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "userMemory",
                        "memory": memory
                    })),
                })
            }
            "search" => {
                let query = args.query.ok_or_else(|| missing("query", action))?;
                let memories = rank_memories(db.list_user_memories()?, &query);
                let selected: Vec<UserMemory> =
                    memories.into_iter().take(clamp_limit(args.limit)).collect();
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format_memories(&selected),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "userMemoryList",
                        "memories": selected
                    })),
                })
            }
            "list" => {
                let memories: Vec<UserMemory> = db
                    .list_user_memories()?
                    .into_iter()
                    .take(clamp_limit(args.limit))
                    .collect();
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format_memories(&memories),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "userMemoryList",
                        "memories": memories
                    })),
                })
            }
            "update" => {
                let id = args.id.ok_or_else(|| missing("id", action))?;
                let content = args.content.ok_or_else(|| missing("content", action))?;
                let memory = db.update_user_memory(&id, &content)?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("User memory updated: {}", memory.id),
                    is_error: false,
                    artifacts: Some(serde_json::json!({
                        "kind": "userMemory",
                        "memory": memory
                    })),
                })
            }
            "delete" => {
                let id = args.id.ok_or_else(|| missing("id", action))?;
                db.delete_user_memory(&id)?;
                Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("User memory deleted: {id}"),
                    is_error: false,
                    artifacts: None,
                })
            }
            other => Err(CoreError::InvalidInput(format!(
                "Unknown manage_user_memory action '{other}'"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn record_then_search_user_memory() {
        let db = Database::open_memory().unwrap();
        let tool = UserMemoryTool;
        let record = serde_json::json!({
            "action": "record",
            "content": "Default response language is Chinese."
        });
        let result = tool
            .execute("call-1", &record.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(!result.is_error);

        let search = serde_json::json!({
            "action": "search",
            "query": "language preference",
            "limit": 5
        });
        let result = tool
            .execute("call-2", &search.to_string(), &db, &[])
            .await
            .unwrap();
        assert!(result.content.contains("Chinese"));
    }

    #[test]
    fn user_memory_capabilities_distinguish_reads_from_writes() {
        let tool = UserMemoryTool;
        let record = serde_json::json!({ "action": "record" });
        let search = serde_json::json!({ "action": "search" });
        let delete = serde_json::json!({ "action": "delete", "id": "mem-1" });

        let record_caps = tool.run_capabilities(&record);
        assert!(!record_caps.read_only);
        assert_eq!(record_caps.resource_keys, vec!["user-memory"]);

        let search_caps = tool.run_capabilities(&search);
        assert!(search_caps.read_only);
        assert!(search_caps.resource_keys.is_empty());

        let delete_caps = tool.run_capabilities(&delete);
        assert!(delete_caps.destructive);
        assert_eq!(delete_caps.resource_keys, vec!["user-memory:mem-1"]);
    }
}
