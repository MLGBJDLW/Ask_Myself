use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::projection::normalize_endpoint_url;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    None,
    #[default]
    ApiKey,
    OAuth,
    ServiceAccount,
    Subscription,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EndpointTransport {
    #[default]
    Http,
    Sse,
    Websocket,
    AsyncJob,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AuthStyle {
    None,
    #[default]
    Bearer,
    Header,
    Query,
    OAuth2,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryStrategy {
    #[default]
    None,
    OpenAiModels,
    ProviderCatalog,
    Static,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum HealthProbe {
    #[default]
    None,
    Models,
    LightweightRequest,
}

/// Provider-owned web-search protocol exposed by one concrete endpoint.
///
/// This belongs to the endpoint rather than the provider identity because a
/// provider can expose Chat Completions, Responses, Messages, and other wire
/// surfaces with different server-tool capabilities.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum NativeSearchDialect {
    OpenAiResponses,
    AnthropicServerTool,
    GeminiGoogleSearch,
    DeepSeekResponses,
    XaiResponses,
    OpenRouterServerTool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct NativeWebSearchCapability {
    pub dialect: NativeSearchDialect,
    #[serde(default)]
    pub supports_domains: bool,
    #[serde(default)]
    pub supports_recency: bool,
    #[serde(default)]
    pub supports_locale: bool,
    #[serde(default)]
    pub supports_location: bool,
    #[serde(default)]
    pub supports_citations: bool,
    #[serde(default)]
    pub supports_stream_events: bool,
    #[serde(default)]
    pub can_mix_client_tools: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderEndpoint {
    pub id: String,
    pub provider_id: String,
    pub region: String,
    pub base_url_template: String,
    pub api_style: String,
    pub transport: EndpointTransport,
    pub auth_style: AuthStyle,
    pub workspace_required: bool,
    pub discovery_strategy: DiscoveryStrategy,
    pub health_probe: HealthProbe,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_web_search: Option<NativeWebSearchCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDescriptor {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub credential_kind: CredentialKind,
    #[serde(default)]
    pub documentation_ref: Option<String>,
    #[serde(default)]
    pub endpoints: Vec<ProviderEndpoint>,
}

#[derive(Debug, Clone)]
pub struct EndpointRegistry {
    providers: Vec<ProviderDescriptor>,
    provider_aliases: HashMap<String, usize>,
    endpoint_ids: HashSet<String>,
}

impl EndpointRegistry {
    pub fn new(providers: Vec<ProviderDescriptor>) -> Result<Self, String> {
        let mut provider_aliases = HashMap::new();
        let mut endpoint_ids = HashSet::new();

        for (index, provider) in providers.iter().enumerate() {
            let provider_id = normalize(&provider.id);
            if provider_id.is_empty() {
                return Err("provider id cannot be empty".into());
            }
            register_alias(&mut provider_aliases, &provider_id, index)?;
            for alias in &provider.aliases {
                register_alias(&mut provider_aliases, alias, index)?;
            }
            for endpoint in &provider.endpoints {
                if normalize(&endpoint.provider_id) != provider_id {
                    return Err(format!(
                        "endpoint '{}' belongs to '{}' instead of '{}'",
                        endpoint.id, endpoint.provider_id, provider.id
                    ));
                }
                if !endpoint_ids.insert(normalize(&endpoint.id)) {
                    return Err(format!("duplicate endpoint id '{}'", endpoint.id));
                }
            }
        }

        Ok(Self {
            providers,
            provider_aliases,
            endpoint_ids,
        })
    }

    pub fn providers(&self) -> &[ProviderDescriptor] {
        &self.providers
    }

    pub fn endpoint(&self, endpoint_id: &str) -> Option<&ProviderEndpoint> {
        let endpoint_id = normalize(endpoint_id);
        if !self.endpoint_ids.contains(&endpoint_id) {
            return None;
        }
        self.providers
            .iter()
            .flat_map(|provider| provider.endpoints.iter())
            .find(|endpoint| normalize(&endpoint.id) == endpoint_id)
    }

    pub fn resolve(
        &self,
        provider_or_alias: &str,
        base_url: Option<&str>,
        api_style: Option<&str>,
    ) -> Option<&ProviderEndpoint> {
        let base_url = normalize_url(base_url);
        let api_style = normalize(api_style.unwrap_or_default());

        if !base_url.is_empty() {
            let exact = self
                .providers
                .iter()
                .flat_map(|provider| provider.endpoints.iter())
                .filter(|endpoint| {
                    normalize_url(Some(&endpoint.base_url_template)) == base_url
                        && (api_style.is_empty() || normalize(&endpoint.api_style) == api_style)
                })
                .collect::<Vec<_>>();
            return (exact.len() == 1).then(|| exact[0]);
        }

        let provider_index = *self.provider_aliases.get(&normalize(provider_or_alias))?;
        let endpoints = &self.providers[provider_index].endpoints;
        let matches = endpoints
            .iter()
            .filter(|endpoint| api_style.is_empty() || normalize(&endpoint.api_style) == api_style)
            .collect::<Vec<_>>();
        (matches.len() == 1).then(|| matches[0])
    }
}

fn register_alias(
    aliases: &mut HashMap<String, usize>,
    alias: &str,
    provider_index: usize,
) -> Result<(), String> {
    let alias = normalize(alias);
    if alias.is_empty() {
        return Err("provider alias cannot be empty".into());
    }
    if let Some(existing) = aliases.insert(alias.clone(), provider_index) {
        if existing != provider_index {
            return Err(format!("provider alias '{alias}' is ambiguous"));
        }
    }
    Ok(())
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn normalize_url(value: Option<&str>) -> String {
    normalize_endpoint_url(value)
}
