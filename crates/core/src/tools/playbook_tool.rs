//! PlaybookTool — wraps existing playbook CRUD operations for agent use.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolResult};

/// Tool that creates, lists, gets details of, adds citations to, or deletes playbooks.
pub struct PlaybookTool;

#[derive(Deserialize)]
struct PlaybookArgs {
    action: String,
    title: Option<String>,
    description: Option<String>,
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
        "Create, list, get details of, add citations to, or delete a playbook. \
         A playbook is a composable collection of evidence cards with annotations."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "add_citation", "list", "get", "delete"],
                    "description": "The operation to perform"
                },
                "title": {
                    "type": "string",
                    "description": "Title for creating a playbook"
                },
                "description": {
                    "type": "string",
                    "description": "Description for creating a playbook"
                },
                "playbook_id": {
                    "type": "string",
                    "description": "Playbook ID for get, delete, or add_citation"
                },
                "chunk_id": {
                    "type": "string",
                    "description": "Chunk ID to cite"
                },
                "annotation": {
                    "type": "string",
                    "description": "Annotation text for the citation"
                }
            },
            "required": ["action"]
        })
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

        match args.action.as_str() {
            "create" => execute_create(call_id, &args, db),
            "add_citation" => execute_add_citation(call_id, &args, db),
            "list" => execute_list(call_id, db),
            "get" => execute_get(call_id, &args, db),
            "delete" => execute_delete(call_id, &args, db),
            other => Ok(ToolResult {
                call_id: call_id.to_string(),
                content: format!("Unknown action: {other}. Valid actions: create, add_citation, list, get, delete"),
                is_error: true,
                artifacts: None,
            }),
        }
    }
}

fn execute_create(
    call_id: &str,
    args: &PlaybookArgs,
    db: &Database,
) -> Result<ToolResult, CoreError> {
    let title = args
        .title
        .as_deref()
        .unwrap_or("Untitled Playbook");
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

fn execute_add_citation(
    call_id: &str,
    args: &PlaybookArgs,
    db: &Database,
) -> Result<ToolResult, CoreError> {
    let playbook_id = args.playbook_id.as_deref().ok_or_else(|| {
        CoreError::InvalidInput("playbook_id is required for add_citation".into())
    })?;
    let chunk_id = args.chunk_id.as_deref().ok_or_else(|| {
        CoreError::InvalidInput("chunk_id is required for add_citation".into())
    })?;
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

fn execute_get(
    call_id: &str,
    args: &PlaybookArgs,
    db: &Database,
) -> Result<ToolResult, CoreError> {
    let playbook_id = args.playbook_id.as_deref().ok_or_else(|| {
        CoreError::InvalidInput("playbook_id is required for get".into())
    })?;

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
            c.chunk_id,
            c.order,
            c.annotation
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
    let playbook_id = args.playbook_id.as_deref().ok_or_else(|| {
        CoreError::InvalidInput("playbook_id is required for delete".into())
    })?;

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
