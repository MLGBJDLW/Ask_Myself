//! Canonical provider and endpoint identity shared by wire capability compilers.
//!
//! A familiar host is not sufficient to inherit provider behavior. Trusted
//! profiles require HTTPS, the default port, and the documented API base path.

use super::ProviderType;

struct ProviderMetadata {
    id: &'static str,
    default_endpoint: &'static str,
}

fn provider_metadata(provider: ProviderType) -> ProviderMetadata {
    let (id, default_endpoint) = match provider {
        ProviderType::OpenAi => ("openai", "https://api.openai.com/v1"),
        ProviderType::OpenRouter => ("openrouter", "https://openrouter.ai/api/v1"),
        ProviderType::Anthropic => ("anthropic", "https://api.anthropic.com/v1"),
        ProviderType::Google => ("google", "https://generativelanguage.googleapis.com/v1beta"),
        ProviderType::DeepSeek => ("deepseek", "https://api.deepseek.com"),
        ProviderType::Ollama => ("ollama", ""),
        ProviderType::LmStudio => ("lmStudio", ""),
        ProviderType::AzureOpenAi => ("azureOpenAi", ""),
        ProviderType::Zhipu => ("zhipu", ""),
        ProviderType::Moonshot => ("moonshot", "https://api.moonshot.ai/v1"),
        ProviderType::Qwen => ("qwen", "https://dashscope.aliyuncs.com/compatible-mode/v1"),
        ProviderType::AlibabaModelStudio => (
            "alibabaModelStudio",
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
        ),
        ProviderType::SiliconFlow => ("siliconFlow", "https://api.siliconflow.cn/v1"),
        ProviderType::Doubao => ("doubao", ""),
        ProviderType::Yi => ("yi", ""),
        ProviderType::Baichuan => ("baichuan", ""),
        ProviderType::Custom => ("custom", ""),
    };
    ProviderMetadata {
        id,
        default_endpoint,
    }
}

pub(super) fn provider_id(provider: ProviderType) -> &'static str {
    provider_metadata(provider).id
}

fn default_endpoint(provider: ProviderType) -> &'static str {
    provider_metadata(provider).default_endpoint
}

