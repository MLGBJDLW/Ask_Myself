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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechPreview {
    pub asset_id: String,
    pub path: String,
    pub media_type: String,
    pub bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClearSpeechCacheResult {
    pub removed_files: u64,
    pub removed_bytes: u64,
}

#[tauri::command]
pub async fn clear_speech_cache_cmd(
    app_handle: AppHandle,
) -> Result<ClearSpeechCacheResult, String> {
    let cache_root = app_handle
        .path()
        .app_cache_dir()
        .map_err(|error| format!("Failed to resolve app cache directory: {error}"))?;
    let data_root = app_handle
        .path()
        .app_data_dir()
        .map_err(|error| format!("Failed to resolve app data directory: {error}"))?;
    let store = nexa_core::managed_assets::ManagedLocalAssetStore::new(cache_root, data_root);
    let (removed_files, removed_bytes) = store
        .clear_audio_cache()
        .map_err(|error| error.to_string())?;
    Ok(ClearSpeechCacheResult {
        removed_files,
        removed_bytes,
    })
}

#[tauri::command]
pub async fn refresh_tts_voice_catalog_cmd(
    config: TextToSpeechConfig,
) -> Result<TtsVoiceCatalogSnapshot, String> {
    let refreshed_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    if !supports_dynamic_tts_voice_catalog(&config.api_style) {
        return Ok(build_tts_voice_catalog(&config, None, refreshed_at));
    }
    match discover_tts_voices(&config).await {
        Ok(voices) => Ok(build_tts_voice_catalog(&config, Some(voices), refreshed_at)),
        Err(error) => {
            warn!(
                "Voice catalog refresh failed for TTS provider {}: {}",
                config.provider, error
            );
            Err(error.to_string())
        }
    }
}

#[tauri::command]
pub async fn synthesize_speech_preview_cmd(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
    text: String,
    config: Option<TextToSpeechConfig>,
) -> Result<SpeechPreview, String> {
    use nexa_core::tools::text_to_speech_tool::synthesize_speech_preview;

    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("Speech text cannot be empty".into());
    }
    let tts = match config {
        Some(config) => config,
        None => {
            state
                .db
                .load_app_config()
                .map_err(|error| error.to_string())?
                .text_to_speech
        }
    };
    let cache_key_material = format!(
        "{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}",
        tts.provider,
        tts.api_style,
        tts.base_url.as_deref().unwrap_or_default().trim(),
        tts.api_key.trim(),
        tts.model,
        tts.voice,
        tts.speed,
        tts.output_format,
        trimmed.split_whitespace().collect::<Vec<_>>().join(" ")
    );
    let cache_root = app_handle
        .path()
        .app_cache_dir()
        .map_err(|error| format!("Failed to resolve app cache directory: {error}"))?;
    let data_root = app_handle
        .path()
        .app_data_dir()
        .map_err(|error| format!("Failed to resolve app data directory: {error}"))?;
    let store = nexa_core::managed_assets::ManagedLocalAssetStore::new(cache_root, data_root);
    if let Some(managed) = store
        .cached_audio(&cache_key_material)
        .map_err(|error| error.to_string())?
    {
        return Ok(SpeechPreview {
            asset_id: managed.asset_id,
            path: managed.path.to_string_lossy().into_owned(),
            media_type: managed.media_type,
            bytes: managed.bytes,
        });
    }
    let preview = synthesize_speech_preview(&tts, trimmed, None, None, None, None)
        .await
        .map_err(|error| error.to_string())?;
    let source_path = preview.path;
    let managed = store
        .cache_audio(&source_path, &cache_key_material, &preview.media_type)
        .map_err(|error| error.to_string())?;
    if source_path != managed.path {
        let _ = std::fs::remove_file(source_path);
    }
    let _ = store.prune_audio_cache(512 * 1024 * 1024, Duration::from_secs(30 * 24 * 60 * 60));
    Ok(SpeechPreview {
        asset_id: managed.asset_id,
        path: managed.path.to_string_lossy().into_owned(),
        media_type: managed.media_type,
        bytes: managed.bytes,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeBackgroundAsset {
    pub asset_id: String,
    pub path: String,
    pub media_type: String,
    pub bytes: u64,
}

#[tauri::command]
pub async fn import_theme_background_cmd(
    app_handle: AppHandle,
    source_path: String,
) -> Result<ThemeBackgroundAsset, String> {
    let cache_root = app_handle
        .path()
        .app_cache_dir()
        .map_err(|error| format!("Failed to resolve app cache directory: {error}"))?;
    let data_root = app_handle
        .path()
        .app_data_dir()
        .map_err(|error| format!("Failed to resolve app data directory: {error}"))?;
    let store = nexa_core::managed_assets::ManagedLocalAssetStore::new(cache_root, data_root);
    let asset = store
        .import_theme_background(Path::new(source_path.trim()))
        .map_err(|error| error.to_string())?;
    Ok(ThemeBackgroundAsset {
        asset_id: asset.asset_id,
        path: asset.path.to_string_lossy().into_owned(),
        media_type: asset.media_type,
        bytes: asset.bytes,
    })
}

#[tauri::command]
pub async fn resolve_theme_background_cmd(
    app_handle: AppHandle,
    asset_id: String,
) -> Result<ThemeBackgroundAsset, String> {
    let cache_root = app_handle
        .path()
        .app_cache_dir()
        .map_err(|error| format!("Failed to resolve app cache directory: {error}"))?;
    let data_root = app_handle
        .path()
        .app_data_dir()
        .map_err(|error| format!("Failed to resolve app data directory: {error}"))?;
    let store = nexa_core::managed_assets::ManagedLocalAssetStore::new(cache_root, data_root);
    let asset = store
        .resolve_theme_background(asset_id.trim())
        .map_err(|error| error.to_string())?;
    Ok(ThemeBackgroundAsset {
        asset_id: asset.asset_id,
        path: asset.path.to_string_lossy().into_owned(),
        media_type: asset.media_type,
        bytes: asset.bytes,
    })
}

#[tauri::command]
pub async fn garbage_collect_theme_assets_cmd(
    app_handle: AppHandle,
    retained_asset_ids: Vec<String>,
) -> Result<ClearSpeechCacheResult, String> {
    let cache_root = app_handle
        .path()
        .app_cache_dir()
        .map_err(|error| format!("Failed to resolve app cache directory: {error}"))?;
    let data_root = app_handle
        .path()
        .app_data_dir()
        .map_err(|error| format!("Failed to resolve app data directory: {error}"))?;
    let store = nexa_core::managed_assets::ManagedLocalAssetStore::new(cache_root, data_root);
    let (removed_files, removed_bytes) = store
        .garbage_collect_theme_assets(&retained_asset_ids)
        .map_err(|error| error.to_string())?;
    Ok(ClearSpeechCacheResult {
        removed_files,
        removed_bytes,
    })
}

#[tauri::command]
pub fn get_web_search_status_cmd(
    state: tauri::State<'_, AppState>,
    web_search: Option<nexa_core::app_settings::WebSearchConfig>,
) -> Result<Vec<nexa_core::web_search::WebSearchProviderStatus>, String> {
    let config = match web_search {
        Some(config) => config,
        None => {
            let config = state.db.load_app_config().map_err(|e| e.to_string())?;
            config.web_search
        }
    };
    Ok(nexa_core::tools::web_search_tool::provider_status_snapshot(
        &config,
    ))
}

#[tauri::command]
pub async fn check_office_runtime_cmd(
    app_handle: AppHandle,
) -> Result<nexa_core::office_runtime::OfficeRuntimeReadiness, String> {
    let data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data directory: {e}"))?;
    tokio::task::spawn_blocking(move || nexa_core::office_runtime::check_office_runtime(&data_dir))
        .await
        .map_err(|e| format!("spawn_blocking: {e}"))
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
