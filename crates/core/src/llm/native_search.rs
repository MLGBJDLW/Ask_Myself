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

/// Compile only controls the selected endpoint explicitly supports. This is
/// intentionally lossy for unsupported intent fields: adapters must not send
/// optimistic OpenAI-shaped options to DeepSeek or Gemini endpoints.
pub fn compile_hosted_search_tool(
    dialect: NativeSearchDialect,
    capability: NativeWebSearchCapability,
    intent: &WebSearchIntent,
) -> serde_json::Value {
    match dialect {
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
        NativeSearchDialect::AnthropicServerTool => {
            serde_json::json!({ "type": "web_search_20260209", "name": "web_search" })
        }
        NativeSearchDialect::GeminiGoogleSearch => serde_json::json!({ "google_search": {} }),
    }
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
}

impl Default for NativeSearchPlan {
    fn default() -> Self {
        Self {
            mode: SearchExecutionMode::Auto,
            dialect: None,
            capability: None,
            trusted_endpoint: false,
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
                preset
                    .models
                    .iter()
                    .find(|candidate| candidate.id.eq_ignore_ascii_case(model.trim()))
                    .and_then(|candidate| candidate.native_web_search)
            });
        let dialect = capability.map(|value| value.dialect);
        Self {
            mode,
            dialect,
            capability,
            trusted_endpoint: dialect.is_some(),
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
            }),
        })
    }
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
            "gemini-3.6-flash",
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
            "gemini-3.6-flash",
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
        assert_eq!(deepseek_pro.dialect, None);
        assert!(deepseek_pro.marker().is_none());

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
    }

    #[test]
    fn responses_compiler_drops_controls_deepseek_does_not_support() {
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
        assert_eq!(
            compile_hosted_search_tool(NativeSearchDialect::DeepSeekResponses, capability, &intent),
            serde_json::json!({ "type": "web_search" })
        );
    }
}
