use std::path::Path;

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    common_validation, find_capabilities, http, invalid_request_error, issue, pricing_estimate,
    provider_source, CancellationResult, CostEstimate, DownloadedAsset, NormalizedProviderError,
    NormalizedVideoRequest, ProviderJobResult, ProviderJobState, ProviderJobStatus,
    ProviderOutputLocator, SubmittedJob, ValidationResult, VideoGenerationAdapter, VideoInputRole,
};
use crate::media_generation::MediaOperation;
use crate::video_provider_catalog::VideoModelManifest;

const PROVIDER_ID: &str = "minimax";
const OFFICIAL_BASE_URL: &str = "https://api.minimax.io";
const MODEL_ID: &str = "MiniMax-H3";

#[derive(Clone)]
pub struct MiniMaxVideoAdapter {
    client: reqwest::Client,
    base_url: url::Url,
    api_key: String,
    provider_source: String,
    allow_insecure_http: bool,
}

impl MiniMaxVideoAdapter {
    pub fn new(
        api_key: impl Into<String>,
        credential_scope: &str,
    ) -> Result<Self, NormalizedProviderError> {
        Self::build(api_key.into(), credential_scope, OFFICIAL_BASE_URL, false)
    }

    fn build(
        api_key: String,
        credential_scope: &str,
        base_url: &str,
        allow_insecure_http: bool,
    ) -> Result<Self, NormalizedProviderError> {
        validate_secret_and_scope(&api_key, credential_scope)?;
        let base_url =
            url::Url::parse(base_url).map_err(|_| configuration_error("Invalid base URL"))?;
        if (!allow_insecure_http && base_url.as_str().trim_end_matches('/') != OFFICIAL_BASE_URL)
            || (allow_insecure_http && !matches!(base_url.scheme(), "http" | "https"))
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err(configuration_error(
                "MiniMax credentials may only be used with the exact official endpoint",
            ));
        }
        Ok(Self {
            client: http::client()?,
            provider_source: provider_source(PROVIDER_ID, &base_url, credential_scope, "v2"),
            base_url,
            api_key,
            allow_insecure_http,
        })
    }

    #[cfg(test)]
    fn for_test(base_url: &str) -> Self {
        Self::build("test-secret".to_string(), "test-credential", base_url, true)
            .expect("test adapter")
    }

    fn endpoint(&self, path: &str) -> url::Url {
        self.base_url.join(path).expect("static MiniMax endpoint")
    }

    fn authenticated(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .header(AUTHORIZATION, format!("Bearer {}", self.api_key))
            .header(CONTENT_TYPE, "application/json")
    }

    fn content(request: &NormalizedVideoRequest) -> Vec<Value> {
        let mut content = vec![json!({ "type": "text", "text": request.prompt })];
        content.extend(request.input_assets.iter().map(|asset| {
            let (kind, field, role) = match asset.role {
                VideoInputRole::FirstFrame => ("image_url", "image_url", "first_frame"),
                VideoInputRole::LastFrame => ("image_url", "image_url", "last_frame"),
                VideoInputRole::ReferenceImage => ("image_url", "image_url", "reference_image"),
                VideoInputRole::InputVideo | VideoInputRole::ReferenceVideo => {
                    ("video_url", "video_url", "reference_video")
                }
                VideoInputRole::ReferenceAudio => ("audio_url", "audio_url", "reference_audio"),
            };
            let mut item = serde_json::Map::new();
            item.insert("type".to_string(), json!(kind));
            item.insert(field.to_string(), json!({ "url": asset.uri }));
            item.insert("role".to_string(), json!(role));
            Value::Object(item)
        }));
        content
    }
}

#[async_trait]
impl VideoGenerationAdapter for MiniMaxVideoAdapter {
    fn provider_id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn provider_source(&self) -> &str {
        &self.provider_source
    }

    fn get_capabilities(&self) -> Vec<VideoModelManifest> {
        find_capabilities(PROVIDER_ID)
            .into_iter()
            .filter(|model| model.api_version.as_deref() == Some("v2"))
            .collect()
    }

