use nexa_core::db::Database;
use nexa_core::db_executor::DatabaseExecutor;
use nexa_core::media_generation::adapters::{
    MiniMaxVideoAdapter, NormalizedVideoRequest, VideoGenerationAdapter, VideoInputAsset,
    VideoInputRole,
};
use nexa_core::media_generation::{
    AddVideoWorkflowShotRequest, BeginMediaJobAttemptRequest, CreateVideoWorkflowRequest,
    DeleteMediaAssetOccurrenceRequest, EnqueuePreparedVideoVariantsRequest,
    ImportMediaAssetRequest, LinkMediaAssetRequest, MediaAssetLocalRetentionPolicy,
    MediaAssetRelationType, MediaGenerationAssetStore, MediaGenerationRuntime,
    MediaJobAttemptState, MediaJobState, MediaOperation, RecordMediaProviderEventRequest,
    ReorderVideoWorkflowVariantsRequest, SaveVideoProviderConnectionRequest,
    SelectVideoWorkflowVariantRequest, TransitionMediaJobRequest, UpdateVideoWorkflowShotRequest,
    VideoShotInput, VideoWorkflowDagNodeKind,
};
use serde_json::json;
use sha2::{Digest, Sha256};

fn runtime() -> MediaGenerationRuntime {
    let database = Database::open_memory().expect("video workflow database");
    MediaGenerationRuntime::new(DatabaseExecutor::new(database, 8).expect("database executor"))
}

