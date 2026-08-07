use nexa_core::media_generation::adapters::{
    CostEstimateKind, MiniMaxHailuoVideoAdapter, MiniMaxVideoAdapter, NormalizedVideoRequest,
    RunwayVideoAdapter, VideoGenerationAdapter, VideoInputAsset, VideoInputRole,
};
use nexa_core::media_generation::MediaOperation;
use nexa_core::video_provider_catalog::{
    find_video_provider_preset, load_video_provider_presets, VideoModelReleaseStatus,
};
use serde_json::Value;

const MINIMAX_CONTRACT: &str =
    include_str!("fixtures/video_providers/minimax-contract-projection.json");
const RUNWAY_CONTRACT: &str =
    include_str!("fixtures/video_providers/runway-contract-projection.json");

fn request(model_id: &str, operation: MediaOperation) -> NormalizedVideoRequest {
    NormalizedVideoRequest {
        idempotency_key: "job-1-attempt-1".to_string(),
        model_id: model_id.to_string(),
        operation,
        prompt: "A quiet ocean at dawn".to_string(),
        duration_seconds: 5,
        resolution: "720P".to_string(),
        aspect_ratio: "16:9".to_string(),
        input_assets: Vec::new(),
        seed: None,
        generate_audio: None,
        callback_url: None,
    }
}

#[test]
fn manifest_scopes_release_status_to_the_exact_provider_contract() {
    let providers = load_video_provider_presets().expect("video provider manifest should parse");
    let minimax_h3 = providers
        .iter()
        .find(|provider| provider.provider_id == "minimax")
        .and_then(|provider| {
            provider
                .models
                .iter()
                .find(|model| model.model_id == "MiniMax-H3")
        })
        .expect("MiniMax H3 capability");
    assert_eq!(minimax_h3.release_status, VideoModelReleaseStatus::Ga);
    assert!(minimax_h3.selectable);
    assert!(!minimax_h3.supports_webhook);
    let h3_text = minimax_h3
        .operation_capabilities
        .iter()
        .find(|capability| capability.operation == MediaOperation::TextToVideo)
        .unwrap();
    assert!(!h3_text
        .aspect_ratios
        .iter()
        .any(|ratio| ratio == "adaptive"));
    let h3_image = minimax_h3
        .operation_capabilities
        .iter()
        .find(|capability| capability.operation == MediaOperation::ImageToVideo)
        .unwrap();
    assert_eq!(h3_image.aspect_ratios, ["adaptive"]);

    let runway_seedance = providers
        .iter()
        .find(|provider| provider.provider_id == "runway")
        .and_then(|provider| {
            provider
                .models
                .iter()
                .find(|model| model.model_id == "seedance2_5")
        })
        .expect("Runway Seedance 2.5 capability");
    assert_eq!(runway_seedance.release_status, VideoModelReleaseStatus::Ga);
    assert!(runway_seedance.selectable);

    let runway_gen45 = providers
        .iter()
        .find(|provider| provider.provider_id == "runway")
        .and_then(|provider| {
            provider
                .models
                .iter()
                .find(|model| model.model_id == "gen4.5")
        })
        .unwrap();
    let gen45_text = runway_gen45
        .operation_capabilities
        .iter()
        .find(|capability| capability.operation == MediaOperation::TextToVideo)
        .unwrap();
    assert_eq!(gen45_text.aspect_ratios, ["16:9", "9:16"]);

    let direct_seedance = providers
        .iter()
        .find(|provider| provider.provider_id == "bytedance")
        .and_then(|provider| provider.models.first())
        .expect("direct Seedance watchlist entry");
    assert_eq!(
        direct_seedance.release_status,
        VideoModelReleaseStatus::Unverified
    );
    assert!(!direct_seedance.selectable);
}

