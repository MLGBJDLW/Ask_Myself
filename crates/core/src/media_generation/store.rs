use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::Url;
use uuid::Uuid;

use crate::db::Database;
use crate::error::CoreError;

use super::model::{
    BeginMediaJobAttemptRequest, CreateMediaJobRequest, DeleteMediaAssetOccurrenceRequest,
    LinkMediaAssetRequest, MediaAssetLocalRetentionPolicy, MediaAssetRecord,
    MediaAssetRelationRecord, MediaAssetRelationType, MediaJobAttemptRecord, MediaJobRecord,
    MediaJobSnapshot, MediaJobState, MediaProviderEventRecord, MediaRecoveryAction,
    MediaRecoveryPlanItem, MediaRemoteDeletionStatus, RecordMediaJobRemoteDeletionResult,
    RecordMediaProviderEventRequest, RegisterMediaAssetRequest, RequestMediaAssetDeletion,
    RequestMediaJobCancellation, RequestMediaJobRemoteDeletion, TransitionMediaJobRequest,
};

const MAX_PROVIDER_EVENT_PAYLOAD_BYTES: usize = 64 * 1024;
const MAX_EXTERNAL_PROVIDER_EVENTS_PER_JOB: i64 = 1_984;
const MAX_PROVIDER_EVENTS_PER_JOB: i64 = 2_048;
const MAX_EXTERNAL_PROVIDER_EVENT_BYTES_PER_JOB: i64 = 4 * 1024 * 1024;
const MAX_PROVIDER_EVENT_BYTES_PER_JOB: i64 = 5 * 1024 * 1024;
const MAX_CREATE_JSON_BYTES: usize = 256 * 1024;

const JOB_PROJECTION: &str = r#"json_object(
    'id', id,
    'idempotencyKey', idempotency_key,
    'projectId', project_id,
    'conversationId', conversation_id,
    'providerId', provider_id,
    'providerSource', provider_source,
    'modelId', model_id,
    'apiVersion', api_version,
    'operation', operation,
    'inputAssetIds', json(input_asset_ids_json),
    'state', state,
    'revision', revision,
    'rawParameters', json(raw_parameters_json),
    'normalizedParameters', json(normalized_parameters_json),
    'providerExtras', json(provider_extras_json),
    'observationMode', observation_mode,
    'currentAttemptId', current_attempt_id,
    'currentProviderTaskId', current_provider_task_id,
    'retryCount', retry_count,
    'maxAttempts', max_attempts,
    'estimatedCostMicros', estimated_cost_micros,
    'finalCostMicros', final_cost_micros,
    'currency', currency,
    'dataRegion', data_region,
    'remoteRetentionExpiresAt', remote_retention_expires_at,
    'cancellationRequestedAt', cancellation_requested_at,
    'cancellationReason', cancellation_reason,
    'allowCrossProviderFallback', CASE allow_cross_provider_fallback
        WHEN 1 THEN json('true') ELSE json('false') END,
    'watermarkPresent', CASE
        WHEN watermark_present IS NULL THEN NULL
        WHEN watermark_present = 1 THEN json('true') ELSE json('false') END,
    'provenance', json(provenance_json),
    'lastProviderObservedAt', last_provider_observed_at,
    'createdAt', created_at,
    'updatedAt', updated_at,
    'completedAt', completed_at,
    'expiresAt', expires_at
)"#;

const ATTEMPT_PROJECTION: &str = r#"json_object(
    'id', id,
    'jobId', job_id,
    'attemptNumber', attempt_number,
    'idempotencyKey', idempotency_key,
    'providerId', provider_id,
    'providerSource', provider_source,
    'modelId', model_id,
    'apiVersion', api_version,
    'dataRegion', data_region,
    'remoteRetentionExpiresAt', remote_retention_expires_at,
    'crossProviderFallbackAuthorized', CASE cross_provider_fallback_authorized
        WHEN 1 THEN json('true') ELSE json('false') END,
    'providerTaskId', provider_task_id,
    'state', state,
    'error', CASE WHEN error_json IS NULL THEN NULL ELSE json(error_json) END,
    'retryClassification', retry_classification,
    'nextEligibleAt', next_eligible_at,
    'cancellationRequestedAt', cancellation_requested_at,
    'cancellationResult', CASE WHEN cancellation_result_json IS NULL THEN NULL ELSE json(cancellation_result_json) END,
    'remoteDeletionRequestedAt', remote_deletion_requested_at,
    'remoteDeletionStatus', remote_deletion_status,
    'remoteDeletionCompletedAt', remote_deletion_completed_at,
    'remoteDeletionError', CASE WHEN remote_deletion_error_json IS NULL THEN NULL ELSE json(remote_deletion_error_json) END,
    'submittedAt', submitted_at,
    'lastObservedAt', last_observed_at,
    'completedAt', completed_at,
    'createdAt', created_at,
    'updatedAt', updated_at
)"#;

const ASSET_PROJECTION: &str = r#"json_object(
    'id', id,
    'contentHashSha256', content_hash_sha256,
    'contentVerifiedAt', content_verified_at,
    'mediaType', media_type,
    'byteLength', byte_length,
    'storageKind', storage_kind,
    'storageKey', storage_key,
    'width', width,
    'height', height,
    'durationMs', duration_ms,
    'localState', local_state,
    'localDeletionRequestedAt', local_deletion_requested_at,
    'localDeletionCompletedAt', local_deletion_completed_at,
    'createdAt', created_at,
    'updatedAt', updated_at
)"#;

const RELATION_PROJECTION: &str = r#"json_object(
    'id', id,
    'jobId', job_id,
    'attemptId', attempt_id,
    'assetId', asset_id,
    'parentAssetId', parent_asset_id,
    'relationType', relation_type,
    'ordinal', ordinal,
    'localRetentionPolicy', local_retention_policy,
    'localRetentionExpiresAt', local_retention_expires_at,
    'metadata', json(metadata_json),
    'createdAt', created_at
)"#;

const EVENT_PROJECTION: &str = r#"json_object(
    'id', id,
    'jobId', job_id,
    'attemptId', attempt_id,
    'sequence', sequence,
    'providerId', provider_id,
    'eventSource', event_source,
    'deduplicationKey', deduplication_key,
    'eventKind', event_kind,
    'payload', json(payload_json),
    'providerCreatedAt', provider_created_at,
    'observedAt', observed_at
)"#;

pub(crate) fn create_job(
    database: &Database,
    request: CreateMediaJobRequest,
) -> Result<MediaJobSnapshot, CoreError> {
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let snapshot = create_job_in_transaction(&tx, request)?;
    tx.commit()?;
    Ok(snapshot)
}

