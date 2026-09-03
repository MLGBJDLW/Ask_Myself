//! Provider-native web-search policy and the provider-neutral request marker.
//!
//! The agent keeps the normal `web_search` tool in its tool registry. A private
//! marker tells a trusted provider adapter whether to replace that local tool
//! with its server-side search dialect or expose both paths in hybrid mode.
//! Keeping the local definition until the wire boundary lets automatic
//! fallbacks retain Nexa Router search without issuing the same query twice.

use serde::{Deserialize, Serialize};

use super::{ProviderType, ToolDefinition};
use crate::error::CoreError;
use crate::model_catalog::NativeWebSearchCapability;
use crate::provider_catalog::load_provider_presets;
use crate::provider_registry::provider_type_from_key;

pub use crate::model_catalog::NativeSearchDialect;

pub const NATIVE_WEB_SEARCH_MARKER: &str = "__nexa_provider_native_web_search";
pub const LOCAL_WEB_SEARCH_TOOL: &str = "web_search";

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SearchExecutionMode {
    #[default]
    Auto,
    ProviderNative,
    NexaRouter,
    Hybrid,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProviderNativeSearchEngine {
    #[default]
    Auto,
    Native,
    Exa,
    Firecrawl,
    Parallel,
    Perplexity,
}

impl ProviderNativeSearchEngine {
    fn as_wire_value(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Native => "native",
            Self::Exa => "exa",
            Self::Firecrawl => "firecrawl",
            Self::Parallel => "parallel",
            Self::Perplexity => "perplexity",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchIntent {
    pub query: String,
    #[serde(default, alias = "domains", skip_serializing_if = "Vec::is_empty")]
    pub allowed_domains: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_domains: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recency: Option<SearchRecency>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u32>,
    #[serde(default)]
    pub evidence_mode: EvidenceMode,
    #[serde(default)]
    pub privacy_mode: SearchPrivacyMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approximate_location: Option<ApproximateLocation>,
    #[serde(default)]
    pub provider_engine: ProviderNativeSearchEngine,
}

impl Default for WebSearchIntent {
    fn default() -> Self {
        Self {
            query: String::new(),
            allowed_domains: Vec::new(),
            blocked_domains: Vec::new(),
            recency: None,
            locale: None,
            max_results: None,
            evidence_mode: EvidenceMode::Citations,
            privacy_mode: SearchPrivacyMode::ProviderDefault,
            approximate_location: None,
            provider_engine: ProviderNativeSearchEngine::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SearchRecency {
    Day,
    Week,
    Month,
    Year,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum EvidenceMode {
    #[default]
    Citations,
    Sources,
    CitationsAndSources,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SearchPrivacyMode {
    #[default]
    ProviderDefault,
    ExternalWebOnly,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApproximateLocation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SearchCitation {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_index: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SearchEvidence {
    pub dialect: NativeSearchDialect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub citations: Vec<SearchCitation>,
}

fn validate_hosted_search_intent(
    dialect: NativeSearchDialect,
    capability: NativeWebSearchCapability,
    intent: &WebSearchIntent,
) -> Result<(), CoreError> {
    if (!intent.allowed_domains.is_empty() || !intent.blocked_domains.is_empty())
        && !capability.supports_domains
    {
        return Err(CoreError::Llm(format!(
            "Provider-native search dialect {dialect:?} does not support domain constraints"
        )));
    }
    if dialect == NativeSearchDialect::XaiResponses
        && !intent.allowed_domains.is_empty()
        && !intent.blocked_domains.is_empty()
    {
        return Err(CoreError::Llm(
            "xAI web search cannot combine allowed and excluded domains".to_string(),
        ));
    }
    if dialect == NativeSearchDialect::OpenRouterServerTool
        && intent.provider_engine != ProviderNativeSearchEngine::Exa
        && !intent.allowed_domains.is_empty()
        && !intent.blocked_domains.is_empty()
    {
        return Err(CoreError::Llm(format!(
            "OpenRouter {:?} web search cannot safely combine allowed and excluded domains; only the Exa engine documents both together",
            intent.provider_engine
        )));
    }
    if intent.recency.is_some() && !capability.supports_recency {
        return Err(CoreError::Llm(format!(
            "Provider-native search dialect {dialect:?} does not support recency constraints"
        )));
    }
    if intent.locale.is_some() && !capability.supports_locale {
        return Err(CoreError::Llm(format!(
            "Provider-native search dialect {dialect:?} does not support locale constraints"
        )));
    }
    if intent.approximate_location.is_some() && !capability.supports_location {
        return Err(CoreError::Llm(format!(
            "Provider-native search dialect {dialect:?} does not support location sharing"
        )));
    }
    if intent.privacy_mode == SearchPrivacyMode::ExternalWebOnly
        && dialect != NativeSearchDialect::OpenAiResponses
    {
        return Err(CoreError::Llm(format!(
            "Provider-native search dialect {dialect:?} cannot guarantee external-web-only access"
        )));
    }
    Ok(())
}

/// Compile only controls the selected endpoint explicitly supports. Required
/// constraints fail closed instead of being silently dropped or optimistically
/// translated into a different provider's wire dialect.
pub fn compile_hosted_search_tool(
    dialect: NativeSearchDialect,
    capability: NativeWebSearchCapability,
    intent: &WebSearchIntent,
) -> Result<serde_json::Value, CoreError> {
    validate_hosted_search_intent(dialect, capability, intent)?;
    let tool = match dialect {
        NativeSearchDialect::OpenAiResponses => {
            let mut tool = serde_json::json!({ "type": "web_search" });
            if capability.supports_domains {
                let mut filters = serde_json::Map::new();
                if !intent.allowed_domains.is_empty() {
                    filters.insert(
                        "allowed_domains".to_string(),
                        serde_json::json!(intent.allowed_domains),
                    );
                }
                if !intent.blocked_domains.is_empty() {
                    filters.insert(
                        "blocked_domains".to_string(),
                        serde_json::json!(intent.blocked_domains),
                    );
                }
                if !filters.is_empty() {
                    tool["filters"] = serde_json::Value::Object(filters);
                }
            }
            if capability.supports_location {
                if let Some(location) = intent.approximate_location.as_ref() {
                    tool["user_location"] = serde_json::json!({
                        "type": "approximate",
                        "country": location.country,
                        "region": location.region,
                        "city": location.city,
                        "timezone": location.timezone,
                    });
                }
            }
            if intent.privacy_mode == SearchPrivacyMode::ExternalWebOnly {
                tool["external_web_access"] = serde_json::json!(true);
            }
            tool
        }
        NativeSearchDialect::DeepSeekResponses => serde_json::json!({ "type": "web_search" }),
        NativeSearchDialect::XaiResponses => {
            let mut tool = serde_json::json!({ "type": "web_search" });
            if capability.supports_domains {
                let mut filters = serde_json::Map::new();
                if !intent.allowed_domains.is_empty() {
                    filters.insert(
                        "allowed_domains".to_string(),
                        serde_json::json!(intent
                            .allowed_domains
                            .iter()
                            .take(5)
                            .collect::<Vec<_>>()),
                    );
                } else if !intent.blocked_domains.is_empty() {
                    filters.insert(
                        "excluded_domains".to_string(),
                        serde_json::json!(intent
                            .blocked_domains
                            .iter()
                            .take(5)
                            .collect::<Vec<_>>()),
                    );
                }
                if !filters.is_empty() {
                    tool["filters"] = serde_json::Value::Object(filters);
                }
            }
            tool
        }
        NativeSearchDialect::OpenRouterServerTool => {
            let mut parameters = serde_json::Map::new();
            parameters.insert(
                "engine".to_string(),
                serde_json::json!(intent.provider_engine.as_wire_value()),
            );
            if capability.supports_domains {
                if !intent.allowed_domains.is_empty() {
                    parameters.insert(
                        "allowed_domains".to_string(),
                        serde_json::json!(intent.allowed_domains),
                    );
                }
                if !intent.blocked_domains.is_empty() {
                    parameters.insert(
                        "excluded_domains".to_string(),
                        serde_json::json!(intent.blocked_domains),
                    );
                }
            }
            if let Some(max_results) = intent.max_results {
                let maximum = if intent.provider_engine == ProviderNativeSearchEngine::Perplexity {
                    20
                } else {
                    25
                };
                parameters.insert(
                    "max_results".to_string(),
                    serde_json::json!(max_results.clamp(1, maximum)),
                );
            }
            serde_json::json!({
                "type": "openrouter:web_search",
                "parameters": parameters,
            })
        }
        NativeSearchDialect::AnthropicServerTool => {
            serde_json::json!({ "type": "web_search_20260209", "name": "web_search" })
        }
        NativeSearchDialect::GeminiGoogleSearch => serde_json::json!({ "google_search": {} }),
    };
    Ok(tool)
}

/// Render provider citations through the same Markdown link path used by
/// Nexa Router results. The finalization layer already extracts these links
/// into the durable citation UI, so provider adapters do not need a second
/// provider-specific presentation model.
pub fn render_citation_appendix(evidence: &SearchEvidence) -> String {
    let mut seen = std::collections::HashSet::new();
    let citations = evidence
        .citations
        .iter()
        .filter(|citation| {
            (citation.url.starts_with("https://") || citation.url.starts_with("http://"))
                && seen.insert(citation.url.as_str())
        })
        .map(|citation| {
            let title = citation
                .title
                .as_deref()
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .unwrap_or(&citation.url);
            format!("- [{title}]({})", citation.url)
        })
        .collect::<Vec<_>>();
    if citations.is_empty() {
        String::new()
    } else {
        format!("\n\nSources:\n{}\n", citations.join("\n"))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NativeSearchPlan {
    pub mode: SearchExecutionMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialect: Option<NativeSearchDialect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<NativeWebSearchCapability>,
    pub trusted_endpoint: bool,
    #[serde(default)]
    pub provider_engine: ProviderNativeSearchEngine,
}

impl Default for NativeSearchPlan {
    fn default() -> Self {
        Self {
            mode: SearchExecutionMode::Auto,
            dialect: None,
            capability: None,
            trusted_endpoint: false,
            provider_engine: ProviderNativeSearchEngine::Auto,
        }
    }
}

impl NativeSearchPlan {
    pub fn resolve(
        mode: SearchExecutionMode,
        provider_type: ProviderType,
        base_url: Option<&str>,
        model: &str,
    ) -> Self {
        Self::resolve_with_engine(
            mode,
            provider_type,
            base_url,
            model,
            ProviderNativeSearchEngine::Auto,
        )
    }

    pub fn resolve_with_engine(
        mode: SearchExecutionMode,
        provider_type: ProviderType,
        base_url: Option<&str>,
        model: &str,
        provider_engine: ProviderNativeSearchEngine,
    ) -> Self {
        let normalized = normalize_base_url(base_url);
        let capability = load_provider_presets()
            .ok()
            .and_then(|presets| {
                presets.into_iter().find(|preset| {
                    provider_type_from_key(&preset.provider) == Some(provider_type)
                        && normalized.as_deref().is_none_or(|actual| {
                            normalize_base_url(Some(&preset.base_url)).as_deref() == Some(actual)
                        })
                })
            })
            // A provider endpoint may describe the protocol, but runtime use
            // is model-gated. This prevents older models and dynamically
            // discovered IDs from inheriting a server-tool contract that was
            // never verified for them.
            .and_then(|preset| {
                let model_capability = preset
                    .models
                    .iter()
                    .find(|candidate| candidate.id.eq_ignore_ascii_case(model.trim()))
                    .and_then(|candidate| candidate.native_web_search);
                model_capability.or_else(|| {
                    preset.native_web_search.filter(|capability| {
                        matches!(
                            capability.dialect,
                            NativeSearchDialect::OpenRouterServerTool
                                | NativeSearchDialect::XaiResponses
                        )
                    })
                })
            });
        let dialect = capability.map(|value| value.dialect);
        Self {
            mode,
            dialect,
            capability,
            trusted_endpoint: dialect.is_some(),
            provider_engine,
        }
    }

    pub fn validate(self) -> Result<(), CoreError> {
        if self.mode == SearchExecutionMode::ProviderNative && self.dialect.is_none() {
            return Err(CoreError::Llm(
                "Provider-native web search is unavailable for this provider endpoint. Use Auto, Nexa Router, or a trusted native endpoint."
                    .to_string(),
            ));
        }
        Ok(())
    }

    pub fn uses_provider_native(self) -> bool {
        self.dialect.is_some()
            && matches!(
                self.mode,
                SearchExecutionMode::Auto
                    | SearchExecutionMode::ProviderNative
                    | SearchExecutionMode::Hybrid
            )
    }

    pub fn marker(self) -> Option<ToolDefinition> {
        self.uses_provider_native().then(|| ToolDefinition {
            name: NATIVE_WEB_SEARCH_MARKER.to_string(),
            description: "Internal provider-native web search request marker".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "xNexaSearchMode": self.mode,
                "xNexaSearchDialect": self.dialect,
                "xNexaSearchCapability": self.capability,
                "xNexaSearchEngine": self.provider_engine,
            }),
        })
    }
}

pub fn marker_engine(tools: &[ToolDefinition]) -> ProviderNativeSearchEngine {
    tools
        .iter()
        .find(|tool| tool.name == NATIVE_WEB_SEARCH_MARKER)
        .and_then(|tool| tool.parameters.get("xNexaSearchEngine"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

pub fn marker_mode(tools: &[ToolDefinition]) -> Option<SearchExecutionMode> {
    tools
        .iter()
        .find(|tool| tool.name == NATIVE_WEB_SEARCH_MARKER)
        .and_then(|tool| tool.parameters.get("xNexaSearchMode"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
}

pub fn marker_dialect(tools: &[ToolDefinition]) -> Option<NativeSearchDialect> {
    tools
        .iter()
        .find(|tool| tool.name == NATIVE_WEB_SEARCH_MARKER)
        .and_then(|tool| tool.parameters.get("xNexaSearchDialect"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
}

pub fn marker_capability(tools: &[ToolDefinition]) -> Option<NativeWebSearchCapability> {
    tools
        .iter()
        .find(|tool| tool.name == NATIVE_WEB_SEARCH_MARKER)
        .and_then(|tool| tool.parameters.get("xNexaSearchCapability"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
}

pub fn is_native_marker(tool: &ToolDefinition) -> bool {
    tool.name == NATIVE_WEB_SEARCH_MARKER
}

pub fn should_send_local_search(tools: &[ToolDefinition]) -> bool {
    marker_mode(tools).is_none_or(|mode| mode == SearchExecutionMode::Hybrid)
}

fn normalize_base_url(base_url: Option<&str>) -> Option<String> {
    base_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_only_enables_native_search_for_exact_trusted_endpoints() {
        let trusted = NativeSearchPlan::resolve(
            SearchExecutionMode::Auto,
            ProviderType::Google,
            Some("https://generativelanguage.googleapis.com/v1beta/"),
            "gemini-3.8-flash",
        );
        assert_eq!(
            trusted.dialect,
            Some(NativeSearchDialect::GeminiGoogleSearch)
        );
        assert!(trusted.uses_provider_native());

        let custom = NativeSearchPlan::resolve(
            SearchExecutionMode::Auto,
            ProviderType::Google,
            Some("https://proxy.example.test/v1beta"),
            "gemini-3.8-flash",
        );
        assert_eq!(custom.dialect, None);
        assert!(!custom.uses_provider_native());
    }

    #[test]
    fn native_only_and_hybrid_keep_distinct_local_tool_policies() {
        let native = NativeSearchPlan::resolve(
            SearchExecutionMode::ProviderNative,
            ProviderType::Anthropic,
            None,
            "claude-sonnet-5",
        )
        .marker()
        .unwrap();
        assert!(!should_send_local_search(&[native]));

        let hybrid = NativeSearchPlan::resolve(
            SearchExecutionMode::Hybrid,
            ProviderType::Anthropic,
            None,
            "claude-sonnet-5",
        )
        .marker()
        .unwrap();
        assert!(should_send_local_search(&[hybrid]));
    }

    #[test]
    fn explicit_native_mode_rejects_unknown_endpoints() {
        let plan = NativeSearchPlan::resolve(
            SearchExecutionMode::ProviderNative,
            ProviderType::Custom,
            Some("https://gateway.example.test/v1"),
            "custom-model",
        );
        assert!(plan.validate().is_err());
    }

    #[test]
    fn endpoint_capability_does_not_leak_to_unverified_models() {
        let plan = NativeSearchPlan::resolve(
            SearchExecutionMode::Auto,
            ProviderType::Google,
            None,
            "gemini-2.5-flash",
        );
        assert_eq!(plan.dialect, None);
        assert!(!plan.trusted_endpoint);
    }

    #[test]
    fn chat_completions_endpoints_do_not_claim_responses_search() {
        let plan = NativeSearchPlan::resolve(
            SearchExecutionMode::Auto,
            ProviderType::Custom,
            Some("https://api.deepseek.com/v1"),
            "deepseek-v4-pro",
        );
        assert_eq!(plan.dialect, None);

        let deepseek_pro = NativeSearchPlan::resolve(
            SearchExecutionMode::Auto,
            ProviderType::DeepSeek,
            Some("https://api.deepseek.com"),
            "deepseek-v4-pro",
        );
        assert_eq!(
            deepseek_pro.dialect,
            Some(NativeSearchDialect::DeepSeekResponses)
        );
        assert!(deepseek_pro.marker().is_some());

        let deepseek_flash = NativeSearchPlan::resolve(
            SearchExecutionMode::ProviderNative,
            ProviderType::DeepSeek,
            Some("https://api.deepseek.com"),
            "deepseek-v4-flash",
        );
        assert_eq!(
            deepseek_flash.dialect,
            Some(NativeSearchDialect::DeepSeekResponses)
        );
        assert!(deepseek_flash.marker().is_some());

        let deepseek_vision = NativeSearchPlan::resolve(
            SearchExecutionMode::ProviderNative,
            ProviderType::DeepSeek,
            Some("https://api.deepseek.com"),
            "deepseek-v4-flash-vision-exp",
        );
        assert_eq!(
            deepseek_vision.dialect,
            Some(NativeSearchDialect::DeepSeekResponses)
        );
        assert!(deepseek_vision.marker().is_some());
    }

    #[test]
    fn responses_compiler_rejects_required_controls_deepseek_does_not_support() {
        let intent = WebSearchIntent {
            allowed_domains: vec!["example.com".to_string()],
            blocked_domains: vec!["blocked.example".to_string()],
            locale: Some("en-US".to_string()),
            recency: Some(SearchRecency::Week),
            max_results: Some(8),
            privacy_mode: SearchPrivacyMode::ExternalWebOnly,
            ..WebSearchIntent::default()
        };
        let capability = NativeWebSearchCapability {
            dialect: NativeSearchDialect::DeepSeekResponses,
            supports_domains: false,
            supports_recency: false,
            supports_locale: false,
            supports_location: false,
            supports_citations: false,
            supports_stream_events: true,
            can_mix_client_tools: true,
        };
        let error =
            compile_hosted_search_tool(NativeSearchDialect::DeepSeekResponses, capability, &intent)
                .expect_err("unsupported required constraints must fail closed");
        assert!(error.to_string().contains("domain constraints"));
    }

    #[test]
    fn xai_and_openrouter_use_their_verified_responses_dialects() {
        let xai = NativeSearchPlan::resolve(
            SearchExecutionMode::Auto,
            ProviderType::OpenAi,
            Some("https://api.x.ai/v1"),
            "grok-4.6",
        );
        assert_eq!(xai.dialect, Some(NativeSearchDialect::XaiResponses));

        let openrouter = NativeSearchPlan::resolve_with_engine(
            SearchExecutionMode::ProviderNative,
            ProviderType::OpenRouter,
            Some("https://openrouter.ai/api/v1"),
            "anthropic/claude-sonnet-5",
            ProviderNativeSearchEngine::Exa,
        );
        assert_eq!(
            openrouter.dialect,
            Some(NativeSearchDialect::OpenRouterServerTool)
        );
        let marker = openrouter.marker().expect("trusted OpenRouter marker");
        assert_eq!(marker_engine(&[marker]), ProviderNativeSearchEngine::Exa);
    }

    #[test]
    fn provider_specific_compilers_preserve_supported_controls() {
        let capability = NativeWebSearchCapability {
            dialect: NativeSearchDialect::XaiResponses,
            supports_domains: true,
            supports_recency: false,
            supports_locale: false,
            supports_location: false,
            supports_citations: true,
            supports_stream_events: true,
            can_mix_client_tools: true,
        };
        let xai = compile_hosted_search_tool(
            NativeSearchDialect::XaiResponses,
            capability,
            &WebSearchIntent {
                blocked_domains: vec!["example.com".to_string()],
                ..WebSearchIntent::default()
            },
        )
        .unwrap();
        assert_eq!(
            xai,
            serde_json::json!({
                "type": "web_search",
                "filters": { "excluded_domains": ["example.com"] },
            })
        );

        let openrouter = compile_hosted_search_tool(
            NativeSearchDialect::OpenRouterServerTool,
            NativeWebSearchCapability {
                dialect: NativeSearchDialect::OpenRouterServerTool,
                ..capability
            },
            &WebSearchIntent {
                allowed_domains: vec!["openai.com".to_string()],
                blocked_domains: vec!["example.com".to_string()],
                max_results: Some(50),
                provider_engine: ProviderNativeSearchEngine::Exa,
                ..WebSearchIntent::default()
            },
        )
        .unwrap();
        assert_eq!(
            openrouter,
            serde_json::json!({
                "type": "openrouter:web_search",
                "parameters": {
                    "engine": "exa",
                    "allowed_domains": ["openai.com"],
                    "excluded_domains": ["example.com"],
                    "max_results": 25,
                },
            })
        );

        let unsafe_filters = compile_hosted_search_tool(
            NativeSearchDialect::OpenRouterServerTool,
            NativeWebSearchCapability {
                dialect: NativeSearchDialect::OpenRouterServerTool,
                ..capability
            },
            &WebSearchIntent {
                allowed_domains: vec!["openai.com".to_string()],
                blocked_domains: vec!["example.com".to_string()],
                provider_engine: ProviderNativeSearchEngine::Parallel,
                ..WebSearchIntent::default()
            },
        )
        .expect_err("engine-specific mutually exclusive filters must fail closed");
        assert!(unsafe_filters.to_string().contains("only the Exa engine"));
    }
}
