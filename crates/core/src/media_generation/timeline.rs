use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::db::Database;
use crate::error::CoreError;

const MAX_TIMELINE_CLIPS: i64 = 200;
const MAX_EXPORT_DURATION_US: i64 = 3_600_000_000;
const MAX_ID_BYTES: usize = 160;
const MAX_DESTINATION_BYTES: usize = 4096;
const EXPORT_LEASE_SECONDS: i64 = 600;
const MAX_ACTIVE_EXPORTS: i64 = 8;
const MAX_EXPORT_HISTORY: i64 = 50;
const MAX_CONCURRENT_EXPORTS: i64 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoTimelineRecord {
    pub id: String,
    pub workflow_id: String,
    pub schema_version: u32,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoTimelineClipRecord {
    pub id: String,
    pub timeline_id: String,
    pub shot_id: String,
    pub shot_title: String,
    pub variant_id: String,
    pub selected_variant_id: Option<String>,
    pub asset_id: String,
    pub asset_content_hash: String,
    pub media_type: String,
    pub ordinal: u32,
    pub source_start_us: u64,
    pub source_duration_us: u64,
    pub available_duration_us: u64,
    pub stale: bool,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoTimelineExportState {
    Validating,
    Queued,
    Running,
    Verifying,
    Publishing,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl VideoTimelineExportState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Validating => "validating",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Verifying => "verifying",
            Self::Publishing => "publishing",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoTimelineExportStageKind {
    Validate,
    Normalize,
    Concatenate,
    Verify,
    Publish,
}

impl VideoTimelineExportStageKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Validate => "validate",
            Self::Normalize => "normalize",
            Self::Concatenate => "concatenate",
            Self::Verify => "verify",
            Self::Publish => "publish",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoTimelineExportStageState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoTimelineOutputProfile {
    pub schema_version: u32,
    pub width: u32,
    pub height: u32,
    pub fit: String,
    pub fps_numerator: u32,
    pub fps_denominator: u32,
    pub pixel_format: String,
    pub video_codec: String,
    pub video_profile: String,
    pub video_level: u8,
    pub video_time_base_numerator: u32,
    pub video_time_base_denominator: u32,
    pub color_primaries: String,
    pub color_transfer: String,
    pub color_space: String,
    pub color_range: String,
    pub video_preset: String,
    pub video_crf: u8,
    pub audio_codec: String,
    pub audio_sample_rate: u32,
    pub audio_channel_layout: String,
}

impl VideoTimelineOutputProfile {
    pub fn validate(&self) -> Result<(), CoreError> {
        let dimensions_ok = self.width.is_multiple_of(2)
            && self.height.is_multiple_of(2)
            && (320..=3840).contains(&self.width)
            && (320..=2160).contains(&self.height)
            && u64::from(self.width) * u64::from(self.height) <= 8_294_400;
        if self.schema_version != 1 || !dimensions_ok {
            return Err(CoreError::InvalidInput(
                "Export profile must use schema v1 and an even 320..3840 x 320..2160 frame no larger than 4K"
                    .to_string(),
            ));
        }
        if self.fit != "contain"
            || self.pixel_format != "yuv420p"
            || self.video_codec != "h264"
            || self.video_profile != "high"
            || self.video_level != 52
            || self.video_time_base_numerator != 1
            || self.video_time_base_denominator != 90_000
            || self.color_primaries != "bt709"
            || self.color_transfer != "bt709"
            || self.color_space != "bt709"
            || self.color_range != "tv"
            || self.audio_codec != "aac"
            || self.audio_sample_rate != 48_000
            || self.audio_channel_layout != "stereo"
            || !matches!(self.video_preset.as_str(), "medium" | "fast")
            || !(18..=28).contains(&self.video_crf)
        {
            return Err(CoreError::InvalidInput(
                "Export profile contains a value outside the PR16 allowlist".to_string(),
            ));
        }
        if !matches!(
            (self.fps_numerator, self.fps_denominator),
            (24, 1) | (25, 1) | (30, 1) | (30_000, 1001) | (50, 1) | (60, 1)
        ) {
            return Err(CoreError::InvalidInput(
                "Export frame rate must be 24, 25, 30, 30000/1001, 50, or 60 fps".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoTimelineExportClipSnapshot {
    pub ordinal: u32,
    pub clip_id: String,
    pub clip_revision: u64,
    pub shot_id: String,
    pub shot_title: String,
    pub variant_id: String,
    pub asset_id: String,
    pub asset_content_hash: String,
    pub source_start_us: u64,
    pub source_duration_us: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoTimelineExportStageRecord {
    pub ordinal: u32,
    pub stage_kind: VideoTimelineExportStageKind,
    pub clip_ordinal: Option<u32>,
    pub state: VideoTimelineExportStageState,
    pub fingerprint_sha256: String,
    pub attempt_count: u32,
    pub progress_basis_points: u32,
    pub intermediate_asset_id: Option<String>,
    pub error: Option<Value>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoTimelineExportRecord {
    pub id: String,
    pub workflow_id: String,
    pub timeline_id: String,
    pub timeline_revision: u64,
    pub state: VideoTimelineExportState,
    pub current_stage: VideoTimelineExportStageKind,
    pub output_profile: VideoTimelineOutputProfile,
    pub ffmpeg_identity: Option<Value>,
    pub clips: Vec<VideoTimelineExportClipSnapshot>,
    pub input_fingerprint_sha256: String,
    pub destination_path: String,
    pub progress_basis_points: u32,
    pub output_asset_id: Option<String>,
    pub cancellation_requested_at: Option<String>,
    pub error: Option<Value>,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub stages: Vec<VideoTimelineExportStageRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoTimelineSnapshot {
    pub timeline: VideoTimelineRecord,
    pub clips: Vec<VideoTimelineClipRecord>,
    pub exports: Vec<VideoTimelineExportRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddVideoTimelineClipRequest {
    pub workflow_id: String,
    pub expected_timeline_revision: u64,
    pub shot_id: String,
    pub variant_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshVideoTimelineClipRequest {
    pub workflow_id: String,
    pub expected_timeline_revision: u64,
    pub clip_id: String,
    pub expected_clip_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateVideoTimelineClipRequest {
    pub workflow_id: String,
    pub expected_timeline_revision: u64,
    pub clip_id: String,
    pub expected_clip_revision: u64,
    pub source_start_us: u64,
    pub source_duration_us: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderVideoTimelineClipsRequest {
    pub workflow_id: String,
    pub expected_timeline_revision: u64,
    pub ordered_clip_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveVideoTimelineClipRequest {
    pub workflow_id: String,
    pub expected_timeline_revision: u64,
    pub clip_id: String,
    pub expected_clip_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateVideoTimelineExportRequest {
    pub workflow_id: String,
    pub expected_timeline_revision: u64,
    pub idempotency_key: String,
    pub destination_path: String,
    pub output_profile: VideoTimelineOutputProfile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelVideoTimelineExportRequest {
    pub export_id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryVideoTimelineExportRequest {
    pub export_id: String,
    pub expected_revision: u64,
    pub destination_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VideoTimelineExportInputPlan {
    pub ordinal: u32,
    pub asset_id: String,
    pub source_start_us: u64,
    pub source_duration_us: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VideoTimelineExportExecutionPlan {
    pub export: VideoTimelineExportRecord,
    pub inputs: Vec<VideoTimelineExportInputPlan>,
}

pub(crate) fn get_timeline(
    database: &Database,
    workflow_id: &str,
) -> Result<VideoTimelineSnapshot, CoreError> {
    let conn = database.conn();
    load_timeline_snapshot(&conn, workflow_id)
}

pub(crate) fn add_clip(
    database: &Database,
    mut request: AddVideoTimelineClipRequest,
) -> Result<VideoTimelineSnapshot, CoreError> {
    request.workflow_id = required(&request.workflow_id, "workflow_id")?;
    request.shot_id = required(&request.shot_id, "shot_id")?;
    request.variant_id = required(&request.variant_id, "variant_id")?;
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let timeline = check_timeline_revision(
        &tx,
        &request.workflow_id,
        request.expected_timeline_revision,
    )?;
    let count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM video_timeline_clips WHERE timeline_id = ?1",
        [&timeline.id],
        |row| row.get(0),
    )?;
    if count >= MAX_TIMELINE_CLIPS {
        return Err(CoreError::InvalidInput(
            "A timeline cannot contain more than 200 clips".to_string(),
        ));
    }
    let source = selected_source(
        &tx,
        &request.workflow_id,
        &request.shot_id,
        &request.variant_id,
    )?;
    let ordinal: i64 = tx.query_row(
        "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM video_timeline_clips WHERE timeline_id = ?1",
        [&timeline.id],
        |row| row.get(0),
    )?;
    let changed = tx.execute(
        "INSERT INTO video_timeline_clips
         (id, timeline_id, shot_id, variant_id, asset_id, ordinal, source_duration_us)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            Uuid::new_v4().to_string(),
            timeline.id,
            request.shot_id,
            request.variant_id,
            source.asset_id,
            ordinal,
            source.available_duration_us,
        ],
    )?;
    if changed != 1 {
        return Err(CoreError::Conflict(
            "Timeline clip could not be added after state change".to_string(),
        ));
    }
    bump_timeline(&tx, &timeline.id)?;
    let snapshot = load_timeline_snapshot(&tx, &request.workflow_id)?;
    tx.commit()?;
    Ok(snapshot)
}

pub(crate) fn refresh_clip(
    database: &Database,
    mut request: RefreshVideoTimelineClipRequest,
) -> Result<VideoTimelineSnapshot, CoreError> {
    request.workflow_id = required(&request.workflow_id, "workflow_id")?;
    request.clip_id = required(&request.clip_id, "clip_id")?;
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let timeline = check_timeline_revision(
        &tx,
        &request.workflow_id,
        request.expected_timeline_revision,
    )?;
    let (shot_id, clip_revision): (String, u64) = tx
        .query_row(
            "SELECT shot_id, revision FROM video_timeline_clips
             WHERE id = ?1 AND timeline_id = ?2",
            rusqlite::params![request.clip_id, timeline.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| CoreError::NotFound(format!("Video timeline clip {}", request.clip_id)))?;
    if clip_revision != request.expected_clip_revision {
        return Err(CoreError::Conflict(
            "Timeline clip changed before its selected source could be refreshed".to_string(),
        ));
    }
    let variant_id: String = tx
        .query_row(
            "SELECT selected_variant_id FROM video_workflow_shots WHERE id = ?1 AND workflow_id = ?2",
            rusqlite::params![shot_id, request.workflow_id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| CoreError::Conflict("The shot has no selected variant".to_string()))?;
    let source = selected_source(&tx, &request.workflow_id, &shot_id, &variant_id)?;
    let changed = tx.execute(
        "UPDATE video_timeline_clips
         SET variant_id = ?2, asset_id = ?3, source_start_us = 0,
             source_duration_us = ?4, revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        rusqlite::params![
            request.clip_id,
            variant_id,
            source.asset_id,
            source.available_duration_us
        ],
    )?;
    if changed != 1 {
        return Err(CoreError::Conflict(
            "Timeline clip changed before its selected source could be refreshed".to_string(),
        ));
    }
    bump_timeline(&tx, &timeline.id)?;
    let snapshot = load_timeline_snapshot(&tx, &request.workflow_id)?;
    tx.commit()?;
    Ok(snapshot)
}

pub(crate) fn update_clip(
    database: &Database,
    mut request: UpdateVideoTimelineClipRequest,
) -> Result<VideoTimelineSnapshot, CoreError> {
    request.workflow_id = required(&request.workflow_id, "workflow_id")?;
    request.clip_id = required(&request.clip_id, "clip_id")?;
    let start = i64::try_from(request.source_start_us)
        .map_err(|_| CoreError::InvalidInput("source_start_us is too large".to_string()))?;
    let duration = i64::try_from(request.source_duration_us)
        .map_err(|_| CoreError::InvalidInput("source_duration_us is too large".to_string()))?;
    if duration <= 0 || start.checked_add(duration).is_none() {
        return Err(CoreError::InvalidInput(
            "Timeline source range must have a positive non-overflowing duration".to_string(),
        ));
    }
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let timeline = check_timeline_revision(
        &tx,
        &request.workflow_id,
        request.expected_timeline_revision,
    )?;
    let (actual_revision, available_duration_us): (u64, i64) = tx
        .query_row(
            "SELECT clips.revision,
                    COALESCE(assets.duration_ms * 1000,
                             json_extract(variants.shot_snapshot_json, '$.durationSeconds') * 1000000)
             FROM video_timeline_clips AS clips
             JOIN media_assets AS assets ON assets.id = clips.asset_id
             JOIN video_workflow_variants AS variants ON variants.id = clips.variant_id
             WHERE clips.id = ?1 AND clips.timeline_id = ?2 AND assets.local_state = 'available'",
            rusqlite::params![request.clip_id, timeline.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| CoreError::Conflict("Timeline clip source is no longer available".to_string()))?;
    if actual_revision != request.expected_clip_revision {
        return Err(CoreError::Conflict(
            "Timeline clip changed before its range could be saved".to_string(),
        ));
    }
    if start + duration > available_duration_us {
        return Err(CoreError::InvalidInput(
            "Timeline source range exceeds the verified source duration".to_string(),
        ));
    }
    let changed = tx.execute(
        "UPDATE video_timeline_clips
         SET source_start_us = ?2, source_duration_us = ?3,
             revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        rusqlite::params![request.clip_id, start, duration],
    )?;
    if changed != 1 {
        return Err(CoreError::Conflict(
            "Timeline clip changed before its range could be saved".to_string(),
        ));
    }
    bump_timeline(&tx, &timeline.id)?;
    let snapshot = load_timeline_snapshot(&tx, &request.workflow_id)?;
    tx.commit()?;
    Ok(snapshot)
}

pub(crate) fn reorder_clips(
    database: &Database,
    mut request: ReorderVideoTimelineClipsRequest,
) -> Result<VideoTimelineSnapshot, CoreError> {
    request.workflow_id = required(&request.workflow_id, "workflow_id")?;
    if request.ordered_clip_ids.len() > MAX_TIMELINE_CLIPS as usize {
        return Err(CoreError::InvalidInput(
            "A timeline cannot contain more than 200 clips".to_string(),
        ));
    }
    let unique = request
        .ordered_clip_ids
        .iter()
        .collect::<std::collections::HashSet<_>>();
    if unique.len() != request.ordered_clip_ids.len() {
        return Err(CoreError::InvalidInput(
            "Timeline reorder cannot contain duplicate clip IDs".to_string(),
        ));
    }
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let timeline = check_timeline_revision(
        &tx,
        &request.workflow_id,
        request.expected_timeline_revision,
    )?;
    let current: Vec<String> = {
        let mut statement = tx.prepare(
            "SELECT id FROM video_timeline_clips WHERE timeline_id = ?1 ORDER BY ordinal, id",
        )?;
        let rows = statement
            .query_map([&timeline.id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    if current.len() != request.ordered_clip_ids.len()
        || current.iter().any(|id| !unique.contains(id))
    {
        return Err(CoreError::Conflict(
            "Timeline membership changed before reorder".to_string(),
        ));
    }
    tx.execute(
        "UPDATE video_timeline_clips SET ordinal = ordinal + 1000000 WHERE timeline_id = ?1",
        [&timeline.id],
    )?;
    for (ordinal, clip_id) in request.ordered_clip_ids.iter().enumerate() {
        tx.execute(
            "UPDATE video_timeline_clips
             SET ordinal = ?2, revision = revision + 1,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id = ?1 AND timeline_id = ?3",
            rusqlite::params![clip_id, ordinal as i64, timeline.id],
        )?;
    }
    bump_timeline(&tx, &timeline.id)?;
    let snapshot = load_timeline_snapshot(&tx, &request.workflow_id)?;
    tx.commit()?;
    Ok(snapshot)
}

pub(crate) fn remove_clip(
    database: &Database,
    mut request: RemoveVideoTimelineClipRequest,
) -> Result<VideoTimelineSnapshot, CoreError> {
    request.workflow_id = required(&request.workflow_id, "workflow_id")?;
    request.clip_id = required(&request.clip_id, "clip_id")?;
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let timeline = check_timeline_revision(
        &tx,
        &request.workflow_id,
        request.expected_timeline_revision,
    )?;
    let revision: u64 = tx
        .query_row(
            "SELECT revision FROM video_timeline_clips WHERE id = ?1 AND timeline_id = ?2",
            rusqlite::params![request.clip_id, timeline.id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| CoreError::NotFound(format!("Video timeline clip {}", request.clip_id)))?;
    if revision != request.expected_clip_revision {
        return Err(CoreError::Conflict(
            "Timeline clip changed before removal".to_string(),
        ));
    }
    let changed = tx.execute(
        "DELETE FROM video_timeline_clips WHERE id = ?1",
        [&request.clip_id],
    )?;
    if changed != 1 {
        return Err(CoreError::Conflict(
            "Timeline clip changed before removal".to_string(),
        ));
    }
    densify_clip_ordinals(&tx, &timeline.id)?;
    bump_timeline(&tx, &timeline.id)?;
    let snapshot = load_timeline_snapshot(&tx, &request.workflow_id)?;
    tx.commit()?;
    Ok(snapshot)
}

pub(crate) fn create_export(
    database: &Database,
    mut request: CreateVideoTimelineExportRequest,
    ffmpeg_identity: Value,
) -> Result<VideoTimelineExportRecord, CoreError> {
    request.workflow_id = required(&request.workflow_id, "workflow_id")?;
    request.idempotency_key = required(&request.idempotency_key, "idempotency_key")?;
    if request.destination_path.trim().len() > MAX_DESTINATION_BYTES {
        return Err(CoreError::InvalidInput(
            "Export destination is too long".to_string(),
        ));
    }
    request.output_profile.validate()?;
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(existing) =
        find_export_by_idempotency_conn(&tx, &request.workflow_id, &request.idempotency_key)?
    {
        if existing.destination_path != request.destination_path
            || existing.output_profile != request.output_profile
        {
            return Err(CoreError::Conflict(
                "Export idempotency key was already used for a different request".to_string(),
            ));
        }
        tx.commit()?;
        return Ok(existing);
    }
    let timeline = check_timeline_revision(
        &tx,
        &request.workflow_id,
        request.expected_timeline_revision,
    )?;
    let clips = load_clips(&tx, &timeline)?;
    if clips.is_empty() {
        return Err(CoreError::InvalidInput(
            "Add at least one selected clip before export".to_string(),
        ));
    }
    if clips.iter().any(|clip| clip.stale) {
        return Err(CoreError::Conflict(
            "Refresh stale timeline clips before export".to_string(),
        ));
    }
    let total_duration = clips.iter().try_fold(0_u64, |total, clip| {
        total
            .checked_add(clip.source_duration_us)
            .ok_or_else(|| CoreError::InvalidInput("Timeline duration overflowed".to_string()))
    })?;
    if total_duration == 0 || total_duration > MAX_EXPORT_DURATION_US as u64 {
        return Err(CoreError::InvalidInput(
            "Export duration must be between one microsecond and one hour".to_string(),
        ));
    }
    let snapshots: Vec<VideoTimelineExportClipSnapshot> = clips
        .iter()
        .map(|clip| VideoTimelineExportClipSnapshot {
            ordinal: clip.ordinal,
            clip_id: clip.id.clone(),
            clip_revision: clip.revision,
            shot_id: clip.shot_id.clone(),
            shot_title: clip.shot_title.clone(),
            variant_id: clip.variant_id.clone(),
            asset_id: clip.asset_id.clone(),
            asset_content_hash: clip.asset_content_hash.clone(),
            source_start_us: clip.source_start_us,
            source_duration_us: clip.source_duration_us,
        })
        .collect();
    let profile_json = serde_json::to_string(&request.output_profile)?;
    let clips_json = serde_json::to_string(&snapshots)?;
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&json!({
            "timelineId": timeline.id,
            "timelineRevision": timeline.revision,
            "profile": request.output_profile,
            "clips": snapshots,
            "ffmpegIdentity": ffmpeg_identity.clone(),
        }))?)
    );
    let active_count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM video_timeline_exports
         WHERE state NOT IN ('completed', 'failed', 'cancelled')",
        [],
        |row| row.get(0),
    )?;
    if active_count >= MAX_ACTIVE_EXPORTS {
        return Err(CoreError::Conflict(format!(
            "At most {MAX_ACTIVE_EXPORTS} video timeline exports may be active or queued"
        )));
    }
    let export_id = Uuid::new_v4().to_string();
    let ffmpeg_identity_json = serde_json::to_string(&ffmpeg_identity)?;
    let changed = tx.execute(
        "INSERT INTO video_timeline_exports
         (id, workflow_id, timeline_id, timeline_revision, idempotency_key,
          state, current_stage, output_profile_json, ffmpeg_identity_json,
          clip_snapshot_json, input_fingerprint_sha256, destination_path)
         VALUES (?1, ?2, ?3, ?4, ?5, 'validating', 'validate', ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            export_id,
            request.workflow_id,
            timeline.id,
            timeline.revision,
            request.idempotency_key,
            profile_json,
            ffmpeg_identity_json,
            clips_json,
            fingerprint,
            request.destination_path,
        ],
    )?;
    if changed != 1 {
        return Err(CoreError::Conflict(
            "Video timeline export could not be created".to_string(),
        ));
    }
    for snapshot in &snapshots {
        tx.execute(
            "INSERT INTO video_timeline_export_inputs
             (export_id, asset_id, asset_content_hash, ordinal, clip_id, variant_id,
              source_start_us, source_duration_us)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                export_id,
                snapshot.asset_id,
                snapshot.asset_content_hash,
                snapshot.ordinal,
                snapshot.clip_id,
                snapshot.variant_id,
                snapshot.source_start_us,
                snapshot.source_duration_us,
            ],
        )?;
    }
    insert_export_stages(&tx, &export_id, &fingerprint, snapshots.len())?;
    let record = load_export(&tx, &export_id)?;
    tx.commit()?;
    Ok(record)
}

pub(crate) fn find_export_by_idempotency(
    database: &Database,
    workflow_id: &str,
    idempotency_key: &str,
) -> Result<Option<VideoTimelineExportRecord>, CoreError> {
    let workflow_id = required(workflow_id, "workflow_id")?;
    let idempotency_key = required(idempotency_key, "idempotency_key")?;
    find_export_by_idempotency_conn(&database.conn(), &workflow_id, &idempotency_key)
}

pub(crate) fn request_export_cancellation(
    database: &Database,
    mut request: CancelVideoTimelineExportRequest,
) -> Result<VideoTimelineExportRecord, CoreError> {
    request.export_id = required(&request.export_id, "export_id")?;
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let record = load_export(&tx, &request.export_id)?;
    if record.revision != request.expected_revision {
        return Err(CoreError::Conflict(
            "Export changed before cancellation was requested".to_string(),
        ));
    }
    if matches!(
        record.state,
        VideoTimelineExportState::Completed
            | VideoTimelineExportState::Failed
            | VideoTimelineExportState::Cancelled
    ) {
        return Err(CoreError::Conflict(
            "Only a non-terminal export can be cancelled".to_string(),
        ));
    }
    let changed = tx.execute(
        "UPDATE video_timeline_exports
         SET cancellation_requested_at = COALESCE(cancellation_requested_at,
                 strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1 AND revision = ?2
           AND publication_commit_started_at IS NULL
           AND state NOT IN ('completed', 'failed', 'cancelled')",
        rusqlite::params![request.export_id, request.expected_revision],
    )?;
    if changed != 1 {
        return Err(CoreError::Conflict(
            "Export changed before cancellation was committed".to_string(),
        ));
    }
    let cancelled = load_export(&tx, &request.export_id)?;
    tx.commit()?;
    Ok(cancelled)
}

pub(crate) fn retry_export(
    database: &Database,
    mut request: RetryVideoTimelineExportRequest,
) -> Result<VideoTimelineExportRecord, CoreError> {
    request.export_id = required(&request.export_id, "export_id")?;
    if request.destination_path.trim().is_empty()
        || request.destination_path.len() > MAX_DESTINATION_BYTES
    {
        return Err(CoreError::InvalidInput(
            "Choose a bounded MP4 export destination".to_string(),
        ));
    }
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let record = load_export(&tx, &request.export_id)?;
    if record.revision != request.expected_revision {
        return Err(CoreError::Conflict(
            "Export changed before retry was requested".to_string(),
        ));
    }
    if !matches!(
        record.state,
        VideoTimelineExportState::Failed | VideoTimelineExportState::Interrupted
    ) {
        return Err(CoreError::Conflict(
            "Only a failed or interrupted export can be retried".to_string(),
        ));
    }
    let other_active: i64 = tx.query_row(
        "SELECT COUNT(*) FROM video_timeline_exports
         WHERE id <> ?1 AND state NOT IN ('completed', 'failed', 'cancelled')",
        [&request.export_id],
        |row| row.get(0),
    )?;
    if other_active >= MAX_ACTIVE_EXPORTS {
        return Err(CoreError::Conflict(format!(
            "At most {MAX_ACTIVE_EXPORTS} video timeline exports may be active or queued"
        )));
    }
    let active_lease: bool = tx.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM video_timeline_export_leases
             WHERE export_id = ?1 AND expires_at_epoch > CAST(strftime('%s','now') AS INTEGER)
         )",
        [&request.export_id],
        |row| row.get(0),
    )?;
    if active_lease {
        return Err(CoreError::Conflict(
            "Wait for the previous export worker lease to expire before retrying".to_string(),
        ));
    }
    let has_verified_output = record.output_asset_id.is_some();
    if has_verified_output {
        tx.execute(
            "UPDATE video_timeline_export_stages
             SET state = CASE WHEN stage_kind = 'publish' THEN 'queued' ELSE state END,
                 progress_basis_points = CASE WHEN stage_kind = 'publish' THEN 0 ELSE progress_basis_points END,
                 error_json = CASE WHEN stage_kind = 'publish' THEN NULL ELSE error_json END,
                 started_at = CASE WHEN stage_kind = 'publish' THEN NULL ELSE started_at END,
                 completed_at = CASE WHEN stage_kind = 'publish' THEN NULL ELSE completed_at END,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE export_id = ?1",
            [&request.export_id],
        )?;
    } else {
        tx.execute(
            "UPDATE video_timeline_export_stages
             SET state = 'queued', progress_basis_points = 0, error_json = NULL,
                 intermediate_asset_id = NULL, started_at = NULL, completed_at = NULL,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE export_id = ?1",
            [&request.export_id],
        )?;
    }
    let changed = tx.execute(
        "UPDATE video_timeline_exports
         SET state = 'queued', current_stage = ?2,
             progress_basis_points = CASE WHEN output_asset_id IS NULL THEN 0 ELSE progress_basis_points END,
             destination_path = ?3, cancellation_requested_at = NULL,
             publication_commit_started_at = NULL,
             error_json = NULL, completed_at = NULL, revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        rusqlite::params![
            request.export_id,
            if has_verified_output {
                "publish"
            } else {
                "validate"
            },
            request.destination_path,
        ],
    )?;
    if changed != 1 {
        return Err(CoreError::Conflict(
            "Export changed before retry was committed".to_string(),
        ));
    }
    let retried = load_export(&tx, &request.export_id)?;
    tx.commit()?;
    Ok(retried)
}

pub(crate) fn export_execution_plan(
    database: &Database,
    export_id: &str,
) -> Result<VideoTimelineExportExecutionPlan, CoreError> {
    let conn = database.conn();
    let export = load_export(&conn, export_id)?;
    let inputs = {
        let mut statement = conn.prepare(
            "SELECT ordinal, asset_id, source_start_us, source_duration_us
             FROM video_timeline_export_inputs WHERE export_id = ?1 ORDER BY ordinal",
        )?;
        let rows = statement
            .query_map([export_id], |row| {
                Ok(VideoTimelineExportInputPlan {
                    ordinal: row.get(0)?,
                    asset_id: row.get(1)?,
                    source_start_us: row.get(2)?,
                    source_duration_us: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    Ok(VideoTimelineExportExecutionPlan { export, inputs })
}

pub(crate) fn list_resumable_exports(database: &Database) -> Result<Vec<String>, CoreError> {
    let conn = database.conn();
    let mut statement = conn.prepare(
        "SELECT id FROM video_timeline_exports
         WHERE state IN ('validating', 'queued', 'running', 'verifying', 'publishing', 'interrupted')
         ORDER BY created_at LIMIT ?1",
    )?;
    let rows = statement
        .query_map([MAX_ACTIVE_EXPORTS], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub(crate) fn try_acquire_export_lease(
    database: &Database,
    export_id: &str,
    owner_id: &str,
    now_epoch: i64,
) -> Result<bool, CoreError> {
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let expires = now_epoch.saturating_add(EXPORT_LEASE_SECONDS);
    let already_owned: bool = tx.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM video_timeline_export_leases
             WHERE export_id = ?1 AND owner_id = ?2 AND expires_at_epoch > ?3
         )",
        rusqlite::params![export_id, owner_id, now_epoch],
        |row| row.get(0),
    )?;
    if !already_owned {
        let active: i64 = tx.query_row(
            "SELECT COUNT(*) FROM video_timeline_export_leases
             WHERE expires_at_epoch > ?1",
            [now_epoch],
            |row| row.get(0),
        )?;
        if active >= MAX_CONCURRENT_EXPORTS {
            tx.commit()?;
            return Ok(false);
        }
    }
    let changed = tx.execute(
        "INSERT INTO video_timeline_export_leases
         (export_id, owner_id, expires_at_epoch) VALUES (?1, ?2, ?3)
         ON CONFLICT(export_id) DO UPDATE SET
             owner_id = excluded.owner_id,
             expires_at_epoch = excluded.expires_at_epoch,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE video_timeline_export_leases.owner_id = excluded.owner_id
            OR video_timeline_export_leases.expires_at_epoch <= ?4",
        rusqlite::params![export_id, owner_id, expires, now_epoch],
    )?;
    tx.commit()?;
    Ok(changed == 1)
}

pub(crate) fn renew_export_lease(
    database: &Database,
    export_id: &str,
    owner_id: &str,
    now_epoch: i64,
) -> Result<bool, CoreError> {
    let conn = database.conn();
    Ok(conn.execute(
        "UPDATE video_timeline_export_leases
         SET expires_at_epoch = ?3,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE export_id = ?1 AND owner_id = ?2",
        rusqlite::params![
            export_id,
            owner_id,
            now_epoch.saturating_add(EXPORT_LEASE_SECONDS)
        ],
    )? == 1)
}

pub(crate) fn release_export_lease(
    database: &Database,
    export_id: &str,
    owner_id: &str,
) -> Result<(), CoreError> {
    database.conn().execute(
        "DELETE FROM video_timeline_export_leases WHERE export_id = ?1 AND owner_id = ?2",
        rusqlite::params![export_id, owner_id],
    )?;
    Ok(())
}

pub(crate) fn export_cancel_requested(
    database: &Database,
    export_id: &str,
) -> Result<bool, CoreError> {
    let conn = database.conn();
    Ok(conn.query_row(
        "SELECT cancellation_requested_at IS NOT NULL FROM video_timeline_exports WHERE id = ?1",
        [export_id],
        |row| row.get(0),
    )?)
}

pub(crate) fn begin_export_stage(
    database: &Database,
    export_id: &str,
    owner_id: &str,
    now_epoch: i64,
    stage_ordinal: u32,
    stage_kind: VideoTimelineExportStageKind,
    export_state: VideoTimelineExportState,
) -> Result<(), CoreError> {
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_owned_lease(&tx, export_id, owner_id, now_epoch)?;
    let stage_changed = tx.execute(
        "UPDATE video_timeline_export_stages
         SET state = 'running', attempt_count = attempt_count + 1,
             progress_basis_points = 0, error_json = NULL,
             started_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'), completed_at = NULL,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE export_id = ?1 AND ordinal = ?2 AND stage_kind = ?3",
        rusqlite::params![export_id, stage_ordinal, stage_kind.as_str()],
    )?;
    let export_changed = tx.execute(
        "UPDATE video_timeline_exports
         SET state = ?2, current_stage = ?3, error_json = NULL,
             revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1 AND cancellation_requested_at IS NULL
           AND state NOT IN ('completed', 'failed', 'cancelled')",
        rusqlite::params![export_id, export_state.as_str(), stage_kind.as_str()],
    )?;
    if stage_changed != 1 || export_changed != 1 {
        return Err(CoreError::Conflict(
            "Export stage could not begin after cancellation or state change".to_string(),
        ));
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn record_export_progress(
    database: &Database,
    export_id: &str,
    owner_id: &str,
    now_epoch: i64,
    stage_ordinal: u32,
    stage_progress: u32,
    overall_progress: u32,
) -> Result<(), CoreError> {
    let stage_progress = stage_progress.min(10_000);
    let overall_progress = overall_progress.min(9_999);
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_owned_lease(&tx, export_id, owner_id, now_epoch)?;
    let stage_changed = tx.execute(
        "UPDATE video_timeline_export_stages
         SET progress_basis_points = MAX(progress_basis_points, ?3),
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE export_id = ?1 AND ordinal = ?2
           AND EXISTS (
               SELECT 1 FROM video_timeline_exports
               WHERE id = ?1 AND cancellation_requested_at IS NULL
                 AND state NOT IN ('completed', 'failed', 'cancelled')
           )",
        rusqlite::params![export_id, stage_ordinal, stage_progress],
    )?;
    let export_changed = tx.execute(
        "UPDATE video_timeline_exports
         SET progress_basis_points = MAX(progress_basis_points, ?2),
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1 AND cancellation_requested_at IS NULL
           AND state NOT IN ('completed', 'failed', 'cancelled')",
        rusqlite::params![export_id, overall_progress],
    )?;
    if stage_changed != 1 || export_changed != 1 {
        return Err(CoreError::Conflict(
            "Export progress lost its cancellation or terminal-state race".to_string(),
        ));
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn complete_export_stage(
    database: &Database,
    export_id: &str,
    owner_id: &str,
    now_epoch: i64,
    stage_ordinal: u32,
    intermediate_asset_id: Option<&str>,
) -> Result<(), CoreError> {
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_owned_lease(&tx, export_id, owner_id, now_epoch)?;
    let changed = tx.execute(
        "UPDATE video_timeline_export_stages
         SET state = 'completed', progress_basis_points = 10000,
             intermediate_asset_id = ?3, error_json = NULL,
             completed_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'),
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE export_id = ?1 AND ordinal = ?2
           AND EXISTS (
               SELECT 1 FROM video_timeline_exports
               WHERE id = ?1 AND cancellation_requested_at IS NULL
                 AND state NOT IN ('completed', 'cancelled')
           )",
        rusqlite::params![export_id, stage_ordinal, intermediate_asset_id],
    )?;
    if changed != 1 {
        return Err(CoreError::Conflict(
            "Export stage could not complete after cancellation or state change".to_string(),
        ));
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn begin_export_publication_commit(
    database: &Database,
    export_id: &str,
    owner_id: &str,
    now_epoch: i64,
) -> Result<(), CoreError> {
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_owned_lease(&tx, export_id, owner_id, now_epoch)?;
    let changed = tx.execute(
        "UPDATE video_timeline_exports
         SET publication_commit_started_at = COALESCE(publication_commit_started_at,
                 strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1 AND state = 'publishing'
           AND cancellation_requested_at IS NULL",
        [export_id],
    )?;
    if changed != 1 {
        return Err(CoreError::Conflict(
            "Export publication lost its cancellation or state race".to_string(),
        ));
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn mark_export_completed(
    database: &Database,
    export_id: &str,
    owner_id: &str,
    now_epoch: i64,
    output_asset_id: &str,
) -> Result<VideoTimelineExportRecord, CoreError> {
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_owned_lease(&tx, export_id, owner_id, now_epoch)?;
    let changed = tx.execute(
        "UPDATE video_timeline_exports
         SET state = 'completed', current_stage = 'publish', progress_basis_points = 10000,
             output_asset_id = ?2, error_json = NULL, completed_at = COALESCE(completed_at,
                 strftime('%Y-%m-%dT%H:%M:%fZ','now')),
             revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1 AND cancellation_requested_at IS NULL
           AND publication_commit_started_at IS NOT NULL
           AND state NOT IN ('completed', 'cancelled')",
        rusqlite::params![export_id, output_asset_id],
    )?;
    if changed != 1 {
        return Err(CoreError::Conflict(
            "Export was cancelled before publication committed".to_string(),
        ));
    }
    let completed = load_export(&tx, export_id)?;
    tx.commit()?;
    Ok(completed)
}

pub(crate) fn record_export_output_asset(
    database: &Database,
    export_id: &str,
    owner_id: &str,
    now_epoch: i64,
    output_asset_id: &str,
) -> Result<(), CoreError> {
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_owned_lease(&tx, export_id, owner_id, now_epoch)?;
    let changed = tx.execute(
        "UPDATE video_timeline_exports
         SET output_asset_id = ?2, revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1 AND cancellation_requested_at IS NULL
           AND state NOT IN ('completed', 'cancelled')",
        rusqlite::params![export_id, output_asset_id],
    )?;
    if changed != 1 {
        return Err(CoreError::Conflict(
            "Verified export output could not be linked after state change".to_string(),
        ));
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn mark_export_cancelled(
    database: &Database,
    export_id: &str,
    owner_id: &str,
    now_epoch: i64,
) -> Result<(), CoreError> {
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_owned_lease(&tx, export_id, owner_id, now_epoch)?;
    tx.execute(
        "UPDATE video_timeline_export_stages
         SET state = CASE WHEN state IN ('running', 'queued', 'interrupted') THEN 'cancelled' ELSE state END,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE export_id = ?1",
        [export_id],
    )?;
    let changed = tx.execute(
        "UPDATE video_timeline_exports
         SET state = 'cancelled', error_json = NULL, revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1 AND cancellation_requested_at IS NOT NULL
           AND state NOT IN ('completed', 'failed')",
        [export_id],
    )?;
    if changed != 1 {
        return Err(CoreError::Conflict(
            "Export cancellation lost its terminal-state race".to_string(),
        ));
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn mark_export_failed(
    database: &Database,
    export_id: &str,
    owner_id: &str,
    now_epoch: i64,
    stage_ordinal: u32,
    code: &str,
    message: &str,
) -> Result<(), CoreError> {
    let error = serde_json::to_string(&json!({
        "code": bounded(code, 80),
        "message": bounded(message, 4096),
    }))?;
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_owned_lease(&tx, export_id, owner_id, now_epoch)?;
    let stage_changed = tx.execute(
        "UPDATE video_timeline_export_stages
         SET state = 'failed', error_json = ?3,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE export_id = ?1 AND ordinal = ?2",
        rusqlite::params![export_id, stage_ordinal, error],
    )?;
    let export_changed = tx.execute(
        "UPDATE video_timeline_exports
         SET state = 'failed', error_json = ?2, revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1 AND cancellation_requested_at IS NULL
           AND state NOT IN ('completed', 'failed', 'cancelled')",
        rusqlite::params![export_id, error],
    )?;
    if stage_changed != 1 || export_changed != 1 {
        return Err(CoreError::Conflict(
            "Export failure lost its cancellation or terminal-state race".to_string(),
        ));
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn mark_live_exports_interrupted(
    database: &Database,
    now_epoch: i64,
) -> Result<usize, CoreError> {
    let mut conn = database.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute(
        "UPDATE video_timeline_export_stages
         SET state = 'interrupted',
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE state = 'running'
           AND export_id IN (
               SELECT exports.id FROM video_timeline_exports AS exports
               LEFT JOIN video_timeline_export_leases AS leases ON leases.export_id = exports.id
               WHERE leases.export_id IS NULL OR leases.expires_at_epoch <= ?1
           )",
        [now_epoch],
    )?;
    let changed = tx.execute(
        "UPDATE video_timeline_exports
         SET state = 'interrupted', revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE state IN ('running', 'verifying', 'publishing')
           AND NOT EXISTS (
               SELECT 1 FROM video_timeline_export_leases AS leases
               WHERE leases.export_id = video_timeline_exports.id
                 AND leases.expires_at_epoch > ?1
           )",
        [now_epoch],
    )?;
    tx.execute(
        "DELETE FROM video_timeline_export_leases WHERE expires_at_epoch <= ?1",
        [now_epoch],
    )?;
    tx.commit()?;
    Ok(changed)
}

fn insert_export_stages(
    conn: &Connection,
    export_id: &str,
    export_fingerprint: &str,
    clip_count: usize,
) -> Result<(), CoreError> {
    let mut stages = vec![(VideoTimelineExportStageKind::Validate, None)];
    stages.extend(
        (0..clip_count).map(|ordinal| (VideoTimelineExportStageKind::Normalize, Some(ordinal))),
    );
    stages.extend([
        (VideoTimelineExportStageKind::Concatenate, None),
        (VideoTimelineExportStageKind::Verify, None),
        (VideoTimelineExportStageKind::Publish, None),
    ]);
    for (ordinal, (kind, clip_ordinal)) in stages.into_iter().enumerate() {
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(format!(
                "{export_fingerprint}\0{}\0{}",
                kind.as_str(),
                clip_ordinal.map_or_else(|| "-".to_string(), |value| value.to_string())
            ))
        );
        conn.execute(
            "INSERT INTO video_timeline_export_stages
             (export_id, ordinal, stage_kind, clip_ordinal, state, fingerprint_sha256)
             VALUES (?1, ?2, ?3, ?4, 'queued', ?5)",
            rusqlite::params![
                export_id,
                ordinal as i64,
                kind.as_str(),
                clip_ordinal.map(|value| value as i64),
                fingerprint,
            ],
        )?;
    }
    Ok(())
}

fn load_timeline_snapshot(
    conn: &Connection,
    workflow_id: &str,
) -> Result<VideoTimelineSnapshot, CoreError> {
    let timeline = load_timeline_record(conn, workflow_id)?;
    let clips = load_clips(conn, &timeline)?;
    let exports = {
        let mut statement = conn.prepare(
            "SELECT id FROM video_timeline_exports
             WHERE workflow_id = ?1 ORDER BY created_at DESC LIMIT ?2",
        )?;
        let rows = statement
            .query_map(rusqlite::params![workflow_id, MAX_EXPORT_HISTORY], |row| {
                row.get::<_, String>(0)
            })?
            .map(|row| load_export(conn, &row?))
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    Ok(VideoTimelineSnapshot {
        timeline,
        clips,
        exports,
    })
}

fn load_timeline_record(
    conn: &Connection,
    workflow_id: &str,
) -> Result<VideoTimelineRecord, CoreError> {
    conn.query_row(
        "SELECT id, workflow_id, schema_version, revision, created_at, updated_at
         FROM video_timelines WHERE workflow_id = ?1",
        [workflow_id],
        |row| {
            Ok(VideoTimelineRecord {
                id: row.get(0)?,
                workflow_id: row.get(1)?,
                schema_version: row.get(2)?,
                revision: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| CoreError::NotFound(format!("Video timeline for workflow {workflow_id}")))
}

fn load_clips(
    conn: &Connection,
    timeline: &VideoTimelineRecord,
) -> Result<Vec<VideoTimelineClipRecord>, CoreError> {
    let mut statement = conn.prepare(
        "SELECT clips.id, clips.timeline_id, clips.shot_id, shots.title,
                clips.variant_id, shots.selected_variant_id, clips.asset_id,
                assets.content_hash_sha256, assets.media_type, clips.ordinal,
                clips.source_start_us, clips.source_duration_us,
                COALESCE(assets.duration_ms * 1000,
                         json_extract(variants.shot_snapshot_json, '$.durationSeconds') * 1000000),
                clips.revision, clips.created_at, clips.updated_at,
                CASE WHEN shots.selected_variant_id = clips.variant_id
                       AND assets.local_state = 'available' THEN 0 ELSE 1 END
         FROM video_timeline_clips AS clips
         JOIN video_workflow_shots AS shots ON shots.id = clips.shot_id
         JOIN video_workflow_variants AS variants ON variants.id = clips.variant_id
         JOIN media_assets AS assets ON assets.id = clips.asset_id
         WHERE clips.timeline_id = ?1 ORDER BY clips.ordinal, clips.id",
    )?;
    let rows = statement
        .query_map([&timeline.id], |row| {
            Ok(VideoTimelineClipRecord {
                id: row.get(0)?,
                timeline_id: row.get(1)?,
                shot_id: row.get(2)?,
                shot_title: row.get(3)?,
                variant_id: row.get(4)?,
                selected_variant_id: row.get(5)?,
                asset_id: row.get(6)?,
                asset_content_hash: row.get(7)?,
                media_type: row.get(8)?,
                ordinal: row.get(9)?,
                source_start_us: row.get(10)?,
                source_duration_us: row.get(11)?,
                available_duration_us: row.get(12)?,
                revision: row.get(13)?,
                created_at: row.get(14)?,
                updated_at: row.get(15)?,
                stale: row.get::<_, i64>(16)? != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn load_export(conn: &Connection, export_id: &str) -> Result<VideoTimelineExportRecord, CoreError> {
    let raw = conn
        .query_row(
            "SELECT workflow_id, timeline_id, timeline_revision, state, current_stage,
                    output_profile_json, ffmpeg_identity_json, clip_snapshot_json,
                    input_fingerprint_sha256, destination_path, progress_basis_points,
                    output_asset_id, cancellation_requested_at, error_json, revision,
                    created_at, updated_at, completed_at
             FROM video_timeline_exports WHERE id = ?1",
            [export_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, u32>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, u64>(14)?,
                    row.get::<_, String>(15)?,
                    row.get::<_, String>(16)?,
                    row.get::<_, Option<String>>(17)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| CoreError::NotFound(format!("Video timeline export {export_id}")))?;
    let stages = load_export_stages(conn, export_id)?;
    Ok(VideoTimelineExportRecord {
        id: export_id.to_string(),
        workflow_id: raw.0,
        timeline_id: raw.1,
        timeline_revision: raw.2,
        state: serde_json::from_value(Value::String(raw.3))?,
        current_stage: serde_json::from_value(Value::String(raw.4))?,
        output_profile: serde_json::from_str(&raw.5)?,
        ffmpeg_identity: raw
            .6
            .map(|value| serde_json::from_str(&value))
            .transpose()?,
        clips: serde_json::from_str(&raw.7)?,
        input_fingerprint_sha256: raw.8,
        destination_path: raw.9,
        progress_basis_points: raw.10,
        output_asset_id: raw.11,
        cancellation_requested_at: raw.12,
        error: raw
            .13
            .map(|value| serde_json::from_str(&value))
            .transpose()?,
        revision: raw.14,
        created_at: raw.15,
        updated_at: raw.16,
        completed_at: raw.17,
        stages,
    })
}

fn find_export_by_idempotency_conn(
    conn: &Connection,
    workflow_id: &str,
    idempotency_key: &str,
) -> Result<Option<VideoTimelineExportRecord>, CoreError> {
    let export_id = conn
        .query_row(
            "SELECT id FROM video_timeline_exports
             WHERE workflow_id = ?1 AND idempotency_key = ?2",
            rusqlite::params![workflow_id, idempotency_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    export_id
        .map(|export_id| load_export(conn, &export_id))
        .transpose()
}

fn load_export_stages(
    conn: &Connection,
    export_id: &str,
) -> Result<Vec<VideoTimelineExportStageRecord>, CoreError> {
    let mut statement = conn.prepare(
        "SELECT ordinal, stage_kind, clip_ordinal, state, fingerprint_sha256,
                attempt_count, progress_basis_points, intermediate_asset_id,
                error_json, started_at, completed_at, updated_at
         FROM video_timeline_export_stages WHERE export_id = ?1 ORDER BY ordinal",
    )?;
    let stages = statement
        .query_map([export_id], |row| {
            let kind: String = row.get(1)?;
            let state: String = row.get(3)?;
            let error: Option<String> = row.get(8)?;
            Ok((
                row.get::<_, u32>(0)?,
                kind,
                row.get::<_, Option<u32>>(2)?,
                state,
                row.get::<_, String>(4)?,
                row.get::<_, u32>(5)?,
                row.get::<_, u32>(6)?,
                row.get::<_, Option<String>>(7)?,
                error,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, String>(11)?,
            ))
        })?
        .map(|row| {
            let row = row?;
            Ok(VideoTimelineExportStageRecord {
                ordinal: row.0,
                stage_kind: serde_json::from_value(Value::String(row.1))?,
                clip_ordinal: row.2,
                state: serde_json::from_value(Value::String(row.3))?,
                fingerprint_sha256: row.4,
                attempt_count: row.5,
                progress_basis_points: row.6,
                intermediate_asset_id: row.7,
                error: row
                    .8
                    .map(|value| serde_json::from_str(&value))
                    .transpose()?,
                started_at: row.9,
                completed_at: row.10,
                updated_at: row.11,
            })
        })
        .collect::<Result<Vec<_>, CoreError>>()?;
    Ok(stages)
}

#[derive(Debug)]
struct SelectedSource {
    asset_id: String,
    available_duration_us: i64,
}

fn selected_source(
    conn: &Connection,
    workflow_id: &str,
    shot_id: &str,
    variant_id: &str,
) -> Result<SelectedSource, CoreError> {
    conn.query_row(
        "SELECT assets.id,
                COALESCE(assets.duration_ms * 1000,
                         json_extract(variants.shot_snapshot_json, '$.durationSeconds') * 1000000)
         FROM video_workflow_shots AS shots
         JOIN video_workflow_variants AS variants
           ON variants.id = shots.selected_variant_id
          AND variants.shot_id = shots.id AND variants.workflow_id = shots.workflow_id
         JOIN media_jobs AS jobs ON jobs.id = variants.job_id AND jobs.state = 'completed'
         JOIN media_asset_relations AS relations
           ON relations.job_id = jobs.id AND relations.relation_type = 'output' AND relations.ordinal = 0
         JOIN media_assets AS assets ON assets.id = relations.asset_id AND assets.local_state = 'available'
         WHERE shots.workflow_id = ?1 AND shots.id = ?2 AND variants.id = ?3",
        rusqlite::params![workflow_id, shot_id, variant_id],
        |row| {
            Ok(SelectedSource {
                asset_id: row.get(0)?,
                available_duration_us: row.get(1)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| {
        CoreError::Conflict(
            "Timeline clips require the shot's selected completed variant and its verified local output"
                .to_string(),
        )
    })
}

fn check_timeline_revision(
    conn: &Connection,
    workflow_id: &str,
    expected_revision: u64,
) -> Result<VideoTimelineRecord, CoreError> {
    let timeline = load_timeline_record(conn, workflow_id)?;
    if timeline.revision != expected_revision {
        return Err(CoreError::Conflict(format!(
            "Video timeline revision changed from {expected_revision} to {}",
            timeline.revision
        )));
    }
    Ok(timeline)
}

fn bump_timeline(conn: &Connection, timeline_id: &str) -> Result<(), CoreError> {
    conn.execute(
        "UPDATE video_timelines
         SET revision = revision + 1,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1",
        [timeline_id],
    )?;
    Ok(())
}

fn densify_clip_ordinals(conn: &Connection, timeline_id: &str) -> Result<(), CoreError> {
    let ids: Vec<String> = {
        let mut statement = conn.prepare(
            "SELECT id FROM video_timeline_clips WHERE timeline_id = ?1 ORDER BY ordinal, id",
        )?;
        let rows = statement
            .query_map([timeline_id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    conn.execute(
        "UPDATE video_timeline_clips SET ordinal = ordinal + 1000000 WHERE timeline_id = ?1",
        [timeline_id],
    )?;
    for (ordinal, id) in ids.iter().enumerate() {
        conn.execute(
            "UPDATE video_timeline_clips SET ordinal = ?2 WHERE id = ?1",
            rusqlite::params![id, ordinal as i64],
        )?;
    }
    Ok(())
}

fn required(value: &str, field: &str) -> Result<String, CoreError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_ID_BYTES {
        return Err(CoreError::InvalidInput(format!(
            "{field} must be between 1 and {MAX_ID_BYTES} bytes"
        )));
    }
    Ok(value.to_string())
}

fn ensure_owned_lease(
    conn: &Connection,
    export_id: &str,
    owner_id: &str,
    now_epoch: i64,
) -> Result<(), CoreError> {
    let owned: bool = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM video_timeline_export_leases
             WHERE export_id = ?1 AND owner_id = ?2 AND expires_at_epoch > ?3
         )",
        rusqlite::params![export_id, owner_id, now_epoch],
        |row| row.get(0),
    )?;
    if !owned {
        return Err(CoreError::Conflict(
            "Timeline export worker lease is no longer owned by this process".to_string(),
        ));
    }
    Ok(())
}

fn bounded(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub(crate) fn validate_destination_path(path: &str) -> Result<PathBuf, CoreError> {
    validate_destination_path_inner(path, false)
}

pub(crate) fn validate_retry_destination_path(path: &str) -> Result<PathBuf, CoreError> {
    validate_destination_path_inner(path, true)
}

fn validate_destination_path_inner(path: &str, allow_existing: bool) -> Result<PathBuf, CoreError> {
    if path.trim().is_empty() || path.len() > MAX_DESTINATION_BYTES {
        return Err(CoreError::InvalidInput(
            "Choose a bounded MP4 export destination".to_string(),
        ));
    }
    let path = PathBuf::from(path);
    if !path.is_absolute()
        || !path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("mp4"))
        || path.file_name().is_none()
    {
        return Err(CoreError::InvalidInput(
            "Export destination must be an absolute .mp4 file path".to_string(),
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        CoreError::InvalidInput("Export destination must have a parent directory".to_string())
    })?;
    let canonical_parent = std::fs::canonicalize(parent).map_err(|error| {
        CoreError::InvalidInput(format!(
            "Export destination directory is unavailable: {error}"
        ))
    })?;
    if !canonical_parent.is_dir()
        || is_link_or_reparse(&std::fs::symlink_metadata(parent)?)
        || is_link_or_reparse(&std::fs::symlink_metadata(&canonical_parent)?)
    {
        return Err(CoreError::InvalidInput(
            "Export destination directory cannot be a symlink or reparse point".to_string(),
        ));
    }
    if path.exists() && is_link_or_reparse(&std::fs::symlink_metadata(&path)?) {
        return Err(CoreError::InvalidInput(
            "Export destination cannot be a symlink or reparse point".to_string(),
        ));
    }
    if path.exists() && !allow_existing {
        return Err(CoreError::Conflict(
            "Export destination already exists; choose a new file name".to_string(),
        ));
    }
    let file_name = path.file_name().expect("checked above");
    Ok(canonical_parent.join(file_name))
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

pub(crate) fn export_owned_partial_path(
    destination: &Path,
    export_id: &str,
    owner_id: &str,
) -> Result<PathBuf, CoreError> {
    let parent = destination.parent().ok_or_else(|| {
        CoreError::InvalidInput("Export destination must have a parent directory".to_string())
    })?;
    let owner_hash = format!("{:x}", Sha256::digest(owner_id.as_bytes()));
    Ok(parent.join(format!(
        ".nexa-{export_id}-{}.partial.mp4",
        &owner_hash[..16]
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_generation::RequestMediaAssetDeletion;

    struct Fixture {
        _temp: tempfile::TempDir,
        database: Database,
        workflow_id: String,
        timeline_id: String,
        shot_ids: Vec<String>,
        variant_ids: Vec<String>,
        asset_ids: Vec<String>,
    }

    fn fixture(count: usize) -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let database = Database::new(temp.path().join("nexa.db")).unwrap();
        let workflow_id = "workflow-timeline".to_string();
        let timeline_id = "timeline-workflow-timeline".to_string();
        let conn = database.conn();
        conn.execute(
            "INSERT INTO video_workflows
             (id, title, brief_json, aspect_ratio, target_duration_ms)
             VALUES (?1, 'Timeline test', '{}', '16:9', 10000)",
            [&workflow_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO video_timelines (id, workflow_id) VALUES (?1, ?2)",
            rusqlite::params![timeline_id, workflow_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO video_provider_connections
             (id, provider_id, display_name, official_base_url, credential_ciphertext, credential_scope)
             VALUES ('connection', 'runway', 'Test', 'https://api.dev.runwayml.com', 'cipher', 'scope')",
            [],
        )
        .unwrap();
        let mut shot_ids = Vec::new();
        let mut variant_ids = Vec::new();
        let mut asset_ids = Vec::new();
        for index in 0..count {
            let shot_id = format!("shot-{index}");
            let variant_id = format!("variant-{index}");
            let job_id = format!("job-{index}");
            let attempt_id = format!("attempt-{index}");
            let asset_id = format!("{:064x}", index + 1);
            conn.execute(
                "INSERT INTO video_workflow_shots
                 (id, workflow_id, ordinal, title, prompt, operation, connection_id,
                  provider_id, model_id, duration_seconds, resolution, aspect_ratio,
                  input_assets_json, retention_policy, watermark_policy, provenance_policy)
                 VALUES (?1, ?2, ?3, ?4, 'prompt', 'text_to_video', 'connection',
                         'runway', 'gen4_turbo', 2, '720p', '16:9', '[]', 'documented', 'none', 'provider')",
                rusqlite::params![shot_id, workflow_id, index as i64, format!("Shot {index}")],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO media_jobs
                 (id, idempotency_key, request_fingerprint_sha256, provider_id, provider_source,
                  model_id, operation, state, raw_parameters_json, normalized_parameters_json,
                  observation_mode, current_attempt_id)
                 VALUES (?1, ?2, ?3, 'runway', 'https://api.dev.runwayml.com/account',
                         'gen4_turbo', 'text_to_video', 'completed', '{}', '{}', 'polling', ?4)",
                rusqlite::params![
                    job_id,
                    format!("job-key-{index}"),
                    "f".repeat(64),
                    attempt_id
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO media_job_attempts
                 (id, job_id, attempt_number, idempotency_key, provider_id, provider_source,
                  model_id, state)
                 VALUES (?1, ?2, 1, ?3, 'runway', 'https://api.dev.runwayml.com/account',
                         'gen4_turbo', 'succeeded')",
                rusqlite::params![attempt_id, job_id, format!("attempt-key-{index}")],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO video_workflow_variants
                 (id, workflow_id, shot_id, ordinal, job_id, connection_id, shot_snapshot_json, label)
                 VALUES (?1, ?2, ?3, 0, ?4, 'connection', '{\"durationSeconds\":2}', 'A')",
                rusqlite::params![variant_id, workflow_id, shot_id, job_id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO media_assets
                 (id, content_hash_sha256, content_verified_at, media_type, byte_length,
                  storage_kind, storage_key, duration_ms)
                 VALUES (?1, ?1, '2026-08-07T00:00:00Z', 'video/mp4', 10,
                         'managed_local', ?2, 2000)",
                rusqlite::params![asset_id, format!("sha256/00/{asset_id}")],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO media_asset_relations
                 (id, job_id, attempt_id, asset_id, relation_type, ordinal)
                 VALUES (?1, ?2, ?3, ?4, 'output', 0)",
                rusqlite::params![format!("relation-{index}"), job_id, attempt_id, asset_id],
            )
            .unwrap();
            conn.execute(
                "UPDATE video_workflow_shots SET selected_variant_id = ?2 WHERE id = ?1",
                rusqlite::params![shot_id, variant_id],
            )
            .unwrap();
            shot_ids.push(shot_id);
            variant_ids.push(variant_id);
            asset_ids.push(asset_id);
        }
        drop(conn);
        Fixture {
            _temp: temp,
            database,
            workflow_id,
            timeline_id,
            shot_ids,
            variant_ids,
            asset_ids,
        }
    }

    fn add(fixture: &Fixture, index: usize, revision: u64) -> VideoTimelineSnapshot {
        add_clip(
            &fixture.database,
            AddVideoTimelineClipRequest {
                workflow_id: fixture.workflow_id.clone(),
                expected_timeline_revision: revision,
                shot_id: fixture.shot_ids[index].clone(),
                variant_id: fixture.variant_ids[index].clone(),
            },
        )
        .unwrap()
    }

    fn profile() -> VideoTimelineOutputProfile {
        VideoTimelineOutputProfile {
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
        }
    }

    fn identity() -> Value {
        json!({
            "schemaVersion": 1,
            "ffmpeg": { "binarySha256": "a".repeat(64) },
            "ffprobe": { "binarySha256": "b".repeat(64) },
        })
    }

    #[test]
    fn timeline_clip_ids_survive_reorder_and_ranges_are_bounded() {
        let fixture = fixture(2);
        let first = add(&fixture, 0, 0);
        let second = add(&fixture, 1, first.timeline.revision);
        let first_id = second.clips[0].id.clone();
        let second_id = second.clips[1].id.clone();
        let reordered = reorder_clips(
            &fixture.database,
            ReorderVideoTimelineClipsRequest {
                workflow_id: fixture.workflow_id.clone(),
                expected_timeline_revision: second.timeline.revision,
                ordered_clip_ids: vec![second_id.clone(), first_id.clone()],
            },
        )
        .unwrap();
        assert_eq!(reordered.clips[0].id, second_id);
        assert_eq!(reordered.clips[1].id, first_id);
        let error = update_clip(
            &fixture.database,
            UpdateVideoTimelineClipRequest {
                workflow_id: fixture.workflow_id.clone(),
                expected_timeline_revision: reordered.timeline.revision,
                clip_id: reordered.clips[0].id.clone(),
                expected_clip_revision: reordered.clips[0].revision,
                source_start_us: 1_500_000,
                source_duration_us: 1_000_000,
            },
        )
        .unwrap_err();
        assert!(matches!(error, CoreError::InvalidInput(_)));
    }

    #[test]
    fn export_snapshot_does_not_mutate_when_the_live_timeline_changes() {
        let fixture = fixture(1);
        let timeline = add(&fixture, 0, 0);
        let export = create_export(
            &fixture.database,
            CreateVideoTimelineExportRequest {
                workflow_id: fixture.workflow_id.clone(),
                expected_timeline_revision: timeline.timeline.revision,
                idempotency_key: "export-key".to_string(),
                destination_path: fixture
                    ._temp
                    .path()
                    .join("output.mp4")
                    .to_string_lossy()
                    .into(),
                output_profile: profile(),
            },
            identity(),
        )
        .unwrap();
        let original_duration = export.clips[0].source_duration_us;
        let updated = update_clip(
            &fixture.database,
            UpdateVideoTimelineClipRequest {
                workflow_id: fixture.workflow_id.clone(),
                expected_timeline_revision: timeline.timeline.revision,
                clip_id: timeline.clips[0].id.clone(),
                expected_clip_revision: timeline.clips[0].revision,
                source_start_us: 250_000,
                source_duration_us: 1_000_000,
            },
        )
        .unwrap();
        assert_eq!(updated.clips[0].source_duration_us, 1_000_000);
        let persisted = load_export(&fixture.database.conn(), &export.id).unwrap();
        assert_eq!(persisted.clips[0].source_duration_us, original_duration);
        assert_eq!(persisted.timeline_revision, export.timeline_revision);
    }

    #[test]
    fn idempotent_export_replay_survives_live_timeline_and_destination_changes() {
        let fixture = fixture(1);
        let timeline = add(&fixture, 0, 0);
        let destination = fixture._temp.path().join("idempotent.mp4");
        let request = CreateVideoTimelineExportRequest {
            workflow_id: fixture.workflow_id.clone(),
            expected_timeline_revision: timeline.timeline.revision,
            idempotency_key: "durable-intent".to_string(),
            destination_path: destination.to_string_lossy().into_owned(),
            output_profile: profile(),
        };
        let created = create_export(&fixture.database, request.clone(), identity()).unwrap();
        std::fs::write(&destination, b"already published").unwrap();
        update_clip(
            &fixture.database,
            UpdateVideoTimelineClipRequest {
                workflow_id: fixture.workflow_id.clone(),
                expected_timeline_revision: timeline.timeline.revision,
                clip_id: timeline.clips[0].id.clone(),
                expected_clip_revision: timeline.clips[0].revision,
                source_start_us: 100_000,
                source_duration_us: 1_000_000,
            },
        )
        .unwrap();
        let replayed = create_export(
            &fixture.database,
            request,
            json!({"schemaVersion": 1, "changed": true}),
        )
        .unwrap();
        assert_eq!(replayed.id, created.id);
        assert_eq!(replayed.timeline_revision, created.timeline_revision);
        assert_eq!(replayed.ffmpeg_identity, created.ffmpeg_identity);
    }

    #[test]
    fn publication_retry_reuses_the_verified_asset_and_immutable_snapshot() {
        let fixture = fixture(1);
        let timeline = add(&fixture, 0, 0);
        let export = create_export(
            &fixture.database,
            CreateVideoTimelineExportRequest {
                workflow_id: fixture.workflow_id.clone(),
                expected_timeline_revision: timeline.timeline.revision,
                idempotency_key: "retry-key".to_string(),
                destination_path: fixture
                    ._temp
                    .path()
                    .join("first.mp4")
                    .to_string_lossy()
                    .into(),
                output_profile: profile(),
            },
            identity(),
        )
        .unwrap();
        let publish_ordinal = export.stages.len() as u32 - 1;
        let owner = "test:11111111-1111-4111-8111-111111111111";
        assert!(try_acquire_export_lease(&fixture.database, &export.id, owner, 10).unwrap());
        for ordinal in 0..publish_ordinal {
            complete_export_stage(&fixture.database, &export.id, owner, 10, ordinal, None).unwrap();
        }
        record_export_output_asset(
            &fixture.database,
            &export.id,
            owner,
            10,
            &fixture.asset_ids[0],
        )
        .unwrap();
        mark_export_failed(
            &fixture.database,
            &export.id,
            owner,
            10,
            publish_ordinal,
            "publish_rename",
            "destination unavailable",
        )
        .unwrap();
        release_export_lease(&fixture.database, &export.id, owner).unwrap();
        let failed = load_export(&fixture.database.conn(), &export.id).unwrap();
        let retried = retry_export(
            &fixture.database,
            RetryVideoTimelineExportRequest {
                export_id: export.id.clone(),
                expected_revision: failed.revision,
                destination_path: fixture
                    ._temp
                    .path()
                    .join("retry.mp4")
                    .to_string_lossy()
                    .into(),
            },
        )
        .unwrap();
        assert_eq!(retried.id, export.id);
        assert_eq!(retried.clips, export.clips);
        assert_eq!(retried.output_asset_id, Some(fixture.asset_ids[0].clone()));
        assert_eq!(retried.state, VideoTimelineExportState::Queued);
        assert_eq!(retried.current_stage, VideoTimelineExportStageKind::Publish);
        assert!(retried.stages[..publish_ordinal as usize]
            .iter()
            .all(|stage| stage.state == VideoTimelineExportStageState::Completed));
        assert_eq!(
            retried.stages[publish_ordinal as usize].state,
            VideoTimelineExportStageState::Queued
        );
        assert!(retried.error.is_none());
    }

    #[test]
    fn lineage_blocks_asset_deletion_but_export_snapshot_does_not_block_clip_removal() {
        let fixture = fixture(1);
        let timeline = add(&fixture, 0, 0);
        create_export(
            &fixture.database,
            CreateVideoTimelineExportRequest {
                workflow_id: fixture.workflow_id.clone(),
                expected_timeline_revision: timeline.timeline.revision,
                idempotency_key: "export-key".to_string(),
                destination_path: fixture
                    ._temp
                    .path()
                    .join("output.mp4")
                    .to_string_lossy()
                    .into(),
                output_profile: profile(),
            },
            identity(),
        )
        .unwrap();
        let deletion = crate::media_generation::store::prepare_asset_deletion(
            &fixture.database,
            RequestMediaAssetDeletion {
                asset_id: fixture.asset_ids[0].clone(),
            },
        )
        .unwrap_err();
        assert!(matches!(deletion, CoreError::Conflict(_)));
        let removed = remove_clip(
            &fixture.database,
            RemoveVideoTimelineClipRequest {
                workflow_id: fixture.workflow_id.clone(),
                expected_timeline_revision: timeline.timeline.revision,
                clip_id: timeline.clips[0].id.clone(),
                expected_clip_revision: timeline.clips[0].revision,
            },
        )
        .unwrap();
        assert!(removed.clips.is_empty());
        assert_eq!(removed.exports[0].clips[0].asset_id, fixture.asset_ids[0]);
    }

    #[test]
    fn export_lease_is_single_owner_until_expiry() {
        let fixture = fixture(0);
        let conn = fixture.database.conn();
        conn.execute(
            "INSERT INTO video_timeline_exports
             (id, workflow_id, timeline_id, timeline_revision, idempotency_key, state,
              current_stage, output_profile_json, clip_snapshot_json,
              input_fingerprint_sha256, destination_path)
             VALUES ('export', ?1, ?2, 0, 'key', 'queued', 'validate', ?3, '[]', ?4, ?5)",
            rusqlite::params![
                fixture.workflow_id,
                fixture.timeline_id,
                serde_json::to_string(&profile()).unwrap(),
                "f".repeat(64),
                fixture._temp.path().join("out.mp4").to_string_lossy(),
            ],
        )
        .unwrap();
        drop(conn);
        assert!(try_acquire_export_lease(&fixture.database, "export", "owner-a", 10).unwrap());
        assert!(!try_acquire_export_lease(&fixture.database, "export", "owner-b", 11).unwrap());
        assert!(try_acquire_export_lease(&fixture.database, "export", "owner-b", 611).unwrap());
        assert!(!renew_export_lease(&fixture.database, "export", "owner-a", 612).unwrap());
    }

    #[test]
    fn valid_export_lease_survives_other_process_recovery_until_expiry() {
        let fixture = fixture(1);
        let timeline = add(&fixture, 0, 0);
        let export = create_export(
            &fixture.database,
            CreateVideoTimelineExportRequest {
                workflow_id: fixture.workflow_id.clone(),
                expected_timeline_revision: timeline.timeline.revision,
                idempotency_key: "recover-key".to_string(),
                destination_path: fixture
                    ._temp
                    .path()
                    .join("recover.mp4")
                    .to_string_lossy()
                    .into_owned(),
                output_profile: profile(),
            },
            identity(),
        )
        .unwrap();
        assert!(try_acquire_export_lease(&fixture.database, &export.id, "owner-a", 10).unwrap());
        begin_export_stage(
            &fixture.database,
            &export.id,
            "owner-a",
            10,
            0,
            VideoTimelineExportStageKind::Validate,
            VideoTimelineExportState::Running,
        )
        .unwrap();
        assert_eq!(
            mark_live_exports_interrupted(&fixture.database, 11).unwrap(),
            0
        );
        assert_eq!(
            load_export(&fixture.database.conn(), &export.id)
                .unwrap()
                .state,
            VideoTimelineExportState::Running
        );
        assert_eq!(
            mark_live_exports_interrupted(&fixture.database, 611).unwrap(),
            1
        );
        assert_eq!(
            load_export(&fixture.database.conn(), &export.id)
                .unwrap()
                .state,
            VideoTimelineExportState::Interrupted
        );
    }

    #[test]
    fn publication_commit_is_a_non_cancellable_terminal_boundary() {
        let fixture = fixture(1);
        let timeline = add(&fixture, 0, 0);
        let export = create_export(
            &fixture.database,
            CreateVideoTimelineExportRequest {
                workflow_id: fixture.workflow_id.clone(),
                expected_timeline_revision: timeline.timeline.revision,
                idempotency_key: "publish-boundary".to_string(),
                destination_path: fixture
                    ._temp
                    .path()
                    .join("boundary.mp4")
                    .to_string_lossy()
                    .into_owned(),
                output_profile: profile(),
            },
            identity(),
        )
        .unwrap();
        let owner = "owner-a";
        assert!(try_acquire_export_lease(&fixture.database, &export.id, owner, 10).unwrap());
        let publish_ordinal = export.stages.len() as u32 - 1;
        begin_export_stage(
            &fixture.database,
            &export.id,
            owner,
            10,
            publish_ordinal,
            VideoTimelineExportStageKind::Publish,
            VideoTimelineExportState::Publishing,
        )
        .unwrap();
        begin_export_publication_commit(&fixture.database, &export.id, owner, 10).unwrap();
        let committing = load_export(&fixture.database.conn(), &export.id).unwrap();
        let error = request_export_cancellation(
            &fixture.database,
            CancelVideoTimelineExportRequest {
                export_id: export.id.clone(),
                expected_revision: committing.revision,
            },
        )
        .unwrap_err();
        assert!(matches!(error, CoreError::Conflict(_)));
        assert!(!export_cancel_requested(&fixture.database, &export.id).unwrap());
        mark_export_completed(
            &fixture.database,
            &export.id,
            owner,
            10,
            &fixture.asset_ids[0],
        )
        .unwrap();
    }

    #[test]
    fn export_leases_enforce_a_cross_process_concurrency_cap() {
        let fixture = fixture(1);
        let timeline = add(&fixture, 0, 0);
        let mut exports = Vec::new();
        for index in 0..3 {
            exports.push(
                create_export(
                    &fixture.database,
                    CreateVideoTimelineExportRequest {
                        workflow_id: fixture.workflow_id.clone(),
                        expected_timeline_revision: timeline.timeline.revision,
                        idempotency_key: format!("cap-{index}"),
                        destination_path: fixture
                            ._temp
                            .path()
                            .join(format!("cap-{index}.mp4"))
                            .to_string_lossy()
                            .into_owned(),
                        output_profile: profile(),
                    },
                    identity(),
                )
                .unwrap(),
            );
        }
        assert!(
            try_acquire_export_lease(&fixture.database, &exports[0].id, "owner-a", 10).unwrap()
        );
        assert!(
            try_acquire_export_lease(&fixture.database, &exports[1].id, "owner-b", 10).unwrap()
        );
        assert!(
            !try_acquire_export_lease(&fixture.database, &exports[2].id, "owner-c", 10).unwrap()
        );
        release_export_lease(&fixture.database, &exports[0].id, "owner-a").unwrap();
        assert!(
            try_acquire_export_lease(&fixture.database, &exports[2].id, "owner-c", 11).unwrap()
        );
    }

    #[test]
    fn retry_cannot_bypass_the_global_active_export_cap() {
        let fixture = fixture(1);
        let timeline = add(&fixture, 0, 0);
        let mut failed_exports = Vec::new();
        for index in 0..9 {
            let export = create_export(
                &fixture.database,
                CreateVideoTimelineExportRequest {
                    workflow_id: fixture.workflow_id.clone(),
                    expected_timeline_revision: timeline.timeline.revision,
                    idempotency_key: format!("failed-{index}"),
                    destination_path: fixture
                        ._temp
                        .path()
                        .join(format!("failed-{index}.mp4"))
                        .to_string_lossy()
                        .into_owned(),
                    output_profile: profile(),
                },
                identity(),
            )
            .unwrap();
            let owner = format!("owner-{index}");
            assert!(try_acquire_export_lease(&fixture.database, &export.id, &owner, 10).unwrap());
            mark_export_failed(
                &fixture.database,
                &export.id,
                &owner,
                10,
                0,
                "test_failure",
                "failed before retry",
            )
            .unwrap();
            release_export_lease(&fixture.database, &export.id, &owner).unwrap();
            failed_exports.push(load_export(&fixture.database.conn(), &export.id).unwrap());
        }
        for (index, export) in failed_exports.iter().take(8).enumerate() {
            retry_export(
                &fixture.database,
                RetryVideoTimelineExportRequest {
                    export_id: export.id.clone(),
                    expected_revision: export.revision,
                    destination_path: fixture
                        ._temp
                        .path()
                        .join(format!("retried-{index}.mp4"))
                        .to_string_lossy()
                        .into_owned(),
                },
            )
            .unwrap();
        }
        let ninth = &failed_exports[8];
        let error = retry_export(
            &fixture.database,
            RetryVideoTimelineExportRequest {
                export_id: ninth.id.clone(),
                expected_revision: ninth.revision,
                destination_path: fixture
                    ._temp
                    .path()
                    .join("retry-over-cap.mp4")
                    .to_string_lossy()
                    .into_owned(),
            },
        )
        .unwrap_err();
        assert!(matches!(error, CoreError::Conflict(_)));
    }
}
