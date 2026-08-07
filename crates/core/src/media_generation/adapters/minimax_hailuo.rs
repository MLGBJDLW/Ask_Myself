use std::path::Path;

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    common_validation, find_capabilities, http, invalid_request_error, issue, pricing_estimate,
    provider_source, submission_error, submission_response_error, CancellationResult, CostEstimate,
    DownloadedAsset, NormalizedProviderError, NormalizedVideoRequest, ProviderCancellationRequest,
    ProviderJobResult, ProviderJobState, ProviderJobStatus, ProviderOutputLocator, SubmittedJob,
    ValidationResult, VideoGenerationAdapter, VideoInputRole,
};
use crate::media_generation::MediaOperation;
use crate::video_provider_catalog::VideoModelManifest;

const PROVIDER_ID: &str = "minimax";
const OFFICIAL_BASE_URL: &str = "https://api.minimax.io";
const HAILUO_23: &str = "MiniMax-Hailuo-2.3";
const HAILUO_23_FAST: &str = "MiniMax-Hailuo-2.3-Fast";
const HAILUO_02: &str = "MiniMax-Hailuo-02";

#[derive(Clone)]
pub struct MiniMaxHailuoVideoAdapter {
    client: reqwest::Client,
    base_url: url::Url,
    api_key: String,
    provider_source: String,
    allow_insecure_http: bool,
}

