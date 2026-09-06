//! Deferred tool discovery and activation for dynamic tool visibility.

use std::collections::HashSet;

use crate::llm::ToolDefinition;
use crate::tools::ToolRegistry;

use super::{context, merge_tool_definitions};

pub(super) fn dynamic_tool_visibility_prompt() -> &'static str {
    "## Dynamic Tool Discovery\nSome enabled tools may be hidden from the current model step to keep context small. `tool_search` is the resident discovery tool. If the task needs a capability and the exact tool is not visible, call `tool_search` with the capability or tool-name fragment before using a nearby substitute. After `tool_search` returns, use the activated matching tool on the next model step. Do not claim an enabled capability is unavailable until `tool_search` fails or returns no match."
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ToolSearchActivation {
    pub(super) activated: Vec<String>,
    pub(super) already_available: Vec<String>,
    pub(super) unknown: Vec<String>,
    pub(super) capacity_limited: Vec<String>,
    pub(super) evicted: Vec<String>,
}

impl ToolSearchActivation {
    pub(super) fn has_changes(&self) -> bool {
        !self.activated.is_empty()
    }
}

pub(super) fn activate_tool_search_matches(
    registry: &ToolRegistry,
    tool_defs: &mut Vec<ToolDefinition>,
    artifacts: Option<&serde_json::Value>,
) -> ToolSearchActivation {
    activate_tool_search_matches_bounded(
        registry,
        tool_defs,
        artifacts,
        "gpt-4o",
        usize::MAX,
        u32::MAX,
    )
}

pub(super) fn activate_tool_search_matches_bounded(
    registry: &ToolRegistry,
    tool_defs: &mut Vec<ToolDefinition>,
    artifacts: Option<&serde_json::Value>,
    model: &str,
    max_definitions: usize,
    max_tool_tokens: u32,
) -> ToolSearchActivation {
    let Some(payload) = tool_search_payload(artifacts) else {
        return ToolSearchActivation::default();
    };
    let Some(matches) = payload.get("matches").and_then(|value| value.as_array()) else {
        return ToolSearchActivation::default();
    };

    let mut activation = ToolSearchActivation::default();
    let mut available: HashSet<String> = tool_defs.iter().map(|tool| tool.name.clone()).collect();
    let mut seen = HashSet::new();
    let mut newly_selected = Vec::new();
    let mut selected_count = tool_defs.len();
    let mut selected_tokens = context::estimate_tool_tokens_for_model(model, tool_defs);
    let requested: HashSet<&str> = matches
        .iter()
        .filter_map(|item| item.get("name").and_then(|name| name.as_str()))
        .collect();

    for item in matches {
        let Some(name) = item.get("name").and_then(|value| value.as_str()) else {
            continue;
        };
        if !seen.insert(name.to_string()) {
            continue;
        }
        if available.contains(name) {
            activation.already_available.push(name.to_string());
            continue;
        }
        if let Some(tool) = registry.get(name) {
            let definition = tool.definition();
            let definition_tokens =
                context::estimate_tool_tokens_for_model(model, std::slice::from_ref(&definition));
            if selected_count >= max_definitions
                || selected_tokens.saturating_add(definition_tokens) > max_tool_tokens
            {
                // Rotate stale deferred definitions out of a full surface.
                // Resident tools and every match in this result stay pinned.
                let mut retained = tool_defs.clone();
                let mut count = selected_count;
                let mut tokens = selected_tokens;
                let mut evicted = Vec::new();
                while count >= max_definitions
                    || tokens.saturating_add(definition_tokens) > max_tool_tokens
                {
                    let candidate = retained.iter().rposition(|definition| {
                        definition.name != "tool_search"
                            && !requested.contains(definition.name.as_str())
                            && registry.get(&definition.name).is_some_and(|tool| {
                                !tool
                                    .categories()
                                    .contains(&crate::tools::ToolCategory::Core)
                            })
                    });
                    let Some(index) = candidate else {
                        break;
                    };
                    let removed = retained.remove(index);
                    count -= 1;
                    tokens = tokens.saturating_sub(context::estimate_tool_tokens_for_model(
                        model,
                        std::slice::from_ref(&removed),
                    ));
                    evicted.push(removed.name);
                }
                if count >= max_definitions
                    || tokens.saturating_add(definition_tokens) > max_tool_tokens
                {
                    activation.capacity_limited.push(name.to_string());
                    continue;
                }
                *tool_defs = retained;
                selected_count = count;
                selected_tokens = tokens;
                for name in &evicted {
                    available.remove(name);
                }
                activation.evicted.extend(evicted);
            }
            selected_count = selected_count.saturating_add(1);
            selected_tokens = selected_tokens.saturating_add(definition_tokens);
            newly_selected.push(definition);
            available.insert(name.to_string());
            activation.activated.push(name.to_string());
        } else {
            activation.unknown.push(name.to_string());
        }
    }

    if !newly_selected.is_empty() {
        *tool_defs = merge_tool_definitions(std::mem::take(tool_defs), newly_selected);
    }

    activation
}

