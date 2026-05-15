use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::approval::ApprovalRisk;
use crate::plugins::ToolPluginInfo;
use crate::tools::{
    default_tool_registry, fallback_tool_access_profile, ToolAccessProfile, ToolRegistry,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolAccessInfo {
    pub name: String,
    pub plugin: ToolPluginInfo,
    pub category: String,
    pub can_read: bool,
    pub can_write: bool,
    pub can_execute: bool,
    pub can_access_network: bool,
    pub needs_approval: bool,
    pub risk_level: ApprovalRisk,
    pub risk_reason: String,
}

impl ToolAccessInfo {
    fn from_profile(
        name: impl Into<String>,
        plugin: ToolPluginInfo,
        profile: ToolAccessProfile,
    ) -> Self {
        Self {
            name: name.into(),
            plugin,
            category: profile.category,
            can_read: profile.can_read,
            can_write: profile.can_write,
            can_execute: profile.can_execute,
            can_access_network: profile.can_access_network,
            needs_approval: profile.needs_approval,
            risk_level: profile.risk_level,
            risk_reason: profile.risk_reason,
        }
    }
}

pub fn tool_access_map() -> Vec<ToolAccessInfo> {
    tool_access_map_for_names(default_tool_registry().tool_names())
}

pub fn tool_access_map_with_extra_names<I, S>(extra_names: I) -> Vec<ToolAccessInfo>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut names = default_tool_registry().tool_names();
    names.extend(
        extra_names
            .into_iter()
            .map(|name| name.as_ref().to_string()),
    );
    tool_access_map_for_names(names)
}

pub fn tool_access_map_for_names<I, S>(names: I) -> Vec<ToolAccessInfo>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let registry = default_tool_registry();
    let mut seen = BTreeSet::new();
    let mut tools = names
        .into_iter()
        .filter_map(|name| {
            let name = name.as_ref().to_string();
            if seen.insert(name.clone()) {
                Some(describe_tool_access_with_registry(&registry, &name))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    tools.sort_by(|a, b| {
        risk_rank(b.risk_level)
            .cmp(&risk_rank(a.risk_level))
            .then_with(|| a.category.cmp(&b.category))
            .then_with(|| a.name.cmp(&b.name))
    });
    tools
}

fn describe_tool_access_with_registry(registry: &ToolRegistry, name: &str) -> ToolAccessInfo {
    ToolAccessInfo::from_profile(
        name,
        registry.plugin_info(name),
        registry
            .get(name)
            .map(|tool| tool.access_profile(&serde_json::Value::Null))
            .unwrap_or_else(|| fallback_tool_access_profile(name, &serde_json::Value::Null)),
    )
}

fn risk_rank(risk: ApprovalRisk) -> u8 {
    match risk {
        ApprovalRisk::Low => 0,
        ApprovalRisk::Medium => 1,
        ApprovalRisk::High => 2,
    }
}

pub fn describe_tool_access(name: &str) -> ToolAccessInfo {
    let registry = default_tool_registry();
    describe_tool_access_with_registry(&registry, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_access_map_marks_high_risk_and_office_tools() {
        let map = tool_access_map();
        let by_name = |name: &str| {
            map.iter()
                .find(|tool| tool.name == name)
                .unwrap_or_else(|| panic!("missing tool {name}"))
        };

        let shell = by_name("run_shell");
        assert_eq!(shell.plugin.id, "desktop-automation");
        assert!(shell.can_execute);
        assert!(shell.needs_approval);
        assert_eq!(shell.risk_level, ApprovalRisk::High);

        let editor = by_name("edit_file");
        assert!(editor.can_write);
        assert!(editor.needs_approval);

        let fetch = by_name("fetch_url");
        assert_eq!(fetch.plugin.id, "web-research");
        assert!(fetch.can_access_network);
        assert!(!fetch.can_write);

        let search = by_name("search_knowledge_base");
        assert_eq!(search.plugin.id, "knowledge-base");
        assert_eq!(search.category, "knowledge");
        assert!(!search.can_write);

        let memory = by_name("manage_agent_memory");
        assert_eq!(memory.category, "memory");
        assert!(memory.can_write);

        let office = by_name("get_document_info");
        assert_eq!(office.plugin.id, "office-documents");
        assert_eq!(office.category, "document_analysis");
        assert!(office.can_read);
        assert!(!office.can_write);
    }

    #[test]
    fn extra_runtime_tools_are_classified() {
        let map = tool_access_map_with_extra_names([
            "spawn_subagent",
            "judge_subagent_results",
            "mcp__web_search__search",
            "mcp__unknown__dangerous",
        ]);
        let by_name = |name: &str| {
            map.iter()
                .find(|tool| tool.name == name)
                .unwrap_or_else(|| panic!("missing tool {name}"))
        };

        assert_eq!(by_name("spawn_subagent").category, "delegation");
        assert_eq!(
            by_name("judge_subagent_results").risk_level,
            ApprovalRisk::Low
        );

        let web_search = by_name("mcp__web_search__search");
        assert_eq!(web_search.plugin.id, "web-research");
        assert_eq!(web_search.category, "web");
        assert!(web_search.can_access_network);
        assert!(!web_search.can_write);

        let unknown_mcp = by_name("mcp__unknown__dangerous");
        assert_eq!(unknown_mcp.plugin.id, "mcp-connectors");
        assert_eq!(unknown_mcp.category, "mcp");
        assert_eq!(unknown_mcp.risk_level, ApprovalRisk::High);
    }

    #[test]
    fn registered_tool_access_info_matches_registry_profile() {
        let registry = default_tool_registry();
        let args = serde_json::json!({
            "program": "git",
            "args": ["status"],
            "cwd": "."
        });
        let profile = registry.access_profile("run_shell", &args);
        let info = describe_tool_access("run_shell");

        assert_eq!(info.category, profile.category);
        assert_eq!(info.can_read, profile.can_read);
        assert_eq!(info.can_write, profile.can_write);
        assert_eq!(info.can_execute, profile.can_execute);
        assert_eq!(info.can_access_network, profile.can_access_network);
        assert_eq!(info.needs_approval, profile.needs_approval);
        assert_eq!(info.risk_level, profile.risk_level);
    }
}
