use super::*;
pub(super) static SPAWN_SUBAGENT_DEF: OnceLock<DelegationToolDef> = OnceLock::new();
pub(super) static SPAWN_SUBAGENT_BATCH_DEF: OnceLock<DelegationToolDef> = OnceLock::new();
pub(super) static JUDGE_SUBAGENT_RESULTS_DEF: OnceLock<DelegationToolDef> = OnceLock::new();
pub(super) const SPAWN_SUBAGENT_JSON: &str =
    include_str!("../../../../../crates/core/prompts/tools/spawn_subagent.json");
pub(super) const SPAWN_SUBAGENT_BATCH_JSON: &str =
    include_str!("../../../../../crates/core/prompts/tools/spawn_subagent_batch.json");
pub(super) const JUDGE_SUBAGENT_RESULTS_JSON: &str =
    include_str!("../../../../../crates/core/prompts/tools/judge_subagent_results.json");
pub(super) const MAX_SUBAGENT_DELEGATION_DEPTH: u8 = 1;
// Initial scheduler credit; this is not sent as a per-request output ceiling.
pub(super) const INITIAL_SUBAGENT_OUTPUT_CREDIT: u32 = 8_192;
pub(super) const SUBAGENT_INTERACTIVE_SURFACE_TOOLS: &[&str] = &[
    "browser_session",
    "computer_observe",
    "computer_control",
    "desktop_automation",
];
pub(super) fn provider_catalog_key(provider_type: ProviderType) -> &'static str {
    match provider_type {
        ProviderType::OpenAi => "open_ai",
        ProviderType::OpenRouter => "openrouter",
        ProviderType::Anthropic => "anthropic",
        ProviderType::Google => "google",
        ProviderType::DeepSeek => "deep_seek",
        ProviderType::Ollama => "ollama",
        ProviderType::LmStudio => "lm_studio",
        ProviderType::AzureOpenAi => "azure_open_ai",
        ProviderType::Zhipu => "zhipu",
        ProviderType::Moonshot => "moonshot",
        ProviderType::Qwen => "qwen",
        ProviderType::AlibabaModelStudio => "alibaba_model_studio",
        ProviderType::SiliconFlow => "siliconflow",
        ProviderType::Doubao => "doubao",
        ProviderType::Yi => "yi",
        ProviderType::Baichuan => "baichuan",
        ProviderType::Custom => "custom",
    }
}
pub(super) struct DelegationToolDef {
    pub(super) description: String,
    pub(super) parameters: serde_json::Value,
}
pub(super) fn delegation_tool_def<'a>(
    lock: &'a OnceLock<DelegationToolDef>,
    json_str: &str,
) -> &'a DelegationToolDef {
    lock.get_or_init(|| {
        let value: serde_json::Value =
            serde_json::from_str(json_str).expect("invalid delegated tool JSON definition");
        DelegationToolDef {
            description: value["description"]
                .as_str()
                .expect("delegated tool JSON missing description")
                .to_string(),
            parameters: value["parameters"].clone(),
        }
    })
}
pub(super) fn spawn_subagent_parameters_schema() -> serde_json::Value {
    let mut schema = delegation_tool_def(&SPAWN_SUBAGENT_DEF, SPAWN_SUBAGENT_JSON)
        .parameters
        .clone();
    schema["properties"]["role_id"]["enum"] = serde_json::json!(role_id_values());
    schema
}
pub(super) fn spawn_subagent_batch_parameters_schema() -> serde_json::Value {
    let mut schema = delegation_tool_def(&SPAWN_SUBAGENT_BATCH_DEF, SPAWN_SUBAGENT_BATCH_JSON)
        .parameters
        .clone();
    let role_ids = serde_json::json!(role_id_values());
    schema["properties"]["tasks"]["items"]["properties"]["role_id"]["enum"] = role_ids;
    schema["properties"]["workflow_template"]["enum"] =
        serde_json::json!(workflow_template_id_values());
    schema
}
pub(super) struct SubagentToolSpec {
    pub(super) name: &'static str,
    pub(super) enabled_by_default: bool,
}
pub(super) const SUBAGENT_TOOL_SPECS: &[SubagentToolSpec] = &[
    SubagentToolSpec {
        name: "search_knowledge_base",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "tool_search",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "read_file",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "read_files",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "retrieve_evidence",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "manage_playbook",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "list_sources",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "list_documents",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "list_dir",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "glob_files",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "search_files",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "grep_files",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "get_chunk_context",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "fetch_url",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "web_search",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "web_research_context",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "browser_evidence_capture",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "desktop_automation",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "write_note",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "search_playbooks",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "edit_file",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "multi_edit",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "submit_feedback",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "get_document_info",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "reindex_document",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "compare_documents",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "manage_source",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "get_statistics",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "search_by_date",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "summarize_document",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "update_plan",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "record_verification",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "spawn_subagent",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "spawn_subagent_batch",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "judge_subagent_results",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "observe_subagent_batch",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "observe_subagent",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "wait_subagent",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "send_subagent_input",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "cancel_subagent",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "close_subagent",
        enabled_by_default: false,
    },
    SubagentToolSpec {
        name: "compile_document",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "query_knowledge_graph",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "run_health_check",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "archive_output",
        enabled_by_default: true,
    },
    SubagentToolSpec {
        name: "get_related_concepts",
        enabled_by_default: true,
    },
];
pub(super) struct SubagentRoleProfile {
    pub(super) id: &'static str,
    pub(super) label: &'static str,
    pub(super) instructions: &'static str,
    pub(super) default_sections: &'static [&'static str],
    pub(super) recommended_tools: &'static [&'static str],
}
pub(super) const ROLE_RESEARCHER_SECTIONS: &[&str] =
    &["Conclusion", "Evidence gathered", "Gaps or uncertainty"];