pub(crate) fn create_job_in_transaction(
    conn: &Connection,
    mut request: CreateMediaJobRequest,
) -> Result<MediaJobSnapshot, CoreError> {
    normalize_create_request(&mut request)?;
    let fingerprint = request_fingerprint(&request)?;

    if let Some((job_id, stored_fingerprint)) = conn
        .query_row(
            "SELECT id, request_fingerprint_sha256 FROM media_jobs WHERE idempotency_key = ?1",
            [&request.idempotency_key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
    {
        if stored_fingerprint != fingerprint {
            return Err(CoreError::Conflict(format!(
                "Media job idempotency key `{}` was already used for different input",
                request.idempotency_key
            )));
        }
        return load_snapshot(conn, &job_id);
    }

    for asset_id in &request.input_asset_ids {
        let asset = load_asset(conn, asset_id)?;
        if asset.local_state != super::model::MediaAssetLocalState::Available {
            return Err(CoreError::Conflict(format!(
                "Input media asset {asset_id} is not locally available"
            )));
        }
    }

    let job_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO media_jobs (
             id, idempotency_key, request_fingerprint_sha256, project_id, conversation_id,
             provider_id, provider_source, model_id, api_version, operation, input_asset_ids_json, state,
             raw_parameters_json, normalized_parameters_json, provider_extras_json,
             observation_mode, max_attempts, estimated_cost_micros, currency, data_region,
             remote_retention_expires_at, allow_cross_provider_fallback
         ) VALUES (
             ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'draft',
             ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21
         )",
        rusqlite::params![
            job_id,
            request.idempotency_key,
            fingerprint,
            request.project_id,
            request.conversation_id,
            request.provider_id,
            request.provider_source,
            request.model_id,
            request.api_version,
            request.operation.as_str(),
            serde_json::to_string(&request.input_asset_ids)?,
            serde_json::to_string(&request.raw_parameters)?,
            serde_json::to_string(&request.normalized_parameters)?,
            serde_json::to_string(&request.provider_extras)?,
            request.observation_mode.as_str(),
            i64::from(request.max_attempts),
            request.estimated_cost_micros,
            request.currency,
            request.data_region,
            request.remote_retention_expires_at,
            bool_to_i64(request.allow_cross_provider_fallback),
        ],
    )?;
    load_snapshot(conn, &job_id)
}

pub(crate) fn get_job(database: &Database, job_id: &str) -> Result<MediaJobSnapshot, CoreError> {
    let conn = database.conn();
    load_snapshot(&conn, required(job_id, "job_id", 128)?)
}

pub(crate) fn get_asset(
    database: &Database,
    asset_id: &str,
) -> Result<MediaAssetRecord, CoreError> {
    let conn = database.conn();
    load_asset(&conn, required(asset_id, "asset_id", 128)?)
}

pub(crate) fn list_recoverable_jobs(
    database: &Database,
) -> Result<Vec<MediaJobSnapshot>, CoreError> {
    let conn = database.conn();
    let mut statement = conn.prepare(
        "SELECT id FROM media_jobs
         WHERE state NOT IN ('completed', 'failed', 'cancelled', 'expired')
         ORDER BY updated_at ASC, id ASC",
    )?;
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    ids.iter().map(|id| load_snapshot(&conn, id)).collect()
}

pub(crate) fn recover_after_restart(database: &Database) -> Result<usize, CoreError> {
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut interrupted_statement = tx.prepare(
        "SELECT jobs.id, attempts.id, jobs.provider_id, jobs.provider_source
         FROM media_jobs AS jobs
         JOIN media_job_attempts AS attempts
           ON attempts.id = jobs.current_attempt_id AND attempts.job_id = jobs.id
         WHERE jobs.state = 'submitting' AND jobs.current_provider_task_id IS NULL
           AND attempts.state IN ('created', 'submitting')",
    )?;
    let interrupted = interrupted_statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(interrupted_statement);
    let mut interrupted_attempts = 0;
    let mut interrupted_jobs = 0;
    for (job_id, attempt_id, provider_id, provider_source) in interrupted {
        interrupted_attempts += tx.execute(
            "UPDATE media_job_attempts
             SET state = 'provider_unknown', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?1 AND job_id = ?2 AND state IN ('created', 'submitting')",
            rusqlite::params![attempt_id, job_id],
        )?;
        let updated_job = tx.execute(
            "UPDATE media_jobs
             SET state = 'provider_unknown', revision = revision + 1,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?1 AND current_attempt_id = ?2 AND state = 'submitting'
               AND current_provider_task_id IS NULL",
            rusqlite::params![job_id, attempt_id],
        )?;
        interrupted_jobs += updated_job;
        if updated_job == 0 {
            continue;
        }
        insert_internal_event(
            &tx,
            InternalEvent {
                job_id: &job_id,
                attempt_id: &attempt_id,
                provider_id: &provider_id,
                event_source: &provider_source,
                deduplication_key: &format!("restart:{job_id}:{attempt_id}"),
                event_kind: "recovery.submission_ambiguous",
                payload: &json!({
                    "reason": "process_restart",
                    "requiredAction": "lookup_by_idempotency_key",
                    "blindResubmissionAllowed": false,
                }),
            },
        )?;
    }
    tx.commit()?;
    tracing::debug!(
        interrupted_jobs,
        interrupted_attempts,
        "Recovered ambiguous media submissions without resubmitting them"
    );
    Ok(interrupted_jobs)
}

pub(crate) fn build_recovery_plan(
    database: &Database,
) -> Result<Vec<MediaRecoveryPlanItem>, CoreError> {
    let conn = database.conn();
    let mut statement = conn.prepare(
        "SELECT id FROM media_jobs
         WHERE state NOT IN ('completed', 'failed', 'cancelled', 'expired')
            OR EXISTS (
                SELECT 1 FROM media_job_attempts
                WHERE media_job_attempts.job_id = media_jobs.id
                  AND media_job_attempts.remote_deletion_status = 'requested'
            )
         ORDER BY updated_at ASC, id ASC",
    )?;
    let job_ids = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    job_ids
        .into_iter()
        .map(|job_id| {
            let job = load_job_record(&conn, &job_id)?;
            let pending_remote = load_pending_remote_deletion_attempt(&conn, &job_id)?;
            let current_attempt = job
                .current_attempt_id
                .as_deref()
                .map(|attempt_id| load_attempt(&conn, attempt_id))
                .transpose()?;
            let (action, attempt_id, provider_source, provider_task_id) = if let Some(attempt) =
                pending_remote
            {
                (
                    MediaRecoveryAction::RequestRemoteDeletion,
                    Some(attempt.id),
                    attempt.provider_source,
                    attempt.provider_task_id,
                )
            } else if job.cancellation_requested_at.is_some() && job.current_attempt_id.is_some() {
                (
                    MediaRecoveryAction::ReconcileCancellation,
                    job.current_attempt_id.clone(),
                    job.provider_source.clone(),
                    job.current_provider_task_id.clone(),
                )
            } else {
                let action = match job.state {
                    MediaJobState::Draft | MediaJobState::Validating => {
                        MediaRecoveryAction::ValidateLocally
                    }
                    MediaJobState::UploadingAssets => MediaRecoveryAction::UploadInputs,
                    MediaJobState::Submitting
                        if job.current_attempt_id.is_none()
                            || current_attempt.as_ref().is_some_and(|attempt| {
                                attempt.state == super::model::MediaJobAttemptState::Failed
                                    && attempt.retry_classification.is_some()
                            }) =>
                    {
                        MediaRecoveryAction::BeginSubmissionAttempt
                    }
                    MediaJobState::Submitting | MediaJobState::ProviderUnknown
                        if job.current_provider_task_id.is_none() =>
                    {
                        MediaRecoveryAction::LookupByIdempotencyKey
                    }
                    MediaJobState::Submitting
                    | MediaJobState::Queued
                    | MediaJobState::Running
                    | MediaJobState::ProviderUnknown => MediaRecoveryAction::ObserveProviderTask,
                    MediaJobState::PostProcessing => MediaRecoveryAction::ResumePostProcessing,
                    MediaJobState::Completed
                    | MediaJobState::Failed
                    | MediaJobState::Cancelled
                    | MediaJobState::Expired => MediaRecoveryAction::ValidateLocally,
                };
                (
                    action,
                    job.current_attempt_id.clone(),
                    job.provider_source.clone(),
                    job.current_provider_task_id.clone(),
                )
            };
            Ok(MediaRecoveryPlanItem {
                job_id: job.id,
                attempt_id,
                revision: job.revision,
                action,
                provider_source,
                provider_task_id,
                cancellation_requested: job.cancellation_requested_at.is_some(),
            })
        })
        .collect()
}

pub(crate) fn transition_job(
    database: &Database,
    mut request: TransitionMediaJobRequest,
) -> Result<MediaJobSnapshot, CoreError> {
    request.job_id = required(&request.job_id, "job_id", 128)?.to_string();

    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = load_job_record(&tx, &request.job_id)?;
    check_revision(&current, request.expected_revision)?;
    if matches!(
        current.state,
        MediaJobState::Submitting
            | MediaJobState::Queued
            | MediaJobState::Running
            | MediaJobState::ProviderUnknown
    ) {
        return Err(CoreError::Conflict(
            "Provider-owned media job states may only advance through a bound provider event"
                .to_string(),
        ));
    }
    if !current.state.can_transition_to(request.next_state) {
        return Err(CoreError::Conflict(format!(
            "Media job {} cannot transition from {} to {}",
            request.job_id,
            current.state.as_str(),
            request.next_state.as_str()
        )));
    }
    if request.next_state == MediaJobState::Completed {
        let attempt_id = current.current_attempt_id.as_deref().ok_or_else(|| {
            CoreError::Conflict("A completed media job requires a current attempt".to_string())
        })?;
        let attempt = load_attempt(&tx, attempt_id)?;
        if attempt.state != super::model::MediaJobAttemptState::Succeeded {
            return Err(CoreError::Conflict(
                "Post-processing can complete only after the current attempt succeeded".to_string(),
            ));
        }
        let output_count = tx.query_row(
            "SELECT COUNT(*) FROM media_asset_relations
             WHERE job_id = ?1 AND attempt_id = ?2 AND relation_type = 'output'",
            rusqlite::params![request.job_id, attempt_id],
            |row| row.get::<_, u64>(0),
        )?;
        if output_count == 0 {
            return Err(CoreError::Conflict(
                "Post-processing can complete only after an output asset is linked".to_string(),
            ));
        }
    }
    if requires_provider_task_identity(request.next_state)
        && current.current_provider_task_id.as_deref().is_none()
    {
        return Err(CoreError::InvalidInput(format!(
            "Media job state {} requires a persisted provider_task_id",
            request.next_state.as_str()
        )));
    }
    tx.execute(
        "UPDATE media_jobs
         SET state = ?2,
             revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
             completed_at = CASE WHEN ?3 = 1 THEN COALESCE(completed_at, strftime('%Y-%m-%dT%H:%M:%fZ','now')) ELSE completed_at END
         WHERE id = ?1",
        rusqlite::params![
            request.job_id,
            request.next_state.as_str(),
            bool_to_i64(request.next_state.is_terminal()),
        ],
    )?;
    let snapshot = load_snapshot(&tx, &request.job_id)?;
    tx.commit()?;
    Ok(snapshot)
}

pub(crate) fn request_cancellation(
    database: &Database,
    mut request: RequestMediaJobCancellation,
) -> Result<MediaJobSnapshot, CoreError> {
    request.job_id = required(&request.job_id, "job_id", 128)?.to_string();
    request.reason = required(&request.reason, "reason", 512)?.to_string();
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = load_job_record(&tx, &request.job_id)?;
    if current.state == MediaJobState::Cancelled
        || (current.cancellation_requested_at.is_some()
            && current.cancellation_reason.as_deref() == Some(request.reason.as_str()))
    {
        return load_snapshot(&tx, &request.job_id);
    }
    check_revision(&current, request.expected_revision)?;
    if current.state.is_terminal() {
        return Err(CoreError::Conflict(format!(
            "Cannot request cancellation for terminal media job {} in state {}",
            request.job_id,
            current.state.as_str()
        )));
    }
    if current.cancellation_requested_at.is_some() {
        return Err(CoreError::Conflict(format!(
            "Media job {} already has a different cancellation request",
            request.job_id
        )));
    }
    tx.execute(
        "UPDATE media_jobs
         SET cancellation_requested_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'), cancellation_reason = ?2,
             revision = revision + 1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        rusqlite::params![request.job_id, request.reason],
    )?;
    if let Some(attempt_id) = current.current_attempt_id.as_deref() {
        tx.execute(
            "UPDATE media_job_attempts
             SET cancellation_requested_at = COALESCE(cancellation_requested_at, strftime('%Y-%m-%dT%H:%M:%fZ','now')),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?1",
            [attempt_id],
        )?;
    }
    let snapshot = load_snapshot(&tx, &request.job_id)?;
    tx.commit()?;
    Ok(snapshot)
}

pub(crate) fn request_remote_deletion(
    database: &Database,
    mut request: RequestMediaJobRemoteDeletion,
) -> Result<MediaJobSnapshot, CoreError> {
    request.job_id = required(&request.job_id, "job_id", 128)?.to_string();
    request.attempt_id = required(&request.attempt_id, "attempt_id", 128)?.to_string();
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = load_job_record(&tx, &request.job_id)?;
    let attempt = load_attempt(&tx, &request.attempt_id)?;
    if attempt.job_id != request.job_id {
        return Err(CoreError::InvalidInput(
            "Remote deletion attempt does not belong to the media job".to_string(),
        ));
    }
    if attempt.remote_deletion_status == MediaRemoteDeletionStatus::Requested {
        return load_snapshot(&tx, &request.job_id);
    }
    check_revision(&current, request.expected_revision)?;
    if attempt.provider_task_id.is_none() {
        return Err(CoreError::Conflict(
            "Remote deletion requires a persisted provider task ID for that attempt".to_string(),
        ));
    }
    if current.current_attempt_id.as_deref() == Some(request.attempt_id.as_str())
        && !current.state.is_terminal()
        && current.cancellation_requested_at.is_none()
    {
        return Err(CoreError::Conflict(
            "An active current attempt must persist cancellation intent before remote deletion"
                .to_string(),
        ));
    }
    tx.execute(
        "UPDATE media_job_attempts
         SET remote_deletion_status = 'requested',
             remote_deletion_requested_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
             remote_deletion_completed_at = NULL, remote_deletion_error_json = NULL,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        [&request.attempt_id],
    )?;
    tx.execute(
        "UPDATE media_jobs SET revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?1",
        [&request.job_id],
    )?;
    let snapshot = load_snapshot(&tx, &request.job_id)?;
    tx.commit()?;
    Ok(snapshot)
}

pub(crate) fn record_remote_deletion_result(
    database: &Database,
    mut request: RecordMediaJobRemoteDeletionResult,
) -> Result<MediaJobSnapshot, CoreError> {
    request.job_id = required(&request.job_id, "job_id", 128)?.to_string();
    request.attempt_id = required(&request.attempt_id, "attempt_id", 128)?.to_string();
    request.event_source = normalize_provider_source(&request.event_source)?;
    request.deduplication_key =
        required(&request.deduplication_key, "deduplication_key", 512)?.to_string();
    request.error = optional(request.error, "error", 2048)?;
    if !matches!(
        request.status,
        MediaRemoteDeletionStatus::Confirmed
            | MediaRemoteDeletionStatus::Unsupported
            | MediaRemoteDeletionStatus::Failed
    ) {
        return Err(CoreError::InvalidInput(
            "Remote deletion result must be confirmed, unsupported, or failed".to_string(),
        ));
    }
    let payload = sanitize_persisted_json(&json!({
        "status": request.status.as_str(),
        "error": request.error,
    }));
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(existing) =
        load_event_by_identity(&tx, &request.event_source, &request.deduplication_key)?
    {
        if existing.job_id == request.job_id
            && existing.attempt_id == request.attempt_id
            && existing.event_kind == "remote_deletion.result"
            && existing.payload == payload
        {
            return load_snapshot(&tx, &request.job_id);
        }
        return Err(CoreError::Conflict(
            "Remote deletion event identity was already used for another occurrence".to_string(),
        ));
    }
    let current = load_job_record(&tx, &request.job_id)?;
    check_revision(&current, request.expected_revision)?;
    let attempt = load_attempt(&tx, &request.attempt_id)?;
    if attempt.job_id != request.job_id {
        return Err(CoreError::Conflict(
            "Remote deletion result attempt does not belong to the media job".to_string(),
        ));
    }
    if attempt.remote_deletion_status != MediaRemoteDeletionStatus::Requested {
        return Err(CoreError::Conflict(
            "Remote deletion has not been requested for this attempt".to_string(),
        ));
    }
    if attempt.provider_source != request.event_source {
        return Err(CoreError::Conflict(
            "Remote deletion result source does not match the current attempt".to_string(),
        ));
    }
    insert_internal_event(
        &tx,
        InternalEvent {
            job_id: &request.job_id,
            attempt_id: &request.attempt_id,
            provider_id: &attempt.provider_id,
            event_source: &request.event_source,
            deduplication_key: &request.deduplication_key,
            event_kind: "remote_deletion.result",
            payload: &payload,
        },
    )?;
    let error_json = payload
        .get("error")
        .filter(|value| !value.is_null())
        .map(|value| serde_json::to_string(&json!({ "message": value })))
        .transpose()?;
    tx.execute(
        "UPDATE media_job_attempts
         SET remote_deletion_status = ?2,
             remote_deletion_completed_at = CASE WHEN ?2 = 'confirmed'
                 THEN strftime('%Y-%m-%dT%H:%M:%fZ','now') ELSE NULL END,
             remote_deletion_error_json = ?3,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        rusqlite::params![request.attempt_id, request.status.as_str(), error_json],
    )?;
    tx.execute(
        "UPDATE media_jobs SET revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?1",
        [&request.job_id],
    )?;
    let snapshot = load_snapshot(&tx, &request.job_id)?;
    tx.commit()?;
    Ok(snapshot)
}

pub(crate) fn prepare_asset_deletion(
    database: &Database,
    request: RequestMediaAssetDeletion,
) -> Result<MediaAssetRecord, CoreError> {
    let asset_id = normalize_sha256(&request.asset_id)?;
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let asset = load_asset(&tx, &asset_id)?;
    if asset.local_state == super::model::MediaAssetLocalState::Deleted
        || asset.local_state == super::model::MediaAssetLocalState::DeletionRequested
    {
        return Ok(asset);
    }
    let references = tx.query_row(
        "SELECT
             (SELECT COUNT(*) FROM media_asset_relations WHERE asset_id = ?1 OR parent_asset_id = ?1)
           + (SELECT COUNT(*) FROM media_exports WHERE asset_id = ?1)
           + (SELECT COUNT(*) FROM video_timeline_clips WHERE asset_id = ?1)
           + (SELECT COUNT(*) FROM video_timeline_export_inputs WHERE asset_id = ?1)
           + (SELECT COUNT(*) FROM video_timeline_exports WHERE output_asset_id = ?1)
           + (SELECT COUNT(*) FROM video_timeline_export_stages WHERE intermediate_asset_id = ?1)
           + (SELECT COUNT(*) FROM media_jobs, json_each(media_jobs.input_asset_ids_json)
              WHERE json_each.value = ?1
                AND media_jobs.state NOT IN ('completed', 'failed', 'cancelled', 'expired'))",
        [&asset_id],
        |row| row.get::<_, u64>(0),
    )?;
    if references > 0 {
        return Err(CoreError::Conflict(format!(
            "Media asset {asset_id} is still referenced by {references} lineage/export occurrence(s)"
        )));
    }
    tx.execute(
        "UPDATE media_assets
         SET local_state = 'deletion_requested',
             local_deletion_requested_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        [&asset_id],
    )?;
    let pending = load_asset(&tx, &asset_id)?;
    tx.commit()?;
    Ok(pending)
}

pub(crate) fn confirm_asset_deleted(
    database: &Database,
    asset_id: &str,
) -> Result<MediaAssetRecord, CoreError> {
    let asset_id = normalize_sha256(asset_id)?;
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = load_asset(&tx, &asset_id)?;
    if current.local_state == super::model::MediaAssetLocalState::Deleted {
        return Ok(current);
    }
    if current.local_state != super::model::MediaAssetLocalState::DeletionRequested {
        return Err(CoreError::Conflict(
            "Media asset deletion was not durably requested".to_string(),
        ));
    }
    tx.execute(
        "UPDATE media_assets
         SET local_state = 'deleted',
             local_deletion_completed_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        [&asset_id],
    )?;
    let deleted = load_asset(&tx, &asset_id)?;
    tx.commit()?;
    Ok(deleted)
}

pub(crate) fn list_pending_asset_deletions(
    database: &Database,
) -> Result<Vec<MediaAssetRecord>, CoreError> {
    let conn = database.conn();
    let mut statement = conn.prepare(&format!(
        "SELECT {ASSET_PROJECTION} FROM media_assets
         WHERE local_state = 'deletion_requested' ORDER BY updated_at ASC, id ASC"
    ))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|value| serde_json::from_str(&value).map_err(CoreError::from))
        .collect()
}

