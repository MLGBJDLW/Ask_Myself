use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::codec::{FramedRead, LinesCodec};

use crate::error::CoreError;

use super::timeline::{
    export_owned_partial_path, validate_destination_path, validate_retry_destination_path,
    VideoTimelineExportExecutionPlan, VideoTimelineExportInputPlan,
};
use super::{
    CancelVideoTimelineExportRequest, CreateVideoTimelineExportRequest, ImportMediaAssetRequest,
    MediaGenerationRuntime, RetryVideoTimelineExportRequest, VideoTimelineExportRecord,
    VideoTimelineExportStageKind, VideoTimelineExportState, VideoTimelineOutputProfile,
};

const MAX_EXPORT_CONCURRENCY: usize = 2;
const MAX_CAPTURE_BYTES: usize = 64 * 1024;
const PROCESS_CONTROL_TICK: Duration = Duration::from_millis(500);
const PROCESS_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);
const LEASE_RENEW_INTERVAL: Duration = Duration::from_secs(60);
const LEASE_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const CHILD_GRACE_PERIOD: Duration = Duration::from_secs(2);
const CHILD_REAP_TIMEOUT: Duration = Duration::from_secs(5);
const TOOL_IDENTITY_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_TOOL_BINARY_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone)]
pub struct VideoTimelineExportCoordinator {
    runtime: MediaGenerationRuntime,
    staging_root: PathBuf,
    ffmpeg_program: String,
    active: Arc<Mutex<HashSet<String>>>,
    permits: Arc<Semaphore>,
}

impl VideoTimelineExportCoordinator {
    pub fn new(
        runtime: MediaGenerationRuntime,
        staging_root: PathBuf,
        ffmpeg_program: String,
    ) -> Self {
        Self {
            runtime,
            staging_root,
            ffmpeg_program,
            active: Arc::new(Mutex::new(HashSet::new())),
            permits: Arc::new(Semaphore::new(MAX_EXPORT_CONCURRENCY)),
        }
    }

    pub async fn create_export(
        &self,
        mut request: CreateVideoTimelineExportRequest,
    ) -> Result<VideoTimelineExportRecord, CoreError> {
        if let Some(existing) = self
            .runtime
            .find_video_timeline_export_by_idempotency(
                &request.workflow_id,
                &request.idempotency_key,
            )
            .await?
        {
            let destination = tokio::task::spawn_blocking({
                let destination = request.destination_path.clone();
                move || validate_retry_destination_path(&destination)
            })
            .await
            .map_err(|error| {
                CoreError::Internal(format!("Replay destination validation failed: {error}"))
            })??;
            if Path::new(&existing.destination_path) != destination
                || existing.output_profile != request.output_profile
            {
                return Err(CoreError::Conflict(
                    "Export idempotency key was already used for a different request".to_string(),
                ));
            }
            if !matches!(
                existing.state,
                VideoTimelineExportState::Completed
                    | VideoTimelineExportState::Failed
                    | VideoTimelineExportState::Cancelled
            ) {
                self.ensure_running(&existing.id).await;
            }
            return Ok(existing);
        }
        let destination = tokio::task::spawn_blocking({
            let destination = request.destination_path.clone();
            move || validate_destination_path(&destination)
        })
        .await
        .map_err(|error| {
            CoreError::Internal(format!("Destination validation failed: {error}"))
        })??;
        request.destination_path = destination.to_string_lossy().into_owned();
        let toolchain = self.resolve_toolchain().await?;
        let record = self
            .runtime
            .create_video_timeline_export(request, toolchain.identity)
            .await?;
        self.ensure_running(&record.id).await;
        Ok(record)
    }

    pub async fn cancel_export(
        &self,
        request: CancelVideoTimelineExportRequest,
    ) -> Result<VideoTimelineExportRecord, CoreError> {
        let record = self.runtime.cancel_video_timeline_export(request).await?;
        self.ensure_running(&record.id).await;
        Ok(record)
    }

    pub async fn retry_export(
        &self,
        mut request: RetryVideoTimelineExportRequest,
    ) -> Result<VideoTimelineExportRecord, CoreError> {
        let destination = tokio::task::spawn_blocking({
            let destination = request.destination_path.clone();
            move || validate_retry_destination_path(&destination)
        })
        .await
        .map_err(|error| {
            CoreError::Internal(format!("Retry destination validation failed: {error}"))
        })??;
        request.destination_path = destination.to_string_lossy().into_owned();
        let record = self.runtime.retry_video_timeline_export(request).await?;
        self.ensure_running(&record.id).await;
        Ok(record)
    }

    pub async fn resume(&self) -> Result<usize, CoreError> {
        let interrupted = self
            .runtime
            .mark_live_video_timeline_exports_interrupted(epoch_seconds())
            .await?;
        if interrupted > 0 {
            tracing::info!(
                interrupted,
                "marked orphaned timeline export stages interrupted"
            );
        }
        let export_ids = self.runtime.list_resumable_video_timeline_exports().await?;
        for export_id in &export_ids {
            self.ensure_running(export_id).await;
        }
        Ok(export_ids.len())
    }

