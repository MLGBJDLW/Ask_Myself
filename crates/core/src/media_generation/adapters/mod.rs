//! Strict provider transports for asynchronous video generation.
//!
//! Adapters translate provider contracts into normalized observations. They do
//! not own retry policy, durable state, lineage, or database access; callers
//! persist their outputs through [`super::MediaGenerationRuntime`].

mod http;
mod minimax;
mod minimax_hailuo;
mod runway;

use std::fmt;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::media_generation::MediaOperation;
use crate::video_provider_catalog::VideoModelManifest;

pub use minimax::MiniMaxVideoAdapter;
pub use minimax_hailuo::MiniMaxHailuoVideoAdapter;
pub use runway::RunwayVideoAdapter;

const MAX_PROMPT_CHARS: usize = 15_000;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoInputRole {
    FirstFrame,
    LastFrame,
    InputVideo,
    ReferenceImage,
    ReferenceVideo,
    ReferenceAudio,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoInputAsset {
    pub role: VideoInputRole,
    pub uri: String,
    pub media_type: String,
    /// True only when the following size/dimension/duration fields came from
    /// Nexa's verified local asset record or a completed provider upload.
    #[serde(default)]
    pub metadata_verified: bool,
    pub byte_length: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
    pub frame_rate: Option<f64>,
    pub video_codec: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedVideoRequest {
    pub idempotency_key: String,
    pub model_id: String,
    pub operation: MediaOperation,
    pub prompt: String,
    pub duration_seconds: u32,
    pub resolution: String,
    pub aspect_ratio: String,
    #[serde(default)]
    pub input_assets: Vec<VideoInputAsset>,
    pub seed: Option<u32>,
    pub generate_audio: Option<bool>,
    pub callback_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationIssue {
    pub field: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationResult {
    pub valid: bool,
    pub issues: Vec<ValidationIssue>,
}

impl ValidationResult {
    fn new(issues: Vec<ValidationIssue>) -> Self {
        Self {
            valid: issues.is_empty(),
            issues,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostEstimateKind {
    Exact,
    Estimated,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostEstimate {
    pub kind: CostEstimateKind,
    pub amount_micros: Option<u64>,
    pub currency: Option<String>,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmittedJob {
    pub provider_task_id: String,
    pub provider_source: String,
    pub estimated_cost: CostEstimate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderJobState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    ProviderUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderOutputLocator {
    pub uri: String,
    pub media_type: String,
    pub expires_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderJobResult {
    pub provider_task_id: String,
    pub outputs: Vec<ProviderOutputLocator>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderJobStatus {
    pub provider_task_id: String,
    pub state: ProviderJobState,
    pub raw_status: String,
    pub result: Option<ProviderJobResult>,
    pub error: Option<NormalizedProviderError>,
    pub billed_usage: Option<ProviderBilledUsage>,
    pub final_cost_micros: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBilledUsage {
    pub total_seconds: Option<u64>,
    pub input_seconds: Option<u64>,
    pub output_seconds: Option<u64>,
    pub input_image_count: Option<u64>,
    pub credits: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancellationResult {
    pub provider_task_id: String,
    pub confirmed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadedAsset {
    pub path: PathBuf,
    pub declared_media_type: String,
    pub detected_media_type: String,
    pub byte_length: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedProviderError {
    pub provider_id: String,
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub retry_after_seconds: Option<u64>,
    pub http_status: Option<u16>,
    pub request_id: Option<String>,
}

impl fmt::Display for NormalizedProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for NormalizedProviderError {}

#[async_trait]
pub trait VideoGenerationAdapter: Send + Sync {
    fn provider_id(&self) -> &'static str;
    fn provider_source(&self) -> &str;
    fn get_capabilities(&self) -> Vec<VideoModelManifest>;
    fn validate(&self, request: &NormalizedVideoRequest) -> ValidationResult;
    async fn estimate_cost(
        &self,
        request: &NormalizedVideoRequest,
    ) -> Result<CostEstimate, NormalizedProviderError>;
    async fn submit(
        &self,
        request: &NormalizedVideoRequest,
    ) -> Result<SubmittedJob, NormalizedProviderError>;
    async fn get_status(
        &self,
        provider_task_id: &str,
    ) -> Result<ProviderJobStatus, NormalizedProviderError>;
    async fn cancel(
        &self,
        provider_task_id: &str,
    ) -> Result<CancellationResult, NormalizedProviderError>;
    async fn download_outputs(
        &self,
        result: &ProviderJobResult,
        destination_directory: &Path,
        max_total_bytes: u64,
    ) -> Result<Vec<DownloadedAsset>, NormalizedProviderError>;
    fn normalize_error(
        &self,
        error: &(dyn std::error::Error + Send + Sync),
    ) -> NormalizedProviderError;
}

fn common_validation(request: &NormalizedVideoRequest) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    if request.idempotency_key.trim().is_empty()
        || request.idempotency_key.len() > MAX_IDEMPOTENCY_KEY_BYTES
    {
        issues.push(issue(
            "idempotencyKey",
            "invalid_idempotency_key",
            "Idempotency key must contain 1-512 bytes",
        ));
    }
    let prompt_chars = request.prompt.chars().count();
    if request.prompt.trim().is_empty() || prompt_chars > MAX_PROMPT_CHARS {
        issues.push(issue(
            "prompt",
            "invalid_prompt",
            "Prompt must contain 1-15000 characters",
        ));
    }
    if request.model_id.trim().is_empty() || request.model_id.len() > 128 {
        issues.push(issue(
            "modelId",
            "invalid_model",
            "Model ID must contain 1-128 bytes",
        ));
    }
    for (index, asset) in request.input_assets.iter().enumerate() {
        if !asset.metadata_verified {
            issues.push(issue(
                &format!("inputAssets[{index}]"),
                "unverified_input_metadata",
                "Provider submission requires verified input asset metadata",
            ));
        }
        if !valid_media_uri(&asset.uri) {
            issues.push(issue(
                &format!("inputAssets[{index}].uri"),
                "invalid_media_uri",
                "Media inputs must use HTTPS or a provider-owned media URI",
            ));
        }
        if !asset.media_type.contains('/') || asset.media_type.len() > 128 {
            issues.push(issue(
                &format!("inputAssets[{index}].mediaType"),
                "invalid_media_type",
                "Media type must be a bounded MIME type",
            ));
        }
    }
    if let Some(callback_url) = &request.callback_url {
        if !valid_public_https_url(callback_url) {
            issues.push(issue(
                "callbackUrl",
                "invalid_callback_url",
                "Callback URL must be public HTTPS without credentials, query, or fragment",
            ));
        }
    }
    issues
}

fn issue(field: &str, code: &str, message: &str) -> ValidationIssue {
    ValidationIssue {
        field: field.to_string(),
        code: code.to_string(),
        message: message.to_string(),
    }
}

fn valid_media_uri(value: &str) -> bool {
    if value.starts_with("mm_file://") || value.starts_with("runway://") {
        return value.len() > 12 && value.len() <= 5000;
    }
    valid_public_https_media_url(value)
}

fn valid_public_https_url(value: &str) -> bool {
    valid_public_https_url_with_query_policy(value, false)
}

fn valid_public_https_media_url(value: &str) -> bool {
    valid_public_https_url_with_query_policy(value, true)
}

fn valid_public_https_url_with_query_policy(value: &str, allow_query: bool) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || (!allow_query && url.query().is_some())
        || url.fragment().is_some()
    {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") {
        return false;
    }
    host.parse::<std::net::IpAddr>()
        .map(|address| !is_private_address(address))
        .unwrap_or(true)
}

fn is_private_address(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(address) => {
            address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address.is_unspecified()
                || address.is_broadcast()
        }
        std::net::IpAddr::V6(address) => {
            address.is_loopback() || address.is_unspecified() || address.is_unique_local()
        }
    }
}

fn provider_source(
    provider_id: &str,
    base_url: &url::Url,
    credential_scope: &str,
    contract_scope: &str,
) -> String {
    let endpoint = format!(
        "{}://{}:{}",
        base_url.scheme(),
        base_url.host_str().unwrap_or("unknown"),
        base_url.port_or_known_default().unwrap_or(0)
    );
    let endpoint_hash = hex_sha256(endpoint.as_bytes());
    let credential_hash = hex_sha256(credential_scope.trim().as_bytes());
    format!(
        "urn:nexa:video:{provider_id}:endpoint-{}:account-{}:contract-{}:provider-managed",
        &endpoint_hash[..16],
        &credential_hash[..16],
        contract_scope
            .chars()
            .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
            .collect::<String>()
    )
}

fn hex_sha256(value: &[u8]) -> String {
    let digest = Sha256::digest(value);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn invalid_request_error(
    provider_id: &str,
    validation: ValidationResult,
) -> NormalizedProviderError {
    let message = validation
        .issues
        .iter()
        .map(|entry| format!("{}: {}", entry.field, entry.message))
        .collect::<Vec<_>>()
        .join("; ");
    NormalizedProviderError {
        provider_id: provider_id.to_string(),
        code: "invalid_request".to_string(),
        message,
        retryable: false,
        retry_after_seconds: None,
        http_status: None,
        request_id: None,
    }
}

fn submission_error(mut error: NormalizedProviderError) -> NormalizedProviderError {
    let ambiguous = matches!(error.code.as_str(), "transport_timeout" | "transport_error")
        || error
            .http_status
            .is_some_and(|status| status == 408 || status >= 500);
    if ambiguous {
        error.code = "submission_outcome_unknown".to_string();
        error.message = format!(
            "Provider may have accepted the generation request; reconcile the attempt before any resubmission. {}",
            error.message
        );
        error.retryable = false;
    }
    error
}

fn find_capabilities(provider_id: &str) -> Vec<VideoModelManifest> {
    crate::video_provider_catalog::load_video_provider_presets()
        .unwrap_or_default()
        .into_iter()
        .filter(|preset| preset.provider_id == provider_id)
        .flat_map(|preset| preset.models)
        .collect()
}

fn pricing_estimate(
    provider_id: &str,
    request: &NormalizedVideoRequest,
) -> Result<CostEstimate, NormalizedProviderError> {
    let manifest = find_capabilities(provider_id)
        .into_iter()
        .find(|candidate| candidate.model_id == request.model_id)
        .ok_or_else(|| NormalizedProviderError {
            provider_id: provider_id.to_string(),
            code: "unsupported_model".to_string(),
            message: "Model is not present in the verified provider manifest".to_string(),
            retryable: false,
            retry_after_seconds: None,
            http_status: None,
            request_id: None,
        })?;
    let tier = manifest.pricing.tiers.iter().find(|tier| {
        tier.resolution == request.resolution
            && tier
                .duration_seconds
                .is_none_or(|duration| duration == request.duration_seconds)
    });
    let tier_amount = tier.and_then(|tier| {
        tier.amount_micros.or_else(|| {
            tier.micros_per_second
                .and_then(|value| value.checked_mul(u64::from(request.duration_seconds)))
        })
    });
    match tier_amount.or_else(|| {
        manifest
            .pricing
            .micros_per_second
            .and_then(|value| value.checked_mul(u64::from(request.duration_seconds)))
    }) {
        Some(amount_micros) => Ok(CostEstimate {
            kind: CostEstimateKind::Exact,
            amount_micros: Some(
                amount_micros.max(manifest.pricing.minimum_amount_micros.unwrap_or(0)),
            ),
            currency: manifest.pricing.currency,
            note: manifest.pricing.note,
        }),
        None => Ok(CostEstimate {
            kind: CostEstimateKind::Unavailable,
            amount_micros: None,
            currency: manifest.pricing.currency,
            note: manifest.pricing.note,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambiguous_submission_errors_are_never_retryable() {
        let timeout = NormalizedProviderError {
            provider_id: "provider".to_string(),
            code: "transport_timeout".to_string(),
            message: "response timed out".to_string(),
            retryable: true,
            retry_after_seconds: None,
            http_status: None,
            request_id: None,
        };
        let normalized = submission_error(timeout);
        assert_eq!(normalized.code, "submission_outcome_unknown");
        assert!(!normalized.retryable);

        let throttled = NormalizedProviderError {
            provider_id: "provider".to_string(),
            code: "http_429".to_string(),
            message: "rate limited".to_string(),
            retryable: true,
            retry_after_seconds: Some(5),
            http_status: Some(429),
            request_id: None,
        };
        let normalized = submission_error(throttled);
        assert_eq!(normalized.code, "http_429");
        assert!(normalized.retryable);
    }
}