    fn validate(&self, request: &NormalizedVideoRequest) -> ValidationResult {
        let mut issues = common_validation(request);
        if request.prompt.chars().count() > 7000 {
            issues.push(issue(
                "prompt",
                "prompt_too_long",
                "MiniMax H3 prompts are limited to 7000 characters",
            ));
        }
        if request.model_id != MODEL_ID {
            issues.push(issue(
                "modelId",
                "unsupported_model",
                "MiniMax V2 currently exposes MiniMax-H3",
            ));
        }
        if !(4..=15).contains(&request.duration_seconds) {
            issues.push(issue(
                "durationSeconds",
                "unsupported_duration",
                "MiniMax H3 duration must be an integer from 4 through 15",
            ));
        }
        if !matches!(request.resolution.as_str(), "768P" | "2K") {
            issues.push(issue(
                "resolution",
                "unsupported_resolution",
                "MiniMax H3 resolution must be 768P or 2K",
            ));
        }
        if !matches!(
            request.aspect_ratio.as_str(),
            "adaptive" | "21:9" | "16:9" | "4:3" | "1:1" | "3:4" | "9:16"
        ) {
            issues.push(issue(
                "aspectRatio",
                "unsupported_aspect_ratio",
                "MiniMax H3 aspect ratio is not supported",
            ));
        }
        if request.seed.is_some() {
            issues.push(issue(
                "seed",
                "unsupported_seed",
                "MiniMax H3 V2 does not publish a seed parameter",
            ));
        }
        if request.generate_audio.is_some() {
            issues.push(issue(
                "generateAudio",
                "unsupported_audio_toggle",
                "MiniMax H3 can use audio context, but V2 publishes no audio-output toggle",
            ));
        }
        if request.callback_url.is_some() {
            issues.push(issue(
                "callbackUrl",
                "unsupported_webhook",
                "MiniMax does not publish callback authentication that Nexa can verify",
            ));
        }

        let first_frames = count_role(request, VideoInputRole::FirstFrame);
        let last_frames = count_role(request, VideoInputRole::LastFrame);
        let reference_images = count_role(request, VideoInputRole::ReferenceImage);
        let reference_videos = count_role(request, VideoInputRole::ReferenceVideo)
            + count_role(request, VideoInputRole::InputVideo);
        let reference_audio = count_role(request, VideoInputRole::ReferenceAudio);
        let keyframe_mode = first_frames + last_frames > 0;
        let reference_mode = reference_images + reference_videos + reference_audio > 0;
        if first_frames > 1
            || last_frames > 1
            || reference_images > 9
            || reference_videos > 3
            || reference_audio > 3
        {
            issues.push(issue(
                "inputAssets",
                "too_many_inputs",
                "MiniMax H3 allows one first/last frame, nine reference images, three reference videos, and three reference audio clips",
            ));
        }
        if last_frames > 0 && first_frames == 0 {
            issues.push(issue(
                "inputAssets",
                "missing_first_frame",
                "A MiniMax H3 last frame must be paired with a first frame",
            ));
        }
        if keyframe_mode && reference_mode {
            issues.push(issue(
                "inputAssets",
                "mixed_input_modes",
                "MiniMax H3 keyframe and reference modes are mutually exclusive",
            ));
        }
        match request.operation {
            MediaOperation::TextToVideo if !request.input_assets.is_empty() => issues.push(issue(
                "inputAssets",
                "unexpected_input",
                "Text-to-video cannot include media inputs",
            )),
            MediaOperation::TextToVideo if request.aspect_ratio == "adaptive" => {
                issues.push(issue(
                    "aspectRatio",
                    "concrete_ratio_required",
                    "Text-to-video requires a concrete aspect ratio",
                ))
            }
            MediaOperation::ImageToVideo if first_frames != 1 || last_frames != 0 => {
                issues.push(issue(
                    "inputAssets",
                    "first_frame_required",
                    "Image-to-video requires exactly one first frame",
                ))
            }
            MediaOperation::FirstLastFrame if first_frames != 1 || last_frames != 1 => {
                issues.push(issue(
                    "inputAssets",
                    "keyframes_required",
                    "First/last-frame video requires exactly one of each keyframe",
                ))
            }
            MediaOperation::VideoToVideo if reference_videos == 0 => issues.push(issue(
                "inputAssets",
                "reference_video_required",
                "Reference video generation requires at least one video input",
            )),
            MediaOperation::TextToVideo
            | MediaOperation::ImageToVideo
            | MediaOperation::FirstLastFrame
            | MediaOperation::VideoToVideo => {}
            _ => issues.push(issue(
                "operation",
                "unsupported_operation",
                "MiniMax H3 adapter does not support this operation",
            )),
        }
        if keyframe_mode && request.aspect_ratio != "adaptive" {
            issues.push(issue(
                "aspectRatio",
                "adaptive_ratio_required",
                "MiniMax H3 keyframe requests derive aspect ratio from the input",
            ));
        }
        for (index, asset) in request.input_assets.iter().enumerate() {
            if !asset.uri.starts_with("https://") && !asset.uri.starts_with("mm_file://") {
                issues.push(issue(
                    &format!("inputAssets[{index}].uri"),
                    "unsupported_locator",
                    "MiniMax H3 inputs must use HTTPS or mm_file locators",
                ));
            }
            let media_matches = match asset.role {
                VideoInputRole::FirstFrame
                | VideoInputRole::LastFrame
                | VideoInputRole::ReferenceImage => asset.media_type.starts_with("image/"),
                VideoInputRole::InputVideo | VideoInputRole::ReferenceVideo => {
                    asset.media_type.starts_with("video/")
                }
                VideoInputRole::ReferenceAudio => asset.media_type.starts_with("audio/"),
            };
            if !media_matches {
                issues.push(issue(
                    &format!("inputAssets[{index}].mediaType"),
                    "media_role_mismatch",
                    "Media MIME type does not match its MiniMax content role",
                ));
            }
            match asset.role {
                VideoInputRole::FirstFrame
                | VideoInputRole::LastFrame
                | VideoInputRole::ReferenceImage => {
                    if asset
                        .byte_length
                        .is_some_and(|bytes| bytes > 30 * 1024 * 1024)
                    {
                        issues.push(issue(
                            &format!("inputAssets[{index}].byteLength"),
                            "input_too_large",
                            "MiniMax H3 images may not exceed 30 MiB",
                        ));
                    }
                    validate_visual_dimensions(&mut issues, index, asset);
                }
                VideoInputRole::InputVideo | VideoInputRole::ReferenceVideo => {
                    if asset
                        .byte_length
                        .is_some_and(|bytes| bytes > 50 * 1024 * 1024)
                    {
                        issues.push(issue(
                            &format!("inputAssets[{index}].byteLength"),
                            "input_too_large",
                            "MiniMax H3 reference videos may not exceed 50 MiB",
                        ));
                    }
                    if asset
                        .duration_ms
                        .is_some_and(|duration| !(2_000..=15_000).contains(&duration))
                    {
                        issues.push(issue(
                            &format!("inputAssets[{index}].durationMs"),
                            "unsupported_reference_duration",
                            "MiniMax H3 reference videos must be 2-15 seconds",
                        ));
                    }
                    validate_visual_dimensions(&mut issues, index, asset);
                }
                VideoInputRole::ReferenceAudio => {
                    if asset
                        .byte_length
                        .is_some_and(|bytes| bytes > 15 * 1024 * 1024)
                    {
                        issues.push(issue(
                            &format!("inputAssets[{index}].byteLength"),
                            "input_too_large",
                            "MiniMax H3 reference audio may not exceed 15 MiB",
                        ));
                    }
                    if asset
                        .duration_ms
                        .is_some_and(|duration| !(2_000..=15_000).contains(&duration))
                    {
                        issues.push(issue(
                            &format!("inputAssets[{index}].durationMs"),
                            "unsupported_reference_duration",
                            "MiniMax H3 reference audio must be 2-15 seconds",
                        ));
                    }
                }
            }
        }
        let total_video_ms = request
            .input_assets
            .iter()
            .filter(|asset| {
                matches!(
                    asset.role,
                    VideoInputRole::InputVideo | VideoInputRole::ReferenceVideo
                )
            })
            .filter_map(|asset| asset.duration_ms)
            .sum::<u64>();
        let total_audio_ms = request
            .input_assets
            .iter()
            .filter(|asset| asset.role == VideoInputRole::ReferenceAudio)
            .filter_map(|asset| asset.duration_ms)
            .sum::<u64>();
        if total_video_ms > 15_000 {
            issues.push(issue(
                "inputAssets",
                "reference_video_duration_exceeded",
                "MiniMax H3 reference videos may total at most 15 seconds",
            ));
        }
        if total_audio_ms > 15_000 {
            issues.push(issue(
                "inputAssets",
                "reference_audio_duration_exceeded",
                "MiniMax H3 reference audio may total at most 15 seconds",
            ));
        }
        ValidationResult::new(issues)
    }

