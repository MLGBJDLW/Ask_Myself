use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::db::Database;
use crate::error::CoreError;
use crate::video_provider_catalog::{load_video_provider_presets, VideoModelManifest};

use super::adapters::{NormalizedVideoRequest, VideoInputAsset, VideoInputRole};
use super::model::{CreateMediaJobRequest, MediaJobState, MediaObservationMode, MediaOperation};
use super::store;

const MAX_WORKFLOW_TITLE_BYTES: usize = 160;
const MAX_SHOT_TITLE_BYTES: usize = 160;
const MAX_PROMPT_CHARS: usize = 15_000;
const MAX_BRIEF_BYTES: usize = 64 * 1024;
const MAX_INPUTS_BYTES: usize = 256 * 1024;
const MAX_VARIANTS_PER_BATCH: u32 = 4;
const MAX_ACTIVE_VIDEO_VARIANTS: i64 = 24;
const MAX_WORKFLOW_VARIANT_HISTORY: i64 = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoProviderConnectionRecord {
    pub id: String,
    pub provider_id: String,
    pub display_name: String,
    pub official_base_url: String,
    pub credential_scope: String,
    pub data_region: Option<String>,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveVideoProviderConnectionRequest {
    pub id: Option<String>,
    pub expected_revision: Option<u64>,
    pub provider_id: String,
    pub display_name: String,
    pub api_key: String,
    pub data_region: Option<String>,
}

pub struct MaterializedVideoProviderConnection {
    pub record: VideoProviderConnectionRecord,
    pub api_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoWorkflowRecord {
    pub id: String,
    pub project_id: Option<String>,
    pub title: String,
    pub brief: Value,
    pub aspect_ratio: String,
    pub target_duration_ms: u64,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoWorkflowShotRecord {
    pub id: String,
    pub workflow_id: String,
    pub ordinal: u32,
    pub title: String,
    pub prompt: String,
    pub operation: MediaOperation,
    pub connection_id: Option<String>,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    pub api_version: Option<String>,
    pub duration_seconds: u32,
    pub resolution: String,
    pub aspect_ratio: String,
    pub input_assets: Vec<VideoInputAsset>,
    pub seed: Option<u32>,
    pub generate_audio: Option<bool>,
    pub data_region: Option<String>,
    pub retention_policy: String,
    pub watermark_policy: String,
    pub provenance_policy: String,
    pub allow_cross_provider_fallback: bool,
    pub selected_variant_id: Option<String>,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoVariantJobRecord {
    pub id: String,
    pub state: MediaJobState,
    pub revision: u64,
    pub provider_id: String,
    pub provider_source: String,
    pub model_id: String,
    pub current_attempt_id: Option<String>,
    pub current_provider_task_id: Option<String>,
    pub retry_count: u32,
    pub max_attempts: u32,
    pub estimated_cost_micros: Option<i64>,
    pub final_cost_micros: Option<i64>,
    pub currency: Option<String>,
    pub cancellation_requested_at: Option<String>,
    pub error: Option<Value>,
    pub retry_classification: Option<String>,
    pub next_eligible_at: Option<String>,
    pub output_asset_id: Option<String>,
    pub output_media_type: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoWorkflowVariantRecord {
    pub id: String,
    pub workflow_id: String,
    pub shot_id: String,
    pub ordinal: u32,
    pub job_id: String,
    pub label: String,
    pub created_at: String,
    pub job: VideoVariantJobRecord,
    #[serde(skip)]
    pub(crate) shot_snapshot: Option<VideoWorkflowShotRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoWorkflowShotSnapshot {
    pub shot: VideoWorkflowShotRecord,
    pub variants: Vec<VideoWorkflowVariantRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VideoQueueSummary {
    pub draft: u32,
    pub active: u32,
    pub completed: u32,
    pub failed: u32,
    pub cancelled: u32,
    pub provider_unknown: u32,
    pub estimated_cost_micros: i64,
    pub final_cost_micros: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoWorkflowSnapshot {
    pub workflow: VideoWorkflowRecord,
    pub shots: Vec<VideoWorkflowShotSnapshot>,
    pub queue: VideoQueueSummary,
    pub dag: VideoWorkflowDag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoWorkflowDagNodeKind {
    Prompt,
    ReferenceAsset,
    GenerateVideo,
    SelectVariant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoWorkflowDagNode {
    pub id: String,
    pub kind: VideoWorkflowDagNodeKind,
    pub shot_id: String,
    pub depends_on: Vec<String>,
    pub variant_ids: Vec<String>,
    pub selected_variant_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoWorkflowDag {
    pub workflow_id: String,
    pub revision: u64,
    pub nodes: Vec<VideoWorkflowDagNode>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoVariantExecutionContext {
    pub workflow_id: String,
    pub shot: VideoWorkflowShotRecord,
    pub job_id: String,
    pub cancel_terminal_record_deletion_authorized: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateVideoWorkflowRequest {
    pub project_id: Option<String>,
    pub title: String,
    #[serde(default = "empty_object")]
    pub brief: Value,
    pub aspect_ratio: String,
    pub target_duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateVideoWorkflowRequest {
    pub workflow_id: String,
    pub expected_revision: u64,
    pub title: String,
    pub brief: Value,
    pub aspect_ratio: String,
    pub target_duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoShotInput {
    pub title: String,
    pub prompt: String,
    pub operation: MediaOperation,
    pub connection_id: Option<String>,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    pub api_version: Option<String>,
    pub duration_seconds: u32,
    pub resolution: String,
    pub aspect_ratio: String,
    #[serde(default)]
    pub input_assets: Vec<VideoInputAsset>,
    pub seed: Option<u32>,
    pub generate_audio: Option<bool>,
    #[serde(default)]
    pub allow_cross_provider_fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddVideoWorkflowShotRequest {
    pub workflow_id: String,
    pub expected_workflow_revision: u64,
    pub shot: VideoShotInput,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateVideoWorkflowShotRequest {
    pub workflow_id: String,
    pub expected_workflow_revision: u64,
    pub shot_id: String,
    pub expected_shot_revision: u64,
    pub shot: VideoShotInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderVideoWorkflowShotsRequest {
    pub workflow_id: String,
    pub expected_workflow_revision: u64,
    pub ordered_shot_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderVideoWorkflowVariantsRequest {
    pub workflow_id: String,
    pub expected_workflow_revision: u64,
    pub shot_id: String,
    pub expected_shot_revision: u64,
    pub ordered_variant_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteVideoWorkflowShotRequest {
    pub workflow_id: String,
    pub expected_workflow_revision: u64,
    pub shot_id: String,
    pub expected_shot_revision: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnqueuePreparedVideoVariantsRequest {
    pub workflow_id: String,
    pub expected_workflow_revision: u64,
    pub shot_id: String,
    pub expected_shot_revision: u64,
    pub idempotency_key: String,
    pub count: u32,
    pub expected_connection_revision: u64,
    pub provider_source: String,
    pub normalized_request: NormalizedVideoRequest,
    pub estimated_cost_micros: Option<i64>,
    pub currency: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectVideoWorkflowVariantRequest {
    pub workflow_id: String,
    pub expected_workflow_revision: u64,
    pub shot_id: String,
    pub expected_shot_revision: u64,
    pub variant_id: String,
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

pub(crate) fn save_provider_connection(
    database: &Database,
    mut request: SaveVideoProviderConnectionRequest,
) -> Result<VideoProviderConnectionRecord, CoreError> {
    request.provider_id = required(&request.provider_id, "provider_id", 64)?;
    request.display_name = required(&request.display_name, "display_name", 160)?;
    request.data_region = optional_bounded(request.data_region, "data_region", 128)?;
    if request.api_key.trim().is_empty() || request.api_key.len() > 4096 {
        return Err(CoreError::InvalidInput(
            "Video provider API key must contain 1-4096 bytes".to_string(),
        ));
    }
    let official_base_url = official_base_url(&request.provider_id)?.to_string();
    let encrypted = crate::crypto::encrypt_api_key(request.api_key.trim())?;
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let id = if let Some(id) = request.id.as_deref() {
        let id = required(id, "connection_id", 128)?;
        let current = load_connection(&tx, &id)?;
        let expected = request.expected_revision.ok_or_else(|| {
            CoreError::InvalidInput("expected_revision is required when updating".to_string())
        })?;
        if current.revision != expected {
            return Err(CoreError::Conflict(format!(
                "Video provider connection revision changed from {expected} to {}",
                current.revision
            )));
        }
        if current.provider_id != request.provider_id {
            return Err(CoreError::InvalidInput(
                "A video provider connection cannot change provider".to_string(),
            ));
        }
        let used: i64 = tx.query_row(
            "SELECT COUNT(*) FROM video_workflow_variants WHERE connection_id = ?1",
            [&id],
            |row| row.get(0),
        )?;
        if used > 0 {
            return Err(CoreError::Conflict(
                "A provider connection used by generated variants is immutable; create a new connection for credential or account changes"
                    .to_string(),
            ));
        }
        tx.execute(
            "UPDATE video_provider_connections
             SET display_name = ?2, credential_ciphertext = ?3, data_region = ?4,
                 revision = revision + 1,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?1",
            rusqlite::params![id, request.display_name, encrypted, request.data_region],
        )?;
        id
    } else {
        if request.expected_revision.is_some() {
            return Err(CoreError::InvalidInput(
                "expected_revision is only valid when updating".to_string(),
            ));
        }
        let id = Uuid::new_v4().to_string();
        let credential_scope = format!("video-connection:{id}");
        tx.execute(
            "INSERT INTO video_provider_connections (
                 id, provider_id, display_name, official_base_url,
                 credential_ciphertext, credential_scope, data_region
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                id,
                request.provider_id,
                request.display_name,
                official_base_url,
                encrypted,
                credential_scope,
                request.data_region,
            ],
        )?;
        id
    };
    let record = load_connection(&tx, &id)?;
    tx.commit()?;
    Ok(record)
}

pub(crate) fn list_provider_connections(
    database: &Database,
) -> Result<Vec<VideoProviderConnectionRecord>, CoreError> {
    let conn = database.conn();
    let mut statement = conn.prepare(
        "SELECT id FROM video_provider_connections ORDER BY provider_id, display_name, id",
    )?;
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    ids.into_iter()
        .map(|id| load_connection(&conn, &id))
        .collect()
}

pub(crate) fn materialize_provider_connection(
    database: &Database,
    connection_id: &str,
) -> Result<MaterializedVideoProviderConnection, CoreError> {
    let conn = database.conn();
    let id = required(connection_id, "connection_id", 128)?;
    let record = load_connection(&conn, &id)?;
    let ciphertext: String = conn.query_row(
        "SELECT credential_ciphertext FROM video_provider_connections WHERE id = ?1",
        [&id],
        |row| row.get(0),
    )?;
    let api_key = crate::crypto::decrypt_api_key(&ciphertext)?;
    if api_key.trim().is_empty() {
        return Err(CoreError::InvalidInput(
            "Video provider credential is empty".to_string(),
        ));
    }
    Ok(MaterializedVideoProviderConnection { record, api_key })
}

pub(crate) fn delete_provider_connection(
    database: &Database,
    connection_id: &str,
    expected_revision: u64,
) -> Result<(), CoreError> {
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let id = required(connection_id, "connection_id", 128)?;
    let current = load_connection(&tx, &id)?;
    if current.revision != expected_revision {
        return Err(CoreError::Conflict(
            "Video provider connection changed".to_string(),
        ));
    }
    let used: i64 = tx.query_row(
        "SELECT COUNT(*) FROM video_workflow_variants WHERE connection_id = ?1",
        [&id],
        |row| row.get(0),
    )?;
    if used > 0 {
        return Err(CoreError::Conflict(
            "A connection used by generated variants cannot be deleted; keep it for durable recovery"
                .to_string(),
        ));
    }
    tx.execute(
        "DELETE FROM video_provider_connections WHERE id = ?1",
        [&id],
    )?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn create_workflow(
    database: &Database,
    mut request: CreateVideoWorkflowRequest,
) -> Result<VideoWorkflowSnapshot, CoreError> {
    normalize_workflow(
        &mut request.title,
        &mut request.brief,
        &mut request.aspect_ratio,
        request.target_duration_ms,
    )?;
    request.project_id = optional_bounded(request.project_id, "project_id", 128)?;
    let conn = database.conn();
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO video_workflows
         (id, project_id, title, brief_json, aspect_ratio, target_duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            id,
            request.project_id,
            request.title,
            serde_json::to_string(&request.brief)?,
            request.aspect_ratio,
            i64::try_from(request.target_duration_ms).map_err(|_| CoreError::InvalidInput(
                "target_duration_ms is too large".to_string()
            ))?,
        ],
    )?;
    load_workflow_snapshot(&conn, &id)
}

pub(crate) fn update_workflow(
    database: &Database,
    mut request: UpdateVideoWorkflowRequest,
) -> Result<VideoWorkflowSnapshot, CoreError> {
    request.workflow_id = required(&request.workflow_id, "workflow_id", 128)?;
    normalize_workflow(
        &mut request.title,
        &mut request.brief,
        &mut request.aspect_ratio,
        request.target_duration_ms,
    )?;
    let conn = database.conn();
    check_workflow_revision(&conn, &request.workflow_id, request.expected_revision)?;
    conn.execute(
        "UPDATE video_workflows
         SET title = ?2, brief_json = ?3, aspect_ratio = ?4, target_duration_ms = ?5,
             revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        rusqlite::params![
            request.workflow_id,
            request.title,
            serde_json::to_string(&request.brief)?,
            request.aspect_ratio,
            i64::try_from(request.target_duration_ms).map_err(|_| CoreError::InvalidInput(
                "target_duration_ms is too large".to_string()
            ))?,
        ],
    )?;
    load_workflow_snapshot(&conn, &request.workflow_id)
}

pub(crate) fn list_workflows(
    database: &Database,
    project_id: Option<String>,
) -> Result<Vec<VideoWorkflowSnapshot>, CoreError> {
    let conn = database.conn();
    let project_id = optional_bounded(project_id, "project_id", 128)?;
    let mut statement = if project_id.is_some() {
        conn.prepare(
            "SELECT id FROM video_workflows WHERE project_id = ?1 ORDER BY updated_at DESC, id",
        )?
    } else {
        conn.prepare("SELECT id FROM video_workflows ORDER BY updated_at DESC, id")?
    };
    let ids = if let Some(project_id) = project_id {
        statement
            .query_map([project_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    ids.into_iter()
        .map(|id| load_workflow_snapshot(&conn, &id))
        .collect()
}

pub(crate) fn get_workflow(
    database: &Database,
    workflow_id: &str,
) -> Result<VideoWorkflowSnapshot, CoreError> {
    let conn = database.conn();
    load_workflow_snapshot(&conn, &required(workflow_id, "workflow_id", 128)?)
}

pub(crate) fn variant_execution_context(
    database: &Database,
    job_id: &str,
) -> Result<VideoVariantExecutionContext, CoreError> {
    let conn = database.conn();
    let job_id = required(job_id, "job_id", 128)?;
    let identity: Option<(String, String, bool)> = conn
        .query_row(
            "SELECT workflow_id, shot_snapshot_json,
                    cancel_terminal_record_deletion_authorized = 1
             FROM video_workflow_variants WHERE job_id = ?1",
            [&job_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let (workflow_id, shot_snapshot, cancel_terminal_record_deletion_authorized) = identity
        .ok_or_else(|| CoreError::NotFound(format!("Video workflow variant job {job_id}")))?;
    Ok(VideoVariantExecutionContext {
        workflow_id,
        shot: serde_json::from_str(&shot_snapshot)?,
        job_id,
        cancel_terminal_record_deletion_authorized,
    })
}

pub(crate) fn list_resumable_variant_contexts(
    database: &Database,
) -> Result<Vec<VideoVariantExecutionContext>, CoreError> {
    let conn = database.conn();
    let mut statement = conn.prepare(
        "SELECT variants.workflow_id, variants.shot_snapshot_json, variants.job_id,
                variants.cancel_terminal_record_deletion_authorized = 1
         FROM video_workflow_variants AS variants
         JOIN media_jobs AS jobs ON jobs.id = variants.job_id
         LEFT JOIN media_job_attempts AS attempts ON attempts.id = jobs.current_attempt_id
         LEFT JOIN video_job_runtime_state AS runtime ON runtime.job_id = jobs.id
         WHERE jobs.state IN (
             'draft', 'validating', 'uploading_assets', 'submitting',
             'queued', 'running', 'post_processing', 'provider_unknown'
         )
           AND NOT (jobs.state = 'submitting' AND attempts.state = 'failed')
           AND NOT (
               jobs.state = 'post_processing'
               AND COALESCE(runtime.materialization_failure_count, 0) >= 12
           )
         ORDER BY jobs.updated_at, variants.created_at",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(
            |(workflow_id, shot_snapshot, job_id, cancel_terminal_record_deletion_authorized)| {
                Ok(VideoVariantExecutionContext {
                    workflow_id,
                    shot: serde_json::from_str(&shot_snapshot)?,
                    job_id,
                    cancel_terminal_record_deletion_authorized,
                })
            },
        )
        .collect()
}

pub(crate) fn materialization_failure_count(
    database: &Database,
    job_id: &str,
) -> Result<u32, CoreError> {
    let conn = database.conn();
    let count = conn.query_row(
        "SELECT COALESCE((SELECT materialization_failure_count
                          FROM video_job_runtime_state WHERE job_id = ?1), 0)",
        [job_id],
        |row| row.get(0),
    )?;
    Ok(count)
}

pub(crate) fn increment_materialization_failure(
    database: &Database,
    job_id: &str,
) -> Result<u32, CoreError> {
    let conn = database.conn();
    conn.execute(
        "INSERT INTO video_job_runtime_state (job_id, materialization_failure_count)
         VALUES (?1, 1)
         ON CONFLICT(job_id) DO UPDATE SET
             materialization_failure_count = MIN(materialization_failure_count + 1, 12),
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
        [job_id],
    )?;
    drop(conn);
    materialization_failure_count(database, job_id)
}

pub(crate) fn reset_materialization_failures(
    database: &Database,
    job_id: &str,
) -> Result<(), CoreError> {
    let conn = database.conn();
    conn.execute(
        "DELETE FROM video_job_runtime_state WHERE job_id = ?1",
        [job_id],
    )?;
    Ok(())
}

fn valid_lease_kind(kind: &str) -> Result<(), CoreError> {
    if matches!(kind, "observe" | "cancel") {
        Ok(())
    } else {
        Err(CoreError::InvalidInput(
            "Invalid video job lease kind".to_string(),
        ))
    }
}

pub(crate) fn try_acquire_job_lease(
    database: &Database,
    job_id: &str,
    kind: &str,
    owner_id: &str,
    ttl_seconds: i64,
) -> Result<bool, CoreError> {
    valid_lease_kind(kind)?;
    if ttl_seconds <= 0 {
        return Err(CoreError::InvalidInput(
            "Video job lease TTL must be positive".to_string(),
        ));
    }
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let affected = tx.execute(
        "INSERT INTO video_job_leases (job_id, lease_kind, owner_id, expires_at_epoch)
         VALUES (?1, ?2, ?3, CAST(strftime('%s','now') AS INTEGER) + ?4)
         ON CONFLICT(job_id, lease_kind) DO UPDATE SET
             owner_id = excluded.owner_id,
             expires_at_epoch = excluded.expires_at_epoch,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE video_job_leases.owner_id = excluded.owner_id
            OR video_job_leases.expires_at_epoch <= CAST(strftime('%s','now') AS INTEGER)",
        rusqlite::params![job_id, kind, owner_id, ttl_seconds],
    )?;
    tx.commit()?;
    Ok(affected == 1)
}

pub(crate) fn renew_job_lease(
    database: &Database,
    job_id: &str,
    kind: &str,
    owner_id: &str,
    ttl_seconds: i64,
) -> Result<bool, CoreError> {
    valid_lease_kind(kind)?;
    let conn = database.conn();
    Ok(conn.execute(
        "UPDATE video_job_leases
         SET expires_at_epoch = CAST(strftime('%s','now') AS INTEGER) + ?4,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE job_id = ?1 AND lease_kind = ?2 AND owner_id = ?3",
        rusqlite::params![job_id, kind, owner_id, ttl_seconds],
    )? == 1)
}

pub(crate) fn release_job_lease(
    database: &Database,
    job_id: &str,
    kind: &str,
    owner_id: &str,
) -> Result<(), CoreError> {
    valid_lease_kind(kind)?;
    let conn = database.conn();
    conn.execute(
        "DELETE FROM video_job_leases
         WHERE job_id = ?1 AND lease_kind = ?2 AND owner_id = ?3",
        rusqlite::params![job_id, kind, owner_id],
    )?;
    Ok(())
}

pub(crate) fn authorize_variant_cancellation(
    database: &Database,
    job_id: &str,
    expected_job_revision: u64,
    authorized: bool,
) -> Result<(), CoreError> {
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let job_id = required(job_id, "job_id", 128)?;
    let revision: Option<u64> = tx
        .query_row(
            "SELECT jobs.revision FROM video_workflow_variants AS variants
             JOIN media_jobs AS jobs ON jobs.id = variants.job_id
             WHERE variants.job_id = ?1",
            [&job_id],
            |row| row.get(0),
        )
        .optional()?;
    let revision = revision
        .ok_or_else(|| CoreError::NotFound(format!("Video workflow variant job {job_id}")))?;
    if revision != expected_job_revision {
        return Err(CoreError::Conflict(
            "Media job changed before cancellation authorization was persisted".to_string(),
        ));
    }
    tx.execute(
        "UPDATE video_workflow_variants
         SET cancel_terminal_record_deletion_authorized = ?2
         WHERE job_id = ?1",
        rusqlite::params![job_id, i64::from(authorized)],
    )?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn add_shot(
    database: &Database,
    mut request: AddVideoWorkflowShotRequest,
) -> Result<VideoWorkflowSnapshot, CoreError> {
    request.workflow_id = required(&request.workflow_id, "workflow_id", 128)?;
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    check_workflow_revision(
        &tx,
        &request.workflow_id,
        request.expected_workflow_revision,
    )?;
    let normalized = normalize_shot(&tx, request.shot)?;
    let ordinal: i64 = tx.query_row(
        "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM video_workflow_shots WHERE workflow_id = ?1",
        [&request.workflow_id],
        |row| row.get(0),
    )?;
    let shot_id = Uuid::new_v4().to_string();
    insert_shot(&tx, &shot_id, &request.workflow_id, ordinal, &normalized)?;
    bump_workflow(&tx, &request.workflow_id)?;
    let snapshot = load_workflow_snapshot(&tx, &request.workflow_id)?;
    tx.commit()?;
    Ok(snapshot)
}

pub(crate) fn update_shot(
    database: &Database,
    mut request: UpdateVideoWorkflowShotRequest,
) -> Result<VideoWorkflowSnapshot, CoreError> {
    request.workflow_id = required(&request.workflow_id, "workflow_id", 128)?;
    request.shot_id = required(&request.shot_id, "shot_id", 128)?;
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    check_workflow_revision(
        &tx,
        &request.workflow_id,
        request.expected_workflow_revision,
    )?;
    let current = load_shot(&tx, &request.shot_id)?;
    if current.workflow_id != request.workflow_id
        || current.revision != request.expected_shot_revision
    {
        return Err(CoreError::Conflict(
            "Video shot changed before it could be saved".to_string(),
        ));
    }
    let shot = normalize_shot(&tx, request.shot)?;
    update_shot_row(&tx, &request.shot_id, &shot)?;
    bump_workflow(&tx, &request.workflow_id)?;
    let snapshot = load_workflow_snapshot(&tx, &request.workflow_id)?;
    tx.commit()?;
    Ok(snapshot)
}

pub(crate) fn reorder_shots(
    database: &Database,
    mut request: ReorderVideoWorkflowShotsRequest,
) -> Result<VideoWorkflowSnapshot, CoreError> {
    request.workflow_id = required(&request.workflow_id, "workflow_id", 128)?;
    if request.ordered_shot_ids.is_empty() || request.ordered_shot_ids.len() > 200 {
        return Err(CoreError::InvalidInput(
            "ordered_shot_ids must contain 1-200 shots".to_string(),
        ));
    }
    for id in &mut request.ordered_shot_ids {
        *id = required(id, "shot_id", 128)?;
    }
    let unique = request
        .ordered_shot_ids
        .iter()
        .collect::<std::collections::HashSet<_>>();
    if unique.len() != request.ordered_shot_ids.len() {
        return Err(CoreError::InvalidInput(
            "Shot order contains duplicates".to_string(),
        ));
    }
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    check_workflow_revision(
        &tx,
        &request.workflow_id,
        request.expected_workflow_revision,
    )?;
    let stored: Vec<String> = {
        let mut statement = tx.prepare(
            "SELECT id FROM video_workflow_shots WHERE workflow_id = ?1 ORDER BY ordinal",
        )?;
        let rows = statement
            .query_map([&request.workflow_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    if stored.len() != request.ordered_shot_ids.len()
        || !stored.iter().all(|id| unique.contains(id))
    {
        return Err(CoreError::Conflict(
            "Shot order must contain the workflow's exact current shot set".to_string(),
        ));
    }
    tx.execute(
        "UPDATE video_workflow_shots SET ordinal = ordinal + 1000000 WHERE workflow_id = ?1",
        [&request.workflow_id],
    )?;
    for (ordinal, shot_id) in request.ordered_shot_ids.iter().enumerate() {
        tx.execute(
            "UPDATE video_workflow_shots SET ordinal = ?2, revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?1",
            rusqlite::params![shot_id, i64::try_from(ordinal).unwrap_or(i64::MAX)],
        )?;
    }
    bump_workflow(&tx, &request.workflow_id)?;
    let snapshot = load_workflow_snapshot(&tx, &request.workflow_id)?;
    tx.commit()?;
    Ok(snapshot)
}

pub(crate) fn reorder_variants(
    database: &Database,
    mut request: ReorderVideoWorkflowVariantsRequest,
) -> Result<VideoWorkflowSnapshot, CoreError> {
    request.workflow_id = required(&request.workflow_id, "workflow_id", 128)?;
    request.shot_id = required(&request.shot_id, "shot_id", 128)?;
    if request.ordered_variant_ids.is_empty() || request.ordered_variant_ids.len() > 500 {
        return Err(CoreError::InvalidInput(
            "ordered_variant_ids must contain 1-500 variants".to_string(),
        ));
    }
    for id in &mut request.ordered_variant_ids {
        *id = required(id, "variant_id", 128)?;
    }
    let unique = request
        .ordered_variant_ids
        .iter()
        .collect::<std::collections::HashSet<_>>();
    if unique.len() != request.ordered_variant_ids.len() {
        return Err(CoreError::InvalidInput(
            "Variant order contains duplicates".to_string(),
        ));
    }
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    check_workflow_revision(
        &tx,
        &request.workflow_id,
        request.expected_workflow_revision,
    )?;
    let shot = load_shot(&tx, &request.shot_id)?;
    if shot.workflow_id != request.workflow_id || shot.revision != request.expected_shot_revision {
        return Err(CoreError::Conflict(
            "Video shot changed before variants could be reordered".to_string(),
        ));
    }
    let stored: Vec<String> = {
        let mut statement = tx.prepare(
            "SELECT id FROM video_workflow_variants
             WHERE workflow_id = ?1 AND shot_id = ?2 ORDER BY ordinal, id",
        )?;
        let rows = statement
            .query_map(
                rusqlite::params![request.workflow_id, request.shot_id],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    if stored.len() != request.ordered_variant_ids.len()
        || !stored.iter().all(|id| unique.contains(id))
    {
        return Err(CoreError::Conflict(
            "Variant order must contain the shot's exact current variant set".to_string(),
        ));
    }
    tx.execute(
        "UPDATE video_workflow_variants SET ordinal = ordinal + 1000000 WHERE shot_id = ?1",
        [&request.shot_id],
    )?;
    for (ordinal, variant_id) in request.ordered_variant_ids.iter().enumerate() {
        tx.execute(
            "UPDATE video_workflow_variants SET ordinal = ?2 WHERE id = ?1",
            rusqlite::params![variant_id, i64::try_from(ordinal).unwrap_or(i64::MAX)],
        )?;
    }
    tx.execute(
        "UPDATE video_workflow_shots SET revision = revision + 1,
         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?1",
        [&request.shot_id],
    )?;
    bump_workflow(&tx, &request.workflow_id)?;
    let snapshot = load_workflow_snapshot(&tx, &request.workflow_id)?;
    tx.commit()?;
    Ok(snapshot)
}

pub(crate) fn delete_shot(
    database: &Database,
    mut request: DeleteVideoWorkflowShotRequest,
) -> Result<VideoWorkflowSnapshot, CoreError> {
    request.workflow_id = required(&request.workflow_id, "workflow_id", 128)?;
    request.shot_id = required(&request.shot_id, "shot_id", 128)?;
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    check_workflow_revision(
        &tx,
        &request.workflow_id,
        request.expected_workflow_revision,
    )?;
    let shot = load_shot(&tx, &request.shot_id)?;
    if shot.workflow_id != request.workflow_id || shot.revision != request.expected_shot_revision {
        return Err(CoreError::Conflict(
            "Video shot changed before deletion".to_string(),
        ));
    }
    let variants: i64 = tx.query_row(
        "SELECT COUNT(*) FROM video_workflow_variants WHERE shot_id = ?1",
        [&request.shot_id],
        |row| row.get(0),
    )?;
    if variants > 0 {
        return Err(CoreError::Conflict(
            "Generated variants preserve audit history; remove an empty shot or duplicate the workflow"
                .to_string(),
        ));
    }
    tx.execute(
        "DELETE FROM video_workflow_shots WHERE id = ?1",
        [&request.shot_id],
    )?;
    compact_shot_ordinals(&tx, &request.workflow_id)?;
    bump_workflow(&tx, &request.workflow_id)?;
    let snapshot = load_workflow_snapshot(&tx, &request.workflow_id)?;
    tx.commit()?;
    Ok(snapshot)
}

pub(crate) fn enqueue_variants(
    database: &Database,
    mut request: EnqueuePreparedVideoVariantsRequest,
) -> Result<VideoWorkflowSnapshot, CoreError> {
    request.workflow_id = required(&request.workflow_id, "workflow_id", 128)?;
    request.shot_id = required(&request.shot_id, "shot_id", 128)?;
    request.idempotency_key = required(&request.idempotency_key, "idempotency_key", 400)?;
    request.provider_source = required(&request.provider_source, "provider_source", 512)?;
    if !(1..=MAX_VARIANTS_PER_BATCH).contains(&request.count) {
        return Err(CoreError::InvalidInput(format!(
            "Variant batch size must be 1-{MAX_VARIANTS_PER_BATCH}"
        )));
    }
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut replayed_jobs = 0_u32;
    for offset in 0..request.count {
        let job_key = format!("{}:{offset}", request.idempotency_key);
        let exists: bool = tx.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM video_workflow_variants AS variants
                 JOIN media_jobs AS jobs ON jobs.id = variants.job_id
                 WHERE variants.workflow_id = ?1 AND variants.shot_id = ?2
                   AND jobs.idempotency_key = ?3
             )",
            rusqlite::params![request.workflow_id, request.shot_id, job_key],
            |row| row.get(0),
        )?;
        replayed_jobs += u32::from(exists);
    }
    let complete_replay = replayed_jobs == request.count;
    let next_key_exists: bool = tx.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM video_workflow_variants AS variants
             JOIN media_jobs AS jobs ON jobs.id = variants.job_id
             WHERE variants.workflow_id = ?1 AND variants.shot_id = ?2
               AND jobs.idempotency_key = ?3
         )",
        rusqlite::params![
            request.workflow_id,
            request.shot_id,
            format!("{}:{}", request.idempotency_key, request.count)
        ],
        |row| row.get(0),
    )?;
    if (replayed_jobs > 0 && !complete_replay) || (complete_replay && next_key_exists) {
        return Err(CoreError::Conflict(
            "Queue idempotency key is already bound to a different variant count".to_string(),
        ));
    }
    if !complete_replay {
        check_workflow_revision(
            &tx,
            &request.workflow_id,
            request.expected_workflow_revision,
        )?;
    }
    let shot = load_shot(&tx, &request.shot_id)?;
    if shot.workflow_id != request.workflow_id || shot.revision != request.expected_shot_revision {
        return Err(CoreError::Conflict(
            "Video shot changed before queueing".to_string(),
        ));
    }
    let mut disclosed_inputs = request.normalized_request.input_assets.clone();
    for input in &mut disclosed_inputs {
        input.local_asset_id = None;
    }
    if request.normalized_request.model_id != shot.model_id.clone().unwrap_or_default()
        || request.normalized_request.operation != shot.operation
        || request.normalized_request.prompt != shot.prompt
        || request.normalized_request.duration_seconds != shot.duration_seconds
        || request.normalized_request.resolution != shot.resolution
        || request.normalized_request.aspect_ratio != shot.aspect_ratio
        || disclosed_inputs != shot.input_assets
        || request.normalized_request.seed != shot.seed
        || request.normalized_request.generate_audio != shot.generate_audio
    {
        return Err(CoreError::Conflict(
            "Prepared provider request does not match the durable shot".to_string(),
        ));
    }
    if complete_replay {
        return load_workflow_snapshot(&tx, &request.workflow_id);
    }
    let active_variants: i64 = tx.query_row(
        "SELECT COUNT(*) FROM video_workflow_variants AS variants
         JOIN media_jobs AS jobs ON jobs.id = variants.job_id
         WHERE jobs.state NOT IN ('completed', 'failed', 'cancelled', 'expired')",
        [],
        |row| row.get(0),
    )?;
    if active_variants + i64::from(request.count) > MAX_ACTIVE_VIDEO_VARIANTS {
        return Err(CoreError::Conflict(format!(
            "Video queue is limited to {MAX_ACTIVE_VIDEO_VARIANTS} active variants"
        )));
    }
    let retained_variants: i64 = tx.query_row(
        "SELECT COUNT(*) FROM video_workflow_variants WHERE workflow_id = ?1",
        [&request.workflow_id],
        |row| row.get(0),
    )?;
    if retained_variants + i64::from(request.count) > MAX_WORKFLOW_VARIANT_HISTORY {
        return Err(CoreError::Conflict(format!(
            "A video workflow retains at most {MAX_WORKFLOW_VARIANT_HISTORY} variants"
        )));
    }
    let provider_id = shot
        .provider_id
        .clone()
        .ok_or_else(|| CoreError::InvalidInput("Shot has no configured provider".to_string()))?;
    let model_id = shot
        .model_id
        .clone()
        .ok_or_else(|| CoreError::InvalidInput("Shot has no configured model".to_string()))?;
    let connection_id = shot.connection_id.as_deref().ok_or_else(|| {
        CoreError::InvalidInput("Queued shot has no provider connection".to_string())
    })?;
    let connection = load_connection(&tx, connection_id)?;
    if connection.revision != request.expected_connection_revision {
        return Err(CoreError::Conflict(
            "Provider connection changed after queue disclosure; review the transfer again"
                .to_string(),
        ));
    }
    let input_asset_ids = request
        .normalized_request
        .input_assets
        .iter()
        .map(|input| {
            input.local_asset_id.clone().ok_or_else(|| {
                CoreError::Conflict(
                    "Prepared reference input has no local CAS identity".to_string(),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut next_ordinal: i64 = tx.query_row(
        "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM video_workflow_variants WHERE shot_id = ?1",
        [&request.shot_id],
        |row| row.get(0),
    )?;
    let mut inserted = false;
    for offset in 0..request.count {
        let job_key = format!("{}:{offset}", request.idempotency_key);
        let ordinal = next_ordinal;
        let mut normalized_request = request.normalized_request.clone();
        normalized_request.idempotency_key = job_key.clone();
        let normalized = serde_json::to_value(&normalized_request)?;
        let raw = json!({
            "workflowId": request.workflow_id,
            "shotId": request.shot_id,
            "shotRevision": shot.revision,
            "variantOrdinal": ordinal,
            "request": normalized,
        });
        let job = store::create_job_in_transaction(
            &tx,
            CreateMediaJobRequest {
                idempotency_key: job_key,
                project_id: load_workflow(&tx, &request.workflow_id)?.project_id,
                conversation_id: None,
                provider_id: provider_id.clone(),
                provider_source: request.provider_source.clone(),
                model_id: model_id.clone(),
                api_version: shot.api_version.clone(),
                operation: shot.operation,
                input_asset_ids: input_asset_ids.clone(),
                raw_parameters: raw,
                normalized_parameters: normalized,
                provider_extras: json!({
                    "videoWorkflowId": request.workflow_id,
                    "videoShotId": request.shot_id,
                    "videoShotRevision": shot.revision,
                }),
                observation_mode: MediaObservationMode::Polling,
                estimated_cost_micros: request.estimated_cost_micros,
                currency: request.currency.clone(),
                data_region: shot.data_region.clone(),
                remote_retention_expires_at: None,
                allow_cross_provider_fallback: shot.allow_cross_provider_fallback,
                max_attempts: 3,
            },
        )?;
        let existing_variant: Option<(String, String)> = tx
            .query_row(
                "SELECT workflow_id, shot_id FROM video_workflow_variants WHERE job_id = ?1",
                [&job.job.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((workflow_id, shot_id)) = existing_variant {
            if workflow_id != request.workflow_id || shot_id != request.shot_id {
                return Err(CoreError::Conflict(
                    "A queued media job is already bound to a different workflow shot".to_string(),
                ));
            }
            continue;
        }
        let variant_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO video_workflow_variants
             (id, workflow_id, shot_id, ordinal, job_id, connection_id,
              shot_snapshot_json, label)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                variant_id,
                request.workflow_id,
                request.shot_id,
                ordinal,
                job.job.id,
                connection_id,
                serde_json::to_string(&shot)?,
                format!("Variant {}", ordinal + 1),
            ],
        )?;
        next_ordinal += 1;
        inserted = true;
    }
    if inserted {
        bump_workflow(&tx, &request.workflow_id)?;
    }
    let snapshot = load_workflow_snapshot(&tx, &request.workflow_id)?;
    tx.commit()?;
    Ok(snapshot)
}

pub(crate) fn select_variant(
    database: &Database,
    mut request: SelectVideoWorkflowVariantRequest,
) -> Result<VideoWorkflowSnapshot, CoreError> {
    request.workflow_id = required(&request.workflow_id, "workflow_id", 128)?;
    request.shot_id = required(&request.shot_id, "shot_id", 128)?;
    request.variant_id = required(&request.variant_id, "variant_id", 128)?;
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    check_workflow_revision(
        &tx,
        &request.workflow_id,
        request.expected_workflow_revision,
    )?;
    let shot = load_shot(&tx, &request.shot_id)?;
    if shot.workflow_id != request.workflow_id || shot.revision != request.expected_shot_revision {
        return Err(CoreError::Conflict(
            "Video shot changed before selection".to_string(),
        ));
    }
    let state: Option<String> = tx
        .query_row(
            "SELECT jobs.state FROM video_workflow_variants AS variants
             JOIN media_jobs AS jobs ON jobs.id = variants.job_id
             WHERE variants.id = ?1 AND variants.shot_id = ?2 AND variants.workflow_id = ?3",
            rusqlite::params![request.variant_id, request.shot_id, request.workflow_id],
            |row| row.get(0),
        )
        .optional()?;
    if state.as_deref() != Some("completed") {
        return Err(CoreError::Conflict(
            "Only a completed variant can be selected".to_string(),
        ));
    }
    let output_count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM video_workflow_variants AS variants
          JOIN media_asset_relations AS relations ON relations.job_id = variants.job_id
         JOIN media_assets AS assets ON assets.id = relations.asset_id
         WHERE variants.id = ?1 AND relations.relation_type = 'output'
           AND assets.local_state = 'available'",
        [&request.variant_id],
        |row| row.get(0),
    )?;
    if output_count == 0 {
        return Err(CoreError::Conflict(
            "A completed variant must have a verified local output before selection".to_string(),
        ));
    }
    tx.execute(
        "UPDATE video_workflow_shots
         SET selected_variant_id = ?2, revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        rusqlite::params![request.shot_id, request.variant_id],
    )?;
    bump_workflow(&tx, &request.workflow_id)?;
    let snapshot = load_workflow_snapshot(&tx, &request.workflow_id)?;
    tx.commit()?;
    Ok(snapshot)
}

fn normalize_workflow(
    title: &mut String,
    brief: &mut Value,
    aspect_ratio: &mut String,
    target_duration_ms: u64,
) -> Result<(), CoreError> {
    *title = required(title, "title", MAX_WORKFLOW_TITLE_BYTES)?;
    if !brief.is_object() || serde_json::to_vec(brief)?.len() > MAX_BRIEF_BYTES {
        return Err(CoreError::InvalidInput(
            "Workflow brief must be an object no larger than 64 KiB".to_string(),
        ));
    }
    *aspect_ratio = required(aspect_ratio, "aspect_ratio", 32)?;
    if target_duration_ms == 0 || target_duration_ms > 3_600_000 {
        return Err(CoreError::InvalidInput(
            "target_duration_ms must be between 1 and 3600000".to_string(),
        ));
    }
    Ok(())
}

fn normalize_shot(
    conn: &Connection,
    mut shot: VideoShotInput,
) -> Result<NormalizedShot, CoreError> {
    shot.title = required(&shot.title, "shot.title", MAX_SHOT_TITLE_BYTES)?;
    shot.prompt = shot.prompt.trim().to_string();
    if shot.prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(CoreError::InvalidInput(
            "Shot prompt cannot exceed 15000 characters".to_string(),
        ));
    }
    if !matches!(
        shot.operation,
        MediaOperation::TextToVideo
            | MediaOperation::ImageToVideo
            | MediaOperation::VideoToVideo
            | MediaOperation::FirstLastFrame
    ) {
        return Err(CoreError::InvalidInput(
            "Shot Board supports text, image, video, and first/last-frame generation".to_string(),
        ));
    }
    shot.resolution = required(&shot.resolution, "shot.resolution", 32)?;
    shot.aspect_ratio = required(&shot.aspect_ratio, "shot.aspect_ratio", 32)?;
    if serde_json::to_vec(&shot.input_assets)?.len() > MAX_INPUTS_BYTES {
        return Err(CoreError::InvalidInput(
            "Shot input assets exceed the 256 KiB workflow bound".to_string(),
        ));
    }
    if shot.input_assets.len() > 30 {
        return Err(CoreError::InvalidInput(
            "A shot cannot contain more than 30 reference inputs".to_string(),
        ));
    }
    let mut input_occurrences = std::collections::HashSet::new();
    for input in &mut shot.input_assets {
        input.local_asset_id = None;
        if !input.metadata_verified {
            return Err(CoreError::InvalidInput(
                "Workflow reference inputs require provider-verified or Nexa-verified metadata"
                    .to_string(),
            ));
        }
        if input.uri.starts_with("mm_file://") || input.uri.starts_with("runway://") {
            return Err(CoreError::InvalidInput(
                "Ephemeral provider locators cannot be persisted in a workflow or exposed to the renderer"
                    .to_string(),
            ));
        }
        let content_hash = input.content_hash_sha256.as_deref().ok_or_else(|| {
            CoreError::InvalidInput(
                "Workflow reference inputs require a verified SHA-256 digest".to_string(),
            )
        })?;
        if content_hash.len() != 64
            || !content_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(CoreError::InvalidInput(
                "Reference input SHA-256 must be 64 lowercase hexadecimal characters".to_string(),
            ));
        }
        if !input_occurrences.insert((video_input_role_name(input.role), content_hash)) {
            return Err(CoreError::InvalidInput(
                "A shot cannot repeat the same reference bytes in the same role".to_string(),
            ));
        }
        let url = url::Url::parse(&input.uri).map_err(|_| {
            CoreError::InvalidInput("Reference input must be a valid HTTPS URL".to_string())
        })?;
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(CoreError::InvalidInput(
                "Reference input URLs must be credential-free HTTPS without query or fragment"
                    .to_string(),
            ));
        }
    }
    if shot.allow_cross_provider_fallback {
        return Err(CoreError::InvalidInput(
            "Automatic cross-provider fallback requires an exact destination and fresh consent; it is not enabled"
                .to_string(),
        ));
    }
    let (
        connection_id,
        provider_id,
        model_id,
        api_version,
        data_region,
        manifest,
        retention_policy,
    ) = match (&shot.connection_id, &shot.provider_id, &shot.model_id) {
        (None, None, None) => (None, None, None, None, None, None, "unverified".to_string()),
        (Some(connection_id), Some(provider_id), Some(model_id)) => {
            let connection = load_connection(conn, connection_id)?;
            if &connection.provider_id != provider_id {
                return Err(CoreError::InvalidInput(
                    "Shot provider does not match its credential connection".to_string(),
                ));
            }
            let (manifest, retention_policy) =
                find_manifest(provider_id, model_id, shot.api_version.as_deref())?;
            validate_manifest_operation(&manifest, &shot)?;
            (
                Some(connection.id),
                Some(provider_id.clone()),
                Some(model_id.clone()),
                manifest.api_version.clone(),
                connection.data_region,
                Some(manifest),
                retention_policy,
            )
        }
        _ => {
            return Err(CoreError::InvalidInput(
                "Shot connection, provider, and model must be configured together".to_string(),
            ))
        }
    };
    Ok(NormalizedShot {
        title: shot.title,
        prompt: shot.prompt,
        operation: shot.operation,
        connection_id,
        provider_id,
        model_id,
        api_version,
        duration_seconds: shot.duration_seconds,
        resolution: shot.resolution,
        aspect_ratio: shot.aspect_ratio,
        input_assets: shot.input_assets,
        seed: shot.seed,
        generate_audio: shot.generate_audio,
        data_region,
        retention_policy,
        watermark_policy: manifest
            .as_ref()
            .map(|manifest| manifest.watermark_policy.clone())
            .unwrap_or_else(|| "unverified".to_string()),
        provenance_policy: manifest
            .as_ref()
            .map(|manifest| manifest.provenance_policy.clone())
            .unwrap_or_else(|| "unverified".to_string()),
        allow_cross_provider_fallback: shot.allow_cross_provider_fallback,
    })
}

#[derive(Debug, Clone)]
struct NormalizedShot {
    title: String,
    prompt: String,
    operation: MediaOperation,
    connection_id: Option<String>,
    provider_id: Option<String>,
    model_id: Option<String>,
    api_version: Option<String>,
    duration_seconds: u32,
    resolution: String,
    aspect_ratio: String,
    input_assets: Vec<VideoInputAsset>,
    seed: Option<u32>,
    generate_audio: Option<bool>,
    data_region: Option<String>,
    retention_policy: String,
    watermark_policy: String,
    provenance_policy: String,
    allow_cross_provider_fallback: bool,
}

fn find_manifest(
    provider_id: &str,
    model_id: &str,
    api_version: Option<&str>,
) -> Result<(VideoModelManifest, String), CoreError> {
    for preset in load_video_provider_presets().map_err(CoreError::from)? {
        if preset.provider_id != provider_id {
            continue;
        }
        if let Some(model) = preset.models.into_iter().find(|model| {
            model.model_id == model_id
                && model.api_version.as_deref() == api_version
                && model.selectable
        }) {
            return Ok((model, preset.retention_policy));
        }
    }
    Err(CoreError::InvalidInput(
        "Shot model is not selectable in the evidence-backed video manifest".to_string(),
    ))
}

fn validate_manifest_operation(
    manifest: &VideoModelManifest,
    shot: &VideoShotInput,
) -> Result<(), CoreError> {
    let capability = manifest
        .operation_capabilities
        .iter()
        .find(|capability| capability.operation == shot.operation)
        .ok_or_else(|| {
            CoreError::InvalidInput("Model does not support this operation".to_string())
        })?;
    let duration = capability
        .duration_options
        .iter()
        .find(|duration| duration.resolution == shot.resolution)
        .ok_or_else(|| {
            CoreError::InvalidInput("Model does not support this resolution".to_string())
        })?;
    let duration_supported = if duration.durations_seconds.is_empty() {
        duration
            .min_duration_seconds
            .is_none_or(|minimum| shot.duration_seconds >= minimum)
            && duration
                .max_duration_seconds
                .is_none_or(|maximum| shot.duration_seconds <= maximum)
    } else {
        duration.durations_seconds.contains(&shot.duration_seconds)
    };
    if !duration_supported {
        return Err(CoreError::InvalidInput(
            "Model does not support this duration and resolution combination".to_string(),
        ));
    }
    if !capability
        .aspect_ratios
        .iter()
        .any(|ratio| ratio == &shot.aspect_ratio)
    {
        return Err(CoreError::InvalidInput(
            "Model does not support this aspect ratio".to_string(),
        ));
    }
    if shot.seed.is_some() && !capability.supports_seed {
        return Err(CoreError::InvalidInput(
            "Model does not support seed".to_string(),
        ));
    }
    if shot.generate_audio == Some(true) && !capability.supports_audio {
        return Err(CoreError::InvalidInput(
            "Model does not support native audio".to_string(),
        ));
    }
    Ok(())
}

fn insert_shot(
    conn: &Connection,
    shot_id: &str,
    workflow_id: &str,
    ordinal: i64,
    shot: &NormalizedShot,
) -> Result<(), CoreError> {
    conn.execute(
        "INSERT INTO video_workflow_shots (
             id, workflow_id, ordinal, title, prompt, operation, connection_id,
             provider_id, model_id, api_version, duration_seconds, resolution,
             aspect_ratio, input_assets_json, seed, generate_audio, data_region,
             retention_policy, watermark_policy, provenance_policy,
             allow_cross_provider_fallback
         ) VALUES (
             ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
             ?15, ?16, ?17, ?18, ?19, ?20, ?21
         )",
        rusqlite::params![
            shot_id,
            workflow_id,
            ordinal,
            shot.title,
            shot.prompt,
            shot.operation.as_str(),
            shot.connection_id,
            shot.provider_id,
            shot.model_id,
            shot.api_version,
            i64::from(shot.duration_seconds),
            shot.resolution,
            shot.aspect_ratio,
            serde_json::to_string(&shot.input_assets)?,
            shot.seed.map(i64::from),
            shot.generate_audio.map(i64::from),
            shot.data_region,
            shot.retention_policy,
            shot.watermark_policy,
            shot.provenance_policy,
            i64::from(shot.allow_cross_provider_fallback),
        ],
    )?;
    Ok(())
}

fn update_shot_row(
    conn: &Connection,
    shot_id: &str,
    shot: &NormalizedShot,
) -> Result<(), CoreError> {
    conn.execute(
        "UPDATE video_workflow_shots SET
             title = ?2, prompt = ?3, operation = ?4, connection_id = ?5,
             provider_id = ?6, model_id = ?7, api_version = ?8,
             duration_seconds = ?9, resolution = ?10, aspect_ratio = ?11,
             input_assets_json = ?12, seed = ?13, generate_audio = ?14,
             data_region = ?15, retention_policy = ?16, watermark_policy = ?17,
             provenance_policy = ?18, allow_cross_provider_fallback = ?19,
             revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        rusqlite::params![
            shot_id,
            shot.title,
            shot.prompt,
            shot.operation.as_str(),
            shot.connection_id,
            shot.provider_id,
            shot.model_id,
            shot.api_version,
            i64::from(shot.duration_seconds),
            shot.resolution,
            shot.aspect_ratio,
            serde_json::to_string(&shot.input_assets)?,
            shot.seed.map(i64::from),
            shot.generate_audio.map(i64::from),
            shot.data_region,
            shot.retention_policy,
            shot.watermark_policy,
            shot.provenance_policy,
            i64::from(shot.allow_cross_provider_fallback),
        ],
    )?;
    Ok(())
}

fn compact_shot_ordinals(conn: &Connection, workflow_id: &str) -> Result<(), CoreError> {
    let ids: Vec<String> = {
        let mut statement = conn.prepare(
            "SELECT id FROM video_workflow_shots WHERE workflow_id = ?1 ORDER BY ordinal, id",
        )?;
        let rows = statement
            .query_map([workflow_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    conn.execute(
        "UPDATE video_workflow_shots SET ordinal = ordinal + 1000000 WHERE workflow_id = ?1",
        [workflow_id],
    )?;
    for (ordinal, id) in ids.iter().enumerate() {
        conn.execute(
            "UPDATE video_workflow_shots SET ordinal = ?2 WHERE id = ?1",
            rusqlite::params![id, i64::try_from(ordinal).unwrap_or(i64::MAX)],
        )?;
    }
    Ok(())
}

fn load_connection(
    conn: &Connection,
    connection_id: &str,
) -> Result<VideoProviderConnectionRecord, CoreError> {
    query_json_optional(
        conn,
        "SELECT json_object(
             'id', id, 'providerId', provider_id, 'displayName', display_name,
             'officialBaseUrl', official_base_url, 'credentialScope', credential_scope,
             'dataRegion', data_region, 'revision', revision,
             'createdAt', created_at, 'updatedAt', updated_at
         ) FROM video_provider_connections WHERE id = ?1",
        connection_id,
    )?
    .ok_or_else(|| CoreError::NotFound(format!("Video provider connection {connection_id}")))
}

fn load_workflow(conn: &Connection, workflow_id: &str) -> Result<VideoWorkflowRecord, CoreError> {
    query_json_optional(
        conn,
        "SELECT json_object(
             'id', id, 'projectId', project_id, 'title', title,
             'brief', json(brief_json), 'aspectRatio', aspect_ratio,
             'targetDurationMs', target_duration_ms, 'revision', revision,
             'createdAt', created_at, 'updatedAt', updated_at
         ) FROM video_workflows WHERE id = ?1",
        workflow_id,
    )?
    .ok_or_else(|| CoreError::NotFound(format!("Video workflow {workflow_id}")))
}

fn load_shot(conn: &Connection, shot_id: &str) -> Result<VideoWorkflowShotRecord, CoreError> {
    query_json_optional(
        conn,
        "SELECT json_object(
             'id', id, 'workflowId', workflow_id, 'ordinal', ordinal,
             'title', title, 'prompt', prompt, 'operation', operation,
             'connectionId', connection_id, 'providerId', provider_id,
             'modelId', model_id, 'apiVersion', api_version,
             'durationSeconds', duration_seconds, 'resolution', resolution,
             'aspectRatio', aspect_ratio, 'inputAssets', json(input_assets_json),
             'seed', seed, 'generateAudio', CASE
                 WHEN generate_audio IS NULL THEN NULL
                 WHEN generate_audio = 1 THEN json('true') ELSE json('false') END,
             'dataRegion', data_region, 'retentionPolicy', retention_policy,
             'watermarkPolicy', watermark_policy, 'provenancePolicy', provenance_policy,
             'allowCrossProviderFallback', CASE WHEN allow_cross_provider_fallback = 1
                 THEN json('true') ELSE json('false') END,
             'selectedVariantId', selected_variant_id, 'revision', revision,
             'createdAt', created_at, 'updatedAt', updated_at
         ) FROM video_workflow_shots WHERE id = ?1",
        shot_id,
    )?
    .ok_or_else(|| CoreError::NotFound(format!("Video workflow shot {shot_id}")))
}

fn load_workflow_snapshot(
    conn: &Connection,
    workflow_id: &str,
) -> Result<VideoWorkflowSnapshot, CoreError> {
    let workflow = load_workflow(conn, workflow_id)?;
    let shot_ids: Vec<String> = {
        let mut statement = conn.prepare(
            "SELECT id FROM video_workflow_shots WHERE workflow_id = ?1 ORDER BY ordinal, id",
        )?;
        let rows = statement
            .query_map([workflow_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    let mut queue = VideoQueueSummary::default();
    let mut shots = Vec::with_capacity(shot_ids.len());
    for shot_id in shot_ids {
        let shot = load_shot(conn, &shot_id)?;
        let variants = load_variants(conn, workflow_id, &shot_id)?;
        for variant in &variants {
            match variant.job.state {
                MediaJobState::Draft => queue.draft += 1,
                MediaJobState::Completed => queue.completed += 1,
                MediaJobState::Failed | MediaJobState::Expired => queue.failed += 1,
                MediaJobState::Cancelled => queue.cancelled += 1,
                MediaJobState::ProviderUnknown => queue.provider_unknown += 1,
                _ => queue.active += 1,
            }
            queue.estimated_cost_micros = queue
                .estimated_cost_micros
                .saturating_add(variant.job.estimated_cost_micros.unwrap_or(0));
            queue.final_cost_micros = queue
                .final_cost_micros
                .saturating_add(variant.job.final_cost_micros.unwrap_or(0));
        }
        shots.push(VideoWorkflowShotSnapshot { shot, variants });
    }
    let dag = project_workflow_dag(&workflow, &shots);
    Ok(VideoWorkflowSnapshot {
        workflow,
        shots,
        queue,
        dag,
    })
}

fn project_workflow_dag(
    workflow: &VideoWorkflowRecord,
    shots: &[VideoWorkflowShotSnapshot],
) -> VideoWorkflowDag {
    let mut nodes = Vec::new();
    let mut previous_select = None;
    for item in shots {
        let shot_id = &item.shot.id;
        let mut projected_revisions = HashSet::new();
        let mut generate_ids = Vec::with_capacity(item.variants.len());
        for variant in &item.variants {
            let snapshot = variant.shot_snapshot.as_ref().unwrap_or(&item.shot);
            let prompt_id = format!("shot:{shot_id}:revision:{}:prompt", snapshot.revision);
            let mut generate_dependencies = vec![prompt_id.clone()];
            if projected_revisions.insert(snapshot.revision) {
                nodes.push(VideoWorkflowDagNode {
                    id: prompt_id,
                    kind: VideoWorkflowDagNodeKind::Prompt,
                    shot_id: shot_id.clone(),
                    depends_on: previous_select.iter().cloned().collect(),
                    variant_ids: Vec::new(),
                    selected_variant_id: None,
                });
            }
            for input in &snapshot.input_assets {
                let reference_id = format!(
                    "shot:{shot_id}:revision:{}:reference:{}:{}",
                    snapshot.revision,
                    video_input_role_name(input.role),
                    input.content_hash_sha256.as_deref().unwrap_or("unverified")
                );
                if !nodes.iter().any(|node| node.id == reference_id) {
                    nodes.push(VideoWorkflowDagNode {
                        id: reference_id.clone(),
                        kind: VideoWorkflowDagNodeKind::ReferenceAsset,
                        shot_id: shot_id.clone(),
                        depends_on: Vec::new(),
                        variant_ids: Vec::new(),
                        selected_variant_id: None,
                    });
                }
                generate_dependencies.push(reference_id);
            }
            let generate_id = format!("shot:{shot_id}:variant:{}:generate", variant.id);
            nodes.push(VideoWorkflowDagNode {
                id: generate_id.clone(),
                kind: VideoWorkflowDagNodeKind::GenerateVideo,
                shot_id: shot_id.clone(),
                depends_on: generate_dependencies.clone(),
                variant_ids: vec![variant.id.clone()],
                selected_variant_id: None,
            });
            generate_ids.push(generate_id);
        }
        if item.variants.is_empty() {
            let prompt_id = format!("shot:{shot_id}:revision:{}:prompt", item.shot.revision);
            nodes.push(VideoWorkflowDagNode {
                id: prompt_id,
                kind: VideoWorkflowDagNodeKind::Prompt,
                shot_id: shot_id.clone(),
                depends_on: previous_select.iter().cloned().collect(),
                variant_ids: Vec::new(),
                selected_variant_id: None,
            });
            for input in &item.shot.input_assets {
                nodes.push(VideoWorkflowDagNode {
                    id: format!(
                        "shot:{shot_id}:revision:{}:reference:{}:{}",
                        item.shot.revision,
                        video_input_role_name(input.role),
                        input.content_hash_sha256.as_deref().unwrap_or("unverified")
                    ),
                    kind: VideoWorkflowDagNodeKind::ReferenceAsset,
                    shot_id: shot_id.clone(),
                    depends_on: Vec::new(),
                    variant_ids: Vec::new(),
                    selected_variant_id: None,
                });
            }
        }
        if !generate_ids.is_empty() {
            let select_id = format!("shot:{shot_id}:select");
            nodes.push(VideoWorkflowDagNode {
                id: select_id.clone(),
                kind: VideoWorkflowDagNodeKind::SelectVariant,
                shot_id: shot_id.clone(),
                depends_on: generate_ids,
                variant_ids: item
                    .variants
                    .iter()
                    .map(|variant| variant.id.clone())
                    .collect(),
                selected_variant_id: item.shot.selected_variant_id.clone(),
            });
            previous_select = Some(select_id);
        }
    }
    VideoWorkflowDag {
        workflow_id: workflow.id.clone(),
        revision: workflow.revision,
        nodes,
    }
}

fn load_variants(
    conn: &Connection,
    workflow_id: &str,
    shot_id: &str,
) -> Result<Vec<VideoWorkflowVariantRecord>, CoreError> {
    let mut statement = conn.prepare(
        "SELECT json_object(
             'id', variants.id, 'workflowId', variants.workflow_id,
             'shotId', variants.shot_id, 'ordinal', variants.ordinal,
             'jobId', variants.job_id, 'label', variants.label,
             'createdAt', variants.created_at,
             'job', json_object(
                 'id', jobs.id, 'state', jobs.state, 'revision', jobs.revision,
                 'providerId', jobs.provider_id, 'providerSource', jobs.provider_source,
                 'modelId', jobs.model_id, 'currentAttemptId', jobs.current_attempt_id,
                 'currentProviderTaskId', jobs.current_provider_task_id,
                 'retryCount', jobs.retry_count, 'maxAttempts', jobs.max_attempts,
                 'estimatedCostMicros', jobs.estimated_cost_micros,
                 'finalCostMicros', jobs.final_cost_micros, 'currency', jobs.currency,
                 'cancellationRequestedAt', jobs.cancellation_requested_at,
                  'error', CASE WHEN attempts.error_json IS NULL THEN NULL ELSE json(attempts.error_json) END,
                  'retryClassification', attempts.retry_classification,
                  'nextEligibleAt', attempts.next_eligible_at,
                 'outputAssetId', outputs.asset_id, 'outputMediaType', outputs.media_type,
                 'createdAt', jobs.created_at, 'updatedAt', jobs.updated_at
             )
         ), variants.shot_snapshot_json
         FROM video_workflow_variants AS variants
         JOIN media_jobs AS jobs ON jobs.id = variants.job_id
         LEFT JOIN media_job_attempts AS attempts ON attempts.id = jobs.current_attempt_id
         LEFT JOIN (
             SELECT relations.job_id, relations.asset_id, assets.media_type
             FROM media_asset_relations AS relations
             JOIN media_assets AS assets ON assets.id = relations.asset_id
             WHERE relations.relation_type = 'output' AND relations.ordinal = 0
               AND assets.local_state = 'available'
         ) AS outputs ON outputs.job_id = jobs.id
         WHERE variants.workflow_id = ?1 AND variants.shot_id = ?2
         ORDER BY variants.ordinal, variants.id",
    )?;
    let variants = statement
        .query_map(rusqlite::params![workflow_id, shot_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .map(|row| {
            let (value, shot_snapshot_json) = row?;
            let mut variant: VideoWorkflowVariantRecord =
                serde_json::from_str(&value).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        value.len(),
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
            variant.shot_snapshot =
                Some(serde_json::from_str(&shot_snapshot_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        shot_snapshot_json.len(),
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?);
            Ok::<_, rusqlite::Error>(variant)
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(CoreError::from)?;
    Ok(variants)
}

fn check_workflow_revision(
    conn: &Connection,
    workflow_id: &str,
    expected_revision: u64,
) -> Result<(), CoreError> {
    let actual: Option<u64> = conn
        .query_row(
            "SELECT revision FROM video_workflows WHERE id = ?1",
            [workflow_id],
            |row| row.get(0),
        )
        .optional()?;
    match actual {
        None => Err(CoreError::NotFound(format!("Video workflow {workflow_id}"))),
        Some(actual) if actual != expected_revision => Err(CoreError::Conflict(format!(
            "Video workflow revision changed from {expected_revision} to {actual}"
        ))),
        Some(_) => Ok(()),
    }
}

fn bump_workflow(conn: &Connection, workflow_id: &str) -> Result<(), CoreError> {
    conn.execute(
        "UPDATE video_workflows SET revision = revision + 1,
         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?1",
        [workflow_id],
    )?;
    Ok(())
}

fn query_json_optional<T: DeserializeOwned>(
    conn: &Connection,
    sql: &str,
    parameter: &str,
) -> Result<Option<T>, CoreError> {
    let value = conn
        .query_row(sql, [parameter], |row| row.get::<_, String>(0))
        .optional()?;
    value
        .map(|value| serde_json::from_str(&value).map_err(CoreError::from))
        .transpose()
}

fn video_input_role_name(role: VideoInputRole) -> &'static str {
    match role {
        VideoInputRole::FirstFrame => "first_frame",
        VideoInputRole::LastFrame => "last_frame",
        VideoInputRole::InputVideo => "input_video",
        VideoInputRole::ReferenceImage => "reference_image",
        VideoInputRole::ReferenceVideo => "reference_video",
        VideoInputRole::ReferenceAudio => "reference_audio",
    }
}

fn official_base_url(provider_id: &str) -> Result<&'static str, CoreError> {
    match provider_id {
        "minimax" => Ok("https://api.minimax.io"),
        "runway" => Ok("https://api.dev.runwayml.com"),
        _ => Err(CoreError::InvalidInput(
            "Only official MiniMax and Runway video connections are supported".to_string(),
        )),
    }
}

fn required(value: &str, field: &str, max_bytes: usize) -> Result<String, CoreError> {
    let value = value.trim();
    if value.is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(CoreError::InvalidInput(format!(
            "{field} must contain 1-{max_bytes} non-control bytes"
        )));
    }
    Ok(value.to_string())
}

fn optional_bounded(
    value: Option<String>,
    field: &str,
    max_bytes: usize,
) -> Result<Option<String>, CoreError> {
    value
        .map(|value| required(&value, field, max_bytes))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_generation::adapters::{MiniMaxVideoAdapter, VideoGenerationAdapter};

    fn database() -> Database {
        Database::open_memory().expect("workflow test database")
    }

    fn connection(database: &Database) -> VideoProviderConnectionRecord {
        save_provider_connection(
            database,
            SaveVideoProviderConnectionRequest {
                id: None,
                expected_revision: None,
                provider_id: "minimax".to_string(),
                display_name: "Studio account".to_string(),
                api_key: "secret-for-tests".to_string(),
                data_region: Some("provider_managed".to_string()),
            },
        )
        .expect("provider connection")
    }

    fn workflow(database: &Database) -> VideoWorkflowSnapshot {
        create_workflow(
            database,
            CreateVideoWorkflowRequest {
                project_id: Some("project-a".to_string()),
                title: "Launch film".to_string(),
                brief: json!({ "theme": "calm product reveal" }),
                aspect_ratio: "16:9".to_string(),
                target_duration_ms: 30_000,
            },
        )
        .expect("workflow")
    }

    fn shot(connection: &VideoProviderConnectionRecord) -> VideoShotInput {
        VideoShotInput {
            title: "Opening".to_string(),
            prompt: "A clean studio product reveal with soft light".to_string(),
            operation: MediaOperation::TextToVideo,
            connection_id: Some(connection.id.clone()),
            provider_id: Some("minimax".to_string()),
            model_id: Some("MiniMax-H3".to_string()),
            api_version: Some("v2".to_string()),
            duration_seconds: 4,
            resolution: "768P".to_string(),
            aspect_ratio: "16:9".to_string(),
            input_assets: Vec::new(),
            seed: None,
            generate_audio: None,
            allow_cross_provider_fallback: false,
        }
    }

    #[test]
    fn credentials_are_encrypted_and_never_projected() {
        let database = database();
        let saved = connection(&database);
        assert_eq!(saved.provider_id, "minimax");
        let serialized = serde_json::to_string(&saved).unwrap();
        assert!(!serialized.contains("secret-for-tests"));

        let materialized = materialize_provider_connection(&database, &saved.id).unwrap();
        assert_eq!(materialized.api_key, "secret-for-tests");
        let ciphertext: String = database
            .conn()
            .query_row(
                "SELECT credential_ciphertext FROM video_provider_connections WHERE id = ?1",
                [&saved.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_ne!(ciphertext, "secret-for-tests");
    }

    #[test]
    fn shot_variants_are_ordered_durable_and_batch_idempotent() {
        let database = database();
        let connection = connection(&database);
        let workflow = workflow(&database);
        let with_shot = add_shot(
            &database,
            AddVideoWorkflowShotRequest {
                workflow_id: workflow.workflow.id.clone(),
                expected_workflow_revision: workflow.workflow.revision,
                shot: shot(&connection),
            },
        )
        .unwrap();
        let durable_shot = &with_shot.shots[0].shot;
        let adapter =
            MiniMaxVideoAdapter::new("secret-for-tests", &connection.credential_scope).unwrap();
        let normalized = NormalizedVideoRequest {
            idempotency_key: "queue-batch".to_string(),
            model_id: "MiniMax-H3".to_string(),
            operation: MediaOperation::TextToVideo,
            prompt: durable_shot.prompt.clone(),
            duration_seconds: 4,
            resolution: "768P".to_string(),
            aspect_ratio: "16:9".to_string(),
            input_assets: Vec::new(),
            seed: None,
            generate_audio: None,
            callback_url: None,
        };
        let mut request = EnqueuePreparedVideoVariantsRequest {
            workflow_id: workflow.workflow.id.clone(),
            expected_workflow_revision: with_shot.workflow.revision,
            shot_id: durable_shot.id.clone(),
            expected_shot_revision: durable_shot.revision,
            idempotency_key: "queue-batch".to_string(),
            count: 2,
            expected_connection_revision: connection.revision,
            provider_source: adapter.provider_source().to_string(),
            normalized_request: normalized,
            estimated_cost_micros: Some(320_000),
            currency: Some("USD".to_string()),
        };
        let revised_connection = save_provider_connection(
            &database,
            SaveVideoProviderConnectionRequest {
                id: Some(connection.id.clone()),
                expected_revision: Some(connection.revision),
                provider_id: connection.provider_id.clone(),
                display_name: "Rotated before queue".to_string(),
                api_key: "rotated-secret-for-tests".to_string(),
                data_region: connection.data_region.clone(),
            },
        )
        .unwrap();
        let stale = enqueue_variants(&database, request.clone()).unwrap_err();
        assert!(matches!(stale, CoreError::Conflict(_)));
        request.expected_connection_revision = revised_connection.revision;
        let queued = enqueue_variants(&database, request.clone()).unwrap();
        assert_eq!(queued.shots[0].variants.len(), 2);
        assert_eq!(queued.shots[0].variants[0].ordinal, 0);
        assert_eq!(queued.shots[0].variants[1].ordinal, 1);
        assert_eq!(queued.queue.draft, 2);
        assert_eq!(queued.queue.estimated_cost_micros, 640_000);

        let replayed = enqueue_variants(&database, request).unwrap();
        assert_eq!(replayed.workflow.revision, queued.workflow.revision);
        assert_eq!(replayed.shots[0].variants.len(), 2);
        assert_eq!(
            replayed.shots[0].variants[0].job_id,
            queued.shots[0].variants[0].job_id
        );

        let job_id = &queued.shots[0].variants[0].job_id;
        assert!(try_acquire_job_lease(&database, job_id, "observe", "owner-a", 600).unwrap());
        assert!(!try_acquire_job_lease(&database, job_id, "observe", "owner-b", 600).unwrap());
        assert!(renew_job_lease(&database, job_id, "observe", "owner-a", 600).unwrap());
        release_job_lease(&database, job_id, "observe", "owner-a").unwrap();
        assert!(try_acquire_job_lease(&database, job_id, "observe", "owner-b", 600).unwrap());
        release_job_lease(&database, job_id, "observe", "owner-b").unwrap();

        for expected in 1..=12 {
            assert_eq!(
                increment_materialization_failure(&database, job_id).unwrap(),
                expected
            );
        }
        assert_eq!(
            increment_materialization_failure(&database, job_id).unwrap(),
            12
        );
        database
            .conn()
            .execute(
                "UPDATE media_jobs SET state = 'post_processing' WHERE id = ?1",
                [job_id],
            )
            .unwrap();
        assert!(!list_resumable_variant_contexts(&database)
            .unwrap()
            .iter()
            .any(|context| context.job_id == *job_id));
        reset_materialization_failures(&database, job_id).unwrap();
        assert_eq!(materialization_failure_count(&database, job_id).unwrap(), 0);
        assert!(list_resumable_variant_contexts(&database)
            .unwrap()
            .iter()
            .any(|context| context.job_id == *job_id));
    }

    #[test]
    fn selection_rejects_an_incomplete_variant() {
        let database = database();
        let connection = connection(&database);
        let workflow = workflow(&database);
        let with_shot = add_shot(
            &database,
            AddVideoWorkflowShotRequest {
                workflow_id: workflow.workflow.id,
                expected_workflow_revision: workflow.workflow.revision,
                shot: shot(&connection),
            },
        )
        .unwrap();
        let shot = with_shot.shots[0].shot.clone();
        let adapter =
            MiniMaxVideoAdapter::new("secret-for-tests", &connection.credential_scope).unwrap();
        let queued = enqueue_variants(
            &database,
            EnqueuePreparedVideoVariantsRequest {
                workflow_id: with_shot.workflow.id.clone(),
                expected_workflow_revision: with_shot.workflow.revision,
                shot_id: shot.id.clone(),
                expected_shot_revision: shot.revision,
                idempotency_key: "single-variant".to_string(),
                count: 1,
                expected_connection_revision: connection.revision,
                provider_source: adapter.provider_source().to_string(),
                normalized_request: NormalizedVideoRequest {
                    idempotency_key: "single-variant".to_string(),
                    model_id: shot.model_id.clone().unwrap(),
                    operation: shot.operation,
                    prompt: shot.prompt.clone(),
                    duration_seconds: shot.duration_seconds,
                    resolution: shot.resolution.clone(),
                    aspect_ratio: shot.aspect_ratio.clone(),
                    input_assets: Vec::new(),
                    seed: None,
                    generate_audio: None,
                    callback_url: None,
                },
                estimated_cost_micros: Some(320_000),
                currency: Some("USD".to_string()),
            },
        )
        .unwrap();
        let error = select_variant(
            &database,
            SelectVideoWorkflowVariantRequest {
                workflow_id: queued.workflow.id.clone(),
                expected_workflow_revision: queued.workflow.revision,
                shot_id: shot.id,
                expected_shot_revision: shot.revision,
                variant_id: queued.shots[0].variants[0].id.clone(),
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("Only a completed variant"));
    }

    #[test]
    fn provider_unknown_variants_still_consume_the_global_active_cap() {
        let database = database();
        let connection = connection(&database);
        let workflow = workflow(&database);
        let mut snapshot = add_shot(
            &database,
            AddVideoWorkflowShotRequest {
                workflow_id: workflow.workflow.id,
                expected_workflow_revision: workflow.workflow.revision,
                shot: shot(&connection),
            },
        )
        .unwrap();
        let durable_shot = snapshot.shots[0].shot.clone();
        let adapter =
            MiniMaxVideoAdapter::new("secret-for-tests", &connection.credential_scope).unwrap();
        for batch in 0..6 {
            snapshot = enqueue_variants(
                &database,
                EnqueuePreparedVideoVariantsRequest {
                    workflow_id: snapshot.workflow.id.clone(),
                    expected_workflow_revision: snapshot.workflow.revision,
                    shot_id: durable_shot.id.clone(),
                    expected_shot_revision: durable_shot.revision,
                    idempotency_key: format!("active-cap-{batch}"),
                    count: 4,
                    expected_connection_revision: connection.revision,
                    provider_source: adapter.provider_source().to_string(),
                    normalized_request: NormalizedVideoRequest {
                        idempotency_key: format!("active-cap-{batch}"),
                        model_id: "MiniMax-H3".to_string(),
                        operation: MediaOperation::TextToVideo,
                        prompt: durable_shot.prompt.clone(),
                        duration_seconds: 4,
                        resolution: "768P".to_string(),
                        aspect_ratio: "16:9".to_string(),
                        input_assets: Vec::new(),
                        seed: None,
                        generate_audio: None,
                        callback_url: None,
                    },
                    estimated_cost_micros: None,
                    currency: None,
                },
            )
            .unwrap();
            database
                .conn()
                .execute(
                    "UPDATE media_jobs SET state = 'provider_unknown'
                     WHERE id IN (SELECT job_id FROM video_workflow_variants WHERE workflow_id = ?1)",
                    [&snapshot.workflow.id],
                )
                .unwrap();
        }
        let rejected = enqueue_variants(
            &database,
            EnqueuePreparedVideoVariantsRequest {
                workflow_id: snapshot.workflow.id.clone(),
                expected_workflow_revision: snapshot.workflow.revision,
                shot_id: durable_shot.id,
                expected_shot_revision: durable_shot.revision,
                idempotency_key: "active-cap-overflow".to_string(),
                count: 1,
                expected_connection_revision: connection.revision,
                provider_source: adapter.provider_source().to_string(),
                normalized_request: NormalizedVideoRequest {
                    idempotency_key: "active-cap-overflow".to_string(),
                    model_id: "MiniMax-H3".to_string(),
                    operation: MediaOperation::TextToVideo,
                    prompt: durable_shot.prompt,
                    duration_seconds: 4,
                    resolution: "768P".to_string(),
                    aspect_ratio: "16:9".to_string(),
                    input_assets: Vec::new(),
                    seed: None,
                    generate_audio: None,
                    callback_url: None,
                },
                estimated_cost_micros: None,
                currency: None,
            },
        )
        .unwrap_err();
        assert!(rejected.to_string().contains("24 active variants"));
    }
}