pub(crate) fn list_registered_asset_storage_keys(
    database: &Database,
) -> Result<Vec<String>, CoreError> {
    let conn = database.conn();
    let mut statement = conn.prepare(
        "SELECT storage_key FROM media_assets WHERE local_state <> 'deleted' ORDER BY storage_key",
    )?;
    let storage_keys = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(CoreError::from)?;
    Ok(storage_keys)
}

pub(crate) struct BeginAttemptClaim {
    pub snapshot: MediaJobSnapshot,
    pub claimed: bool,
}

pub(crate) fn begin_attempt(
    database: &Database,
    request: BeginMediaJobAttemptRequest,
) -> Result<MediaJobSnapshot, CoreError> {
    Ok(begin_attempt_claim(database, request)?.snapshot)
}

pub(crate) fn begin_attempt_claim(
    database: &Database,
    mut request: BeginMediaJobAttemptRequest,
) -> Result<BeginAttemptClaim, CoreError> {
    request.job_id = required(&request.job_id, "job_id", 128)?.to_string();
    request.idempotency_key =
        required(&request.idempotency_key, "idempotency_key", 256)?.to_string();
    request.provider_id = required(&request.provider_id, "provider_id", 128)?.to_string();
    request.provider_source = normalize_provider_source(&request.provider_source)?;
    request.model_id = required(&request.model_id, "model_id", 256)?.to_string();
    request.api_version = optional(request.api_version, "api_version", 128)?;
    request.data_region = optional(request.data_region, "data_region", 128)?;
    request.remote_retention_expires_at = optional_timestamp(
        request.remote_retention_expires_at,
        "remote_retention_expires_at",
    )?;
    if let Some(reconciliation) = request.provider_unknown_reconciliation.as_mut() {
        reconciliation.observed_at = optional_timestamp(
            Some(reconciliation.observed_at.clone()),
            "provider_unknown_reconciliation.observed_at",
        )?
        .ok_or_else(|| {
            CoreError::InvalidInput("Reconciliation observed_at cannot be empty".to_string())
        })?;
        reconciliation.lookup_source = normalize_provider_source(&reconciliation.lookup_source)?;
        reconciliation.lookup_idempotency_key = required(
            &reconciliation.lookup_idempotency_key,
            "provider_unknown_reconciliation.lookup_idempotency_key",
            256,
        )?
        .to_string();
        ensure_object(
            &reconciliation.lookup_evidence,
            "provider_unknown_reconciliation.lookup_evidence",
        )?;
        reconciliation.lookup_evidence = sanitize_persisted_json(&reconciliation.lookup_evidence);
        ensure_json_size(
            &reconciliation.lookup_evidence,
            "provider_unknown_reconciliation.lookup_evidence",
            MAX_PROVIDER_EVENT_PAYLOAD_BYTES,
        )?;
    }

    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(existing) =
        load_attempt_by_idempotency(&tx, &request.job_id, &request.idempotency_key)?
    {
        if existing.provider_id != request.provider_id
            || existing.provider_source != request.provider_source
            || existing.model_id != request.model_id
            || existing.api_version != request.api_version
            || existing.data_region != request.data_region
            || existing.remote_retention_expires_at != request.remote_retention_expires_at
        {
            return Err(CoreError::Conflict(format!(
                "Media attempt idempotency key `{}` was already used for different input",
                request.idempotency_key
            )));
        }
        return Ok(BeginAttemptClaim {
            snapshot: load_snapshot(&tx, &request.job_id)?,
            claimed: false,
        });
    }

    let current = load_job_record(&tx, &request.job_id)?;
    check_revision(&current, request.expected_revision)?;
    if !matches!(
        current.state,
        MediaJobState::Submitting | MediaJobState::ProviderUnknown
    ) {
        return Err(CoreError::Conflict(format!(
            "Media job {} must be submitting before an attempt begins",
            request.job_id
        )));
    }
    if current.state == MediaJobState::Submitting {
        if let Some(attempt_id) = current.current_attempt_id.as_deref() {
            let attempt = load_attempt(&tx, attempt_id)?;
            if !attempt.state.is_terminal() {
                return Err(CoreError::Conflict(format!(
                    "Media job {} already has active attempt {}",
                    request.job_id, attempt_id
                )));
            }
            if attempt.state != super::model::MediaJobAttemptState::Failed
                || attempt.retry_classification.is_none()
            {
                return Err(CoreError::Conflict(
                    "A retry after a terminal attempt requires persisted retry classification"
                        .to_string(),
                ));
            }
            if attempt
                .next_eligible_at
                .as_deref()
                .is_some_and(|timestamp| {
                    DateTime::parse_from_rfc3339(timestamp)
                        .map(|eligible| eligible.with_timezone(&Utc) > Utc::now())
                        .unwrap_or(true)
                })
            {
                return Err(CoreError::Conflict(
                    "The media attempt is not yet eligible for retry".to_string(),
                ));
            }
        }
    }
    let prior_unknown_attempt = if current.state == MediaJobState::ProviderUnknown {
        let reconciliation = request
            .provider_unknown_reconciliation
            .as_ref()
            .ok_or_else(|| {
                CoreError::InvalidInput(
                    "A provider_unknown job requires durable idempotency-lookup evidence before resubmission"
                        .to_string(),
                )
            })?;
        let attempt_id = current.current_attempt_id.as_deref().ok_or_else(|| {
            CoreError::Conflict(
                "A provider_unknown job has no current attempt to reconcile".to_string(),
            )
        })?;
        let attempt = load_attempt(&tx, attempt_id)?;
        if attempt.state != super::model::MediaJobAttemptState::ProviderUnknown {
            return Err(CoreError::Conflict(
                "The current attempt is not awaiting provider reconciliation".to_string(),
            ));
        }
        if reconciliation.lookup_source != attempt.provider_source
            || reconciliation.lookup_idempotency_key != attempt.idempotency_key
        {
            return Err(CoreError::Conflict(
                "Reconciliation evidence must target the unknown attempt's exact source and idempotency key"
                    .to_string(),
            ));
        }
        Some((attempt, reconciliation.clone()))
    } else {
        if request.provider_unknown_reconciliation.is_some() {
            return Err(CoreError::InvalidInput(
                "Reconciliation evidence is accepted only for provider_unknown jobs".to_string(),
            ));
        }
        None
    };
    let changes_provider_boundary = current.provider_id != request.provider_id
        || current.provider_source != request.provider_source
        || current.data_region != request.data_region;
    if changes_provider_boundary && !current.allow_cross_provider_fallback {
        return Err(CoreError::InvalidInput(format!(
            "Media job {} does not allow cross-provider fallback",
            request.job_id
        )));
    }
    let attempt_number = u32::try_from(current.attempts_started(&tx)? + 1)
        .map_err(|_| CoreError::Internal("Media attempt counter overflowed".to_string()))?;
    if attempt_number > current.max_attempts {
        return Err(CoreError::Conflict(format!(
            "Media job {} has exhausted its {} attempts",
            request.job_id, current.max_attempts
        )));
    }

    let attempt_id = Uuid::new_v4().to_string();
    if let Some((prior_attempt, reconciliation)) = prior_unknown_attempt {
        let evidence = json!({
            "outcome": "idempotency_lookup_confirmed_absent",
            "observedAt": reconciliation.observed_at,
            "lookupSource": reconciliation.lookup_source,
            "lookupIdempotencyKey": reconciliation.lookup_idempotency_key,
            "lookupEvidence": reconciliation.lookup_evidence,
            "replacementAttemptId": attempt_id,
        });
        tx.execute(
            "UPDATE media_job_attempts
             SET state = 'failed', error_json = ?2, retry_classification = 'reconciled_not_found',
                 completed_at = COALESCE(completed_at, strftime('%Y-%m-%dT%H:%M:%fZ','now')),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?1",
            rusqlite::params![prior_attempt.id, serde_json::to_string(&evidence)?],
        )?;
        insert_internal_event(
            &tx,
            InternalEvent {
                job_id: &request.job_id,
                attempt_id: &prior_attempt.id,
                provider_id: &prior_attempt.provider_id,
                event_source: &prior_attempt.provider_source,
                deduplication_key: &format!("reconciliation:{}", request.idempotency_key),
                event_kind: "reconciliation.idempotency_lookup_confirmed_absent",
                payload: &evidence,
            },
        )?;
    }
    tx.execute(
        "INSERT INTO media_job_attempts (
             id, job_id, attempt_number, idempotency_key, provider_id, provider_source,
             model_id, api_version, data_region, remote_retention_expires_at,
             cross_provider_fallback_authorized, state
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'created')",
        rusqlite::params![
            attempt_id,
            request.job_id,
            i64::from(attempt_number),
            request.idempotency_key,
            request.provider_id,
            request.provider_source,
            request.model_id,
            request.api_version,
            request.data_region,
            request.remote_retention_expires_at,
            bool_to_i64(changes_provider_boundary),
        ],
    )?;
    for (ordinal, asset_id) in current.input_asset_ids.iter().enumerate() {
        let input_asset = load_asset(&tx, asset_id)?;
        if input_asset.local_state != super::model::MediaAssetLocalState::Available {
            return Err(CoreError::Conflict(format!(
                "Input media asset {asset_id} is no longer locally available"
            )));
        }
        let relation_id = stable_id(
            "media-attempt-input",
            &format!(
                "{}\0{}\0{}\0{}",
                request.job_id, attempt_id, ordinal, asset_id
            ),
        );
        let metadata = json!({
            "providerId": request.provider_id.clone(),
            "providerSource": request.provider_source.clone(),
            "modelId": request.model_id.clone(),
            "apiVersion": request.api_version.clone(),
            "dataRegion": request.data_region.clone(),
        });
        tx.execute(
            "INSERT INTO media_asset_relations (
                 id, job_id, attempt_id, asset_id, relation_type, ordinal, metadata_json
             ) VALUES (?1, ?2, ?3, ?4, 'input', ?5, ?6)",
            rusqlite::params![
                relation_id,
                request.job_id,
                attempt_id,
                asset_id,
                i64::try_from(ordinal).map_err(|_| CoreError::Internal(
                    "Input asset ordinal overflowed".to_string()
                ))?,
                serde_json::to_string(&metadata)?,
            ],
        )?;
    }
    tx.execute(
        "UPDATE media_jobs
         SET current_attempt_id = ?2, provider_id = ?3, provider_source = ?4,
             model_id = ?5, api_version = ?6, data_region = ?7,
             remote_retention_expires_at = ?8, retry_count = ?9,
             state = 'submitting', current_provider_task_id = NULL,
             revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        rusqlite::params![
            request.job_id,
            attempt_id,
            request.provider_id,
            request.provider_source,
            request.model_id,
            request.api_version,
            request.data_region,
            request.remote_retention_expires_at,
            i64::from(attempt_number.saturating_sub(1)),
        ],
    )?;
    let snapshot = load_snapshot(&tx, &request.job_id)?;
    tx.commit()?;
    Ok(BeginAttemptClaim {
        snapshot,
        claimed: true,
    })
}

