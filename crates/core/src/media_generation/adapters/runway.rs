use std::path::Path;

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::{
    common_validation, find_capabilities, http, invalid_request_error, issue, pricing_estimate,
    provider_source, submission_error, submission_response_error, CancellationResult, CostEstimate,
    CostEstimateKind, DownloadedAsset, NormalizedProviderError, NormalizedVideoRequest,
    ProviderBilledUsage, ProviderCancellationRequest, ProviderJobResult, ProviderJobState,
    ProviderJobStatus, ProviderOutputLocator, SubmittedJob, ValidationResult,
    VideoGenerationAdapter, VideoInputAsset, VideoInputRole,
};
use crate::media_generation::MediaOperation;
use crate::video_provider_catalog::VideoModelManifest;

const PROVIDER_ID: &str = "runway";
const OFFICIAL_BASE_URL: &str = "https://api.dev.runwayml.com";
const API_VERSION: &str = "2024-11-06";

#[derive(Clone)]
pub struct RunwayVideoAdapter {
    client: reqwest::Client,
    base_url: url::Url,
    api_key: String,
    provider_source: String,
    allow_insecure_http: bool,
}

impl RunwayVideoAdapter {
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
        if api_key.trim().is_empty() || api_key.len() > 4096 {
            return Err(configuration_error(
                "Runway API key must contain 1-4096 bytes",
            ));
        }
        if credential_scope.trim().is_empty() || credential_scope.len() > 256 {
            return Err(configuration_error(
                "Credential scope must contain 1-256 non-secret bytes",
            ));
        }
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
                "Runway credentials may only be used with the exact official endpoint",
            ));
        }
        Ok(Self {
            client: http::client()?,
            provider_source: provider_source(PROVIDER_ID, &base_url, credential_scope, API_VERSION),
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
        self.base_url.join(path).expect("static Runway endpoint")
    }

    fn authenticated(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .header(AUTHORIZATION, format!("Bearer {}", self.api_key))
            .header(CONTENT_TYPE, "application/json")
            .header("X-Runway-Version", API_VERSION)
    }

    fn submit_path(operation: MediaOperation) -> Option<&'static str> {
        match operation {
            MediaOperation::TextToVideo => Some("/v1/text_to_video"),
            MediaOperation::ImageToVideo => Some("/v1/image_to_video"),
            MediaOperation::VideoToVideo => Some("/v1/video_to_video"),
            _ => None,
        }
    }

    fn payload(request: &NormalizedVideoRequest) -> Result<Value, NormalizedProviderError> {
        let ratio = dimensions_for(request).ok_or_else(|| {
            configuration_error("Resolution and aspect ratio do not map to a Runway ratio")
        })?;
        let mut payload = Map::new();
        payload.insert("model".to_string(), json!(request.model_id));
        payload.insert("promptText".to_string(), json!(request.prompt));
        payload.insert("ratio".to_string(), json!(ratio));
        payload.insert("duration".to_string(), json!(request.duration_seconds));
        if let Some(seed) = request.seed {
            payload.insert("seed".to_string(), json!(seed));
        }
        if let Some(audio) = request.generate_audio {
            payload.insert("audio".to_string(), json!(audio));
        }
        match request.operation {
            MediaOperation::ImageToVideo => {
                let first = find_role(request, VideoInputRole::FirstFrame)
                    .ok_or_else(|| configuration_error("Missing first-frame input"))?;
                payload.insert("promptImage".to_string(), json!(first.uri));
            }
            MediaOperation::VideoToVideo => {
                let input = find_role(request, VideoInputRole::InputVideo)
                    .ok_or_else(|| configuration_error("Missing input video"))?;
                payload.insert("promptVideo".to_string(), json!(input.uri));
                payload.insert("mode".to_string(), json!("reference"));
            }
            MediaOperation::TextToVideo => {}
            _ => return Err(configuration_error("Unsupported Runway operation")),
        }
        if request.model_id == "seedance2_5" {
            insert_references(&mut payload, request);
        }
        Ok(Value::Object(payload))
    }
}