pub(super) const ROLE_VERIFIER_SECTIONS: &[&str] =
    &["Verdict", "Checks performed", "Unverified or risky claims"];
pub(super) const ROLE_CRITIC_SECTIONS: &[&str] =
    &["Main concerns", "Failure modes", "Suggested fixes"];
pub(super) const ROLE_PLANNER_SECTIONS: &[&str] = &["Plan", "Dependencies", "Verification gates"];
pub(super) const ROLE_WRITER_SECTIONS: &[&str] = &["Draft", "Assumptions", "Follow-up edits"];
pub(super) const ROLE_CONNECTOR_SECTIONS: &[&str] =
    &["Connector options", "Setup risks", "Recommended path"];
pub(super) const ROLE_DESKTOP_OPERATOR_SECTIONS: &[&str] =
    &["Action result", "Observed state", "Next safe action"];
pub(super) const SUBAGENT_ROLE_PROFILES: &[SubagentRoleProfile] = &[
    SubagentRoleProfile {
        id: "researcher",
        label: "Researcher",
        instructions: "Find and summarize relevant evidence. Prefer retrieval before synthesis, distinguish direct evidence from inference, and return only material useful to the supervisor.",
        default_sections: ROLE_RESEARCHER_SECTIONS,
        recommended_tools: &[
            "search_knowledge_base",
            "retrieve_evidence",
            "read_file",
            "read_files",
            "list_sources",
            "list_documents",
            "list_dir",
            "glob_files",
            "search_files",
            "grep_files",
            "get_chunk_context",
            "fetch_url",
            "web_search",
            "web_research_context",
            "search_playbooks",
            "get_document_info",
            "search_by_date",
            "summarize_document",
            "query_knowledge_graph",
            "get_related_concepts",
            "record_verification",
        ],
    },
    SubagentRoleProfile {
        id: "verifier",
        label: "Verifier",
        instructions: "Check whether a proposed answer or plan is supported. Look for missing evidence, stale assumptions, contradictions, and unverifiable claims. Prefer concise pass/fail findings.",
        default_sections: ROLE_VERIFIER_SECTIONS,
        recommended_tools: &[
            "search_knowledge_base",
            "retrieve_evidence",
            "read_file",
            "read_files",
            "glob_files",
            "search_files",
            "grep_files",
            "fetch_url",
            "web_search",
            "web_research_context",
            "compare_documents",
            "get_document_info",
            "run_health_check",
            "record_verification",
        ],
    },
    SubagentRoleProfile {
        id: "critic",
        label: "Critic",
        instructions: "Stress-test the proposed approach. Identify brittle reasoning, missing edge cases, UX or trust risks, and places where the supervisor should simplify or narrow scope.",
        default_sections: ROLE_CRITIC_SECTIONS,
        recommended_tools: &[
            "read_file",
            "read_files",
            "glob_files",
            "search_files",
            "grep_files",
            "compare_documents",
            "search_knowledge_base",
            "retrieve_evidence",
            "record_verification",
        ],
    },
    SubagentRoleProfile {
        id: "planner",
        label: "Planner",
        instructions: "Turn the goal into a practical sequence with dependencies, risk controls, and verification gates. Keep the plan executable and avoid speculative work.",
        default_sections: ROLE_PLANNER_SECTIONS,
        recommended_tools: &[
            "update_plan",
            "search_playbooks",
            "search_knowledge_base",
            "list_sources",
            "list_documents",
            "record_verification",
        ],
    },
    SubagentRoleProfile {
        id: "writer",
        label: "Writer",
        instructions: "Produce a clean draft or synthesis for the supervisor to adapt. Keep the output grounded in supplied context and note assumptions rather than silently inventing details.",
        default_sections: ROLE_WRITER_SECTIONS,
        recommended_tools: &[
            "read_file",
            "read_files",
            "glob_files",
            "search_files",
            "grep_files",
            "retrieve_evidence",
            "search_knowledge_base",
            "search_playbooks",
            "record_verification",
        ],
    },
    SubagentRoleProfile {
        id: "connector",
        label: "Connector Specialist",
        instructions: "Evaluate external connector or MCP options. Focus on tool availability, lifecycle, credentials, timeout behavior, and safe defaults before recommending setup.",
        default_sections: ROLE_CONNECTOR_SECTIONS,
        recommended_tools: &[
            "list_sources",
            "search_playbooks",
            "search_knowledge_base",
            "fetch_url",
            "web_search",
            "web_research_context",
            "record_verification",
        ],
    },
    SubagentRoleProfile {
        id: "desktop_operator",
        label: "Desktop Operator",
        instructions: "Plan a narrow user-visible browser or desktop action for the supervisor to perform. Delegated workers do not receive interactive surface control or approval authority; inspect only supplied evidence, state the exact proposed action, and never infer private screen state you cannot observe.",
        default_sections: ROLE_DESKTOP_OPERATOR_SECTIONS,
        recommended_tools: &[
            "fetch_url",
            "read_file",
            "list_dir",
            "record_verification",
        ],
    },
];