fn tool_search_payload(artifacts: Option<&serde_json::Value>) -> Option<&serde_json::Value> {
    let value = artifacts?;
    if value.get("kind").and_then(|kind| kind.as_str()) == Some("toolSearchResults") {
        return Some(value);
    }
    let nested = value.get("artifacts")?;
    if nested.get("kind").and_then(|kind| kind.as_str()) == Some("toolSearchResults") {
        return Some(nested);
    }
    None
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use crate::error::CoreError;
    use crate::tools::{Tool, ToolCategory, ToolResult};

    use super::*;

    struct DeferredFakeTool;

    #[async_trait]
    impl Tool for DeferredFakeTool {
        fn name(&self) -> &str {
            "mcp__fake_server__search_docs"
        }

        fn description(&self) -> &str {
            "Searches fake MCP documents for test coverage."
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
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

    #[test]
    fn dynamic_tool_visibility_prompt_points_to_resident_search() {
        let prompt = dynamic_tool_visibility_prompt();

        assert!(prompt.contains("tool_search"));
        assert!(prompt.contains("resident discovery tool"));
        assert!(prompt.contains("before using a nearby substitute"));
    }

    #[test]
    fn tool_search_results_activate_hidden_runtime_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DeferredFakeTool));
        let mut tool_defs = Vec::new();
        let artifacts = serde_json::json!({
            "kind": "toolSearchResults",
            "matches": [
                { "name": "mcp__fake_server__search_docs", "score": 12 },
                { "name": "missing_tool", "score": 4 }
            ]
        });

        let activation = activate_tool_search_matches(&registry, &mut tool_defs, Some(&artifacts));

        assert_eq!(
            activation.activated,
            vec!["mcp__fake_server__search_docs".to_string()]
        );
        assert_eq!(activation.unknown, vec!["missing_tool".to_string()]);
        assert_eq!(tool_defs.len(), 1);
        assert_eq!(tool_defs[0].name, "mcp__fake_server__search_docs");
    }

    #[test]
    fn tool_search_results_do_not_duplicate_already_visible_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DeferredFakeTool));
        let mut tool_defs = registry.definitions();
        let artifacts = serde_json::json!({
            "kind": "toolSearchResults",
            "matches": [
                { "name": "mcp__fake_server__search_docs", "score": 12 }
            ]
        });

        let activation = activate_tool_search_matches(&registry, &mut tool_defs, Some(&artifacts));

        assert!(activation.activated.is_empty());
        assert_eq!(
            activation.already_available,
            vec!["mcp__fake_server__search_docs".to_string()]
        );
        assert_eq!(tool_defs.len(), 1);
    }

    #[test]
    fn a_full_surface_rotates_deferred_tools_but_keeps_resident_search() {
        struct OldTool;
        #[async_trait]
        impl Tool for OldTool {
            fn name(&self) -> &str {
                "old_deferred_tool"
            }
            fn description(&self) -> &str {
                "Older deferred capability"
            }
            fn parameters_schema(&self) -> serde_json::Value {
                serde_json::json!({"type":"object"})
            }
            fn categories(&self) -> &'static [ToolCategory] {
                &[ToolCategory::Mcp]
            }
            async fn execute(
                &self,
                _: crate::tools::ToolExecutionContext<'_>,
            ) -> Result<ToolResult, CoreError> {
                unreachable!()
            }
        }
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(OldTool));
        registry.register(Box::new(DeferredFakeTool));
        let old = registry.get("old_deferred_tool").unwrap().definition();
        let mut resident = old.clone();
        resident.name = "tool_search".into();
        let mut definitions = vec![resident, old];
        let artifacts = serde_json::json!({"kind":"toolSearchResults", "matches":[{"name":"mcp__fake_server__search_docs"}]});
        let result = activate_tool_search_matches_bounded(
            &registry,
            &mut definitions,
            Some(&artifacts),
            "gpt-4o",
            2,
            u32::MAX,
        );
        assert_eq!(result.activated, vec!["mcp__fake_server__search_docs"]);
        assert_eq!(result.evicted, vec!["old_deferred_tool"]);
        assert_eq!(definitions.len(), 2);
        assert!(definitions
            .iter()
            .any(|definition| definition.name == "tool_search"));
        // An oversized definition must not evict unrelated tools pointlessly.
        let before = definitions.clone();
        let reverse = serde_json::json!({"kind":"toolSearchResults", "matches":[{"name":"old_deferred_tool"}]});
        let blocked = activate_tool_search_matches_bounded(
            &registry,
            &mut definitions,
            Some(&reverse),
            "gpt-4o",
            2,
            1,
        );
        assert_eq!(blocked.capacity_limited, vec!["old_deferred_tool"]);
        assert_eq!(definitions.len(), before.len());
        assert!(blocked.evicted.is_empty());
    }
}
