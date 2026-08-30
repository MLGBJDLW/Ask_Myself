use serde::Deserialize;
use tauri::State;

use super::policy::NavigationActor;
use super::state::{
    BrowserBounds, BrowserControlOwner, BrowserSessionInfo, BrowserState, BrowserTabInfo,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserCreateInput {
    pub conversation_id: Option<String>,
    pub profile_id: Option<String>,
    pub url: Option<String>,
    #[serde(default)]
    pub open_initial_url_on_reuse: bool,
    pub bounds: Option<BrowserBounds>,
}

#[tauri::command]
pub async fn browser_create_session_cmd(
    state: State<'_, BrowserState>,
    input: BrowserCreateInput,
) -> Result<BrowserSessionInfo, String> {
    state
        .create_session(
            input.conversation_id,
            input.profile_id,
            input.url.as_deref(),
            input.open_initial_url_on_reuse,
            NavigationActor::User,
            input.bounds,
        )
        .await
}

#[tauri::command]
pub fn browser_list_sessions_cmd(
    state: State<'_, BrowserState>,
) -> Result<Vec<BrowserSessionInfo>, String> {
    state.list_sessions()
}

#[tauri::command]
pub fn browser_active_session_cmd(
    state: State<'_, BrowserState>,
    conversation_id: String,
) -> Result<Option<BrowserSessionInfo>, String> {
    state.active_session(&conversation_id)
}

#[tauri::command]
pub async fn browser_open_tab_cmd(
    state: State<'_, BrowserState>,
    session_id: String,
    url: String,
    bounds: Option<BrowserBounds>,
) -> Result<BrowserTabInfo, String> {
    state
        .open_tab(&session_id, &url, NavigationActor::User, bounds)
        .await
}

#[tauri::command]
pub async fn browser_open_popup_cmd(
    state: State<'_, BrowserState>,
    session_id: String,
    source_tab_id: String,
    url: String,
    bounds: Option<BrowserBounds>,
) -> Result<BrowserTabInfo, String> {
    state
        .open_popup(&session_id, &source_tab_id, &url, bounds)
        .await
}

#[tauri::command]
pub async fn browser_navigate_cmd(
    state: State<'_, BrowserState>,
    session_id: String,
    tab_id: String,
    url: String,
) -> Result<BrowserTabInfo, String> {
    state
        .navigate(&session_id, &tab_id, &url, NavigationActor::User, None)
        .await
}

#[tauri::command]
pub fn browser_activate_tab_cmd(
    state: State<'_, BrowserState>,
    session_id: String,
    tab_id: String,
) -> Result<BrowserSessionInfo, String> {
    state.acquire_control(&session_id, BrowserControlOwner::User)?;
    state.activate_tab(&session_id, &tab_id)
}

#[tauri::command]
pub fn browser_set_bounds_cmd(
    state: State<'_, BrowserState>,
    session_id: String,
    bounds: BrowserBounds,
    visible: bool,
    visibility_revision: u64,
) -> Result<(), String> {
    state.set_bounds(&session_id, bounds, visible, visibility_revision)
}

#[tauri::command]
pub fn browser_go_back_cmd(
    state: State<'_, BrowserState>,
    session_id: String,
    tab_id: String,
) -> Result<(), String> {
    state.acquire_control(&session_id, BrowserControlOwner::User)?;
    state.go_back(&session_id, &tab_id)
}

#[tauri::command]
pub fn browser_go_forward_cmd(
    state: State<'_, BrowserState>,
    session_id: String,
    tab_id: String,
) -> Result<(), String> {
    state.acquire_control(&session_id, BrowserControlOwner::User)?;
    state.go_forward(&session_id, &tab_id)
}

#[tauri::command]
pub fn browser_reload_cmd(
    state: State<'_, BrowserState>,
    session_id: String,
    tab_id: String,
) -> Result<(), String> {
    state.acquire_control(&session_id, BrowserControlOwner::User)?;
    state.reload(&session_id, &tab_id)
}

#[tauri::command]
pub fn browser_stop_cmd(
    state: State<'_, BrowserState>,
    session_id: String,
    tab_id: String,
) -> Result<(), String> {
    state.acquire_control(&session_id, BrowserControlOwner::User)?;
    state.stop(&session_id, &tab_id)
}

#[tauri::command]
pub async fn browser_begin_element_pick_cmd(
    state: State<'_, BrowserState>,
    session_id: String,
    tab_id: String,
) -> Result<(), String> {
    state.acquire_control(&session_id, BrowserControlOwner::User)?;
    state
        .eval_action(
            &session_id,
            &tab_id,
            "window.__NEXA_BROWSER_RUNTIME__?.beginPick('element')",
        )
        .await?;
    Ok(())
}

#[tauri::command]
pub async fn browser_begin_region_pick_cmd(
    state: State<'_, BrowserState>,
    session_id: String,
    tab_id: String,
) -> Result<(), String> {
    state.acquire_control(&session_id, BrowserControlOwner::User)?;
    state
        .eval_action(
            &session_id,
            &tab_id,
            "window.__NEXA_BROWSER_RUNTIME__?.beginPick('region')",
        )
        .await?;
    Ok(())
}

#[tauri::command]
pub async fn browser_take_pick_cmd(
    state: State<'_, BrowserState>,
    session_id: String,
    tab_id: String,
) -> Result<Option<serde_json::Value>, String> {
    let value = state
        .eval_action(
            &session_id,
            &tab_id,
            "window.__NEXA_BROWSER_RUNTIME__?.takeArtifact()",
        )
        .await?;
    Ok((!value.is_null()).then_some(value))
}

#[tauri::command]
pub async fn browser_selected_text_cmd(
    state: State<'_, BrowserState>,
    session_id: String,
    tab_id: String,
) -> Result<String, String> {
    state
        .eval_action(
            &session_id,
            &tab_id,
            "window.__NEXA_BROWSER_RUNTIME__?.selectedText() || ''",
        )
        .await?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "Could not read selected page text".to_string())
}

#[tauri::command]
pub fn browser_acquire_control_cmd(
    state: State<'_, BrowserState>,
    session_id: String,
    owner: String,
) -> Result<BrowserSessionInfo, String> {
    match owner.as_str() {
        "user" => state.acquire_control(&session_id, BrowserControlOwner::User),
        "none" => state.release_control(&session_id),
        _ => Err("Browser control owner must be user or none".to_string()),
    }
}

#[tauri::command]
pub fn browser_close_tab_cmd(
    state: State<'_, BrowserState>,
    session_id: String,
    tab_id: String,
) -> Result<BrowserSessionInfo, String> {
    state.acquire_control(&session_id, BrowserControlOwner::User)?;
    state.close_tab(&session_id, &tab_id)
}

#[tauri::command]
pub fn browser_close_session_cmd(
    state: State<'_, BrowserState>,
    session_id: String,
) -> Result<(), String> {
    state.acquire_control(&session_id, BrowserControlOwner::User)?;
    state.close_session(&session_id)
}
