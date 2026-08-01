use serde::{Deserialize, Serialize};

use super::{ModelCapabilities, ReasoningCapability};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityProbeStatus {
    Passed,
    Failed,
    NotRun,
}

/// Capability fields observed by a probe. Optional booleans distinguish an
/// explicit negative result from a field the probe did not test.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedModelCapabilities {
    #[serde(default)]
    pub reasoning: Option<ReasoningCapability>,
    #[serde(default)]
    pub vision: Option<bool>,
    #[serde(default)]
    pub audio_input: Option<bool>,
    #[serde(default)]
    pub audio_output: Option<bool>,
    #[serde(default)]
    pub video_input: Option<bool>,
    #[serde(default)]
    pub video_output: Option<bool>,
    #[serde(default)]
    pub tool_calling: Option<bool>,
    #[serde(default)]
    pub parallel_tool_calling: Option<bool>,
    #[serde(default)]
    pub structured_output: Option<bool>,
    #[serde(default)]
    pub image_generation: Option<bool>,
    #[serde(default)]
    pub image_editing: Option<bool>,
    #[serde(default)]
    pub multi_reference_editing: Option<bool>,
    #[serde(default)]
    pub realtime: Option<bool>,
    #[serde(default)]
    pub prompt_cache: Option<bool>,
    #[serde(default)]
    pub async_jobs: Option<bool>,
    #[serde(default)]
    pub batch: Option<bool>,
    #[serde(default)]
    pub dimension_override: Option<bool>,
}

impl VerifiedModelCapabilities {
    pub(crate) fn apply_to(&self, target: &mut ModelCapabilities) {
        if self.reasoning.is_some() {
            target.reasoning = self.reasoning.clone();
        }
        macro_rules! apply_bool {
            ($($field:ident),+ $(,)?) => {
                $(if let Some(value) = self.$field { target.$field = value; })+
            };
        }
        apply_bool!(
            vision,
            audio_input,
            audio_output,
            video_input,
            video_output,
            tool_calling,
            parallel_tool_calling,
            structured_output,
            image_generation,
            image_editing,
            multi_reference_editing,
            realtime,
            prompt_cache,
            async_jobs,
            batch,
            dimension_override,
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityProbeResult {
    pub model_id: String,
    pub endpoint_id: String,
    pub status: CapabilityProbeStatus,
    pub verified_at: String,
    #[serde(default)]
    pub capabilities: Option<VerifiedModelCapabilities>,
    #[serde(default)]
    pub failure_reason: Option<String>,
}

impl CapabilityProbeResult {
    pub fn passed(
        model_id: impl Into<String>,
        endpoint_id: impl Into<String>,
        verified_at: impl Into<String>,
    ) -> Self {
        Self {
            model_id: model_id.into(),
            endpoint_id: endpoint_id.into(),
            status: CapabilityProbeStatus::Passed,
            verified_at: verified_at.into(),
            capabilities: None,
            failure_reason: None,
        }
    }

    pub fn is_passed(&self) -> bool {
        self.status == CapabilityProbeStatus::Passed
    }
}