pub(crate) fn record_provider_event(
    database: &Database,
    mut request: RecordMediaProviderEventRequest,
) -> Result<MediaJobSnapshot, CoreError> {
    normalize_provider_event(&mut request)?;
    let payload_json = serde_json::to_string(&request.payload)?;
    let error_json = request
        .error
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let cancellation_result_json = request
        .cancellation_result
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let provenance_json = request
        .provenance
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    if let Some(existing) =
        load_event_by_identity(&tx, &request.event_source, &request.deduplication_key)?
    {
        if existing.job_id != request.job_id
            || existing.attempt_id != request.attempt_id
            || existing.provider_id != request.provider_id
            || existing.event_kind != request.event_kind
            || existing.payload != request.payload
        {
            return Err(CoreError::Conflict(format!(
                "Provider event ({}, {}) was already recorded with different data",
                request.event_source, request.deduplication_key
            )));
        }
        return load_snapshot(&tx, &request.job_id);
    }

    let current = load_job_record(&tx, &request.job_id)?;
    check_revision(&current, request.expected_revision)?;
    if current.state.is_terminal() {
        return Err(CoreError::Conflict(format!(
            "Terminal media job {} is immutable",
            request.job_id
        )));
    }
    if current.current_attempt_id.as_deref() != Some(request.attempt_id.as_str()) {
        return Err(CoreError::Conflict(
            "Provider events are accepted only for the job's current attempt".to_string(),
        ));
    }
    let attempt = load_attempt(&tx, &request.attempt_id)?;
    if attempt.job_id != request.job_id
        || attempt.provider_id != request.provider_id
        || attempt.provider_source != request.event_source
        || current.provider_id != attempt.provider_id
        || current.provider_source != attempt.provider_source
    {
        return Err(CoreError::InvalidInput(
            "Provider event identity does not match the bound current attempt".to_string(),
        ));
    }
    if let (Some(job_task), Some(attempt_task)) = (
        current.current_provider_task_id.as_deref(),
        attempt.provider_task_id.as_deref(),
    ) {
        if job_task != attempt_task {
            return Err(CoreError::Conflict(
                "Persisted media job and attempt provider task IDs diverged".to_string(),
            ));
        }
    }
    for existing_task in [
        current.current_provider_task_id.as_deref(),
        attempt.provider_task_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if request
            .provider_task_id
            .as_deref()
            .is_some_and(|requested| requested != existing_task)
        {
            return Err(CoreError::Conflict(
                "A provider task ID is immutable within a media attempt".to_string(),
            ));
        }
    }
    let provider_task_id = request
        .provider_task_id
        .as_deref()
        .or(attempt.provider_task_id.as_deref())
        .or(current.current_provider_task_id.as_deref());
    if let Some(next) = request.next_job_state {
        if !current.state.can_transition_to(next) {
            return Err(CoreError::Conflict(format!(
                "Provider event cannot transition media job {} from {} to {}",
                request.job_id,
                current.state.as_str(),
                next.as_str()
            )));
        }
        let required_attempt_state = provider_projected_attempt_state(next).ok_or_else(|| {
            CoreError::InvalidInput(format!(
                "Provider events cannot directly project media job state {}",
                next.as_str()
            ))
        })?;
        if request.attempt_state != Some(required_attempt_state) {
            return Err(CoreError::InvalidInput(format!(
                "Provider transition to {} requires attempt state {} in the same event",
                next.as_str(),
                required_attempt_state.as_str()
            )));
        }
        if next == MediaJobState::Submitting && request.retry_classification.is_none() {
            return Err(CoreError::InvalidInput(
                "A transient provider failure must include retry_classification".to_string(),
            ));
        }
        if requires_provider_task_identity(next) && provider_task_id.is_none() {
            return Err(CoreError::InvalidInput(format!(
                "Media job state {} requires a persisted provider_task_id",
                next.as_str()
            )));
        }
    }
    if let Some(next) = request.attempt_state {
        let clear_attention = error_json.is_none()
            && matches!(
                next,
                super::model::MediaJobAttemptState::Accepted
                    | super::model::MediaJobAttemptState::Observing
                    | super::model::MediaJobAttemptState::Succeeded
            );
        if request.next_job_state.is_none()
            && next != super::model::MediaJobAttemptState::Submitting
            && !(current.state == MediaJobState::Submitting
                && next == super::model::MediaJobAttemptState::Failed
                && request.retry_classification.is_some())
        {
            return Err(CoreError::InvalidInput(
                "Attempt progress must be projected to the matching job state in the same event"
                    .to_string(),
            ));
        }
        if !attempt.state.can_transition_to(next) {
            return Err(CoreError::Conflict(format!(
                "Media attempt {} cannot transition from {} to {}",
                request.attempt_id,
                attempt.state.as_str(),
                next.as_str()
            )));
        }
        ensure_provider_event_budget(&tx, &request.job_id, payload_json.len(), false)?;
        tx.execute(
            "UPDATE media_job_attempts
             SET state = ?2,
                 provider_task_id = COALESCE(?3, provider_task_id),
                 error_json = CASE WHEN ?9 = 1 THEN NULL ELSE COALESCE(?4, error_json) END,
                 retry_classification = CASE WHEN ?9 = 1 THEN NULL ELSE COALESCE(?5, retry_classification) END,
                 next_eligible_at = CASE WHEN ?9 = 1 THEN NULL ELSE COALESCE(?6, next_eligible_at) END,
                 cancellation_result_json = COALESCE(?7, cancellation_result_json),
                 submitted_at = CASE WHEN ?2 IN ('submitting', 'accepted', 'observing')
                     THEN COALESCE(submitted_at, strftime('%Y-%m-%dT%H:%M:%fZ','now')) ELSE submitted_at END,
                 last_observed_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
                 completed_at = CASE WHEN ?8 = 1 THEN COALESCE(completed_at, strftime('%Y-%m-%dT%H:%M:%fZ','now')) ELSE completed_at END,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?1",
            rusqlite::params![
                request.attempt_id,
                next.as_str(),
                provider_task_id,
                error_json,
                request.retry_classification,
                request.next_eligible_at,
                cancellation_result_json,
                bool_to_i64(next.is_terminal()),
                bool_to_i64(clear_attention),
            ],
        )?;
    } else {
        ensure_provider_event_budget(&tx, &request.job_id, payload_json.len(), false)?;
        tx.execute(
            "UPDATE media_job_attempts
             SET provider_task_id = COALESCE(?2, provider_task_id),
                 error_json = COALESCE(?3, error_json),
                 retry_classification = COALESCE(?4, retry_classification),
                 next_eligible_at = COALESCE(?5, next_eligible_at),
                 cancellation_result_json = COALESCE(?6, cancellation_result_json),
                 last_observed_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?1",
            rusqlite::params![
                request.attempt_id,
                provider_task_id,
                error_json,
                request.retry_classification,
                request.next_eligible_at,
                cancellation_result_json,
            ],
        )?;
    }

    let event_id = Uuid::new_v4().to_string();
    let event_sequence = tx.query_row(
        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM media_provider_events WHERE job_id = ?1",
        [&request.job_id],
        |row| row.get::<_, i64>(0),
    )?;
    tx.execute(
        "INSERT INTO media_provider_events (
             id, job_id, attempt_id, sequence, provider_id, event_source,
             deduplication_key, event_kind, payload_json, provider_created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            event_id,
            request.job_id,
            request.attempt_id,
            event_sequence,
            request.provider_id,
            request.event_source,
            request.deduplication_key,
            request.event_kind,
            payload_json,
            request.provider_created_at,
        ],
    )?;

    let next_state = request.next_job_state.unwrap_or(current.state);
    tx.execute(
        "UPDATE media_jobs
         SET state = ?2,
             revision = revision + 1,
             current_provider_task_id = CASE WHEN ?2 = 'submitting'
                 THEN NULL ELSE COALESCE(?3, current_provider_task_id) END,
             final_cost_micros = COALESCE(?4, final_cost_micros),
             watermark_present = COALESCE(?5, watermark_present),
             provenance_json = COALESCE(?6, provenance_json),
             last_provider_observed_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
             completed_at = CASE WHEN ?7 = 1 THEN COALESCE(completed_at, strftime('%Y-%m-%dT%H:%M:%fZ','now')) ELSE completed_at END
         WHERE id = ?1",
        rusqlite::params![
            request.job_id,
            next_state.as_str(),
            provider_task_id,
            request.final_cost_micros,
            request.watermark_present.map(bool_to_i64),
            provenance_json,
            bool_to_i64(next_state.is_terminal()),
        ],
    )?;
    let snapshot = load_snapshot(&tx, &request.job_id)?;
    tx.commit()?;
    Ok(snapshot)
}

