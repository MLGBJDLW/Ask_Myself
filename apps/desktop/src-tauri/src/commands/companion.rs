use super::*;

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use nexa_core::app_settings::{CompanionActiveRunPolicy, CompanionSettings};
use nexa_core::companion::{
    discover_codex_home, load_companion_pack, scan_companion_packs, CompanionPackCatalog,
    CompanionProjection, CompanionState, NormalizedCompanionPack,
};
use tauri::Emitter;

fn managed_companion_root(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|root| root.join("pets"))
        .map_err(|error| format!("Failed to resolve managed Companion directory: {error}"))
}

fn configured_codex_home(state: &AppState) -> Result<Option<PathBuf>, String> {
    let config = state
        .db
        .load_app_config()
        .map_err(|error| error.to_string())?;
    Ok(discover_codex_home(
        config.companion.codex_import_path.as_deref(),
    ))
}

#[tauri::command]
pub fn scan_companion_packs_cmd(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<CompanionPackCatalog, String> {
    let managed_root = managed_companion_root(&app)?;
    let codex_home = configured_codex_home(&state)?;
    Ok(scan_companion_packs(&managed_root, codex_home.as_deref()))
}

fn canonical_manifest_parent(manifest_path: &Path) -> Result<PathBuf, String> {
    manifest_path
        .parent()
        .ok_or_else(|| "Companion manifest has no package directory".to_string())?
        .canonicalize()
        .map_err(|error| format!("Companion package directory is unavailable: {error}"))
}

fn copy_validated_pack(
    manifest_path: &Path,
    pack: &NormalizedCompanionPack,
    managed_root: &Path,
) -> Result<PathBuf, String> {
    fs::create_dir_all(managed_root)
        .map_err(|error| format!("Failed to create managed Companion directory: {error}"))?;
    let destination = managed_root.join(&pack.id);
    if destination.exists() {
        let existing_manifest = ["companion.json", "pet.json"]
            .into_iter()
            .map(|name| destination.join(name))
            .find(|path| path.is_file());
        if let Some(existing_manifest) = existing_manifest {
            if load_companion_pack(&existing_manifest, true)
                .is_ok_and(|existing| existing.content_hash == pack.content_hash)
            {
                return Ok(existing_manifest);
            }
        }
        return Err(format!(
            "A managed Companion Pack named '{}' already exists",
            pack.id
        ));
    }

    let source_root = canonical_manifest_parent(manifest_path)?;
    let source_asset = PathBuf::from(&pack.spritesheet_path)
        .canonicalize()
        .map_err(|error| format!("Companion spritesheet is unavailable: {error}"))?;
    let relative_asset = source_asset.strip_prefix(&source_root).map_err(|_| {
        "Validated Companion spritesheet is outside its package directory".to_string()
    })?;
    let staging = managed_root.join(format!(".{}-{}.tmp", pack.id, Uuid::new_v4()));
    fs::create_dir(&staging)
        .map_err(|error| format!("Failed to create Companion staging directory: {error}"))?;

    let manifest_name =
        if manifest_path.file_name().and_then(|value| value.to_str()) == Some("companion.json") {
            "companion.json"
        } else {
            "pet.json"
        };
    let staged_manifest = staging.join(manifest_name);
    let staged_asset = staging.join(relative_asset);
    let result = (|| -> Result<(), String> {
        if let Some(parent) = staged_asset.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Failed to create Companion asset directory: {error}"))?;
        }
        fs::copy(manifest_path, &staged_manifest)
            .map_err(|error| format!("Failed to copy Companion manifest: {error}"))?;
        fs::copy(&source_asset, &staged_asset)
            .map_err(|error| format!("Failed to copy Companion spritesheet: {error}"))?;
        // The staging directory is intentionally randomized; bind the
        // manifest id to its final managed directory after the atomic rename.
        load_companion_pack(&staged_manifest, false)
            .map_err(|error| format!("Staged Companion validation failed: {error}"))?;
        fs::rename(&staging, &destination)
            .map_err(|error| format!("Failed to atomically install Companion Pack: {error}"))?;
        Ok(())
    })();
    if result.is_err() && staging.starts_with(managed_root) {
        let _ = fs::remove_dir_all(&staging);
    }
    result?;
    Ok(destination.join(manifest_name))
}