impl MiniMaxHailuoVideoAdapter {
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
                "MiniMax API key must contain 1-4096 bytes",
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
                "MiniMax credentials may only be used with the exact official endpoint",
            ));
        }
        Ok(Self {
            client: http::client()?,
            provider_source: provider_source(PROVIDER_ID, &base_url, credential_scope, "v1"),
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
}

#[async_trait]
impl VideoGenerationAdapter for MiniMaxHailuoVideoAdapter {
    fn provider_id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn provider_source(&self) -> &str {
        &self.provider_source
    }

    fn get_capabilities(&self) -> Vec<VideoModelManifest> {
        find_capabilities(PROVIDER_ID)
            .into_iter()
            .filter(|model| model.api_version.as_deref() == Some("v1"))
            .collect()
    }

    fn validate(&self, request: &NormalizedVideoRequest) -> ValidationResult {
        let mut issues = common_validation(request);
        if request.prompt.chars().count() > 2000 {
            issues.push(issue(
                "prompt",
                "prompt_too_long",
                "Legacy MiniMax video prompts are limited to 2000 characters",
            ));
        }
        if !matches!(
            request.model_id.as_str(),
            HAILUO_23 | HAILUO_23_FAST | HAILUO_02
        ) {
            issues.push(issue(
                "modelId",
                "unsupported_model",
                "Legacy MiniMax adapter supports Hailuo 2.3, 2.3 Fast, and 02",
            ));
        }
        let operation_supported = match request.model_id.as_str() {
            HAILUO_23 => matches!(
                request.operation,
                MediaOperation::TextToVideo | MediaOperation::ImageToVideo
            ),
            HAILUO_23_FAST => request.operation == MediaOperation::ImageToVideo,
            HAILUO_02 => matches!(
                request.operation,
                MediaOperation::TextToVideo
                    | MediaOperation::ImageToVideo
                    | MediaOperation::FirstLastFrame
            ),
            _ => false,
        };
        if !operation_supported {
            issues.push(issue(
                "operation",
                "unsupported_operation",
                "Model does not support this legacy MiniMax operation",
            ));
        }
        if !valid_duration_resolution(request) {
            issues.push(issue(
                "resolution",
                "unsupported_duration_resolution",
                "Duration and resolution do not match this Hailuo model/operation matrix",
            ));
        }
        if request.aspect_ratio != "adaptive" {
            issues.push(issue(
                "aspectRatio",
                "unsupported_aspect_ratio",
                "Legacy Hailuo API does not publish an output aspect-ratio parameter",
            ));
        }
        if request.seed.is_some() {
            issues.push(issue(
                "seed",
                "unsupported_seed",
                "Legacy Hailuo API does not publish a seed parameter",
            ));
        }
        if request.generate_audio.is_some() {
            issues.push(issue(
                "generateAudio",
                "unsupported_audio",
                "Legacy Hailuo API does not publish an audio-generation parameter",
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
        match request.operation {
            MediaOperation::TextToVideo if !request.input_assets.is_empty() => issues.push(issue(
                "inputAssets",
                "unexpected_input",
                "Legacy text-to-video cannot include media inputs",
            )),
            MediaOperation::ImageToVideo if first_frames != 1 || last_frames != 0 => issues.push(
                issue(
                    "inputAssets",
                    "first_frame_required",
                    "Legacy image-to-video requires exactly one first-frame image",
                ),
            ),
            MediaOperation::FirstLastFrame
                if last_frames != 1 || first_frames > 1 || request.input_assets.len() > 2 =>
            {
                issues.push(issue(
                    "inputAssets",
                    "last_frame_required",
                    "Legacy first/last-frame generation requires one last frame and allows one optional first frame",
                ))
            }
            _ => {}
        }
        for (index, asset) in request.input_assets.iter().enumerate() {
            if !matches!(
                asset.media_type.as_str(),
                "image/jpeg" | "image/jpg" | "image/png" | "image/webp"
            ) {
                issues.push(issue(
                    &format!("inputAssets[{index}].mediaType"),
                    "unsupported_image_format",
                    "Legacy Hailuo images must be JPEG, PNG, or WebP",
                ));
            }
            if asset.byte_length.is_none() || asset.width.is_none() || asset.height.is_none() {
                issues.push(issue(
                    &format!("inputAssets[{index}]"),
                    "missing_media_metadata",
                    "Legacy Hailuo images require verified byte length and dimensions",
                ));
            }
            if !matches!(
                asset.role,
                VideoInputRole::FirstFrame | VideoInputRole::LastFrame
            ) || !asset.media_type.starts_with("image/")
                || (!asset.uri.starts_with("https://") && !asset.uri.starts_with("data:image/"))
            {
                issues.push(issue(
                    &format!("inputAssets[{index}]"),
                    "unsupported_input",
                    "Legacy Hailuo inputs must be first/last-frame images at public HTTPS URLs",
                ));
            }
            if asset
                .byte_length
                .is_some_and(|bytes| bytes > 20 * 1024 * 1024)
            {
                issues.push(issue(
                    &format!("inputAssets[{index}].byteLength"),
                    "input_too_large",
                    "Legacy Hailuo images must be smaller than 20 MiB",
                ));
            }
            if let (Some(width), Some(height)) = (asset.width, asset.height) {
                let short_edge = width.min(height);
                let ratio = f64::from(width) / f64::from(height.max(1));
                if short_edge <= 300 || !(0.4..=2.5).contains(&ratio) {
                    issues.push(issue(
                        &format!("inputAssets[{index}]"),
                        "invalid_image_dimensions",
                        "Legacy Hailuo images require a short edge over 300px and aspect ratio 0.4-2.5",
                    ));
                }
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
        pricing_estimate(PROVIDER_ID, request)
    }

    async fn submit(
        &self,
        request: &NormalizedVideoRequest,
    ) -> Result<SubmittedJob, NormalizedProviderError> {
        let validation = self.validate(request);
        if !validation.valid {
            return Err(invalid_request_error(PROVIDER_ID, validation));
        }
        let mut payload = json!({
            "model": request.model_id,
            "prompt": request.prompt,
            "duration": request.duration_seconds,
            "resolution": request.resolution,
        });
        if let Some(first) = find_role(request, VideoInputRole::FirstFrame) {
            payload["first_frame_image"] = json!(first.uri);
        }
        if let Some(last) = find_role(request, VideoInputRole::LastFrame) {
            payload["last_frame_image"] = json!(last.uri);
        }
        let response: CreateResponse = http::execute_json(
            PROVIDER_ID,
            &self.api_key,
            self.authenticated(self.client.post(self.endpoint("/v1/video_generation")))
                .json(&payload),
        )
        .await
        .map_err(submission_error)?;
        ensure_base_response(&response.base_resp, &self.api_key)?;
        let task_id = scalar_string(&response.task_id).ok_or_else(|| {
            submission_response_error(configuration_error("MiniMax returned an invalid task ID"))
        })?;
        let task_id = validate_task_id(&task_id).map_err(submission_response_error)?;
        Ok(SubmittedJob {
            provider_task_id: task_id,
            provider_source: self.provider_source.clone(),
            estimated_cost: pricing_estimate(PROVIDER_ID, request)?,
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
            self.authenticated(self.client.get(self.endpoint("/v1/query/video_generation")))
                .query(&[("task_id", task_id.as_str())]),
        )
        .await?;
        ensure_base_response(&response.base_resp, &self.api_key)?;
        if response
            .task_id
            .as_ref()
            .and_then(scalar_string)
            .is_some_and(|returned| returned != task_id)
        {
            return Err(configuration_error(
                "MiniMax query returned a different task ID",
            ));
        }
        let normalized_status = response.status.to_ascii_lowercase();
        let state = match normalized_status.as_str() {
            "preparing" | "queueing" | "queued" => ProviderJobState::Queued,
            "processing" | "running" => ProviderJobState::Running,
            "success" | "succeeded" => ProviderJobState::Succeeded,
            "fail" | "failed" => ProviderJobState::Failed,
            "cancelled" | "canceled" => ProviderJobState::Cancelled,
            _ => ProviderJobState::ProviderUnknown,
        };
        let result = if state == ProviderJobState::Succeeded {
            let file_id = response
                .file_id
                .as_ref()
                .and_then(scalar_string)
                .ok_or_else(|| configuration_error("Succeeded MiniMax task has no file ID"))?;
            let file: RetrieveResponse = http::execute_json(
                PROVIDER_ID,
                &self.api_key,
                self.authenticated(self.client.get(self.endpoint("/v1/files/retrieve")))
                    .query(&[("file_id", file_id.as_str())]),
            )
            .await?;
            ensure_base_response(&file.base_resp, &self.api_key)?;
            Some(ProviderJobResult {
                provider_task_id: task_id.clone(),
                outputs: vec![ProviderOutputLocator {
                    uri: file.file.download_url,
                    media_type: "video/mp4".to_string(),
                    expires_hint: Some("temporary_download_promptly".to_string()),
                }],
                width: response.video_width,
                height: response.video_height,
                duration_ms: None,
            })
        } else {
            None
        };
        let error = (state == ProviderJobState::Failed).then(|| NormalizedProviderError {
            provider_id: PROVIDER_ID.to_string(),
            code: "generation_failed".to_string(),
            message: http::sanitize_message(
                response
                    .error_message
                    .as_deref()
                    .unwrap_or("MiniMax generation failed"),
                Some(&self.api_key),
            ),
            retryable: false,
            retry_after_seconds: None,
            http_status: None,
            request_id: None,
        });
        Ok(ProviderJobStatus {
            provider_task_id: task_id,
            state,
            raw_status: response.status,
            result,
            error,
            billed_usage: None,
            final_cost_micros: None,
        })
    }

    async fn cancel(
        &self,
        request: &ProviderCancellationRequest,
    ) -> Result<CancellationResult, NormalizedProviderError> {
        let task_id = validate_task_id(&request.provider_task_id)?;
        Err(NormalizedProviderError {
            provider_id: PROVIDER_ID.to_string(),
            code: "cancellation_unsupported".to_string(),
            message: format!(
                "Legacy MiniMax task {task_id} cannot be cancelled through the published API"
            ),
            retryable: false,
            retry_after_seconds: None,
            http_status: None,
            request_id: None,
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

fn valid_duration_resolution(request: &NormalizedVideoRequest) -> bool {
    matches!(
        (
            request.model_id.as_str(),
            request.operation,
            request.resolution.as_str(),
            request.duration_seconds,
        ),
        (HAILUO_02, MediaOperation::ImageToVideo, "512P", 6 | 10)
            | (_, _, "768P", 6 | 10)
            | (_, _, "1080P", 6)
    )
}

fn count_role(request: &NormalizedVideoRequest, role: VideoInputRole) -> usize {
    request
        .input_assets
        .iter()
        .filter(|asset| asset.role == role)
        .count()
}

fn find_role(
    request: &NormalizedVideoRequest,
    role: VideoInputRole,
) -> Option<&super::VideoInputAsset> {
    request.input_assets.iter().find(|asset| asset.role == role)
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

fn ensure_base_response(
    response: &Option<BaseResponse>,
    api_key: &str,
) -> Result<(), NormalizedProviderError> {
    let Some(response) = response else {
        return Ok(());
    };
    if response.status_code == 0 {
        return Ok(());
    }
    Err(NormalizedProviderError {
        provider_id: PROVIDER_ID.to_string(),
        code: response.status_code.to_string(),
        message: http::sanitize_message(&response.status_msg, Some(api_key)),
        retryable: matches!(response.status_code, 1002 | 1039),
        retry_after_seconds: None,
        http_status: Some(200),
        request_id: None,
    })
}

fn scalar_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(ToString::to_string)
        .or_else(|| value.as_u64().map(|value| value.to_string()))
        .filter(|value| !value.trim().is_empty())
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
struct BaseResponse {
    status_code: i64,
    #[serde(default)]
    status_msg: String,
}

#[derive(Deserialize)]
struct CreateResponse {
    task_id: Value,
    base_resp: Option<BaseResponse>,
}

#[derive(Deserialize)]
struct QueryResponse {
    task_id: Option<Value>,
    status: String,
    file_id: Option<Value>,
    video_width: Option<u32>,
    video_height: Option<u32>,
    error_message: Option<String>,
    base_resp: Option<BaseResponse>,
}

#[derive(Deserialize)]
struct RetrieveResponse {
    file: RetrievedFile,
    base_resp: Option<BaseResponse>,
}

#[derive(Deserialize)]
struct RetrievedFile {
    download_url: String,
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;
    use crate::media_generation::adapters::VideoInputAsset;

    fn request(model_id: &str, operation: MediaOperation) -> NormalizedVideoRequest {
        NormalizedVideoRequest {
            idempotency_key: "job-1-attempt-1".to_string(),
            model_id: model_id.to_string(),
            operation,
            prompt: "A quiet ocean at dawn".to_string(),
            duration_seconds: 6,
            resolution: "768P".to_string(),
            aspect_ratio: "adaptive".to_string(),
            input_assets: Vec::new(),
            seed: None,
            generate_audio: None,
            callback_url: None,
        }
    }

    #[test]
    fn validation_keeps_legacy_model_matrices_separate() {
        let adapter = MiniMaxHailuoVideoAdapter::new("secret", "credential-1").unwrap();
        assert!(
            adapter
                .validate(&request(HAILUO_23, MediaOperation::TextToVideo))
                .valid
        );
        assert!(
            !adapter
                .validate(&request(HAILUO_23_FAST, MediaOperation::TextToVideo))
                .valid
        );

        let mut image = request(HAILUO_02, MediaOperation::ImageToVideo);
        image.resolution = "512P".to_string();
        image.duration_seconds = 10;
        image.input_assets.push(VideoInputAsset {
            role: VideoInputRole::FirstFrame,
            uri: "https://cdn.example.com/first.png".to_string(),
            media_type: "image/png".to_string(),
            metadata_verified: true,
            byte_length: Some(1024),
            content_hash_sha256: None,
            local_asset_id: None,
            width: Some(1024),
            height: Some(768),
            duration_ms: None,
            frame_rate: None,
            video_codec: None,
        });
        assert!(adapter.validate(&image).valid);
    }

    #[test]
    fn provider_source_distinguishes_v1_from_v2() {
        let v1 = MiniMaxHailuoVideoAdapter::new("secret", "credential-1").unwrap();
        let v2 = super::super::MiniMaxVideoAdapter::new("secret", "credential-1").unwrap();
        assert_ne!(v1.provider_source(), v2.provider_source());
    }

    #[test]
    fn test_constructor_accepts_local_endpoint_without_exposing_it_publicly() {
        let adapter = MiniMaxHailuoVideoAdapter::for_test("http://127.0.0.1:12345");
        assert_eq!(adapter.provider_id(), PROVIDER_ID);
    }

    #[tokio::test]
    async fn submit_uses_legacy_contract_and_checks_business_status() {
        let response =
            r#"{"task_id":"106916112212032","base_resp":{"status_code":0,"status_msg":"success"}}"#;
        let (base_url, captured) = serve_once(response).await;
        let adapter = MiniMaxHailuoVideoAdapter::for_test(&base_url);
        let submitted = adapter
            .submit(&request(HAILUO_23, MediaOperation::TextToVideo))
            .await
            .unwrap();
        assert_eq!(submitted.provider_task_id, "106916112212032");
        let request = captured.lock().unwrap().clone();
        assert!(request.starts_with("POST /v1/video_generation HTTP/1.1"));
        assert!(!request.to_ascii_lowercase().contains("idempotency"));
    }

    #[tokio::test]
    async fn http_200_business_error_is_normalized() {
        let response = r#"{"task_id":"","base_resp":{"status_code":1002,"status_msg":"Authorization: Bearer test-secret rate limited"}}"#;
        let (base_url, _) = serve_once(response).await;
        let adapter = MiniMaxHailuoVideoAdapter::for_test(&base_url);
        let error = adapter
            .submit(&request(HAILUO_23, MediaOperation::TextToVideo))
            .await
            .unwrap_err();
        assert_eq!(error.code, "1002");
        assert!(error.retryable);
        assert!(!error.message.contains("test-secret"));
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