pub(crate) fn register_asset(
    database: &Database,
    mut request: RegisterMediaAssetRequest,
) -> Result<MediaAssetRecord, CoreError> {
    normalize_asset(&mut request)?;
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(existing) = load_asset_optional(&tx, &request.content_hash_sha256)? {
        if existing.media_type != request.media_type
            || existing.byte_length != request.byte_length
            || existing.storage_kind != request.storage_kind
            || existing.storage_key != request.storage_key
            || existing.width != request.width
            || existing.height != request.height
            || existing.duration_ms != request.duration_ms
        {
            return Err(CoreError::Conflict(format!(
                "Verified media asset {} was already registered with different metadata",
                request.content_hash_sha256
            )));
        }
        if existing.local_state == super::model::MediaAssetLocalState::DeletionRequested {
            return Err(CoreError::Conflict(format!(
                "Verified media asset {} has deletion pending",
                request.content_hash_sha256
            )));
        }
        if existing.local_state == super::model::MediaAssetLocalState::Deleted {
            tx.execute(
                "UPDATE media_assets
                 SET local_state = 'available', local_deletion_requested_at = NULL,
                     local_deletion_completed_at = NULL, content_verified_at = ?2,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                 WHERE id = ?1",
                rusqlite::params![request.content_hash_sha256, request.content_verified_at],
            )?;
            let restored = load_asset(&tx, &request.content_hash_sha256)?;
            tx.commit()?;
            return Ok(restored);
        }
        return Ok(existing);
    }
    tx.execute(
        "INSERT INTO media_assets (
             id, content_hash_sha256, content_verified_at, media_type, byte_length,
             storage_kind, storage_key, width, height, duration_ms
         ) VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            request.content_hash_sha256,
            request.content_verified_at,
            request.media_type,
            u64_to_i64(request.byte_length, "byte_length")?,
            request.storage_kind.as_str(),
            request.storage_key,
            request.width.map(i64::from),
            request.height.map(i64::from),
            request
                .duration_ms
                .map(|value| u64_to_i64(value, "duration_ms"))
                .transpose()?,
        ],
    )?;
    let asset = load_asset(&tx, &request.content_hash_sha256)?;
    tx.commit()?;
    Ok(asset)
}