    async fn estimate_cost(
        &self,
        request: &NormalizedVideoRequest,
    ) -> Result<CostEstimate, NormalizedProviderError> {
        let validation = self.validate(request);
        if !validation.valid {
            return Err(invalid_request_error(PROVIDER_ID, validation));
        }
        h3_cost_estimate(request)
    }

    async fn submit(
        &self,
        request: &NormalizedVideoRequest,
    ) -> Result<SubmittedJob, NormalizedProviderError> {
        let validation = self.validate(request);
        if !validation.valid {
            return Err(invalid_request_error(PROVIDER_ID, validation));
        }
        let payload = json!({
            "model": MODEL_ID,
            "content": Self::content(request),
            "resolution": request.resolution,
            "duration": request.duration_seconds,
            "ratio": request.aspect_ratio,
        });
        let response: CreateResponse = http::execute_json(
            PROVIDER_ID,
            &self.api_key,
            self.authenticated(self.client.post(self.endpoint("/v2/video_generation")))
                .json(&payload),
        )
        .await?;
        if response.task_id.trim().is_empty() || response.task_id.len() > 256 {
            return Err(configuration_error("MiniMax returned an invalid task ID"));
        }
        Ok(SubmittedJob {
            provider_task_id: response.task_id,
            provider_source: self.provider_source.clone(),
            estimated_cost: h3_cost_estimate(request)?,
        })
    }

