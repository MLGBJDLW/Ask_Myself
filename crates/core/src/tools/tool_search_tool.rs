//! ToolSearchTool - searchable enabled tool catalog.

#[cfg(test)]
use crate::db::Database;

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::error::CoreError;

use super::{
    default_tool_registry, Tool, ToolCategory, ToolDef, ToolExecutionContext, ToolRegistry,
    ToolResult,
};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/tool_search.json");

#[derive(Deserialize)]
struct ToolSearchArgs {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolSearchMatch {
    name: String,
    score: usize,
    description: String,
}

pub struct ToolSearchTool;

#[async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &str {
        "tool_search"
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

    async fn execute(&self, ctx: ToolExecutionContext<'_>) -> Result<ToolResult, CoreError> {
        if let Some(registry) = ctx.tool_registry {
            self.execute_against_registry(ctx.call_id, ctx.arguments, registry)
        } else {
            let registry = default_tool_registry();
            self.execute_against_registry(ctx.call_id, ctx.arguments, &registry)
        }
    }
}

impl ToolSearchTool {
    fn execute_against_registry(
        &self,
        call_id: &str,
        arguments: &str,
        registry: &ToolRegistry,
    ) -> Result<ToolResult, CoreError> {
        let args: ToolSearchArgs = serde_json::from_str(arguments)
            .map_err(|e| CoreError::InvalidInput(format!("Invalid tool_search arguments: {e}")))?;
        let query = args.query.trim();
        if query.is_empty() {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "tool_search query must not be empty.".to_string(),
                is_error: true,
                artifacts: None,
            });
        }

        let limit = args.limit.unwrap_or(8).clamp(1, 20);
        let query_terms = tokenize(query);
        let mut matches = registry
            .definitions()
            .into_iter()
            .filter_map(|tool| {
                let haystack = format!("{} {}", tool.name, tool.description).to_lowercase();
                let score = score_tool(&tool.name, &haystack, &query_terms);
                (score > 0).then(|| ToolSearchMatch {
                    name: tool.name,
                    score,
                    description: truncate_description(&tool.description),
                })
            })
            .collect::<Vec<_>>();

        matches.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.name.cmp(&b.name)));
        matches.truncate(limit);

        let mut content = format!(
            "Found {} enabled tool match(es) for {:?}. Matching hidden tools are activated for the next model step when dynamic visibility is enabled.",
            matches.len(),
            query
        );
        for item in &matches {
            content.push_str(&format!("\n- {}: {}", item.name, item.description));
        }
        if matches.is_empty() {
            content.push_str("\nNo enabled tool matched. Disabled MCP connectors are not discoverable until they are connected.");
        }

        Ok(ToolResult {
            call_id: call_id.to_string(),
            content,
            is_error: false,
            artifacts: Some(json!({
                "kind": "toolSearchResults",
                "query": query,
                "matches": matches,
            })),
        })
    }
}

fn tokenize(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .filter(|term| !term.is_empty())
        .map(str::to_string)
        .collect()
}

fn score_tool(name: &str, haystack: &str, terms: &[String]) -> usize {
    let mut score = 0usize;
    let lower_name = name.to_lowercase();
    for term in terms {
        if lower_name == *term {
            score += 20;
        } else if lower_name.contains(term) {
            score += 12;
        } else if haystack.contains(term) {
            score += 4;
        }
    }
    score
}

fn truncate_description(description: &str) -> String {
    const LIMIT: usize = 180;
    let mut text = description.lines().next().unwrap_or("").trim().to_string();
    if text.chars().count() > LIMIT {
        text = text.chars().take(LIMIT).collect::<String>();
        text.push_str("...");
    }
    text
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;

    struct RuntimeMcpTool;

    #[async_trait]
    impl Tool for RuntimeMcpTool {
        fn name(&self) -> &str {
            "mcp__runtime__browser_snapshot"
        }

        fn description(&self) -> &str {
            "Captures browser snapshot data from a connected runtime MCP connector."
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "selector": { "type": "string" }
                }
            })
        }

        fn categories(&self) -> &'static [ToolCategory] {
            &[ToolCategory::Mcp]
        }

        async fn execute(
            &self,
            context: crate::tools::ToolExecutionContext<'_>,
        ) -> Result<ToolResult, CoreError> {
            let crate::tools::ToolExecutionContext {
                call_id,
                arguments: _arguments,
                db: _db,
                source_scope: _source_scope,
                ..
            } = context;
            Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "ok".to_string(),
                is_error: false,
                artifacts: None,
            })
        }
    }

    #[tokio::test]
    async fn tool_search_finds_file_search_tools() {
        let db = Database::open_memory().unwrap();
        let args = serde_json::json!({ "query": "grep local files" });
        let result = ToolSearchTool
            .execute(crate::tools::ToolExecutionContext::new(
                "tool-search",
                &args.to_string(),
                &db,
                &[],
            ))
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("grep_files") || result.content.contains("search_files"));
    }

    #[tokio::test]
    async fn tool_search_uses_runtime_registry_for_mcp_tools() {
        let db = Database::open_memory().unwrap();
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(RuntimeMcpTool));
        let args = serde_json::json!({ "query": "browser snapshot" });
        let result = ToolSearchTool
            .execute(ToolExecutionContext {
                call_id: "tool-search",
                arguments: &args.to_string(),
                db: &db,
                source_scope: &[],
                activity_runtime: None,
                conversation_id: None,
                turn_id: None,
                tool_registry: Some(&registry),
                cancel_token: None,
                event_tx: None,
            })
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("mcp__runtime__browser_snapshot"));
    }
}