    async fn ensure_running(&self, export_id: &str) {
        let mut active = self.active.lock().await;
        if !active.insert(export_id.to_string()) {
            return;
        }
        drop(active);
        let this = self.clone();
        let export_id = export_id.to_string();
        tokio::spawn(async move {
            let _permit = match this.permits.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => {
                    this.active.lock().await.remove(&export_id);
                    return;
                }
            };
            if let Err(error) = this.run_export(&export_id).await {
                tracing::warn!(export_id, error = %error, "timeline export coordinator stopped");
            }
            this.active.lock().await.remove(&export_id);
        });
    }

    async fn run_export(&self, export_id: &str) -> Result<(), CoreError> {
        let owner_id = format!("{}:{}", std::process::id(), uuid::Uuid::new_v4());
        loop {
            if self
                .runtime
                .try_acquire_video_timeline_export_lease(export_id, &owner_id, epoch_seconds())
                .await?
            {
                break;
            }
            let plan = self
                .runtime
                .video_timeline_export_execution_plan(export_id)
                .await?;
            if matches!(
                plan.export.state,
                VideoTimelineExportState::Completed
                    | VideoTimelineExportState::Failed
                    | VideoTimelineExportState::Cancelled
            ) {
                return Ok(());
            }
            tokio::time::sleep(LEASE_RETRY_INTERVAL).await;
        }
        let result = self.run_owned_export(export_id, &owner_id).await;
        let handled = match result {
            Ok(()) => Ok(()),
            Err(ExportRunError::Cancelled) => {
                self.cleanup_owned_artifacts(export_id, &owner_id).await;
                if self
                    .runtime
                    .renew_video_timeline_export_lease(export_id, &owner_id, epoch_seconds())
                    .await?
                {
                    self.runtime
                        .mark_video_timeline_export_cancelled(export_id, &owner_id, epoch_seconds())
                        .await?;
                }
                Ok(())
            }
            Err(ExportRunError::LeaseLost) => {
                self.cleanup_owned_artifacts(export_id, &owner_id).await;
                Ok(())
            }
            Err(ExportRunError::Failed {
                stage_ordinal,
                code,
                message,
            }) => {
                self.cleanup_owned_artifacts(export_id, &owner_id).await;
                if self
                    .runtime
                    .renew_video_timeline_export_lease(export_id, &owner_id, epoch_seconds())
                    .await?
                {
                    if self
                        .runtime
                        .video_timeline_export_cancel_requested(export_id)
                        .await?
                    {
                        self.runtime
                            .mark_video_timeline_export_cancelled(
                                export_id,
                                &owner_id,
                                epoch_seconds(),
                            )
                            .await?;
                    } else {
                        self.runtime
                            .mark_video_timeline_export_failed(
                                export_id,
                                &owner_id,
                                epoch_seconds(),
                                stage_ordinal,
                                code,
                                &message,
                            )
                            .await?;
                    }
                }
                Ok(())
            }
        };
        if let Err(error) = self
            .runtime
            .release_video_timeline_export_lease(export_id, &owner_id)
            .await
        {
            tracing::warn!(export_id, error = %error, "failed to release timeline export lease");
        }
        handled
    }

    async fn run_owned_export(
        &self,
        export_id: &str,
        owner_id: &str,
    ) -> Result<(), ExportRunError> {
        let plan = self
            .runtime
            .video_timeline_export_execution_plan(export_id)
            .await
            .map_err(|error| ExportRunError::failed(0, "load_plan", error))?;
        if matches!(
            plan.export.state,
            VideoTimelineExportState::Completed
                | VideoTimelineExportState::Failed
                | VideoTimelineExportState::Cancelled
        ) {
            return Ok(());
        }
        self.cleanup_orphaned_artifacts(export_id, owner_id, &plan)
            .await?;
        self.check_control(export_id, owner_id).await?;
        let resolved_toolchain = self
            .resolve_toolchain()
            .await
            .map_err(|error| ExportRunError::failed(0, "resolve_toolchain", error))?;
        if plan.export.ffmpeg_identity.as_ref() != Some(&resolved_toolchain.identity) {
            return Err(ExportRunError::message(
                0,
                "toolchain_changed",
                "The configured FFmpeg and ffprobe binaries no longer match this immutable export snapshot",
            ));
        }
        let staging = self.prepare_staging(export_id, owner_id).await?;
        let toolchain = self
            .snapshot_toolchain(&staging, &resolved_toolchain)
            .await
            .map_err(|error| ExportRunError::failed(0, "snapshot_toolchain", error))?;

        if let Some(output_asset_id) = plan.export.output_asset_id.as_deref() {
            let source = self
                .runtime
                .resolve_asset_path(output_asset_id)
                .await
                .map_err(|error| ExportRunError::failed(0, "resolve_verified_output", error))?;
            self.publish_verified(
                export_id,
                owner_id,
                plan.export.stages.len().saturating_sub(1) as u32,
                &source,
                output_asset_id,
                Path::new(&plan.export.destination_path),
            )
            .await?;
            self.cleanup_staging(&staging).await;
            return Ok(());
        }

        self.begin_stage(
            export_id,
            owner_id,
            0,
            VideoTimelineExportStageKind::Validate,
            VideoTimelineExportState::Validating,
        )
        .await?;
        let validated = self
            .validate_inputs(export_id, owner_id, &plan, &toolchain.ffprobe)
            .await?;
        self.complete_stage(export_id, owner_id, 0, None).await?;

        let mut segments = Vec::with_capacity(validated.len());
        for (index, input) in validated.iter().enumerate() {
            self.check_control(export_id, owner_id).await?;
            let stage_ordinal = (index + 1) as u32;
            self.begin_stage(
                export_id,
                owner_id,
                stage_ordinal,
                VideoTimelineExportStageKind::Normalize,
                VideoTimelineExportState::Running,
            )
            .await?;
            let segment_name = format!("segment-{index:06}.mp4");
            let segment_path = staging.join(&segment_name);
            let args = normalize_args(input, &plan.export.output_profile, &segment_name);
            self.run_progress_process(
                export_id,
                owner_id,
                stage_ordinal,
                ProcessStage::Normalize {
                    index,
                    count: validated.len(),
                },
                &toolchain.ffmpeg,
                &args,
                &staging,
                input.source_duration_us,
            )
            .await?;
            let probe = self
                .probe_file(
                    export_id,
                    owner_id,
                    stage_ordinal,
                    &segment_path,
                    &toolchain.ffprobe,
                )
                .await?;
            validate_normalized_probe(
                &probe,
                &plan.export.output_profile,
                input.source_duration_us,
            )
            .map_err(|error| {
                ExportRunError::failed(stage_ordinal, "normalized_clip_mismatch", error)
            })?;
            self.complete_stage(export_id, owner_id, stage_ordinal, None)
                .await?;
            segments.push(segment_name);
        }

        let concat_stage = validated.len() as u32 + 1;
        self.begin_stage(
            export_id,
            owner_id,
            concat_stage,
            VideoTimelineExportStageKind::Concatenate,
            VideoTimelineExportState::Running,
        )
        .await?;
        let manifest = staging.join("segments.ffconcat");
        write_concat_manifest(&manifest, &segments)
            .await
            .map_err(|error| {
                ExportRunError::failed(concat_stage, "write_concat_manifest", error)
            })?;
        let combined = staging.join("combined.mp4");
        let concat_args = concat_args("segments.ffconcat", "combined.mp4");
        let total_duration_us = validated.iter().map(|input| input.source_duration_us).sum();
        self.run_progress_process(
            export_id,
            owner_id,
            concat_stage,
            ProcessStage::Concatenate,
            &toolchain.ffmpeg,
            &concat_args,
            &staging,
            total_duration_us,
        )
        .await?;
        self.complete_stage(export_id, owner_id, concat_stage, None)
            .await?;

        let verify_stage = concat_stage + 1;
        self.begin_stage(
            export_id,
            owner_id,
            verify_stage,
            VideoTimelineExportStageKind::Verify,
            VideoTimelineExportState::Verifying,
        )
        .await?;
        let probe = self
            .probe_file(
                export_id,
                owner_id,
                verify_stage,
                &combined,
                &toolchain.ffprobe,
            )
            .await?;
        validate_normalized_probe(&probe, &plan.export.output_profile, total_duration_us)
            .map_err(|error| ExportRunError::failed(verify_stage, "output_mismatch", error))?;
        let metadata = tokio::fs::metadata(&combined)
            .await
            .map_err(|error| ExportRunError::failed(verify_stage, "output_metadata", error))?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(ExportRunError::message(
                verify_stage,
                "empty_output",
                "FFmpeg exited successfully but did not create a non-empty regular output",
            ));
        }
        self.check_control(export_id, owner_id).await?;
        let asset = self
            .runtime
            .import_asset(ImportMediaAssetRequest {
                source_path: combined.clone(),
                declared_media_type: "video/mp4".to_string(),
                expected_sha256: None,
                expected_byte_length: Some(metadata.len()),
                width: Some(plan.export.output_profile.width),
                height: Some(plan.export.output_profile.height),
                duration_ms: Some(total_duration_us.div_ceil(1000)),
            })
            .await
            .map_err(|error| {
                ExportRunError::failed(verify_stage, "register_output_asset", error)
            })?;
        self.runtime
            .record_video_timeline_export_output_asset(
                export_id,
                owner_id,
                epoch_seconds(),
                &asset.id,
            )
            .await
            .map_err(|error| ExportRunError::failed(verify_stage, "link_output_asset", error))?;
        self.complete_stage(export_id, owner_id, verify_stage, Some(asset.id.clone()))
            .await?;

        let publish_stage = verify_stage + 1;
        let verified_source =
            self.runtime
                .resolve_asset_path(&asset.id)
                .await
                .map_err(|error| {
                    ExportRunError::failed(publish_stage, "resolve_verified_output", error)
                })?;
        self.publish_verified(
            export_id,
            owner_id,
            publish_stage,
            &verified_source,
            &asset.id,
            Path::new(&plan.export.destination_path),
        )
        .await?;
        self.cleanup_staging(&staging).await;
        Ok(())
    }

    async fn validate_inputs(
        &self,
        export_id: &str,
        owner_id: &str,
        plan: &VideoTimelineExportExecutionPlan,
        ffprobe: &Path,
    ) -> Result<Vec<ValidatedInput>, ExportRunError> {
        let mut validated = Vec::with_capacity(plan.inputs.len());
        for input in &plan.inputs {
            self.check_control(export_id, owner_id).await?;
            let path = self
                .runtime
                .resolve_asset_path(&input.asset_id)
                .await
                .map_err(|error| ExportRunError::failed(0, "resolve_input_asset", error))?;
            let metadata = tokio::fs::symlink_metadata(&path)
                .await
                .map_err(|error| ExportRunError::failed(0, "inspect_input_asset", error))?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(ExportRunError::message(
                    0,
                    "unsafe_input_asset",
                    "A timeline input is not a regular managed file",
                ));
            }
            let digest =
                hash_file_with_control(&self.runtime, export_id, owner_id, 0, &path).await?;
            if digest != input.asset_id {
                return Err(ExportRunError::message(
                    0,
                    "input_hash_mismatch",
                    "A managed timeline input no longer matches its content address",
                ));
            }
            let probe = self
                .probe_file(export_id, owner_id, 0, &path, ffprobe)
                .await?;
            validate_source_probe(&probe, input)
                .map_err(|error| ExportRunError::failed(0, "invalid_source_range", error))?;
            validated.push(ValidatedInput {
                path,
                source_start_us: input.source_start_us,
                source_duration_us: input.source_duration_us,
                has_audio: probe.audio.is_some(),
            });
        }
        Ok(validated)
    }

    async fn publish_verified(
        &self,
        export_id: &str,
        owner_id: &str,
        stage_ordinal: u32,
        source: &Path,
        output_asset_id: &str,
        destination: &Path,
    ) -> Result<(), ExportRunError> {
        self.begin_stage(
            export_id,
            owner_id,
            stage_ordinal,
            VideoTimelineExportStageKind::Publish,
            VideoTimelineExportState::Publishing,
        )
        .await?;
        let validated_destination = tokio::task::spawn_blocking({
            let destination = destination.to_string_lossy().into_owned();
            move || validate_retry_destination_path(&destination)
        })
        .await
        .map_err(|error| ExportRunError::failed(stage_ordinal, "validate_publish_path", error))?
        .map_err(|error| ExportRunError::failed(stage_ordinal, "validate_publish_path", error))?;
        if validated_destination != destination {
            return Err(ExportRunError::message(
                stage_ordinal,
                "destination_changed",
                "The export destination no longer resolves to its validated path",
            ));
        }
        let published_new = if destination.exists() {
            let source_length = tokio::fs::metadata(source)
                .await
                .map_err(|error| ExportRunError::failed(stage_ordinal, "source_metadata", error))?
                .len();
            let destination_length = tokio::fs::metadata(destination)
                .await
                .map_err(|error| {
                    ExportRunError::failed(stage_ordinal, "destination_metadata", error)
                })?
                .len();
            if source_length != destination_length {
                return Err(ExportRunError::message(
                    stage_ordinal,
                    "destination_collision",
                    "The chosen export destination contains different bytes",
                ));
            }
            let digest = hash_file_with_control(
                &self.runtime,
                export_id,
                owner_id,
                stage_ordinal,
                destination,
            )
            .await?;
            if digest != output_asset_id {
                return Err(ExportRunError::message(
                    stage_ordinal,
                    "destination_collision",
                    "The chosen export destination now contains different bytes",
                ));
            }
            revalidate_publish_path(destination, true, stage_ordinal).await?;
            false
        } else {
            let partial = export_owned_partial_path(destination, export_id, owner_id)
                .map_err(|error| ExportRunError::failed(stage_ordinal, "partial_path", error))?;
            if partial.exists() {
                tokio::fs::remove_file(&partial).await.map_err(|error| {
                    ExportRunError::failed(stage_ordinal, "remove_owned_partial", error)
                })?;
            }
            copy_with_control(
                &self.runtime,
                export_id,
                owner_id,
                stage_ordinal,
                source,
                &partial,
            )
            .await?;
            self.check_control(export_id, owner_id).await?;
            let partial_metadata =
                tokio::fs::symlink_metadata(&partial)
                    .await
                    .map_err(|error| {
                        ExportRunError::failed(stage_ordinal, "inspect_owned_partial", error)
                    })?;
            if !partial_metadata.is_file() || is_link_or_reparse(&partial_metadata) {
                return Err(ExportRunError::message(
                    stage_ordinal,
                    "unsafe_owned_partial",
                    "The owned publication partial is not a regular private file",
                ));
            }
            let partial_digest =
                hash_file_with_control(&self.runtime, export_id, owner_id, stage_ordinal, &partial)
                    .await?;
            if partial_digest != output_asset_id {
                return Err(ExportRunError::message(
                    stage_ordinal,
                    "partial_hash_mismatch",
                    "The owned publication partial changed before commit",
                ));
            }
            revalidate_publish_path(destination, false, stage_ordinal).await?;
            self.runtime
                .begin_video_timeline_export_publication_commit(
                    export_id,
                    owner_id,
                    epoch_seconds(),
                )
                .await
                .map_err(|error| {
                    ExportRunError::failed(stage_ordinal, "begin_publication_commit", error)
                })?;
            let partial_metadata =
                tokio::fs::symlink_metadata(&partial)
                    .await
                    .map_err(|error| {
                        ExportRunError::failed(stage_ordinal, "reinspect_owned_partial", error)
                    })?;
            if !partial_metadata.is_file() || is_link_or_reparse(&partial_metadata) {
                return Err(ExportRunError::message(
                    stage_ordinal,
                    "unsafe_owned_partial",
                    "The owned publication partial changed during commit",
                ));
            }
            let partial_digest =
                hash_file_with_control(&self.runtime, export_id, owner_id, stage_ordinal, &partial)
                    .await?;
            if partial_digest != output_asset_id {
                return Err(ExportRunError::message(
                    stage_ordinal,
                    "partial_hash_mismatch",
                    "The owned publication partial changed during commit",
                ));
            }
            revalidate_publish_path(destination, false, stage_ordinal).await?;
            atomic_publish_no_replace(&partial, destination)
                .await
                .map_err(|error| {
                    ExportRunError::failed(stage_ordinal, "publish_no_replace", error)
                })?;
            true
        };
        if !published_new {
            self.check_control(export_id, owner_id).await?;
            self.runtime
                .begin_video_timeline_export_publication_commit(
                    export_id,
                    owner_id,
                    epoch_seconds(),
                )
                .await
                .map_err(|error| {
                    ExportRunError::failed(stage_ordinal, "begin_publication_commit", error)
                })?;
            revalidate_publish_path(destination, true, stage_ordinal).await?;
            let destination_length = tokio::fs::metadata(destination)
                .await
                .map_err(|error| {
                    ExportRunError::failed(stage_ordinal, "destination_metadata", error)
                })?
                .len();
            let source_length = tokio::fs::metadata(source)
                .await
                .map_err(|error| ExportRunError::failed(stage_ordinal, "source_metadata", error))?
                .len();
            if destination_length != source_length {
                return Err(ExportRunError::message(
                    stage_ordinal,
                    "destination_collision",
                    "The chosen export destination changed during commit",
                ));
            }
            let digest = hash_file_with_control(
                &self.runtime,
                export_id,
                owner_id,
                stage_ordinal,
                destination,
            )
            .await?;
            if digest != output_asset_id {
                return Err(ExportRunError::message(
                    stage_ordinal,
                    "destination_collision",
                    "The chosen export destination changed during commit",
                ));
            }
            revalidate_publish_path(destination, true, stage_ordinal).await?;
        }
        let completion = async {
            self.complete_stage(export_id, owner_id, stage_ordinal, None)
                .await?;
            self.runtime
                .mark_video_timeline_export_completed(
                    export_id,
                    owner_id,
                    epoch_seconds(),
                    output_asset_id,
                )
                .await
                .map_err(|error| ExportRunError::failed(stage_ordinal, "complete_export", error))?;
            Ok::<(), ExportRunError>(())
        }
        .await;
        completion?;
        Ok(())
    }

    async fn begin_stage(
        &self,
        export_id: &str,
        owner_id: &str,
        ordinal: u32,
        kind: VideoTimelineExportStageKind,
        state: VideoTimelineExportState,
    ) -> Result<(), ExportRunError> {
        self.runtime
            .begin_video_timeline_export_stage(
                export_id,
                owner_id,
                epoch_seconds(),
                ordinal,
                kind,
                state,
            )
            .await
            .map_err(|error| ExportRunError::failed(ordinal, "begin_stage", error))
    }

    async fn complete_stage(
        &self,
        export_id: &str,
        owner_id: &str,
        ordinal: u32,
        asset_id: Option<String>,
    ) -> Result<(), ExportRunError> {
        self.runtime
            .complete_video_timeline_export_stage(
                export_id,
                owner_id,
                epoch_seconds(),
                ordinal,
                asset_id,
            )
            .await
            .map_err(|error| ExportRunError::failed(ordinal, "complete_stage", error))
    }

    async fn check_control(&self, export_id: &str, owner_id: &str) -> Result<(), ExportRunError> {
        if self
            .runtime
            .video_timeline_export_cancel_requested(export_id)
            .await
            .map_err(|error| ExportRunError::failed(0, "read_cancel_intent", error))?
        {
            return Err(ExportRunError::Cancelled);
        }
        if !self
            .runtime
            .renew_video_timeline_export_lease(export_id, owner_id, epoch_seconds())
            .await
            .map_err(|error| ExportRunError::failed(0, "renew_lease", error))?
        {
            return Err(ExportRunError::LeaseLost);
        }
        Ok(())
    }

    async fn resolve_toolchain(&self) -> Result<ResolvedToolchain, CoreError> {
        let configured = self.ffmpeg_program.clone();
        let ffmpeg = tokio::task::spawn_blocking(move || resolve_executable(&configured))
            .await
            .map_err(|error| CoreError::Internal(format!("FFmpeg resolution failed: {error}")))??;
        let ffprobe_candidate = derive_ffprobe_path(&ffmpeg.to_string_lossy());
        let ffprobe = tokio::task::spawn_blocking(move || resolve_executable(&ffprobe_candidate))
            .await
            .map_err(|error| {
                CoreError::Internal(format!("ffprobe resolution failed: {error}"))
            })??;
        let ffmpeg_hash = hash_tool_binary(&ffmpeg).await?;
        let ffprobe_hash = hash_tool_binary(&ffprobe).await?;
        let ffmpeg_version = run_identity_command(&ffmpeg).await?;
        let ffprobe_version = run_identity_command(&ffprobe).await?;
        let identity = json!({
            "schemaVersion": 1,
            "ffmpeg": tool_identity_value(&ffmpeg_version, &ffmpeg_hash),
            "ffprobe": tool_identity_value(&ffprobe_version, &ffprobe_hash),
        });
        Ok(ResolvedToolchain {
            ffmpeg,
            ffprobe,
            identity,
        })
    }

    async fn snapshot_toolchain(
        &self,
        staging: &Path,
        resolved: &ResolvedToolchain,
    ) -> Result<ResolvedToolchain, CoreError> {
        let ffmpeg = snapshot_tool_binary(&resolved.ffmpeg, staging, "nexa-ffmpeg").await?;
        let ffprobe = snapshot_tool_binary(&resolved.ffprobe, staging, "nexa-ffprobe").await?;
        let ffmpeg_hash = hash_tool_binary(&ffmpeg).await?;
        let ffprobe_hash = hash_tool_binary(&ffprobe).await?;
        let ffmpeg_version = run_identity_command(&ffmpeg).await?;
        let ffprobe_version = run_identity_command(&ffprobe).await?;
        let identity = json!({
            "schemaVersion": 1,
            "ffmpeg": tool_identity_value(&ffmpeg_version, &ffmpeg_hash),
            "ffprobe": tool_identity_value(&ffprobe_version, &ffprobe_hash),
        });
        if identity != resolved.identity {
            return Err(CoreError::Conflict(
                "FFmpeg tools changed while their immutable execution snapshot was created"
                    .to_string(),
            ));
        }
        Ok(ResolvedToolchain {
            ffmpeg,
            ffprobe,
            identity,
        })
    }

    async fn probe_file(
        &self,
        export_id: &str,
        owner_id: &str,
        stage_ordinal: u32,
        path: &Path,
        ffprobe: &Path,
    ) -> Result<MediaProbe, ExportRunError> {
        let args = vec![
            OsString::from("-v"),
            OsString::from("error"),
            OsString::from("-print_format"),
            OsString::from("json"),
            OsString::from("-show_format"),
            OsString::from("-show_streams"),
            path.as_os_str().to_owned(),
        ];
        let output = self
            .run_checked_output(
                export_id,
                owner_id,
                stage_ordinal,
                ffprobe,
                &args,
                Path::new("."),
            )
            .await?;
        parse_probe(&output)
            .map_err(|error| ExportRunError::failed(stage_ordinal, "probe_output", error))
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_progress_process(
        &self,
        export_id: &str,
        owner_id: &str,
        stage_ordinal: u32,
        stage: ProcessStage,
        program: &Path,
        args: &[OsString],
        working_directory: &Path,
        expected_duration_us: u64,
    ) -> Result<(), ExportRunError> {
        reject_link_or_reparse(program)
            .map_err(|error| ExportRunError::failed(stage_ordinal, "unsafe_media_tool", error))?;
        reject_link_or_reparse(working_directory).map_err(|error| {
            ExportRunError::failed(stage_ordinal, "unsafe_working_directory", error)
        })?;
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        crate::background_process::configure_tokio_background(&mut command);
        let mut child = command
            .spawn()
            .map_err(|error| ExportRunError::failed(stage_ordinal, "spawn_ffmpeg", error))?;
        let mut child_stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or_else(|| {
            ExportRunError::message(
                stage_ordinal,
                "missing_progress_pipe",
                "FFmpeg progress pipe was unavailable",
            )
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            ExportRunError::message(
                stage_ordinal,
                "missing_diagnostic_pipe",
                "FFmpeg diagnostic pipe was unavailable",
            )
        })?;
        let mut lines = FramedRead::new(stdout, LinesCodec::new_with_max_length(4096));
        let stderr_task = tokio::spawn(read_bounded(stderr));
        let started = tokio::time::Instant::now();
        let mut control = tokio::time::interval(PROCESS_CONTROL_TICK);
        control.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_lease_renewed = tokio::time::Instant::now();
        let mut last_progress = 0_u32;
        let mut progress_open = true;
        let status = loop {
            tokio::select! {
                line = lines.next(), if progress_open => {
                    match line {
                        Some(Ok(line)) => {
                            if let Some(out_time_us) = parse_progress_time_us(&line) {
                                let stage_progress = progress_basis_points(out_time_us, expected_duration_us);
                                if stage_progress >= last_progress.saturating_add(50) {
                                    last_progress = stage_progress;
                                    let overall = overall_progress(stage, stage_progress);
                                    self.runtime.record_video_timeline_export_progress(
                                        export_id,
                                        owner_id,
                                        epoch_seconds(),
                                        stage_ordinal,
                                        stage_progress,
                                        overall,
                                    ).await.map_err(|error| ExportRunError::failed(stage_ordinal, "record_progress", error))?;
                                }
                            }
                        }
                        Some(Err(error)) => {
                            terminate_and_reap(&mut child, child_stdin.take()).await;
                            return Err(ExportRunError::failed(stage_ordinal, "invalid_progress", error));
                        }
                        None => {
                            progress_open = false;
                        }
                    }
                }
                _ = control.tick() => {
                    if started.elapsed() > PROCESS_TIMEOUT {
                        terminate_and_reap(&mut child, child_stdin.take()).await;
                        return Err(ExportRunError::message(stage_ordinal, "ffmpeg_timeout", "FFmpeg exceeded the six-hour export limit"));
                    }
                    if self.runtime.video_timeline_export_cancel_requested(export_id).await
                        .map_err(|error| ExportRunError::failed(stage_ordinal, "read_cancel_intent", error))? {
                        terminate_and_reap(&mut child, child_stdin.take()).await;
                        return Err(ExportRunError::Cancelled);
                    }
                    if last_lease_renewed.elapsed() >= LEASE_RENEW_INTERVAL {
                        let renewed = self.runtime.renew_video_timeline_export_lease(
                            export_id,
                            owner_id,
                            epoch_seconds(),
                        ).await.map_err(|error| ExportRunError::failed(stage_ordinal, "renew_lease", error))?;
                        if !renewed {
                            terminate_and_reap(&mut child, child_stdin.take()).await;
                            return Err(ExportRunError::LeaseLost);
                        }
                        last_lease_renewed = tokio::time::Instant::now();
                    }
                    if let Some(status) = child.try_wait().map_err(|error| ExportRunError::failed(stage_ordinal, "poll_ffmpeg", error))? {
                        break status;
                    }
                }
            }
        };
        let stderr = stderr_task.await.unwrap_or_default();
        if !status.success() {
            return Err(ExportRunError::message(
                stage_ordinal,
                "ffmpeg_failed",
                &format!(
                    "FFmpeg exited with code {:?}: {}",
                    status.code(),
                    redact_diagnostic(&stderr)
                ),
            ));
        }
        self.runtime
            .record_video_timeline_export_progress(
                export_id,
                owner_id,
                epoch_seconds(),
                stage_ordinal,
                10_000,
                overall_progress(stage, 10_000),
            )
            .await
            .map_err(|error| ExportRunError::failed(stage_ordinal, "record_progress", error))?;
        Ok(())
    }

    async fn run_checked_output(
        &self,
        export_id: &str,
        owner_id: &str,
        stage_ordinal: u32,
        program: &Path,
        args: &[OsString],
        working_directory: &Path,
    ) -> Result<Vec<u8>, ExportRunError> {
        reject_link_or_reparse(program)
            .map_err(|error| ExportRunError::failed(stage_ordinal, "unsafe_media_tool", error))?;
        reject_link_or_reparse(working_directory).map_err(|error| {
            ExportRunError::failed(stage_ordinal, "unsafe_working_directory", error)
        })?;
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        crate::background_process::configure_tokio_background(&mut command);
        let mut child = command
            .spawn()
            .map_err(|error| ExportRunError::failed(stage_ordinal, "spawn_media_probe", error))?;
        let mut child_stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or_else(|| {
            ExportRunError::message(
                stage_ordinal,
                "missing_stdout",
                "Media process stdout was unavailable",
            )
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            ExportRunError::message(
                stage_ordinal,
                "missing_stderr",
                "Media process stderr was unavailable",
            )
        })?;
        let stdout_task = tokio::spawn(read_bounded(stdout));
        let stderr_task = tokio::spawn(read_bounded(stderr));
        let started = tokio::time::Instant::now();
        let mut control = tokio::time::interval(PROCESS_CONTROL_TICK);
        control.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_lease_renewed = tokio::time::Instant::now();
        let status = loop {
            control.tick().await;
            if self
                .runtime
                .video_timeline_export_cancel_requested(export_id)
                .await
                .map_err(|error| {
                    ExportRunError::failed(stage_ordinal, "read_cancel_intent", error)
                })?
            {
                terminate_and_reap(&mut child, child_stdin.take()).await;
                return Err(ExportRunError::Cancelled);
            }
            if started.elapsed() > Duration::from_secs(30) {
                terminate_and_reap(&mut child, child_stdin.take()).await;
                return Err(ExportRunError::message(
                    stage_ordinal,
                    "probe_timeout",
                    "Media probe exceeded 30 seconds",
                ));
            }
            if last_lease_renewed.elapsed() >= LEASE_RENEW_INTERVAL {
                if !self
                    .runtime
                    .renew_video_timeline_export_lease(export_id, owner_id, epoch_seconds())
                    .await
                    .map_err(|error| ExportRunError::failed(stage_ordinal, "renew_lease", error))?
                {
                    terminate_and_reap(&mut child, child_stdin.take()).await;
                    return Err(ExportRunError::LeaseLost);
                }
                last_lease_renewed = tokio::time::Instant::now();
            }
            if let Some(status) = child
                .try_wait()
                .map_err(|error| ExportRunError::failed(stage_ordinal, "poll_media_probe", error))?
            {
                break status;
            }
        };
        let stdout = stdout_task.await.unwrap_or_default();
        let stderr = stderr_task.await.unwrap_or_default();
        if !status.success() {
            return Err(ExportRunError::message(
                stage_ordinal,
                "media_probe_failed",
                &format!("Media probe failed: {}", redact_diagnostic(&stderr)),
            ));
        }
        Ok(stdout)
    }

    async fn cleanup_staging(&self, staging: &Path) {
        let root = self.staging_root.clone();
        let staging = staging.to_path_buf();
        let staging_for_cleanup = staging.clone();
        let cleanup =
            tokio::task::spawn_blocking(move || safe_remove_staging(&root, &staging_for_cleanup))
                .await;
        match cleanup {
            Ok(Ok(())) => {}
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(Err(error)) => {
                tracing::warn!(path = %staging.display(), error = %error, "failed to clean timeline export staging");
            }
            Err(error) => {
                tracing::warn!(path = %staging.display(), error = %error, "timeline export staging cleanup task failed");
            }
        }
        if let Some(parent) = staging.parent() {
            let _ = tokio::fs::remove_dir(parent).await;
        }
    }

    async fn cleanup_owned_artifacts(&self, export_id: &str, owner_id: &str) {
        if let Ok(staging) = self.staging_directory(export_id, owner_id) {
            self.cleanup_staging(&staging).await;
        }
        if let Ok(plan) = self
            .runtime
            .video_timeline_export_execution_plan(export_id)
            .await
        {
            if let Ok(partial) = export_owned_partial_path(
                Path::new(&plan.export.destination_path),
                export_id,
                owner_id,
            ) {
                match tokio::fs::remove_file(&partial).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        tracing::warn!(path = %partial.display(), error = %error, "failed to clean timeline export owned partial")
                    }
                }
            }
        }
    }

    async fn cleanup_orphaned_artifacts(
        &self,
        export_id: &str,
        owner_id: &str,
        plan: &VideoTimelineExportExecutionPlan,
    ) -> Result<(), ExportRunError> {
        let current = self.staging_directory(export_id, owner_id)?;
        let export_directory = current.parent().ok_or_else(|| {
            ExportRunError::message(0, "invalid_staging", "Export staging has no parent")
        })?;
        let root = self.staging_root.clone();
        let export_directory = export_directory.to_path_buf();
        let current = current.to_path_buf();
        tokio::task::spawn_blocking(move || {
            cleanup_orphaned_staging_paths(&root, &export_directory, &current)
        })
        .await
        .map_err(|error| ExportRunError::failed(0, "cleanup_orphaned_staging", error))?
        .map_err(|error| ExportRunError::failed(0, "cleanup_orphaned_staging", error))?;

        let destination = Path::new(&plan.export.destination_path);
        let parent = destination.parent().ok_or_else(|| {
            ExportRunError::message(0, "invalid_destination", "Export destination has no parent")
        })?;
        reject_link_or_reparse(parent)
            .map_err(|error| ExportRunError::failed(0, "unsafe_destination_parent", error))?;
        let prefix = format!(".nexa-{export_id}-");
        let mut entries = tokio::fs::read_dir(parent)
            .await
            .map_err(|error| ExportRunError::failed(0, "scan_orphaned_partials", error))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| ExportRunError::failed(0, "scan_orphaned_partials", error))?
        {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some(owner_hash) = name
                .strip_prefix(&prefix)
                .and_then(|value| value.strip_suffix(".partial.mp4"))
            else {
                continue;
            };
            if owner_hash.len() != 16 || !owner_hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                continue;
            }
            let metadata = tokio::fs::symlink_metadata(entry.path())
                .await
                .map_err(|error| ExportRunError::failed(0, "inspect_orphaned_partial", error))?;
            if metadata.is_dir() && !is_link_or_reparse(&metadata) {
                continue;
            }
            tokio::fs::remove_file(entry.path())
                .await
                .map_err(|error| ExportRunError::failed(0, "remove_orphaned_partial", error))?;
        }
        Ok(())
    }

    async fn prepare_staging(
        &self,
        export_id: &str,
        owner_id: &str,
    ) -> Result<PathBuf, ExportRunError> {
        let staging = self.staging_directory(export_id, owner_id)?;
        tokio::fs::create_dir_all(&self.staging_root)
            .await
            .map_err(|error| ExportRunError::failed(0, "create_staging_root", error))?;
        reject_link_or_reparse(&self.staging_root)
            .map_err(|error| ExportRunError::failed(0, "unsafe_staging_root", error))?;
        let export_directory = staging.parent().ok_or_else(|| {
            ExportRunError::message(0, "invalid_staging", "Export staging has no parent")
        })?;
        match tokio::fs::create_dir(export_directory).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(ExportRunError::failed(0, "create_export_staging", error)),
        }
        reject_link_or_reparse(export_directory)
            .map_err(|error| ExportRunError::failed(0, "unsafe_export_staging", error))?;
        tokio::fs::create_dir(&staging)
            .await
            .map_err(|error| ExportRunError::failed(0, "create_owner_staging", error))?;
        reject_link_or_reparse(&staging)
            .map_err(|error| ExportRunError::failed(0, "unsafe_owner_staging", error))?;
        let canonical_root = tokio::fs::canonicalize(&self.staging_root)
            .await
            .map_err(|error| ExportRunError::failed(0, "canonicalize_staging_root", error))?;
        let canonical_staging = tokio::fs::canonicalize(&staging)
            .await
            .map_err(|error| ExportRunError::failed(0, "canonicalize_staging", error))?;
        if !canonical_staging.starts_with(&canonical_root) {
            return Err(ExportRunError::message(
                0,
                "staging_escape",
                "Timeline export staging escaped its private root",
            ));
        }
        Ok(canonical_staging)
    }

    fn staging_directory(
        &self,
        export_id: &str,
        owner_id: &str,
    ) -> Result<PathBuf, ExportRunError> {
        let parsed = uuid::Uuid::parse_str(export_id)
            .map_err(|error| ExportRunError::failed(0, "invalid_export_id", error))?;
        if parsed.to_string() != export_id {
            return Err(ExportRunError::message(
                0,
                "invalid_export_id",
                "Timeline export ID was not in canonical UUID form",
            ));
        }
        let owner_token = owner_id.rsplit(':').next().unwrap_or_default();
        let owner = uuid::Uuid::parse_str(owner_token)
            .map_err(|error| ExportRunError::failed(0, "invalid_lease_owner", error))?;
        if owner.to_string() != owner_token {
            return Err(ExportRunError::message(
                0,
                "invalid_lease_owner",
                "Timeline export lease owner was not in canonical UUID form",
            ));
        }
        Ok(self
            .staging_root
            .join(parsed.to_string())
            .join(owner.to_string()))
    }
}

