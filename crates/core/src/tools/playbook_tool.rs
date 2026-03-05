//! PlaybookTool — wraps existing playbook CRUD operations for agent use.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/manage_playbook.json");

/// Tool that creates, lists, gets details of, adds citations to, or deletes playbooks.
pub struct PlaybookTool;

#[derive(Deserialize)]
struct PlaybookArgs {
    action: String,
    title: Option<String>,
    description: Option<String>,
    body_md: Option<String>,
    playbook_id: Option<String>,
    chunk_id: Option<String>,
    annotation: Option<String>,
}

#[async_trait]
impl Tool for PlaybookTool {
    fn name(&self) -> &str {
        "manage_playbook"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: PlaybookArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid manage_playbook arguments: {e}"))
        })?;

        let db = db.clone();
        let call_id = call_id.to_string();
        tokio::task::spawn_blocking(move || {
            match args.action.as_str() {
                "create" => execute_create(&call_id, &args, &db),
                "update" => execute_update(&call_id, &args, &db),
                "add_citation" => execute_add_citation(&call_id, &args, &db),
                "list" => execute_list(&call_id, &db),
                "get" => execute_get(&call_id, &args, &db),
                "delete" => execute_delete(&call_id, &args, &db),
                other => Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("Unknown action: {other}. Valid actions: create, update, add_citation, list, get, delete"),
                    is_error: true,
                    artifacts: None,
                }),
            }
        })
        .await
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?
    }
}

fn execute_create(
    call_id: &str,
    args: &PlaybookArgs,
    db: &Database,
) -> Result<ToolResult, CoreError> {
    let title = args.title.as_deref().unwrap_or("Untitled Playbook");
    let description = args.description.as_deref().unwrap_or("");

    let playbook = db.create_playbook(title, description, "")?;

    let content = format!(
        "Created playbook \"{}\" (id: {})",
        playbook.title, playbook.id
    );
    let artifacts = serde_json::to_value(&playbook).ok();

    Ok(ToolResult {
        call_id: call_id.to_string(),
        content,
        is_error: false,
        artifacts,
    })
}

fn execute_update(
    call_id: &str,
    args: &PlaybookArgs,
    db: &Database,
) -> Result<ToolResult, CoreError> {
    let playbook_id = args
        .playbook_id
        .as_deref()
        .ok_or_else(|| CoreError::InvalidInput("playbook_id is required for update".into()))?;

    let new_title = args.title.as_deref();
    let new_desc = args.body_md.as_deref().or(args.description.as_deref());

    if new_title.is_none() && new_desc.is_none() {
        return Ok(ToolResult {
            call_id: call_id.to_string(),
            content: "Nothing to update — provide at least title, description, or body_md.".into(),
            is_error: true,
            artifacts: None,
        });
    }

    let existing = db.get_playbook(playbook_id)?;
    let title = new_title.unwrap_or(&existing.title);
    let description = new_desc.unwrap_or(&existing.description);

    let playbook = db.update_playbook(playbook_id, title, description)?;

    let content = format!("Updated playbook \"{}\" (id: {})", playbook.title, playbook.id);
    let artifacts = serde_json::to_value(&playbook).ok();

    Ok(ToolResult {
        call_id: call_id.to_string(),
        content,
        is_error: false,
        artifacts,
    })
}

fn execute_add_citation(
    call_id: &str,
    args: &PlaybookArgs,
    db: &Database,
) -> Result<ToolResult, CoreError> {
    let playbook_id = args.playbook_id.as_deref().ok_or_else(|| {
        CoreError::InvalidInput("playbook_id is required for add_citation".into())
    })?;
    let chunk_id = args
        .chunk_id
        .as_deref()
        .ok_or_else(|| CoreError::InvalidInput("chunk_id is required for add_citation".into()))?;
    let annotation = args.annotation.as_deref().unwrap_or("");

    // Determine the next sort_order by checking existing citations.
    let existing = db.list_citations(playbook_id)?;
    let next_order = existing.iter().map(|c| c.order).max().unwrap_or(0) + 1;

    let citation = db.add_citation(playbook_id, chunk_id, annotation, next_order)?;

    let content = format!(
        "Added citation (id: {}) to playbook {}",
        citation.id, playbook_id
    );
    let artifacts = serde_json::to_value(&citation).ok();

    Ok(ToolResult {
        call_id: call_id.to_string(),
        content,
        is_error: false,
        artifacts,
    })
}

fn execute_get(call_id: &str, args: &PlaybookArgs, db: &Database) -> Result<ToolResult, CoreError> {
    let playbook_id = args
        .playbook_id
        .as_deref()
        .ok_or_else(|| CoreError::InvalidInput("playbook_id is required for get".into()))?;

    let playbook = db.get_playbook(playbook_id)?;

    let mut text = format!(
        "Playbook: {} (id: {})\nDescription: {}\nCitations: {}\n",
        playbook.title,
        playbook.id,
        playbook.description,
        playbook.citations.len()
    );
    for c in &playbook.citations {
        text.push_str(&format!(
            "  - chunk {} (order {}): {}\n",
            c.chunk_id, c.order, c.annotation
        ));
    }

    let artifacts = serde_json::to_value(&playbook).ok();

    Ok(ToolResult {
        call_id: call_id.to_string(),
        content: text,
        is_error: false,
        artifacts,
    })
}

fn execute_delete(
    call_id: &str,
    args: &PlaybookArgs,
    db: &Database,
) -> Result<ToolResult, CoreError> {
    let playbook_id = args
        .playbook_id
        .as_deref()
        .ok_or_else(|| CoreError::InvalidInput("playbook_id is required for delete".into()))?;

    db.delete_playbook(playbook_id)?;

    Ok(ToolResult {
        call_id: call_id.to_string(),
        content: format!("Deleted playbook {playbook_id}"),
        is_error: false,
        artifacts: None,
    })
}

fn execute_list(call_id: &str, db: &Database) -> Result<ToolResult, CoreError> {
    let playbooks = db.list_playbooks()?;

    if playbooks.is_empty() {
        return Ok(ToolResult {
            call_id: call_id.to_string(),
            content: "No playbooks found.".into(),
            is_error: false,
            artifacts: None,
        });
    }

    let mut text = format!("Found {} playbook(s):\n\n", playbooks.len());
    for pb in &playbooks {
        text.push_str(&format!(
            "- {} (id: {}, {} citations)\n",
            pb.title,
            pb.id,
            pb.citations.len()
        ));
    }

    let artifacts = serde_json::to_value(&playbooks).ok();

    Ok(ToolResult {
        call_id: call_id.to_string(),
        content: text,
        is_error: false,
        artifacts,
    })
}
