//! Client-side orchestration quality profiles.
//!
//! These profiles tune the Nexa runtime. They are deliberately separate from
//! provider reasoning effort, which must remain limited to values the selected
//! model actually supports.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OrchestrationProfile {
    #[default]
    Balanced,
    Deep,
    CodeUltra,
    ResearchUltra,
    Custom,
}

impl OrchestrationProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Balanced => "balanced",
            Self::Deep => "deep",
            Self::CodeUltra => "codeUltra",
            Self::ResearchUltra => "researchUltra",
            Self::Custom => "custom",
        }
    }

    pub fn from_wire(value: Option<&str>) -> Result<Self, String> {
        match value.map(str::trim).filter(|value| !value.is_empty()) {
            None => Ok(Self::Balanced),
            Some(value) if value.eq_ignore_ascii_case("balanced") => Ok(Self::Balanced),
            Some(value) if value.eq_ignore_ascii_case("deep") => Ok(Self::Deep),
            Some(value) if value.eq_ignore_ascii_case("codeUltra") => Ok(Self::CodeUltra),
            Some(value) if value.eq_ignore_ascii_case("researchUltra") => Ok(Self::ResearchUltra),
            Some(value) if value.eq_ignore_ascii_case("custom") => Ok(Self::Custom),
            Some(value) => Err(format!("Unsupported orchestration profile '{value}'.")),
        }
    }

    pub fn is_ultra(self) -> bool {
        matches!(self, Self::CodeUltra | Self::ResearchUltra)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CustomOrchestrationOptions {
    pub max_iterations: Option<u32>,
    pub max_parallel: Option<u32>,
    pub max_calls_per_turn: Option<u32>,
    pub delegated_token_budget: Option<u32>,
    pub verification_reserve_percent: Option<u32>,
    pub retry_limit: Option<u8>,
    pub min_evidence_sources: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedOrchestrationProfile {
    pub profile: OrchestrationProfile,
    pub max_iterations: u32,
    pub max_parallel: u32,
    pub max_calls_per_turn: u32,
    pub delegated_token_budget: u32,
    pub verification_reserve_percent: u32,
    pub retry_limit: u8,
    pub min_evidence_sources: u8,
    pub require_independent_verifier: bool,
    pub require_isolated_writes: bool,
}

impl ResolvedOrchestrationProfile {
    pub fn prompt_section(&self) -> String {
        format!(
            "## Orchestration Quality Profile\n\n\
             The user selected `{}`. This is a client-side execution policy, not a provider reasoning-effort value.\n\
             - Runtime retry limit: {}. Verification reserve: {}%. Minimum evidence sources: {}.\n\
             - Independent verifier required: {}. Isolated write scopes required: {}.\n\
             - Do not claim the model received an `ultra` reasoning parameter. Provider reasoning controls remain independently capability-gated.",
            self.profile.as_str(),
            self.retry_limit,
            self.verification_reserve_percent,
            self.min_evidence_sources,
            self.require_independent_verifier,
            self.require_isolated_writes,
        )
    }
}

#[derive(Debug, Clone)]
pub struct OrchestrationProfileInput {
    pub profile: OrchestrationProfile,
    pub custom: Option<CustomOrchestrationOptions>,
    pub max_iterations: u32,
    pub max_parallel: Option<u32>,
    pub max_calls_per_turn: Option<u32>,
    pub delegated_token_budget: Option<u32>,
    pub verification_reserve_percent: Option<u32>,
}

pub fn resolve_orchestration_profile(
    input: OrchestrationProfileInput,
) -> ResolvedOrchestrationProfile {
    let defaults = match input.profile {
        OrchestrationProfile::Balanced => (
            input.max_iterations,
            input.max_parallel.unwrap_or(2),
            input.max_calls_per_turn.unwrap_or(4),
            input.delegated_token_budget.unwrap_or(24_000),
            input.verification_reserve_percent.unwrap_or(0),
            1,
            1,
            false,
            false,
        ),
        OrchestrationProfile::Deep => (32, 4, 8, 64_000, 30, 2, 2, true, false),
        OrchestrationProfile::CodeUltra => (56, 6, 12, 96_000, 30, 3, 1, true, true),
        OrchestrationProfile::ResearchUltra => (56, 6, 12, 96_000, 35, 3, 3, true, false),
        OrchestrationProfile::Custom => {
            let custom = input.custom.as_ref().cloned().unwrap_or_default();
            (
                custom.max_iterations.unwrap_or(24).clamp(4, 96),
                custom.max_parallel.unwrap_or(3).clamp(1, 8),
                custom.max_calls_per_turn.unwrap_or(6).clamp(1, 24),
                custom
                    .delegated_token_budget
                    .unwrap_or(48_000)
                    .clamp(4_096, 192_000),
                custom
                    .verification_reserve_percent
                    .unwrap_or(25)
                    .clamp(10, 50),
                custom.retry_limit.unwrap_or(2).clamp(0, 5),
                custom.min_evidence_sources.unwrap_or(2).clamp(0, 8),
                true,
                false,
            )
        }
    };

    ResolvedOrchestrationProfile {
        profile: input.profile,
        max_iterations: input.max_iterations.max(defaults.0),
        max_parallel: input.max_parallel.unwrap_or_default().max(defaults.1),
        max_calls_per_turn: input.max_calls_per_turn.unwrap_or_default().max(defaults.2),
        delegated_token_budget: input
            .delegated_token_budget
            .unwrap_or_default()
            .max(defaults.3),
        verification_reserve_percent: input
            .verification_reserve_percent
            .unwrap_or_default()
            .max(defaults.4)
            .min(50),
        retry_limit: defaults.5,
        min_evidence_sources: defaults.6,
        require_independent_verifier: defaults.7,
        require_isolated_writes: defaults.8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(profile: OrchestrationProfile) -> OrchestrationProfileInput {
        OrchestrationProfileInput {
            profile,
            custom: None,
            max_iterations: 20,
            max_parallel: None,
            max_calls_per_turn: None,
            delegated_token_budget: None,
            verification_reserve_percent: None,
        }
    }

    #[test]
    fn ultra_is_an_orchestration_policy_not_a_provider_effort() {
        let profile = resolve_orchestration_profile(input(OrchestrationProfile::CodeUltra));
        assert_eq!(profile.max_iterations, 56);
        assert!(profile.require_isolated_writes);
        assert!(profile
            .prompt_section()
            .contains("not a provider reasoning-effort"));
        assert!(OrchestrationProfile::from_wire(Some("ultra")).is_err());
    }

    #[test]
    fn custom_values_are_bounded() {
        let profile = resolve_orchestration_profile(OrchestrationProfileInput {
            profile: OrchestrationProfile::Custom,
            custom: Some(CustomOrchestrationOptions {
                max_parallel: Some(99),
                verification_reserve_percent: Some(1),
                ..Default::default()
            }),
            ..input(OrchestrationProfile::Custom)
        });
        assert_eq!(profile.max_parallel, 8);
        assert_eq!(profile.verification_reserve_percent, 10);
    }
}
