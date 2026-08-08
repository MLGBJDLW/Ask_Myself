//! Durable Agent lifecycle projection and safe Companion Pack contract.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::agent_run::{AgentRunEvent, AgentRunEventKind, AgentRunPhase};
use crate::conversation::AgentTaskRun;
use crate::db::Database;
use crate::error::CoreError;

pub const COMPANION_PACK_SCHEMA_VERSION: u16 = 1;
const MAX_SPRITESHEET_BYTES: u64 = 10 * 1024 * 1024;
const MAX_DECODED_SPRITESHEET_BYTES: u64 = 32 * 1024 * 1024;
const DECODED_RGBA_BYTES_PER_PIXEL: u64 = 4;
const MAX_FRAME_EDGE: u32 = 512;
const MAX_GRID_EDGE: u32 = 16;
const MAX_TOTAL_FRAMES: u32 = 256;
const MAX_ANIMATION_FPS: u16 = 60;
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
const MAX_CATALOG_ERRORS: usize = 64;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CompanionState {
    Idle,
    Thinking,
    Searching,
    Browsing,
    ReadingFiles,
    RunningTool,
    Coding,
    WaitingForApproval,
    WaitingForUser,
    Reviewing,
    Succeeded,
    Failed,
    Cancelled,
    Sleeping,
}

