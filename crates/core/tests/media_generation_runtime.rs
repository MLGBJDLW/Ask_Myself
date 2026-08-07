use nexa_core::db::Database;
use nexa_core::db_executor::DatabaseExecutor;
use nexa_core::error::CoreError;
use nexa_core::media_generation::{
    BeginMediaJobAttemptRequest, CreateMediaJobRequest, DeleteMediaAssetOccurrenceRequest,
    ImportMediaAssetRequest, LinkMediaAssetRequest, MediaAssetLocalRetentionPolicy,
    MediaAssetLocalState, MediaAssetRelationType, MediaGenerationAssetStore,
    MediaGenerationRuntime, MediaJobAttemptState, MediaJobSnapshot, MediaJobState,
    MediaObservationMode, MediaOperation, MediaRecoveryAction, MediaRemoteDeletionStatus,
    ProviderUnknownReconciliation, RecordMediaJobRemoteDeletionResult,
    RecordMediaProviderEventRequest, RequestMediaAssetDeletion, RequestMediaJobCancellation,
    RequestMediaJobRemoteDeletion, TransitionMediaJobRequest,
};
use serde_json::json;
use sha2::{Digest, Sha256};

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

fn transition(snapshot: &MediaJobSnapshot, next_state: MediaJobState) -> TransitionMediaJobRequest {
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
) -> MediaJobSnapshot {
    runtime
        .begin_attempt(BeginMediaJobAttemptRequest {
            job_id: snapshot.job.id.clone(),
            expected_revision: snapshot.job.revision,
            idempotency_key: "attempt-1".to_string(),
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

fn provider_event(
    snapshot: &MediaJobSnapshot,
    attempt_id: &str,
    deduplication_key: &str,
    provider_task_id: &str,
    attempt_state: MediaJobAttemptState,
    next_job_state: MediaJobState,
) -> RecordMediaProviderEventRequest {
    RecordMediaProviderEventRequest {
        job_id: snapshot.job.id.clone(),
        expected_revision: snapshot.job.revision,
        attempt_id: attempt_id.to_string(),
        provider_id: "provider-a".to_string(),
        event_source: "urn:nexa:provider-a:endpoint-1:account-hash-a:us".to_string(),
        deduplication_key: deduplication_key.to_string(),
        event_kind: format!("job.{}", next_job_state.as_str()),
        payload: json!({ "status": next_job_state.as_str() }),
        provider_created_at: Some("2026-08-07T02:00:00Z".to_string()),
        provider_task_id: Some(provider_task_id.to_string()),
        attempt_state: Some(attempt_state),
        next_job_state: Some(next_job_state),
        error: None,
        retry_classification: None,
        next_eligible_at: None,
        cancellation_result: None,
        final_cost_micros: None,
        watermark_present: None,
        provenance: None,
    }
}

#[tokio::test]
async fn cancellation_intent_stays_nonterminal_until_confirmation() {
    let runtime = runtime(Database::open_memory().unwrap());
    let draft = runtime
        .create_job(create_request("cancel-contract"))
        .await
        .unwrap();
    let replay = runtime
        .create_job(create_request("cancel-contract"))
        .await
        .unwrap();
    assert_eq!(draft, replay);
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

    let confirmed = runtime
        .transition_job(transition(&requested, MediaJobState::Cancelled))
        .await
        .unwrap();
    assert_eq!(confirmed.job.state, MediaJobState::Cancelled);
}

#[tokio::test]
async fn provider_cancellation_request_result_and_confirmation_are_distinct() {
    let runtime = runtime(Database::open_memory().unwrap());
    let submitting = submitting_job(&runtime, "provider-cancel-contract").await;
    let started = begin_attempt(&runtime, &submitting).await;
    let attempt_id = started.attempts[0].id.clone();
    let queued = runtime
        .record_provider_event(provider_event(
            &started,
            &attempt_id,
            "cancel-accepted",
            "provider-task-cancel",
            MediaJobAttemptState::Accepted,
            MediaJobState::Queued,
        ))
        .await
        .unwrap();
    assert!(matches!(
        runtime
            .request_remote_deletion(RequestMediaJobRemoteDeletion {
                job_id: queued.job.id.clone(),
                expected_revision: queued.job.revision,
                attempt_id: attempt_id.clone(),
            })
            .await
            .unwrap_err(),
        CoreError::Conflict(_)
    ));
    let requested = runtime
        .request_cancellation(RequestMediaJobCancellation {
            job_id: queued.job.id.clone(),
            expected_revision: queued.job.revision,
            reason: "user_requested".to_string(),
        })
        .await
        .unwrap();
    assert!(requested.attempts[0].cancellation_requested_at.is_some());
    let requested = runtime
        .request_remote_deletion(RequestMediaJobRemoteDeletion {
            job_id: requested.job.id.clone(),
            expected_revision: requested.job.revision,
            attempt_id: attempt_id.clone(),
        })
        .await
        .unwrap();

    let unsupported = runtime
        .record_provider_event(RecordMediaProviderEventRequest {
            job_id: requested.job.id.clone(),
            expected_revision: requested.job.revision,
            attempt_id: attempt_id.clone(),
            provider_id: "provider-a".to_string(),
            event_source: "urn:nexa:provider-a:endpoint-1:account-hash-a:us".to_string(),
            deduplication_key: "cancel-unsupported".to_string(),
            event_kind: "cancellation.unsupported".to_string(),
            payload: json!({ "status": "unsupported" }),
            provider_created_at: None,
            provider_task_id: Some("provider-task-cancel".to_string()),
            attempt_state: None,
            next_job_state: None,
            error: None,
            retry_classification: None,
            next_eligible_at: None,
            cancellation_result: Some(json!({ "status": "unsupported" })),
            final_cost_micros: None,
            watermark_present: None,
            provenance: None,
        })
        .await
        .unwrap();
    assert_eq!(unsupported.job.state, MediaJobState::Queued);
    assert_eq!(
        unsupported.attempts[0].state,
        MediaJobAttemptState::Accepted
    );
    assert_eq!(
        unsupported.attempts[0]
            .cancellation_result
            .as_ref()
            .unwrap()["status"],
        "unsupported"
    );

    let mut confirmed = provider_event(
        &unsupported,
        &attempt_id,
        "cancel-confirmed",
        "provider-task-cancel",
        MediaJobAttemptState::Cancelled,
        MediaJobState::Cancelled,
    );
    confirmed.cancellation_result = Some(json!({ "status": "confirmed" }));
    let confirmed = runtime.record_provider_event(confirmed).await.unwrap();
    assert_eq!(confirmed.job.state, MediaJobState::Cancelled);
    assert_eq!(confirmed.attempts[0].state, MediaJobAttemptState::Cancelled);
}

#[tokio::test]
async fn restart_recovery_never_blindly_resubmits_ambiguous_work() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("media-runtime.db");
    let first = runtime(Database::new(&path).unwrap());
    let submitting = submitting_job(&first, "restart-contract").await;
    let started = begin_attempt(&first, &submitting).await;
    assert_eq!(started.attempts.len(), 1);
    drop(first);

    let restarted = runtime(Database::new(&path).unwrap());
    assert_eq!(restarted.recover_after_restart().await.unwrap(), 1);
    let recoverable = restarted.list_recoverable_jobs().await.unwrap();
    assert_eq!(recoverable.len(), 1);
    assert_eq!(recoverable[0].job.state, MediaJobState::ProviderUnknown);
    assert_eq!(
        recoverable[0].attempts[0].state,
        MediaJobAttemptState::ProviderUnknown
    );
    assert_eq!(recoverable[0].attempts.len(), 1);
}

#[tokio::test]
async fn restart_keeps_pre_side_effect_submission_ready_for_its_first_attempt() {
    let runtime = runtime(Database::open_memory().unwrap());
    let submitting = submitting_job(&runtime, "restart-before-attempt").await;
    assert!(submitting.job.current_attempt_id.is_none());
    assert_eq!(runtime.recover_after_restart().await.unwrap(), 0);
    let recovered = runtime.get_job(&submitting.job.id).await.unwrap();
    assert_eq!(recovered.job.state, MediaJobState::Submitting);
    let plan = runtime.build_recovery_plan().await.unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].action, MediaRecoveryAction::BeginSubmissionAttempt);
    let started = begin_attempt(&runtime, &recovered).await;
    assert_eq!(started.attempts.len(), 1);
}

