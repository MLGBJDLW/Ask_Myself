//! RelatedConceptsTool — wiki index, MOC, hot concepts, suggestions, trends, and gaps.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/get_related_concepts.json");

#[derive(Deserialize)]
struct RelatedConceptsArgs {
    action: String,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default = "default_days")]
    days: u32,
}

fn default_limit() -> usize {
    10
}

fn default_days() -> u32 {
    30
}

pub struct RelatedConceptsTool;

#[async_trait]
impl Tool for RelatedConceptsTool {
    fn name(&self) -> &str {
        "get_related_concepts"
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
        let args: RelatedConceptsArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid get_related_concepts arguments: {e}"))
        })?;

        let db = db.clone();
        let call_id = call_id.to_string();

        tokio::task::spawn_blocking(move || {
            match args.action.as_str() {
                "wiki_index" => {
                    let index = db.generate_wiki_index()?;
                    let mut lines = vec![format!(
                        "**Wiki Index** — {} entities across {} documents ({} compiled)\n",
                        index.total_entities, index.total_documents, index.compiled_documents,
                    )];
                    for (type_name, entries) in &index.by_type {
                        lines.push(format!("### {type_name} ({})", entries.len()));
                        for entry in entries.iter().take(args.limit) {
                            lines.push(format!(
                                "- **{}**: {} (docs: {}, links: {})",
                                entry.entity.name,
                                entry.entity.description,
                                entry.document_count,
                                entry.link_count,
                            ));
                        }
                        lines.push(String::new());
                    }
                    Ok(ToolResult {
                        call_id,
                        content: lines.join("\n"),
                        is_error: false,
                        artifacts: None,
                    })
                }
                "moc" => {
                    let topic = args.topic.as_deref().unwrap_or("");
                    if topic.is_empty() {
                        return Ok(ToolResult {
                            call_id,
                            content: "Error: 'topic' is required for 'moc' action.".into(),
                            is_error: true,
                            artifacts: None,
                        });
                    }
                    match db.generate_moc(topic) {
                        Ok(moc) => {
                            let mut lines = vec![format!("**Map of Content: {}**\n", moc.topic)];
                            if !moc.related_entities.is_empty() {
                                lines.push("**Related Entities:**".into());
                                for e in &moc.related_entities {
                                    lines.push(format!("- {} ({:?})", e.name, e.entity_type));
                                }
                                lines.push(String::new());
                            }
                            if !moc.documents.is_empty() {
                                lines.push("**Documents:**".into());
                                for d in &moc.documents {
                                    let summary_str = d
                                        .summary
                                        .as_deref()
                                        .map(|s| format!(" — {s}"))
                                        .unwrap_or_default();
                                    lines.push(format!(
                                        "- [{}] {}{} (relevance: {:.2})",
                                        d.document_id, d.title, summary_str, d.relevance,
                                    ));
                                }
                                lines.push(String::new());
                            }
                            if !moc.sub_topics.is_empty() {
                                lines.push(format!(
                                    "**Sub-topics:** {}",
                                    moc.sub_topics.join(", "),
                                ));
                            }
                            Ok(ToolResult {
                                call_id,
                                content: lines.join("\n"),
                                is_error: false,
                                artifacts: None,
                            })
                        }
                        Err(_) => Ok(ToolResult {
                            call_id,
                            content: format!("Topic '{topic}' not found as a known entity. Try 'wiki_index' to see available entities."),
                            is_error: false,
                            artifacts: None,
                        }),
                    }
                }
                "hot" => {
                    let concepts = db.get_hot_concepts(args.limit)?;
                    if concepts.is_empty() {
                        Ok(ToolResult {
                            call_id,
                            content: "No hot concepts found. The knowledge base may not have enough compiled data yet.".into(),
                            is_error: false,
                            artifacts: None,
                        })
                    } else {
                        let mut lines = vec![format!("**Hot Concepts** ({} found):\n", concepts.len())];
                        for c in &concepts {
                            lines.push(format!(
                                "- **{}** ({:?}): score {:.1}, {} recent queries",
                                c.entity.name, c.entity.entity_type, c.score, c.recent_queries,
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
                "suggestions" => {
                    let suggestions = db.suggest_explorations(args.limit)?;
                    if suggestions.is_empty() {
                        Ok(ToolResult {
                            call_id,
                            content: "No exploration suggestions available yet. Build up the knowledge graph by compiling more documents.".into(),
                            is_error: false,
                            artifacts: None,
                        })
                    } else {
                        let mut lines = vec![format!("**Exploration Suggestions** ({}):\n", suggestions.len())];
                        for (i, s) in suggestions.iter().enumerate() {
                            lines.push(format!("{}. {s}", i + 1));
                        }
                        Ok(ToolResult {
                            call_id,
                            content: lines.join("\n"),
                            is_error: false,
                            artifacts: None,
                        })
                    }
                }
                "trends" => {
                    let trends = db.get_query_trends(args.days)?;
                    if trends.is_empty() {
                        Ok(ToolResult {
                            call_id,
                            content: format!("No query trends found in the last {} days.", args.days),
                            is_error: false,
                            artifacts: None,
                        })
                    } else {
                        let mut lines = vec![format!("**Query Trends** (last {} days, {} topics):\n", args.days, trends.len())];
                        for t in &trends {
                            lines.push(format!(
                                "- **{}**: {} queries (first: {}, last: {})",
                                t.topic, t.count, t.first_queried, t.last_queried,
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
                "gaps" => {
                    let gaps = db.get_knowledge_gaps(2)?;
                    if gaps.is_empty() {
                        Ok(ToolResult {
                            call_id,
                            content: "No knowledge gaps detected. Your knowledge base covers queried topics well.".into(),
                            is_error: false,
                            artifacts: None,
                        })
                    } else {
                        let mut lines = vec![format!("**Knowledge Gaps** ({} found):\n", gaps.len())];
                        for g in &gaps {
                            lines.push(format!(
                                "- **{}**: queried {} times, avg confidence {:.2}\n  → {}",
                                g.topic, g.query_count, g.avg_confidence, g.suggestion,
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
                    content: format!(
                        "Unknown action '{other}'. Valid actions: wiki_index, moc, hot, suggestions, trends, gaps."
                    ),
                    is_error: true,
                    artifacts: None,
                }),
            }
        })
        .await
        .map_err(|e| CoreError::Internal(format!("Task join error: {e}")))?
    }
}