#[derive(Debug)]
enum ExportRunError {
    Cancelled,
    LeaseLost,
    Failed {
        stage_ordinal: u32,
        code: &'static str,
        message: String,
    },
}

impl ExportRunError {
    fn failed(stage_ordinal: u32, code: &'static str, error: impl std::fmt::Display) -> Self {
        Self::message(stage_ordinal, code, &error.to_string())
    }

    fn message(stage_ordinal: u32, code: &'static str, message: &str) -> Self {
        Self::Failed {
            stage_ordinal,
            code,
            message: message.chars().take(4096).collect(),
        }
    }
}

#[derive(Debug)]
struct ResolvedToolchain {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    identity: Value,
}

impl std::fmt::Display for ExportRunError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("export cancelled"),
            Self::LeaseLost => formatter.write_str("export lease lost"),
            Self::Failed { code, message, .. } => write!(formatter, "{code}: {message}"),
        }
    }
}

#[derive(Debug)]
struct ValidatedInput {
    path: PathBuf,
    source_start_us: u64,
    source_duration_us: u64,
    has_audio: bool,
}

#[derive(Debug, Clone, Copy)]
enum ProcessStage {
    Normalize { index: usize, count: usize },
    Concatenate,
}

#[derive(Debug)]
struct MediaProbe {
    duration_us: u64,
    format_name: String,
    stream_count: usize,
    video: VideoProbe,
    audio: Option<AudioProbe>,
}

