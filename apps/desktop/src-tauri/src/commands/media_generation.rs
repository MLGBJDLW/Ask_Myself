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

#[tauri::command]
pub async fn save_video_provider_connection_cmd(
    state: tauri::State<'_, AppState>,
    request: nexa_core::media_generation::SaveVideoProviderConnectionRequest,
) -> Result<nexa_core::media_generation::VideoProviderConnectionRecord, String> {
    state
        .media_generation
        .save_video_provider_connection(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_video_provider_connections_cmd(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<nexa_core::media_generation::VideoProviderConnectionRecord>, String> {
    state
        .media_generation
        .list_video_provider_connections()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn delete_video_provider_connection_cmd(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    expected_revision: u64,
) -> Result<(), String> {
    state
        .media_generation
        .delete_video_provider_connection(&connection_id, expected_revision)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn create_video_workflow_cmd(
    state: tauri::State<'_, AppState>,
    request: nexa_core::media_generation::CreateVideoWorkflowRequest,
) -> Result<nexa_core::media_generation::VideoWorkflowSnapshot, String> {
    state
        .media_generation
        .create_video_workflow(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn update_video_workflow_cmd(
    state: tauri::State<'_, AppState>,
    request: nexa_core::media_generation::UpdateVideoWorkflowRequest,
) -> Result<nexa_core::media_generation::VideoWorkflowSnapshot, String> {
    state
        .media_generation
        .update_video_workflow(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_video_workflows_cmd(
    state: tauri::State<'_, AppState>,
    project_id: Option<String>,
) -> Result<Vec<nexa_core::media_generation::VideoWorkflowSnapshot>, String> {
    state
        .media_generation
        .list_video_workflows(project_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_video_workflow_cmd(
    state: tauri::State<'_, AppState>,
    workflow_id: String,
) -> Result<nexa_core::media_generation::VideoWorkflowSnapshot, String> {
    state
        .media_generation
        .get_video_workflow(&workflow_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn add_video_workflow_shot_cmd(
    state: tauri::State<'_, AppState>,
    request: nexa_core::media_generation::AddVideoWorkflowShotRequest,
) -> Result<nexa_core::media_generation::VideoWorkflowSnapshot, String> {
    state
        .media_generation
        .add_video_workflow_shot(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn update_video_workflow_shot_cmd(
    state: tauri::State<'_, AppState>,
    request: nexa_core::media_generation::UpdateVideoWorkflowShotRequest,
) -> Result<nexa_core::media_generation::VideoWorkflowSnapshot, String> {
    state
        .media_generation
        .update_video_workflow_shot(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn reorder_video_workflow_shots_cmd(
    state: tauri::State<'_, AppState>,
    request: nexa_core::media_generation::ReorderVideoWorkflowShotsRequest,
) -> Result<nexa_core::media_generation::VideoWorkflowSnapshot, String> {
    state
        .media_generation
        .reorder_video_workflow_shots(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn reorder_video_workflow_variants_cmd(
    state: tauri::State<'_, AppState>,
    request: nexa_core::media_generation::ReorderVideoWorkflowVariantsRequest,
) -> Result<nexa_core::media_generation::VideoWorkflowSnapshot, String> {
    state
        .media_generation
        .reorder_video_workflow_variants(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn delete_video_workflow_shot_cmd(
    state: tauri::State<'_, AppState>,
    request: nexa_core::media_generation::DeleteVideoWorkflowShotRequest,
) -> Result<nexa_core::media_generation::VideoWorkflowSnapshot, String> {
    state
        .media_generation
        .delete_video_workflow_shot(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn queue_video_shot_variants_cmd(
    state: tauri::State<'_, AppState>,
    request: nexa_core::media_generation::QueueVideoShotVariantsRequest,
) -> Result<nexa_core::media_generation::VideoWorkflowSnapshot, String> {
    state
        .video_generation_coordinator
        .queue_shot_variants(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn preview_video_shot_queue_cmd(
    state: tauri::State<'_, AppState>,
    request: nexa_core::media_generation::PreviewVideoShotQueueRequest,
) -> Result<nexa_core::media_generation::VideoQueueDisclosure, String> {
    state
        .video_generation_coordinator
        .preview_shot_queue(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn inspect_video_reference_image_cmd(
    state: tauri::State<'_, AppState>,
    uri: String,
) -> Result<nexa_core::media_generation::VerifiedVideoReferenceImage, String> {
    state
        .video_generation_coordinator
        .inspect_reference_image(uri)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn retry_video_variant_cmd(
    state: tauri::State<'_, AppState>,
    request: nexa_core::media_generation::RetryVideoVariantRequest,
) -> Result<nexa_core::media_generation::VideoWorkflowSnapshot, String> {
    state
        .video_generation_coordinator
        .retry_variant(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn cancel_video_variant_cmd(
    state: tauri::State<'_, AppState>,
    request: nexa_core::media_generation::CancelVideoVariantRequest,
) -> Result<nexa_core::media_generation::VideoWorkflowSnapshot, String> {
    state
        .video_generation_coordinator
        .cancel_variant(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn select_video_workflow_variant_cmd(
    state: tauri::State<'_, AppState>,
    request: nexa_core::media_generation::SelectVideoWorkflowVariantRequest,
) -> Result<nexa_core::media_generation::VideoWorkflowSnapshot, String> {
    state
        .video_generation_coordinator
        .select_variant(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn resolve_media_generation_asset_path_cmd(
    state: tauri::State<'_, AppState>,
    asset_id: String,
) -> Result<String, String> {
    state
        .media_generation
        .resolve_asset_path(&asset_id)
        .await
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|error| error.to_string())
}
