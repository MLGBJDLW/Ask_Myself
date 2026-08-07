use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::CoreError;

use super::adapters::{
    CostEstimate, MiniMaxHailuoVideoAdapter, MiniMaxVideoAdapter, NormalizedProviderError,
    NormalizedVideoRequest, ProviderCancellationRequest, ProviderJobResult, ProviderJobState,
    RunwayVideoAdapter, VideoGenerationAdapter,
};
use super::{
    BeginMediaJobAttemptRequest, EnqueuePreparedVideoVariantsRequest, ImportMediaAssetRequest,
    LinkMediaAssetRequest, MediaAssetLocalRetentionPolicy, MediaAssetRelationType,
    MediaGenerationRuntime, MediaJobAttemptState, MediaJobSnapshot, MediaJobState,
    RecordMediaProviderEventRequest, RequestMediaJobCancellation,
    SelectVideoWorkflowVariantRequest, TransitionMediaJobRequest, VideoVariantExecutionContext,
    VideoWorkflowSnapshot,
};

const MAX_OUTPUT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_CONCURRENT_SUBMISSIONS: usize = 3;
const MAX_CONCURRENT_REFERENCE_INSPECTIONS: usize = 2;
const MAX_STATUS_LOOKUP_FAILURES: u32 = 12;
const MAX_OUTPUT_MATERIALIZATION_FAILURES: u32 = 12;
const MAX_CANCELLATION_ATTEMPTS: u32 = 5;
const MAX_POLL_BACKOFF_SECONDS: u64 = 300;
const MAX_TOTAL_REFERENCE_BYTES: u64 = 32 * 1024 * 1024;
const VIDEO_JOB_LEASE_TTL_SECONDS: i64 = 600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueVideoShotVariantsRequest {
    pub workflow_id: String,
    pub expected_workflow_revision: u64,
    pub shot_id: String,
    pub expected_shot_revision: u64,
    pub idempotency_key: String,
    pub count: u32,
    pub expected_connection_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewVideoShotQueueRequest {
    pub workflow_id: String,
    pub expected_workflow_revision: u64,
    pub shot_id: String,
    pub expected_shot_revision: u64,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoQueueInputDisclosure {
    pub ordinal: u32,
    pub role: String,
    pub uri: String,
    pub media_type: String,
    pub byte_length: Option<u64>,
    pub content_hash_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoQueueDisclosure {
    pub workflow_id: String,
    pub shot_id: String,
    pub shot_revision: u64,
    pub provider_id: String,
    pub model_id: String,
    pub api_version: Option<String>,
    pub official_base_url: String,
    pub connection_id: String,
    pub connection_revision: u64,
    pub connection_name: String,
    pub credential_scope: String,
    pub data_region: Option<String>,
    pub retention_policy: String,
    pub deletion_policy: String,
    pub ordered_inputs: Vec<VideoQueueInputDisclosure>,
    pub count: u32,
    pub estimated_cost_micros_per_variant: Option<i64>,
    pub estimated_cost_micros_total: Option<i64>,
    pub currency: Option<String>,
    pub cross_provider_fallback_authorized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedVideoReferenceImage {
    pub uri: String,
    pub media_type: String,
    pub byte_length: u64,
    pub content_hash_sha256: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryVideoVariantRequest {
    pub job_id: String,
    pub expected_job_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelVideoVariantRequest {
    pub job_id: String,
    pub expected_job_revision: u64,
    pub reason: String,
    #[serde(default)]
    pub allow_terminal_record_deletion: bool,
}

#[derive(Clone)]
pub struct VideoGenerationCoordinator {
    runtime: MediaGenerationRuntime,
    in_flight: Arc<tokio::sync::Mutex<HashSet<String>>>,
    submission_permits: Arc<tokio::sync::Semaphore>,
    reference_inspection_permits: Arc<tokio::sync::Semaphore>,
    download_root: Arc<PathBuf>,
    poll_interval: Duration,
}

impl VideoGenerationCoordinator {
    pub fn new(runtime: MediaGenerationRuntime, download_root: PathBuf) -> Self {
        Self {
            runtime,
            in_flight: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
            submission_permits: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_SUBMISSIONS)),
            reference_inspection_permits: Arc::new(tokio::sync::Semaphore::new(
                MAX_CONCURRENT_REFERENCE_INSPECTIONS,
            )),
            download_root: Arc::new(download_root),
            poll_interval: Duration::from_secs(5),
        }
    }

    pub async fn queue_shot_variants(
        &self,
        request: QueueVideoShotVariantsRequest,
    ) -> Result<VideoWorkflowSnapshot, CoreError> {
        if request.idempotency_key.trim().is_empty() || request.idempotency_key.len() > 400 {
            return Err(CoreError::InvalidInput(
                "Queue idempotency key must contain 1-400 bytes".to_string(),
            ));
        }
        let before = self
            .runtime
            .get_video_workflow(&request.workflow_id)
            .await?;
        let shot = before
            .shots
            .iter()
            .find(|candidate| candidate.shot.id == request.shot_id)
            .map(|candidate| candidate.shot.clone())
            .ok_or_else(|| CoreError::NotFound(format!("Video shot {}", request.shot_id)))?;
        let connection_id = shot.connection_id.as_deref().ok_or_else(|| {
            CoreError::InvalidInput("Configure a provider connection before queueing".to_string())
        })?;
        let credential = self
            .runtime
            .materialize_video_provider_connection(connection_id)
            .await?;
        if credential.record.revision != request.expected_connection_revision {
            return Err(CoreError::Conflict(
                "Provider connection changed after queue disclosure; review the transfer again"
                    .to_string(),
            ));
        }
        let adapter = build_adapter(&credential, &shot)?;
        let mut normalized_request = normalized_request(&shot, &request.idempotency_key)?;
        validate_request(adapter.as_ref(), &normalized_request)?;
        self.preflight_inputs(&mut normalized_request, true).await?;
        let submission_request = self.materialize_local_inputs(&normalized_request).await?;
        validate_request(adapter.as_ref(), &submission_request)?;
        let estimate = adapter
            .estimate_cost(&normalized_request)
            .await
            .map_err(provider_error)?;
        let provider_source = adapter.provider_source().to_string();
        let existing_jobs = before
            .shots
            .iter()
            .flat_map(|shot| shot.variants.iter().map(|variant| variant.job_id.clone()))
            .collect::<HashSet<_>>();
        let queued = self
            .runtime
            .enqueue_video_workflow_variants(EnqueuePreparedVideoVariantsRequest {
                workflow_id: request.workflow_id.clone(),
                expected_workflow_revision: request.expected_workflow_revision,
                shot_id: request.shot_id,
                expected_shot_revision: request.expected_shot_revision,
                idempotency_key: request.idempotency_key,
                count: request.count,
                expected_connection_revision: request.expected_connection_revision,
                provider_source,
                normalized_request,
                estimated_cost_micros: micros(&estimate)?,
                currency: estimate.currency,
            })
            .await?;
        let new_job_ids = queued
            .shots
            .iter()
            .flat_map(|shot| shot.variants.iter())
            .filter(|variant| {
                !existing_jobs.contains(&variant.job_id)
                    || variant.job.state == MediaJobState::Draft
            })
            .map(|variant| variant.job_id.clone())
            .collect::<Vec<_>>();
        for job_id in new_job_ids {
            let context = self
                .runtime
                .video_variant_execution_context(&job_id)
                .await?;
            self.submit_context(context).await?;
        }
        self.runtime.get_video_workflow(&request.workflow_id).await
    }

    pub async fn preview_shot_queue(
        &self,
        request: PreviewVideoShotQueueRequest,
    ) -> Result<VideoQueueDisclosure, CoreError> {
        if !(1..=4).contains(&request.count) {
            return Err(CoreError::InvalidInput(
                "Variant batch size must be 1-4".to_string(),
            ));
        }
        let snapshot = self
            .runtime
            .get_video_workflow(&request.workflow_id)
            .await?;
        if snapshot.workflow.revision != request.expected_workflow_revision {
            return Err(CoreError::Conflict(
                "Video workflow changed before queue disclosure".to_string(),
            ));
        }
        let shot = snapshot
            .shots
            .iter()
            .find(|candidate| candidate.shot.id == request.shot_id)
            .map(|candidate| candidate.shot.clone())
            .ok_or_else(|| CoreError::NotFound(format!("Video shot {}", request.shot_id)))?;
        if shot.revision != request.expected_shot_revision {
            return Err(CoreError::Conflict(
                "Video shot changed before queue disclosure".to_string(),
            ));
        }
        let connection_id = shot.connection_id.as_deref().ok_or_else(|| {
            CoreError::InvalidInput("Configure a provider connection before queueing".to_string())
        })?;
        let credential = self
            .runtime
            .materialize_video_provider_connection(connection_id)
            .await?;
        let adapter = build_adapter(&credential, &shot)?;
        let mut normalized = normalized_request(&shot, "queue-disclosure")?;
        validate_request(adapter.as_ref(), &normalized)?;
        self.preflight_inputs(&mut normalized, false).await?;
        let estimate = adapter
            .estimate_cost(&normalized)
            .await
            .map_err(provider_error)?;
        let per_variant = micros(&estimate)?;
        let total = per_variant
            .map(|amount| {
                amount.checked_mul(i64::from(request.count)).ok_or_else(|| {
                    CoreError::InvalidInput("Estimated queue cost is too large".to_string())
                })
            })
            .transpose()?;
        let model = crate::video_provider_catalog::load_video_provider_presets()
            .map_err(CoreError::from)?
            .into_iter()
            .filter(|preset| preset.provider_id == shot.provider_id.clone().unwrap_or_default())
            .flat_map(|preset| preset.models)
            .find(|model| {
                Some(model.model_id.as_str()) == shot.model_id.as_deref()
                    && model.api_version == shot.api_version
            })
            .ok_or_else(|| CoreError::InvalidInput("Video manifest changed".to_string()))?;
        Ok(VideoQueueDisclosure {
            workflow_id: request.workflow_id,
            shot_id: shot.id.clone(),
            shot_revision: shot.revision,
            provider_id: shot.provider_id.clone().unwrap_or_default(),
            model_id: shot.model_id.clone().unwrap_or_default(),
            api_version: shot.api_version.clone(),
            official_base_url: credential.record.official_base_url,
            connection_id: credential.record.id,
            connection_revision: credential.record.revision,
            connection_name: credential.record.display_name,
            credential_scope: credential.record.credential_scope,
            data_region: shot.data_region.clone(),
            retention_policy: shot.retention_policy.clone(),
            deletion_policy: format!(
                "{}; terminal task record deletion requires explicit consent: {}",
                model.cancellation_scope, model.cancellation_may_delete_terminal_record
            ),
            ordered_inputs: shot
                .input_assets
                .iter()
                .enumerate()
                .map(|(ordinal, input)| VideoQueueInputDisclosure {
                    ordinal: u32::try_from(ordinal).unwrap_or(u32::MAX),
                    role: video_input_role_name(&input.role).to_string(),
                    uri: input.uri.clone(),
                    media_type: input.media_type.clone(),
                    byte_length: input.byte_length,
                    content_hash_sha256: input.content_hash_sha256.clone().unwrap_or_default(),
                })
                .collect(),
            count: request.count,
            estimated_cost_micros_per_variant: per_variant,
            estimated_cost_micros_total: total,
            currency: estimate.currency,
            cross_provider_fallback_authorized: false,
        })
    }

    pub async fn inspect_reference_image(
        &self,
        uri: String,
    ) -> Result<VerifiedVideoReferenceImage, CoreError> {
        let _permit = self
            .reference_inspection_permits
            .acquire()
            .await
            .map_err(|_| {
                CoreError::Internal("Reference inspection coordinator closed".to_string())
            })?;
        let uri = uri.trim().to_string();
        let inspected = super::adapters::inspect_public_reference_image(&uri)
            .await
            .map_err(provider_error)?;
        Ok(VerifiedVideoReferenceImage {
            uri,
            media_type: inspected.media_type,
            byte_length: inspected.byte_length,
            content_hash_sha256: inspected.content_hash_sha256,
            width: inspected.width,
            height: inspected.height,
        })
    }

    async fn preflight_inputs(
        &self,
        request: &mut NormalizedVideoRequest,
        import_to_cas: bool,
    ) -> Result<(), CoreError> {
        let _permit = self
            .reference_inspection_permits
            .acquire()
            .await
            .map_err(|_| {
                CoreError::Internal("Reference inspection coordinator closed".to_string())
            })?;
        let mut total_bytes = 0_u64;
        for input in &mut request.input_assets {
            let expected_hash = input.content_hash_sha256.as_deref().ok_or_else(|| {
                CoreError::InvalidInput(
                    "Reference input is missing its verified SHA-256".to_string(),
                )
            })?;
            let inspected = super::adapters::inspect_public_reference_image(&input.uri)
                .await
                .map_err(provider_error)?;
            if inspected.media_type != input.media_type
                || Some(inspected.byte_length) != input.byte_length
                || Some(inspected.width) != input.width
                || Some(inspected.height) != input.height
                || inspected.content_hash_sha256 != expected_hash
            {
                return Err(CoreError::Conflict(
                    "Reference input bytes changed after inspection; inspect and disclose the exact input again"
                        .to_string(),
                ));
            }
            total_bytes = total_bytes.saturating_add(inspected.byte_length);
            if total_bytes > MAX_TOTAL_REFERENCE_BYTES {
                return Err(CoreError::InvalidInput(
                    "Reference inputs exceed the 32 MiB per-shot transfer limit".to_string(),
                ));
            }
            if import_to_cas {
                let staging_root = self.download_root.join("reference-imports");
                tokio::fs::create_dir_all(&staging_root)
                    .await
                    .map_err(|error| {
                        CoreError::Internal(format!("Failed to create reference staging: {error}"))
                    })?;
                let staging_path = staging_root.join(format!("{}.bin", Uuid::new_v4()));
                tokio::fs::write(&staging_path, &inspected.bytes)
                    .await
                    .map_err(|error| {
                        CoreError::Internal(format!("Failed to stage reference bytes: {error}"))
                    })?;
                let imported = self
                    .runtime
                    .import_asset(ImportMediaAssetRequest {
                        source_path: staging_path.clone(),
                        declared_media_type: inspected.media_type,
                        expected_sha256: Some(inspected.content_hash_sha256),
                        expected_byte_length: Some(inspected.byte_length),
                        width: Some(inspected.width),
                        height: Some(inspected.height),
                        duration_ms: None,
                    })
                    .await;
                let _ = tokio::fs::remove_file(&staging_path).await;
                input.local_asset_id = Some(imported?.id);
            }
        }
        Ok(())
    }

    async fn materialize_local_inputs(
        &self,
        request: &NormalizedVideoRequest,
    ) -> Result<NormalizedVideoRequest, CoreError> {
        let mut materialized = request.clone();
        let mut total_bytes = 0_u64;
        for input in &mut materialized.input_assets {
            let asset_id = input.local_asset_id.as_deref().ok_or_else(|| {
                CoreError::Conflict("Queued reference input has no local CAS identity".to_string())
            })?;
            let path = self.runtime.resolve_asset_path(asset_id).await?;
            let metadata = tokio::fs::metadata(&path).await?;
            total_bytes = total_bytes.saturating_add(metadata.len());
            if total_bytes > MAX_TOTAL_REFERENCE_BYTES {
                return Err(CoreError::InvalidInput(
                    "Reference inputs exceed the 32 MiB per-shot transfer limit".to_string(),
                ));
            }
            if input.byte_length != Some(metadata.len()) {
                return Err(CoreError::Conflict(
                    "Local reference asset length no longer matches its queued identity"
                        .to_string(),
                ));
            }
            let bytes = tokio::fs::read(path).await?;
            let digest = format!("{:x}", Sha256::digest(&bytes));
            if input.content_hash_sha256.as_deref() != Some(&digest) {
                return Err(CoreError::Conflict(
                    "Local reference asset digest no longer matches its queued identity"
                        .to_string(),
                ));
            }
            input.uri = format!(
                "data:{};base64,{}",
                input.media_type,
                BASE64_STANDARD.encode(bytes)
            );
        }
        Ok(materialized)
    }

    pub async fn retry_variant(
        &self,
        request: RetryVideoVariantRequest,
    ) -> Result<VideoWorkflowSnapshot, CoreError> {
        let snapshot = self.runtime.get_job(&request.job_id).await?;
        if snapshot.job.revision != request.expected_job_revision {
            return Err(CoreError::Conflict(
                "Media job changed before retry was requested".to_string(),
            ));
        }
        let context = self
            .runtime
            .video_variant_execution_context(&request.job_id)
            .await?;
        let workflow_id = context.workflow_id.clone();
        match snapshot.job.state {
            MediaJobState::ProviderUnknown if snapshot.job.current_provider_task_id.is_some() => {
                self.start_observation(context).await;
            }
            MediaJobState::PostProcessing
                if snapshot
                    .attempts
                    .last()
                    .is_some_and(|attempt| attempt.error.is_some()) =>
            {
                self.runtime
                    .reset_video_materialization_failures(&request.job_id)
                    .await?;
                self.start_observation(context).await;
            }
            MediaJobState::Submitting
                if snapshot
                    .attempts
                    .last()
                    .is_some_and(|attempt| attempt.state == MediaJobAttemptState::Failed) =>
            {
                self.submit_context(context).await?;
            }
            _ => {
                return Err(CoreError::Conflict(
                    "Only output materialization, status reconciliation, or a classified transient submission can be retried"
                        .to_string(),
                ));
            }
        }
        self.runtime.get_video_workflow(&workflow_id).await
    }

    pub async fn cancel_variant(
        &self,
        request: CancelVideoVariantRequest,
    ) -> Result<VideoWorkflowSnapshot, CoreError> {
        let context = self
            .runtime
            .video_variant_execution_context(&request.job_id)
            .await?;
        let workflow_id = context.workflow_id.clone();
        let snapshot = self.runtime.get_job(&request.job_id).await?;
        if snapshot.job.revision != request.expected_job_revision {
            return Err(CoreError::Conflict(
                "Media job changed before cancellation was requested".to_string(),
            ));
        }
        if snapshot.job.state.is_terminal() {
            return Err(CoreError::Conflict(
                "Media job is already terminal".to_string(),
            ));
        }
        if snapshot.job.current_attempt_id.is_none() {
            self.runtime
                .transition_job(TransitionMediaJobRequest {
                    job_id: request.job_id,
                    expected_revision: snapshot.job.revision,
                    next_state: MediaJobState::Cancelled,
                })
                .await?;
            return self.runtime.get_video_workflow(&workflow_id).await;
        }
        if snapshot.job.current_provider_task_id.is_none() {
            return Err(CoreError::Conflict(
                "Provider submission is still in flight or requires reconciliation; retry cancellation after a durable task ID is available"
                    .to_string(),
            ));
        }
        self.runtime
            .authorize_video_variant_cancellation(
                &request.job_id,
                snapshot.job.revision,
                request.allow_terminal_record_deletion,
            )
            .await?;
        self.runtime
            .request_cancellation(RequestMediaJobCancellation {
                job_id: request.job_id.clone(),
                expected_revision: snapshot.job.revision,
                reason: request.reason,
            })
            .await?;
        let coordinator = self.clone();
        tokio::spawn(async move {
            if let Err(error) = coordinator
                .execute_provider_cancellation(context, request.allow_terminal_record_deletion)
                .await
            {
                tracing::warn!(error = %error, "video cancellation request failed");
            }
        });
        self.runtime.get_video_workflow(&workflow_id).await
    }

    async fn execute_provider_cancellation(
        &self,
        context: VideoVariantExecutionContext,
        allow_terminal_record_deletion: bool,
    ) -> Result<(), CoreError> {
        let lease_owner = format!("cancel-{}", Uuid::new_v4());
        if !self
            .runtime
            .try_acquire_video_job_lease(
                &context.job_id,
                "cancel",
                &lease_owner,
                VIDEO_JOB_LEASE_TTL_SECONDS,
            )
            .await?
        {
            return Ok(());
        }
        let result = self
            .execute_provider_cancellation_with_lease(
                context.clone(),
                allow_terminal_record_deletion,
                &lease_owner,
            )
            .await;
        let release = self
            .runtime
            .release_video_job_lease(&context.job_id, "cancel", &lease_owner)
            .await;
        result.and(release)
    }

    async fn execute_provider_cancellation_with_lease(
        &self,
        context: VideoVariantExecutionContext,
        allow_terminal_record_deletion: bool,
        lease_owner: &str,
    ) -> Result<(), CoreError> {
        let requested = self.runtime.get_job(&context.job_id).await?;
        if requested.job.state.is_terminal() {
            return Ok(());
        }
        let provider_task_id = requested
            .job
            .current_provider_task_id
            .clone()
            .ok_or_else(|| {
                CoreError::Conflict(
                    "Provider submission has no durable task ID; reconcile it before cancellation"
                        .to_string(),
                )
            })?;
        let credential = materialize_for_context(&self.runtime, &context).await?;
        let adapter = build_adapter(&credential, &context.shot)?;
        for cancellation_attempt in 1..=MAX_CANCELLATION_ATTEMPTS {
            if !self
                .runtime
                .renew_video_job_lease(
                    &context.job_id,
                    "cancel",
                    lease_owner,
                    VIDEO_JOB_LEASE_TTL_SECONDS,
                )
                .await?
            {
                return Ok(());
            }
            let outcome = match adapter
                .cancel(&ProviderCancellationRequest {
                    provider_task_id: provider_task_id.clone(),
                    allow_terminal_record_deletion,
                })
                .await
            {
                Ok(result) => {
                    let payload = serde_json::to_value(&result)?;
                    (
                        "provider.cancellation_result".to_string(),
                        payload.clone(),
                        None,
                        Some(payload),
                        result.confirmed,
                        false,
                        None,
                    )
                }
                Err(error) => {
                    let retryable = error.retryable;
                    let classification = retryable.then_some(error.code.clone());
                    let error_value = serde_json::to_value(&error)?;
                    (
                        "provider.cancellation_failed".to_string(),
                        json!({ "error": error_value }),
                        Some(error_value.clone()),
                        Some(json!({ "confirmed": false, "error": error_value })),
                        false,
                        retryable,
                        classification,
                    )
                }
            };
            let mut recorded = false;
            for _ in 0..3 {
                let latest = self.runtime.get_job(&context.job_id).await?;
                if latest.job.state.is_terminal() {
                    return Ok(());
                }
                let attempt_id = latest.job.current_attempt_id.clone().ok_or_else(|| {
                    CoreError::Conflict("Cancellation has no current provider attempt".to_string())
                })?;
                if latest.job.current_provider_task_id.as_deref() != Some(&provider_task_id) {
                    return Err(CoreError::Conflict(
                        "Cancellation task changed while recording its result".to_string(),
                    ));
                }
                let persisted = self
                    .runtime
                    .record_provider_event(RecordMediaProviderEventRequest {
                        job_id: latest.job.id.clone(),
                        expected_revision: latest.job.revision,
                        attempt_id: attempt_id.clone(),
                        provider_id: latest.job.provider_id.clone(),
                        event_source: latest.job.provider_source.clone(),
                        deduplication_key: format!(
                            "job:{}:attempt:{}:cancel:{}:{}",
                            latest.job.id,
                            attempt_id,
                            cancellation_attempt,
                            fingerprint(&outcome.1)
                        ),
                        event_kind: outcome.0.clone(),
                        payload: outcome.1.clone(),
                        provider_created_at: None,
                        provider_task_id: Some(provider_task_id.clone()),
                        attempt_state: outcome.4.then_some(MediaJobAttemptState::Cancelled),
                        next_job_state: outcome.4.then_some(MediaJobState::Cancelled),
                        error: outcome.2.clone(),
                        retry_classification: outcome.6.clone(),
                        next_eligible_at: outcome.5.then(|| {
                            (Utc::now()
                                + chrono::Duration::from_std(backoff_delay(cancellation_attempt))
                                    .unwrap_or_else(|_| chrono::Duration::seconds(300)))
                            .to_rfc3339_opts(SecondsFormat::Millis, true)
                        }),
                        cancellation_result: outcome.3.clone(),
                        final_cost_micros: None,
                        watermark_present: None,
                        provenance: None,
                    })
                    .await;
                match persisted {
                    Ok(_) => {
                        recorded = true;
                        break;
                    }
                    Err(CoreError::Conflict(_)) => continue,
                    Err(error) => return Err(error),
                }
            }
            if !recorded {
                return Err(CoreError::Conflict(
                    "Cancellation result raced repeated provider observations".to_string(),
                ));
            }
            if outcome.4 {
                return Ok(());
            }
            if outcome.5 && cancellation_attempt < MAX_CANCELLATION_ATTEMPTS {
                tokio::time::sleep(backoff_delay(cancellation_attempt)).await;
                continue;
            }
            self.start_observation(context).await;
            return Ok(());
        }
        Ok(())
    }

    pub async fn select_variant(
        &self,
        request: SelectVideoWorkflowVariantRequest,
    ) -> Result<VideoWorkflowSnapshot, CoreError> {
        self.runtime.select_video_workflow_variant(request).await
    }

    /// Resumes only jobs whose durable state proves that observation or a
    /// pre-submission local step is safe. Ambiguous submissions and classified
    /// transient failures remain visible for explicit reconciliation/retry.
    pub async fn resume(&self) -> Result<usize, CoreError> {
        let contexts = self.runtime.list_resumable_video_variant_contexts().await?;
        let count = contexts.len();
        for context in contexts {
            let snapshot = self.runtime.get_job(&context.job_id).await?;
            if snapshot.job.cancellation_requested_at.is_some()
                && snapshot.job.current_attempt_id.is_some()
            {
                let coordinator = self.clone();
                let authorized = context.cancel_terminal_record_deletion_authorized;
                tokio::spawn(async move {
                    if let Err(error) = coordinator
                        .execute_provider_cancellation(context, authorized)
                        .await
                    {
                        tracing::warn!(error = %error, "video cancellation recovery failed");
                    }
                });
                continue;
            }
            match snapshot.job.state {
                MediaJobState::Draft
                | MediaJobState::Validating
                | MediaJobState::UploadingAssets
                | MediaJobState::Submitting
                    if snapshot.job.current_attempt_id.is_none() =>
                {
                    let coordinator = self.clone();
                    tokio::spawn(async move {
                        if let Err(error) = coordinator.submit_context(context).await {
                            tracing::warn!(error = %error, "video variant resume submission failed");
                        }
                    });
                }
                MediaJobState::Queued
                | MediaJobState::Running
                | MediaJobState::PostProcessing
                | MediaJobState::ProviderUnknown
                    if snapshot.job.current_provider_task_id.is_some() =>
                {
                    self.start_observation(context).await;
                }
                _ => {}
            }
        }
        Ok(count)
    }

    async fn submit_context(&self, context: VideoVariantExecutionContext) -> Result<(), CoreError> {
        let _submission_permit =
            self.submission_permits.acquire().await.map_err(|_| {
                CoreError::Internal("Video submission coordinator closed".to_string())
            })?;
        let credential = materialize_for_context(&self.runtime, &context).await?;
        let adapter = build_adapter(&credential, &context.shot)?;
        let mut snapshot = self.runtime.get_job(&context.job_id).await?;
        let persisted_request: NormalizedVideoRequest =
            serde_json::from_value(snapshot.job.normalized_parameters.clone())?;
        let request = self.materialize_local_inputs(&persisted_request).await?;
        validate_request(adapter.as_ref(), &request)?;
        if snapshot.job.state == MediaJobState::Draft {
            snapshot = self
                .runtime
                .transition_job(TransitionMediaJobRequest {
                    job_id: snapshot.job.id.clone(),
                    expected_revision: snapshot.job.revision,
                    next_state: MediaJobState::Validating,
                })
                .await?;
        }
        if snapshot.job.state == MediaJobState::Validating {
            snapshot = self
                .runtime
                .transition_job(TransitionMediaJobRequest {
                    job_id: snapshot.job.id.clone(),
                    expected_revision: snapshot.job.revision,
                    next_state: MediaJobState::UploadingAssets,
                })
                .await?;
        }
        if snapshot.job.state == MediaJobState::UploadingAssets {
            snapshot = self
                .runtime
                .transition_job(TransitionMediaJobRequest {
                    job_id: snapshot.job.id.clone(),
                    expected_revision: snapshot.job.revision,
                    next_state: MediaJobState::Submitting,
                })
                .await?;
        }
        if snapshot.job.state != MediaJobState::Submitting {
            return Err(CoreError::Conflict(format!(
                "Media job {} is not eligible for provider submission",
                snapshot.job.id
            )));
        }
        let attempt_number = snapshot.attempts.len() + 1;
        let claim = self
            .runtime
            .begin_attempt_claim(BeginMediaJobAttemptRequest {
                job_id: snapshot.job.id.clone(),
                expected_revision: snapshot.job.revision,
                idempotency_key: format!(
                    "{}:attempt:{attempt_number}",
                    snapshot.job.idempotency_key
                ),
                provider_id: snapshot.job.provider_id.clone(),
                provider_source: adapter.provider_source().to_string(),
                model_id: snapshot.job.model_id.clone(),
                api_version: snapshot.job.api_version.clone(),
                data_region: snapshot.job.data_region.clone(),
                remote_retention_expires_at: snapshot.job.remote_retention_expires_at.clone(),
                provider_unknown_reconciliation: None,
            })
            .await?;
        if !claim.claimed {
            return Ok(());
        }
        snapshot = claim.snapshot;
        let attempt_id =
            snapshot.job.current_attempt_id.clone().ok_or_else(|| {
                CoreError::Internal("Provider attempt was not persisted".to_string())
            })?;
        match adapter.submit(&request).await {
            Ok(submitted) => {
                self.runtime
                    .record_provider_event(RecordMediaProviderEventRequest {
                        job_id: snapshot.job.id.clone(),
                        expected_revision: snapshot.job.revision,
                        attempt_id: attempt_id.clone(),
                        provider_id: snapshot.job.provider_id.clone(),
                        event_source: snapshot.job.provider_source.clone(),
                        deduplication_key: format!("submitted:{}", submitted.provider_task_id),
                        event_kind: "provider.submitted".to_string(),
                        payload: serde_json::to_value(&submitted)?,
                        provider_created_at: None,
                        provider_task_id: Some(submitted.provider_task_id),
                        attempt_state: Some(MediaJobAttemptState::Accepted),
                        next_job_state: Some(MediaJobState::Queued),
                        error: None,
                        retry_classification: None,
                        next_eligible_at: None,
                        cancellation_result: None,
                        final_cost_micros: None,
                        watermark_present: None,
                        provenance: Some(json!({
                            "providerSource": snapshot.job.provider_source,
                            "workflowId": context.workflow_id,
                            "shotId": context.shot.id,
                        })),
                    })
                    .await?;
                self.start_observation(context).await;
            }
            Err(error) => {
                let is_unknown = error.code == "submission_outcome_unknown";
                let retryable = error.retryable && !is_unknown;
                let next_job_state = if is_unknown {
                    MediaJobState::ProviderUnknown
                } else if retryable {
                    MediaJobState::Submitting
                } else {
                    MediaJobState::Failed
                };
                let attempt_state = if is_unknown {
                    MediaJobAttemptState::ProviderUnknown
                } else {
                    MediaJobAttemptState::Failed
                };
                self.runtime
                    .record_provider_event(RecordMediaProviderEventRequest {
                        job_id: snapshot.job.id.clone(),
                        expected_revision: snapshot.job.revision,
                        attempt_id: attempt_id.clone(),
                        provider_id: snapshot.job.provider_id.clone(),
                        event_source: snapshot.job.provider_source.clone(),
                        deduplication_key: format!(
                            "job:{}:attempt:{}:submit-error:{}:{}",
                            snapshot.job.id,
                            attempt_id,
                            attempt_number,
                            fingerprint(&serde_json::to_value(&error)?)
                        ),
                        event_kind: "provider.submission_failed".to_string(),
                        payload: json!({ "error": error }),
                        provider_created_at: None,
                        provider_task_id: None,
                        attempt_state: Some(attempt_state),
                        next_job_state: Some(next_job_state),
                        error: Some(serde_json::to_value(&error)?),
                        retry_classification: retryable.then_some(error.code.clone()),
                        next_eligible_at: retryable.then(|| {
                            let delay = error.retry_after_seconds.unwrap_or(0) as i64;
                            (Utc::now() + chrono::Duration::seconds(delay))
                                .to_rfc3339_opts(SecondsFormat::Millis, true)
                        }),
                        cancellation_result: None,
                        final_cost_micros: None,
                        watermark_present: None,
                        provenance: None,
                    })
                    .await?;
            }
        }
        Ok(())
    }

    async fn start_observation(&self, context: VideoVariantExecutionContext) {
        let lease_owner = format!("observe-{}", Uuid::new_v4());
        let acquired = self
            .runtime
            .try_acquire_video_job_lease(
                &context.job_id,
                "observe",
                &lease_owner,
                VIDEO_JOB_LEASE_TTL_SECONDS,
            )
            .await;
        if !matches!(acquired, Ok(true)) {
            if let Err(error) = acquired {
                tracing::warn!(job_id = context.job_id, error = %error, "video observation lease failed");
            }
            return;
        }
        let mut in_flight = self.in_flight.lock().await;
        if !in_flight.insert(context.job_id.clone()) {
            drop(in_flight);
            let _ = self
                .runtime
                .release_video_job_lease(&context.job_id, "observe", &lease_owner)
                .await;
            return;
        }
        drop(in_flight);
        let coordinator = self.clone();
        tokio::spawn(async move {
            let job_id = context.job_id.clone();
            if let Err(error) = coordinator.observe_context(context, &lease_owner).await {
                tracing::warn!(job_id, error = %error, "video provider observation stopped");
            }
            coordinator.in_flight.lock().await.remove(&job_id);
            let _ = coordinator
                .runtime
                .release_video_job_lease(&job_id, "observe", &lease_owner)
                .await;
        });
    }

    async fn observe_context(
        &self,
        context: VideoVariantExecutionContext,
        lease_owner: &str,
    ) -> Result<(), CoreError> {
        let credential = materialize_for_context(&self.runtime, &context).await?;
        let adapter = build_adapter(&credential, &context.shot)?;
        let mut lookup_failures = 0_u32;
        let mut materialization_failures = self
            .runtime
            .video_materialization_failure_count(&context.job_id)
            .await?;
        if materialization_failures >= MAX_OUTPUT_MATERIALIZATION_FAILURES {
            return Ok(());
        }
        loop {
            if !self
                .runtime
                .renew_video_job_lease(
                    &context.job_id,
                    "observe",
                    lease_owner,
                    VIDEO_JOB_LEASE_TTL_SECONDS,
                )
                .await?
            {
                return Ok(());
            }
            let snapshot = self.runtime.get_job(&context.job_id).await?;
            if snapshot.job.state.is_terminal() {
                return Ok(());
            }
            let provider_task_id = snapshot
                .job
                .current_provider_task_id
                .as_deref()
                .ok_or_else(|| {
                    CoreError::Conflict(
                        "Observed media job has no durable provider task ID".to_string(),
                    )
                })?;
            match adapter.get_status(provider_task_id).await {
                Ok(status) => {
                    lookup_failures = 0;
                    if snapshot.job.state == MediaJobState::PostProcessing {
                        if let Some(result) = status.result {
                            match self
                                .finish_outputs(
                                    snapshot,
                                    &context,
                                    adapter.as_ref(),
                                    result,
                                    lease_owner,
                                )
                                .await
                            {
                                Ok(()) => return Ok(()),
                                Err(error) => {
                                    self.record_post_processing_error(&context, &error).await;
                                    materialization_failures = self
                                        .runtime
                                        .increment_video_materialization_failure(&context.job_id)
                                        .await?;
                                    if materialization_failures
                                        >= MAX_OUTPUT_MATERIALIZATION_FAILURES
                                    {
                                        return Ok(());
                                    }
                                    tokio::time::sleep(backoff_delay(materialization_failures))
                                        .await;
                                    continue;
                                }
                            }
                        }
                    } else {
                        let terminal = matches!(
                            status.state,
                            ProviderJobState::Succeeded
                                | ProviderJobState::Failed
                                | ProviderJobState::Cancelled
                        );
                        let updated = match self.record_status(snapshot, &context, &status).await {
                            Ok(updated) => updated,
                            Err(CoreError::Conflict(_)) => continue,
                            Err(error) => return Err(error),
                        };
                        if status.state == ProviderJobState::Succeeded {
                            let result = status.result.ok_or_else(|| {
                                CoreError::Internal(
                                    "Provider succeeded without an output result".to_string(),
                                )
                            })?;
                            match self
                                .finish_outputs(
                                    updated,
                                    &context,
                                    adapter.as_ref(),
                                    result,
                                    lease_owner,
                                )
                                .await
                            {
                                Ok(()) => return Ok(()),
                                Err(error) => {
                                    self.record_post_processing_error(&context, &error).await;
                                    materialization_failures = self
                                        .runtime
                                        .increment_video_materialization_failure(&context.job_id)
                                        .await?;
                                    if materialization_failures
                                        >= MAX_OUTPUT_MATERIALIZATION_FAILURES
                                    {
                                        return Ok(());
                                    }
                                    tokio::time::sleep(backoff_delay(materialization_failures))
                                        .await;
                                    continue;
                                }
                            }
                        }
                        if terminal {
                            return Ok(());
                        }
                    }
                }
                Err(error) => {
                    lookup_failures = lookup_failures.saturating_add(1);
                    if !error.retryable || lookup_failures >= MAX_STATUS_LOOKUP_FAILURES {
                        self.record_lookup_failure(&context, &error).await?;
                        return Ok(());
                    }
                    tracing::warn!(
                        job_id = context.job_id,
                        code = error.code,
                        "video status lookup failed and will retry"
                    );
                    tokio::time::sleep(backoff_delay(lookup_failures)).await;
                    continue;
                }
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    async fn record_post_processing_error(
        &self,
        context: &VideoVariantExecutionContext,
        error: &CoreError,
    ) {
        let Ok(snapshot) = self.runtime.get_job(&context.job_id).await else {
            return;
        };
        if snapshot.job.state != MediaJobState::PostProcessing {
            return;
        }
        let Some(attempt_id) = snapshot.job.current_attempt_id.clone() else {
            return;
        };
        let error_value = json!({
            "code": "output_materialization_failed",
            "message": error.to_string(),
        });
        let _ = self
            .runtime
            .record_provider_event(RecordMediaProviderEventRequest {
                job_id: snapshot.job.id.clone(),
                expected_revision: snapshot.job.revision,
                attempt_id: attempt_id.clone(),
                provider_id: snapshot.job.provider_id.clone(),
                event_source: snapshot.job.provider_source.clone(),
                deduplication_key: format!(
                    "job:{}:attempt:{}:post-processing-error:{}",
                    snapshot.job.id,
                    attempt_id,
                    fingerprint(&error_value)
                ),
                event_kind: "local.output_materialization_failed".to_string(),
                payload: error_value.clone(),
                provider_created_at: None,
                provider_task_id: snapshot.job.current_provider_task_id.clone(),
                attempt_state: None,
                next_job_state: None,
                error: Some(error_value),
                retry_classification: Some("local_output_materialization".to_string()),
                next_eligible_at: None,
                cancellation_result: None,
                final_cost_micros: None,
                watermark_present: None,
                provenance: None,
            })
            .await;
    }

    async fn record_lookup_failure(
        &self,
        context: &VideoVariantExecutionContext,
        error: &NormalizedProviderError,
    ) -> Result<(), CoreError> {
        for _ in 0..3 {
            let snapshot = self.runtime.get_job(&context.job_id).await?;
            if snapshot.job.state.is_terminal() {
                return Ok(());
            }
            let attempt_id = snapshot.job.current_attempt_id.clone().ok_or_else(|| {
                CoreError::Conflict("Status lookup failure has no current attempt".to_string())
            })?;
            let error_value = serde_json::to_value(error)?;
            let recorded = self
                .runtime
                .record_provider_event(RecordMediaProviderEventRequest {
                    job_id: snapshot.job.id.clone(),
                    expected_revision: snapshot.job.revision,
                    attempt_id,
                    provider_id: snapshot.job.provider_id.clone(),
                    event_source: snapshot.job.provider_source.clone(),
                    deduplication_key: format!(
                        "status-lookup-terminal:{}:{}",
                        snapshot
                            .job
                            .current_provider_task_id
                            .as_deref()
                            .unwrap_or("unknown"),
                        fingerprint(&error_value)
                    ),
                    event_kind: "provider.status_lookup_unrecoverable".to_string(),
                    payload: json!({ "error": error_value }),
                    provider_created_at: None,
                    provider_task_id: snapshot.job.current_provider_task_id.clone(),
                    attempt_state: Some(MediaJobAttemptState::ProviderUnknown),
                    next_job_state: Some(MediaJobState::ProviderUnknown),
                    error: Some(error_value),
                    retry_classification: None,
                    next_eligible_at: None,
                    cancellation_result: None,
                    final_cost_micros: None,
                    watermark_present: None,
                    provenance: None,
                })
                .await;
            match recorded {
                Ok(_) => return Ok(()),
                Err(CoreError::Conflict(_)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(CoreError::Conflict(
            "Status lookup failure raced repeated job updates".to_string(),
        ))
    }

    async fn record_status(
        &self,
        snapshot: MediaJobSnapshot,
        context: &VideoVariantExecutionContext,
        status: &super::adapters::ProviderJobStatus,
    ) -> Result<MediaJobSnapshot, CoreError> {
        let (attempt_state, next_job_state) = match status.state {
            ProviderJobState::Queued => {
                if matches!(
                    snapshot.job.state,
                    MediaJobState::Queued | MediaJobState::Running
                ) {
                    (None, None)
                } else {
                    (
                        Some(MediaJobAttemptState::Accepted),
                        Some(MediaJobState::Queued),
                    )
                }
            }
            ProviderJobState::Running => {
                if snapshot.job.state == MediaJobState::Running {
                    (None, None)
                } else {
                    (
                        Some(MediaJobAttemptState::Observing),
                        Some(MediaJobState::Running),
                    )
                }
            }
            ProviderJobState::Succeeded => (
                Some(MediaJobAttemptState::Succeeded),
                Some(MediaJobState::PostProcessing),
            ),
            ProviderJobState::Failed => (
                Some(MediaJobAttemptState::Failed),
                Some(
                    if status.error.as_ref().is_some_and(|error| error.retryable) {
                        MediaJobState::Submitting
                    } else {
                        MediaJobState::Failed
                    },
                ),
            ),
            ProviderJobState::Cancelled => (
                Some(MediaJobAttemptState::Cancelled),
                Some(MediaJobState::Cancelled),
            ),
            ProviderJobState::ProviderUnknown => (
                Some(MediaJobAttemptState::ProviderUnknown),
                Some(MediaJobState::ProviderUnknown),
            ),
        };
        let attempt_id = snapshot.job.current_attempt_id.clone().ok_or_else(|| {
            CoreError::Conflict("Provider status has no current attempt".to_string())
        })?;
        let payload = serde_json::to_value(status)?;
        self.runtime
            .record_provider_event(RecordMediaProviderEventRequest {
                job_id: snapshot.job.id.clone(),
                expected_revision: snapshot.job.revision,
                attempt_id,
                provider_id: snapshot.job.provider_id.clone(),
                event_source: snapshot.job.provider_source.clone(),
                deduplication_key: format!(
                    "status:{}:{}",
                    status.provider_task_id,
                    fingerprint(&payload)
                ),
                event_kind: format!("provider.status.{}", status.raw_status),
                payload,
                provider_created_at: None,
                provider_task_id: Some(status.provider_task_id.clone()),
                attempt_state,
                next_job_state,
                error: status.error.as_ref().map(|error| json!(error)),
                retry_classification: status
                    .error
                    .as_ref()
                    .filter(|error| error.retryable)
                    .map(|error| error.code.clone()),
                next_eligible_at: status.error.as_ref().filter(|error| error.retryable).map(
                    |error| {
                        let delay = error.retry_after_seconds.unwrap_or(0) as i64;
                        (Utc::now() + chrono::Duration::seconds(delay))
                            .to_rfc3339_opts(SecondsFormat::Millis, true)
                    },
                ),
                cancellation_result: None,
                final_cost_micros: status
                    .final_cost_micros
                    .map(i64::try_from)
                    .transpose()
                    .map_err(|_| {
                        CoreError::InvalidInput("Final provider cost is too large".to_string())
                    })?,
                watermark_present: None,
                provenance: Some(json!({
                    "providerSource": snapshot.job.provider_source,
                    "workflowId": context.workflow_id,
                    "shotId": context.shot.id,
                    "billedUsage": status.billed_usage,
                })),
            })
            .await
    }

    async fn finish_outputs(
        &self,
        mut snapshot: MediaJobSnapshot,
        context: &VideoVariantExecutionContext,
        adapter: &dyn VideoGenerationAdapter,
        result: ProviderJobResult,
        lease_owner: &str,
    ) -> Result<(), CoreError> {
        let attempt_id = snapshot.job.current_attempt_id.clone().ok_or_else(|| {
            CoreError::Conflict("Provider output has no producing attempt".to_string())
        })?;
        let job_directory = self.download_root.join(&snapshot.job.id).join(lease_owner);
        if tokio::fs::try_exists(&job_directory).await.unwrap_or(false) {
            tokio::fs::remove_dir_all(&job_directory)
                .await
                .map_err(|error| {
                    CoreError::Internal(format!(
                        "Failed to clear stale video download directory: {error}"
                    ))
                })?;
        }
        tokio::fs::create_dir_all(&job_directory)
            .await
            .map_err(|error| {
                CoreError::Internal(format!(
                    "Failed to create video download directory: {error}"
                ))
            })?;
        let outcome = async {
            let downloaded = adapter
                .download_outputs(&result, &job_directory, MAX_OUTPUT_BYTES)
                .await
                .map_err(provider_error)?;
            if downloaded.is_empty() {
                Err(CoreError::Internal(
                    "Provider result contained no downloadable outputs".to_string(),
                ))?;
            }
            for (ordinal, output) in downloaded.into_iter().enumerate() {
                let asset = self
                    .runtime
                    .import_asset(ImportMediaAssetRequest {
                        source_path: output.path,
                        declared_media_type: output.detected_media_type,
                        expected_sha256: Some(output.sha256),
                        expected_byte_length: Some(output.byte_length),
                        width: result.width,
                        height: result.height,
                        duration_ms: result.duration_ms,
                    })
                    .await?;
                snapshot = self
                    .runtime
                    .link_asset(LinkMediaAssetRequest {
                        job_id: snapshot.job.id.clone(),
                        expected_revision: snapshot.job.revision,
                        idempotency_key: format!(
                            "output:{}:{}:{}",
                            snapshot.job.id, attempt_id, ordinal
                        ),
                        attempt_id: attempt_id.clone(),
                        asset_id: asset.id,
                        parent_asset_id: None,
                        relation_type: MediaAssetRelationType::Output,
                        ordinal: u32::try_from(ordinal).map_err(|_| {
                            CoreError::Internal("Provider output ordinal overflowed".to_string())
                        })?,
                        local_retention_policy: MediaAssetLocalRetentionPolicy::RetainUntilDeleted,
                        local_retention_expires_at: None,
                        metadata: json!({
                            "workflowId": context.workflow_id,
                            "shotId": context.shot.id,
                            "providerSource": snapshot.job.provider_source,
                        }),
                    })
                    .await?;
            }
            self.runtime
                .transition_job(TransitionMediaJobRequest {
                    job_id: snapshot.job.id,
                    expected_revision: snapshot.job.revision,
                    next_state: MediaJobState::Completed,
                })
                .await?;
            Ok(())
        }
        .await;
        if let Err(error) = tokio::fs::remove_dir_all(&job_directory).await {
            tracing::warn!(error = %error, "failed to remove imported video download staging directory");
        }
        outcome
    }
}

async fn materialize_for_context(
    runtime: &MediaGenerationRuntime,
    context: &VideoVariantExecutionContext,
) -> Result<super::MaterializedVideoProviderConnection, CoreError> {
    let connection_id = context.shot.connection_id.as_deref().ok_or_else(|| {
        CoreError::Conflict(
            "Variant shot no longer has the credential connection required for recovery"
                .to_string(),
        )
    })?;
    runtime
        .materialize_video_provider_connection(connection_id)
        .await
}

fn build_adapter(
    connection: &super::MaterializedVideoProviderConnection,
    shot: &super::VideoWorkflowShotRecord,
) -> Result<Box<dyn VideoGenerationAdapter>, CoreError> {
    let adapter: Box<dyn VideoGenerationAdapter> = match (
        connection.record.provider_id.as_str(),
        shot.api_version.as_deref(),
    ) {
        ("minimax", Some("v2")) => Box::new(
            MiniMaxVideoAdapter::new(&connection.api_key, &connection.record.credential_scope)
                .map_err(provider_error)?,
        ),
        ("minimax", Some("v1")) => Box::new(
            MiniMaxHailuoVideoAdapter::new(
                &connection.api_key,
                &connection.record.credential_scope,
            )
            .map_err(provider_error)?,
        ),
        ("runway", Some("2024-11-06")) => Box::new(
            RunwayVideoAdapter::new(&connection.api_key, &connection.record.credential_scope)
                .map_err(provider_error)?,
        ),
        _ => {
            return Err(CoreError::InvalidInput(
                "Shot provider and API version do not map to an enabled official adapter"
                    .to_string(),
            ))
        }
    };
    if shot.provider_id.as_deref() != Some(adapter.provider_id()) {
        return Err(CoreError::Conflict(
            "Shot provider no longer matches its credential connection".to_string(),
        ));
    }
    Ok(adapter)
}

fn normalized_request(
    shot: &super::VideoWorkflowShotRecord,
    idempotency_key: &str,
) -> Result<NormalizedVideoRequest, CoreError> {
    Ok(NormalizedVideoRequest {
        idempotency_key: idempotency_key.to_string(),
        model_id: shot
            .model_id
            .clone()
            .ok_or_else(|| CoreError::InvalidInput("Shot has no configured model".to_string()))?,
        operation: shot.operation,
        prompt: shot.prompt.clone(),
        duration_seconds: shot.duration_seconds,
        resolution: shot.resolution.clone(),
        aspect_ratio: shot.aspect_ratio.clone(),
        input_assets: shot.input_assets.clone(),
        seed: shot.seed,
        generate_audio: shot.generate_audio,
        callback_url: None,
    })
}

fn validate_request(
    adapter: &dyn VideoGenerationAdapter,
    request: &NormalizedVideoRequest,
) -> Result<(), CoreError> {
    let validation = adapter.validate(request);
    if validation.valid {
        return Ok(());
    }
    Err(CoreError::InvalidInput(
        validation
            .issues
            .into_iter()
            .map(|issue| format!("{}: {}", issue.field, issue.message))
            .collect::<Vec<_>>()
            .join("; "),
    ))
}

fn micros(estimate: &CostEstimate) -> Result<Option<i64>, CoreError> {
    estimate
        .amount_micros
        .map(i64::try_from)
        .transpose()
        .map_err(|_| CoreError::InvalidInput("Estimated provider cost is too large".to_string()))
}

fn provider_error(error: NormalizedProviderError) -> CoreError {
    CoreError::InvalidInput(format!("{}: {}", error.code, error.message))
}

fn fingerprint(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn backoff_delay(failures: u32) -> Duration {
    let multiplier = 1_u64 << failures.saturating_sub(1).min(6);
    Duration::from_secs(
        5_u64
            .saturating_mul(multiplier)
            .min(MAX_POLL_BACKOFF_SECONDS),
    )
}

fn video_input_role_name(role: &super::adapters::VideoInputRole) -> &'static str {
    match role {
        super::adapters::VideoInputRole::FirstFrame => "first_frame",
        super::adapters::VideoInputRole::LastFrame => "last_frame",
        super::adapters::VideoInputRole::InputVideo => "input_video",
        super::adapters::VideoInputRole::ReferenceImage => "reference_image",
        super::adapters::VideoInputRole::ReferenceVideo => "reference_video",
        super::adapters::VideoInputRole::ReferenceAudio => "reference_audio",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::db_executor::DatabaseExecutor;
    use crate::media_generation::{MediaGenerationAssetStore, MediaOperation};

    #[tokio::test]
    async fn provider_submission_materializes_the_exact_cas_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("reference.png");
        let bytes = BASE64_STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl2nWQAAAAASUVORK5CYII=")
            .unwrap();
        std::fs::write(&source_path, &bytes).unwrap();
        let runtime = MediaGenerationRuntime::with_asset_store(
            DatabaseExecutor::new(Database::open_memory().unwrap(), 8).unwrap(),
            MediaGenerationAssetStore::new(directory.path().join("assets")),
        );
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let asset = runtime
            .import_asset(ImportMediaAssetRequest {
                source_path,
                declared_media_type: "image/png".to_string(),
                expected_sha256: Some(hash.clone()),
                expected_byte_length: Some(bytes.len() as u64),
                width: Some(1),
                height: Some(1),
                duration_ms: None,
            })
            .await
            .unwrap();
        let coordinator =
            VideoGenerationCoordinator::new(runtime, directory.path().join("generation-downloads"));
        let request = NormalizedVideoRequest {
            idempotency_key: "cas-input-test".to_string(),
            model_id: "MiniMax-H3".to_string(),
            operation: MediaOperation::ImageToVideo,
            prompt: "Animate the reference".to_string(),
            duration_seconds: 4,
            resolution: "768P".to_string(),
            aspect_ratio: "adaptive".to_string(),
            input_assets: vec![super::super::adapters::VideoInputAsset {
                role: super::super::adapters::VideoInputRole::FirstFrame,
                uri: "https://cdn.example.com/reference.png".to_string(),
                media_type: "image/png".to_string(),
                metadata_verified: true,
                byte_length: Some(bytes.len() as u64),
                content_hash_sha256: Some(hash),
                local_asset_id: Some(asset.id),
                width: Some(1),
                height: Some(1),
                duration_ms: None,
                frame_rate: None,
                video_codec: None,
            }],
            seed: None,
            generate_audio: None,
            callback_url: None,
        };
        let materialized = coordinator
            .materialize_local_inputs(&request)
            .await
            .unwrap();
        assert_eq!(
            materialized.input_assets[0].uri,
            format!("data:image/png;base64,{}", BASE64_STANDARD.encode(bytes))
        );
        assert_eq!(
            request.input_assets[0].uri,
            "https://cdn.example.com/reference.png"
        );
    }
}