pub(super) fn trusted_url(provider: ProviderType, base_url: Option<&str>) -> Option<reqwest::Url> {
    let endpoint = base_url.unwrap_or_else(|| default_endpoint(provider));
    let url = reqwest::Url::parse(endpoint).ok()?;
    (url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

fn path_is(url: &reqwest::Url, expected: &[&str]) -> bool {
    let path = url.path().trim_end_matches('/');
    expected.contains(&path)
}

fn endpoint_matches(
    provider: ProviderType,
    base_url: Option<&str>,
    hosts: &[&str],
    paths: &[&str],
) -> bool {
    let Some(url) = trusted_url(provider, base_url) else {
        return false;
    };
    hosts.contains(&url.host_str().unwrap_or_default()) && path_is(&url, paths)
}

pub(super) fn is_openai_public_endpoint(provider: ProviderType, base_url: Option<&str>) -> bool {
    endpoint_matches(provider, base_url, &["api.openai.com"], &["/v1"])
}

pub(super) fn is_xai_public_endpoint(provider: ProviderType, base_url: Option<&str>) -> bool {
    provider == ProviderType::OpenAi
        && endpoint_matches(provider, base_url, &["api.x.ai"], &["/v1"])
}

pub(super) fn is_minimax_public_endpoint(provider: ProviderType, base_url: Option<&str>) -> bool {
    provider == ProviderType::OpenAi
        && endpoint_matches(provider, base_url, &["api.minimax.io"], &["/v1"])
}

pub(super) fn is_mistral_public_endpoint(provider: ProviderType, base_url: Option<&str>) -> bool {
    provider == ProviderType::OpenAi
        && endpoint_matches(provider, base_url, &["api.mistral.ai"], &["/v1"])
}

pub(super) fn is_meta_model_api_endpoint(provider: ProviderType, base_url: Option<&str>) -> bool {
    provider == ProviderType::OpenAi
        && endpoint_matches(provider, base_url, &["api.meta.ai"], &["/v1"])
}

pub(super) fn is_openrouter_public_endpoint(
    provider: ProviderType,
    base_url: Option<&str>,
) -> bool {
    endpoint_matches(provider, base_url, &["openrouter.ai"], &["/api/v1"])
}

pub(super) fn is_deepseek_public_endpoint(provider: ProviderType, base_url: Option<&str>) -> bool {
    endpoint_matches(provider, base_url, &["api.deepseek.com"], &["", "/v1"])
}

pub(super) fn is_deepseek_anthropic_endpoint(
    provider: ProviderType,
    base_url: Option<&str>,
) -> bool {
    provider == ProviderType::DeepSeek
        && endpoint_matches(provider, base_url, &["api.deepseek.com"], &["/anthropic"])
}

pub(super) fn is_moonshot_public_endpoint(provider: ProviderType, base_url: Option<&str>) -> bool {
    endpoint_matches(
        provider,
        base_url,
        &["api.moonshot.ai", "api.moonshot.cn"],
        &["/v1"],
    )
}

pub(super) fn is_anthropic_public_endpoint(provider: ProviderType, base_url: Option<&str>) -> bool {
    endpoint_matches(provider, base_url, &["api.anthropic.com"], &["/v1"])
}

pub(super) fn is_google_public_endpoint(provider: ProviderType, base_url: Option<&str>) -> bool {
    provider == ProviderType::Google
        && endpoint_matches(
            provider,
            base_url,
            &["generativelanguage.googleapis.com"],
            &["/v1beta"],
        )
}

pub(super) fn is_siliconflow_public_endpoint(
    provider: ProviderType,
    base_url: Option<&str>,
) -> bool {
    endpoint_matches(provider, base_url, &["api.siliconflow.cn"], &["/v1"])
}

pub(super) fn is_zhipu_model_api_endpoint(provider: ProviderType, base_url: Option<&str>) -> bool {
    provider == ProviderType::Zhipu
        && endpoint_matches(
            provider,
            base_url,
            &["open.bigmodel.cn", "api.z.ai"],
            &["/api/paas/v4"],
        )
}

pub(super) fn is_azure_openai_endpoint(provider: ProviderType, base_url: Option<&str>) -> bool {
    let Some(url) = trusted_url(provider, base_url) else {
        return false;
    };
    url.host_str()
        .is_some_and(|host| host.ends_with(".openai.azure.com"))
        && url.path().starts_with("/openai/")
}

pub(super) fn is_alibaba_chat_endpoint(provider: ProviderType, base_url: Option<&str>) -> bool {
    if !matches!(
        provider,
        ProviderType::Qwen | ProviderType::AlibabaModelStudio
    ) {
        return false;
    }
    let Some(url) = trusted_url(provider, base_url) else {
        return false;
    };
    let host = url.host_str().unwrap_or_default();
    path_is(&url, &["/compatible-mode/v1"])
        && (matches!(
            host,
            "dashscope.aliyuncs.com"
                | "dashscope-intl.aliyuncs.com"
                | "dashscope-us.aliyuncs.com"
                | "token-plan.cn-beijing.maas.aliyuncs.com"
                | "token-plan.ap-southeast-1.maas.aliyuncs.com"
        ) || host.ends_with(".maas.aliyuncs.com"))
}

pub(super) fn endpoint_id(provider: ProviderType, base_url: Option<&str>) -> String {
    let endpoint = base_url.unwrap_or_else(|| default_endpoint(provider));
    let normalized = endpoint.trim().trim_end_matches('/').to_ascii_lowercase();
    if normalized.is_empty() {
        return format!("{}-default", provider_id(provider));
    }
    let Some(url) = trusted_url(provider, base_url) else {
        return custom_endpoint_id(&normalized);
    };
    let host = url.host_str().unwrap_or_default();
    let known = match host {
        "api.openai.com" if path_is(&url, &["/v1"]) => Some("openai-public"),
        "api.x.ai" if path_is(&url, &["/v1"]) => Some("xai-public"),
        "api.minimax.io" if path_is(&url, &["/v1"]) => Some("minimax-public"),
        "api.mistral.ai" if path_is(&url, &["/v1"]) => Some("mistral-public"),
        "api.meta.ai" if path_is(&url, &["/v1"]) => Some("meta-model-api-public"),
        "openrouter.ai" if path_is(&url, &["/api/v1"]) => Some("openrouter-public"),
        "api.deepseek.com" if path_is(&url, &["", "/v1"]) => Some("deepseek-public"),
        "api.deepseek.com" if path_is(&url, &["/anthropic"]) => Some("deepseek-anthropic-public"),
        "api.moonshot.ai" | "api.moonshot.cn" if path_is(&url, &["/v1"]) => Some("moonshot-public"),
        "api.anthropic.com" if path_is(&url, &["/v1"]) => Some("anthropic-public"),
        "generativelanguage.googleapis.com" if path_is(&url, &["/v1beta"]) => Some("google-public"),
        "token-plan.cn-beijing.maas.aliyuncs.com" if path_is(&url, &["/compatible-mode/v1"]) => {
            Some("token-plan-cn")
        }
        "token-plan.ap-southeast-1.maas.aliyuncs.com"
            if path_is(&url, &["/compatible-mode/v1"]) =>
        {
            Some("token-plan-global")
        }
        "dashscope.aliyuncs.com" if path_is(&url, &["/compatible-mode/v1"]) => {
            Some("alibaba-cn-beijing")
        }
        "dashscope-intl.aliyuncs.com" if path_is(&url, &["/compatible-mode/v1"]) => {
            Some("qwencloud-global")
        }
        "dashscope-us.aliyuncs.com" if path_is(&url, &["/compatible-mode/v1"]) => {
            Some("alibaba-us-virginia")
        }
        "api.siliconflow.cn" if path_is(&url, &["/v1"]) => Some("siliconflow-public"),
        _ => None,
    };
    if let Some(known) = known {
        return known.to_string();
    }
    if host.ends_with(".maas.aliyuncs.com") && path_is(&url, &["/compatible-mode/v1"]) {
        let digest = blake3::hash(host.as_bytes()).to_hex();
        return format!("alibaba-workspace-{}", &digest[..12]);
    }
    custom_endpoint_id(&normalized)
}

fn custom_endpoint_id(normalized: &str) -> String {
    let digest = blake3::hash(normalized.as_bytes()).to_hex();
    format!("custom-{}", &digest[..16])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_identity_requires_the_canonical_security_boundary() {
        assert_eq!(
            endpoint_id(
                ProviderType::DeepSeek,
                Some("https://api.deepseek.com/anthropic")
            ),
            "deepseek-anthropic-public"
        );
        assert!(is_deepseek_anthropic_endpoint(
            ProviderType::DeepSeek,
            Some("https://api.deepseek.com/anthropic")
        ));
        assert_eq!(
            endpoint_id(ProviderType::OpenAi, Some("https://api.meta.ai/v1/")),
            "meta-model-api-public"
        );
        assert!(is_meta_model_api_endpoint(
            ProviderType::OpenAi,
            Some("https://api.meta.ai/v1")
        ));
        assert_eq!(
            endpoint_id(
                ProviderType::Qwen,
                Some("https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1")
            ),
            "token-plan-cn"
        );
        assert_eq!(
            endpoint_id(
                ProviderType::Qwen,
                Some("https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1")
            ),
            "token-plan-global"
        );
        for endpoint in [
            "https://open.bigmodel.cn/api/paas/v4",
            "https://api.z.ai/api/paas/v4",
        ] {
            assert!(is_zhipu_model_api_endpoint(
                ProviderType::Zhipu,
                Some(endpoint)
            ));
        }
        for endpoint in [
            "http://dashscope.aliyuncs.com/compatible-mode/v1",
            "https://dashscope.aliyuncs.com:8443/compatible-mode/v1",
            "https://dashscope.aliyuncs.com/apps/anthropic",
        ] {
            assert!(
                endpoint_id(ProviderType::AlibabaModelStudio, Some(endpoint))
                    .starts_with("custom-")
            );
            assert!(!is_alibaba_chat_endpoint(
                ProviderType::AlibabaModelStudio,
                Some(endpoint)
            ));
        }
    }

    #[test]
    fn public_profiles_reject_familiar_hosts_with_modified_boundaries() {
        let cases: &[(ProviderType, &str, fn(ProviderType, Option<&str>) -> bool)] = &[
            (
                ProviderType::OpenAi,
                "https://api.openai.com/internal",
                is_openai_public_endpoint,
            ),
            (
                ProviderType::OpenAi,
                "https://api.x.ai/v2",
                is_xai_public_endpoint,
            ),
            (
                ProviderType::OpenAi,
                "http://api.minimax.io/v1",
                is_minimax_public_endpoint,
            ),
            (
                ProviderType::OpenAi,
                "https://api.mistral.ai:8443/v1",
                is_mistral_public_endpoint,
            ),
            (
                ProviderType::OpenAi,
                "https://api.meta.ai/v2",
                is_meta_model_api_endpoint,
            ),
            (
                ProviderType::OpenRouter,
                "https://openrouter.ai/v2",
                is_openrouter_public_endpoint,
            ),
            (
                ProviderType::DeepSeek,
                "https://api.deepseek.com:8443/v1",
                is_deepseek_public_endpoint,
            ),
            (
                ProviderType::DeepSeek,
                "http://api.deepseek.com/anthropic",
                is_deepseek_anthropic_endpoint,
            ),
            (
                ProviderType::Moonshot,
                "http://api.moonshot.ai/v1",
                is_moonshot_public_endpoint,
            ),
            (
                ProviderType::Anthropic,
                "https://api.anthropic.com/v2",
                is_anthropic_public_endpoint,
            ),
            (
                ProviderType::Google,
                "https://generativelanguage.googleapis.com:8443/v1beta",
                is_google_public_endpoint,
            ),
            (
                ProviderType::SiliconFlow,
                "https://api.siliconflow.cn/compatible-mode/v1",
                is_siliconflow_public_endpoint,
            ),
        ];
        for (provider, endpoint, predicate) in cases {
            assert!(!predicate(*provider, Some(endpoint)), "accepted {endpoint}");
            assert!(endpoint_id(*provider, Some(endpoint)).starts_with("custom-"));
        }
    }
}