#[derive(Debug)]
struct VideoProbe {
    codec: String,
    width: u32,
    height: u32,
    pixel_format: Option<String>,
    frame_rate: Option<(u32, u32)>,
    profile: Option<String>,
    level: Option<u32>,
    time_base: Option<(u32, u32)>,
    color_primaries: Option<String>,
    color_transfer: Option<String>,
    color_space: Option<String>,
    color_range: Option<String>,
    duration_us: Option<u64>,
}

#[derive(Debug)]
struct AudioProbe {
    codec: String,
    sample_rate: Option<u32>,
    channels: Option<u32>,
    channel_layout: Option<String>,
    time_base: Option<(u32, u32)>,
    duration_us: Option<u64>,
}

fn normalize_args(
    input: &ValidatedInput,
    profile: &VideoTimelineOutputProfile,
    output: &str,
) -> Vec<OsString> {
    let start = format_seconds(input.source_start_us);
    let duration = format_seconds(input.source_duration_us);
    let fps = format!("{}/{}", profile.fps_numerator, profile.fps_denominator);
    let video_filter = format!(
        "[0:v:0]trim=start={start}:duration={duration},setpts=PTS-STARTPTS,scale={}:{}:force_original_aspect_ratio=decrease:out_color_matrix=bt709:out_range=tv,pad={}:{}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={fps},format={},setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709[v]",
        profile.width, profile.height, profile.width, profile.height, profile.pixel_format
    );
    let audio_filter = if input.has_audio {
        format!(
            "[0:a:0]atrim=start={start}:duration={duration},asetpts=PTS-STARTPTS,aresample={},aformat=sample_fmts=fltp:channel_layouts={},apad=whole_dur={duration},atrim=duration={duration}[a]",
            profile.audio_sample_rate, profile.audio_channel_layout
        )
    } else {
        format!(
            "anullsrc=r={}:cl={},atrim=duration={duration},asetpts=PTS-STARTPTS[a]",
            profile.audio_sample_rate, profile.audio_channel_layout
        )
    };
    vec![
        "-n".into(),
        "-loglevel".into(),
        "error".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-stats_period".into(),
        "0.25".into(),
        "-i".into(),
        input.path.as_os_str().to_owned(),
        "-t".into(),
        duration.into(),
        "-filter_complex".into(),
        format!("{video_filter};{audio_filter}").into(),
        "-map".into(),
        "[v]".into(),
        "-map".into(),
        "[a]".into(),
        "-c:v".into(),
        "libx264".into(),
        "-profile:v".into(),
        profile.video_profile.clone().into(),
        "-level:v".into(),
        "5.2".into(),
        "-preset".into(),
        profile.video_preset.clone().into(),
        "-crf".into(),
        profile.video_crf.to_string().into(),
        "-fps_mode".into(),
        "cfr".into(),
        "-color_primaries".into(),
        profile.color_primaries.clone().into(),
        "-color_trc".into(),
        profile.color_transfer.clone().into(),
        "-colorspace".into(),
        profile.color_space.clone().into(),
        "-color_range".into(),
        "tv".into(),
        "-video_track_timescale".into(),
        profile.video_time_base_denominator.to_string().into(),
        "-c:a".into(),
        "aac".into(),
        "-ar".into(),
        profile.audio_sample_rate.to_string().into(),
        "-ac".into(),
        "2".into(),
        "-movflags".into(),
        "+faststart".into(),
        output.into(),
    ]
}