#[async_trait]
impl VideoGenerationAdapter for RunwayVideoAdapter {
    fn provider_id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn provider_source(&self) -> &str {
        &self.provider_source
    }

    fn get_capabilities(&self) -> Vec<VideoModelManifest> {
        find_capabilities(PROVIDER_ID)
    }

    fn validate(&self, request: &NormalizedVideoRequest) -> ValidationResult {
        let mut issues = common_validation(request);
        let model_supported = matches!(
            request.model_id.as_str(),
            "gen4.5" | "gen4_turbo" | "seedance2_5"
        );
        if !model_supported {
            issues.push(issue(
                "modelId",
                "unsupported_model",
                "Runway adapter supports gen4.5, gen4_turbo, and seedance2_5",
            ));
        }
        let operation_supported = match request.model_id.as_str() {
            "gen4.5" => matches!(
                request.operation,
                MediaOperation::TextToVideo | MediaOperation::ImageToVideo
            ),
            "gen4_turbo" => request.operation == MediaOperation::ImageToVideo,
            "seedance2_5" => matches!(
                request.operation,
                MediaOperation::TextToVideo
                    | MediaOperation::ImageToVideo
                    | MediaOperation::VideoToVideo
            ),
            _ => false,
        };
        if !operation_supported {
            issues.push(issue(
                "operation",
                "unsupported_operation",
                "Model does not support this operation in the live Runway contract",
            ));
        }
        let prompt_utf16 = request.prompt.encode_utf16().count();
        let max_prompt = if request.model_id == "seedance2_5" {
            15_000
        } else {
            1_000
        };
        if prompt_utf16 == 0 || prompt_utf16 > max_prompt {
            issues.push(issue(
                "prompt",
                "unsupported_prompt_length",
                &format!("Runway model permits at most {max_prompt} UTF-16 code units"),
            ));
        }
        let duration_range = if request.model_id == "seedance2_5" {
            4..=30
        } else {
            2..=10
        };
        if !duration_range.contains(&request.duration_seconds) {
            issues.push(issue(
                "durationSeconds",
                "unsupported_duration",
                "Duration falls outside this Runway model's contract",
            ));
        }
        let allowed_resolutions: &[&str] = if request.model_id == "seedance2_5" {
            &["480P", "720P"]
        } else {
            &["720P"]
        };
        if !allowed_resolutions.contains(&request.resolution.as_str()) {
            issues.push(issue(
                "resolution",
                "unsupported_resolution",
                "Resolution is not supported by this Runway model",
            ));
        }
        if dimensions_for(request).is_none() {
            issues.push(issue(
                "aspectRatio",
                "unsupported_aspect_ratio",
                "Resolution and aspect ratio do not map to a provider ratio",
            ));
        }
        if request.model_id == "gen4.5"
            && request.operation == MediaOperation::TextToVideo
            && !matches!(request.aspect_ratio.as_str(), "16:9" | "9:16")
        {
            issues.push(issue(
                "aspectRatio",
                "unsupported_text_to_video_ratio",
                "Runway Gen-4.5 text-to-video supports only 16:9 or 9:16",
            ));
        }
        if request.model_id == "seedance2_5" && request.seed.is_some() {
            issues.push(issue(
                "seed",
                "unsupported_seed",
                "Runway Seedance 2.5 does not publish a seed parameter",
            ));
        }
        if request.model_id != "seedance2_5" && request.generate_audio.is_some() {
            issues.push(issue(
                "generateAudio",
                "unsupported_audio",
                "This Runway model does not publish an audio-generation parameter",
            ));
        }
        if request.callback_url.is_some() {
            issues.push(issue(
                "callbackUrl",
                "unsupported_webhook",
                "Runway task API does not publish a webhook parameter",
            ));
        }

        let first_frames = count_role(request, VideoInputRole::FirstFrame);
        let input_videos = count_role(request, VideoInputRole::InputVideo);
        match request.operation {
            MediaOperation::TextToVideo
                if request.input_assets.iter().any(|asset| {
                    matches!(
                        asset.role,
                        VideoInputRole::FirstFrame | VideoInputRole::InputVideo
                    )
                }) =>
            {
                issues.push(issue(
                    "inputAssets",
                    "unexpected_primary_input",
                    "Text-to-video cannot include a first frame or primary input video",
                ));
            }
            MediaOperation::ImageToVideo if first_frames != 1 => issues.push(issue(
                "inputAssets",
                "first_frame_required",
                "Image-to-video requires exactly one first-frame image",
            )),
            MediaOperation::VideoToVideo if input_videos != 1 => issues.push(issue(
                "inputAssets",
                "input_video_required",
                "Video-to-video requires exactly one primary input video",
            )),
            _ => {}
        }
        if request
            .input_assets
            .iter()
            .any(|asset| asset.role == VideoInputRole::LastFrame)
        {
            issues.push(issue(
                "inputAssets",
                "unsupported_last_frame",
                "The enabled Runway adapter paths do not accept a last-frame input",
            ));
        }
        let reference_images = count_role(request, VideoInputRole::ReferenceImage);
        let reference_videos = count_role(request, VideoInputRole::ReferenceVideo);
        let reference_audio = count_role(request, VideoInputRole::ReferenceAudio);
        if request.model_id != "seedance2_5"
            && reference_images + reference_videos + reference_audio > 0
        {
            issues.push(issue(
                "inputAssets",
                "unsupported_references",
                "This Runway model does not accept reference media on the enabled endpoint",
            ));
        }
        if reference_images > 30 || reference_videos > 10 || reference_audio > 10 {
            issues.push(issue(
                "inputAssets",
                "too_many_references",
                "Runway Seedance 2.5 reference counts exceed the live contract",
            ));
        }
        if request.model_id == "seedance2_5"
            && request.operation == MediaOperation::ImageToVideo
            && reference_images + reference_videos > 0
        {
            issues.push(issue(
                "inputAssets",
                "unsupported_image_to_video_references",
                "Runway Seedance 2.5 image-to-video accepts reference audio but not additional reference images or videos",
            ));
        }
        if request.model_id == "seedance2_5"
            && request.operation == MediaOperation::VideoToVideo
            && reference_videos > 9
        {
            issues.push(issue(
                "inputAssets",
                "too_many_reference_videos",
                "Runway Seedance 2.5 video-to-video accepts at most nine reference videos",
            ));
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
        if request.model_id == "seedance2_5" && total_video_ms > 30_000 {
            issues.push(issue(
                "inputAssets",
                "video_duration_exceeded",
                "Runway Seedance 2.5 input and reference videos may total at most 30 seconds",
            ));
        }
        let total_audio_ms = request
            .input_assets
            .iter()
            .filter(|asset| asset.role == VideoInputRole::ReferenceAudio)
            .filter_map(|asset| asset.duration_ms)
            .sum::<u64>();
        if request.model_id == "seedance2_5" && total_audio_ms > 30_000 {
            issues.push(issue(
                "inputAssets",
                "audio_duration_exceeded",
                "Runway Seedance 2.5 reference audio may total at most 30 seconds",
            ));
        }
        for (index, asset) in request.input_assets.iter().enumerate() {
            if !asset.uri.starts_with("https://")
                && !asset.uri.starts_with("runway://")
                && !asset.uri.starts_with("data:image/")
            {
                issues.push(issue(
                    &format!("inputAssets[{index}].uri"),
                    "unsupported_locator",
                    "Runway inputs must use HTTPS or runway locators",
                ));
            }
            let valid_type = match asset.role {
                VideoInputRole::FirstFrame
                | VideoInputRole::LastFrame
                | VideoInputRole::ReferenceImage => matches!(
                    asset.media_type.as_str(),
                    "image/jpg" | "image/jpeg" | "image/png" | "image/webp"
                ),
                VideoInputRole::InputVideo | VideoInputRole::ReferenceVideo => {
                    runway_video_codec_supported(asset)
                }
                VideoInputRole::ReferenceAudio => matches!(
                    asset.media_type.as_str(),
                    "audio/mpeg"
                        | "audio/mp3"
                        | "audio/wav"
                        | "audio/wave"
                        | "audio/x-wav"
                        | "audio/flac"
                        | "audio/x-flac"
                        | "audio/mp4"
                        | "audio/x-m4a"
                        | "audio/aac"
                        | "audio/x-aac"
                ),
            };
            if !valid_type {
                issues.push(issue(
                    &format!("inputAssets[{index}].mediaType"),
                    "media_role_mismatch",
                    "Media MIME type does not match its Runway input role",
                ));
            }
            if asset.byte_length.is_none() {
                issues.push(issue(
                    &format!("inputAssets[{index}].byteLength"),
                    "missing_media_metadata",
                    "Runway inputs require a verified byte length",
                ));
            }
            let size_limit = if asset.uri.starts_with("runway://") {
                200 * 1024 * 1024
            } else if asset.uri.starts_with("data:image/") {
                5 * 1024 * 1024
            } else if asset.media_type.starts_with("image/") {
                16 * 1024 * 1024
            } else {
                32 * 1024 * 1024
            };
            if asset.byte_length.is_some_and(|bytes| bytes > size_limit) {
                issues.push(issue(
                    &format!("inputAssets[{index}].byteLength"),
                    "input_too_large",
                    "Runway input exceeds the locator-specific byte limit",
                ));
            }
            if asset.uri.starts_with("https://") && asset.uri.len() > 2_048 {
                issues.push(issue(
                    &format!("inputAssets[{index}].uri"),
                    "input_url_too_long",
                    "Runway HTTPS input URLs may not exceed 2048 bytes",
                ));
            }
            if asset.uri.starts_with("https://")
                && url::Url::parse(&asset.uri)
                    .ok()
                    .is_some_and(|url| !matches!(url.host(), Some(url::Host::Domain(_))))
            {
                issues.push(issue(
                    &format!("inputAssets[{index}].uri"),
                    "ip_input_url_unsupported",
                    "Runway HTTPS inputs require a domain host rather than an IP literal",
                ));
            }
            match asset.role {
                VideoInputRole::InputVideo | VideoInputRole::ReferenceVideo => {
                    if asset.width.is_none()
                        || asset.height.is_none()
                        || asset.duration_ms.is_none()
                    {
                        issues.push(issue(
                            &format!("inputAssets[{index}]"),
                            "missing_media_metadata",
                            "Runway videos require verified dimensions and duration",
                        ));
                    }
                    if request.model_id == "seedance2_5"
                        && asset
                            .width
                            .zip(asset.height)
                            .is_some_and(|(width, height)| width.min(height) < 480)
                    {
                        issues.push(issue(
                            &format!("inputAssets[{index}]"),
                            "video_resolution_too_small",
                            "Runway Seedance 2.5 videos must be at least 480p",
                        ));
                    }
                }
                VideoInputRole::FirstFrame | VideoInputRole::ReferenceImage => {
                    if asset.width.is_none() || asset.height.is_none() {
                        issues.push(issue(
                            &format!("inputAssets[{index}]"),
                            "missing_media_metadata",
                            "Runway images require verified dimensions",
                        ));
                    }
                    if asset
                        .width
                        .zip(asset.height)
                        .is_some_and(|(width, height)| {
                            let ratio = f64::from(width) / f64::from(height.max(1));
                            !(0.4..=4.0).contains(&ratio)
                        })
                    {
                        issues.push(issue(
                            &format!("inputAssets[{index}]"),
                            "unsupported_image_ratio",
                            "Runway image aspect ratio must be between 0.4 and 4",
                        ));
                    }
                }
                VideoInputRole::ReferenceAudio if asset.duration_ms.is_none() => {
                    issues.push(issue(
                        &format!("inputAssets[{index}].durationMs"),
                        "missing_media_metadata",
                        "Runway reference audio requires verified duration",
                    ));
                }
                _ => {}
            }
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
        runway_cost_estimate(request)
    }

    async fn submit(
        &self,
        request: &NormalizedVideoRequest,
    ) -> Result<SubmittedJob, NormalizedProviderError> {
        let validation = self.validate(request);
        if !validation.valid {
            return Err(invalid_request_error(PROVIDER_ID, validation));
        }
        for asset in request
            .input_assets
            .iter()
            .filter(|asset| asset.uri.starts_with("https://"))
        {
            http::preflight_remote_input(
                PROVIDER_ID,
                &asset.uri,
                &asset.media_type,
                asset.byte_length.unwrap_or(0),
            )
            .await?;
        }
        let path = Self::submit_path(request.operation)
            .ok_or_else(|| configuration_error("Unsupported Runway operation"))?;
        let response: CreateResponse = http::execute_json(
            PROVIDER_ID,
            &self.api_key,
            self.authenticated(self.client.post(self.endpoint(path)))
                .json(&Self::payload(request)?),
        )
        .await
        .map_err(submission_error)?;
        validate_task_id(&response.id).map_err(submission_response_error)?;
        let estimated_cost = response
            .estimated_cost
            .and_then(|cost| credits_to_micros(cost.credits))
            .map(|amount_micros| CostEstimate {
                kind: CostEstimateKind::Estimated,
                amount_micros: Some(amount_micros),
                currency: Some("USD".to_string()),
                note: "Runway task response estimate at USD 0.01 per credit".to_string(),
            })
            .unwrap_or(runway_cost_estimate(request)?);
        Ok(SubmittedJob {
            provider_task_id: response.id,
            provider_source: self.provider_source.clone(),
            estimated_cost,
        })
    }

    async fn get_status(
        &self,
        provider_task_id: &str,
    ) -> Result<ProviderJobStatus, NormalizedProviderError> {
        let task_id = validate_task_id(provider_task_id)?;
        let response: TaskResponse = http::execute_json(
            PROVIDER_ID,
            &self.api_key,
            self.authenticated(
                self.client
                    .get(self.endpoint(&format!("/v1/tasks/{task_id}"))),
            ),
        )
        .await?;
        if response.id != task_id {
            return Err(configuration_error(
                "Runway query returned a different task ID",
            ));
        }
        let state = match response.status.as_str() {
            "PENDING" | "THROTTLED" => ProviderJobState::Queued,
            "RUNNING" => ProviderJobState::Running,
            "SUCCEEDED" => ProviderJobState::Succeeded,
            "FAILED" => ProviderJobState::Failed,
            "CANCELLED" => ProviderJobState::Cancelled,
            _ => ProviderJobState::ProviderUnknown,
        };
        let error = (state == ProviderJobState::Failed).then(|| NormalizedProviderError {
            provider_id: PROVIDER_ID.to_string(),
            code: response
                .failure_code
                .as_deref()
                .map(|value| http::sanitize_message(value, Some(&self.api_key)))
                .unwrap_or_else(|| "generation_failed".to_string()),
            message: response
                .failure
                .as_deref()
                .map(|value| http::sanitize_message(value, Some(&self.api_key)))
                .unwrap_or_else(|| "Runway generation failed".to_string()),
            retryable: false,
            retry_after_seconds: None,
            http_status: None,
            request_id: None,
        });
        let result = if state == ProviderJobState::Succeeded {
            let outputs = response
                .output
                .unwrap_or_default()
                .into_iter()
                .map(|uri| ProviderOutputLocator {
                    media_type: media_type_for_output(&uri).to_string(),
                    uri,
                    expires_hint: Some("24_to_48_hours_refresh_by_query".to_string()),
                })
                .collect::<Vec<_>>();
            if outputs.is_empty() {
                return Err(configuration_error("Succeeded Runway task has no outputs"));
            }
            Some(ProviderJobResult {
                provider_task_id: response.id.clone(),
                outputs,
                width: None,
                height: None,
                duration_ms: None,
            })
        } else {
            None
        };
        let billed_credits = response.cost.as_ref().map(|cost| cost.credits);
        Ok(ProviderJobStatus {
            provider_task_id: response.id,
            state,
            raw_status: response.status,
            result,
            error,
            billed_usage: billed_credits.map(|credits| ProviderBilledUsage {
                total_seconds: None,
                input_seconds: None,
                output_seconds: None,
                input_image_count: None,
                credits: Some(credits),
            }),
            final_cost_micros: billed_credits.and_then(credits_to_micros),
        })
    }

    async fn cancel(
        &self,
        request: &ProviderCancellationRequest,
    ) -> Result<CancellationResult, NormalizedProviderError> {
        if !request.allow_terminal_record_deletion {
            return Err(NormalizedProviderError {
                provider_id: PROVIDER_ID.to_string(),
                code: "destructive_confirmation_required".to_string(),
                message: "Runway cancellation may delete a task that completes during the request"
                    .to_string(),
                retryable: false,
                retry_after_seconds: None,
                http_status: None,
                request_id: None,
            });
        }
        let task_id = validate_task_id(&request.provider_task_id)?;
        let before = self.get_status(&task_id).await?;
        if matches!(
            before.state,
            ProviderJobState::Succeeded | ProviderJobState::Failed | ProviderJobState::Cancelled
        ) {
            return Err(NormalizedProviderError {
                provider_id: PROVIDER_ID.to_string(),
                code: "task_not_cancellable".to_string(),
                message: "Runway task is already terminal; cancellation was not attempted"
                    .to_string(),
                retryable: false,
                retry_after_seconds: None,
                http_status: None,
                request_id: None,
            });
        }
        http::execute_no_content(
            PROVIDER_ID,
            &self.api_key,
            self.authenticated(
                self.client
                    .delete(self.endpoint(&format!("/v1/tasks/{task_id}"))),
            ),
        )
        .await?;
        Ok(CancellationResult {
            provider_task_id: task_id,
            // The endpoint returns the same 204 for cancellation and deletion.
            // Persist the acknowledgement, but do not claim terminal cancelled.
            confirmed: false,
            detail: "cancel_or_delete_acknowledged_reconciliation_required".to_string(),
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

fn dimensions_for(request: &NormalizedVideoRequest) -> Option<&'static str> {
    let ratio = request.aspect_ratio.as_str();
    if request.model_id == "seedance2_5" {
        return match (request.resolution.as_str(), ratio) {
            ("480P", "21:9") => Some("992:432"),
            ("480P", "16:9") => Some("864:496"),
            ("480P", "4:3") => Some("752:560"),
            ("480P", "1:1") => Some("640:640"),
            ("480P", "3:4") => Some("560:752"),
            ("480P", "9:16") => Some("496:864"),
            ("720P", "21:9") => Some("1470:630"),
            ("720P", "16:9") => Some("1280:720"),
            ("720P", "4:3") => Some("1112:834"),
            ("720P", "1:1") => Some("960:960"),
            ("720P", "3:4") => Some("834:1112"),
            ("720P", "9:16") => Some("720:1280"),
            _ => None,
        };
    }
    if request.resolution != "720P" {
        return None;
    }
    match ratio {
        "21:9" => Some("1584:672"),
        "16:9" => Some("1280:720"),
        "4:3" => Some("1104:832"),
        "1:1" => Some("960:960"),
        "3:4" => Some("832:1104"),
        "9:16" => Some("720:1280"),
        _ => None,
    }
}

fn runway_video_codec_supported(asset: &VideoInputAsset) -> bool {
    let Some(codec) = asset
        .video_codec
        .as_deref()
        .map(|codec| codec.to_ascii_lowercase())
    else {
        return false;
    };
    let codec = codec.as_str();
    match asset.media_type.as_str() {
        "video/mp4" => matches!(codec, "h264" | "h265" | "hevc" | "av1"),
        "video/quicktime" => {
            matches!(codec, "h264" | "h265" | "hevc" | "mjpeg") || codec.starts_with("prores")
        }
        "video/x-matroska" => {
            matches!(
                codec,
                "h264" | "h265" | "hevc" | "vp8" | "vp9" | "av1" | "mpeg2"
            )
        }
        "video/webm" => matches!(codec, "vp8" | "vp9" | "av1"),
        "video/3gpp" => codec == "h264",
        "video/ogg" => codec == "theora",
        "video/x-msvideo" => matches!(codec, "h264" | "mjpeg" | "msmpeg4v3"),
        "video/x-flv" => matches!(codec, "flv1" | "h264"),
        "video/mpeg" => codec == "mpeg2",
        _ => false,
    }
}

fn insert_references(payload: &mut Map<String, Value>, request: &NormalizedVideoRequest) {
    let images = references(request, VideoInputRole::ReferenceImage, None);
    let videos = references(request, VideoInputRole::ReferenceVideo, Some("video"));
    let audio = references(request, VideoInputRole::ReferenceAudio, Some("audio"));
    if !images.is_empty() {
        payload.insert("references".to_string(), Value::Array(images));
    }
    if !videos.is_empty() {
        payload.insert("referenceVideos".to_string(), Value::Array(videos));
    }
    if !audio.is_empty() {
        payload.insert("referenceAudio".to_string(), Value::Array(audio));
    }
}

fn references(
    request: &NormalizedVideoRequest,
    role: VideoInputRole,
    kind: Option<&str>,
) -> Vec<Value> {
    request
        .input_assets
        .iter()
        .filter(|asset| asset.role == role)
        .map(|asset| match kind {
            Some(kind) => json!({ "type": kind, "uri": asset.uri }),
            None => json!({ "uri": asset.uri }),
        })
        .collect()
}

fn find_role(request: &NormalizedVideoRequest, role: VideoInputRole) -> Option<&VideoInputAsset> {
    request.input_assets.iter().find(|asset| asset.role == role)
}

fn count_role(request: &NormalizedVideoRequest, role: VideoInputRole) -> usize {
    request
        .input_assets
        .iter()
        .filter(|asset| asset.role == role)
        .count()
}

fn runway_cost_estimate(
    request: &NormalizedVideoRequest,
) -> Result<CostEstimate, NormalizedProviderError> {
    let mut estimate = pricing_estimate(PROVIDER_ID, request)?;
    if request.model_id != "seedance2_5" {
        return Ok(estimate);
    }
    let input_per_second = match request.resolution.as_str() {
        "480P" => 100_000_u64,
        "720P" => 150_000_u64,
        _ => return Ok(estimate),
    };
    let mut missing_duration = false;
    let mut input_seconds = 0_u64;
    for asset in request.input_assets.iter().filter(|asset| {
        matches!(
            asset.role,
            VideoInputRole::InputVideo | VideoInputRole::ReferenceVideo
        )
    }) {
        match asset.duration_ms {
            Some(duration_ms) => {
                input_seconds = input_seconds.saturating_add(duration_ms.div_ceil(1000))
            }
            None => missing_duration = true,
        }
    }
    if let Some(amount) = estimate.amount_micros {
        estimate.amount_micros = amount
            .checked_add(input_seconds.saturating_mul(input_per_second))
            .map(|amount| amount.max(800_000));
    }
    if missing_duration {
        estimate.kind = CostEstimateKind::Estimated;
        estimate.note.push_str(
            " Input/reference-video duration was unavailable, so that usage is not included.",
        );
    }
    Ok(estimate)
}

fn validate_task_id(value: &str) -> Result<String, NormalizedProviderError> {
    let value = value.trim();
    let uuid = uuid::Uuid::parse_str(value)
        .map_err(|_| configuration_error("Runway task ID must be a UUID"))?;
    if uuid.get_version_num() != 4 {
        return Err(configuration_error(
            "Runway task ID must be a version 4 UUID",
        ));
    }
    Ok(value.to_string())
}

fn credits_to_micros(credits: f64) -> Option<u64> {
    if !credits.is_finite() || credits < 0.0 {
        return None;
    }
    let micros = credits * 10_000.0;
    (micros <= u64::MAX as f64).then(|| micros.round() as u64)
}

fn media_type_for_output(uri: &str) -> &'static str {
    let path = url::Url::parse(uri)
        .ok()
        .map(|url| url.path().to_ascii_lowercase())
        .unwrap_or_default();
    if path.ends_with(".mov") {
        "video/quicktime"
    } else if path.ends_with(".zip") {
        "application/zip"
    } else {
        "video/mp4"
    }
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
#[serde(rename_all = "camelCase")]
struct CreateResponse {
    id: String,
    estimated_cost: Option<CreditAmount>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskResponse {
    id: String,
    status: String,
    output: Option<Vec<String>>,
    failure: Option<String>,
    failure_code: Option<String>,
    cost: Option<CreditAmount>,
}

#[derive(Deserialize)]
struct CreditAmount {
    credits: f64,
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    fn request(model: &str, operation: MediaOperation) -> NormalizedVideoRequest {
        NormalizedVideoRequest {
            idempotency_key: "job-1-attempt-1".to_string(),
            model_id: model.to_string(),
            operation,
            prompt: "A quiet ocean at dawn".to_string(),
            duration_seconds: if model == "seedance2_5" { 8 } else { 5 },
            resolution: "720P".to_string(),
            aspect_ratio: "16:9".to_string(),
            input_assets: Vec::new(),
            seed: None,
            generate_audio: (model == "seedance2_5").then_some(true),
            callback_url: None,
        }
    }

    #[test]
    fn validation_is_model_and_operation_specific() {
        let adapter = RunwayVideoAdapter::new("secret", "credential-1").unwrap();
        assert!(
            adapter
                .validate(&request("gen4.5", MediaOperation::TextToVideo))
                .valid
        );

        let invalid = request("gen4_turbo", MediaOperation::TextToVideo);
        assert!(adapter
            .validate(&invalid)
            .issues
            .iter()
            .any(|issue| issue.code == "unsupported_operation"));

        let seedance = request("seedance2_5", MediaOperation::TextToVideo);
        assert!(adapter.validate(&seedance).valid);
        assert_eq!(dimensions_for(&seedance), Some("1280:720"));
    }

    #[tokio::test]
    async fn submit_sends_required_version_and_maps_response_estimate() {
        let response =
            r#"{"id":"497f6eca-6276-4993-bfeb-53cbbbba6f08","estimatedCost":{"credits":60}}"#;
        let (base_url, captured) = serve_once(response).await;
        let adapter = RunwayVideoAdapter::for_test(&base_url);
        let submitted = adapter
            .submit(&request("gen4.5", MediaOperation::TextToVideo))
            .await
            .unwrap();
        assert_eq!(submitted.estimated_cost.amount_micros, Some(600_000));
        assert_eq!(submitted.estimated_cost.kind, CostEstimateKind::Estimated);
        let request = captured.lock().unwrap().clone();
        assert!(request.starts_with("POST /v1/text_to_video HTTP/1.1"));
        assert!(request
            .to_ascii_lowercase()
            .contains("x-runway-version: 2024-11-06"));
        assert!(!request.to_ascii_lowercase().contains("idempotency"));
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["model"], "gen4.5");
        assert_eq!(body["ratio"], "1280:720");
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
