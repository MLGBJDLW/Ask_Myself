use super::*;

/// Returns the shared evidence-backed manifest that the renderer must use for
/// model availability and validation controls. It includes non-selectable
/// watchlist entries so the UI never has to infer release status.
#[tauri::command]
pub async fn list_video_generation_capabilities_cmd(
) -> Result<Vec<nexa_core::video_provider_catalog::VideoProviderPreset>, String> {
    nexa_core::video_provider_catalog::load_video_provider_presets()
        .map_err(|error| error.to_string())
}

/// Creates the durable provider-neutral job only. Provider submission belongs
/// to an adapter and never runs inside this renderer-facing command.
#[tauri::command]
pub async fn create_media_generation_job_cmd(
    state: tauri::State<'_, AppState>,
    request: nexa_core::media_generation::CreateMediaJobRequest,
) -> Result<nexa_core::media_generation::MediaJobSnapshot, String> {
    state
        .media_generation
        .create_job(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_media_generation_job_cmd(
    state: tauri::State<'_, AppState>,
    job_id: String,
) -> Result<nexa_core::media_generation::MediaJobSnapshot, String> {
    state
        .media_generation
        .get_job(&job_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_recoverable_media_generation_jobs_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<nexa_core::media_generation::MediaJobSnapshot>, String> {
    state
        .media_generation
        .list_recoverable_jobs()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_media_generation_provider_events_cmd(
    state: tauri::State<'_, AppState>,
    job_id: String,
    after_sequence: u64,
    limit: u32,
) -> Result<Vec<nexa_core::media_generation::MediaProviderEventRecord>, String> {
    state
        .media_generation
        .list_provider_events(&job_id, after_sequence, limit)
        .await
        .map_err(|error| error.to_string())
}

/// Records cancellation intent. The job remains non-terminal until an adapter
/// confirms cancellation through a durable provider event.
#[tauri::command]
pub async fn request_media_generation_cancellation_cmd(
    state: tauri::State<'_, AppState>,
    request: nexa_core::media_generation::RequestMediaJobCancellation,
) -> Result<nexa_core::media_generation::MediaJobSnapshot, String> {
    state
        .media_generation
        .request_cancellation(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn request_media_generation_remote_deletion_cmd(
    state: tauri::State<'_, AppState>,
    request: nexa_core::media_generation::RequestMediaJobRemoteDeletion,
) -> Result<nexa_core::media_generation::MediaJobSnapshot, String> {
    state
        .media_generation
        .request_remote_deletion(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn delete_media_generation_asset_occurrence_cmd(
    state: tauri::State<'_, AppState>,
    request: nexa_core::media_generation::DeleteMediaAssetOccurrenceRequest,
) -> Result<nexa_core::media_generation::MediaJobSnapshot, String> {
    state
        .media_generation
        .delete_asset_occurrence(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn delete_media_generation_asset_cmd(
    state: tauri::State<'_, AppState>,
    request: nexa_core::media_generation::RequestMediaAssetDeletion,
) -> Result<nexa_core::media_generation::MediaAssetRecord, String> {
    state
        .media_generation
        .delete_asset(request)
        .await
        .map_err(|error| error.to_string())
}
