//! Protocol exits for exposing selected Nexa capabilities to other agents.
//!
//! Protocol exits are not plugins. They are host-owned interfaces that let
//! external agents call bounded Nexa capabilities under source scope and
//! approval policy.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolExitKind {
    McpServer,
    AcpAgent,
    A2aAgent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProtocolExitMaturity {
    Design,
    Candidate,
    Stable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolExitDefinition {
    pub id: &'static str,
    pub kind: ProtocolExitKind,
    pub maturity: ProtocolExitMaturity,
    pub label: &'static str,
    pub description: &'static str,
    pub source_scope_required: bool,
    pub approval_required: bool,
    pub exported_capabilities: &'static [&'static str],
}

pub const NEXA_MCP_SERVER_EXPORT: ProtocolExitDefinition = ProtocolExitDefinition {
    id: "nexa-mcp-server",
    kind: ProtocolExitKind::McpServer,
    maturity: ProtocolExitMaturity::Candidate,
    label: "Nexa MCP Server",
    description:
        "Expose selected local knowledge and document capabilities to external agents through MCP.",
    source_scope_required: true,
    approval_required: true,
    exported_capabilities: &[
        "search_knowledge_base",
        "retrieve_evidence",
        "list_sources",
        "get_document_info",
    ],
};

pub const NEXA_ACP_AGENT_EXPORT: ProtocolExitDefinition = ProtocolExitDefinition {
    id: "nexa-acp-agent",
    kind: ProtocolExitKind::AcpAgent,
    maturity: ProtocolExitMaturity::Design,
    label: "Nexa ACP Agent",
    description:
        "Allow IDE or editor hosts to delegate tasks to Nexa after MCP server export stabilizes.",
    source_scope_required: true,
    approval_required: true,
    exported_capabilities: &[],
};

pub const NEXA_A2A_AGENT_EXPORT: ProtocolExitDefinition = ProtocolExitDefinition {
    id: "nexa-a2a-agent",
    kind: ProtocolExitKind::A2aAgent,
    maturity: ProtocolExitMaturity::Design,
    label: "Nexa A2A Agent",
    description: "Allow agent-to-agent task exchange after a stable scoped protocol export exists.",
    source_scope_required: true,
    approval_required: true,
    exported_capabilities: &[],
};

pub const PROTOCOL_EXITS: &[ProtocolExitDefinition] = &[
    NEXA_MCP_SERVER_EXPORT,
    NEXA_ACP_AGENT_EXPORT,
    NEXA_A2A_AGENT_EXPORT,
];

pub fn protocol_exit_by_id(id: &str) -> Option<&'static ProtocolExitDefinition> {
    PROTOCOL_EXITS.iter().find(|exit| exit.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_server_export_is_the_first_candidate_and_requires_scope() {
        let export = protocol_exit_by_id("nexa-mcp-server").expect("missing MCP server export");

        assert_eq!(export.kind, ProtocolExitKind::McpServer);
        assert_eq!(export.maturity, ProtocolExitMaturity::Candidate);
        assert!(export.source_scope_required);
        assert!(export.approval_required);
        assert!(export
            .exported_capabilities
            .contains(&"search_knowledge_base"));
        assert!(export.exported_capabilities.contains(&"retrieve_evidence"));
    }

    #[test]
    fn acp_and_a2a_do_not_advance_before_mcp_server_export() {
        let mcp = protocol_exit_by_id("nexa-mcp-server").expect("missing MCP server export");

        for export in PROTOCOL_EXITS
            .iter()
            .filter(|export| export.kind != ProtocolExitKind::McpServer)
        {
            assert!(export.maturity < ProtocolExitMaturity::Stable);
            assert!(export.maturity <= mcp.maturity);
            assert!(export.exported_capabilities.is_empty());
        }
    }
}