    async fn get_status(
        &self,
        provider_task_id: &str,
    ) -> Result<ProviderJobStatus, NormalizedProviderError> {
        let task_id = validate_task_id(provider_task_id)?;
        let response: QueryResponse = http::execute_json(
            PROVIDER_ID,
            &self.api_key,
            self.authenticated(
                self.client
                    .get(self.endpoint(&format!("/v2/query/video_generation/{task_id}"))),
            ),
        )
        .await?;
        let task = response.task;
        if task.id != task_id {
            return Err(configuration_error(
                "MiniMax query returned a different task ID",
            ));
        }
        let state = match task.status.as_str() {
            "queued" => ProviderJobState::Queued,
            "running" => ProviderJobState::Running,
            "succeeded" => ProviderJobState::Succeeded,
            "failed" => ProviderJobState::Failed,
            "cancelled" => ProviderJobState::Cancelled,
            _ => ProviderJobState::ProviderUnknown,
        };
        let error = task.error.map(|error| NormalizedProviderError {
            provider_id: PROVIDER_ID.to_string(),
            code: http::sanitize_message(&error.code.to_string(), Some(&self.api_key)),
            message: http::sanitize_message(&error.message, Some(&self.api_key)),
            retryable: false,
            retry_after_seconds: None,
            http_status: None,
            request_id: None,
        });
        let result = if state == ProviderJobState::Succeeded {
            let uri = task
                .content
                .and_then(|content| content.url)
                .filter(|url| !url.trim().is_empty())
                .ok_or_else(|| configuration_error("Succeeded MiniMax task has no output URL"))?;
            Some(ProviderJobResult {
                provider_task_id: task.id.clone(),
                outputs: vec![ProviderOutputLocator {
                    uri,
                    media_type: "video/mp4".to_string(),
                    expires_hint: Some("time_limited_refresh_by_query".to_string()),
                }],
                width: None,
                height: None,
                duration_ms: task.duration.map(|seconds| u64::from(seconds) * 1000),
            })
        } else {
            None
        };
        Ok(ProviderJobStatus {
            provider_task_id: task.id,
            state,
            raw_status: task.status,
            result,
            error,
            final_cost_micros: None,
        })
    }

    async fn cancel(
        &self,
        provider_task_id: &str,
    ) -> Result<CancellationResult, NormalizedProviderError> {
        let task_id = validate_task_id(provider_task_id)?;
        let response: DeleteResponse = http::execute_json(
            PROVIDER_ID,
            &self.api_key,
            self.authenticated(
                self.client
                    .delete(self.endpoint(&format!("/v2/video_generation/{task_id}"))),
            ),
        )
        .await?;
        if response.task_id != task_id {
            return Err(configuration_error(
                "MiniMax cancel returned a different task ID",
            ));
        }
        if response.action != "cancelled" || response.status != "cancelled" {
            return Err(NormalizedProviderError {
                provider_id: PROVIDER_ID.to_string(),
                code: "task_not_cancelled".to_string(),
                message:
                    "MiniMax deleted a terminal task record instead of confirming cancellation"
                        .to_string(),
                retryable: false,
                retry_after_seconds: None,
                http_status: None,
                request_id: None,
            });
        }
        Ok(CancellationResult {
            provider_task_id: response.task_id,
            confirmed: true,
            detail: "queued_task_cancelled".to_string(),
        })
    }