pub(crate) fn link_asset(
    database: &Database,
    mut request: LinkMediaAssetRequest,
) -> Result<MediaJobSnapshot, CoreError> {
    normalize_asset_link(&mut request)?;
    let relation_id = stable_id(
        "media-asset-relation",
        &format!("{}\0{}", request.job_id, request.idempotency_key),
    );
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let attempt = load_attempt(&tx, &request.attempt_id)?;
    if attempt.job_id != request.job_id {
        return Err(CoreError::InvalidInput(
            "Asset lineage attempt does not belong to the job".to_string(),
        ));
    }
    let metadata = request.metadata.as_object_mut().ok_or_else(|| {
        CoreError::InvalidInput("Asset relation metadata must be a JSON object".to_string())
    })?;
    metadata.insert("providerId".to_string(), json!(attempt.provider_id));
    metadata.insert("providerSource".to_string(), json!(attempt.provider_source));
    metadata.insert("modelId".to_string(), json!(attempt.model_id));
    metadata.insert("apiVersion".to_string(), json!(attempt.api_version));
    metadata.insert("dataRegion".to_string(), json!(attempt.data_region));

    if let Some(existing) = load_relation_optional(&tx, &relation_id)? {
        if existing.job_id != request.job_id
            || existing.attempt_id != request.attempt_id
            || existing.asset_id != request.asset_id
            || existing.parent_asset_id != request.parent_asset_id
            || existing.relation_type != request.relation_type
            || existing.ordinal != request.ordinal
            || existing.local_retention_policy != request.local_retention_policy
            || existing.local_retention_expires_at != request.local_retention_expires_at
            || existing.metadata != request.metadata
        {
            return Err(CoreError::Conflict(format!(
                "Media asset relation idempotency key `{}` was already used for different input",
                request.idempotency_key
            )));
        }
        return load_snapshot(&tx, &request.job_id);
    }

    let current = load_job_record(&tx, &request.job_id)?;
    check_revision(&current, request.expected_revision)?;
    let asset = load_asset(&tx, &request.asset_id)?;
    if asset.local_state != super::model::MediaAssetLocalState::Available {
        return Err(CoreError::Conflict(
            "Only locally available assets can be linked".to_string(),
        ));
    }
    if let Some(parent_asset_id) = request.parent_asset_id.as_deref() {
        let parent = load_asset(&tx, parent_asset_id)?;
        if parent.local_state != super::model::MediaAssetLocalState::Available {
            return Err(CoreError::Conflict(
                "Only locally available parent assets can be linked".to_string(),
            ));
        }
    }
    if current.current_attempt_id.as_deref() != Some(request.attempt_id.as_str()) {
        return Err(CoreError::Conflict(
            "Asset lineage may be appended only to the current attempt".to_string(),
        ));
    }
    let metadata_json = serde_json::to_string(&request.metadata)?;

    tx.execute(
        "INSERT INTO media_asset_relations (
             id, job_id, attempt_id, asset_id, parent_asset_id, relation_type,
             ordinal, local_retention_policy, local_retention_expires_at, metadata_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            relation_id,
            request.job_id,
            request.attempt_id,
            request.asset_id,
            request.parent_asset_id,
            request.relation_type.as_str(),
            i64::from(request.ordinal),
            request.local_retention_policy.as_str(),
            request.local_retention_expires_at,
            metadata_json,
        ],
    )?;
    tx.execute(
        "UPDATE media_jobs SET revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?1",
        [&request.job_id],
    )?;
    let snapshot = load_snapshot(&tx, &request.job_id)?;
    tx.commit()?;
    Ok(snapshot)
}

