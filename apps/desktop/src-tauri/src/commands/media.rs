use super::*;

// ── OCR ─────────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_ocr_config_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<nexa_core::ocr::OcrConfig, String> {
    state.db.load_ocr_config().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_ocr_config_cmd(
    state: tauri::State<'_, AppState>,
    config: nexa_core::ocr::OcrConfig,
) -> Result<(), String> {
    state.db.save_ocr_config(&config).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn check_ocr_models_cmd(config: nexa_core::ocr::OcrConfig) -> Result<bool, String> {
    tokio::task::spawn_blocking(move || nexa_core::ocr::check_ocr_models_exist(&config))
        .await
        .map_err(|e| format!("spawn_blocking: {e}"))
}

#[tauri::command]
pub async fn download_ocr_models_cmd(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
    config: nexa_core::ocr::OcrConfig,
) -> Result<(), String> {
    let app_cfg = state.db.load_app_config().map_err(|e| e.to_string())?;
    let hf_mirror_base = app_cfg.hf_mirror_base_url.clone();
    let ghproxy_base = app_cfg.ghproxy_base_url.clone();
    tokio::task::spawn_blocking(move || {
        nexa_core::ocr::download_ocr_models(&config, &hf_mirror_base, &ghproxy_base, |progress| {
            emit_app_event(&app_handle, "ocr:download-progress", &progress);
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

#[tauri::command]
pub fn delete_ocr_models_cmd(config: nexa_core::ocr::OcrConfig) -> Result<(), String> {
    nexa_core::ocr::delete_ocr_models(&config).map_err(|e| e.to_string())
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedModelPaths {
    pub root: String,
    pub embedding: String,
    pub ocr: String,
    pub whisper: String,
}

#[tauri::command]
pub fn get_managed_model_paths_cmd(
    root: Option<String>,
    local_model: Option<String>,
) -> Result<ManagedModelPaths, String> {
    let custom_root = root.filter(|path| !path.trim().is_empty());
    let root = match custom_root.as_deref() {
        Some(path) => {
            let path = std::path::PathBuf::from(path.trim());
            if !path.is_absolute() {
                return Err("local model root must be an absolute path".to_string());
            }
            path
        }
        None => nexa_core::embed::default_model_root().map_err(|error| error.to_string())?,
    };
    let model = local_model
        .map(|value| nexa_core::embed::LocalEmbeddingModel::from_config_str(&value))
        .unwrap_or_default();
    let whisper = if custom_root.is_some() {
        root.join("whisper")
    } else {
        #[cfg(feature = "video")]
        {
            std::path::PathBuf::from(nexa_core::video::VideoConfig::default().model_path)
        }
        #[cfg(not(feature = "video"))]
        {
            root.join("whisper")
        }
    };
    Ok(ManagedModelPaths {
        embedding: root.join(model.model_name()).to_string_lossy().into_owned(),
        ocr: root.join("paddleocr").to_string_lossy().into_owned(),
        whisper: whisper.to_string_lossy().into_owned(),
        root: root.to_string_lossy().into_owned(),
    })
}

// ── Video ───────────────────────────────────────────────────────────

#[cfg(feature = "video")]
#[tauri::command]
pub fn get_video_config_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<nexa_core::video::VideoConfig, String> {
    state.db.load_video_config().map_err(|e| e.to_string())
}

#[cfg(feature = "video")]
#[tauri::command]
pub fn save_video_config_cmd(
    state: tauri::State<'_, AppState>,
    config: nexa_core::video::VideoConfig,
) -> Result<(), String> {
    state
        .db
        .save_video_config(&config)
        .map_err(|e| e.to_string())
}

#[cfg(feature = "video")]
#[tauri::command]
pub async fn check_whisper_model_cmd(
    config: nexa_core::video::VideoConfig,
) -> Result<bool, String> {
    tokio::task::spawn_blocking(move || nexa_core::video::check_whisper_model_exists(&config))
        .await
        .map_err(|e| format!("spawn_blocking: {e}"))
}

#[cfg(feature = "video")]
#[tauri::command]
pub async fn download_whisper_model_cmd(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
    config: nexa_core::video::VideoConfig,
) -> Result<(), String> {
    let app_cfg = state.db.load_app_config().map_err(|e| e.to_string())?;
    let hf_mirror_base = app_cfg.hf_mirror_base_url.clone();
    tokio::task::spawn_blocking(move || {
        nexa_core::video::download_whisper_model(&config, &hf_mirror_base, |progress| {
            emit_app_event(&app_handle, "video:download-progress", &progress);
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

#[cfg(feature = "video")]
#[tauri::command]
pub async fn check_ffmpeg_cmd(config: nexa_core::video::VideoConfig) -> Result<bool, String> {
    tokio::task::spawn_blocking(move || {
        nexa_core::video::check_ffmpeg(&config).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

#[cfg(feature = "video")]
#[tauri::command]
pub async fn download_ffmpeg_cmd(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let data_dir = app_handle
        .path()
        .app_local_data_dir()
        .map_err(|e| format!("Failed to get data dir: {e}"))?;
    let db = state.db.clone();
    let app_cfg = db.load_app_config().map_err(|e| e.to_string())?;
    let ghproxy_base = app_cfg.ghproxy_base_url.clone();

    let path = tokio::task::spawn_blocking(move || {
        nexa_core::video::download_ffmpeg(&data_dir, &ghproxy_base, |progress| {
            emit_app_event(&app_handle, "ffmpeg:download-progress", &progress);
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))??;

    let path_str = path.to_string_lossy().to_string();

    // Auto-save ffmpeg path to config
    let mut config = db.load_video_config().map_err(|e| e.to_string())?;
    config.ffmpeg_path = Some(path_str.clone());
    db.save_video_config(&config).map_err(|e| e.to_string())?;

    Ok(path_str)
}

#[cfg(feature = "video")]
#[tauri::command]
pub fn delete_whisper_model_cmd(state: tauri::State<'_, AppState>) -> Result<(), String> {
    if state.whisper_busy.load(Ordering::SeqCst) {
        return Err("Cannot delete model while transcription is in progress".into());
    }
    let config = state.db.load_video_config().map_err(|e| e.to_string())?;
    nexa_core::video::delete_whisper_model(&config).map_err(|e| e.to_string())
}

#[cfg(feature = "video")]
#[tauri::command]
pub async fn transcribe_audio_buffer_cmd(
    audio_data: Vec<u8>,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let db = state.db.clone();
    let whisper_busy = state.whisper_busy.clone();
    if whisper_busy.swap(true, Ordering::SeqCst) {
        return Err("Transcription already in progress".into());
    }
    struct BusyGuard(Arc<AtomicBool>);
    impl Drop for BusyGuard {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let _busy_guard = BusyGuard(whisper_busy);

    let app_config = db.load_app_config().map_err(|e| e.to_string())?;
    let speech_config = app_config.speech_to_text;
    match speech_config.api_style.as_str() {
        "openai_transcription" | "dashscope_asr" => {
            nexa_core::speech_to_text::transcribe_cloud_wav(audio_data, &speech_config)
                .await
                .map_err(|e| e.to_string())
        }
        "sherpa_onnx" => tokio::task::spawn_blocking(move || {
            with_voice_wav(&audio_data, |wav_path| {
                nexa_core::speech_to_text::transcribe_sherpa_wav(wav_path, &speech_config)
                    .map_err(|e| e.to_string())
            })
        })
        .await
        .map_err(|e| format!("spawn_blocking: {e}"))?,
        "local_whisper" => tokio::task::spawn_blocking(move || {
            let config = db.load_video_config().map_err(|e| e.to_string())?;
            with_voice_wav(&audio_data, |wav_path| {
                let segments = nexa_core::video::transcribe_audio(wav_path, &config)
                    .map_err(|e| e.to_string())?;
                Ok(segments
                    .iter()
                    .map(|segment| segment.text.trim())
                    .collect::<Vec<_>>()
                    .join(" "))
            })
        })
        .await
        .map_err(|e| format!("spawn_blocking: {e}"))?,
        style => Err(format!("Unsupported speech-to-text API style: {style}")),
    }
}

#[cfg(feature = "video")]
fn with_voice_wav<T>(
    audio_data: &[u8],
    operation: impl FnOnce(&std::path::Path) -> Result<T, String>,
) -> Result<T, String> {
    let temp_dir = std::env::temp_dir().join("nexa-voice");
    std::fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;
    let wav_path = temp_dir.join(format!("voice-{}.wav", Uuid::new_v4()));
    std::fs::write(&wav_path, audio_data).map_err(|e| e.to_string())?;
    struct WavGuard(PathBuf);
    impl Drop for WavGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _guard = WavGuard(wav_path.clone());
    operation(&wav_path)
}

#[cfg(all(test, feature = "video"))]
mod tests {
    use super::*;

    #[test]
    fn managed_model_defaults_preserve_the_legacy_whisper_location() {
        let paths = get_managed_model_paths_cmd(None, None).expect("default paths should resolve");

        assert_eq!(
            std::path::PathBuf::from(paths.whisper),
            std::path::PathBuf::from(nexa_core::video::VideoConfig::default().model_path),
        );
    }

    #[test]
    fn managed_model_custom_root_moves_whisper_with_the_other_models() {
        let custom_root = std::env::current_dir()
            .expect("current directory should resolve")
            .join("custom-model-root");
        let paths =
            get_managed_model_paths_cmd(Some(custom_root.to_string_lossy().into_owned()), None)
                .expect("custom paths should resolve");

        assert_eq!(
            std::path::PathBuf::from(paths.whisper),
            custom_root.join("whisper"),
        );
    }
}

#[cfg(feature = "video")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptChunk {
    pub text: String,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub chunk_type: String,
}

#[cfg(feature = "video")]
#[tauri::command]
pub async fn analyze_video_cmd(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<serde_json::Value, String> {
    let db = state.db.clone();
    let whisper_busy = state.whisper_busy.clone();

    // Validate path is within a registered source directory.
    validate_path_in_scope(&db, &path)?;

    tokio::task::spawn_blocking(move || {
        let config = db.load_video_config().map_err(|e| e.to_string())?;
        let file_path = std::path::Path::new(&path);
        if !file_path.is_file() {
            return Err(format!("File not found: {path}"));
        }

        let file_name = file_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        // Set whisper_busy guard; ensure it resets even on panic.
        whisper_busy.store(true, Ordering::SeqCst);
        struct WhisperGuard(Arc<AtomicBool>);
        impl Drop for WhisperGuard {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let _guard = WhisperGuard(whisper_busy);

        let ah = app_handle.clone();
        let fname = file_name.clone();
        let result = nexa_core::video::analyze_video(file_path, &config, move |progress| {
            emit_app_event(
                &ah,
                "video:processing-progress",
                &serde_json::json!({
                    "progress": progress.progress_pct,
                    "phase": progress.phase,
                    "detail": progress.detail,
                    "fileName": &fname,
                }),
            );
        })
        .map_err(|e| e.to_string())?;

        let segment_count = result.transcript_segments.len();
        let frame_texts_count = result.frame_texts.len();
        Ok(serde_json::json!({
            "transcript": result.full_transcript,
            "segmentCount": segment_count,
            "transcriptSegments": result.transcript_segments,
            "durationSecs": result.duration_secs,
            "frameTextsCount": frame_texts_count,
            "frameTexts": result.frame_texts,
            "thumbnailPath": result.thumbnail_path.map(|p| p.to_string_lossy().to_string()),
            "metadata": result.metadata,
        }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

#[cfg(feature = "video")]
#[tauri::command]
pub async fn get_video_transcript_cmd(
    state: tauri::State<'_, AppState>,
    file_path: String,
) -> Result<Vec<TranscriptChunk>, String> {
    let db = state.db.clone();

    // Validate path is within a registered source directory.
    validate_path_in_scope(&db, &file_path)?;

    tokio::task::spawn_blocking(move || {
        let rows = db
            .list_document_chunks_by_path(&file_path)
            .map_err(|e| e.to_string())?;
        let chunks = rows
            .into_iter()
            .map(|row| {
                let heading: Option<String> =
                    serde_json::from_str::<serde_json::Value>(&row.metadata_json)
                        .ok()
                        .and_then(|v| {
                            v.get("heading_context")
                                .and_then(|h| h.as_str().map(String::from))
                        });
                let chunk_type = if heading
                    .as_deref()
                    .is_some_and(|h| h.starts_with("[Frame OCR"))
                {
                    "frame_ocr"
                } else {
                    "transcript"
                };
                TranscriptChunk {
                    text: row.content,
                    start_ms: Some(row.start_offset),
                    end_ms: Some(row.end_offset),
                    chunk_type: chunk_type.to_string(),
                }
            })
            .collect::<Vec<_>>();
        Ok(chunks)
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

#[cfg(feature = "video")]
#[tauri::command]
pub async fn get_video_metadata_cmd(
    state: tauri::State<'_, AppState>,
    file_path: String,
) -> Result<serde_json::Value, String> {
    let db = state.db.clone();

    // Validate path is within a registered source directory.
    validate_path_in_scope(&db, &file_path)?;

    tokio::task::spawn_blocking(move || {
        match db.get_document_storage_metadata_by_path(&file_path) {
            Ok(document) => {
                let meta: serde_json::Value =
                    serde_json::from_str(&document.metadata_json).unwrap_or(serde_json::json!({}));
                Ok(serde_json::json!({
                    "mimeType": document.mime_type,
                    "durationSecs": meta.get("duration_secs").and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))),
                    "width": meta.get("video_width").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))),
                    "height": meta.get("video_height").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))),
                    "codec": meta.get("video_codec").and_then(|v| v.as_str()),
                    "framerate": meta.get("video_framerate").and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))),
                    "thumbnailPath": meta.get("thumbnail_path").and_then(|v| v.as_str()),
                    "creationTime": meta.get("video_creation_time").and_then(|v| v.as_str()),
                }))
            }
            Err(e) => Err(e.to_string()),
        }
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}
