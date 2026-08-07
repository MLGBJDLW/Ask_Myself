use std::sync::Arc;

use crate::db_executor::DatabaseExecutor;
use crate::error::CoreError;

#[cfg(test)]
use super::model::RegisterMediaAssetRequest;
use super::model::{
    BeginMediaJobAttemptRequest, CreateMediaJobRequest, DeleteMediaAssetOccurrenceRequest,
    LinkMediaAssetRequest, MediaAssetRecord, MediaJobSnapshot, MediaProviderEventRecord,
    MediaRecoveryPlanItem, RecordMediaJobRemoteDeletionResult, RecordMediaProviderEventRequest,
    RequestMediaAssetDeletion, RequestMediaJobCancellation, RequestMediaJobRemoteDeletion,
    TransitionMediaJobRequest,
};
use super::store;
use super::{ImportMediaAssetRequest, MediaGenerationAssetStore};

/// Provider-neutral durable boundary for asynchronous media generation.
///
/// Each method runs SQLite work on the bounded database lanes. Callers never
/// receive a database handle, so transition, idempotency, and lineage
/// invariants cannot be bypassed by provider adapters or the desktop host.
#[derive(Clone)]
pub struct MediaGenerationRuntime {
    database: DatabaseExecutor,
    asset_store: Option<Arc<MediaGenerationAssetStore>>,
    asset_mutation_lock: Arc<tokio::sync::Mutex<()>>,
}