#[tokio::test]
async fn retry_requires_durable_lookup_evidence_and_rejects_stale_attempt_events() {
    let runtime = runtime(Database::open_memory().unwrap());
    let submitting = submitting_job(&runtime, "reconcile-contract").await;
    let started = begin_attempt(&runtime, &submitting).await;
    let first_attempt_id = started.attempts[0].id.clone();

    let duplicate = runtime
        .begin_attempt(BeginMediaJobAttemptRequest {
            job_id: started.job.id.clone(),
            expected_revision: started.job.revision,
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
    assert!(matches!(duplicate, CoreError::Conflict(_)));

    assert_eq!(runtime.recover_after_restart().await.unwrap(), 1);
    let unknown = runtime.get_job(&started.job.id).await.unwrap();
    let plan = runtime.build_recovery_plan().await.unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].action, MediaRecoveryAction::LookupByIdempotencyKey);

    let oversized_evidence = runtime
        .begin_attempt(BeginMediaJobAttemptRequest {
            job_id: unknown.job.id.clone(),
            expected_revision: unknown.job.revision,
            idempotency_key: "attempt-oversized".to_string(),
            provider_id: "provider-a".to_string(),
            provider_source: "urn:nexa:provider-a:endpoint-1:account-hash-a:us".to_string(),
            model_id: "video-model-1".to_string(),
            api_version: Some("2026-08-01".to_string()),
            data_region: Some("us".to_string()),
            remote_retention_expires_at: Some("2026-08-14T00:00:00Z".to_string()),
            provider_unknown_reconciliation: Some(ProviderUnknownReconciliation {
                observed_at: "2026-08-07T03:00:00Z".to_string(),
                lookup_source: "urn:nexa:provider-a:endpoint-1:account-hash-a:us".to_string(),
                lookup_idempotency_key: "attempt-1".to_string(),
                lookup_evidence: json!({ "details": "x".repeat(70_000) }),
            }),
        })
        .await
        .unwrap_err();
    assert!(matches!(oversized_evidence, CoreError::InvalidInput(_)));

    let retried = runtime
        .begin_attempt(BeginMediaJobAttemptRequest {
            job_id: unknown.job.id.clone(),
            expected_revision: unknown.job.revision,
            idempotency_key: "attempt-2".to_string(),
            provider_id: "provider-a".to_string(),
            provider_source: "urn:nexa:provider-a:endpoint-1:account-hash-a:us".to_string(),
            model_id: "video-model-1".to_string(),
            api_version: Some("2026-08-01".to_string()),
            data_region: Some("us".to_string()),
            remote_retention_expires_at: Some("2026-08-21T00:00:00Z".to_string()),
            provider_unknown_reconciliation: Some(ProviderUnknownReconciliation {
                observed_at: "2026-08-07T03:00:00Z".to_string(),
                lookup_source: "urn:nexa:provider-a:endpoint-1:account-hash-a:us".to_string(),
                lookup_idempotency_key: "attempt-1".to_string(),
                lookup_evidence: json!({ "lookup": "idempotency_key", "matches": 0 }),
            }),
        })
        .await
        .unwrap();
    assert_eq!(retried.attempts.len(), 2);
    assert_eq!(
        retried.attempts[0].remote_retention_expires_at.as_deref(),
        Some("2026-08-14T00:00:00.000Z")
    );
    assert_eq!(
        retried.attempts[1].remote_retention_expires_at.as_deref(),
        Some("2026-08-21T00:00:00.000Z")
    );
    assert_eq!(
        retried.job.remote_retention_expires_at.as_deref(),
        Some("2026-08-21T00:00:00.000Z")
    );
    assert_eq!(retried.attempts[0].state, MediaJobAttemptState::Failed);
    assert_eq!(retried.attempts[1].state, MediaJobAttemptState::Created);
    assert_eq!(retried.provider_event_count, 2);
    assert!(retried.job.current_provider_task_id.is_none());

    let stale = runtime
        .record_provider_event(provider_event(
            &retried,
            &first_attempt_id,
            "late-old-attempt",
            "stale-provider-task",
            MediaJobAttemptState::Accepted,
            MediaJobState::Queued,
        ))
        .await
        .unwrap_err();
    assert!(matches!(stale, CoreError::Conflict(_)));
}