#[tokio::test]
async fn durable_shot_board_queues_an_idempotent_variant_batch() {
    let runtime = runtime();
    let connection = runtime
        .save_video_provider_connection(SaveVideoProviderConnectionRequest {
            id: None,
            expected_revision: None,
            provider_id: "minimax".to_string(),
            display_name: "Studio account".to_string(),
            api_key: "test-secret".to_string(),
            data_region: Some("provider_managed".to_string()),
        })
        .await
        .unwrap();
    let workflow = runtime
        .create_video_workflow(CreateVideoWorkflowRequest {
            project_id: Some("project-a".to_string()),
            title: "Launch film".to_string(),
            brief: json!({ "purpose": "launch" }),
            aspect_ratio: "16:9".to_string(),
            target_duration_ms: 30_000,
        })
        .await
        .unwrap();
    let with_shot = runtime
        .add_video_workflow_shot(AddVideoWorkflowShotRequest {
            workflow_id: workflow.workflow.id.clone(),
            expected_workflow_revision: workflow.workflow.revision,
            shot: VideoShotInput {
                title: "Opening".to_string(),
                prompt: "A clean studio product reveal".to_string(),
                operation: MediaOperation::TextToVideo,
                connection_id: Some(connection.id.clone()),
                provider_id: Some("minimax".to_string()),
                model_id: Some("MiniMax-H3".to_string()),
                api_version: Some("v2".to_string()),
                duration_seconds: 4,
                resolution: "768P".to_string(),
                aspect_ratio: "16:9".to_string(),
                input_assets: Vec::new(),
                seed: None,
                generate_audio: None,
                allow_cross_provider_fallback: false,
            },
        })
        .await
        .unwrap();
    let shot = with_shot.shots[0].shot.clone();
    let credential = runtime
        .materialize_video_provider_connection(&connection.id)
        .await
        .unwrap();
    let adapter =
        MiniMaxVideoAdapter::new(credential.api_key, &connection.credential_scope).unwrap();
    let request = EnqueuePreparedVideoVariantsRequest {
        workflow_id: workflow.workflow.id,
        expected_workflow_revision: with_shot.workflow.revision,
        shot_id: shot.id.clone(),
        expected_shot_revision: shot.revision,
        idempotency_key: "shot-opening-batch-one".to_string(),
        count: 2,
        expected_connection_revision: connection.revision,
        provider_source: adapter.provider_source().to_string(),
        normalized_request: NormalizedVideoRequest {
            idempotency_key: "shot-opening-batch-one".to_string(),
            model_id: shot.model_id.clone().unwrap(),
            operation: shot.operation,
            prompt: shot.prompt.clone(),
            duration_seconds: shot.duration_seconds,
            resolution: shot.resolution.clone(),
            aspect_ratio: shot.aspect_ratio.clone(),
            input_assets: Vec::new(),
            seed: None,
            generate_audio: None,
            callback_url: None,
        },
        estimated_cost_micros: Some(320_000),
        currency: Some("USD".to_string()),
    };
    let queued = runtime
        .enqueue_video_workflow_variants(request.clone())
        .await
        .unwrap();
    assert_eq!(queued.queue.draft, 2);
    assert_eq!(queued.queue.estimated_cost_micros, 640_000);
    assert_eq!(queued.shots[0].variants.len(), 2);
    assert_eq!(
        queued
            .dag
            .nodes
            .iter()
            .map(|node| node.kind)
            .collect::<Vec<_>>(),
        vec![
            VideoWorkflowDagNodeKind::Prompt,
            VideoWorkflowDagNodeKind::GenerateVideo,
            VideoWorkflowDagNodeKind::GenerateVideo,
            VideoWorkflowDagNodeKind::SelectVariant,
        ]
    );
    assert_eq!(queued.dag.nodes[1].variant_ids.len(), 1);
    assert_eq!(queued.dag.nodes[2].variant_ids.len(), 1);
    assert_ne!(
        queued.dag.nodes[1].variant_ids,
        queued.dag.nodes[2].variant_ids
    );
    assert_eq!(
        queued.dag.nodes[1].depends_on,
        vec![queued.dag.nodes[0].id.clone()]
    );
    assert_eq!(
        queued.dag.nodes[2].depends_on,
        vec![queued.dag.nodes[0].id.clone()]
    );
    assert_eq!(
        queued.dag.nodes[3].depends_on,
        vec![
            queued.dag.nodes[1].id.clone(),
            queued.dag.nodes[2].id.clone()
        ]
    );

    let update_connection = runtime
        .save_video_provider_connection(SaveVideoProviderConnectionRequest {
            id: Some(connection.id.clone()),
            expected_revision: Some(connection.revision),
            provider_id: connection.provider_id.clone(),
            display_name: "Changed account".to_string(),
            api_key: "changed-secret".to_string(),
            data_region: connection.data_region.clone(),
        })
        .await
        .unwrap_err();
    assert!(matches!(
        update_connection,
        nexa_core::error::CoreError::Conflict(_)
    ));

    let replayed = runtime
        .enqueue_video_workflow_variants(request.clone())
        .await
        .unwrap();
    assert_eq!(replayed.workflow.revision, queued.workflow.revision);
    assert_eq!(replayed.shots[0].variants, queued.shots[0].variants);

    let changed_count = EnqueuePreparedVideoVariantsRequest {
        count: 1,
        ..request
    };
    assert!(runtime
        .enqueue_video_workflow_variants(changed_count)
        .await
        .is_err());

    let original_order = queued.shots[0]
        .variants
        .iter()
        .map(|variant| variant.id.clone())
        .collect::<Vec<_>>();
    let reordered = runtime
        .reorder_video_workflow_variants(ReorderVideoWorkflowVariantsRequest {
            workflow_id: queued.workflow.id.clone(),
            expected_workflow_revision: queued.workflow.revision,
            shot_id: queued.shots[0].shot.id.clone(),
            expected_shot_revision: queued.shots[0].shot.revision,
            ordered_variant_ids: original_order.iter().rev().cloned().collect(),
        })
        .await
        .unwrap();
    assert_eq!(reordered.shots[0].variants[0].id, original_order[1]);
    assert_eq!(reordered.shots[0].variants[1].id, original_order[0]);

    let historical_job_id = reordered.shots[0].variants[0].job_id.clone();
    let historical = runtime
        .video_variant_execution_context(&historical_job_id)
        .await
        .unwrap();
    assert_eq!(historical.shot.prompt, "A clean studio product reveal");
    let historical_shot_revision = historical.shot.revision;
    let current_shot = reordered.shots[0].shot.clone();
    let updated = runtime
        .update_video_workflow_shot(UpdateVideoWorkflowShotRequest {
            workflow_id: reordered.workflow.id,
            expected_workflow_revision: reordered.workflow.revision,
            shot_id: current_shot.id.clone(),
            expected_shot_revision: current_shot.revision,
            shot: VideoShotInput {
                title: current_shot.title,
                prompt: "A revised product reveal with a warmer palette".to_string(),
                operation: current_shot.operation,
                connection_id: current_shot.connection_id,
                provider_id: current_shot.provider_id,
                model_id: current_shot.model_id,
                api_version: current_shot.api_version,
                duration_seconds: current_shot.duration_seconds,
                resolution: current_shot.resolution,
                aspect_ratio: current_shot.aspect_ratio,
                input_assets: current_shot.input_assets,
                seed: current_shot.seed,
                generate_audio: current_shot.generate_audio,
                allow_cross_provider_fallback: false,
            },
        })
        .await
        .unwrap();
    assert_eq!(
        updated.shots[0].shot.prompt,
        "A revised product reveal with a warmer palette"
    );
    let historical_prompt_id = format!(
        "shot:{}:revision:{}:prompt",
        updated.shots[0].shot.id, historical_shot_revision
    );
    assert!(updated
        .dag
        .nodes
        .iter()
        .filter(|node| node.kind == VideoWorkflowDagNodeKind::GenerateVideo)
        .all(|node| node.depends_on.contains(&historical_prompt_id)));
    let preserved = runtime
        .video_variant_execution_context(&historical_job_id)
        .await
        .unwrap();
    assert_eq!(preserved.shot.prompt, "A clean studio product reveal");
}