fn concat_args(manifest: &str, output: &str) -> Vec<OsString> {
    [
        "-n",
        "-loglevel",
        "error",
        "-progress",
        "pipe:1",
        "-stats_period",
        "0.25",
        "-f",
        "concat",
        "-safe",
        "1",
        "-i",
        manifest,
        "-c",
        "copy",
        "-movflags",
        "+faststart",
        output,
    ]
    .into_iter()
    .map(OsString::from)
    .collect()
}

async fn write_concat_manifest(path: &Path, segment_names: &[String]) -> Result<(), CoreError> {
    if segment_names.is_empty()
        || segment_names.iter().any(|name| {
            !name.starts_with("segment-")
                || !name.ends_with(".mp4")
                || name
                    .chars()
                    .any(|ch| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '.'))
        })
    {
        return Err(CoreError::InvalidInput(
            "Concat manifest accepts generated portable segment names only".to_string(),
        ));
    }
    let mut body = String::from("ffconcat version 1.0\n");
    for name in segment_names {
        body.push_str("file '");
        body.push_str(name);
        body.push_str("'\n");
    }
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await?;
    file.write_all(body.as_bytes()).await?;
    file.sync_all().await?;
    Ok(())
}

fn parse_probe(bytes: &[u8]) -> Result<MediaProbe, CoreError> {
    let value: Value = serde_json::from_slice(bytes)?;
    let streams = value
        .get("streams")
        .and_then(Value::as_array)
        .ok_or_else(|| CoreError::Video("ffprobe did not return a streams array".to_string()))?;
    let video = streams
        .iter()
        .find(|stream| stream.get("codec_type").and_then(Value::as_str) == Some("video"))
        .ok_or_else(|| CoreError::Video("Media has no video stream".to_string()))?;
    let audio = streams
        .iter()
        .find(|stream| stream.get("codec_type").and_then(Value::as_str) == Some("audio"));
    let duration_seconds = value
        .pointer("/format/duration")
        .and_then(Value::as_str)
        .or_else(|| video.get("duration").and_then(Value::as_str))
        .ok_or_else(|| CoreError::Video("Media duration is unavailable".to_string()))?;
    let duration_us = parse_seconds_to_us(duration_seconds)?;
    let format_name = value
        .pointer("/format/format_name")
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Video("Media container format is unavailable".to_string()))?
        .to_string();
    Ok(MediaProbe {
        duration_us,
        format_name,
        stream_count: streams.len(),
        video: VideoProbe {
            codec: string_field(video, "codec_name")?,
            width: u32_field(video, "width")?,
            height: u32_field(video, "height")?,
            pixel_format: video
                .get("pix_fmt")
                .and_then(Value::as_str)
                .map(str::to_string),
            frame_rate: video
                .get("avg_frame_rate")
                .and_then(Value::as_str)
                .and_then(parse_ratio),
            profile: video
                .get("profile")
                .and_then(Value::as_str)
                .map(str::to_string),
            level: video
                .get("level")
                .and_then(Value::as_u64)
                .and_then(|value| value.try_into().ok()),
            time_base: video
                .get("time_base")
                .and_then(Value::as_str)
                .and_then(parse_ratio),
            color_primaries: optional_string_field(video, "color_primaries"),
            color_transfer: optional_string_field(video, "color_transfer"),
            color_space: optional_string_field(video, "color_space"),
            color_range: optional_string_field(video, "color_range"),
            duration_us: optional_duration_us(video, "duration")?,
        },
        audio: audio
            .map(|audio| {
                Ok::<AudioProbe, CoreError>(AudioProbe {
                    codec: string_field(audio, "codec_name")?,
                    sample_rate: audio
                        .get("sample_rate")
                        .and_then(Value::as_str)
                        .and_then(|value| value.parse().ok()),
                    channels: audio
                        .get("channels")
                        .and_then(Value::as_u64)
                        .and_then(|value| value.try_into().ok()),
                    channel_layout: optional_string_field(audio, "channel_layout"),
                    time_base: audio
                        .get("time_base")
                        .and_then(Value::as_str)
                        .and_then(parse_ratio),
                    duration_us: optional_duration_us(audio, "duration")?,
                })
            })
            .transpose()?,
    })
}