pub(crate) fn delete_asset_occurrence(
    database: &Database,
    mut request: DeleteMediaAssetOccurrenceRequest,
) -> Result<MediaJobSnapshot, CoreError> {
    request.job_id = required(&request.job_id, "job_id", 128)?.to_string();
    request.relation_id = required(&request.relation_id, "relation_id", 128)?.to_string();
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let job = load_job_record(&tx, &request.job_id)?;
    check_revision(&job, request.expected_revision)?;
    if !job.state.is_terminal() {
        return Err(CoreError::Conflict(
            "Asset occurrences can be removed only after the media job is terminal".to_string(),
        ));
    }
    let relation = load_relation_optional(&tx, &request.relation_id)?.ok_or_else(|| {
        CoreError::NotFound(format!("Media asset relation {}", request.relation_id))
    })?;
    if relation.job_id != request.job_id {
        return Err(CoreError::InvalidInput(
            "Media asset relation does not belong to the requested job".to_string(),
        ));
    }
    let selected_video_shot: Option<(String, String)> =
        if relation.relation_type == MediaAssetRelationType::Output {
            tx.query_row(
                "SELECT shots.id, shots.workflow_id
             FROM video_workflow_variants AS variants
             JOIN video_workflow_shots AS shots
               ON shots.id = variants.shot_id
              AND shots.selected_variant_id = variants.id
             WHERE variants.job_id = ?1",
                [&request.job_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
        } else {
            None
        };
    tx.execute(
        "DELETE FROM media_asset_relations WHERE id = ?1",
        [&request.relation_id],
    )?;
    if let Some((shot_id, workflow_id)) = selected_video_shot {
        tx.execute(
            "UPDATE video_workflow_shots
             SET selected_variant_id = NULL, revision = revision + 1,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?1",
            [&shot_id],
        )?;
        tx.execute(
            "UPDATE video_workflows
             SET revision = revision + 1,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?1",
            [&workflow_id],
        )?;
    }
    tx.execute(
        "UPDATE media_jobs SET revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?1",
        [&request.job_id],
    )?;
    let snapshot = load_snapshot(&tx, &request.job_id)?;
    tx.commit()?;
    Ok(snapshot)
}

fn load_snapshot(conn: &Connection, job_id: &str) -> Result<MediaJobSnapshot, CoreError> {
    let job = load_job_record(conn, job_id)?;
    let attempts = query_json_rows::<MediaJobAttemptRecord>(
        conn,
        &format!(
            "SELECT {ATTEMPT_PROJECTION} FROM media_job_attempts WHERE job_id = ?1 ORDER BY attempt_number ASC"
        ),
        job_id,
    )?;
    let assets = query_json_rows::<MediaAssetRecord>(
        conn,
        &format!(
            "SELECT {ASSET_PROJECTION} FROM media_assets
             WHERE id IN (SELECT asset_id FROM media_asset_relations WHERE job_id = ?1)
                OR id IN (SELECT parent_asset_id FROM media_asset_relations WHERE job_id = ?1)
             ORDER BY created_at ASC, id ASC"
        ),
        job_id,
    )?;
    let asset_relations = query_json_rows::<MediaAssetRelationRecord>(
        conn,
        &format!(
            "SELECT {RELATION_PROJECTION} FROM media_asset_relations WHERE job_id = ?1
             ORDER BY created_at ASC, relation_type ASC, ordinal ASC, id ASC"
        ),
        job_id,
    )?;
    let provider_event_count = conn.query_row(
        "SELECT COUNT(*) FROM media_provider_events WHERE job_id = ?1",
        [job_id],
        |row| row.get::<_, u64>(0),
    )?;
    let provider_events = query_json_rows::<MediaProviderEventRecord>(
        conn,
        &format!(
            "SELECT {EVENT_PROJECTION} FROM (
                 SELECT * FROM media_provider_events
                 WHERE job_id = ?1 ORDER BY sequence DESC LIMIT 100
             ) ORDER BY sequence ASC"
        ),
        job_id,
    )?;
    Ok(MediaJobSnapshot {
        job,
        attempts,
        assets,
        asset_relations,
        provider_event_count,
        provider_events,
    })
}

pub(crate) fn list_provider_events(
    database: &Database,
    job_id: &str,
    after_sequence: u64,
    limit: u32,
) -> Result<Vec<MediaProviderEventRecord>, CoreError> {
    let job_id = required(job_id, "job_id", 128)?;
    let limit = limit.clamp(1, 500);
    let after_sequence = u64_to_i64(after_sequence, "after_sequence")?;
    let conn = database.conn();
    load_job_record(&conn, job_id)?;
    let mut statement = conn.prepare(&format!(
        "SELECT {EVENT_PROJECTION} FROM media_provider_events
         WHERE job_id = ?1 AND sequence > ?2 ORDER BY sequence ASC LIMIT ?3"
    ))?;
    let rows = statement
        .query_map(
            rusqlite::params![job_id, after_sequence, i64::from(limit)],
            |row| row.get::<_, String>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|value| serde_json::from_str(&value).map_err(CoreError::from))
        .collect()
}

fn load_job_record(conn: &Connection, job_id: &str) -> Result<MediaJobRecord, CoreError> {
    query_json_optional(
        conn,
        &format!("SELECT {JOB_PROJECTION} FROM media_jobs WHERE id = ?1"),
        job_id,
    )?
    .ok_or_else(|| CoreError::NotFound(format!("Media job {job_id}")))
}

fn load_attempt(conn: &Connection, attempt_id: &str) -> Result<MediaJobAttemptRecord, CoreError> {
    query_json_optional(
        conn,
        &format!("SELECT {ATTEMPT_PROJECTION} FROM media_job_attempts WHERE id = ?1"),
        attempt_id,
    )?
    .ok_or_else(|| CoreError::NotFound(format!("Media job attempt {attempt_id}")))
}

fn load_attempt_by_idempotency(
    conn: &Connection,
    job_id: &str,
    idempotency_key: &str,
) -> Result<Option<MediaJobAttemptRecord>, CoreError> {
    let json = conn
        .query_row(
            &format!(
                "SELECT {ATTEMPT_PROJECTION} FROM media_job_attempts WHERE job_id = ?1 AND idempotency_key = ?2"
            ),
            rusqlite::params![job_id, idempotency_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    json.map(|value| serde_json::from_str(&value).map_err(CoreError::from))
        .transpose()
}

fn load_pending_remote_deletion_attempt(
    conn: &Connection,
    job_id: &str,
) -> Result<Option<MediaJobAttemptRecord>, CoreError> {
    let json = conn
        .query_row(
            &format!(
                "SELECT {ATTEMPT_PROJECTION} FROM media_job_attempts
                 WHERE job_id = ?1 AND remote_deletion_status = 'requested'
                 ORDER BY attempt_number ASC LIMIT 1"
            ),
            [job_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    json.map(|value| serde_json::from_str(&value).map_err(CoreError::from))
        .transpose()
}

fn load_asset(conn: &Connection, asset_id: &str) -> Result<MediaAssetRecord, CoreError> {
    load_asset_optional(conn, asset_id)?
        .ok_or_else(|| CoreError::NotFound(format!("Verified media asset {asset_id}")))
}

fn load_asset_optional(
    conn: &Connection,
    asset_id: &str,
) -> Result<Option<MediaAssetRecord>, CoreError> {
    query_json_optional(
        conn,
        &format!("SELECT {ASSET_PROJECTION} FROM media_assets WHERE id = ?1"),
        asset_id,
    )
}

fn load_relation_optional(
    conn: &Connection,
    relation_id: &str,
) -> Result<Option<MediaAssetRelationRecord>, CoreError> {
    query_json_optional(
        conn,
        &format!("SELECT {RELATION_PROJECTION} FROM media_asset_relations WHERE id = ?1"),
        relation_id,
    )
}

fn load_event_by_identity(
    conn: &Connection,
    event_source: &str,
    deduplication_key: &str,
) -> Result<Option<MediaProviderEventRecord>, CoreError> {
    let json = conn
        .query_row(
            &format!(
                "SELECT {EVENT_PROJECTION} FROM media_provider_events WHERE event_source = ?1 AND deduplication_key = ?2"
            ),
            rusqlite::params![event_source, deduplication_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    json.map(|value| serde_json::from_str(&value).map_err(CoreError::from))
        .transpose()
}

fn query_json_optional<T: DeserializeOwned>(
    conn: &Connection,
    sql: &str,
    parameter: &str,
) -> Result<Option<T>, CoreError> {
    let json = conn
        .query_row(sql, [parameter], |row| row.get::<_, String>(0))
        .optional()?;
    json.map(|value| serde_json::from_str(&value).map_err(CoreError::from))
        .transpose()
}

fn query_json_rows<T: DeserializeOwned>(
    conn: &Connection,
    sql: &str,
    parameter: &str,
) -> Result<Vec<T>, CoreError> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement
        .query_map([parameter], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|value| serde_json::from_str(&value).map_err(CoreError::from))
        .collect()
}

fn normalize_create_request(request: &mut CreateMediaJobRequest) -> Result<(), CoreError> {
    request.idempotency_key =
        required(&request.idempotency_key, "idempotency_key", 256)?.to_string();
    request.project_id = optional(request.project_id.take(), "project_id", 128)?;
    request.conversation_id = optional(request.conversation_id.take(), "conversation_id", 128)?;
    request.provider_id = required(&request.provider_id, "provider_id", 128)?.to_string();
    request.provider_source = normalize_provider_source(&request.provider_source)?;
    request.model_id = required(&request.model_id, "model_id", 256)?.to_string();
    request.api_version = optional(request.api_version.take(), "api_version", 128)?;
    request.currency = optional(request.currency.take(), "currency", 16)?;
    request.data_region = optional(request.data_region.take(), "data_region", 128)?;
    request.remote_retention_expires_at = optional_timestamp(
        request.remote_retention_expires_at.take(),
        "remote_retention_expires_at",
    )?;
    if request.input_asset_ids.len() > 64 {
        return Err(CoreError::InvalidInput(
            "input_asset_ids cannot contain more than 64 assets".to_string(),
        ));
    }
    request.input_asset_ids = request
        .input_asset_ids
        .iter()
        .map(|asset_id| normalize_sha256(asset_id))
        .collect::<Result<Vec<_>, _>>()?;
    ensure_object(&request.raw_parameters, "raw_parameters")?;
    ensure_object(&request.normalized_parameters, "normalized_parameters")?;
    ensure_object(&request.provider_extras, "provider_extras")?;
    request.raw_parameters = sanitize_persisted_json(&request.raw_parameters);
    request.normalized_parameters = sanitize_persisted_json(&request.normalized_parameters);
    request.provider_extras = sanitize_persisted_json(&request.provider_extras);
    for (field, value) in [
        ("raw_parameters", &request.raw_parameters),
        ("normalized_parameters", &request.normalized_parameters),
        ("provider_extras", &request.provider_extras),
    ] {
        let bytes = serde_json::to_vec(value)?.len();
        if bytes > MAX_CREATE_JSON_BYTES {
            return Err(CoreError::InvalidInput(format!(
                "{field} cannot exceed {MAX_CREATE_JSON_BYTES} bytes"
            )));
        }
    }
    validate_nonnegative(request.estimated_cost_micros, "estimated_cost_micros")?;
    if !(1..=10).contains(&request.max_attempts) {
        return Err(CoreError::InvalidInput(
            "max_attempts must be between 1 and 10".to_string(),
        ));
    }
    Ok(())
}

fn normalize_provider_event(
    request: &mut RecordMediaProviderEventRequest,
) -> Result<(), CoreError> {
    request.job_id = required(&request.job_id, "job_id", 128)?.to_string();
    request.attempt_id = required(&request.attempt_id, "attempt_id", 128)?.to_string();
    request.provider_id = required(&request.provider_id, "provider_id", 128)?.to_string();
    request.event_source = normalize_provider_source(&request.event_source)?;
    request.deduplication_key =
        required(&request.deduplication_key, "deduplication_key", 512)?.to_string();
    request.event_kind = required(&request.event_kind, "event_kind", 128)?.to_string();
    ensure_object(&request.payload, "payload")?;
    request.payload = sanitize_persisted_json(&request.payload);
    request.provider_created_at =
        optional_timestamp(request.provider_created_at.take(), "provider_created_at")?;
    request.provider_task_id = optional(request.provider_task_id.take(), "provider_task_id", 512)?;
    if let Some(error) = request.error.as_ref() {
        ensure_object(error, "error")?;
    }
    request.error = request.error.as_ref().map(sanitize_persisted_json);
    request.retry_classification = optional(
        request.retry_classification.take(),
        "retry_classification",
        128,
    )?;
    request.next_eligible_at =
        optional_timestamp(request.next_eligible_at.take(), "next_eligible_at")?;
    if let Some(result) = request.cancellation_result.as_ref() {
        ensure_object(result, "cancellation_result")?;
    }
    request.cancellation_result = request
        .cancellation_result
        .as_ref()
        .map(sanitize_persisted_json);
    validate_nonnegative(request.final_cost_micros, "final_cost_micros")?;
    if let Some(provenance) = request.provenance.as_ref() {
        ensure_object(provenance, "provenance")?;
    }
    request.provenance = request.provenance.as_ref().map(sanitize_persisted_json);
    ensure_json_size(
        &json!({
            "payload": request.payload.clone(),
            "error": request.error.clone(),
            "cancellationResult": request.cancellation_result.clone(),
            "provenance": request.provenance.clone(),
        }),
        "provider event persisted JSON envelope",
        MAX_PROVIDER_EVENT_PAYLOAD_BYTES,
    )?;
    Ok(())
}

fn normalize_asset(request: &mut RegisterMediaAssetRequest) -> Result<(), CoreError> {
    request.content_hash_sha256 = normalize_sha256(&request.content_hash_sha256)?;
    request.content_verified_at = optional_timestamp(
        Some(request.content_verified_at.clone()),
        "content_verified_at",
    )?
    .ok_or_else(|| CoreError::InvalidInput("content_verified_at cannot be empty".to_string()))?;
    request.media_type = required(&request.media_type, "media_type", 255)?.to_string();
    if !request.media_type.contains('/') {
        return Err(CoreError::InvalidInput(
            "media_type must be a valid MIME type".to_string(),
        ));
    }
    if request.byte_length == 0 {
        return Err(CoreError::InvalidInput(
            "byte_length must be greater than zero for a verified asset".to_string(),
        ));
    }
    u64_to_i64(request.byte_length, "byte_length")?;
    request.storage_key = required(&request.storage_key, "storage_key", 2048)?.to_string();
    Ok(())
}

fn normalize_asset_link(request: &mut LinkMediaAssetRequest) -> Result<(), CoreError> {
    request.job_id = required(&request.job_id, "job_id", 128)?.to_string();
    request.idempotency_key =
        required(&request.idempotency_key, "idempotency_key", 256)?.to_string();
    request.attempt_id = required(&request.attempt_id, "attempt_id", 128)?.to_string();
    request.asset_id = normalize_sha256(&request.asset_id)?;
    request.parent_asset_id = request
        .parent_asset_id
        .take()
        .map(|value| normalize_sha256(&value))
        .transpose()?;
    request.local_retention_expires_at = optional_timestamp(
        request.local_retention_expires_at.take(),
        "local_retention_expires_at",
    )?;
    match request.local_retention_policy {
        MediaAssetLocalRetentionPolicy::RetainUntilDeleted
            if request.local_retention_expires_at.is_some() =>
        {
            return Err(CoreError::InvalidInput(
                "retain_until_deleted occurrences cannot have local_retention_expires_at"
                    .to_string(),
            ));
        }
        MediaAssetLocalRetentionPolicy::DeleteAfterExpiry
            if request.local_retention_expires_at.is_none() =>
        {
            return Err(CoreError::InvalidInput(
                "delete_after_expiry occurrences require local_retention_expires_at".to_string(),
            ));
        }
        _ => {}
    }
    if request.parent_asset_id.as_deref() == Some(request.asset_id.as_str()) {
        return Err(CoreError::InvalidInput(
            "An asset cannot be its own lineage parent".to_string(),
        ));
    }
    if matches!(
        request.relation_type,
        MediaAssetRelationType::DerivedFrom
            | MediaAssetRelationType::VariantOf
            | MediaAssetRelationType::Extends
            | MediaAssetRelationType::Edits
            | MediaAssetRelationType::AudioTrack
    ) && request.parent_asset_id.is_none()
    {
        return Err(CoreError::InvalidInput(format!(
            "{} relations require a parent_asset_id",
            request.relation_type.as_str()
        )));
    }
    ensure_object(&request.metadata, "metadata")?;
    request.metadata = sanitize_persisted_json(&request.metadata);
    ensure_json_size(
        &request.metadata,
        "asset relation metadata",
        MAX_PROVIDER_EVENT_PAYLOAD_BYTES,
    )?;
    Ok(())
}

fn ensure_json_size(value: &Value, field: &str, limit: usize) -> Result<(), CoreError> {
    if serde_json::to_vec(value)?.len() > limit {
        return Err(CoreError::InvalidInput(format!(
            "{field} cannot exceed {limit} bytes"
        )));
    }
    Ok(())
}

fn request_fingerprint(request: &CreateMediaJobRequest) -> Result<String, CoreError> {
    let semantic = canonicalize_json(&json!({
        "projectId": request.project_id,
        "conversationId": request.conversation_id,
        "providerId": request.provider_id,
        "providerSource": request.provider_source,
        "modelId": request.model_id,
        "apiVersion": request.api_version,
        "operation": request.operation,
        "inputAssetIds": request.input_asset_ids,
        "rawParameters": request.raw_parameters,
        "normalizedParameters": request.normalized_parameters,
        "providerExtras": request.provider_extras,
        "observationMode": request.observation_mode,
        "estimatedCostMicros": request.estimated_cost_micros,
        "currency": request.currency,
        "dataRegion": request.data_region,
        "remoteRetentionExpiresAt": request.remote_retention_expires_at,
        "allowCrossProviderFallback": request.allow_cross_provider_fallback,
        "maxAttempts": request.max_attempts,
    }));
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&semantic)?)
    ))
}

fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let mut canonical = serde_json::Map::new();
            for key in keys {
                canonical.insert(key.clone(), canonicalize_json(&object[key]));
            }
            Value::Object(canonical)
        }
        Value::Array(values) => Value::Array(values.iter().map(canonicalize_json).collect()),
        other => other.clone(),
    }
}