#[tokio::test]
async fn workflow_rejects_ephemeral_or_credential_bearing_reference_locators() {
    let runtime = runtime();
    let workflow = runtime
        .create_video_workflow(CreateVideoWorkflowRequest {
            project_id: None,
            title: "References".to_string(),
            brief: json!({}),
            aspect_ratio: "16:9".to_string(),
            target_duration_ms: 5_000,
        })
        .await
        .unwrap();
    for uri in [
        "runway://uploaded/provider-secret",
        "mm_file://uploaded/provider-secret",
        "https://cdn.example/reference.png?token=secret",
    ] {
        let error = runtime
            .add_video_workflow_shot(AddVideoWorkflowShotRequest {
                workflow_id: workflow.workflow.id.clone(),
                expected_workflow_revision: workflow.workflow.revision,
                shot: VideoShotInput {
                    title: "Unsafe reference".to_string(),
                    prompt: "Animate this frame".to_string(),
                    operation: MediaOperation::ImageToVideo,
                    connection_id: None,
                    provider_id: None,
                    model_id: None,
                    api_version: None,
                    duration_seconds: 5,
                    resolution: "768P".to_string(),
                    aspect_ratio: "16:9".to_string(),
                    input_assets: vec![VideoInputAsset {
                        role: VideoInputRole::FirstFrame,
                        uri: uri.to_string(),
                        media_type: "image/png".to_string(),
                        metadata_verified: true,
                        byte_length: Some(1_024),
                        content_hash_sha256: Some("ab".repeat(32)),
                        local_asset_id: None,
                        width: Some(1280),
                        height: Some(720),
                        duration_ms: None,
                        frame_rate: None,
                        video_codec: None,
                    }],
                    seed: None,
                    generate_audio: None,
                    allow_cross_provider_fallback: false,
                },
            })
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            nexa_core::error::CoreError::InvalidInput(_)
        ));
    }
}