impl CompanionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Thinking => "thinking",
            Self::Searching => "searching",
            Self::Browsing => "browsing",
            Self::ReadingFiles => "readingFiles",
            Self::RunningTool => "runningTool",
            Self::Coding => "coding",
            Self::WaitingForApproval => "waitingForApproval",
            Self::WaitingForUser => "waitingForUser",
            Self::Reviewing => "reviewing",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Sleeping => "sleeping",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompanionProjection {
    pub run_id: String,
    pub state: CompanionState,
    pub label: String,
    pub source_event_seq: Option<u64>,
    pub terminal: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompanionPackManifest {
    pub schema_version: u16,
    pub id: String,
    pub display_name: String,
    pub spritesheet: String,
    pub frame: CompanionFrameGrid,
    pub animations: HashMap<String, CompanionAnimation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompanionFrameGrid {
    pub width: u32,
    pub height: u32,
    pub columns: u32,
    pub rows: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompanionAnimation {
    pub row: u32,
    pub start_column: u32,
    pub frames: u32,
    pub fps: u16,
    #[serde(default)]
    pub looping: bool,
    #[serde(default)]
    pub fallback: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompanionPackDialect {
    NexaV1,
    NexaV2,
    CodexTuiV1,
    CodexDesktopV2,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedAnimation {
    pub frames: Vec<u16>,
    pub fps: f32,
    pub looping: bool,
    #[serde(default)]
    pub fallback: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedCompanionPack {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub dialect: CompanionPackDialect,
    pub compatibility: String,
    pub spritesheet_path: String,
    pub content_hash: String,
    pub frame: CompanionFrameGrid,
    pub animations: HashMap<String, NormalizedAnimation>,
    #[serde(default)]
    pub experimental_features: Vec<String>,
    pub managed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CompanionPackCatalog {
    pub packs: Vec<NormalizedCompanionPack>,
    pub errors: Vec<CompanionPackCatalogError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompanionPackCatalogError {
    pub manifest_path: String,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExplicitFrameAnimation {
    frames: Vec<u16>,
    fps: f32,
    #[serde(default, rename = "loop")]
    looping: bool,
    #[serde(default)]
    fallback: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexCompanionManifest {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default = "default_codex_spritesheet_path")]
    spritesheet_path: String,
    #[serde(default)]
    sprite_version_number: Option<u16>,
    #[serde(default)]
    frame: Option<CompanionFrameGrid>,
    #[serde(default)]
    animations: HashMap<String, ExplicitFrameAnimation>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NexaV2CompanionManifest {
    schema_version: u16,
    id: String,
    display_name: String,
    #[serde(default)]
    description: Option<String>,
    spritesheet: String,
    frame: CompanionFrameGrid,
    animations: HashMap<String, NormalizedAnimation>,
}

impl Database {
    pub fn get_companion_projection(&self, run_id: &str) -> Result<CompanionProjection, CoreError> {
        let run = self.get_agent_task_run(run_id)?;
        let events = self.list_agent_run_events(run_id)?;
        Ok(project_companion_state(&run, &events))
    }
}

pub fn project_companion_state(
    run: &AgentTaskRun,
    events: &[AgentRunEvent],
) -> CompanionProjection {
    if let Some(state) = terminal_state(&run.status) {
        return CompanionProjection {
            run_id: run.id.clone(),
            state,
            label: run
                .error_message
                .clone()
                .filter(|message| !message.trim().is_empty())
                .unwrap_or_else(|| terminal_label(state).to_string()),
            source_event_seq: events.last().map(|event| event.event_seq),
            terminal: true,
        };
    }

    let latest = events.last();
    let (state, label) = latest
        .map(state_from_event)
        .unwrap_or_else(|| state_from_run_phase(run));
    CompanionProjection {
        run_id: run.id.clone(),
        state,
        label,
        source_event_seq: latest.map(|event| event.event_seq),
        terminal: false,
    }
}

fn terminal_state(status: &str) -> Option<CompanionState> {
    match status.trim().to_ascii_lowercase().as_str() {
        "completed" | "succeeded" => Some(CompanionState::Succeeded),
        "failed" | "timed_out" => Some(CompanionState::Failed),
        "cancelled" | "canceled" => Some(CompanionState::Cancelled),
        _ => None,
    }
}

fn terminal_label(state: CompanionState) -> &'static str {
    match state {
        CompanionState::Succeeded => "Task completed",
        CompanionState::Failed => "Task failed",
        CompanionState::Cancelled => "Task cancelled",
        _ => "Task finished",
    }
}

fn state_from_event(event: &AgentRunEvent) -> (CompanionState, String) {
    let label = if event.label.trim().is_empty() {
        event.phase.as_str().replace('_', " ")
    } else {
        event.label.clone()
    };
    let state = match event.kind {
        AgentRunEventKind::ApprovalRequested => CompanionState::WaitingForApproval,
        AgentRunEventKind::ApprovalResolved => CompanionState::Thinking,
        AgentRunEventKind::Error => CompanionState::Failed,
        AgentRunEventKind::Done => match event.status.as_deref() {
            Some("cancelled") => CompanionState::Cancelled,
            Some("failed" | "timed_out") => CompanionState::Failed,
            Some("completed") => CompanionState::Succeeded,
            _ => CompanionState::Reviewing,
        },
        AgentRunEventKind::ToolPreparing
        | AgentRunEventKind::ToolStarted
        | AgentRunEventKind::ToolProgress
        | AgentRunEventKind::ToolCompleted => tool_state(&event.label),
        AgentRunEventKind::Thinking
        | AgentRunEventKind::PlanUpdated
        | AgentRunEventKind::OutputDelta
        | AgentRunEventKind::StreamReset
        | AgentRunEventKind::RecoveryAttempt => CompanionState::Thinking,
        AgentRunEventKind::AutoCompacted => CompanionState::Reviewing,
        AgentRunEventKind::Status => match event.phase {
            AgentRunPhase::AwaitingUserInput => CompanionState::WaitingForUser,
            AgentRunPhase::Approval => CompanionState::WaitingForApproval,
            AgentRunPhase::Tooling => tool_state(&event.label),
            AgentRunPhase::Done => CompanionState::Reviewing,
            _ => CompanionState::Thinking,
        },
        AgentRunEventKind::UsageUpdated => CompanionState::Reviewing,
    };
    (state, label)
}

fn state_from_run_phase(run: &AgentTaskRun) -> (CompanionState, String) {
    let state = match run.phase.trim().to_ascii_lowercase().as_str() {
        "approval" | "waiting_approval" => CompanionState::WaitingForApproval,
        "awaiting_user_input" | "waiting_user" => CompanionState::WaitingForUser,
        "tooling" => CompanionState::RunningTool,
        "accounting" | "reviewing" => CompanionState::Reviewing,
        "done" => CompanionState::Succeeded,
        _ => CompanionState::Thinking,
    };
    (state, run.title.clone())
}

fn tool_state(tool_name: &str) -> CompanionState {
    let tool_name = tool_name.trim().to_ascii_lowercase();
    if ["web_search", "native_search", "search_web"]
        .iter()
        .any(|needle| tool_name.contains(needle))
    {
        return CompanionState::Searching;
    }
    if ["browser", "fetch_url", "navigate", "http"]
        .iter()
        .any(|needle| tool_name.contains(needle))
    {
        return CompanionState::Browsing;
    }
    if [
        "read_file",
        "read_files",
        "list_dir",
        "glob",
        "grep",
        "document_info",
    ]
    .iter()
    .any(|needle| tool_name.contains(needle))
    {
        return CompanionState::ReadingFiles;
    }
    if [
        "edit_file",
        "multi_edit",
        "write_file",
        "create_file",
        "apply_patch",
        "run_shell",
    ]
    .iter()
    .any(|needle| tool_name.contains(needle))
    {
        return CompanionState::Coding;
    }
    CompanionState::RunningTool
}

pub fn validate_companion_pack(
    manifest: &CompanionPackManifest,
    spritesheet_size: u64,
) -> Result<(), CoreError> {
    if manifest.schema_version != COMPANION_PACK_SCHEMA_VERSION {
        return Err(CoreError::InvalidInput(format!(
            "Unsupported Companion Pack schema version {}; expected {}",
            manifest.schema_version, COMPANION_PACK_SCHEMA_VERSION
        )));
    }
    if manifest.id.trim().is_empty() || manifest.display_name.trim().is_empty() {
        return Err(CoreError::InvalidInput(
            "Companion Pack id and display name must not be empty".to_string(),
        ));
    }
    validate_asset_path(&manifest.spritesheet)?;
    if spritesheet_size == 0 || spritesheet_size > MAX_SPRITESHEET_BYTES {
        return Err(CoreError::InvalidInput(format!(
            "Companion spritesheet must be between 1 byte and {MAX_SPRITESHEET_BYTES} bytes"
        )));
    }
    let frame = &manifest.frame;
    if frame.width == 0
        || frame.height == 0
        || frame.width > MAX_FRAME_EDGE
        || frame.height > MAX_FRAME_EDGE
    {
        return Err(CoreError::InvalidInput(format!(
            "Companion frame dimensions must be between 1 and {MAX_FRAME_EDGE} pixels"
        )));
    }
    if frame.columns == 0
        || frame.rows == 0
        || frame.columns > MAX_GRID_EDGE
        || frame.rows > MAX_GRID_EDGE
        || frame.columns.saturating_mul(frame.rows) > MAX_TOTAL_FRAMES
    {
        return Err(CoreError::InvalidInput(
            "Companion frame grid exceeds the safe frame limit".to_string(),
        ));
    }
    validate_decoded_spritesheet_budget(
        frame.width.saturating_mul(frame.columns),
        frame.height.saturating_mul(frame.rows),
    )?;
    if manifest.animations.is_empty() {
        return Err(CoreError::InvalidInput(
            "Companion Pack must define at least one animation".to_string(),
        ));
    }
    for (name, animation) in &manifest.animations {
        if name.trim().is_empty()
            || animation.frames == 0
            || animation.fps == 0
            || animation.fps > MAX_ANIMATION_FPS
            || animation.row >= frame.rows
            || animation.start_column >= frame.columns
            || animation.start_column.saturating_add(animation.frames) > frame.columns
        {
            return Err(CoreError::InvalidInput(format!(
                "Companion animation '{name}' exceeds the safe grid or FPS limits"
            )));
        }
        if let Some(fallback) = animation.fallback.as_deref() {
            if fallback == name || !manifest.animations.contains_key(fallback) {
                return Err(CoreError::InvalidInput(format!(
                    "Companion animation '{name}' has an invalid fallback"
                )));
            }
        }
    }
    Ok(())
}

fn validate_asset_path(value: &str) -> Result<(), CoreError> {
    let path = Path::new(value.trim());
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if value.trim().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
        || !matches!(extension.as_str(), "png" | "webp")
    {
        return Err(CoreError::InvalidInput(
            "Companion spritesheet must be a safe relative PNG or WebP path".to_string(),
        ));
    }
    Ok(())
}

pub fn companion_pack_cache_key(
    manifest: &CompanionPackManifest,
    spritesheet_bytes: &[u8],
) -> Result<String, CoreError> {
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(manifest)?);
    hasher.update([0]);
    hasher.update(spritesheet_bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn default_codex_spritesheet_path() -> String {
    "spritesheet.webp".to_string()
}

fn validate_pack_id(id: &str) -> Result<String, CoreError> {
    let id = id.trim();
    if id.is_empty()
        || id.len() > 96
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(CoreError::InvalidInput(
            "Companion Pack id must use 1-96 ASCII letters, numbers, '-' or '_'".to_string(),
        ));
    }
    Ok(id.to_string())
}

fn validate_normalized_animations(
    animations: &HashMap<String, NormalizedAnimation>,
    total_frames: u32,
) -> Result<(), CoreError> {
    if animations.is_empty() {
        return Err(CoreError::InvalidInput(
            "Companion Pack must define at least one animation".to_string(),
        ));
    }
    for (name, animation) in animations {
        if name.trim().is_empty()
            || animation.frames.is_empty()
            || !animation.fps.is_finite()
            || animation.fps <= 0.0
            || animation.fps > f32::from(MAX_ANIMATION_FPS)
            || animation
                .frames
                .iter()
                .any(|frame| u32::from(*frame) >= total_frames)
        {
            return Err(CoreError::InvalidInput(format!(
                "Companion animation '{name}' exceeds the safe frame or FPS limits"
            )));
        }
        if let Some(fallback) = animation.fallback.as_deref() {
            if fallback == name || !animations.contains_key(fallback) {
                return Err(CoreError::InvalidInput(format!(
                    "Companion animation '{name}' has an invalid fallback"
                )));
            }
        }
    }
    Ok(())
}

fn validate_frame_grid(frame: &CompanionFrameGrid) -> Result<u32, CoreError> {
    let total_frames = frame.columns.saturating_mul(frame.rows);
    if frame.width == 0
        || frame.height == 0
        || frame.width > MAX_FRAME_EDGE
        || frame.height > MAX_FRAME_EDGE
        || frame.columns == 0
        || frame.rows == 0
        || frame.columns > MAX_GRID_EDGE
        || frame.rows > MAX_GRID_EDGE
        || total_frames > MAX_TOTAL_FRAMES
    {
        return Err(CoreError::InvalidInput(
            "Companion frame grid exceeds the safe frame limit".to_string(),
        ));
    }
    validate_decoded_spritesheet_budget(
        frame.width.saturating_mul(frame.columns),
        frame.height.saturating_mul(frame.rows),
    )?;
    Ok(total_frames)
}

fn validate_decoded_spritesheet_budget(width: u32, height: u32) -> Result<(), CoreError> {
    let decoded_bytes = u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(DECODED_RGBA_BYTES_PER_PIXEL);
    if decoded_bytes > MAX_DECODED_SPRITESHEET_BYTES {
        return Err(CoreError::InvalidInput(format!(
            "Companion spritesheet exceeds the {MAX_DECODED_SPRITESHEET_BYTES}-byte decoded cache limit"
        )));
    }
    Ok(())
}

fn default_codex_frame(rows: u32) -> CompanionFrameGrid {
    CompanionFrameGrid {
        width: 192,
        height: 208,
        columns: 8,
        rows,
    }
}

fn animation_row(row: u16, columns: u16, fps: f32) -> NormalizedAnimation {
    let start = row.saturating_mul(columns);
    NormalizedAnimation {
        frames: (start..start.saturating_add(columns)).collect(),
        fps,
        looping: true,
        fallback: None,
    }
}

fn default_codex_animations() -> HashMap<String, NormalizedAnimation> {
    HashMap::from([
        ("idle".to_string(), animation_row(0, 8, 8.0)),
        ("moveRight".to_string(), animation_row(1, 8, 10.0)),
        ("moveLeft".to_string(), animation_row(2, 8, 10.0)),
        ("waving".to_string(), animation_row(3, 8, 10.0)),
        ("jumping".to_string(), animation_row(4, 8, 10.0)),
        ("failed".to_string(), animation_row(5, 8, 8.0)),
        ("waiting".to_string(), animation_row(6, 8, 8.0)),
        ("running".to_string(), animation_row(7, 8, 12.0)),
        ("review".to_string(), animation_row(8, 8, 8.0)),
    ])
}

fn normalize_nexa_v1(
    manifest: CompanionPackManifest,
) -> Result<
    (
        String,
        String,
        Option<String>,
        CompanionFrameGrid,
        HashMap<String, NormalizedAnimation>,
    ),
    CoreError,
> {
    validate_companion_pack(&manifest, 1)?;
    let animations = manifest
        .animations
        .into_iter()
        .map(|(name, animation)| {
            let start = animation
                .row
                .saturating_mul(manifest.frame.columns)
                .saturating_add(animation.start_column);
            let end = start.saturating_add(animation.frames);
            (
                name,
                NormalizedAnimation {
                    frames: (start..end)
                        .filter_map(|frame| u16::try_from(frame).ok())
                        .collect(),
                    fps: f32::from(animation.fps),
                    looping: animation.looping,
                    fallback: animation.fallback,
                },
            )
        })
        .collect();
    Ok((
        manifest.id,
        manifest.display_name,
        None,
        manifest.frame,
        animations,
    ))
}

fn resolve_pack_asset(package_root: &Path, relative: &str) -> Result<PathBuf, CoreError> {
    validate_asset_path(relative)?;
    let canonical_root = package_root.canonicalize().map_err(|error| {
        CoreError::InvalidInput(format!("Companion Pack root is unavailable: {error}"))
    })?;
    let candidate = canonical_root.join(relative);
    let metadata = fs::metadata(&candidate).map_err(|error| {
        CoreError::InvalidInput(format!("Companion spritesheet is unavailable: {error}"))
    })?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_SPRITESHEET_BYTES {
        return Err(CoreError::InvalidInput(format!(
            "Companion spritesheet must be between 1 byte and {MAX_SPRITESHEET_BYTES} bytes"
        )));
    }
    let canonical_asset = candidate.canonicalize().map_err(|error| {
        CoreError::InvalidInput(format!("Companion spritesheet is unavailable: {error}"))
    })?;
    if !canonical_asset.starts_with(&canonical_root) {
        return Err(CoreError::InvalidInput(
            "Companion spritesheet resolves outside its package".to_string(),
        ));
    }
    Ok(canonical_asset)
}

fn validate_spritesheet_geometry(path: &Path, frame: &CompanionFrameGrid) -> Result<(), CoreError> {
    let (actual_width, actual_height) = image::image_dimensions(path).map_err(|error| {
        CoreError::InvalidInput(format!("Companion spritesheet cannot be decoded: {error}"))
    })?;
    let expected_width = frame.width.saturating_mul(frame.columns);
    let expected_height = frame.height.saturating_mul(frame.rows);
    validate_decoded_spritesheet_budget(actual_width, actual_height)?;
    if actual_width != expected_width || actual_height != expected_height {
        return Err(CoreError::InvalidInput(format!(
            "Companion spritesheet is {actual_width}x{actual_height}; expected {expected_width}x{expected_height}"
        )));
    }
    Ok(())
}

pub fn load_companion_pack(
    manifest_path: &Path,
    managed: bool,
) -> Result<NormalizedCompanionPack, CoreError> {
    let metadata = fs::metadata(manifest_path).map_err(|error| {
        CoreError::InvalidInput(format!("Companion manifest is unavailable: {error}"))
    })?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_MANIFEST_BYTES {
        return Err(CoreError::InvalidInput(format!(
            "Companion manifest must be between 1 byte and {MAX_MANIFEST_BYTES} bytes"
        )));
    }
    let manifest_bytes = fs::read(manifest_path)?;
    let manifest_value: serde_json::Value = serde_json::from_slice(&manifest_bytes)?;
    if let Some(schema_version) = manifest_value
        .get("schemaVersion")
        .and_then(|value| value.as_u64())
    {
        if !matches!(schema_version, 1 | 2) {
            return Err(CoreError::InvalidInput(format!(
                "Unsupported Nexa Companion schemaVersion {schema_version}"
            )));
        }
    }
    let package_root = manifest_path.parent().ok_or_else(|| {
        CoreError::InvalidInput("Companion manifest has no package directory".to_string())
    })?;

    let (
        dialect,
        compatibility,
        id,
        display_name,
        description,
        relative_asset,
        frame,
        animations,
        experimental_features,
    ) = if manifest_value
        .get("schemaVersion")
        .and_then(|value| value.as_u64())
        == Some(1)
    {
        let manifest: CompanionPackManifest = serde_json::from_value(manifest_value)?;
        let relative_asset = manifest.spritesheet.clone();
        let (id, display_name, description, frame, animations) = normalize_nexa_v1(manifest)?;
        (
            CompanionPackDialect::NexaV1,
            "native".to_string(),
            id,
            display_name,
            description,
            relative_asset,
            frame,
            animations,
            Vec::new(),
        )
    } else if manifest_value
        .get("schemaVersion")
        .and_then(|value| value.as_u64())
        == Some(2)
    {
        let manifest: NexaV2CompanionManifest = serde_json::from_value(manifest_value)?;
        if manifest.schema_version != 2 {
            return Err(CoreError::InvalidInput(
                "Unsupported Nexa Companion schema".to_string(),
            ));
        }
        (
            CompanionPackDialect::NexaV2,
            "native".to_string(),
            manifest.id,
            manifest.display_name,
            manifest.description,
            manifest.spritesheet,
            manifest.frame,
            manifest.animations,
            Vec::new(),
        )
    } else {
        let manifest: CodexCompanionManifest = serde_json::from_value(manifest_value)?;
        let sprite_version = manifest.sprite_version_number.unwrap_or(1);
        if !matches!(sprite_version, 1 | 2) {
            return Err(CoreError::InvalidInput(format!(
                "Unsupported Codex spriteVersionNumber {sprite_version}"
            )));
        }
        let dialect = if sprite_version == 2 {
            CompanionPackDialect::CodexDesktopV2
        } else {
            CompanionPackDialect::CodexTuiV1
        };
        let frame = manifest
            .frame
            .unwrap_or_else(|| default_codex_frame(if sprite_version == 2 { 11 } else { 9 }));
        if frame != default_codex_frame(if sprite_version == 2 { 11 } else { 9 }) {
            return Err(CoreError::InvalidInput(
                "Codex Companion frame geometry must match its versioned atlas".to_string(),
            ));
        }
        let id = manifest.id.unwrap_or_else(|| {
            package_root
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("codex-pet")
                .to_string()
        });
        let display_name = manifest.display_name.unwrap_or_else(|| id.clone());
        let animations = if manifest.animations.is_empty() {
            default_codex_animations()
        } else {
            manifest
                .animations
                .into_iter()
                .map(|(name, animation)| {
                    (
                        name,
                        NormalizedAnimation {
                            frames: animation.frames,
                            fps: animation.fps,
                            looping: animation.looping,
                            fallback: animation.fallback,
                        },
                    )
                })
                .collect()
        };
        let experimental = if sprite_version == 2 {
            vec!["directional_look_rows".to_string()]
        } else {
            Vec::new()
        };
        (
            dialect,
            if sprite_version == 2 {
                "experimental".to_string()
            } else {
                "compatible".to_string()
            },
            id,
            display_name,
            manifest.description,
            manifest.spritesheet_path,
            frame,
            animations,
            experimental,
        )
    };

    let id = validate_pack_id(&id)?;
    if managed && package_root.file_name().and_then(|value| value.to_str()) != Some(id.as_str()) {
        return Err(CoreError::InvalidInput(
            "Managed Companion Pack directory must match its validated id".to_string(),
        ));
    }
    if display_name.trim().is_empty() || display_name.len() > 160 {
        return Err(CoreError::InvalidInput(
            "Companion Pack display name must use 1-160 characters".to_string(),
        ));
    }
    let total_frames = validate_frame_grid(&frame)?;
    validate_normalized_animations(&animations, total_frames)?;
    let asset_path = resolve_pack_asset(package_root, &relative_asset)?;
    validate_spritesheet_geometry(&asset_path, &frame)?;
    let asset_bytes = fs::read(&asset_path)?;
    let mut hasher = Sha256::new();
    hasher.update(&manifest_bytes);
    hasher.update([0]);
    hasher.update(&asset_bytes);

    Ok(NormalizedCompanionPack {
        id,
        display_name: display_name.trim().to_string(),
        description: description.filter(|value| !value.trim().is_empty()),
        dialect,
        compatibility,
        spritesheet_path: asset_path.to_string_lossy().to_string(),
        content_hash: format!("{:x}", hasher.finalize()),
        frame,
        animations,
        experimental_features,
        managed,
    })
}

fn collect_manifest_candidates(root: &Path, names: &[&str]) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return candidates;
    };
    for entry in entries.flatten().take(512) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Some(candidate) = names
            .iter()
            .map(|name| path.join(name))
            .find(|candidate| candidate.is_file())
        {
            candidates.push(candidate);
        }
    }
    candidates.sort();
    candidates
}

pub fn resolve_codex_home(configured_path: Option<&str>) -> Option<PathBuf> {
    let raw = configured_path
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("CODEX_HOME").ok())?;

    #[cfg(target_os = "windows")]
    let raw = {
        let normalized = raw.replace('\\', "/");
        if let Some(rest) = normalized.strip_prefix("/mnt/") {
            let mut parts = rest.splitn(2, '/');
            let drive = parts.next().unwrap_or_default();
            let tail = parts.next().unwrap_or_default();
            if drive.len() == 1 && drive.as_bytes()[0].is_ascii_alphabetic() {
                format!(
                    "{}:\\{}",
                    drive.to_ascii_uppercase(),
                    tail.replace('/', "\\")
                )
            } else {
                raw
            }
        } else {
            raw
        }
    };

    let path = if raw == "~" {
        dirs::home_dir()?
    } else if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        dirs::home_dir()?.join(rest)
    } else {
        PathBuf::from(raw)
    };
    Some(path)
}

pub fn discover_codex_home(configured_path: Option<&str>) -> Option<PathBuf> {
    resolve_codex_home(configured_path).or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
}

pub fn scan_companion_packs(
    managed_root: &Path,
    codex_home: Option<&Path>,
) -> CompanionPackCatalog {
    let mut candidates = collect_manifest_candidates(managed_root, &["companion.json", "pet.json"])
        .into_iter()
        .map(|path| (path, true))
        .collect::<Vec<_>>();
    if let Some(codex_home) = codex_home {
        candidates.extend(
            collect_manifest_candidates(&codex_home.join("pets"), &["pet.json"])
                .into_iter()
                .map(|path| (path, false)),
        );
        candidates.extend(
            collect_manifest_candidates(&codex_home.join("avatars"), &["avatar.json"])
                .into_iter()
                .map(|path| (path, false)),
        );
    }

    let mut packs = Vec::new();
    let mut errors = Vec::new();
    for (manifest, managed) in candidates {
        match load_companion_pack(&manifest, managed) {
            Ok(pack) => packs.push(pack),
            Err(error) if errors.len() < MAX_CATALOG_ERRORS => {
                errors.push(CompanionPackCatalogError {
                    manifest_path: manifest.to_string_lossy().to_string(),
                    message: error.to_string(),
                });
            }
            Err(_) => {}
        }
    }
    packs.sort_by(|left, right| left.display_name.cmp(&right.display_name));
    packs.dedup_by(|left, right| left.id == right.id && left.content_hash == right.content_hash);
    CompanionPackCatalog { packs, errors }
}

pub fn companion_animation_for_state(
    state: CompanionState,
    animations: &HashMap<String, NormalizedAnimation>,
) -> Option<String> {
    let candidates: &[&str] = match state {
        CompanionState::Idle | CompanionState::Sleeping => &["idle"],
        CompanionState::WaitingForApproval | CompanionState::WaitingForUser => &["waiting", "idle"],
        CompanionState::Reviewing | CompanionState::ReadingFiles => &["review", "running", "idle"],
        CompanionState::Succeeded => &["waving", "wave", "jumping", "idle"],
        CompanionState::Failed | CompanionState::Cancelled => &["failed", "sad", "idle"],
        CompanionState::Thinking
        | CompanionState::Searching
        | CompanionState::Browsing
        | CompanionState::RunningTool
        | CompanionState::Coding => &["running", "idle"],
    };
    candidates
        .iter()
        .find(|candidate| animations.contains_key(**candidate))
        .map(|candidate| (*candidate).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_run::{
        AgentRunDisplayKind, AgentRunEventImportance, AgentRunEventPersistence,
        AgentRunEventVisibility, AGENT_RUN_EVENT_VERSION,
    };

    fn run(status: &str, phase: &str) -> AgentTaskRun {
        AgentTaskRun {
            id: "run-1".into(),
            conversation_id: "conversation-1".into(),
            turn_id: "turn-1".into(),
            user_message_id: "message-1".into(),
            status: status.into(),
            phase: phase.into(),
            title: "Task".into(),
            route_kind: None,
            summary: None,
            error_message: None,
            provider: None,
            model: None,
            plan: None,
            artifacts: None,
            created_at: "2026-08-08T00:00:00Z".into(),
            updated_at: "2026-08-08T00:00:00Z".into(),
            started_at: None,
            finished_at: None,
        }
    }

    fn event(kind: AgentRunEventKind, phase: AgentRunPhase, label: &str) -> AgentRunEvent {
        AgentRunEvent {
            version: AGENT_RUN_EVENT_VERSION,
            run_id: "run-1".into(),
            turn_id: "turn-1".into(),
            event_seq: 1,
            kind,
            phase,
            visibility: AgentRunEventVisibility::User,
            persistence: AgentRunEventPersistence::Durable,
            display_kind: AgentRunDisplayKind::Status,
            importance: AgentRunEventImportance::Normal,
            label: label.into(),
            status: Some("running".into()),
            payload: serde_json::json!({}),
            created_at: Some("2026-08-08T00:00:00Z".into()),
        }
    }

    #[test]
    fn projects_semantic_tool_and_waiting_states_from_durable_events() {
        let searching = project_companion_state(
            &run("running", "tooling"),
            &[event(
                AgentRunEventKind::ToolStarted,
                AgentRunPhase::Tooling,
                "web_search",
            )],
        );
        assert_eq!(searching.state, CompanionState::Searching);

        let waiting = project_companion_state(
            &run("running", "approval"),
            &[event(
                AgentRunEventKind::ApprovalRequested,
                AgentRunPhase::Approval,
                "run_shell",
            )],
        );
        assert_eq!(waiting.state, CompanionState::WaitingForApproval);
    }

    #[test]
    fn terminal_run_status_wins_over_stale_event_state() {
        let projection = project_companion_state(
            &run("completed", "done"),
            &[event(
                AgentRunEventKind::ToolStarted,
                AgentRunPhase::Tooling,
                "edit_file",
            )],
        );
        assert_eq!(projection.state, CompanionState::Succeeded);
        assert!(projection.terminal);
    }

    #[test]
    fn validates_safe_pack_and_hashes_manifest_with_asset() {
        let manifest = CompanionPackManifest {
            schema_version: COMPANION_PACK_SCHEMA_VERSION,
            id: "nexa-orbit".into(),
            display_name: "Nexa Orbit".into(),
            spritesheet: "sprites/orbit.webp".into(),
            frame: CompanionFrameGrid {
                width: 192,
                height: 208,
                columns: 8,
                rows: 9,
            },
            animations: HashMap::from([
                (
                    "idle".into(),
                    CompanionAnimation {
                        row: 0,
                        start_column: 0,
                        frames: 4,
                        fps: 8,
                        looping: true,
                        fallback: None,
                    },
                ),
                (
                    "thinking".into(),
                    CompanionAnimation {
                        row: 1,
                        start_column: 0,
                        frames: 6,
                        fps: 12,
                        looping: true,
                        fallback: Some("idle".into()),
                    },
                ),
            ]),
        };
        validate_companion_pack(&manifest, 1024).unwrap();
        let first = companion_pack_cache_key(&manifest, b"asset-a").unwrap();
        let second = companion_pack_cache_key(&manifest, b"asset-b").unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn rejects_parent_paths_scripts_and_unsafe_animation_limits() {
        let mut manifest = CompanionPackManifest {
            schema_version: 1,
            id: "unsafe".into(),
            display_name: "Unsafe".into(),
            spritesheet: "../pet.js".into(),
            frame: CompanionFrameGrid {
                width: 64,
                height: 64,
                columns: 8,
                rows: 1,
            },
            animations: HashMap::from([(
                "idle".into(),
                CompanionAnimation {
                    row: 0,
                    start_column: 0,
                    frames: 8,
                    fps: 12,
                    looping: true,
                    fallback: None,
                },
            )]),
        };
        assert!(validate_companion_pack(&manifest, 1024).is_err());
        manifest.spritesheet = "pet.webp".into();
        manifest.animations.get_mut("idle").unwrap().fps = 61;
        assert!(validate_companion_pack(&manifest, 1024).is_err());
    }

    fn write_test_atlas(path: &Path, width: u32, height: u32) {
        image::RgbaImage::from_pixel(width, height, image::Rgba([32, 64, 96, 255]))
            .save(path)
            .expect("write test atlas");
    }

    #[test]
    fn scans_and_normalizes_confirmed_codex_v1_packages() {
        let root = tempfile::tempdir().expect("tempdir");
        let package = root.path().join("pets").join("seedy");
        fs::create_dir_all(&package).unwrap();
        write_test_atlas(&package.join("spritesheet.png"), 1536, 1872);
        fs::write(
            package.join("pet.json"),
            serde_json::to_vec(&serde_json::json!({
                "id": "seedy",
                "displayName": "Seedy",
                "spritesheetPath": "spritesheet.png",
                "animations": {
                    "idle": { "frames": [0, 1, 2], "fps": 8, "loop": true },
                    "running": { "frames": [56, 57, 58], "fps": 12, "loop": true, "fallback": "idle" }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let catalog = scan_companion_packs(&root.path().join("managed"), Some(root.path()));
        assert!(catalog.errors.is_empty());
        assert_eq!(catalog.packs.len(), 1);
        assert_eq!(catalog.packs[0].dialect, CompanionPackDialect::CodexTuiV1);
        assert_eq!(catalog.packs[0].compatibility, "compatible");
        assert_eq!(catalog.packs[0].frame, default_codex_frame(9));
        assert_eq!(
            companion_animation_for_state(CompanionState::Coding, &catalog.packs[0].animations)
                .as_deref(),
            Some("running")
        );
    }

    #[test]
    fn codex_v2_is_feature_detected_and_unknown_versions_fail_closed() {
        let root = tempfile::tempdir().expect("tempdir");
        let package = root.path().join("v2-pet");
        fs::create_dir_all(&package).unwrap();
        write_test_atlas(&package.join("spritesheet.png"), 1536, 2288);
        let manifest = package.join("pet.json");
        fs::write(
            &manifest,
            serde_json::to_vec(&serde_json::json!({
                "id": "v2-pet",
                "displayName": "V2 Pet",
                "spriteVersionNumber": 2,
                "spritesheetPath": "spritesheet.png"
            }))
            .unwrap(),
        )
        .unwrap();

        let loaded = load_companion_pack(&manifest, false).expect("load v2 fixture");
        assert_eq!(loaded.dialect, CompanionPackDialect::CodexDesktopV2);
        assert_eq!(loaded.compatibility, "experimental");
        assert_eq!(loaded.experimental_features, vec!["directional_look_rows"]);

        fs::write(
            &manifest,
            serde_json::to_vec(&serde_json::json!({
                "id": "future-pet",
                "displayName": "Future Pet",
                "spriteVersionNumber": 3,
                "spritesheetPath": "spritesheet.png"
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(load_companion_pack(&manifest, false)
            .expect_err("unknown versions must not be guessed")
            .to_string()
            .contains("spriteVersionNumber 3"));
    }

    #[test]
    fn rejects_decoded_geometry_mismatch_and_manifest_path_escape() {
        let root = tempfile::tempdir().expect("tempdir");
        let package = root.path().join("unsafe-pet");
        fs::create_dir_all(&package).unwrap();
        write_test_atlas(&package.join("spritesheet.png"), 32, 32);
        let manifest = package.join("pet.json");
        fs::write(
            &manifest,
            serde_json::to_vec(&serde_json::json!({
                "id": "unsafe-pet",
                "displayName": "Unsafe Pet",
                "spritesheetPath": "spritesheet.png"
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(load_companion_pack(&manifest, false)
            .expect_err("wrong atlas geometry must fail")
            .to_string()
            .contains("expected 1536x1872"));

        fs::write(
            &manifest,
            serde_json::to_vec(&serde_json::json!({
                "id": "unsafe-pet",
                "displayName": "Unsafe Pet",
                "spritesheetPath": "../spritesheet.png"
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(load_companion_pack(&manifest, false).is_err());
    }

    #[test]
    fn rejects_managed_manifest_id_directory_mismatch() {
        let root = tempfile::tempdir().expect("tempdir");
        let package = root.path().join("other");
        fs::create_dir_all(&package).unwrap();
        write_test_atlas(&package.join("spritesheet.png"), 32, 32);
        let manifest = package.join("pet.json");
        fs::write(
            &manifest,
            serde_json::to_vec(&serde_json::json!({
                "id": "victim",
                "displayName": "Mismatched Pack",
                "spritesheetPath": "spritesheet.png"
            }))
            .unwrap(),
        )
        .unwrap();

        assert!(load_companion_pack(&manifest, true)
            .expect_err("managed identity must be bound to its directory")
            .to_string()
            .contains("directory must match"));
    }

    #[test]
    fn rejects_atlas_over_decoded_cache_budget() {
        assert!(validate_decoded_spritesheet_budget(1536, 2288).is_ok());
        assert!(validate_decoded_spritesheet_budget(8192, 8192)
            .expect_err("large compressed atlases must fail before WebView decode")
            .to_string()
            .contains("decoded cache limit"));
    }
}