fn validate_source_probe(
    probe: &MediaProbe,
    input: &VideoTimelineExportInputPlan,
) -> Result<(), CoreError> {
    let end = input
        .source_start_us
        .checked_add(input.source_duration_us)
        .ok_or_else(|| CoreError::InvalidInput("Source range overflowed".to_string()))?;
    let video_duration = probe.video.duration_us.unwrap_or(probe.duration_us);
    if input.source_duration_us == 0 || end > video_duration.saturating_add(100_000) {
        return Err(CoreError::InvalidInput(
            "Timeline source range exceeds the probe-verified duration".to_string(),
        ));
    }
    Ok(())
}

fn validate_normalized_probe(
    probe: &MediaProbe,
    profile: &VideoTimelineOutputProfile,
    expected_duration_us: u64,
) -> Result<(), CoreError> {
    if probe.video.codec != "h264"
        || !probe.format_name.split(',').any(|name| name == "mp4")
        || probe.stream_count != 2
        || probe.video.width != profile.width
        || probe.video.height != profile.height
        || probe.video.pixel_format.as_deref() != Some(profile.pixel_format.as_str())
        || probe.video.frame_rate != Some((profile.fps_numerator, profile.fps_denominator))
        || !probe
            .video
            .profile
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case(&profile.video_profile))
        || probe.video.level != Some(u32::from(profile.video_level))
        || probe.video.time_base
            != Some((
                profile.video_time_base_numerator,
                profile.video_time_base_denominator,
            ))
        || probe.video.color_primaries.as_deref() != Some(profile.color_primaries.as_str())
        || probe.video.color_transfer.as_deref() != Some(profile.color_transfer.as_str())
        || probe.video.color_space.as_deref() != Some(profile.color_space.as_str())
        || probe.video.color_range.as_deref() != Some(profile.color_range.as_str())
    {
        return Err(CoreError::Video(
            "Normalized video stream does not match the frozen export profile".to_string(),
        ));
    }
    let audio = probe
        .audio
        .as_ref()
        .ok_or_else(|| CoreError::Video("Normalized output has no audio stream".to_string()))?;
    if audio.codec != "aac"
        || audio.sample_rate != Some(profile.audio_sample_rate)
        || audio.channels != Some(2)
        || audio.channel_layout.as_deref() != Some(profile.audio_channel_layout.as_str())
        || audio.time_base != Some((1, profile.audio_sample_rate))
    {
        return Err(CoreError::Video(
            "Normalized audio stream does not match the frozen export profile".to_string(),
        ));
    }
    let frame_tolerance = 1_000_000_u64.saturating_mul(profile.fps_denominator.into())
        / u64::from(profile.fps_numerator);
    let tolerance = frame_tolerance.saturating_add(100_000);
    let video_duration = probe.video.duration_us.ok_or_else(|| {
        CoreError::Video("Normalized video stream duration is unavailable".to_string())
    })?;
    let audio_duration = audio.duration_us.ok_or_else(|| {
        CoreError::Video("Normalized audio stream duration is unavailable".to_string())
    })?;
    if probe.duration_us.abs_diff(expected_duration_us) > tolerance
        || video_duration.abs_diff(expected_duration_us) > tolerance
        || audio_duration.abs_diff(expected_duration_us) > tolerance
        || video_duration.abs_diff(audio_duration) > tolerance
    {
        return Err(CoreError::Video(
            "Normalized output duration is outside the declared frame tolerance".to_string(),
        ));
    }
    Ok(())
}

fn parse_progress_time_us(line: &str) -> Option<u64> {
    let (key, value) = line.split_once('=')?;
    match key {
        "out_time_us" => value.parse().ok(),
        "out_time_ms" => value.parse().ok(),
        _ => None,
    }
}

fn progress_basis_points(current: u64, total: u64) -> u32 {
    if total == 0 {
        return 0;
    }
    current
        .min(total)
        .saturating_mul(10_000)
        .checked_div(total)
        .unwrap_or(0)
        .try_into()
        .unwrap_or(10_000)
}

fn overall_progress(stage: ProcessStage, stage_progress: u32) -> u32 {
    match stage {
        ProcessStage::Normalize { index, count } => {
            let count = count.max(1) as u64;
            let completed = 500_u64 + (index as u64 * 7_500 / count);
            let share = u64::from(stage_progress) * 7_500 / count / 10_000;
            (completed + share).min(7_999) as u32
        }
        ProcessStage::Concatenate => 8_000 + stage_progress.min(10_000) / 10,
    }
}

fn format_seconds(microseconds: u64) -> String {
    format!(
        "{}.{:06}",
        microseconds / 1_000_000,
        microseconds % 1_000_000
    )
}

fn parse_seconds_to_us(value: &str) -> Result<u64, CoreError> {
    let seconds: f64 = value
        .parse()
        .map_err(|_| CoreError::Video("Media duration is malformed".to_string()))?;
    if !seconds.is_finite() || seconds <= 0.0 || seconds > 21_600.0 {
        return Err(CoreError::Video(
            "Media duration is outside the six-hour probe bound".to_string(),
        ));
    }
    Ok((seconds * 1_000_000.0).round() as u64)
}

fn parse_ratio(value: &str) -> Option<(u32, u32)> {
    let (numerator, denominator) = value.split_once('/')?;
    let numerator = numerator.parse::<u32>().ok()?;
    let denominator = denominator.parse::<u32>().ok()?;
    (denominator != 0).then_some((numerator, denominator))
}

fn string_field(value: &Value, field: &str) -> Result<String, CoreError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| CoreError::Video(format!("ffprobe omitted {field}")))
}

fn optional_string_field(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_string)
}

fn optional_duration_us(value: &Value, field: &str) -> Result<Option<u64>, CoreError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(parse_seconds_to_us)
        .transpose()
}

fn u32_field(value: &Value, field: &str) -> Result<u32, CoreError> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|value| value.try_into().ok())
        .ok_or_else(|| CoreError::Video(format!("ffprobe omitted {field}")))
}

async fn read_bounded<R: tokio::io::AsyncRead + Unpin>(mut reader: R) -> Vec<u8> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if output.len() + read > MAX_CAPTURE_BYTES {
                    let remove = (output.len() + read) - MAX_CAPTURE_BYTES;
                    output.drain(..remove.min(output.len()));
                }
                output.extend_from_slice(&buffer[..read]);
            }
        }
    }
    output
}

fn resolve_executable(program: &str) -> Result<PathBuf, CoreError> {
    let configured = PathBuf::from(program);
    let mut candidates = Vec::new();
    if configured.is_absolute() || configured.components().count() > 1 {
        candidates.push(configured);
    } else {
        let path = std::env::var_os("PATH").ok_or_else(|| {
            CoreError::NotFound(format!(
                "Executable {program} was not found because PATH is empty"
            ))
        })?;
        for directory in std::env::split_paths(&path) {
            candidates.push(directory.join(&configured));
            #[cfg(windows)]
            if configured.extension().is_none() {
                candidates.push(directory.join(format!("{program}.exe")));
            }
        }
    }
    for candidate in candidates {
        if candidate.is_file() {
            let canonical = std::fs::canonicalize(&candidate)?;
            if canonical.is_file() {
                return Ok(canonical);
            }
        }
    }
    Err(CoreError::NotFound(format!(
        "Configured media executable {program} was not found"
    )))
}

async fn hash_tool_binary(path: &Path) -> Result<String, CoreError> {
    let metadata = tokio::fs::symlink_metadata(path).await?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_TOOL_BINARY_BYTES {
        return Err(CoreError::InvalidInput(
            "FFmpeg tools must be regular files no larger than 512 MiB".to_string(),
        ));
    }
    hash_file(path).await
}

async fn snapshot_tool_binary(
    source: &Path,
    staging: &Path,
    portable_name: &str,
) -> Result<PathBuf, CoreError> {
    reject_link_or_reparse(source)?;
    reject_link_or_reparse(staging)?;
    let extension = source.extension().and_then(|value| value.to_str());
    let file_name = extension
        .map(|extension| format!("{portable_name}.{extension}"))
        .unwrap_or_else(|| portable_name.to_string());
    let destination = staging.join(file_name);
    let mut input = tokio::fs::File::open(source).await?;
    let metadata = input.metadata().await?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_TOOL_BINARY_BYTES {
        return Err(CoreError::InvalidInput(
            "FFmpeg tools must be regular files no larger than 512 MiB".to_string(),
        ));
    }
    let mut output = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .await?;
    tokio::io::copy(&mut input, &mut output).await?;
    output.sync_all().await?;
    drop(output);
    tokio::fs::set_permissions(&destination, metadata.permissions()).await?;
    reject_link_or_reparse(&destination)?;
    Ok(destination)
}

