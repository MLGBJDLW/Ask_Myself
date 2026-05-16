use super::*;

// ── App Config ──────────────────────────────────────────────────────

#[tauri::command]
pub fn get_app_config_cmd(state: tauri::State<'_, AppState>) -> Result<AppConfig, String> {
    state.db.load_app_config().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_app_config_cmd(
    state: tauri::State<'_, AppState>,
    config: AppConfig,
) -> Result<(), String> {
    state.db.save_app_config(&config).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn check_office_runtime_cmd(
    app_handle: AppHandle,
) -> Result<nexa_core::office_runtime::OfficeRuntimeReadiness, String> {
    let data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data directory: {e}"))?;
    Ok(nexa_core::office_runtime::check_office_runtime(&data_dir))
}

#[tauri::command]
pub async fn prepare_office_runtime_cmd(
    app_handle: AppHandle,
    _state: tauri::State<'_, AppState>,
) -> Result<nexa_core::office_runtime::OfficePrepareResult, String> {
    let data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data directory: {e}"))?;

    tokio::task::spawn_blocking(move || {
        nexa_core::office_runtime::prepare_office_runtime(&data_dir).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

// ── Setup Wizard ───────────────────────────────────────────────────

#[tauri::command]
pub fn get_wizard_state_cmd(state: tauri::State<'_, AppState>) -> Result<WizardState, String> {
    state.db.load_wizard_state().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_wizard_completed_cmd(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state
        .db
        .save_wizard_state(&WizardState { completed: true })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn reset_wizard_cmd(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state
        .db
        .save_wizard_state(&WizardState { completed: false })
        .map_err(|e| e.to_string())
}
