use serde::{Deserialize, Serialize};

use super::ModelCapabilities;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityProbeStatus {
    Passed,
    Failed,
    NotRun,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityProbeResult {
    pub model_id: String,
    pub endpoint_id: String,
    pub status: CapabilityProbeStatus,
    pub verified_at: String,
    #[serde(default)]
    pub capabilities: Option<ModelCapabilities>,
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
