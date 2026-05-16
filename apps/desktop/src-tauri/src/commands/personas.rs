use super::*;

#[tauri::command]
pub fn list_personas_cmd(state: tauri::State<'_, AppState>) -> Result<Vec<PersonaProfile>, String> {
    nexa_core::persona::list_personas(&state.db).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_persona_cmd(
    state: tauri::State<'_, AppState>,
    input: SavePersonaInput,
) -> Result<PersonaProfile, String> {
    state.db.save_persona(&input).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_persona_cmd(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    state.db.delete_persona(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn toggle_persona_cmd(
    state: tauri::State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    state
        .db
        .toggle_persona(&id, enabled)
        .map_err(|e| e.to_string())
}