#[tauri::command]
pub fn import_companion_pack_cmd(
    app: AppHandle,
    manifest_path: String,
) -> Result<NormalizedCompanionPack, String> {
    let manifest_path = PathBuf::from(manifest_path);
    let pack = load_companion_pack(&manifest_path, false).map_err(|error| error.to_string())?;
    let managed_root = managed_companion_root(&app)?;
    let installed_manifest = copy_validated_pack(&manifest_path, &pack, &managed_root)?;
    load_companion_pack(&installed_manifest, true).map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanionAssetData {
    pub data_url: String,
    pub content_hash: String,
}

#[tauri::command]
pub fn read_companion_asset_cmd(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    pet_id: String,
    content_hash: String,
) -> Result<CompanionAssetData, String> {
    let managed_root = managed_companion_root(&app)?;
    let codex_home = configured_codex_home(&state)?;
    let catalog = scan_companion_packs(&managed_root, codex_home.as_deref());
    let pack = catalog
        .packs
        .into_iter()
        .find(|pack| pack.id == pet_id && pack.content_hash == content_hash)
        .ok_or_else(|| "Companion Pack identity is stale or unavailable".to_string())?;
    let bytes = fs::read(&pack.spritesheet_path)
        .map_err(|error| format!("Failed to read Companion spritesheet: {error}"))?;
    if bytes.len() > 10 * 1024 * 1024 {
        return Err("Companion spritesheet exceeds the safe size limit".to_string());
    }
    let mime = match Path::new(&pack.spritesheet_path)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        _ => return Err("Companion spritesheet has an unsupported type".to_string()),
    };
    Ok(CompanionAssetData {
        data_url: format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ),
        content_hash: pack.content_hash,
    })
}

#[tauri::command]
pub fn delete_managed_companion_pack_cmd(app: AppHandle, pet_id: String) -> Result<(), String> {
    let managed_root = managed_companion_root(&app)?;
    let catalog = scan_companion_packs(&managed_root, None);
    let pack = catalog
        .packs
        .into_iter()
        .find(|pack| pack.id == pet_id && pack.managed)
        .ok_or_else(|| "Managed Companion Pack was not found".to_string())?;
    let canonical_root = managed_root
        .canonicalize()
        .map_err(|error| format!("Managed Companion directory is unavailable: {error}"))?;
    let target = managed_root
        .join(&pack.id)
        .canonicalize()
        .map_err(|error| format!("Managed Companion Pack is unavailable: {error}"))?;
    if target == canonical_root || !target.starts_with(&canonical_root) {
        return Err("Refusing to delete outside the managed Companion directory".to_string());
    }
    let manifest = ["companion.json", "pet.json"]
        .into_iter()
        .map(|name| target.join(name))
        .find(|path| path.is_file())
        .ok_or_else(|| "Managed Companion Pack manifest is unavailable".to_string())?;
    let current = load_companion_pack(&manifest, true)
        .map_err(|error| format!("Managed Companion Pack failed identity validation: {error}"))?;
    if current.id != pack.id || current.content_hash != pack.content_hash {
        return Err("Managed Companion Pack identity changed; refresh before deleting".to_string());
    }
    fs::remove_dir_all(target)
        .map_err(|error| format!("Failed to delete managed Companion Pack: {error}"))
}

fn update_companion_settings(
    app: &AppHandle,
    state: &AppState,
    update: impl FnOnce(&mut CompanionSettings),
) -> Result<CompanionSettings, String> {
    let mut config = state
        .db
        .load_app_config()
        .map_err(|error| error.to_string())?;
    update(&mut config.companion);
    state
        .db
        .save_app_config(&config)
        .map_err(|error| error.to_string())?;
    let _ = app.emit("companion://settings-changed", &config.companion);
    Ok(config.companion)
}

fn is_active_status(status: &str) -> bool {
    !matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "completed" | "succeeded" | "failed" | "timed_out" | "cancelled" | "canceled"
    )
}

fn companion_state_priority(state: CompanionState) -> u8 {
    match state {
        CompanionState::WaitingForApproval => 8,
        CompanionState::WaitingForUser => 7,
        CompanionState::Failed => 6,
        CompanionState::Thinking
        | CompanionState::Searching
        | CompanionState::Browsing
        | CompanionState::ReadingFiles
        | CompanionState::RunningTool
        | CompanionState::Coding => 5,
        CompanionState::Reviewing => 4,
        CompanionState::Succeeded | CompanionState::Cancelled => 3,
        CompanionState::Idle => 2,
        CompanionState::Sleeping => 1,
    }
}

fn terminal_hold_is_active(
    run: &nexa_core::conversation::AgentTaskRun,
    settings: &CompanionSettings,
) -> bool {
    let Some(finished_at) = run.finished_at.as_deref() else {
        return false;
    };
    let Ok(finished_at) = DateTime::parse_from_rfc3339(finished_at) else {
        return false;
    };
    let hold_ms = if run.status.eq_ignore_ascii_case("failed") {
        settings.failure_hold_ms
    } else {
        settings.success_hold_ms
    };
    let age = Utc::now().signed_duration_since(finished_at.with_timezone(&Utc));
    age.num_milliseconds() >= 0 && age.num_milliseconds() <= i64::from(hold_ms)
}

