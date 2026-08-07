//! Shared, evidence-backed video provider capability catalog.
//!
//! Provider/model release status is scoped to the exact API surface. An
//! aggregator contract never promotes an unverified direct-provider endpoint.

use serde::{Deserialize, Serialize};

use crate::media_generation::MediaOperation;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoModelReleaseStatus {
    Ga,
    Preview,
    Announced,
    ContractPending,
    Deprecated,
    Unverified,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoProviderPreset {
    pub id: String,
    pub name: String,
    pub provider_id: String,
    pub api_style: String,
    pub base_url: String,
    pub requires_api_key: bool,
    pub api_version: Option<String>,
    pub description: String,
    #[serde(default)]
    pub data_regions: Vec<String>,
    pub retention_policy: String,
    pub models: Vec<VideoModelManifest>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoModelManifest {
    pub provider_id: String,
    pub model_id: String,
    pub display_name: String,
    pub api_version: Option<String>,
    pub release_status: VideoModelReleaseStatus,
    pub selectable: bool,
    #[serde(default)]
    pub operation_capabilities: Vec<VideoOperationCapability>,
    pub supports_negative_prompt: bool,
    pub supports_webhook: bool,
    pub supports_cancellation: bool,
    pub cancellation_scope: String,
    pub cancellation_may_delete_terminal_record: bool,
    #[serde(default)]
    pub regions: Vec<String>,
    pub moderation_policy: String,
    pub pricing: VideoModelPricing,
    pub output_url_ttl: String,
    pub watermark_policy: String,
    pub provenance_policy: String,
    pub last_verified_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoOperationCapability {
    pub operation: MediaOperation,
    #[serde(default)]
    pub duration_options: Vec<VideoDurationOption>,
    #[serde(default)]
    pub aspect_ratios: Vec<String>,
    #[serde(default)]
    pub input_roles: Vec<String>,
    pub supports_audio: bool,
    pub supports_seed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoDurationOption {
    pub resolution: String,
    pub min_duration_seconds: Option<u32>,
    pub max_duration_seconds: Option<u32>,
    #[serde(default)]
    pub durations_seconds: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoModelPricing {
    pub currency: Option<String>,
    pub kind: String,
    pub credits_per_second: Option<u32>,
    pub micros_per_second: Option<u64>,
    pub minimum_amount_micros: Option<u64>,
    pub free_reference_images: Option<u32>,
    pub additional_reference_image_micros: Option<u64>,
    #[serde(default)]
    pub tiers: Vec<VideoPriceTier>,
    #[serde(default)]
    pub input_video_tiers: Vec<VideoPriceTier>,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoPriceTier {
    pub resolution: String,
    pub duration_seconds: Option<u32>,
    pub amount_micros: Option<u64>,
    pub micros_per_second: Option<u64>,
}

const VIDEO_PROVIDER_PRESETS_JSON: &str =
    include_str!("../../../shared/video-provider-presets.json");

pub fn load_video_provider_presets() -> Result<Vec<VideoProviderPreset>, serde_json::Error> {
    serde_json::from_str(VIDEO_PROVIDER_PRESETS_JSON)
}

pub fn find_video_provider_preset(
    provider_id: &str,
    api_style: &str,
    base_url: &str,
) -> Option<VideoProviderPreset> {
    let normalized = normalize_base_url(base_url)?;
    load_video_provider_presets()
        .ok()?
        .into_iter()
        .find(|preset| {
            preset.provider_id == provider_id.trim()
                && preset.api_style == api_style.trim()
                && normalize_base_url(&preset.base_url).as_deref() == Some(normalized.as_str())
        })
}

fn normalize_base_url(value: &str) -> Option<String> {
    let mut url = url::Url::parse(value.trim()).ok()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(&path);
    Some(url.to_string().trim_end_matches('/').to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_status_is_scoped_to_provider_contract() {
        let presets = load_video_provider_presets().expect("video presets should parse");
        let minimax_h3 = presets
            .iter()
            .find(|preset| preset.provider_id == "minimax")
            .and_then(|preset| {
                preset
                    .models
                    .iter()
                    .find(|model| model.model_id == "MiniMax-H3")
            })
            .expect("MiniMax H3 should be present");
        assert_eq!(minimax_h3.release_status, VideoModelReleaseStatus::Ga);
        assert!(minimax_h3.selectable);

        let runway_seedance = presets
            .iter()
            .find(|preset| preset.provider_id == "runway")
            .and_then(|preset| {
                preset
                    .models
                    .iter()
                    .find(|model| model.model_id == "seedance2_5")
            })
            .expect("Runway Seedance 2.5 should be present");
        assert_eq!(runway_seedance.release_status, VideoModelReleaseStatus::Ga);
        assert!(runway_seedance.selectable);

        let direct_seedance = presets
            .iter()
            .find(|preset| preset.provider_id == "bytedance")
            .and_then(|preset| preset.models.first())
            .expect("direct-provider watchlist should be present");
        assert_eq!(
            direct_seedance.release_status,
            VideoModelReleaseStatus::Unverified
        );
        assert!(!direct_seedance.selectable);
    }

    #[test]
    fn trusted_provider_lookup_requires_exact_official_endpoint() {
        assert!(find_video_provider_preset(
            "minimax",
            "minimax_video_v2",
            "https://api.minimax.io/"
        )
        .is_some());
        assert!(find_video_provider_preset(
            "minimax",
            "minimax_video_v2",
            "https://api.minimax.io.evil.example"
        )
        .is_none());
        assert!(find_video_provider_preset(
            "minimax",
            "minimax_video_v2",
            "https://api.minimax.io?token=secret"
        )
        .is_none());
    }
}