#[tokio::test]
async fn transient_provider_failure_appends_retry_and_can_delete_old_remote_attempt() {
    let runtime = runtime(Database::open_memory().unwrap());
    let submitting = submitting_job(&runtime, "transient-retry").await;
    let started = begin_attempt(&runtime, &submitting).await;
    let first_attempt_id = started.attempts[0].id.clone();
    let queued = runtime
        .record_provider_event(provider_event(
            &started,
            &first_attempt_id,
            "retry-accepted",
            "provider-task-retry-1",
            MediaJobAttemptState::Accepted,
            MediaJobState::Queued,
        ))
        .await
        .unwrap();
    let mut transient = provider_event(
        &queued,
        &first_attempt_id,
        "retry-transient-failure",
        "provider-task-retry-1",
        MediaJobAttemptState::Failed,
        MediaJobState::Submitting,
    );
    transient.error = Some(json!({ "code": "rate_limited" }));
    transient.retry_classification = Some("transient_rate_limit".to_string());
    let retry_ready = runtime.record_provider_event(transient).await.unwrap();
    assert_eq!(retry_ready.job.state, MediaJobState::Submitting);
    assert_eq!(retry_ready.attempts[0].state, MediaJobAttemptState::Failed);
    assert!(retry_ready.job.current_provider_task_id.is_none());
    assert_eq!(runtime.recover_after_restart().await.unwrap(), 0);
    let retry_plan = runtime.build_recovery_plan().await.unwrap();
    assert_eq!(retry_plan.len(), 1);
    assert_eq!(
        retry_plan[0].action,
        MediaRecoveryAction::BeginSubmissionAttempt
    );
    assert!(retry_plan[0].provider_task_id.is_none());

    let retried = runtime
        .begin_attempt(BeginMediaJobAttemptRequest {
            job_id: retry_ready.job.id.clone(),
            expected_revision: retry_ready.job.revision,
            idempotency_key: "attempt-2".to_string(),
            provider_id: "provider-a".to_string(),
            provider_source: "urn:nexa:provider-a:endpoint-1:account-hash-a:us".to_string(),
            model_id: "video-model-1".to_string(),
            api_version: Some("2026-08-01".to_string()),
            data_region: Some("us".to_string()),
            remote_retention_expires_at: Some("2026-08-21T00:00:00Z".to_string()),
            provider_unknown_reconciliation: None,
        })
        .await
        .unwrap();
    assert_eq!(retried.attempts.len(), 2);
    assert_eq!(
        retried.job.current_attempt_id,
        Some(retried.attempts[1].id.clone())
    );

    let deletion_requested = runtime
        .request_remote_deletion(RequestMediaJobRemoteDeletion {
            job_id: retried.job.id.clone(),
            expected_revision: retried.job.revision,
            attempt_id: first_attempt_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(
        deletion_requested.attempts[0].remote_deletion_status,
        MediaRemoteDeletionStatus::Requested
    );
    let deletion_confirmed = runtime
        .record_remote_deletion_result(RecordMediaJobRemoteDeletionResult {
            job_id: deletion_requested.job.id.clone(),
            expected_revision: deletion_requested.job.revision,
            attempt_id: first_attempt_id,
            event_source: "urn:nexa:provider-a:endpoint-1:account-hash-a:us".to_string(),
            deduplication_key: "retry-old-remote-deleted".to_string(),
            status: MediaRemoteDeletionStatus::Confirmed,
            error: None,
        })
        .await
        .unwrap();
    assert_eq!(
        deletion_confirmed.attempts[0].remote_deletion_status,
        MediaRemoteDeletionStatus::Confirmed
    );
}

#[tokio::test]
async fn provider_projection_enforces_source_task_and_completion_invariants() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = MediaGenerationRuntime::with_asset_store(
        DatabaseExecutor::new(Database::open_memory().unwrap(), 16).unwrap(),
        MediaGenerationAssetStore::new(directory.path().join("assets")),
    );
    let submitting = submitting_job(&runtime, "transition-contract").await;
    let started = begin_attempt(&runtime, &submitting).await;
    let attempt_id = started.attempts[0].id.clone();

    let mut oversized = provider_event(
        &started,
        &attempt_id,
        "oversized-event",
        "provider-task-1",
        MediaJobAttemptState::Accepted,
        MediaJobState::Queued,
    );
    oversized.error = Some(json!({ "details": "x".repeat(70_000) }));
    assert!(matches!(
        runtime.record_provider_event(oversized).await.unwrap_err(),
        CoreError::InvalidInput(_)
    ));

    let mut wrong_source = provider_event(
        &started,
        &attempt_id,
        "wrong-source",
        "provider-task-1",
        MediaJobAttemptState::Accepted,
        MediaJobState::Queued,
    );
    wrong_source.event_source = "urn:nexa:provider-a:endpoint-1:different-account:us".to_string();
    assert!(matches!(
        runtime
            .record_provider_event(wrong_source)
            .await
            .unwrap_err(),
        CoreError::InvalidInput(_)
    ));

    let queued = runtime
        .record_provider_event(provider_event(
            &started,
            &attempt_id,
            "accepted",
            "provider-task-1",
            MediaJobAttemptState::Accepted,
            MediaJobState::Queued,
        ))
        .await
        .unwrap();
    assert!(matches!(
        runtime
            .transition_job(transition(&queued, MediaJobState::Completed))
            .await
            .unwrap_err(),
        CoreError::Conflict(_)
    ));
    assert!(matches!(
        runtime
            .record_provider_event(provider_event(
                &queued,
                &attempt_id,
                "task-mismatch",
                "provider-task-2",
                MediaJobAttemptState::Observing,
                MediaJobState::Running,
            ))
            .await
            .unwrap_err(),
        CoreError::Conflict(_)
    ));

    let running = runtime
        .record_provider_event(provider_event(
            &queued,
            &attempt_id,
            "running",
            "provider-task-1",
            MediaJobAttemptState::Observing,
            MediaJobState::Running,
        ))
        .await
        .unwrap();
    let post_processing = runtime
        .record_provider_event(provider_event(
            &running,
            &attempt_id,
            "succeeded",
            "provider-task-1",
            MediaJobAttemptState::Succeeded,
            MediaJobState::PostProcessing,
        ))
        .await
        .unwrap();
    assert!(matches!(
        runtime
            .transition_job(transition(&post_processing, MediaJobState::Completed))
            .await
            .unwrap_err(),
        CoreError::Conflict(_)
    ));

    let source = directory.path().join("output.mp4");
    std::fs::write(&source, b"\0\0\0\x18ftypisom\0\0\0\0verified-output").unwrap();
    let asset = runtime
        .import_asset(ImportMediaAssetRequest {
            source_path: source,
            declared_media_type: "video/mp4".to_string(),
            expected_sha256: None,
            expected_byte_length: None,
            width: Some(1280),
            height: Some(720),
            duration_ms: Some(5_000),
        })
        .await
        .unwrap();
    let asset_id = asset.id.clone();
    let asset_storage_key = asset.storage_key.clone();
    let linked = runtime
        .link_asset(LinkMediaAssetRequest {
            job_id: post_processing.job.id.clone(),
            expected_revision: post_processing.job.revision,
            idempotency_key: "final-output".to_string(),
            attempt_id: attempt_id.clone(),
            asset_id: asset.id,
            parent_asset_id: None,
            relation_type: MediaAssetRelationType::Output,
            ordinal: 0,
            local_retention_policy: MediaAssetLocalRetentionPolicy::DeleteAfterExpiry,
            local_retention_expires_at: Some("2026-08-30T00:00:00Z".to_string()),
            metadata: json!({ "providerTaskId": "provider-task-1", "watermarkPresent": false }),
        })
        .await
        .unwrap();
    assert_eq!(
        linked.asset_relations[0].local_retention_policy,
        MediaAssetLocalRetentionPolicy::DeleteAfterExpiry
    );
    assert_eq!(
        linked.asset_relations[0]
            .local_retention_expires_at
            .as_deref(),
        Some("2026-08-30T00:00:00.000Z")
    );
    let completed = runtime
        .transition_job(transition(&linked, MediaJobState::Completed))
        .await
        .unwrap();
    assert_eq!(completed.job.state, MediaJobState::Completed);
    assert!(completed.asset_relations[0]
        .metadata
        .get("modelId")
        .is_some());

    let late = runtime
        .record_provider_event(RecordMediaProviderEventRequest {
            job_id: completed.job.id.clone(),
            expected_revision: completed.job.revision,
            attempt_id: attempt_id.clone(),
            provider_id: "provider-a".to_string(),
            event_source: "urn:nexa:provider-a:endpoint-1:account-hash-a:us".to_string(),
            deduplication_key: "late-terminal".to_string(),
            event_kind: "job.observed".to_string(),
            payload: json!({ "status": "completed" }),
            provider_created_at: None,
            provider_task_id: Some("provider-task-1".to_string()),
            attempt_state: None,
            next_job_state: None,
            error: None,
            retry_classification: None,
            next_eligible_at: None,
            cancellation_result: None,
            final_cost_micros: Some(999),
            watermark_present: Some(true),
            provenance: Some(json!({ "late": true })),
        })
        .await
        .unwrap_err();
    assert!(matches!(late, CoreError::Conflict(_)));

    let remote_requested = runtime
        .request_remote_deletion(RequestMediaJobRemoteDeletion {
            job_id: completed.job.id.clone(),
            expected_revision: completed.job.revision,
            attempt_id: attempt_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(
        remote_requested.attempts[0].remote_deletion_status,
        MediaRemoteDeletionStatus::Requested
    );
    let remote_result = runtime
        .record_remote_deletion_result(RecordMediaJobRemoteDeletionResult {
            job_id: remote_requested.job.id.clone(),
            expected_revision: remote_requested.job.revision,
            attempt_id,
            event_source: "urn:nexa:provider-a:endpoint-1:account-hash-a:us".to_string(),
            deduplication_key: "remote-delete-unsupported".to_string(),
            status: MediaRemoteDeletionStatus::Unsupported,
            error: Some("provider does not expose deletion".to_string()),
        })
        .await
        .unwrap();
    let relation_id = remote_result.asset_relations[0].id.clone();
    let unlinked = runtime
        .delete_asset_occurrence(DeleteMediaAssetOccurrenceRequest {
            job_id: remote_result.job.id.clone(),
            expected_revision: remote_result.job.revision,
            relation_id,
        })
        .await
        .unwrap();
    assert!(unlinked.asset_relations.is_empty());
    let deleted = runtime
        .delete_asset(RequestMediaAssetDeletion { asset_id })
        .await
        .unwrap();
    assert_eq!(deleted.local_state, MediaAssetLocalState::Deleted);
    assert!(!directory
        .path()
        .join("assets")
        .join(asset_storage_key)
        .exists());
}

#[tokio::test]
async fn event_scope_and_attempt_lineage_survive_content_deduplication() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = MediaGenerationRuntime::with_asset_store(
        DatabaseExecutor::new(Database::open_memory().unwrap(), 16).unwrap(),
        MediaGenerationAssetStore::new(directory.path().join("assets")),
    );
    let submitting = submitting_job(&runtime, "lineage-contract").await;
    let started = begin_attempt(&runtime, &submitting).await;
    let attempt_id = started.attempts[0].id.clone();
    let accepted = runtime
        .record_provider_event(RecordMediaProviderEventRequest {
            job_id: started.job.id.clone(),
            expected_revision: started.job.revision,
            attempt_id: attempt_id.clone(),
            provider_id: "provider-a".to_string(),
            event_source: "urn:nexa:provider-a:endpoint-1:account-hash-a:us".to_string(),
            deduplication_key: "event-1".to_string(),
            event_kind: "job.accepted".to_string(),
            payload: json!({
                "status": "queued",
                "authorization": "Bearer secret",
                "downloadUrl": "https://provider.example/output.mp4?signature=secret",
                "message": "Authorization: Bearer sk-provider-secret",
                "details": "api_key=provider-secret",
                "header": "x-api-key: provider-secret",
                "credentialUrl": "https://user:password@provider.example/output.mp4"
            }),
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
        })
        .await
        .unwrap();
    assert_eq!(accepted.provider_events.len(), 1);
    assert_eq!(accepted.provider_event_count, 1);
    let event_page = runtime
        .list_provider_events(&accepted.job.id, 0, 10)
        .await
        .unwrap();
    assert_eq!(event_page.len(), 1);
    assert_eq!(event_page[0].sequence, 1);
    assert_eq!(
        accepted.provider_events[0].payload["authorization"],
        "[REDACTED]"
    );
    assert_eq!(
        accepted.provider_events[0].payload["downloadUrl"],
        "https://provider.example/output.mp4?[REDACTED]"
    );
    assert_eq!(accepted.provider_events[0].payload["message"], "[REDACTED]");
    assert_eq!(accepted.provider_events[0].payload["details"], "[REDACTED]");
    assert_eq!(accepted.provider_events[0].payload["header"], "[REDACTED]");
    assert_eq!(
        accepted.provider_events[0].payload["credentialUrl"],
        "[REDACTED]"
    );

    let source = directory.path().join("provider-output.mp4");
    std::fs::write(&source, b"\0\0\0\x18ftypisom\0\0\0\0verified-media").unwrap();
    let asset = runtime
        .import_asset(ImportMediaAssetRequest {
            source_path: source,
            declared_media_type: "video/mp4".to_string(),
            expected_sha256: None,
            expected_byte_length: None,
            width: Some(1280),
            height: Some(720),
            duration_ms: Some(5_000),
        })
        .await
        .unwrap();
    let tampered_hash = runtime
        .import_asset(ImportMediaAssetRequest {
            source_path: directory.path().join("provider-output.mp4"),
            declared_media_type: "video/mp4".to_string(),
            expected_sha256: Some("00".repeat(32)),
            expected_byte_length: None,
            width: None,
            height: None,
            duration_ms: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(tampered_hash, CoreError::Conflict(_)));

    let invalid_source = directory.path().join("invalid.mp4");
    std::fs::write(&invalid_source, b"not-an-mp4").unwrap();
    let invalid_signature = runtime
        .import_asset(ImportMediaAssetRequest {
            source_path: invalid_source,
            declared_media_type: "video/mp4".to_string(),
            expected_sha256: None,
            expected_byte_length: None,
            width: None,
            height: None,
            duration_ms: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(invalid_signature, CoreError::InvalidInput(_)));
    assert!(matches!(
        runtime
            .link_asset(LinkMediaAssetRequest {
                job_id: accepted.job.id.clone(),
                expected_revision: accepted.job.revision,
                idempotency_key: "invalid-retention".to_string(),
                attempt_id: attempt_id.clone(),
                asset_id: asset.id.clone(),
                parent_asset_id: None,
                relation_type: MediaAssetRelationType::Output,
                ordinal: 0,
                local_retention_policy: MediaAssetLocalRetentionPolicy::DeleteAfterExpiry,
                local_retention_expires_at: None,
                metadata: json!({}),
            })
            .await
            .unwrap_err(),
        CoreError::InvalidInput(_)
    ));
    let first_relation = runtime
        .link_asset(LinkMediaAssetRequest {
            job_id: accepted.job.id.clone(),
            expected_revision: accepted.job.revision,
            idempotency_key: "output-1".to_string(),
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
    let second_relation = runtime
        .link_asset(LinkMediaAssetRequest {
            job_id: first_relation.job.id.clone(),
            expected_revision: first_relation.job.revision,
            idempotency_key: "output-2".to_string(),
            attempt_id,
            asset_id: asset.id,
            parent_asset_id: None,
            relation_type: MediaAssetRelationType::Output,
            ordinal: 1,
            local_retention_policy: MediaAssetLocalRetentionPolicy::DeleteAfterExpiry,
            local_retention_expires_at: Some("2026-09-01T00:00:00Z".to_string()),
            metadata: json!({ "variant": 2 }),
        })
        .await
        .unwrap();
    assert_eq!(second_relation.assets.len(), 1);
    assert_eq!(second_relation.asset_relations.len(), 2);
    assert_ne!(
        second_relation.asset_relations[0].local_retention_policy,
        second_relation.asset_relations[1].local_retention_policy
    );
}

#[tokio::test]
async fn provider_event_history_has_a_per_job_byte_budget() {
    let runtime = runtime(Database::open_memory().unwrap());
    let submitting = submitting_job(&runtime, "event-budget").await;
    let mut current = begin_attempt(&runtime, &submitting).await;
    let attempt_id = current.attempts[0].id.clone();
    let mut exhausted = false;

    for sequence in 0..100_u32 {
        let result = runtime
            .record_provider_event(RecordMediaProviderEventRequest {
                job_id: current.job.id.clone(),
                expected_revision: current.job.revision,
                attempt_id: attempt_id.clone(),
                provider_id: "provider-a".to_string(),
                event_source: "urn:nexa:provider-a:endpoint-1:account-hash-a:us".to_string(),
                deduplication_key: format!("large-observation-{sequence}"),
                event_kind: "job.diagnostic".to_string(),
                payload: json!({ "diagnostic": "x".repeat(60_000) }),
                provider_created_at: None,
                provider_task_id: None,
                attempt_state: None,
                next_job_state: None,
                error: None,
                retry_classification: None,
                next_eligible_at: None,
                cancellation_result: None,
                final_cost_micros: None,
                watermark_present: None,
                provenance: None,
            })
            .await;
        match result {
            Ok(snapshot) => current = snapshot,
            Err(CoreError::Conflict(message)) if message.contains("provider event budget") => {
                exhausted = true;
                break;
            }
            Err(error) => panic!("unexpected provider event result: {error}"),
        }
    }

    assert!(
        exhausted,
        "large unique events must hit a durable byte budget"
    );
    assert!(current.provider_event_count < 100);
}

#[tokio::test]
async fn ordered_inputs_are_fingerprinted_and_linked_per_attempt() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = MediaGenerationRuntime::with_asset_store(
        DatabaseExecutor::new(Database::open_memory().unwrap(), 16).unwrap(),
        MediaGenerationAssetStore::new(directory.path().join("assets")),
    );
    let mut asset_ids = Vec::new();
    for (name, marker) in [("first.png", 1_u8), ("last.png", 2_u8)] {
        let source = directory.path().join(name);
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&[marker; 64]);
        std::fs::write(&source, bytes).unwrap();
        asset_ids.push(
            runtime
                .import_asset(ImportMediaAssetRequest {
                    source_path: source,
                    declared_media_type: "image/png".to_string(),
                    expected_sha256: None,
                    expected_byte_length: None,
                    width: Some(1920),
                    height: Some(1080),
                    duration_ms: None,
                })
                .await
                .unwrap()
                .id,
        );
    }

    let mut request = create_request("ordered-inputs");
    request.operation = MediaOperation::FirstLastFrame;
    request.input_asset_ids = asset_ids.clone();
    let draft = runtime.create_job(request.clone()).await.unwrap();
    assert_eq!(draft.job.input_asset_ids, asset_ids);
    assert!(matches!(
        runtime
            .delete_asset(RequestMediaAssetDeletion {
                asset_id: asset_ids[0].clone(),
            })
            .await
            .unwrap_err(),
        CoreError::Conflict(_)
    ));

    request.input_asset_ids.reverse();
    assert!(matches!(
        runtime.create_job(request).await.unwrap_err(),
        CoreError::Conflict(_)
    ));

    let validating = runtime
        .transition_job(transition(&draft, MediaJobState::Validating))
        .await
        .unwrap();
    let uploading = runtime
        .transition_job(transition(&validating, MediaJobState::UploadingAssets))
        .await
        .unwrap();
    let submitting = runtime
        .transition_job(transition(&uploading, MediaJobState::Submitting))
        .await
        .unwrap();
    let started = begin_attempt(&runtime, &submitting).await;
    assert_eq!(started.asset_relations.len(), 2);
    assert_eq!(
        started.asset_relations[0].relation_type,
        MediaAssetRelationType::Input
    );
    assert_eq!(started.asset_relations[0].ordinal, 0);
    assert_eq!(started.asset_relations[0].asset_id, asset_ids[0]);
    assert_eq!(started.asset_relations[1].ordinal, 1);
    assert_eq!(started.asset_relations[1].asset_id, asset_ids[1]);
    assert_eq!(
        started.asset_relations[0].metadata["providerId"],
        "provider-a"
    );
    assert!(matches!(
        runtime
            .delete_asset(RequestMediaAssetDeletion {
                asset_id: asset_ids[0].clone(),
            })
            .await
            .unwrap_err(),
        CoreError::Conflict(_)
    ));

    let mut credential_source = create_request("credential-source");
    credential_source.provider_source =
        "https://user:password@provider.example/account/us".to_string();
    assert!(matches!(
        runtime.create_job(credential_source).await.unwrap_err(),
        CoreError::InvalidInput(_)
    ));
}

#[tokio::test]
async fn local_deletion_is_reference_safe_and_database_failure_rolls_back_blob() {
    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("asset-runtime.db");
    let asset_root = directory.path().join("assets");
    let source = directory.path().join("orphan.mp4");
    let bytes = b"\0\0\0\x18ftypisom\0\0\0\0rollback-verified";
    std::fs::write(&source, bytes).unwrap();
    let hash = format!("{:x}", Sha256::digest(bytes));

    {
        let conn = rusqlite::Connection::open(&database_path).unwrap();
        nexa_core::migrations::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO media_assets (
                 id, content_hash_sha256, content_verified_at, media_type, byte_length,
                 storage_kind, storage_key
             ) VALUES (?1, ?1, '2026-08-07T00:00:00.000Z', 'video/mp4', ?2,
                 'managed_local', 'sha256/ff/different.mp4')",
            rusqlite::params![hash, i64::try_from(bytes.len()).unwrap()],
        )
        .unwrap();
    }
    let runtime = MediaGenerationRuntime::with_asset_store(
        DatabaseExecutor::new(Database::new(&database_path).unwrap(), 16).unwrap(),
        MediaGenerationAssetStore::new(&asset_root),
    );
    assert!(matches!(
        runtime
            .import_asset(ImportMediaAssetRequest {
                source_path: source.clone(),
                declared_media_type: "video/mp4".to_string(),
                expected_sha256: Some(hash.clone()),
                expected_byte_length: Some(bytes.len() as u64),
                width: None,
                height: None,
                duration_ms: None,
            })
            .await
            .unwrap_err(),
        CoreError::Conflict(_)
    ));
    assert!(!asset_root
        .join(format!("sha256/{}/{}.mp4", &hash[..2], hash))
        .exists());

    let fresh_source = directory.path().join("fresh.mp4");
    std::fs::write(&fresh_source, b"\0\0\0\x18ftypisom\0\0\0\0fresh-delete").unwrap();
    let asset = runtime
        .import_asset(ImportMediaAssetRequest {
            source_path: fresh_source,
            declared_media_type: "video/mp4".to_string(),
            expected_sha256: None,
            expected_byte_length: None,
            width: None,
            height: None,
            duration_ms: None,
        })
        .await
        .unwrap();
    let deleted = runtime
        .delete_asset(RequestMediaAssetDeletion {
            asset_id: asset.id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(deleted.local_state, MediaAssetLocalState::Deleted);
    assert!(!asset_root.join(asset.storage_key).exists());
}

#[tokio::test]
async fn concurrent_same_hash_import_never_rolls_back_the_committed_blob() {
    let directory = tempfile::tempdir().unwrap();
    let asset_root = directory.path().join("assets");
    let source = directory.path().join("same.mp4");
    std::fs::write(&source, b"\0\0\0\x18ftypisom\0\0\0\0concurrent-import").unwrap();
    let runtime = MediaGenerationRuntime::with_asset_store(
        DatabaseExecutor::new(Database::open_memory().unwrap(), 16).unwrap(),
        MediaGenerationAssetStore::new(&asset_root),
    );
    let first_runtime = runtime.clone();
    let first_source = source.clone();
    let first = async move {
        first_runtime
            .import_asset(ImportMediaAssetRequest {
                source_path: first_source,
                declared_media_type: "video/mp4".to_string(),
                expected_sha256: None,
                expected_byte_length: None,
                width: Some(1280),
                height: Some(720),
                duration_ms: None,
            })
            .await
    };
    let second_runtime = runtime.clone();
    let second = async move {
        second_runtime
            .import_asset(ImportMediaAssetRequest {
                source_path: source,
                declared_media_type: "video/mp4".to_string(),
                expected_sha256: None,
                expected_byte_length: None,
                width: Some(1920),
                height: Some(1080),
                duration_ms: None,
            })
            .await
    };
    let (first, second) = tokio::join!(first, second);
    let committed = match (first, second) {
        (Ok(asset), Err(CoreError::Conflict(_))) | (Err(CoreError::Conflict(_)), Ok(asset)) => {
            asset
        }
        outcome => panic!("expected one committed import and one metadata conflict: {outcome:?}"),
    };
    assert!(asset_root.join(committed.storage_key).is_file());
}
