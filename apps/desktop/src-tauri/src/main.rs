#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent_stream;
mod agent_stream_bridge;
mod agent_task_events;
mod app_events;
mod browser;
mod commands;
mod companion_window;
mod delegation_scheduler;
mod desktop_agent_session;
mod subagent_tool;
mod terminal_agent_tool;

use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use commands::{
    AgentState, AppState, ApprovalState, DownloadCancelFlag, DreamingSchedulerState,
    McpManagerState, RealtimeTranscriptionState, TaskOrchestratorSchedulerState,
};
use nexa_core::app_settings::WindowCloseBehavior;
use nexa_core::db::Database;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};
use tauri_plugin_window_state::StateFlags;
use tokio::sync::Mutex as TokioMutex;

/// One-shot migration of user data from the pre-rebrand "ask-myself" layout
/// to the new "nexa" layout. Runs on every startup but is a no-op once the
/// new paths exist, so it is safe to call repeatedly.
///
/// Migrates, in this order:
///   1. SQLite DB:      `ask-myself.db` -> `nexa.db` (same data_dir)
///   2. Models cache:   `<data_dir>/ask-myself/` -> `<data_dir>/nexa/`
///   3. Legacy-identifier fallback: on OSes where `app_data_dir()` is keyed
///      by the bundle identifier (Tauri v2 behaviour on Windows & macOS),
///      the old `com.askmyself.desktop` directory contains the user's data,
///      while the current `data_dir` is a freshly-created empty dir under
///      `com.nexa.desktop`. Detect this case by looking at a sibling of
///      `data_dir` named `com.askmyself.desktop` and migrate the DB + models
///      from there.
///
/// Failure policy: log + continue. We do NOT fail startup if a rename fails
/// (users can still use the app with fresh state; data is not destroyed).
fn migrate_legacy_data_dir(data_dir: &Path) {
    // Helper: rename if src exists and dst does not. Logs outcome.
    let try_rename = |src: &Path, dst: &Path, label: &str| {
        if !src.exists() {
            return;
        }
        if dst.exists() {
            log::info!(
                "[migrate] {label}: destination {} already exists; skipping",
                dst.display()
            );
            return;
        }
        match std::fs::rename(src, dst) {
            Ok(()) => log::info!("[migrate] {label}: {} -> {}", src.display(), dst.display()),
            Err(e) => log::warn!(
                "[migrate] {label}: failed to rename {} -> {}: {e}",
                src.display(),
                dst.display()
            ),
        }
    };

    // 1 & 2: same-directory migration (works when data_dir is identifier-agnostic,
    //        e.g. Linux XDG path, or when user manually copied old data).
    try_rename(
        &data_dir.join("ask-myself.db"),
        &data_dir.join("nexa.db"),
        "db (same dir)",
    );
    try_rename(
        &data_dir.join("ask-myself"),
        &data_dir.join("nexa"),
        "models dir (same dir)",
    );

    // 3: cross-identifier migration. On Windows & macOS, Tauri v2's
    //    app_data_dir() is `<appdata-root>/<bundle-identifier>`, so the
    //    legacy data lives in a sibling directory.
    if let Some(parent) = data_dir.parent() {
        let legacy_root = parent.join("com.askmyself.desktop");
        if legacy_root.exists() && legacy_root != data_dir {
            try_rename(
                &legacy_root.join("ask-myself.db"),
                &data_dir.join("nexa.db"),
                "db (legacy identifier)",
            );
            try_rename(
                &legacy_root.join("nexa.db"),
                &data_dir.join("nexa.db"),
                "db (legacy identifier, already renamed)",
            );
            try_rename(
                &legacy_root.join("ask-myself"),
                &data_dir.join("nexa"),
                "models dir (legacy identifier)",
            );
            try_rename(
                &legacy_root.join("nexa"),
                &data_dir.join("nexa"),
                "models dir (legacy identifier, already renamed)",
            );
        }
    }
}

