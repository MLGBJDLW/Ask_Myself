use super::*;
use tauri::Emitter;

// ── App Config ──────────────────────────────────────────────────────

#[tauri::command]
pub fn get_app_config_cmd(state: tauri::State<'_, AppState>) -> Result<AppConfig, String> {
    state.db.load_app_config().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_app_config_cmd(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
    config: AppConfig,
) -> Result<(), String> {
    state
        .db
        .save_app_config(&config)
        .map_err(|e| e.to_string())?;
    crate::companion_window::apply_companion_settings(
        &app_handle,
        &config.companion,
        config.companion.enabled,
    );
    let _ = app_handle.emit("companion://settings-changed", &config.companion);
    Ok(())
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
pub async fn generate_theme_resource_plugin_cmd(
    state: tauri::State<'_, AppState>,
    description: String,
) -> Result<nexa_core::theme_resource_plugin::ThemeResourcePlugin, String> {
    let description = description.trim();
    if description.is_empty() {
        return Err("Describe the theme you want first.".to_string());
    }
    if description.chars().count() > 4_000 {
        return Err("Theme descriptions must be at most 4000 characters.".to_string());
    }

    let config = state
        .db
        .get_default_agent_config()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Set a default chat model before generating a theme.".to_string())?;
    let provider = create_provider(db_config_to_provider_config(&config, Some(90)))
        .map_err(|error| error.to_string())?;
    let provider_type = provider_type_for_config(&config);
    let request = CompletionRequest {
        model: config.model.clone(),
        messages: vec![
            Message::text(
                Role::System,
                r##"You design safe declarative Nexa themes. Return one JSON object only; do not use Markdown.
Schema:
{
  "name": "1-80 character theme name",
  "theme": {
    "baseTheme": "dark|light|midnight|aurora|bloom|dream",
    "mode": "dark|light",
    "colors": {
      "surface0": "#RRGGBB", "surface1": "#RRGGBB", "surface2": "#RRGGBB",
      "textPrimary": "#RRGGBB", "textSecondary": "#RRGGBB", "accent": "#RRGGBB",
      "accentHover": "#RRGGBB", "accentSubtle": "#RRGGBB", "success": "#RRGGBB",
      "warning": "#RRGGBB", "danger": "#RRGGBB", "info": "#RRGGBB", "border": "#RRGGBB",
      "contextPrompts": "#RRGGBB", "contextConversation": "#RRGGBB",
      "contextToolResults": "#RRGGBB", "contextTools": "#RRGGBB",
      "contextMcp": "#RRGGBB", "contextOverhead": "#RRGGBB"
    },
    "effects": { "surfaceOpacity": 0.35-1, "glassBlur": 0-48, "shadowIntensity": 0-2, "radiusScale": 0.5-2 },
    "background": {
      "kind": "none|color|gradient", "value": "safe color or gradient without URL",
      "fit": "cover|contain|tile", "position": "center", "opacity": 0-1, "dim": 0-1,
      "blur": 0-32, "overlayColor": "#RRGGBB"
    }
  }
}
Use semantic contrast suitable for a desktop chat interface. Never emit CSS rules, url(), @import, scripts, or remote resources."##,
            ),
            Message::text(Role::User, description),
        ],
        temperature: Some(0.5),
        max_tokens: Some(2_048),
        tools: None,
        stop: None,
        thinking_budget: None,
        reasoning_enabled: Some(false),
        reasoning_effort: None,
        provider_type: Some(provider_type),
        routing_session_id: None,
        parallel_tool_calls: false,
    };
    let response = provider
        .complete(&request)
        .await
        .map_err(|error| format!("Theme generation failed: {error}"))?;
    let value = parse_model_json_object(&response.content)?;
    nexa_core::theme_resource_plugin::ThemeResourcePlugin::from_generated_value(value, description)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn generate_theme_background_cmd(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
    prompt: String,
) -> Result<ThemeBackgroundAsset, String> {
    use nexa_core::tools::image_generation_tool::GenerateImageTool;
    use nexa_core::tools::{Tool, ToolExecutionContext};

    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("Describe the theme background you want first.".to_string());
    }
    if prompt.chars().count() > 8_000 {
        return Err("Theme background prompts must be at most 8000 characters.".to_string());
    }

    let call_id = format!("theme-background-{}", uuid::Uuid::new_v4());
    let arguments = serde_json::json!({
        "prompt": format!(
            "Create a subtle desktop application background that leaves the center readable and contains no text, logos, UI, frames, or watermarks. Theme direction: {prompt}"
        ),
        "filename": "nexa-theme-background.png"
    })
    .to_string();
    let result = GenerateImageTool
        .execute(ToolExecutionContext::new(
            &call_id,
            &arguments,
            state.db.as_ref(),
            &[],
        ))
        .await
        .map_err(|error| error.to_string())?;
    if result.is_error {
        return Err(result.content);
    }
    let source_path = result
        .artifacts
        .as_ref()
        .and_then(|artifacts| artifacts.get("previewPath"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Image generation returned no preview asset.".to_string())?;

    let cache_root = app_handle
        .path()
        .app_cache_dir()
        .map_err(|error| format!("Failed to resolve app cache directory: {error}"))?;
    let data_root = app_handle
        .path()
        .app_data_dir()
        .map_err(|error| format!("Failed to resolve app data directory: {error}"))?;
    let store = nexa_core::managed_assets::ManagedLocalAssetStore::new(cache_root, data_root);
    let imported = store.import_theme_background(Path::new(source_path));
    let _ = std::fs::remove_file(source_path);
    let asset = imported.map_err(|error| error.to_string())?;
    Ok(ThemeBackgroundAsset {
        asset_id: asset.asset_id,
        path: asset.path.to_string_lossy().into_owned(),
        media_type: asset.media_type,
        bytes: asset.bytes,
    })
}

fn parse_model_json_object(content: &str) -> Result<serde_json::Value, String> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(content.trim()) {
        if value.is_object() {
            return Ok(value);
        }
    }

    let mut start = None;
    let mut depth = 0_u32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, character) in content.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' if start.is_some() => in_string = true,
            '{' => {
                start.get_or_insert(index);
                depth += 1;
            }
            '}' if start.is_some() => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let json = &content[start.expect("start exists")..index + character.len_utf8()];
                    return serde_json::from_str(json)
                        .map_err(|error| format!("The generated theme JSON is invalid: {error}"));
                }
            }
            _ => {}
        }
    }
    Err("The model did not return a theme JSON object.".to_string())
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

#[cfg(test)]
mod theme_resource_tests {
    use super::parse_model_json_object;

    #[test]
    fn extracts_json_from_a_fenced_model_response() {
        let value = parse_model_json_object(
            "Here is the draft:\n```json\n{\"name\":\"Ocean\",\"theme\":{}}\n```",
        )
        .expect("embedded object");

        assert_eq!(value["name"], "Ocean");
    }

    #[test]
    fn keeps_braces_inside_json_strings_balanced() {
        let value = parse_model_json_object(
            "prefix {\"name\":\"Ocean { calm }\",\"theme\":{\"mode\":\"dark\"}} suffix",
        )
        .expect("embedded object");

        assert_eq!(value["name"], "Ocean { calm }");
    }
}
