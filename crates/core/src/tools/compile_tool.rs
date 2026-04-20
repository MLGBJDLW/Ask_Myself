//! CompileTool — check document compilation status or list uncompiled documents.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/compile_document.json");

#[derive(Deserialize)]
struct CompileArgs {
    document_id: Option<String>,
    #[serde(default)]
    compile_all: bool,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    10
}

pub struct CompileTool;

#[async_trait]
impl Tool for CompileTool {
    fn name(&self) -> &str {
        "compile_document"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::Knowledge]
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: CompileArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid compile_document arguments: {e}"))
        })?;

        let db = db.clone();
        let call_id = call_id.to_string();

        tokio::task::spawn_blocking(move || {
            if let Some(ref doc_id) = args.document_id {
                // Return compilation status for a specific document
                let summary = db.get_document_summary(doc_id)?;
                let entities = db.get_entities_for_document(doc_id)?;
                let stats = db.get_compile_stats()?;

                let status = if let Some(ref s) = summary {
                    format!(
                        "**Document {} — Compiled**\n\n**Summary:** {}\n\n**Key Points:**\n{}\n\n**Tags:** {}\n\n**Entities found:** {}\n\n**Model:** {}\n**Compiled at:** {}\n\n**KB Stats:** {}/{} docs compiled, {} entities, {} links",
                        doc_id,
                        s.summary,
                        s.key_points.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n"),
                        s.tags.join(", "),
                        entities.len(),
                        s.model_used,
                        s.compiled_at,
                        stats.compiled_docs, stats.total_docs, stats.total_entities, stats.total_links,
                    )
                } else {
                    format!(
                        "**Document {} — Not yet compiled**\n\nThis document has not been processed by the knowledge compiler. It needs to be compiled to extract summaries, entities, and relationships.\n\n**KB Stats:** {}/{} docs compiled, {} entities, {} links",
                        doc_id,
                        stats.compiled_docs, stats.total_docs, stats.total_entities, stats.total_links,
                    )
                };

                Ok(ToolResult {
                    call_id,
                    content: status,
                    is_error: false,
                    artifacts: None,
                })
            } else if args.compile_all {
                // List uncompiled documents
                let pending = db.get_uncompiled_document_ids(args.limit)?;
                let stats = db.get_compile_stats()?;

                if pending.is_empty() {
                    Ok(ToolResult {
                        call_id,
                        content: format!(
                            "All documents are compiled! Stats: {}/{} docs, {} entities, {} links.",
                            stats.compiled_docs, stats.total_docs, stats.total_entities, stats.total_links,
                        ),
                        is_error: false,
                        artifacts: None,
                    })
                } else {
                    let ids_str = pending
                        .iter()
                        .map(|id| id.to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    Ok(ToolResult {
                        call_id,
                        content: format!(
                            "**{} documents pending compilation** (showing up to {}):\n\nDocument IDs: {}\n\n**KB Stats:** {}/{} docs compiled, {} entities, {} links.\n\nTo compile, trigger compilation at the application level or examine individual documents.",
                            pending.len(), args.limit, ids_str,
                            stats.compiled_docs, stats.total_docs, stats.total_entities, stats.total_links,
                        ),
                        is_error: false,
                        artifacts: None,
                    })
                }
            } else {
                // No arguments — return general stats
                let stats = db.get_compile_stats()?;
                Ok(ToolResult {
                    call_id,
                    content: format!(
                        "**Knowledge Compilation Stats:**\n- Total documents: {}\n- Compiled documents: {}\n- Total entities: {}\n- Total links: {}\n\nUse `document_id` to check a specific document, or `compile_all: true` to list pending documents.",
                        stats.total_docs, stats.compiled_docs, stats.total_entities, stats.total_links,
                    ),
                    is_error: false,
                    artifacts: None,
                })
            }
        })
        .await
        .map_err(|e| CoreError::Internal(format!("Task join error: {e}")))?
    }
}