#[tokio::test]
async fn deleting_a_selected_output_occurrence_clears_the_selection_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = MediaGenerationRuntime::with_asset_store(
        DatabaseExecutor::new(Database::open_memory().unwrap(), 8).unwrap(),
        MediaGenerationAssetStore::new(directory.path().join("assets")),
    );
    let connection = runtime
        .save_video_provider_connection(SaveVideoProviderConnectionRequest {
            id: None,
            expected_revision: None,
            provider_id: "minimax".to_string(),
            display_name: "Selection test".to_string(),
            api_key: "selection-secret".to_string(),
            data_region: None,
        })
        .await
        .unwrap();
    let workflow = runtime
        .create_video_workflow(CreateVideoWorkflowRequest {
            project_id: None,
            title: "Selection cleanup".to_string(),
            brief: json!({}),
            aspect_ratio: "16:9".to_string(),
            target_duration_ms: 4_000,
        })
        .await
        .unwrap();
    let with_shot = runtime
        .add_video_workflow_shot(AddVideoWorkflowShotRequest {
            workflow_id: workflow.workflow.id,
            expected_workflow_revision: workflow.workflow.revision,
            shot: VideoShotInput {
                title: "Opening".to_string(),
                prompt: "A product reveal".to_string(),
                operation: MediaOperation::TextToVideo,
                connection_id: Some(connection.id.clone()),
                provider_id: Some("minimax".to_string()),
                model_id: Some("MiniMax-H3".to_string()),
                api_version: Some("v2".to_string()),
                duration_seconds: 4,
                resolution: "768P".to_string(),
                aspect_ratio: "16:9".to_string(),
                input_assets: Vec::new(),
                seed: None,
                generate_audio: None,
                allow_cross_provider_fallback: false,
            },
        })
        .await
        .unwrap();
    let shot = with_shot.shots[0].shot.clone();
    let adapter =
        MiniMaxVideoAdapter::new("selection-secret", &connection.credential_scope).unwrap();
    let queued = runtime
        .enqueue_video_workflow_variants(EnqueuePreparedVideoVariantsRequest {
            workflow_id: with_shot.workflow.id.clone(),
            expected_workflow_revision: with_shot.workflow.revision,
            shot_id: shot.id.clone(),
            expected_shot_revision: shot.revision,
            idempotency_key: "selection-batch".to_string(),
            count: 1,
            expected_connection_revision: connection.revision,
            provider_source: adapter.provider_source().to_string(),
            normalized_request: NormalizedVideoRequest {
                idempotency_key: "selection-batch".to_string(),
                model_id: "MiniMax-H3".to_string(),
                operation: MediaOperation::TextToVideo,
                prompt: shot.prompt.clone(),
                duration_seconds: 4,
                resolution: "768P".to_string(),
                aspect_ratio: "16:9".to_string(),
                input_assets: Vec::new(),
                seed: None,
                generate_audio: None,
                callback_url: None,
            },
            estimated_cost_micros: None,
            currency: None,
        })
        .await
        .unwrap();
    let job_id = queued.shots[0].variants[0].job_id.clone();
    let mut job = runtime.get_job(&job_id).await.unwrap();
    for next_state in [
        MediaJobState::Validating,
        MediaJobState::UploadingAssets,
        MediaJobState::Submitting,
    ] {
        job = runtime
            .transition_job(TransitionMediaJobRequest {
                job_id: job_id.clone(),
                expected_revision: job.job.revision,
                next_state,
            })
            .await
            .unwrap();
    }
    job = runtime
        .begin_attempt(BeginMediaJobAttemptRequest {
            job_id: job_id.clone(),
            expected_revision: job.job.revision,
            idempotency_key: "selection-attempt".to_string(),
            provider_id: "minimax".to_string(),
            provider_source: adapter.provider_source().to_string(),
            model_id: "MiniMax-H3".to_string(),
            api_version: Some("v2".to_string()),
            data_region: None,
            remote_retention_expires_at: None,
            provider_unknown_reconciliation: None,
        })
        .await
        .unwrap();
    let attempt_id = job.job.current_attempt_id.clone().unwrap();
    job = runtime
        .record_provider_event(RecordMediaProviderEventRequest {
            job_id: job_id.clone(),
            expected_revision: job.job.revision,
            attempt_id: attempt_id.clone(),
            provider_id: "minimax".to_string(),
            event_source: adapter.provider_source().to_string(),
            deduplication_key: "selection-accepted".to_string(),
            event_kind: "provider.submitted".to_string(),
            payload: json!({ "status": "queued" }),
            provider_created_at: None,
            provider_task_id: Some("selection-task".to_string()),
            attempt_state: Some(MediaJobAttemptState::Accepted),
            next_job_state: Some(MediaJobState::Queued),
            error: None,
            retry_classification: None,
            next_eligible_at: None,
            cancellation_result: None,
            final_cost_micros: None,
            watermark_present: None,
            provenance: None,
        })
        .await
        .unwrap();
    job = runtime
        .record_provider_event(RecordMediaProviderEventRequest {
            job_id: job_id.clone(),
            expected_revision: job.job.revision,
            attempt_id: attempt_id.clone(),
            provider_id: "minimax".to_string(),
            event_source: adapter.provider_source().to_string(),
            deduplication_key: "selection-running".to_string(),
            event_kind: "provider.status.running".to_string(),
            payload: json!({ "status": "running" }),
            provider_created_at: None,
            provider_task_id: Some("selection-task".to_string()),
            attempt_state: Some(MediaJobAttemptState::Observing),
            next_job_state: Some(MediaJobState::Running),
            error: None,
            retry_classification: None,
            next_eligible_at: None,
            cancellation_result: None,
            final_cost_micros: None,
            watermark_present: None,
            provenance: None,
        })
        .await
        .unwrap();
    job = runtime
        .record_provider_event(RecordMediaProviderEventRequest {
            job_id: job_id.clone(),
            expected_revision: job.job.revision,
            attempt_id: attempt_id.clone(),
            provider_id: "minimax".to_string(),
            event_source: adapter.provider_source().to_string(),
            deduplication_key: "selection-succeeded".to_string(),
            event_kind: "provider.status.succeeded".to_string(),
            payload: json!({ "status": "succeeded" }),
            provider_created_at: None,
            provider_task_id: Some("selection-task".to_string()),
            attempt_state: Some(MediaJobAttemptState::Succeeded),
            next_job_state: Some(MediaJobState::PostProcessing),
            error: None,
            retry_classification: None,
            next_eligible_at: None,
            cancellation_result: None,
            final_cost_micros: None,
            watermark_present: None,
            provenance: None,
        })
        .await
        .unwrap();
    let source = directory.path().join("output.mp4");
    let bytes = b"\0\0\0\x0cftypisom";
    std::fs::write(&source, bytes).unwrap();
    let hash = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let asset = runtime
        .import_asset(ImportMediaAssetRequest {
            source_path: source,
            declared_media_type: "video/mp4".to_string(),
            expected_sha256: Some(hash),
            expected_byte_length: Some(bytes.len() as u64),
            width: Some(1280),
            height: Some(720),
            duration_ms: Some(4_000),
        })
        .await
        .unwrap();
    job = runtime
        .link_asset(LinkMediaAssetRequest {
            job_id: job_id.clone(),
            expected_revision: job.job.revision,
            idempotency_key: "selection-output".to_string(),
            attempt_id,
            asset_id: asset.id,
            parent_asset_id: None,
            relation_type: MediaAssetRelationType::Output,
            ordinal: 0,
            local_retention_policy: MediaAssetLocalRetentionPolicy::RetainUntilDeleted,
            local_retention_expires_at: None,
            metadata: json!({}),
        })
        .await
        .unwrap();
    job = runtime
        .transition_job(TransitionMediaJobRequest {
            job_id: job_id.clone(),
            expected_revision: job.job.revision,
            next_state: MediaJobState::Completed,
        })
        .await
        .unwrap();
    let selected = runtime
        .select_video_workflow_variant(SelectVideoWorkflowVariantRequest {
            workflow_id: queued.workflow.id.clone(),
            expected_workflow_revision: queued.workflow.revision,
            shot_id: shot.id,
            expected_shot_revision: shot.revision,
            variant_id: queued.shots[0].variants[0].id.clone(),
        })
        .await
        .unwrap();
    assert!(selected.shots[0].shot.selected_variant_id.is_some());
    let relation_id = job.asset_relations[0].id.clone();
    runtime
        .delete_asset_occurrence(DeleteMediaAssetOccurrenceRequest {
            job_id,
            expected_revision: job.job.revision,
            relation_id,
        })
        .await
        .unwrap();
    let cleared = runtime
        .get_video_workflow(&queued.workflow.id)
        .await
        .unwrap();
    assert_eq!(cleared.shots[0].shot.selected_variant_id, None);
    assert!(cleared.workflow.revision > selected.workflow.revision);
}

#[tokio::test]
async fn renderer_connection_projection_never_contains_the_api_key() {
    let runtime = runtime();
    let saved = runtime
        .save_video_provider_connection(SaveVideoProviderConnectionRequest {
            id: None,
            expected_revision: None,
            provider_id: "runway".to_string(),
            display_name: "Production".to_string(),
            api_key: "renderer-must-not-see-this".to_string(),
            data_region: None,
        })
        .await
        .unwrap();
    let serialized = serde_json::to_string(&saved).unwrap();
    assert!(!serialized.contains("renderer-must-not-see-this"));
    let listed = runtime.list_video_provider_connections().await.unwrap();
    assert!(!serde_json::to_string(&listed)
        .unwrap()
        .contains("renderer-must-not-see-this"));
}
