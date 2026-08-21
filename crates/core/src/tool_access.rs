use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::approval::ApprovalRisk;
use crate::plugins::CapabilityOwner;
use crate::tools::{
    default_tool_registry, ToolCapabilityDescriptor, ToolInputStreamingMode, ToolInterruptBehavior,
    ToolRegistry, ToolRenderKind,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolAccessInfo {
    pub name: String,
    pub owner: CapabilityOwner,
    pub category: String,
    pub can_read: bool,
    pub can_write: bool,
    pub can_execute: bool,
    pub can_access_network: bool,
    pub needs_approval: bool,
    pub risk_level: ApprovalRisk,
    pub risk_reason: String,
    pub render_kind: ToolRenderKind,
    pub input_streaming: ToolInputStreamingMode,
    pub read_only: bool,
    pub destructive: bool,
    pub concurrency_safe: bool,
    pub interrupt_behavior: ToolInterruptBehavior,
    pub resource_keys: Vec<String>,
}

impl ToolAccessInfo {
    fn from_descriptor(descriptor: ToolCapabilityDescriptor) -> Self {
        let profile = descriptor.access_profile;
        let capabilities = descriptor.capabilities;
        Self {
            name: descriptor.name,
            owner: descriptor.owner,
            category: profile.category,
            can_read: profile.can_read,
            can_write: profile.can_write,
            can_execute: profile.can_execute,
            can_access_network: profile.can_access_network,
            needs_approval: profile.needs_approval,
            risk_level: profile.risk_level,
            risk_reason: profile.risk_reason,
            render_kind: descriptor.ui.render_kind,
            input_streaming: capabilities.input_streaming,
            read_only: capabilities.read_only,
            destructive: capabilities.destructive,
            concurrency_safe: capabilities.concurrency_safe,
            interrupt_behavior: capabilities.interrupt_behavior,
            resource_keys: descriptor.resources.keys,
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
    ToolAccessInfo::from_descriptor(registry.capability_descriptor(name, &serde_json::Value::Null))
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
        assert_eq!(shell.owner.id, "desktop-automation");
        assert!(shell.can_execute);
        assert!(shell.needs_approval);
        assert_eq!(shell.risk_level, ApprovalRisk::High);

        let editor = by_name("edit_file");
        assert!(editor.can_write);
        assert!(editor.needs_approval);

        let fetch = by_name("fetch_url");
        assert_eq!(fetch.owner.id, "web-research");
        assert!(fetch.can_access_network);
        assert!(!fetch.can_write);

        let download_asset = by_name("download_asset");
        assert_eq!(download_asset.owner.id, "web-research");
        assert!(download_asset.can_access_network);
        assert!(download_asset.can_write);
        assert!(download_asset.needs_approval);
        assert_eq!(download_asset.risk_level, ApprovalRisk::Medium);

        let search = by_name("search_knowledge_base");
        assert_eq!(search.owner.id, "knowledge-base");
        assert_eq!(search.category, "knowledge");
        assert!(!search.can_write);

        let memory = by_name("manage_agent_memory");
        assert_eq!(memory.category, "memory");
        assert!(memory.can_write);

        let office = by_name("get_document_info");
        assert_eq!(office.owner.id, "office-documents");
        assert_eq!(office.category, "document_analysis");
        assert!(office.can_read);
        assert!(!office.can_write);

        let office_artifact = by_name("office_artifact");
        assert_eq!(office_artifact.owner.id, "office-documents");
        assert_eq!(office_artifact.category, "filesystem");
        assert!(office_artifact.can_execute);
    }

    #[test]
    fn extra_runtime_tools_are_classified() {
        let map = tool_access_map_with_extra_names([
            "spawn_subagent",
            "judge_subagent_results",
            "web_search",
            "web_research_context",
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

        let web_search = by_name("web_search");
        assert_eq!(web_search.owner.id, "web-research");
        assert_eq!(web_search.category, "web");
        assert_eq!(web_search.render_kind, ToolRenderKind::Search);
        assert!(web_search.read_only);
        assert!(web_search.can_access_network);
        assert!(!web_search.can_write);

        let web_context = by_name("web_research_context");
        assert_eq!(web_context.owner.id, "web-research");
        assert_eq!(web_context.category, "web");
        assert!(web_context.read_only);
        assert!(web_context.can_access_network);

        let unknown_mcp = by_name("mcp__unknown__dangerous");
        assert_eq!(unknown_mcp.owner.id, "mcp-connectors");
        assert_eq!(unknown_mcp.category, "mcp");
        assert!(!unknown_mcp.read_only);
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
        let descriptor = registry.capability_descriptor("run_shell", &serde_json::Value::Null);
        let info = describe_tool_access("run_shell");

        assert_eq!(info.category, profile.category);
        assert_eq!(info.can_read, profile.can_read);
        assert_eq!(info.can_write, profile.can_write);
        assert_eq!(info.can_execute, profile.can_execute);
        assert_eq!(info.can_access_network, profile.can_access_network);
        assert_eq!(info.needs_approval, profile.needs_approval);
        assert_eq!(info.risk_level, profile.risk_level);
        assert_eq!(info.render_kind, descriptor.ui.render_kind);
        assert_eq!(
            info.input_streaming,
            descriptor.capabilities.input_streaming
        );
        assert_eq!(info.read_only, descriptor.capabilities.read_only);
        assert_eq!(info.destructive, descriptor.capabilities.destructive);
        assert_eq!(
            info.concurrency_safe,
            descriptor.capabilities.concurrency_safe
        );
        assert_eq!(
            info.interrupt_behavior,
            descriptor.capabilities.interrupt_behavior
        );
        assert_eq!(info.resource_keys, descriptor.resources.keys);
    }
}
