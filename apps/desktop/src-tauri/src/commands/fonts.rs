use super::*;
use nexa_core::font_assets::{FontAsset, FontAssetStore};

fn font_store(app: &AppHandle) -> Result<FontAssetStore, String> {
    app.path()
        .app_data_dir()
        .map(FontAssetStore::new)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_font_assets_cmd(app_handle: AppHandle) -> Result<Vec<FontAsset>, String> {
    let store = font_store(&app_handle)?;
    tokio::task::spawn_blocking(move || store.list().map_err(|error| error.to_string()))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn import_font_assets_cmd(
    app_handle: AppHandle,
    source_path: String,
) -> Result<Vec<FontAsset>, String> {
    let store = font_store(&app_handle)?;
    tokio::task::spawn_blocking(move || {
        store
            .import(Path::new(&source_path))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn remove_font_asset_cmd(app_handle: AppHandle, asset_id: String) -> Result<(), String> {
    let store = font_store(&app_handle)?;
    tokio::task::spawn_blocking(move || store.remove(&asset_id).map_err(|error| error.to_string()))
        .await
        .map_err(|error| error.to_string())?
}