async fn run_identity_command(path: &Path) -> Result<Vec<u8>, CoreError> {
    let mut command = Command::new(path);
    command
        .arg("-version")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::background_process::configure_tokio_background(&mut command);
    let mut child = command.spawn()?;
    let child_stdin = child.stdin.take();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| CoreError::Internal("Media identity stdout was unavailable".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| CoreError::Internal("Media identity stderr was unavailable".to_string()))?;
    let stdout_task = tokio::spawn(read_bounded(stdout));
    let stderr_task = tokio::spawn(read_bounded(stderr));
    let status = match tokio::time::timeout(TOOL_IDENTITY_TIMEOUT, child.wait()).await {
        Ok(status) => status?,
        Err(_) => {
            terminate_and_reap(&mut child, child_stdin).await;
            return Err(CoreError::Video(
                "Media tool identity check exceeded 10 seconds".to_string(),
            ));
        }
    };
    let stdout = stdout_task.await.unwrap_or_default();
    let stderr = stderr_task.await.unwrap_or_default();
    if !status.success() {
        return Err(CoreError::Video(format!(
            "Media tool identity check failed: {}",
            redact_diagnostic(&stderr)
        )));
    }
    Ok(stdout)
}

fn tool_identity_value(output: &[u8], binary_sha256: &str) -> Value {
    let text = String::from_utf8_lossy(output);
    let version = text.lines().next().unwrap_or("unknown");
    let configuration = text
        .lines()
        .find(|line| line.starts_with("configuration:"))
        .unwrap_or("configuration: unknown");
    json!({
        "binarySha256": binary_sha256,
        "versionOutputSha256": format!("{:x}", Sha256::digest(output)),
        "version": version.chars().take(500).collect::<String>(),
        "configuration": configuration.chars().take(4000).collect::<String>(),
    })
}

async fn terminate_and_reap(child: &mut Child, mut stdin: Option<ChildStdin>) {
    if let Some(mut stdin) = stdin.take() {
        let _ = stdin.write_all(b"q\n").await;
        let _ = stdin.shutdown().await;
    }
    if matches!(
        tokio::time::timeout(CHILD_GRACE_PERIOD, child.wait()).await,
        Ok(Ok(_))
    ) {
        return;
    }
    let _ = child.start_kill();
    let _ = tokio::time::timeout(CHILD_REAP_TIMEOUT, child.wait()).await;
}

fn reject_link_or_reparse(path: &Path) -> Result<(), CoreError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(CoreError::InvalidInput(format!(
            "Path {} cannot be a symlink",
            path.display()
        )));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(CoreError::InvalidInput(format!(
                "Path {} cannot be a reparse point",
                path.display()
            )));
        }
    }
    Ok(())
}

async fn revalidate_publish_path(
    destination: &Path,
    expect_existing: bool,
    stage_ordinal: u32,
) -> Result<(), ExportRunError> {
    let validated = tokio::task::spawn_blocking({
        let destination = destination.to_string_lossy().into_owned();
        move || validate_retry_destination_path(&destination)
    })
    .await
    .map_err(|error| ExportRunError::failed(stage_ordinal, "revalidate_publish_path", error))?
    .map_err(|error| ExportRunError::failed(stage_ordinal, "revalidate_publish_path", error))?;
    if validated != destination {
        return Err(ExportRunError::message(
            stage_ordinal,
            "destination_changed",
            "The export destination parent changed before publication commit",
        ));
    }
    let metadata = tokio::fs::symlink_metadata(destination).await;
    match (expect_existing, metadata) {
        (false, Err(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        (false, Ok(_)) => Err(ExportRunError::message(
            stage_ordinal,
            "destination_collision",
            "The export destination appeared before publication commit",
        )),
        (true, Ok(metadata)) if metadata.is_file() && !is_link_or_reparse(&metadata) => Ok(()),
        (true, Ok(_)) => Err(ExportRunError::message(
            stage_ordinal,
            "unsafe_destination",
            "The existing export destination is no longer a regular file",
        )),
        (_, Err(error)) => Err(ExportRunError::failed(
            stage_ordinal,
            "inspect_publish_path",
            error,
        )),
    }
}

async fn atomic_publish_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    let source = source.to_path_buf();
    let destination = destination.to_path_buf();
    tokio::task::spawn_blocking(move || atomic_rename_no_replace(&source, &destination))
        .await
        .map_err(|error| std::io::Error::other(format!("publication task failed: {error}")))?
}

#[cfg(windows)]
fn atomic_rename_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::iter::once;
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(once(0))
        .collect::<Vec<_>>();
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn atomic_rename_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source = CString::new(source.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "source contains NUL")
    })?;
    let destination = CString::new(destination.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "destination contains NUL")
    })?;
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(target_vendor = "apple")]
fn atomic_rename_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source = CString::new(source.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "source contains NUL")
    })?;
    let destination = CString::new(destination.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "destination contains NUL")
    })?;
    let result =
        unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
    if result == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(all(unix, not(target_os = "linux"), not(target_vendor = "apple")))]
fn atomic_rename_no_replace(_source: &Path, _destination: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "this platform has no configured atomic no-replace rename primitive",
    ))
}

fn safe_remove_staging(root: &Path, staging: &Path) -> Result<(), std::io::Error> {
    let root_metadata = std::fs::symlink_metadata(root)?;
    if is_link_or_reparse(&root_metadata) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing cleanup through a linked staging root",
        ));
    }
    let metadata = std::fs::symlink_metadata(staging)?;
    if is_link_or_reparse(&metadata) {
        return if metadata.is_dir() {
            std::fs::remove_dir(staging)
        } else {
            std::fs::remove_file(staging)
        };
    }
    let canonical_root = std::fs::canonicalize(root)?;
    let canonical_staging = std::fs::canonicalize(staging)?;
    if !canonical_staging.starts_with(&canonical_root) || canonical_staging == canonical_root {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing to recursively remove staging outside its private root",
        ));
    }
    std::fs::remove_dir_all(canonical_staging)
}

fn cleanup_orphaned_staging_paths(
    root: &Path,
    export_directory: &Path,
    current: &Path,
) -> Result<(), std::io::Error> {
    let metadata = match std::fs::symlink_metadata(export_directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if is_link_or_reparse(&metadata) {
        safe_remove_staging(root, export_directory)?;
        return Ok(());
    }
    for entry in std::fs::read_dir(export_directory)? {
        let entry = entry?;
        if entry.path() != current {
            safe_remove_staging(root, &entry.path())?;
        }
    }
    Ok(())
}

fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        return metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    }
    #[cfg(not(windows))]
    false
}

async fn hash_file(path: &Path) -> Result<String, CoreError> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

