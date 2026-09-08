use super::*;
use nexa_core::turn_file_changes::TurnFileChangeSummary;

#[tauri::command]
pub async fn get_conversation_file_changes_cmd(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<Vec<TurnFileChangeSummary>, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        db.conversation_file_changes(&conversation_id)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn get_turn_file_diff_cmd(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    turn_id: String,
    absolute_path: String,
) -> Result<serde_json::Value, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        db.turn_file_diff(&conversation_id, &turn_id, &absolute_path)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}
