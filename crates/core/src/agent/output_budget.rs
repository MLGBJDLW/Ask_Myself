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
    /// The internal reserve is always concrete so context trimming can remain
    /// deterministic. It is sent to the provider only when the user or a
    /// verified endpoint/model catalog is authoritative. Unknown/private
    /// routes keep the reserve local and let the provider enforce its own
    /// output boundary instead of inheriting an arbitrary Nexa cap.
    pub(super) fn wire_max_tokens(self) -> Option<u32> {
        match self.authority {
            OutputBudgetAuthority::AutomaticFallbackReserve => None,
            OutputBudgetAuthority::SavedExplicitOverride
            | OutputBudgetAuthority::VerifiedCatalogCapability => Some(
                self.catalog_cap
                    .into_iter()
                    .chain(self.context_cap)
                    .fold(self.requested_tokens, u32::min),
            ),
        }
    }

    pub(super) fn wire_max_tokens_for_prompt(self, prompt_tokens: u32) -> Option<u32> {
        self.wire_max_tokens().map(|limit| {
            self.context_cap.map_or(limit, |capacity| {
                limit.min(
                    capacity
                        .saturating_sub(prompt_tokens)
                        .saturating_sub(crate::conversation::memory::context_safety_buffer(
                            capacity,
                        ))
                        .max(1),
                )
            })
        })
    }

    pub(super) fn diagnostic(self) -> String {
        format!(
            "per-request output budget: requested={}, effective_reserve={}, wire_max={:?}, authority={}, catalog_cap={:?}, context_cap={:?}",
            self.requested_tokens,
            self.effective_tokens,
            self.wire_max_tokens(),
            self.authority.label(),
            self.catalog_cap,
            self.context_cap,
        )
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
        // A familiar model alias is not sufficient authority on an edited or
        // private endpoint. The host deliberately marks those routes as
        // provider-managed; borrowing the public catalog's output/context
        // limits could make the custom server reject the request outright.
        // `None` retains the legacy in-process/test path where no endpoint
        // resolution was supplied, while a resolved route must be catalog
        // authoritative before global catalog limits participate.
        let catalog_is_route_authoritative =
            self.catalog_limits_authoritative.unwrap_or_else(|| {
                self.context_window_resolution.is_none_or(|resolution| {
                    resolution.authority
                        == crate::conversation::memory::ContextWindowAuthority::Catalog
                })
            });
        let catalog_limits = catalog_is_route_authoritative
            .then(|| {
                self.provider_type.and_then(|provider| {
                    crate::provider_catalog::model_limits_from_catalog(provider, model)
                })
            })
            .flatten();
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
            .map(|capacity| capacity.max(1));
        // Reserve space before prompt construction without turning the local
        // input/output allocation into a provider output ceiling. The wire
        // limit uses the actual prompt headroom once that prompt exists.
        let reserve_cap = context_cap.map(|capacity| {
            if automatic {
                capacity.saturating_div(2).max(1)
            } else {
                capacity
            }
        });
        let effective_tokens = catalog_cap
            .into_iter()
            .chain(reserve_cap)
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
