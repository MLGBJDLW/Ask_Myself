//! Provider-aware per-sample output budgeting with explicit provenance.

use super::*;

pub(super) const FALLBACK_AGENT_RESPONSE_TOKENS: u32 = 16_384;
pub(super) const FALLBACK_DEEPSEEK_RESPONSE_TOKENS: u32 = 32_768;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OutputBudgetAuthority {
    SavedExplicitOverride,
    VerifiedCatalogCapability,
    AutomaticFallbackReserve,
}

impl OutputBudgetAuthority {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::SavedExplicitOverride => "saved explicit per-request setting",
            Self::VerifiedCatalogCapability => "verified model catalog capability",
            Self::AutomaticFallbackReserve => "unknown-model automatic fallback reserve",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OutputBudgetPlan {
    pub(super) requested_tokens: u32,
    pub(super) effective_tokens: u32,
    pub(super) authority: OutputBudgetAuthority,
    pub(super) catalog_cap: Option<u32>,
    pub(super) context_cap: Option<u32>,
}

impl OutputBudgetPlan {
    pub(super) fn diagnostic(self) -> String {
        format!(
            "per-request output budget: requested={}, effective={}, authority={}, catalog_cap={:?}, context_cap={:?}",
            self.requested_tokens,
            self.effective_tokens,
            self.authority.label(),
            self.catalog_cap,
            self.context_cap,
        )
    }

    /// Conservative advisory for JSON-wrapped UTF-8 text tool arguments. This
    /// is not an execution limit: it gives the model a concrete chunk target
    /// after a truncation while leaving room for JSON escaping and call prose.
    pub(super) fn recommended_text_tool_chunk_chars(self, step_cap: u32) -> u32 {
        step_cap.saturating_mul(2).clamp(2_048, 32_768)
    }
}

fn is_deepseek_model_family(model: &str) -> bool {
    model.trim().to_ascii_lowercase().split('/').any(|segment| {
        segment == "deepseek"
            || segment.starts_with("deepseek-")
            || segment.starts_with("deepseek_")
            || segment.starts_with("deepseek.")
    })
}

impl AgentConfig {
    /// Resolve one physical provider sample. This is not a turn, tool-round,
    /// continuation, or cumulative run budget. Explicit saved configuration
    /// remains authoritative; automatic reserves are model-aware and every
    /// result is clamped only by verified catalog/context capacity.
    pub(super) fn resolved_output_budget(&self, model: &str) -> OutputBudgetPlan {
        let automatic = self.max_tokens.is_none();
        let fallback = if self.provider_type == Some(ProviderType::DeepSeek)
            || is_deepseek_model_family(model)
        {
            FALLBACK_DEEPSEEK_RESPONSE_TOKENS
        } else {
            FALLBACK_AGENT_RESPONSE_TOKENS
        };
        let catalog_limits = self.provider_type.and_then(|provider| {
            crate::provider_catalog::model_limits_from_catalog(provider, model)
        });
        let catalog_cap = catalog_limits
            .as_ref()
            .and_then(|limits| limits.max_output_tokens)
            .and_then(|tokens| u32::try_from(tokens).ok());
        let (requested_tokens, authority) = match (self.max_tokens, catalog_cap) {
            (Some(explicit), _) => (explicit, OutputBudgetAuthority::SavedExplicitOverride),
            (None, Some(catalog)) => (catalog, OutputBudgetAuthority::VerifiedCatalogCapability),
            (None, None) => (fallback, OutputBudgetAuthority::AutomaticFallbackReserve),
        };
        let context_cap = self
            .context_window_resolution
            .and_then(|resolution| resolution.capacity_tokens)
            .or(self.context_window)
            .or_else(|| {
                catalog_limits
                    .as_ref()
                    .and_then(|limits| limits.context_tokens)
                    .and_then(|tokens| u32::try_from(tokens).ok())
            })
            .map(|capacity| {
                if automatic {
                    capacity.saturating_div(2).max(1)
                } else {
                    capacity.max(1)
                }
            });
        let effective_tokens = catalog_cap
            .into_iter()
            .chain(context_cap)
            .fold(requested_tokens, u32::min);

        OutputBudgetPlan {
            requested_tokens,
            effective_tokens,
            authority,
            catalog_cap,
            context_cap,
        }
    }

    pub(super) fn resolved_max_response_tokens(&self, model: &str) -> u32 {
        self.resolved_output_budget(model).effective_tokens
    }
}