impl MediaGenerationRuntime {
    pub fn new(database: DatabaseExecutor) -> Self {
        Self {
            database,
            asset_store: None,
            asset_mutation_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub fn with_asset_store(
        database: DatabaseExecutor,
        asset_store: MediaGenerationAssetStore,
    ) -> Self {
        Self {
            database,
            asset_store: Some(Arc::new(asset_store)),
            asset_mutation_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub async fn create_job(
        &self,
        request: CreateMediaJobRequest,
    ) -> Result<MediaJobSnapshot, CoreError> {
        Ok(self
            .database
            .write(move |database| store::create_job(database, request))
            .await?
            .value)
    }

    pub async fn get_job(&self, job_id: &str) -> Result<MediaJobSnapshot, CoreError> {
        let job_id = job_id.to_string();
        Ok(self
            .database
            .read(move |database| store::get_job(database, &job_id))
            .await?
            .value)
    }

    /// Returns every non-terminal job that an adapter must reconcile after
    /// process restart. Draft/local validation work is intentionally excluded.
    pub async fn list_recoverable_jobs(&self) -> Result<Vec<MediaJobSnapshot>, CoreError> {
        Ok(self
            .database
            .read(store::list_recoverable_jobs)
            .await?
            .value)
    }

    pub async fn list_provider_events(
        &self,
        job_id: &str,
        after_sequence: u64,
        limit: u32,
    ) -> Result<Vec<MediaProviderEventRecord>, CoreError> {
        let job_id = job_id.to_string();
        Ok(self
            .database
            .read(move |database| {
                store::list_provider_events(database, &job_id, after_sequence, limit)
            })
            .await?
            .value)
    }

    /// Converts ambiguous interrupted submissions to `provider_unknown`.
    /// This never creates an attempt or resubmits provider work.
    pub async fn recover_after_restart(&self) -> Result<usize, CoreError> {
        let recovered = self
            .database
            .write(store::recover_after_restart)
            .await?
            .value;
        if let Some(asset_store) = self.asset_store.clone() {
            let _asset_guard = self.asset_mutation_lock.lock().await;
            let pending = self
                .database
                .read(store::list_pending_asset_deletions)
                .await?
                .value;
            for asset in pending {
                let asset_id = asset.id;
                let storage_key = asset.storage_key.clone();
                let asset_store = asset_store.clone();
                match tokio::task::spawn_blocking(move || {
                    asset_store.delete_storage_key(&storage_key)
                })
                .await
                {
                    Ok(Ok(())) => {
                        self.database
                            .write(move |database| {
                                store::confirm_asset_deleted(database, &asset_id)
                            })
                            .await?;
                    }
                    Ok(Err(error)) => {
                        tracing::warn!(
                            asset_id,
                            error = %error,
                            "media asset deletion recovery remains pending"
                        );
                    }
                    Err(error) => {
                        tracing::warn!(
                            asset_id,
                            error = %error,
                            "media asset deletion recovery task failed"
                        );
                    }
                }
            }
            let registered = self
                .database
                .read(store::list_registered_asset_storage_keys)
                .await?
                .value;
            match tokio::task::spawn_blocking(move || asset_store.reconcile_untracked(&registered))
                .await
            {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    tracing::warn!(error = %error, "media asset reconciliation will retry later");
                }
                Err(error) => {
                    tracing::warn!(error = %error, "media asset reconciliation task failed");
                }
            }
        }
        Ok(recovered)
    }

    pub async fn build_recovery_plan(&self) -> Result<Vec<MediaRecoveryPlanItem>, CoreError> {
        Ok(self.database.read(store::build_recovery_plan).await?.value)
    }

    pub async fn transition_job(
        &self,
        request: TransitionMediaJobRequest,
    ) -> Result<MediaJobSnapshot, CoreError> {
        Ok(self
            .database
            .write(move |database| store::transition_job(database, request))
            .await?
            .value)
    }

    /// Persists cancellation intent without claiming the provider cancelled.
    /// A later provider observation performs the terminal state transition.
    pub async fn request_cancellation(
        &self,
        request: RequestMediaJobCancellation,
    ) -> Result<MediaJobSnapshot, CoreError> {
        Ok(self
            .database
            .write(move |database| store::request_cancellation(database, request))
            .await?
            .value)
    }

    pub async fn request_remote_deletion(
        &self,
        request: RequestMediaJobRemoteDeletion,
    ) -> Result<MediaJobSnapshot, CoreError> {
        Ok(self
            .database
            .write(move |database| store::request_remote_deletion(database, request))
            .await?
            .value)
    }

    pub async fn record_remote_deletion_result(
        &self,
        request: RecordMediaJobRemoteDeletionResult,
    ) -> Result<MediaJobSnapshot, CoreError> {
        Ok(self
            .database
            .write(move |database| store::record_remote_deletion_result(database, request))
            .await?
            .value)
    }

    pub async fn begin_attempt(
        &self,
        request: BeginMediaJobAttemptRequest,
    ) -> Result<MediaJobSnapshot, CoreError> {
        Ok(self
            .database
            .write(move |database| store::begin_attempt(database, request))
            .await?
            .value)
    }

    /// Atomically deduplicates a provider observation and applies its attempt
    /// and job projections. Duplicate `(source, id)` events are idempotent.
    pub async fn record_provider_event(
        &self,
        request: RecordMediaProviderEventRequest,
    ) -> Result<MediaJobSnapshot, CoreError> {
        Ok(self
            .database
            .write(move |database| store::record_provider_event(database, request))
            .await?
            .value)
    }

    /// Registers only assets whose bytes and SHA-256 were verified by Nexa.
    /// Unverified remote locators remain provider event data.
    pub async fn import_asset(
        &self,
        request: ImportMediaAssetRequest,
    ) -> Result<MediaAssetRecord, CoreError> {
        let _asset_guard = self.asset_mutation_lock.lock().await;
        let asset_store = self.asset_store.clone().ok_or_else(|| {
            CoreError::Internal("Media generation asset store is not configured".to_string())
        })?;
        let verified = tokio::task::spawn_blocking(move || asset_store.import_verified(request))
            .await
            .map_err(|error| {
                CoreError::Internal(format!("Media asset import task failed: {error}"))
            })??;
        let registration = verified.registration.clone();
        let result = self
            .database
            .write(move |database| store::register_asset(database, registration))
            .await;
        match result {
            Ok(outcome) => Ok(outcome.value),
            Err(error) => {
                let rollback_store = self.asset_store.clone().ok_or_else(|| {
                    CoreError::Internal(
                        "Media generation asset store disappeared during rollback".to_string(),
                    )
                })?;
                tokio::task::spawn_blocking(move || rollback_store.rollback_import(&verified))
                    .await
                    .map_err(|join_error| {
                        CoreError::Internal(format!(
                            "Media asset rollback task failed after {error}: {join_error}"
                        ))
                    })?;
                Err(error)
            }
        }
    }

    pub async fn delete_asset(
        &self,
        request: RequestMediaAssetDeletion,
    ) -> Result<MediaAssetRecord, CoreError> {
        let _asset_guard = self.asset_mutation_lock.lock().await;
        let asset = self
            .database
            .write(move |database| store::prepare_asset_deletion(database, request))
            .await?
            .value;
        if asset.local_state == super::model::MediaAssetLocalState::Deleted {
            return Ok(asset);
        }
        let asset_store = self.asset_store.clone().ok_or_else(|| {
            CoreError::Internal("Media generation asset store is not configured".to_string())
        })?;
        let storage_key = asset.storage_key;
        tokio::task::spawn_blocking(move || asset_store.delete_storage_key(&storage_key))
            .await
            .map_err(|error| {
                CoreError::Internal(format!("Media asset deletion task failed: {error}"))
            })??;
        let asset_id = asset.id;
        Ok(self
            .database
            .write(move |database| store::confirm_asset_deleted(database, &asset_id))
            .await?
            .value)
    }

    #[cfg(test)]
    async fn register_asset(
        &self,
        request: RegisterMediaAssetRequest,
    ) -> Result<MediaAssetRecord, CoreError> {
        Ok(self
            .database
            .write(move |database| store::register_asset(database, request))
            .await?
            .value)
    }

    pub async fn link_asset(
        &self,
        request: LinkMediaAssetRequest,
    ) -> Result<MediaJobSnapshot, CoreError> {
        Ok(self
            .database
            .write(move |database| store::link_asset(database, request))
            .await?
            .value)
    }

    pub async fn delete_asset_occurrence(
        &self,
        request: DeleteMediaAssetOccurrenceRequest,
    ) -> Result<MediaJobSnapshot, CoreError> {
        Ok(self
            .database
            .write(move |database| store::delete_asset_occurrence(database, request))
            .await?
            .value)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::db::Database;
    use crate::media_generation::MediaAssetLocalRetentionPolicy;
    use crate::media_generation::{
        MediaAssetRelationType, MediaAssetStorageKind, MediaJobAttemptState, MediaJobState,
        MediaObservationMode, MediaOperation,
    };

    fn runtime(database: Database) -> MediaGenerationRuntime {
        MediaGenerationRuntime::new(DatabaseExecutor::new(database, 16).unwrap())
    }

    fn create_request(key: &str) -> CreateMediaJobRequest {
        CreateMediaJobRequest {
            idempotency_key: key.to_string(),
            project_id: Some("project-video".to_string()),
            conversation_id: Some("conversation-video".to_string()),
            provider_id: "provider-a".to_string(),
            provider_source: "urn:nexa:provider-a:endpoint-1:account-hash-a:us".to_string(),
            model_id: "video-model-1".to_string(),
            api_version: Some("2026-08-01".to_string()),
            operation: MediaOperation::TextToVideo,
            input_asset_ids: Vec::new(),
            raw_parameters: json!({ "prompt": "ocean at dawn" }),
            normalized_parameters: json!({ "prompt": "ocean at dawn", "durationSeconds": 5 }),
            provider_extras: json!({}),
            observation_mode: MediaObservationMode::Hybrid,
            estimated_cost_micros: Some(125_000),
            currency: Some("USD".to_string()),
            data_region: Some("us".to_string()),
            remote_retention_expires_at: Some("2026-08-14T00:00:00Z".to_string()),
            allow_cross_provider_fallback: false,
            max_attempts: 3,
        }
    }

    fn transition(
        snapshot: &MediaJobSnapshot,
        next_state: MediaJobState,
    ) -> TransitionMediaJobRequest {
        TransitionMediaJobRequest {
            job_id: snapshot.job.id.clone(),
            expected_revision: snapshot.job.revision,
            next_state,
        }
    }

    async fn submitting_job(runtime: &MediaGenerationRuntime, key: &str) -> MediaJobSnapshot {
        let draft = runtime.create_job(create_request(key)).await.unwrap();
        let validating = runtime
            .transition_job(transition(&draft, MediaJobState::Validating))
            .await
            .unwrap();
        let uploading = runtime
            .transition_job(transition(&validating, MediaJobState::UploadingAssets))
            .await
            .unwrap();
        runtime
            .transition_job(transition(&uploading, MediaJobState::Submitting))
            .await
            .unwrap()
    }

    async fn begin_attempt(
        runtime: &MediaGenerationRuntime,
        snapshot: &MediaJobSnapshot,
        key: &str,
    ) -> MediaJobSnapshot {
        runtime
            .begin_attempt(BeginMediaJobAttemptRequest {
                job_id: snapshot.job.id.clone(),
                expected_revision: snapshot.job.revision,
                idempotency_key: key.to_string(),
                provider_id: "provider-a".to_string(),
                provider_source: "urn:nexa:provider-a:endpoint-1:account-hash-a:us".to_string(),
                model_id: "video-model-1".to_string(),
                api_version: Some("2026-08-01".to_string()),
                data_region: Some("us".to_string()),
                remote_retention_expires_at: Some("2026-08-14T00:00:00Z".to_string()),
                provider_unknown_reconciliation: None,
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn create_is_idempotent_and_rejects_key_reuse_for_different_input() {
        let runtime = runtime(Database::open_memory().unwrap());
        let first = runtime
            .create_job(create_request("create-idempotent"))
            .await
            .unwrap();
        let replay = runtime
            .create_job(create_request("create-idempotent"))
            .await
            .unwrap();
        assert_eq!(replay, first);

        let mut changed = create_request("create-idempotent");
        changed.normalized_parameters = json!({ "prompt": "different" });
        let error = runtime.create_job(changed).await.unwrap_err();
        assert!(matches!(error, CoreError::Conflict(_)));
    }

    #[tokio::test]
    async fn cancellation_request_is_not_terminal_until_confirmed() {
        let runtime = runtime(Database::open_memory().unwrap());
        let draft = runtime
            .create_job(create_request("cancel-semantics"))
            .await
            .unwrap();
        let validating = runtime
            .transition_job(transition(&draft, MediaJobState::Validating))
            .await
            .unwrap();
        let requested = runtime
            .request_cancellation(RequestMediaJobCancellation {
                job_id: validating.job.id.clone(),
                expected_revision: validating.job.revision,
                reason: "user_requested".to_string(),
            })
            .await
            .unwrap();
        assert_eq!(requested.job.state, MediaJobState::Validating);
        assert!(requested.job.cancellation_requested_at.is_some());
        assert_eq!(
            requested.job.cancellation_reason.as_deref(),
            Some("user_requested")
        );

        let confirmed = runtime
            .transition_job(transition(&requested, MediaJobState::Cancelled))
            .await
            .unwrap();
        assert_eq!(confirmed.job.state, MediaJobState::Cancelled);
        assert!(confirmed.job.completed_at.is_some());
    }

    #[tokio::test]
    async fn restart_marks_ambiguous_submission_unknown_without_resubmitting() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("media-runtime.db");
        let first_runtime = runtime(Database::new(&path).unwrap());
        let submitting = submitting_job(&first_runtime, "restart-ambiguous").await;
        let started = begin_attempt(&first_runtime, &submitting, "attempt-1").await;
        assert_eq!(started.attempts.len(), 1);
        drop(first_runtime);

        let restarted_runtime = runtime(Database::new(&path).unwrap());
        assert_eq!(restarted_runtime.recover_after_restart().await.unwrap(), 1);
        let recoverable = restarted_runtime.list_recoverable_jobs().await.unwrap();
        assert_eq!(recoverable.len(), 1);
        assert_eq!(recoverable[0].job.state, MediaJobState::ProviderUnknown);
        assert_eq!(
            recoverable[0].attempts[0].state,
            MediaJobAttemptState::ProviderUnknown
        );
        assert_eq!(recoverable[0].attempts.len(), 1);

        let error = restarted_runtime
            .begin_attempt(BeginMediaJobAttemptRequest {
                job_id: recoverable[0].job.id.clone(),
                expected_revision: recoverable[0].job.revision,
                idempotency_key: "attempt-2".to_string(),
                provider_id: "provider-a".to_string(),
                provider_source: "urn:nexa:provider-a:endpoint-1:account-hash-a:us".to_string(),
                model_id: "video-model-1".to_string(),
                api_version: Some("2026-08-01".to_string()),
                data_region: Some("us".to_string()),
                remote_retention_expires_at: Some("2026-08-14T00:00:00Z".to_string()),
                provider_unknown_reconciliation: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(error, CoreError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn provider_events_dedupe_by_source_and_id() {
        let runtime = runtime(Database::open_memory().unwrap());
        let submitting = submitting_job(&runtime, "event-dedupe").await;
        let started = begin_attempt(&runtime, &submitting, "event-attempt").await;
        let attempt_id = started.attempts[0].id.clone();
        let accepted = RecordMediaProviderEventRequest {
            job_id: started.job.id.clone(),
            expected_revision: started.job.revision,
            attempt_id: attempt_id.clone(),
            provider_id: "provider-a".to_string(),
            event_source: "urn:nexa:provider-a:endpoint-1:account-hash-a:us".to_string(),
            deduplication_key: "event-1".to_string(),
            event_kind: "job.accepted".to_string(),
            payload: json!({ "status": "queued" }),
            provider_created_at: Some("2026-08-07T02:00:00Z".to_string()),
            provider_task_id: Some("provider-task-1".to_string()),
            attempt_state: Some(MediaJobAttemptState::Accepted),
            next_job_state: Some(MediaJobState::Queued),
            error: None,
            retry_classification: None,
            next_eligible_at: None,
            cancellation_result: None,
            final_cost_micros: None,
            watermark_present: None,
            provenance: None,
        };
        let first = runtime
            .record_provider_event(accepted.clone())
            .await
            .unwrap();
        let replay = runtime.record_provider_event(accepted).await.unwrap();
        assert_eq!(replay.job.revision, first.job.revision);
        assert_eq!(replay.provider_events.len(), 1);

        let second_source = runtime
            .record_provider_event(RecordMediaProviderEventRequest {
                job_id: first.job.id.clone(),
                expected_revision: first.job.revision,
                attempt_id,
                provider_id: "provider-a".to_string(),
                event_source: "urn:nexa:provider-a:endpoint-1:account-hash-b:us".to_string(),
                deduplication_key: "event-1".to_string(),
                event_kind: "job.observed".to_string(),
                payload: json!({ "status": "queued" }),
                provider_created_at: Some("2026-08-07T02:00:01Z".to_string()),
                provider_task_id: Some("provider-task-1".to_string()),
                attempt_state: Some(MediaJobAttemptState::Observing),
                next_job_state: None,
                error: None,
                retry_classification: None,
                next_eligible_at: None,
                cancellation_result: None,
                final_cost_micros: None,
                watermark_present: None,
                provenance: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(second_source, CoreError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn content_dedupe_preserves_attempt_level_lineage_occurrences() {
        let runtime = runtime(Database::open_memory().unwrap());
        let submitting = submitting_job(&runtime, "asset-lineage").await;
        let started = begin_attempt(&runtime, &submitting, "asset-attempt").await;
        let attempt_id = started.attempts[0].id.clone();
        let asset_hash = "ab".repeat(32);
        let asset = runtime
            .register_asset(RegisterMediaAssetRequest {
                content_hash_sha256: asset_hash.clone(),
                content_verified_at: "2026-08-07T02:10:00Z".to_string(),
                media_type: "video/mp4".to_string(),
                byte_length: 1_024,
                storage_kind: MediaAssetStorageKind::ManagedLocal,
                storage_key: "media-cas/ab/asset.mp4".to_string(),
                width: Some(1280),
                height: Some(720),
                duration_ms: Some(5_000),
            })
            .await
            .unwrap();
        let replay = runtime
            .register_asset(RegisterMediaAssetRequest {
                content_hash_sha256: asset_hash,
                content_verified_at: "2026-08-07T02:10:00Z".to_string(),
                media_type: "video/mp4".to_string(),
                byte_length: 1_024,
                storage_kind: MediaAssetStorageKind::ManagedLocal,
                storage_key: "media-cas/ab/asset.mp4".to_string(),
                width: Some(1280),
                height: Some(720),
                duration_ms: Some(5_000),
            })
            .await
            .unwrap();
        assert_eq!(replay.id, asset.id);

        let first_link = runtime
            .link_asset(LinkMediaAssetRequest {
                job_id: started.job.id.clone(),
                expected_revision: started.job.revision,
                idempotency_key: "output-occurrence-1".to_string(),
                attempt_id: attempt_id.clone(),
                asset_id: asset.id.clone(),
                parent_asset_id: None,
                relation_type: MediaAssetRelationType::Output,
                ordinal: 0,
                local_retention_policy: MediaAssetLocalRetentionPolicy::RetainUntilDeleted,
                local_retention_expires_at: None,
                metadata: json!({ "variant": 1 }),
            })
            .await
            .unwrap();
        let second_link = runtime
            .link_asset(LinkMediaAssetRequest {
                job_id: first_link.job.id.clone(),
                expected_revision: first_link.job.revision,
                idempotency_key: "output-occurrence-2".to_string(),
                attempt_id,
                asset_id: asset.id,
                parent_asset_id: None,
                relation_type: MediaAssetRelationType::Output,
                ordinal: 1,
                local_retention_policy: MediaAssetLocalRetentionPolicy::DeleteAfterExpiry,
                local_retention_expires_at: Some("2026-08-30T00:00:00Z".to_string()),
                metadata: json!({ "variant": 2 }),
            })
            .await
            .unwrap();
        assert_eq!(second_link.assets.len(), 1);
        assert_eq!(second_link.asset_relations.len(), 2);
        assert_ne!(
            second_link.asset_relations[0].local_retention_policy,
            second_link.asset_relations[1].local_retention_policy
        );
        assert!(second_link
            .asset_relations
            .iter()
            .all(|relation| !relation.attempt_id.is_empty()));
    }
}