async fn hash_file_with_control(
    runtime: &MediaGenerationRuntime,
    export_id: &str,
    owner_id: &str,
    stage_ordinal: u32,
    path: &Path,
) -> Result<String, ExportRunError> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| ExportRunError::failed(stage_ordinal, "open_hash_input", error))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 256 * 1024];
    let mut last_control = tokio::time::Instant::now();
    let mut last_lease = tokio::time::Instant::now();
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| ExportRunError::failed(stage_ordinal, "read_hash_input", error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        if last_control.elapsed() >= PROCESS_CONTROL_TICK {
            if runtime
                .video_timeline_export_cancel_requested(export_id)
                .await
                .map_err(|error| {
                    ExportRunError::failed(stage_ordinal, "read_cancel_intent", error)
                })?
            {
                return Err(ExportRunError::Cancelled);
            }
            last_control = tokio::time::Instant::now();
        }
        if last_lease.elapsed() >= LEASE_RENEW_INTERVAL {
            if !runtime
                .renew_video_timeline_export_lease(export_id, owner_id, epoch_seconds())
                .await
                .map_err(|error| ExportRunError::failed(stage_ordinal, "renew_lease", error))?
            {
                return Err(ExportRunError::LeaseLost);
            }
            last_lease = tokio::time::Instant::now();
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

async fn copy_with_control(
    runtime: &MediaGenerationRuntime,
    export_id: &str,
    owner_id: &str,
    stage_ordinal: u32,
    source: &Path,
    owned_partial: &Path,
) -> Result<(), ExportRunError> {
    let mut input = tokio::fs::File::open(source)
        .await
        .map_err(|error| ExportRunError::failed(stage_ordinal, "open_verified_output", error))?;
    let total = input
        .metadata()
        .await
        .map_err(|error| ExportRunError::failed(stage_ordinal, "read_output_metadata", error))?
        .len();
    let mut output = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(owned_partial)
        .await
        .map_err(|error| ExportRunError::failed(stage_ordinal, "create_owned_partial", error))?;
    let mut copied = 0_u64;
    let mut buffer = vec![0_u8; 256 * 1024];
    let mut last_control = tokio::time::Instant::now();
    let mut last_lease = tokio::time::Instant::now();
    let mut last_progress = 0_u32;
    while copied < total {
        if last_control.elapsed() >= PROCESS_CONTROL_TICK {
            if runtime
                .video_timeline_export_cancel_requested(export_id)
                .await
                .map_err(|error| {
                    ExportRunError::failed(stage_ordinal, "read_cancel_intent", error)
                })?
            {
                drop(output);
                let _ = tokio::fs::remove_file(owned_partial).await;
                return Err(ExportRunError::Cancelled);
            }
            last_control = tokio::time::Instant::now();
        }
        if last_lease.elapsed() >= LEASE_RENEW_INTERVAL {
            if !runtime
                .renew_video_timeline_export_lease(export_id, owner_id, epoch_seconds())
                .await
                .map_err(|error| ExportRunError::failed(stage_ordinal, "renew_lease", error))?
            {
                drop(output);
                let _ = tokio::fs::remove_file(owned_partial).await;
                return Err(ExportRunError::LeaseLost);
            }
            last_lease = tokio::time::Instant::now();
        }
        let read = input.read(&mut buffer).await.map_err(|error| {
            ExportRunError::failed(stage_ordinal, "read_verified_output", error)
        })?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .await
            .map_err(|error| ExportRunError::failed(stage_ordinal, "write_owned_partial", error))?;
        copied = copied.saturating_add(read as u64);
        let progress = progress_basis_points(copied, total);
        if progress >= last_progress.saturating_add(50) || copied == total {
            last_progress = progress;
            runtime
                .record_video_timeline_export_progress(
                    export_id,
                    owner_id,
                    epoch_seconds(),
                    stage_ordinal,
                    progress,
                    9_500 + progress / 20,
                )
                .await
                .map_err(|error| ExportRunError::failed(stage_ordinal, "record_progress", error))?;
        }
    }
    output
        .flush()
        .await
        .map_err(|error| ExportRunError::failed(stage_ordinal, "flush_owned_partial", error))?;
    output
        .sync_all()
        .await
        .map_err(|error| ExportRunError::failed(stage_ordinal, "sync_owned_partial", error))?;
    Ok(())
}

fn redact_diagnostic(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.split_whitespace()
        .map(|token| {
            if token.starts_with('/') || token.contains(":\\") || token.contains(":/") {
                "[path]"
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(2000)
        .collect()
}

fn epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn derive_ffprobe_path(ffmpeg_program: &str) -> String {
    let path = Path::new(ffmpeg_program);
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("ffmpeg");
    let probe_stem = if stem.eq_ignore_ascii_case("ffmpeg") {
        "ffprobe".to_string()
    } else {
        stem.replacen("ffmpeg", "ffprobe", 1)
    };
    let file_name = if extension.is_empty() {
        probe_stem
    } else {
        format!("{probe_stem}.{extension}")
    };
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.join(&file_name).to_string_lossy().into_owned())
        .unwrap_or(file_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_parser_is_bounded_and_monotonic_friendly() {
        assert_eq!(
            parse_progress_time_us("out_time_us=1500000"),
            Some(1_500_000)
        );
        assert_eq!(parse_progress_time_us("progress=end"), None);
        assert_eq!(parse_progress_time_us("out_time_us=-1"), None);
        assert_eq!(progress_basis_points(5, 10), 5_000);
        assert_eq!(progress_basis_points(20, 10), 10_000);
    }

    #[test]
    fn concat_manifest_rejects_non_generated_names() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let temp = tempfile::tempdir().unwrap();
        runtime.block_on(async {
            assert!(write_concat_manifest(
                &temp.path().join("list.ffconcat"),
                &["../secret.mp4".to_string()]
            )
            .await
            .is_err());
            write_concat_manifest(
                &temp.path().join("list.ffconcat"),
                &["segment-000000.mp4".to_string()],
            )
            .await
            .unwrap();
            let body = tokio::fs::read_to_string(temp.path().join("list.ffconcat"))
                .await
                .unwrap();
            assert_eq!(body, "ffconcat version 1.0\nfile 'segment-000000.mp4'\n");
            assert!(write_concat_manifest(
                &temp.path().join("list.ffconcat"),
                &["segment-000000.mp4".to_string()],
            )
            .await
            .is_err());
        });
    }

    #[test]
    fn normalize_args_keep_input_as_one_argv_value() {
        let input = ValidatedInput {
            path: PathBuf::from("C:\\odd path\\quote' & clip.mp4"),
            source_start_us: 1_250_000,
            source_duration_us: 2_500_000,
            has_audio: true,
        };
        let profile = VideoTimelineOutputProfile {
            schema_version: 1,
            width: 1280,
            height: 720,
            fit: "contain".to_string(),
            fps_numerator: 30_000,
            fps_denominator: 1001,
            pixel_format: "yuv420p".to_string(),
            video_codec: "h264".to_string(),
            video_profile: "high".to_string(),
            video_level: 52,
            video_time_base_numerator: 1,
            video_time_base_denominator: 90_000,
            color_primaries: "bt709".to_string(),
            color_transfer: "bt709".to_string(),
            color_space: "bt709".to_string(),
            color_range: "tv".to_string(),
            video_preset: "medium".to_string(),
            video_crf: 20,
            audio_codec: "aac".to_string(),
            audio_sample_rate: 48_000,
            audio_channel_layout: "stereo".to_string(),
        };
        let args = normalize_args(&input, &profile, "segment-000000.mp4");
        assert!(args.iter().any(|value| value == input.path.as_os_str()));
        assert!(args.iter().any(|value| value == "-n"));
        assert!(!args.iter().any(|value| value == "-y"));
        let filter = args
            .iter()
            .find(|value| value.to_string_lossy().contains("setparams="))
            .unwrap()
            .to_string_lossy();
        assert!(filter.contains("apad=whole_dur=2.500000"));
        assert!(filter.contains("color_primaries=bt709"));
        assert!(!args
            .iter()
            .any(|value| value.to_string_lossy().contains("shell")));
    }

    #[test]
    fn probe_verification_requires_exact_profile_and_bounded_duration() {
        let mut probe = MediaProbe {
            duration_us: 2_000_000,
            format_name: "mov,mp4,m4a,3gp,3g2,mj2".to_string(),
            stream_count: 2,
            video: VideoProbe {
                codec: "h264".to_string(),
                width: 1280,
                height: 720,
                pixel_format: Some("yuv420p".to_string()),
                frame_rate: Some((30, 1)),
                profile: Some("High".to_string()),
                level: Some(52),
                time_base: Some((1, 90_000)),
                color_primaries: Some("bt709".to_string()),
                color_transfer: Some("bt709".to_string()),
                color_space: Some("bt709".to_string()),
                color_range: Some("tv".to_string()),
                duration_us: Some(2_000_000),
            },
            audio: Some(AudioProbe {
                codec: "aac".to_string(),
                sample_rate: Some(48_000),
                channels: Some(2),
                channel_layout: Some("stereo".to_string()),
                time_base: Some((1, 48_000)),
                duration_us: Some(2_000_000),
            }),
        };
        let profile = VideoTimelineOutputProfile {
            schema_version: 1,
            width: 1280,
            height: 720,
            fit: "contain".to_string(),
            fps_numerator: 30,
            fps_denominator: 1,
            pixel_format: "yuv420p".to_string(),
            video_codec: "h264".to_string(),
            video_profile: "high".to_string(),
            video_level: 52,
            video_time_base_numerator: 1,
            video_time_base_denominator: 90_000,
            color_primaries: "bt709".to_string(),
            color_transfer: "bt709".to_string(),
            color_space: "bt709".to_string(),
            color_range: "tv".to_string(),
            video_preset: "medium".to_string(),
            video_crf: 20,
            audio_codec: "aac".to_string(),
            audio_sample_rate: 48_000,
            audio_channel_layout: "stereo".to_string(),
        };
        validate_normalized_probe(&probe, &profile, 2_000_000).unwrap();
        assert!(validate_normalized_probe(&probe, &profile, 3_000_000).is_err());
        probe.video.color_space = Some("bt2020nc".to_string());
        assert!(validate_normalized_probe(&probe, &profile, 2_000_000).is_err());
    }

    #[test]
    fn tool_snapshot_is_content_identical_and_create_new() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("ffmpeg-test.bin");
        std::fs::write(&source, b"immutable media tool").unwrap();
        let staging = temp.path().join("staging");
        std::fs::create_dir(&staging).unwrap();
        runtime.block_on(async {
            let snapshot = snapshot_tool_binary(&source, &staging, "nexa-ffmpeg")
                .await
                .unwrap();
            assert_eq!(
                hash_file(&source).await.unwrap(),
                hash_file(&snapshot).await.unwrap()
            );
            assert!(snapshot_tool_binary(&source, &staging, "nexa-ffmpeg")
                .await
                .is_err());
        });
    }

    #[test]
    fn atomic_publication_never_replaces_an_existing_destination() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let temp = tempfile::tempdir().unwrap();
        runtime.block_on(async {
            let first_partial = temp.path().join("first.partial");
            let destination = temp.path().join("output.mp4");
            std::fs::write(&first_partial, b"first").unwrap();
            atomic_publish_no_replace(&first_partial, &destination)
                .await
                .unwrap();
            assert!(!first_partial.exists());
            assert_eq!(std::fs::read(&destination).unwrap(), b"first");

            let second_partial = temp.path().join("second.partial");
            std::fs::write(&second_partial, b"second").unwrap();
            assert!(atomic_publish_no_replace(&second_partial, &destination)
                .await
                .is_err());
            assert!(second_partial.exists());
            assert_eq!(std::fs::read(&destination).unwrap(), b"first");
        });
    }

    #[test]
    fn staging_cleanup_is_contained_and_never_removes_the_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let owned = root.join("export").join("owner");
        std::fs::create_dir_all(&owned).unwrap();
        std::fs::write(owned.join("segment.mp4"), b"segment").unwrap();
        safe_remove_staging(&root, &owned).unwrap();
        assert!(!owned.exists());
        assert!(root.exists());
        assert_eq!(
            safe_remove_staging(&root, &root).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn recovery_cleanup_removes_only_orphaned_owner_staging() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let export = root.join("export");
        let current = export.join("current");
        let orphan = export.join("orphan");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(&orphan).unwrap();
        std::fs::write(orphan.join("combined.mp4"), b"orphan").unwrap();
        cleanup_orphaned_staging_paths(&root, &export, &current).unwrap();
        assert!(current.exists());
        assert!(!orphan.exists());
    }

    #[cfg(unix)]
    #[test]
    fn staging_cleanup_unlinks_a_symlink_without_following_it() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let victim = temp.path().join("victim");
        let link = root.join("export").join("owner");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::fs::create_dir(&victim).unwrap();
        std::fs::write(victim.join("keep.txt"), b"keep").unwrap();
        symlink(&victim, &link).unwrap();
        safe_remove_staging(&root, &link).unwrap();
        assert!(victim.join("keep.txt").exists());
        assert!(!link.exists());
    }

    #[cfg(unix)]
    #[test]
    fn staging_cleanup_refuses_a_replaced_root_symlink() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let victim = temp.path().join("victim");
        let victim_owner = victim.join("export").join("owner");
        std::fs::create_dir_all(&victim_owner).unwrap();
        std::fs::write(victim_owner.join("keep.txt"), b"keep").unwrap();
        symlink(&victim, &root).unwrap();
        assert_eq!(
            safe_remove_staging(&root, &root.join("export").join("owner"))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(victim_owner.join("keep.txt").exists());
    }
}