fn load_or_create_routing_session_secret(data_dir: &Path) -> io::Result<String> {
    let secret_path = data_dir.join("routing-session-secret");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&secret_path) {
        Ok(mut file) => {
            let secret = uuid::Uuid::new_v4().to_string();
            file.write_all(secret.as_bytes())?;
            file.sync_all()?;
            Ok(secret)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let mut secret = String::new();
            std::fs::File::open(secret_path)?.read_to_string(&mut secret)?;
            let secret = secret.trim().to_string();
            if secret.is_empty() {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "routing session secret is empty",
                ))
            } else {
                Ok(secret)
            }
        }
        Err(error) => Err(error),
    }
}

fn persisted_window_state_flags() -> StateFlags {
    // Native decorations are controlled by tauri.conf.json. Restoring this flag
    // would let a pre-custom-frame state re-enable the system title bar.
    StateFlags::all() & !StateFlags::DECORATIONS
}

const TRAY_SHOW_ID: &str = "tray_show";
const TRAY_SHOW_COMPANION_ID: &str = "tray_show_companion";
const TRAY_HIDE_COMPANION_ID: &str = "tray_hide_companion";
const TRAY_LOCK_COMPANION_ID: &str = "tray_lock_companion";
const TRAY_UNLOCK_COMPANION_ID: &str = "tray_unlock_companion";
const TRAY_RESET_COMPANION_ID: &str = "tray_reset_companion";
const TRAY_COMPANION_SETTINGS_ID: &str = "tray_companion_settings";
const TRAY_QUIT_ID: &str = "tray_quit";

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn install_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, TRAY_SHOW_ID, "Show Nexa", true, None::<&str>)?;
    let show_companion_item = MenuItem::with_id(
        app,
        TRAY_SHOW_COMPANION_ID,
        "Show Desktop Pet",
        true,
        None::<&str>,
    )?;
    let hide_companion_item = MenuItem::with_id(
        app,
        TRAY_HIDE_COMPANION_ID,
        "Hide Desktop Pet",
        true,
        None::<&str>,
    )?;
    let unlock_companion_item = MenuItem::with_id(
        app,
        TRAY_UNLOCK_COMPANION_ID,
        "Unlock Desktop Pet",
        true,
        None::<&str>,
    )?;
    let lock_companion_item = MenuItem::with_id(
        app,
        TRAY_LOCK_COMPANION_ID,
        "Lock Desktop Pet",
        true,
        None::<&str>,
    )?;
    let reset_companion_item = MenuItem::with_id(
        app,
        TRAY_RESET_COMPANION_ID,
        "Reset Pet Position",
        true,
        None::<&str>,
    )?;
    let companion_settings_item = MenuItem::with_id(
        app,
        TRAY_COMPANION_SETTINGS_ID,
        "Open Pet Settings",
        true,
        None::<&str>,
    )?;
    let quit_item = MenuItem::with_id(app, TRAY_QUIT_ID, "Quit Nexa", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &show_item,
            &show_companion_item,
            &hide_companion_item,
            &lock_companion_item,
            &unlock_companion_item,
            &reset_companion_item,
            &companion_settings_item,
            &quit_item,
        ],
    )?;
    let icon = app.default_window_icon().cloned();

    let mut tray = TrayIconBuilder::with_id("nexa-main")
        .tooltip("Nexa")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            TRAY_SHOW_ID => show_main_window(app),
            TRAY_SHOW_COMPANION_ID => {
                if let Err(error) = companion_window::show_companion(app) {
                    log::warn!("Failed to show Desktop Pet from tray: {error}");
                }
            }
            TRAY_HIDE_COMPANION_ID => {
                if let Err(error) = companion_window::hide_companion(app) {
                    log::warn!("Failed to hide Desktop Pet from tray: {error}");
                }
            }
            TRAY_LOCK_COMPANION_ID => {
                if let Err(error) = companion_window::lock_companion(app) {
                    log::warn!("Failed to lock Desktop Pet from tray: {error}");
                }
            }
            TRAY_UNLOCK_COMPANION_ID => {
                if let Err(error) = companion_window::unlock_companion(app) {
                    log::warn!("Failed to unlock Desktop Pet from tray: {error}");
                }
            }
            TRAY_RESET_COMPANION_ID => {
                if let Err(error) = companion_window::reset_companion_position(app) {
                    log::warn!("Failed to reset Desktop Pet position from tray: {error}");
                }
            }
            TRAY_COMPANION_SETTINGS_ID => {
                show_main_window(app);
                let _ = app.emit("companion://open-settings", ());
            }
            TRAY_QUIT_ID => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = icon {
        tray = tray.icon(icon);
    }
    tray.build(app)?;
    Ok(())
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(persisted_window_state_flags())
                .build(),
        )
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("failed to resolve app data directory");
            std::fs::create_dir_all(&data_dir).expect("failed to create app data directory");

            match load_or_create_routing_session_secret(&data_dir) {
                Ok(secret) => {
                    if !nexa_core::llm::prompt_cache::configure_routing_session_secret(
                        secret.as_bytes(),
                    ) {
                        log::warn!("Routing session secret was already configured");
                    }
                }
                Err(error) => log::warn!(
                    "Failed to load routing session secret; using a process-local fallback: {error}"
                ),
            }

            // Migrate legacy user data (ask-myself -> nexa). Safe to call every start.
            migrate_legacy_data_dir(&data_dir);

            // Materialize bundled skill assets to disk so run_shell can exec them
            // and <SKILL_DIR> placeholders in prompts resolve correctly.
            match nexa_core::skills::materialize_skills_to_disk(&data_dir) {
                Ok(path) => log::info!("Materialized skills to {}", path.display()),
                Err(e) => log::warn!(
                    "Failed to materialize skills: {e}. Python-backed document editing via shell scripts will be unavailable."
                ),
            }
            if let Some(bin_dir) = nexa_core::office_runtime::configure_app_managed_python_env(&data_dir) {
                log::info!(
                    "Configured app-managed Office Python environment at {}",
                    bin_dir.display()
                );
            }

            let db_path = data_dir.join("nexa.db");
            let db = Database::new(&db_path).expect("failed to initialize database");
            match db.mark_interrupted_agent_task_runs() {
                Ok(count) if count > 0 => {
                    log::info!("Marked {count} interrupted agent task run(s) from a previous app process")
                }
                Ok(_) => {}
                Err(e) => log::warn!("Failed to mark interrupted agent task runs: {e}"),
            }
            match db
                .list_skills()
                .and_then(|skills| nexa_core::skills::materialize_user_skills_to_disk(&data_dir, &skills))
            {
                Ok(()) => log::info!("Materialized user skills to {}", data_dir.join("skills/user").display()),
                Err(e) => log::warn!("Failed to materialize user skills: {e}"),
            }
            let db = Arc::new(db);
            let db_executor = nexa_core::db_executor::DatabaseExecutor::new((*db).clone(), 64)
                .expect("failed to initialize bounded database executor");
            let activity_runtime = nexa_core::activity::ActivityRuntime::with_database((*db).clone())
                .expect("failed to initialize durable activity runtime");
            let context_compaction = nexa_core::context_maintenance::ContextCompactionService::new(
                db_executor.clone(),
                activity_runtime,
            );
            let media_generation = nexa_core::media_generation::MediaGenerationRuntime::with_asset_store(
                db_executor.clone(),
                nexa_core::media_generation::MediaGenerationAssetStore::new(
                    data_dir.join("generation-assets"),
                ),
            );
            let recovered_media_jobs =
                tauri::async_runtime::block_on(media_generation.recover_after_restart())?;
            if recovered_media_jobs > 0 {
                log::info!(
                    "Marked {recovered_media_jobs} ambiguous media submission(s) provider_unknown after restart"
                );
            }
            let media_recovery_plan =
                tauri::async_runtime::block_on(media_generation.build_recovery_plan())?;
            if !media_recovery_plan.is_empty() {
                log::info!(
                    "Prepared {} durable media recovery action(s) before exposing renderer state",
                    media_recovery_plan.len()
                );
            }
            #[cfg(feature = "video")]
            let voice_audio_spool = Arc::new(
                nexa_core::voice_audio_spool::VoiceAudioSpool::new(
                    data_dir.join("voice-spool"),
                )
                .expect("failed to initialize managed voice audio spool"),
            );

            app.manage(AppState {
                db: db.clone(),
                db_executor,
                context_compaction,
                media_generation,
                #[cfg(feature = "video")]
                whisper_busy: Arc::new(AtomicBool::new(false)),
                #[cfg(feature = "video")]
                voice_audio_spool,
                #[cfg(feature = "video")]
                voice_spool_append_permits: Arc::new(tokio::sync::Semaphore::new(4)),
                scan_lock: Arc::new(std::sync::Mutex::new(())),
            });
            app.manage(AgentState {
                sessions: nexa_core::runtime::AgentSessionManager::new(),
            });
            app.manage(McpManagerState {
                manager: Arc::new(TokioMutex::new(nexa_core::mcp::McpManager::new())),
            });
            app.manage(ApprovalState::default());
            app.manage(RealtimeTranscriptionState::default());
            app.manage(commands::TerminalState::default());
            app.manage(browser::BrowserState::new(
                app.handle().clone(),
                data_dir.join("browser-profiles"),
            ));
            app.manage(DownloadCancelFlag(Arc::new(AtomicBool::new(false))));
            let companion_settings = db
                .load_app_config()
                .map(|config| config.companion)
                .unwrap_or_default();
            companion_window::create_companion_window(app, &companion_settings);
            install_tray(app)?;

            // Initialise the file watcher for auto-indexing.
            let handle = app.handle().clone();
            commands::init_watcher(handle);
            commands::init_task_orchestrator_scheduler(app.handle().clone());
            commands::init_dreaming_scheduler(app.handle().clone());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Sources
            commands::add_source,
            commands::list_sources,
            commands::get_source,
            commands::list_source_tree_cmd,
            commands::update_source,
            commands::delete_source,
            // Ingest
            commands::scan_source,
            commands::scan_all_sources,
            // Search
            commands::search,
            commands::get_evidence_card,
            commands::get_evidence_cards,
            // Index
            commands::get_index_stats,
            commands::rebuild_index,
            // Playbooks
            commands::create_playbook,
            commands::list_playbooks,
            commands::get_playbook,
            commands::update_playbook,
            commands::delete_playbook,
            // Citations
            commands::add_citation,
            commands::list_citations,
            commands::remove_citation,
            // Query log
            commands::get_recent_queries,
            commands::clear_recent_queries,
            // Hybrid search
            commands::hybrid_search,
            // Answer cache
            commands::clear_answer_cache,
            // Embeddings
            commands::embed_source,
            commands::rebuild_embeddings,
            // Feedback
            commands::add_feedback,
            commands::get_feedback_for_query,
            commands::delete_feedback,
            commands::set_message_feedback_cmd,
            commands::get_message_feedback_cmd,
            // Privacy
            commands::get_privacy_config,
            commands::save_privacy_config,
            // Index (extra)
            commands::optimize_fts_index,
            // Citations (extra)
            commands::update_citation_note,
            commands::reorder_citations,
            // Embedder config
            commands::get_embedder_config_cmd,
            commands::save_embedder_config_cmd,
            commands::test_api_connection_cmd,
            commands::check_local_model_cmd,
            commands::download_local_model_cmd,
            commands::cancel_model_download_cmd,
            commands::delete_local_model_cmd,
            // File
            commands::open_file_in_default_app,
            commands::show_in_file_explorer,
            commands::preview_file_cmd,
            commands::probe_web_preview_cmd,
            commands::save_text_file_cmd,
            commands::read_generated_image_data_url_cmd,
            commands::save_generated_image_cmd,
            // Watcher
            commands::start_watching,
            commands::stop_watching,
            commands::get_watcher_status,
            // Personas
            commands::list_personas_cmd,
            commands::save_persona_cmd,
            commands::delete_persona_cmd,
            commands::toggle_persona_cmd,
            // Projects
            commands::create_project_cmd,
            commands::list_projects_cmd,
            commands::get_project_cmd,
            commands::update_project_cmd,
            commands::delete_project_cmd,
            commands::list_project_memories_cmd,
            commands::get_project_workspace_cmd,
            commands::get_project_narrative_cmd,
            commands::get_companion_projection_cmd,
            commands::scan_companion_packs_cmd,
            commands::import_companion_pack_cmd,
            commands::read_companion_asset_cmd,
            commands::delete_managed_companion_pack_cmd,
            commands::get_global_companion_projection_cmd,
            commands::companion_command_cmd,
            companion_window::companion_renderer_ready_cmd,
            companion_window::show_companion_cmd,
            companion_window::hide_companion_cmd,
            companion_window::toggle_companion_cmd,
            companion_window::set_companion_interaction_cmd,
            companion_window::persist_companion_position_cmd,
            companion_window::reset_companion_position_cmd,
            companion_window::get_companion_window_diagnostics_cmd,
            commands::create_project_knowledge_claim_cmd,
            commands::review_project_knowledge_claim_cmd,
            commands::create_project_memory_cmd,
            commands::update_project_memory_cmd,
            commands::delete_project_memory_cmd,
            commands::move_conversation_to_project_cmd,
            commands::remove_conversation_from_project_cmd,
            // Conversations
            commands::create_conversation_cmd,
            commands::list_conversations_cmd,
            commands::list_archived_conversations_cmd,
            commands::get_conversation_cmd,
            commands::get_conversation_turns_cmd,
            commands::list_interaction_requests_cmd,
            commands::get_interaction_request_cmd,
            commands::mark_interaction_presented_cmd,
            commands::mark_interaction_partially_answered_cmd,
            commands::append_interaction_supplement_cmd,
            commands::submit_interaction_response_cmd,
            commands::get_interaction_response_cmd,
            commands::acknowledge_interaction_cmd,
            commands::cancel_interaction_cmd,
            commands::supersede_interaction_cmd,
            commands::fail_interaction_cmd,
            commands::get_agent_task_runs_cmd,
            commands::list_recent_agent_task_runs_cmd,
            commands::list_agent_task_run_summaries_cmd,
            commands::get_agent_task_run_events_cmd,
            commands::get_agent_run_events_cmd,
            commands::get_run_usage_snapshot_cmd,
            commands::get_conversation_usage_snapshot_cmd,
            commands::get_ai_usage_analytics_cmd,
            commands::delete_ai_usage_records_cmd,
            commands::export_ai_usage_cmd,
            commands::get_agent_subtask_runs_cmd,
            commands::get_agent_execution_graph_cmd,
            commands::get_agent_task_artifacts_cmd,
            commands::list_persisted_agent_task_artifacts_cmd,
            commands::create_agent_task_artifact_cmd,
            commands::update_agent_task_artifact_cmd,
            commands::list_agent_task_artifact_versions_cmd,
            commands::pause_agent_task_run_cmd,
            commands::list_task_resume_checkpoints_cmd,
            commands::get_task_resume_prompt_cmd,
            commands::get_investigation_graph_cmd,
            commands::list_tool_access_map_cmd,
            commands::list_capability_packages_cmd,
            commands::get_package_host_snapshot_cmd,
            commands::set_package_host_package_enabled_cmd,
            commands::set_package_host_package_health_cmd,
            commands::list_project_tools_cmd,
            commands::update_conversation_collection_context_cmd,
            commands::update_conversation_persona_cmd,
            commands::update_conversation_model_cmd,
            commands::delete_conversation_cmd,
            commands::archive_conversation_cmd,
            commands::unarchive_conversation_cmd,
            commands::delete_conversations_batch_cmd,
            commands::delete_all_conversations_cmd,
            commands::rename_conversation_cmd,
            commands::generate_title_cmd,
            commands::update_conversation_system_prompt_cmd,
            // Conversation maintenance
            commands::get_conversation_stats_cmd,
            commands::cleanup_empty_conversations_cmd,
            commands::compact_conversation_cmd,
            commands::start_context_compaction_cmd,
            commands::observe_context_compaction_cmd,
            commands::cancel_context_compaction_cmd,
            // Durable media generation jobs
            commands::create_media_generation_job_cmd,
            commands::list_video_generation_capabilities_cmd,
            commands::get_media_generation_job_cmd,
            commands::list_recoverable_media_generation_jobs_cmd,
            commands::list_media_generation_provider_events_cmd,
            commands::request_media_generation_cancellation_cmd,
            commands::request_media_generation_remote_deletion_cmd,
            commands::delete_media_generation_asset_occurrence_cmd,
            commands::delete_media_generation_asset_cmd,
            commands::search_conversations_cmd,
            // Conversation checkpoints
            commands::list_checkpoints_cmd,
            commands::restore_checkpoint_cmd,
            commands::branch_checkpoint_cmd,
            commands::delete_checkpoint_cmd,
            commands::list_file_checkpoints_cmd,
            commands::restore_file_checkpoint_cmd,
            commands::delete_file_checkpoint_cmd,
            // Conversation sources
            commands::set_conversation_sources_cmd,
            commands::get_conversation_sources_cmd,
            // User memories
            commands::list_user_memories_cmd,
            commands::create_user_memory_cmd,
            commands::update_user_memory_cmd,
            commands::delete_user_memory_cmd,
            commands::list_agent_procedural_memories_cmd,
            commands::delete_agent_procedural_memory_cmd,
            // Agent scratchpad
            commands::get_agent_scratchpad_cmd,
            // Agent configs
            commands::list_agent_configs_cmd,
            commands::save_agent_config_cmd,
            commands::delete_agent_config_cmd,
            commands::set_default_agent_config_cmd,
            commands::get_settings_schema_state_v2_cmd,
            commands::list_settings_profiles_v2_cmd,
            commands::save_settings_profile_v2_cmd,
            commands::save_capability_binding_v2_cmd,
            commands::delete_vision_observation_cache_cmd,
            commands::clear_vision_observation_cache_cmd,
            commands::migrate_settings_schema_v2_cmd,
            commands::rollback_settings_schema_v2_cmd,
            commands::get_capability_registry_projection_cmd,
            commands::set_capability_registry_read_mode_cmd,
            commands::test_agent_connection_cmd,
            commands::refresh_provider_model_catalog_cmd,
            commands::list_provider_presets_cmd,
            commands::list_workflow_templates_cmd,
            commands::save_workflow_automation_cmd,
            commands::list_workflow_automations_cmd,
            commands::delete_workflow_automation_cmd,
            commands::set_workflow_automation_enabled_cmd,
            commands::list_due_workflow_automations_cmd,
            commands::list_due_task_orchestrator_queue_cmd,
            commands::preview_workflow_automation_prompt_cmd,
            commands::prepare_workflow_automation_delivery_cmd,
            commands::prepare_due_workflow_automation_delivery_cmd,
            commands::queue_workflow_automation_delivery_cmd,
            commands::queue_due_workflow_automation_delivery_cmd,
            commands::start_due_workflow_automation_run_cmd,
            commands::record_workflow_automation_run_cmd,
            commands::list_workflow_automation_scheduler_events_cmd,
            commands::list_workflow_automation_scheduler_events_for_task_run_cmd,
            commands::export_workflow_automation_trajectory_cmd,
            // Agent chat
            commands::agent_chat_cmd,
            commands::record_agent_frontend_paint_cmd,
            commands::agent_steer_cmd,
            commands::agent_stop_cmd,
            // Terminal
            commands::terminal_start_session_cmd,
            commands::terminal_write_session_cmd,
            commands::terminal_resize_session_cmd,
            commands::terminal_close_session_cmd,
            commands::terminal_bind_session_cmd,
            commands::terminal_snapshot_session_cmd,
            commands::terminal_list_sessions_cmd,
            commands::terminal_active_session_cmd,
            // Browser Workspace
            browser::browser_create_session_cmd,
            browser::browser_list_sessions_cmd,
            browser::browser_active_session_cmd,
            browser::browser_open_tab_cmd,
            browser::browser_open_popup_cmd,
            browser::browser_navigate_cmd,
            browser::browser_activate_tab_cmd,
            browser::browser_set_bounds_cmd,
            browser::browser_go_back_cmd,
            browser::browser_go_forward_cmd,
            browser::browser_reload_cmd,
            browser::browser_stop_cmd,
            browser::browser_begin_element_pick_cmd,
            browser::browser_begin_region_pick_cmd,
            browser::browser_take_pick_cmd,
            browser::browser_selected_text_cmd,
            browser::browser_acquire_control_cmd,
            browser::browser_close_tab_cmd,
            browser::browser_close_session_cmd,
            // Model info
            commands::get_model_context_window,
            // Image attachment
            commands::prepare_image_attachment,
            // App Config
            commands::get_app_config_cmd,
            commands::save_app_config_cmd,
            commands::synthesize_speech_preview_cmd,
            commands::refresh_tts_voice_catalog_cmd,
            commands::clear_speech_cache_cmd,
            commands::import_theme_background_cmd,
            commands::resolve_theme_background_cmd,
            commands::garbage_collect_theme_assets_cmd,
            commands::get_web_search_status_cmd,
            commands::check_office_runtime_cmd,
            commands::prepare_office_runtime_cmd,
            commands::check_update_from_source_cmd,
            // Setup Wizard
            commands::get_wizard_state_cmd,
            commands::set_wizard_completed_cmd,
            commands::reset_wizard_cmd,
            // OCR
            commands::get_ocr_config_cmd,
            commands::save_ocr_config_cmd,
            commands::check_ocr_models_cmd,
            commands::download_ocr_models_cmd,
            commands::delete_ocr_models_cmd,
            commands::get_managed_model_paths_cmd,
            // Video
            #[cfg(feature = "video")]
            commands::get_video_config_cmd,
            #[cfg(feature = "video")]
            commands::save_video_config_cmd,
            #[cfg(feature = "video")]
            commands::get_media_runtime_status_cmd,
            #[cfg(feature = "video")]
            commands::check_whisper_model_cmd,
            #[cfg(feature = "video")]
            commands::download_whisper_model_cmd,
            #[cfg(feature = "video")]
            commands::delete_whisper_model_cmd,
            #[cfg(feature = "video")]
            commands::start_voice_audio_spool_cmd,
            #[cfg(feature = "video")]
            commands::append_voice_audio_spool_cmd,
            #[cfg(feature = "video")]
            commands::finish_voice_audio_spool_cmd,
            #[cfg(feature = "video")]
            commands::list_voice_audio_spools_cmd,
            #[cfg(feature = "video")]
            commands::transcribe_voice_audio_spool_cmd,
            #[cfg(feature = "video")]
            commands::cancel_voice_audio_spool_cmd,
            commands::start_realtime_transcription_cmd,
            commands::append_realtime_transcription_audio_cmd,
            commands::finish_realtime_transcription_cmd,
            commands::cancel_realtime_transcription_cmd,
            #[cfg(feature = "video")]
            commands::check_ffmpeg_cmd,
            #[cfg(feature = "video")]
            commands::download_ffmpeg_cmd,
            #[cfg(feature = "video")]
            commands::analyze_video_cmd,
            #[cfg(feature = "video")]
            commands::get_video_transcript_cmd,
            #[cfg(feature = "video")]
            commands::get_video_metadata_cmd,
            // Skills
            commands::list_skills_cmd,
            commands::save_skill_cmd,
            commands::delete_skill_cmd,
            commands::toggle_skill_cmd,
            commands::list_builtin_skills_cmd,
            commands::import_skill_from_md_cmd,
            commands::parse_skill_markdown_cmd,
            commands::inspect_skill_install_source_cmd,
            commands::install_skills_from_source_cmd,
            commands::export_skill_to_md_cmd,
            commands::scan_skill_content_cmd,
            commands::list_skill_change_proposals_cmd,
            commands::apply_skill_change_proposal_cmd,
            commands::reject_skill_change_proposal_cmd,
            commands::discover_skills_in_directory_cmd,
            commands::import_skills_from_directory_cmd,
            // MCP
            commands::list_mcp_servers_cmd,
            commands::save_mcp_server_cmd,
            commands::delete_mcp_server_cmd,
            commands::toggle_mcp_server_cmd,
            commands::test_mcp_server_cmd,
            commands::test_mcp_server_direct_cmd,
            commands::list_mcp_tools_cmd,
            // Trace analytics
            commands::get_trace_summary,
            commands::get_recent_traces,
            commands::export_agent_task_trajectory_cmd,
            commands::save_agent_trajectory_cmd,
            commands::load_agent_trajectory_cmd,
            commands::list_agent_trajectories_cmd,
            commands::run_trajectory_eval_pack_cmd,
            commands::compare_trajectory_replay_cmd,
            commands::replay_trajectory_session_cmd,
            commands::run_stored_trajectory_smoke_eval_cmd,
            commands::run_developer_eval_smoke_workflow_cmd,
            commands::run_developer_eval_nightly_workflow_cmd,
            commands::run_agent_quality_eval_cmd,
            commands::get_learning_governance_snapshot_cmd,
            commands::capture_browser_evidence_cmd,
            // Knowledge compilation
            commands::compile_document_cmd,
            commands::compile_pending_documents_cmd,
            commands::get_compile_stats_cmd,
            commands::get_knowledge_map_cmd,
            commands::get_knowledge_graph_cmd,
            commands::run_knowledge_health_check_cmd,
            commands::compile_after_scan_cmd,
            commands::start_dream_cmd,
            commands::list_dream_runs_cmd,
            commands::list_dream_run_events_cmd,
            commands::list_dream_artifacts_cmd,
            commands::apply_dream_artifact_cmd,
            commands::update_dream_artifact_cmd,
            commands::reject_dream_artifact_cmd,
            commands::undo_dream_artifact_cmd,
            // Scan errors
            commands::get_scan_errors_cmd,
            commands::clear_scan_errors_cmd,
            commands::clear_scan_error_cmd,
            // Knowledge loop
            commands::get_knowledge_gaps_cmd,
            commands::suggest_explorations_cmd,
            // Tool approval
            commands::approve_tool_call_cmd,
            commands::list_tool_permission_policies_cmd,
            commands::delete_tool_permission_policy_cmd,
            commands::clear_tool_permission_policies_cmd,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| match event {
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::CloseRequested { api, .. },
            ..
        } if label == "main" => {
            let minimize_to_tray = app_handle
                .try_state::<AppState>()
                .and_then(|state| state.db.load_app_config().ok())
                .is_some_and(|config| {
                    config.window_close_behavior == WindowCloseBehavior::MinimizeToTray
                });
            if minimize_to_tray {
                api.prevent_close();
                if let Some(window) = app_handle.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
        }
        tauri::RunEvent::Exit => {
            if let Some(browser_state) = app_handle.try_state::<browser::BrowserState>() {
                browser_state.close_all_sessions();
            }
            if let Some(scheduler_state) = app_handle.try_state::<TaskOrchestratorSchedulerState>()
            {
                commands::shutdown_task_orchestrator_scheduler(&scheduler_state);
            }
            if let Some(scheduler_state) = app_handle.try_state::<DreamingSchedulerState>() {
                commands::shutdown_dreaming_scheduler(&scheduler_state);
            }
            // Shutdown MCP manager: kill all managed processes
            if let Some(mcp_state) = app_handle.try_state::<McpManagerState>() {
                tauri::async_runtime::block_on(async {
                    let mut manager = mcp_state.manager.lock().await;
                    manager.shutdown().await;
                });
            }
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_window_state_never_restores_native_decorations() {
        let flags = persisted_window_state_flags();

        assert!(!flags.contains(StateFlags::DECORATIONS));
        assert!(flags.contains(
            StateFlags::SIZE
                | StateFlags::POSITION
                | StateFlags::MAXIMIZED
                | StateFlags::VISIBLE
                | StateFlags::FULLSCREEN
        ));
    }
}