    async fn download_outputs(
        &self,
        result: &ProviderJobResult,
        destination_directory: &Path,
        max_total_bytes: u64,
    ) -> Result<Vec<DownloadedAsset>, NormalizedProviderError> {
        http::download_outputs(
            PROVIDER_ID,
            &self.client,
            result,
            destination_directory,
            max_total_bytes,
            self.allow_insecure_http,
        )
        .await
    }

    fn normalize_error(
        &self,
        error: &(dyn std::error::Error + Send + Sync),
    ) -> NormalizedProviderError {
        NormalizedProviderError {
            provider_id: PROVIDER_ID.to_string(),
            code: "adapter_error".to_string(),
            message: http::sanitize_message(&error.to_string(), Some(&self.api_key)),
            retryable: false,
            retry_after_seconds: None,
            http_status: None,
            request_id: None,
        }
    }
}

fn count_role(request: &NormalizedVideoRequest, role: VideoInputRole) -> usize {
    request
        .input_assets
        .iter()
        .filter(|asset| asset.role == role)
        .count()
}

fn validate_visual_dimensions(
    issues: &mut Vec<super::ValidationIssue>,
    index: usize,
    asset: &super::VideoInputAsset,
) {
    if let (Some(width), Some(height)) = (asset.width, asset.height) {
        let ratio = f64::from(width) / f64::from(height.max(1));
        if !(256..=5760).contains(&width)
            || !(256..=5760).contains(&height)
            || !(0.4..=2.5).contains(&ratio)
        {
            issues.push(issue(
                &format!("inputAssets[{index}]"),
                "invalid_media_dimensions",
                "MiniMax H3 visual inputs require 256-5760px dimensions and aspect ratio 0.4-2.5",
            ));
        }
    }
}

fn h3_cost_estimate(
    request: &NormalizedVideoRequest,
) -> Result<CostEstimate, NormalizedProviderError> {
    let mut estimate = pricing_estimate(PROVIDER_ID, request)?;
    let per_second = match request.resolution.as_str() {
        "768P" => 80_000_u64,
        "2K" => 130_000_u64,
        _ => return Ok(estimate),
    };
    let image_count = request
        .input_assets
        .iter()
        .filter(|asset| {
            matches!(
                asset.role,
                VideoInputRole::FirstFrame
                    | VideoInputRole::LastFrame
                    | VideoInputRole::ReferenceImage
            )
        })
        .count();
    let additional_images = image_count.saturating_sub(5) as u64;
    let mut missing_video_duration = false;
    let mut input_video_seconds = 0_u64;
    for asset in request.input_assets.iter().filter(|asset| {
        matches!(
            asset.role,
            VideoInputRole::InputVideo | VideoInputRole::ReferenceVideo
        )
    }) {
        match asset.duration_ms {
            Some(duration_ms) => {
                input_video_seconds = input_video_seconds.saturating_add(duration_ms.div_ceil(1000))
            }
            None => missing_video_duration = true,
        }
    }
    if let Some(amount) = estimate.amount_micros {
        estimate.amount_micros = amount
            .checked_add(additional_images.saturating_mul(40_000))
            .and_then(|amount| amount.checked_add(input_video_seconds.saturating_mul(per_second)));
    }
    if missing_video_duration {
        estimate.kind = super::CostEstimateKind::Estimated;
        estimate
            .note
            .push_str(" Reference-video duration was unavailable, so its usage is not included.");
    }
    Ok(estimate)
}

fn validate_secret_and_scope(
    api_key: &str,
    credential_scope: &str,
) -> Result<(), NormalizedProviderError> {
    if api_key.trim().is_empty() || api_key.len() > 4096 {
        return Err(configuration_error(
            "MiniMax API key must contain 1-4096 bytes",
        ));
    }
    if credential_scope.trim().is_empty() || credential_scope.len() > 256 {
        return Err(configuration_error(
            "Credential scope must contain 1-256 non-secret bytes",
        ));
    }
    Ok(())
}