fn sanitize_persisted_json(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut sanitized = serde_json::Map::new();
            for (key, value) in object {
                let compact_key = key
                    .chars()
                    .filter(|character| character.is_ascii_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect::<String>();
                let value = if is_sensitive_key(&compact_key) {
                    Value::String("[REDACTED]".to_string())
                } else {
                    sanitize_persisted_json(value)
                };
                sanitized.insert(key.clone(), value);
            }
            Value::Object(sanitized)
        }
        Value::Array(values) => Value::Array(values.iter().map(sanitize_persisted_json).collect()),
        Value::String(value) => Value::String(redact_sensitive_string(value)),
        other => other.clone(),
    }
}

fn is_sensitive_key(compact_key: &str) -> bool {
    matches!(
        compact_key,
        "authorization"
            | "apikey"
            | "accesstoken"
            | "refreshtoken"
            | "bearertoken"
            | "password"
            | "secret"
            | "clientsecret"
            | "credential"
            | "credentials"
            | "cookie"
            | "setcookie"
            | "signature"
            | "xamzsignature"
    ) || compact_key.ends_with("token")
        || compact_key.ends_with("secret")
}

fn redact_url_query(value: &str) -> String {
    if !(value.starts_with("https://") || value.starts_with("http://")) {
        return value.to_string();
    }
    let query_or_fragment = value
        .char_indices()
        .find_map(|(index, character)| matches!(character, '?' | '#').then_some(index));
    match query_or_fragment {
        Some(index) => format!("{}?[REDACTED]", &value[..index]),
        None => value.to_string(),
    }
}

fn redact_sensitive_string(value: &str) -> String {
    let redacted_url = redact_url_query(value);
    if Url::parse(&redacted_url)
        .is_ok_and(|url| !url.username().is_empty() || url.password().is_some())
    {
        return "[REDACTED]".to_string();
    }
    let lowercase = redacted_url.to_ascii_lowercase();
    const INLINE_CREDENTIAL_MARKERS: &[&str] = &[
        "authorization:",
        "authorization=",
        "bearer ",
        "api_key=",
        "api_key:",
        "api-key=",
        "api-key:",
        "apikey=",
        "apikey:",
        "x-api-key=",
        "x-api-key:",
        "access_token=",
        "access_token:",
        "refresh_token=",
        "refresh_token:",
        "client_secret=",
        "client_secret:",
        "password=",
        "password:",
        "secret=",
        "secret:",
        "credential=",
        "credential:",
        "signature=",
        "signature:",
    ];
    if INLINE_CREDENTIAL_MARKERS
        .iter()
        .any(|marker| lowercase.contains(marker))
    {
        "[REDACTED]".to_string()
    } else {
        redacted_url
    }
}

struct InternalEvent<'a> {
    job_id: &'a str,
    attempt_id: &'a str,
    provider_id: &'a str,
    event_source: &'a str,
    deduplication_key: &'a str,
    event_kind: &'a str,
    payload: &'a Value,
}

fn insert_internal_event(conn: &Connection, event: InternalEvent<'_>) -> Result<(), CoreError> {
    let payload_json = serde_json::to_string(&sanitize_persisted_json(event.payload))?;
    ensure_provider_event_budget(conn, event.job_id, payload_json.len(), true)?;
    let sequence = conn.query_row(
        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM media_provider_events WHERE job_id = ?1",
        [event.job_id],
        |row| row.get::<_, i64>(0),
    )?;
    conn.execute(
        "INSERT INTO media_provider_events (
             id, job_id, attempt_id, sequence, provider_id, event_source,
             deduplication_key, event_kind, payload_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            Uuid::new_v4().to_string(),
            event.job_id,
            event.attempt_id,
            sequence,
            event.provider_id,
            event.event_source,
            event.deduplication_key,
            event.event_kind,
            payload_json,
        ],
    )?;
    Ok(())
}

fn ensure_provider_event_budget(
    conn: &Connection,
    job_id: &str,
    additional_payload_bytes: usize,
    internal: bool,
) -> Result<(), CoreError> {
    let (event_count, payload_bytes) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(length(CAST(payload_json AS BLOB))), 0)
         FROM media_provider_events WHERE job_id = ?1",
        [job_id],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )?;
    let additional_payload_bytes = i64::try_from(additional_payload_bytes)
        .map_err(|_| CoreError::InvalidInput("Provider event payload is too large".to_string()))?;
    let projected_bytes = payload_bytes
        .checked_add(additional_payload_bytes)
        .ok_or_else(|| {
            CoreError::InvalidInput("Provider event byte budget overflowed".to_string())
        })?;
    let count_limit = if internal {
        MAX_PROVIDER_EVENTS_PER_JOB
    } else {
        MAX_EXTERNAL_PROVIDER_EVENTS_PER_JOB
    };
    let byte_limit = if internal {
        MAX_PROVIDER_EVENT_BYTES_PER_JOB
    } else {
        MAX_EXTERNAL_PROVIDER_EVENT_BYTES_PER_JOB
    };
    if event_count >= count_limit || projected_bytes > byte_limit {
        return Err(CoreError::Conflict(format!(
            "Media job {job_id} exhausted its persisted provider event budget"
        )));
    }
    Ok(())
}

fn stable_id(namespace: &str, value: &str) -> String {
    format!("{:x}", Sha256::digest(format!("{namespace}\0{value}")))
}

fn normalize_sha256(value: &str) -> Result<String, CoreError> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.len() != 64 || !normalized.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CoreError::InvalidInput(
            "content hash must be a 64-character SHA-256 hex digest".to_string(),
        ));
    }
    Ok(normalized)
}

fn normalize_provider_source(value: &str) -> Result<String, CoreError> {
    let value = required(value, "provider_source", 1024)?;
    let parsed = Url::parse(value).map_err(|error| {
        CoreError::InvalidInput(format!(
            "provider_source must be an absolute endpoint/account/region URI: {error}"
        ))
    })?;
    let lower = value.to_ascii_lowercase();
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || lower.contains("api_key=")
        || lower.contains("apikey=")
        || lower.contains("token=")
        || lower.contains("secret=")
        || lower.contains("authorization=")
    {
        return Err(CoreError::InvalidInput(
            "provider_source must not contain query parameters, fragments, or credentials"
                .to_string(),
        ));
    }
    Ok(parsed.to_string())
}

const fn requires_provider_task_identity(state: MediaJobState) -> bool {
    matches!(
        state,
        MediaJobState::Queued
            | MediaJobState::Running
            | MediaJobState::PostProcessing
            | MediaJobState::Completed
    )
}

const fn provider_projected_attempt_state(
    state: MediaJobState,
) -> Option<super::model::MediaJobAttemptState> {
    use super::model::MediaJobAttemptState;

    match state {
        MediaJobState::Submitting => Some(MediaJobAttemptState::Failed),
        MediaJobState::Queued => Some(MediaJobAttemptState::Accepted),
        MediaJobState::Running => Some(MediaJobAttemptState::Observing),
        MediaJobState::PostProcessing => Some(MediaJobAttemptState::Succeeded),
        MediaJobState::Failed => Some(MediaJobAttemptState::Failed),
        MediaJobState::Cancelled => Some(MediaJobAttemptState::Cancelled),
        MediaJobState::Expired => Some(MediaJobAttemptState::Expired),
        MediaJobState::ProviderUnknown => Some(MediaJobAttemptState::ProviderUnknown),
        _ => None,
    }
}

fn required<'a>(value: &'a str, field: &str, max_len: usize) -> Result<&'a str, CoreError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(CoreError::InvalidInput(format!("{field} cannot be empty")));
    }
    if value.len() > max_len {
        return Err(CoreError::InvalidInput(format!(
            "{field} cannot exceed {max_len} bytes"
        )));
    }
    Ok(value)
}

fn optional(
    value: Option<String>,
    field: &str,
    max_len: usize,
) -> Result<Option<String>, CoreError> {
    value
        .map(|value| required(&value, field, max_len).map(str::to_string))
        .transpose()
}

fn optional_timestamp(value: Option<String>, field: &str) -> Result<Option<String>, CoreError> {
    let Some(value) = optional(value, field, 64)? else {
        return Ok(None);
    };
    let parsed = DateTime::parse_from_rfc3339(&value).map_err(|error| {
        CoreError::InvalidInput(format!("{field} must be an RFC 3339 timestamp: {error}"))
    })?;
    Ok(Some(
        parsed
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true),
    ))
}

fn ensure_object(value: &Value, field: &str) -> Result<(), CoreError> {
    if !value.is_object() {
        return Err(CoreError::InvalidInput(format!(
            "{field} must be a JSON object"
        )));
    }
    Ok(())
}

fn validate_nonnegative(value: Option<i64>, field: &str) -> Result<(), CoreError> {
    if value.is_some_and(|value| value < 0) {
        return Err(CoreError::InvalidInput(format!(
            "{field} cannot be negative"
        )));
    }
    Ok(())
}

fn u64_to_i64(value: u64, field: &str) -> Result<i64, CoreError> {
    i64::try_from(value).map_err(|_| {
        CoreError::InvalidInput(format!("{field} exceeds the supported integer range"))
    })
}

const fn bool_to_i64(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

fn check_revision(job: &MediaJobRecord, expected_revision: u64) -> Result<(), CoreError> {
    if job.revision != expected_revision {
        return Err(CoreError::Conflict(format!(
            "Media job {} revision changed from {} to {}; reload before retrying",
            job.id, expected_revision, job.revision
        )));
    }
    Ok(())
}

trait MediaJobRecordExt {
    fn attempts_started(&self, conn: &Connection) -> Result<i64, CoreError>;
}

impl MediaJobRecordExt for MediaJobRecord {
    fn attempts_started(&self, conn: &Connection) -> Result<i64, CoreError> {
        conn.query_row(
            "SELECT COUNT(*) FROM media_job_attempts WHERE job_id = ?1",
            [&self.id],
            |row| row.get(0),
        )
        .map_err(CoreError::from)
    }
}
