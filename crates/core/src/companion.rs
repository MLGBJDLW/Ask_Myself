//! Durable Agent lifecycle projection and safe Companion Pack contract.

use std::collections::HashMap;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::agent_run::{AgentRunEvent, AgentRunEventKind, AgentRunPhase};
use crate::conversation::AgentTaskRun;
use crate::db::Database;
use crate::error::CoreError;

pub const COMPANION_PACK_SCHEMA_VERSION: u16 = 1;
const MAX_SPRITESHEET_BYTES: u64 = 10 * 1024 * 1024;
const MAX_FRAME_EDGE: u32 = 512;
const MAX_GRID_EDGE: u32 = 16;
const MAX_TOTAL_FRAMES: u32 = 256;
const MAX_ANIMATION_FPS: u16 = 24;

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
        manifest.animations.get_mut("idle").unwrap().fps = 60;
        assert!(validate_companion_pack(&manifest, 1024).is_err());
    }
}