#[test]
fn checked_in_contract_projections_pin_the_live_model_and_version_branches() {
    let minimax: Value = serde_json::from_str(MINIMAX_CONTRACT).unwrap();
    assert_eq!(minimax["v2"]["model"], "MiniMax-H3");
    assert_eq!(minimax["v2"]["duration"]["minimum"], 4);
    assert_eq!(minimax["v2"]["duration"]["maximum"], 15);
    assert_eq!(minimax["legacy"]["cancelScope"], "unsupported");
    assert_eq!(minimax["sourceSha256"].as_str().unwrap().len(), 64);

    let runway: Value = serde_json::from_str(RUNWAY_CONTRACT).unwrap();
    assert_eq!(runway["apiVersionHeader"]["value"], "2024-11-06");
    assert_eq!(
        runway["models"]["seedance2_5"]["operations"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        runway["models"]["seedance2_5"]["ratios"]
            .as_array()
            .unwrap()
            .len(),
        12
    );
    assert_eq!(runway["sourceSha256"].as_str().unwrap().len(), 64);
}

#[test]
fn trusted_manifest_lookup_requires_the_exact_official_origin() {
    assert!(
        find_video_provider_preset("runway", "runway_tasks", "https://api.dev.runwayml.com/")
            .is_some()
    );
    assert!(find_video_provider_preset(
        "runway",
        "runway_tasks",
        "https://api.dev.runwayml.com.evil.example"
    )
    .is_none());
    assert!(find_video_provider_preset(
        "runway",
        "runway_tasks",
        "https://api.dev.runwayml.com?key=secret"
    )
    .is_none());
}

#[test]
fn minimax_validation_enforces_the_h3_multimodal_matrix() {
    let adapter = MiniMaxVideoAdapter::new("secret", "credential-1").unwrap();
    let mut text = request("MiniMax-H3", MediaOperation::TextToVideo);
    text.resolution = "2K".to_string();
    assert!(adapter.validate(&text).valid);
    text.duration_seconds = 4;
    assert!(adapter.validate(&text).valid);
    text.duration_seconds = 15;
    assert!(adapter.validate(&text).valid);
    text.duration_seconds = 3;
    assert!(!adapter.validate(&text).valid);
    text.duration_seconds = 16;
    assert!(!adapter.validate(&text).valid);
    text.duration_seconds = 5;
    text.callback_url = Some("https://relay.example.com/minimax".to_string());
    assert!(adapter
        .validate(&text)
        .issues
        .iter()
        .any(|issue| issue.code == "unsupported_webhook"));
    text.callback_url = None;

    let mut keyframes = text;
    keyframes.operation = MediaOperation::FirstLastFrame;
    keyframes.aspect_ratio = "adaptive".to_string();
    keyframes.input_assets = vec![
        VideoInputAsset {
            role: VideoInputRole::FirstFrame,
            uri: "https://cdn.example.com/first.png".to_string(),
            media_type: "image/png".to_string(),
            metadata_verified: true,
            byte_length: Some(1024),
            width: Some(1024),
            height: Some(768),
            duration_ms: None,
            frame_rate: None,
            video_codec: None,
        },
        VideoInputAsset {
            role: VideoInputRole::LastFrame,
            uri: "https://cdn.example.com/last.png".to_string(),
            media_type: "image/png".to_string(),
            metadata_verified: true,
            byte_length: Some(1024),
            width: Some(1024),
            height: Some(768),
            duration_ms: None,
            frame_rate: None,
            video_codec: None,
        },
    ];
    assert!(adapter.validate(&keyframes).valid);

    keyframes.input_assets.push(VideoInputAsset {
        role: VideoInputRole::ReferenceAudio,
        uri: "https://cdn.example.com/guide.mp3".to_string(),
        media_type: "audio/mpeg".to_string(),
        metadata_verified: true,
        byte_length: Some(1024),
        width: None,
        height: None,
        duration_ms: Some(2_000),
        frame_rate: None,
        video_codec: None,
    });
    assert!(adapter
        .validate(&keyframes)
        .issues
        .iter()
        .any(|issue| issue.code == "mixed_input_modes"));

    keyframes.input_assets.clear();
    keyframes.operation = MediaOperation::ImageToVideo;
    keyframes.input_assets.push(VideoInputAsset {
        role: VideoInputRole::FirstFrame,
        uri: "https://cdn.example.com/first.png?signature=temporary".to_string(),
        media_type: "image/png".to_string(),
        metadata_verified: true,
        byte_length: Some(1024),
        width: Some(1024),
        height: Some(768),
        duration_ms: None,
        frame_rate: None,
        video_codec: None,
    });
    assert!(adapter.validate(&keyframes).valid);
    keyframes.input_assets[0].metadata_verified = false;
    assert!(adapter
        .validate(&keyframes)
        .issues
        .iter()
        .any(|issue| issue.code == "unverified_input_metadata"));
    keyframes.input_assets[0].metadata_verified = true;
    keyframes.input_assets[0].uri = "runway://cross-provider".to_string();
    assert!(adapter
        .validate(&keyframes)
        .issues
        .iter()
        .any(|issue| issue.code == "unsupported_locator"));
}

#[test]
fn runway_validation_is_model_operation_and_capability_specific() {
    let adapter = RunwayVideoAdapter::new("secret", "credential-1").unwrap();
    assert!(
        adapter
            .validate(&request("gen4.5", MediaOperation::TextToVideo))
            .valid
    );

    assert!(adapter
        .validate(&request("gen4_turbo", MediaOperation::TextToVideo))
        .issues
        .iter()
        .any(|issue| issue.code == "unsupported_operation"));

    let mut seedance = request("seedance2_5", MediaOperation::TextToVideo);
    seedance.duration_seconds = 30;
    seedance.generate_audio = Some(true);
    assert!(adapter.validate(&seedance).valid);

    seedance.seed = Some(42);
    assert!(adapter
        .validate(&seedance)
        .issues
        .iter()
        .any(|issue| issue.code == "unsupported_seed"));

    seedance.seed = None;
    seedance.operation = MediaOperation::ImageToVideo;
    seedance.input_assets.push(VideoInputAsset {
        role: VideoInputRole::FirstFrame,
        uri: "mm_file://cross-provider".to_string(),
        media_type: "image/png".to_string(),
        metadata_verified: true,
        byte_length: Some(1024),
        width: Some(1024),
        height: Some(768),
        duration_ms: None,
        frame_rate: None,
        video_codec: None,
    });
    assert!(adapter
        .validate(&seedance)
        .issues
        .iter()
        .any(|issue| issue.code == "unsupported_locator"));
}

#[test]
fn minimax_hailuo_validation_preserves_legacy_model_matrices() {
    let adapter = MiniMaxHailuoVideoAdapter::new("secret", "credential-1").unwrap();
    let mut hailuo = request("MiniMax-Hailuo-2.3", MediaOperation::TextToVideo);
    hailuo.duration_seconds = 6;
    hailuo.resolution = "768P".to_string();
    hailuo.aspect_ratio = "adaptive".to_string();
    assert!(adapter.validate(&hailuo).valid);
    hailuo.callback_url = Some("https://relay.example.com/minimax".to_string());
    assert!(adapter
        .validate(&hailuo)
        .issues
        .iter()
        .any(|issue| issue.code == "unsupported_webhook"));
    hailuo.callback_url = None;

    hailuo.model_id = "MiniMax-Hailuo-2.3-Fast".to_string();
    assert!(adapter
        .validate(&hailuo)
        .issues
        .iter()
        .any(|issue| issue.code == "unsupported_operation"));

    hailuo.model_id = "MiniMax-Hailuo-02".to_string();
    hailuo.operation = MediaOperation::ImageToVideo;
    hailuo.duration_seconds = 10;
    hailuo.resolution = "512P".to_string();
    hailuo.input_assets.push(VideoInputAsset {
        role: VideoInputRole::FirstFrame,
        uri: "https://cdn.example.com/first.png".to_string(),
        media_type: "image/png".to_string(),
        metadata_verified: true,
        byte_length: Some(1024),
        width: Some(1024),
        height: Some(768),
        duration_ms: None,
        frame_rate: None,
        video_codec: None,
    });
    assert!(adapter.validate(&hailuo).valid);
}

#[test]
fn provider_sources_are_account_scoped_without_exposing_secrets() {
    let first = MiniMaxVideoAdapter::new("secret-a", "credential-1").unwrap();
    let second = MiniMaxVideoAdapter::new("secret-b", "credential-2").unwrap();
    assert_ne!(first.provider_source(), second.provider_source());
    assert!(!first.provider_source().contains("secret-a"));
    assert!(!second.provider_source().contains("secret-b"));
    assert!(first
        .provider_source()
        .starts_with("urn:nexa:video:minimax:"));

    let legacy = MiniMaxHailuoVideoAdapter::new("secret-a", "credential-1").unwrap();
    assert_ne!(first.provider_source(), legacy.provider_source());
}

#[tokio::test]
async fn cost_estimates_follow_verified_provider_formulas() {
    let h3 = MiniMaxVideoAdapter::new("secret", "credential-1").unwrap();
    let mut h3_request = request("MiniMax-H3", MediaOperation::VideoToVideo);
    h3_request.resolution = "2K".to_string();
    h3_request.duration_seconds = 5;
    h3_request.input_assets.push(VideoInputAsset {
        role: VideoInputRole::InputVideo,
        uri: "https://cdn.example.com/reference.mp4".to_string(),
        media_type: "video/mp4".to_string(),
        metadata_verified: true,
        byte_length: Some(1024),
        width: Some(1920),
        height: Some(1080),
        duration_ms: Some(2_500),
        frame_rate: Some(30.0),
        video_codec: Some("h264".to_string()),
    });
    let estimate = h3.estimate_cost(&h3_request).await.unwrap();
    assert_eq!(estimate.kind, CostEstimateKind::Exact);
    assert_eq!(estimate.amount_micros, Some(1_040_000));

    let legacy = MiniMaxHailuoVideoAdapter::new("secret", "credential-1").unwrap();
    let mut legacy_request = request("MiniMax-Hailuo-02", MediaOperation::ImageToVideo);
    legacy_request.resolution = "512P".to_string();
    legacy_request.aspect_ratio = "adaptive".to_string();
    legacy_request.duration_seconds = 10;
    legacy_request.input_assets.push(VideoInputAsset {
        role: VideoInputRole::FirstFrame,
        uri: "https://cdn.example.com/first.png".to_string(),
        media_type: "image/png".to_string(),
        metadata_verified: true,
        byte_length: Some(1024),
        width: Some(1024),
        height: Some(768),
        duration_ms: None,
        frame_rate: None,
        video_codec: None,
    });
    assert_eq!(
        legacy
            .estimate_cost(&legacy_request)
            .await
            .unwrap()
            .amount_micros,
        Some(150_000)
    );

    let runway = RunwayVideoAdapter::new("secret", "credential-1").unwrap();
    let mut seedance = request("seedance2_5", MediaOperation::VideoToVideo);
    seedance.resolution = "480P".to_string();
    seedance.duration_seconds = 4;
    seedance.generate_audio = Some(true);
    seedance.input_assets.push(VideoInputAsset {
        role: VideoInputRole::InputVideo,
        uri: "https://cdn.example.com/input.mp4".to_string(),
        media_type: "video/mp4".to_string(),
        metadata_verified: true,
        byte_length: Some(1024),
        width: Some(1280),
        height: Some(720),
        duration_ms: Some(2_000),
        frame_rate: Some(30.0),
        video_codec: Some("h264".to_string()),
    });
    assert_eq!(
        runway.estimate_cost(&seedance).await.unwrap().amount_micros,
        Some(1_000_000)
    );
}
