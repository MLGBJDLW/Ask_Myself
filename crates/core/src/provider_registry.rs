//! Provider registry for key parsing and adapter selection.
//!
//! The provider catalog owns model metadata and capabilities. This registry
//! owns stable provider identifiers, aliases, and the adapter implementation
//! each provider should use.

use crate::llm::ProviderType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderAdapterKind {
    OpenAiCompatible,
    Anthropic,
    Google,
    Ollama,
}

#[derive(Debug, Clone, Copy)]
pub struct ProviderRegistryEntry {
    pub provider_type: ProviderType,
    pub canonical_key: &'static str,
    pub aliases: &'static [&'static str],
    pub adapter: ProviderAdapterKind,
}

const PROVIDER_REGISTRY: &[ProviderRegistryEntry] = &[
    ProviderRegistryEntry {
        provider_type: ProviderType::OpenAi,
        canonical_key: "open_ai",
        aliases: &["openai", "open_ai"],
        adapter: ProviderAdapterKind::OpenAiCompatible,
    },
    ProviderRegistryEntry {
        provider_type: ProviderType::OpenRouter,
        canonical_key: "openrouter",
        aliases: &["openrouter", "open_router"],
        adapter: ProviderAdapterKind::OpenAiCompatible,
    },
    ProviderRegistryEntry {
        provider_type: ProviderType::Anthropic,
        canonical_key: "anthropic",
        aliases: &["anthropic"],
        adapter: ProviderAdapterKind::Anthropic,
    },
    ProviderRegistryEntry {
        provider_type: ProviderType::Google,
        canonical_key: "google",
        aliases: &["google", "gemini"],
        adapter: ProviderAdapterKind::Google,
    },
    ProviderRegistryEntry {
        provider_type: ProviderType::DeepSeek,
        canonical_key: "deep_seek",
        aliases: &["deepseek", "deep_seek"],
        adapter: ProviderAdapterKind::OpenAiCompatible,
    },
    ProviderRegistryEntry {
        provider_type: ProviderType::Ollama,
        canonical_key: "ollama",
        aliases: &["ollama"],
        adapter: ProviderAdapterKind::Ollama,
    },
    ProviderRegistryEntry {
        provider_type: ProviderType::LmStudio,
        canonical_key: "lm_studio",
        aliases: &["lmstudio", "lm_studio"],
        adapter: ProviderAdapterKind::OpenAiCompatible,
    },
    ProviderRegistryEntry {
        provider_type: ProviderType::AzureOpenAi,
        canonical_key: "azure_open_ai",
        aliases: &["azure", "azure_openai", "azure_open_ai", "azureopenai"],
        adapter: ProviderAdapterKind::OpenAiCompatible,
    },
    ProviderRegistryEntry {
        provider_type: ProviderType::Zhipu,
        canonical_key: "zhipu",
        aliases: &["zhipu", "glm"],
        adapter: ProviderAdapterKind::OpenAiCompatible,
    },
    ProviderRegistryEntry {
        provider_type: ProviderType::Moonshot,
        canonical_key: "moonshot",
        aliases: &["moonshot", "kimi"],
        adapter: ProviderAdapterKind::OpenAiCompatible,
    },
    ProviderRegistryEntry {
        provider_type: ProviderType::Qwen,
        canonical_key: "qwen",
        aliases: &["qwen", "tongyi"],
        adapter: ProviderAdapterKind::OpenAiCompatible,
    },
    ProviderRegistryEntry {
        provider_type: ProviderType::Doubao,
        canonical_key: "doubao",
        aliases: &["doubao"],
        adapter: ProviderAdapterKind::OpenAiCompatible,
    },
    ProviderRegistryEntry {
        provider_type: ProviderType::Yi,
        canonical_key: "yi",
        aliases: &["yi", "lingyiwanwu"],
        adapter: ProviderAdapterKind::OpenAiCompatible,
    },
    ProviderRegistryEntry {
        provider_type: ProviderType::Baichuan,
        canonical_key: "baichuan",
        aliases: &["baichuan"],
        adapter: ProviderAdapterKind::OpenAiCompatible,
    },
    ProviderRegistryEntry {
        provider_type: ProviderType::Custom,
        canonical_key: "custom",
        aliases: &["custom"],
        adapter: ProviderAdapterKind::OpenAiCompatible,
    },
];

pub fn provider_registry_entries() -> &'static [ProviderRegistryEntry] {
    PROVIDER_REGISTRY
}

pub fn provider_type_from_key(key: &str) -> Option<ProviderType> {
    let normalized = normalize_provider_key(key);
    PROVIDER_REGISTRY
        .iter()
        .find(|entry| {
            entry.canonical_key == normalized
                || entry.aliases.iter().any(|alias| *alias == normalized)
        })
        .map(|entry| entry.provider_type)
}

pub fn provider_type_for_parts(provider: &str, base_url: Option<&str>) -> ProviderType {
    let base_url_lower = base_url.unwrap_or_default().to_ascii_lowercase();
    if base_url_lower.contains("deepseek") {
        return ProviderType::DeepSeek;
    }
    if base_url_lower.contains("openrouter.ai") {
        return ProviderType::OpenRouter;
    }

    provider_type_from_key(provider).unwrap_or(ProviderType::Custom)
}

pub fn provider_adapter_for_type(provider_type: ProviderType) -> ProviderAdapterKind {
    PROVIDER_REGISTRY
        .iter()
        .find(|entry| entry.provider_type == provider_type)
        .map(|entry| entry.adapter)
        .unwrap_or(ProviderAdapterKind::OpenAiCompatible)
}

fn normalize_provider_key(provider: &str) -> String {
    provider.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_maps_keys_aliases_and_adapters() {
        assert_eq!(
            provider_type_from_key("open_ai"),
            Some(ProviderType::OpenAi)
        );
        assert_eq!(provider_type_from_key("gemini"), Some(ProviderType::Google));
        assert_eq!(
            provider_type_for_parts("custom", Some("https://api.deepseek.com")),
            ProviderType::DeepSeek
        );
        assert_eq!(
            provider_type_for_parts("open_ai", Some("https://api.deepseek.com")),
            ProviderType::DeepSeek
        );
        assert_eq!(
            provider_type_from_key("open_router"),
            Some(ProviderType::OpenRouter)
        );
        assert_eq!(
            provider_type_for_parts("custom", Some("https://openrouter.ai/api/v1")),
            ProviderType::OpenRouter
        );
        assert_eq!(
            provider_type_for_parts("open_ai", Some("https://openrouter.ai/api/v1")),
            ProviderType::OpenRouter
        );
        assert_eq!(
            provider_adapter_for_type(ProviderType::Qwen),
            ProviderAdapterKind::OpenAiCompatible
        );
        assert_eq!(
            provider_adapter_for_type(ProviderType::Anthropic),
            ProviderAdapterKind::Anthropic
        );
    }
}