fn validate_task_id(value: &str) -> Result<String, NormalizedProviderError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 256
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(configuration_error("Invalid MiniMax task ID"));
    }
    Ok(value.to_string())
}

fn configuration_error(message: &str) -> NormalizedProviderError {
    NormalizedProviderError {
        provider_id: PROVIDER_ID.to_string(),
        code: "invalid_configuration".to_string(),
        message: message.to_string(),
        retryable: false,
        retry_after_seconds: None,
        http_status: None,
        request_id: None,
    }
}

#[derive(Deserialize)]
struct CreateResponse {
    task_id: String,
}

#[derive(Deserialize)]
struct QueryResponse {
    task: MiniMaxTask,
}

#[derive(Deserialize)]
struct MiniMaxTask {
    id: String,
    status: String,
    content: Option<MiniMaxContent>,
    error: Option<MiniMaxError>,
    duration: Option<u32>,
}

#[derive(Deserialize)]
struct MiniMaxContent {
    url: Option<String>,
}

#[derive(Deserialize)]
struct MiniMaxError {
    code: Value,
    message: String,
}

#[derive(Deserialize)]
struct DeleteResponse {
    task_id: String,
    action: String,
    status: String,
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;
    use crate::media_generation::adapters::VideoInputAsset;

    fn request() -> NormalizedVideoRequest {
        NormalizedVideoRequest {
            idempotency_key: "job-1-attempt-1".to_string(),
            model_id: MODEL_ID.to_string(),
            operation: MediaOperation::TextToVideo,
            prompt: "A quiet ocean at dawn".to_string(),
            duration_seconds: 5,
            resolution: "2K".to_string(),
            aspect_ratio: "16:9".to_string(),
            input_assets: Vec::new(),
            seed: None,
            generate_audio: None,
            callback_url: None,
        }
    }

    #[test]
    fn validation_enforces_h3_mode_matrix() {
        let adapter = MiniMaxVideoAdapter::new("secret", "credential-1").unwrap();
        assert!(adapter.validate(&request()).valid);

        let mut invalid = request();
        invalid.operation = MediaOperation::FirstLastFrame;
        invalid.aspect_ratio = "16:9".to_string();
        invalid.input_assets = vec![VideoInputAsset {
            role: VideoInputRole::LastFrame,
            uri: "https://cdn.example.com/last.png".to_string(),
            media_type: "image/png".to_string(),
            byte_length: None,
            width: None,
            height: None,
            duration_ms: None,
        }];
        let result = adapter.validate(&invalid);
        assert!(!result.valid);
        assert!(result
            .issues
            .iter()
            .any(|issue| issue.code == "missing_first_frame"));
        assert!(result
            .issues
            .iter()
            .any(|issue| issue.code == "adaptive_ratio_required"));
    }

    #[tokio::test]
    async fn submit_uses_v2_multimodal_contract_and_bearer_auth() {
        let (base_url, captured) = serve_once(r#"{"task_id":"424010985738629"}"#).await;
        let adapter = MiniMaxVideoAdapter::for_test(&base_url);
        let submitted = adapter.submit(&request()).await.unwrap();
        assert_eq!(submitted.provider_task_id, "424010985738629");
        let request = captured.lock().unwrap().clone();
        assert!(request.starts_with("POST /v2/video_generation HTTP/1.1"));
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: bearer test-secret"));
        assert!(!request.to_ascii_lowercase().contains("idempotency"));
        let body = request.split("\r\n\r\n").nth(1).unwrap();
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["model"], MODEL_ID);
        assert_eq!(body["content"][0]["type"], "text");
        assert_eq!(body["resolution"], "2K");
    }

    async fn serve_once(body: &'static str) -> (String, Arc<Mutex<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(String::new()));
        let captured_task = captured.clone();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0_u8; 16 * 1024];
            let mut used = 0_usize;
            loop {
                let read = stream.read(&mut buffer[used..]).await.unwrap();
                used += read;
                let raw = String::from_utf8_lossy(&buffer[..used]);
                let Some(header_end) = raw.find("\r\n\r\n") else {
                    continue;
                };
                let content_length = raw[..header_end]
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length: ")
                            .or_else(|| line.strip_prefix("Content-Length: "))
                    })
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0);
                if used >= header_end + 4 + content_length {
                    break;
                }
            }
            *captured_task.lock().unwrap() = String::from_utf8_lossy(&buffer[..used]).to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        (format!("http://{address}"), captured)
    }
}
