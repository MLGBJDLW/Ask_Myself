//! KnowledgeGraphTool — query entity relationships, paths, and knowledge maps.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/query_knowledge_graph.json");

#[derive(Deserialize)]
struct KnowledgeGraphArgs {
    action: String,
    #[serde(default)]
    entity_name: Option<String>,
    #[serde(default)]
    target_name: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    20
}

pub struct KnowledgeGraphTool;

#[async_trait]
impl Tool for KnowledgeGraphTool {
    fn name(&self) -> &str {
        "query_knowledge_graph"
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
        let args: KnowledgeGraphArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid query_knowledge_graph arguments: {e}"))
        })?;

        let db = db.clone();
        let call_id = call_id.to_string();

        tokio::task::spawn_blocking(move || {
            match args.action.as_str() {
                "related" => {
                    let name = args.entity_name.as_deref().unwrap_or("");
                    if name.is_empty() {
                        return Ok(ToolResult {
                            call_id,
                            content: "Error: entity_name is required for 'related' action.".into(),
                            is_error: true,
                            artifacts: None,
                        });
                    }
                    let entity = match db.find_entity_by_name(name) {
                        Ok(e) => e,
                        Err(_) => {
                            return Ok(ToolResult {
                                call_id,
                                content: format!("No entity found with name '{name}'. Try the 'search' action to find similar entities."),
                                is_error: false,
                                artifacts: None,
                            });
                        }
                    };
                    let nodes = db.get_related_entities(&entity.id, 2)?;
                    let mut lines = vec![format!("**Related entities for '{name}'** ({} found):\n", nodes.len())];
                    for node in nodes.iter().take(args.limit) {
                        let link_desc: Vec<String> = node.links.iter().map(|l| {
                            format!("  - {} → {}: {}", l.source_entity_id, l.target_entity_id, l.relation_type)
                        }).collect();
                        lines.push(format!(
                            "- **{}** ({:?}, depth {}): {}\n{}",
                            node.entity.name,
                            node.entity.entity_type,
                            node.depth,
                            node.entity.description,
                            link_desc.join("\n"),
                        ));
                    }
                    Ok(ToolResult {
                        call_id,
                        content: lines.join("\n"),
                        is_error: false,
                        artifacts: None,
                    })
                }
                "path" => {
                    let from_name = args.entity_name.as_deref().unwrap_or("");
                    let to_name = args.target_name.as_deref().unwrap_or("");
                    if from_name.is_empty() || to_name.is_empty() {
                        return Ok(ToolResult {
                            call_id,
                            content: "Error: both entity_name and target_name are required for 'path' action.".into(),
                            is_error: true,
                            artifacts: None,
                        });
                    }
                    let from = match db.find_entity_by_name(from_name) {
                        Ok(e) => e,
                        Err(_) => {
                            return Ok(ToolResult {
                                call_id,
                                content: format!("Entity '{from_name}' not found."),
                                is_error: false,
                                artifacts: None,
                            });
                        }
                    };
                    let to = match db.find_entity_by_name(to_name) {
                        Ok(e) => e,
                        Err(_) => {
                            return Ok(ToolResult {
                                call_id,
                                content: format!("Entity '{to_name}' not found."),
                                is_error: false,
                                artifacts: None,
                            });
                        }
                    };
                    match db.find_entity_path(&from.id, &to.id)? {
                        Some(path) => {
                            let names: Vec<String> = path.iter().map(|e| e.name.clone()).collect();
                            Ok(ToolResult {
                                call_id,
                                content: format!(
                                    "**Path from '{}' to '{}'** ({} steps):\n\n{}",
                                    from_name, to_name, path.len() - 1,
                                    names.join(" → "),
                                ),
                                is_error: false,
                                artifacts: None,
                            })
                        }
                        None => Ok(ToolResult {
                            call_id,
                            content: format!("No path found between '{from_name}' and '{to_name}'."),
                            is_error: false,
                            artifacts: None,
                        }),
                    }
                }
                "map" => {
                    let map = db.get_knowledge_map(args.limit)?;
                    let mut lines = vec![format!(
                        "**Knowledge Map** ({} entities, {} links):\n",
                        map.total_entities, map.total_links,
                    )];
                    for entity in &map.entities {
                        lines.push(format!(
                            "- **{}** ({:?}): {} (mentions: {})",
                            entity.name, entity.entity_type, entity.description, entity.mention_count,
                        ));
                    }
                    Ok(ToolResult {
                        call_id,
                        content: lines.join("\n"),
                        is_error: false,
                        artifacts: None,
                    })
                }
                "search" => {
                    let query = args.entity_name.as_deref().unwrap_or("");
                    if query.is_empty() {
                        return Ok(ToolResult {
                            call_id,
                            content: "Error: entity_name is required as the search query.".into(),
                            is_error: true,
                            artifacts: None,
                        });
                    }
                    let entities = db.search_entities(query)?;
                    if entities.is_empty() {
                        Ok(ToolResult {
                            call_id,
                            content: format!("No entities found matching '{query}'."),
                            is_error: false,
                            artifacts: None,
                        })
                    } else {
                        let mut lines = vec![format!("**Entity search results for '{query}'** ({} found):\n", entities.len())];
                        for e in entities.iter().take(args.limit) {
                            lines.push(format!(
                                "- **{}** ({:?}): {} (mentions: {})",
                                e.name, e.entity_type, e.description, e.mention_count,
                            ));
                        }
                        Ok(ToolResult {
                            call_id,
                            content: lines.join("\n"),
                            is_error: false,
                            artifacts: None,
                        })
                    }
                }
                other => Ok(ToolResult {
                    call_id,
                    content: format!("Unknown action '{other}'. Valid actions: related, path, map, search."),
                    is_error: true,
                    artifacts: None,
                }),
            }
        })
        .await
        .map_err(|e| CoreError::Internal(format!("Task join error: {e}")))?
    }
}