#[tauri::command]
pub fn get_global_companion_projection_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<Option<CompanionProjection>, String> {
    let config = state
        .db
        .load_app_config()
        .map_err(|error| error.to_string())?;
    if config.companion.active_run_policy == CompanionActiveRunPolicy::PinnedRun {
        if let Some(run_id) = config.companion.pinned_run_id.as_deref() {
            return state
                .db
                .get_companion_projection(run_id)
                .map(Some)
                .map_err(|error| error.to_string());
        }
    }
    let runs = state
        .db
        .list_recent_agent_task_runs(50)
        .map_err(|error| error.to_string())?;
    if config.companion.active_run_policy == CompanionActiveRunPolicy::HighestPriority {
        let mut selected: Option<(u8, CompanionProjection)> = None;
        for item in &runs {
            if !is_active_status(&item.run.status)
                && !terminal_hold_is_active(&item.run, &config.companion)
            {
                continue;
            }
            let projection = state
                .db
                .get_companion_projection(&item.run.id)
                .map_err(|error| error.to_string())?;
            let priority = companion_state_priority(projection.state);
            if selected
                .as_ref()
                .is_none_or(|(current, _)| priority > *current)
            {
                selected = Some((priority, projection));
            }
        }
        return Ok(selected.map(|(_, projection)| projection));
    }
    let selected = match config.companion.active_run_policy {
        CompanionActiveRunPolicy::PinnedRun => config
            .companion
            .pinned_run_id
            .as_deref()
            .and_then(|run_id| runs.iter().find(|item| item.run.id == run_id)),
        CompanionActiveRunPolicy::PinnedProject => config
            .companion
            .pinned_project_id
            .as_deref()
            .and_then(|project_id| {
                runs.iter().find(|item| {
                    item.project_id.as_deref() == Some(project_id)
                        && is_active_status(&item.run.status)
                })
            }),
        CompanionActiveRunPolicy::HighestPriority => unreachable!("handled above"),
    }
    .or_else(|| {
        runs.first()
            .filter(|item| terminal_hold_is_active(&item.run, &config.companion))
    });

    selected
        .map(|item| state.db.get_companion_projection(&item.run.id))
        .transpose()
        .map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanionCommandResult {
    pub message: String,
    pub open_settings: bool,
}

#[tauri::command]
pub fn companion_command_cmd(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    input: String,
) -> Result<CompanionCommandResult, String> {
    let mut parts = input.split_whitespace();
    let action = parts.next().unwrap_or("status").to_ascii_lowercase();
    let mut open_settings = false;
    let message = match action.as_str() {
        "show" => {
            crate::companion_window::show_companion(&app)?;
            "Desktop Pet shown".to_string()
        }
        "hide" | "sleep" => {
            crate::companion_window::hide_companion(&app)?;
            "Desktop Pet hidden".to_string()
        }
        "toggle" => {
            crate::companion_window::toggle_companion_cmd(app.clone())?;
            "Desktop Pet visibility toggled".to_string()
        }
        "reset" => {
            crate::companion_window::reset_companion_position(&app)?;
            "Desktop Pet position reset".to_string()
        }
        "unlock" => {
            crate::companion_window::unlock_companion(&app)?;
            "Desktop Pet unlocked".to_string()
        }
        "enable" => {
            let settings = update_companion_settings(&app, &state, |settings| {
                settings.enabled = true;
            })?;
            crate::companion_window::apply_companion_settings(&app, &settings, true);
            "Desktop Pet enabled".to_string()
        }
        "disable" => {
            let settings = update_companion_settings(&app, &state, |settings| {
                settings.enabled = false;
            })?;
            crate::companion_window::apply_companion_settings(&app, &settings, false);
            "Desktop Pet disabled".to_string()
        }
        "select" => {
            let pet_id = parts
                .next()
                .ok_or_else(|| "Usage: /pet select <pet-id>".to_string())?;
            let managed_root = managed_companion_root(&app)?;
            let codex_home = configured_codex_home(&state)?;
            let catalog = scan_companion_packs(&managed_root, codex_home.as_deref());
            let pack = catalog
                .packs
                .iter()
                .find(|pack| pack.id == pet_id)
                .ok_or_else(|| format!("Desktop Pet '{pet_id}' was not found"))?;
            update_companion_settings(&app, &state, |settings| {
                settings.selected_pet_id = Some(pack.id.clone());
            })?;
            format!("Selected Desktop Pet '{}'", pack.display_name)
        }
        "settings" => {
            open_settings = true;
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
            }
            let _ = app.emit("companion://open-settings", ());
            "Opened Desktop Pet settings".to_string()
        }
        "status" | "" => {
            let config = state.db.load_app_config().map_err(|error| error.to_string())?;
            format!(
                "Desktop Pet is {} ({:?})",
                if config.companion.enabled { "enabled" } else { "disabled" },
                config.companion.interaction_mode
            )
        }
        _ => {
            return Err(
                "Unknown pet command. Use show, hide, sleep, toggle, enable, disable, unlock, reset, settings, status, or select <id>"
                    .to_string(),
            )
        }
    };
    Ok(CompanionCommandResult {
        message,
        open_settings,
    })
}
