#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod subagent_tool;

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use commands::{AgentState, AppState, ApprovalState, DownloadCancelFlag, McpManagerState};
use nexa_core::db::Database;
use tauri::Manager;
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

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("failed to resolve app data directory");
            std::fs::create_dir_all(&data_dir).expect("failed to create app data directory");

            // Migrate legacy user data (ask-myself -> nexa). Safe to call every start.
            migrate_legacy_data_dir(&data_dir);

            // Materialize bundled skill assets to disk so run_shell can exec them
            // and <SKILL_DIR> placeholders in prompts resolve correctly. Degrade
            // gracefully on failure: doc-editing fast-path still works without scripts.
            match nexa_core::skills::materialize_skills_to_disk(&data_dir) {
                Ok(path) => log::info!("Materialized skills to {}", path.display()),
                Err(e) => log::warn!(
                    "Failed to materialize skills: {e}. Doc-editing via shell scripts will be unavailable; edit_document fast-path still works."
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
            let db = Arc::new(db);

            app.manage(AppState {
                db: db.clone(),
                #[cfg(feature = "video")]
                whisper_busy: Arc::new(AtomicBool::new(false)),
                scan_lock: Arc::new(std::sync::Mutex::new(())),
            });
            app.manage(AgentState {
                running: TokioMutex::new(HashMap::new()),
            });
            app.manage(McpManagerState {
                manager: TokioMutex::new(nexa_core::mcp::McpManager::new()),
            });
            app.manage(ApprovalState::default());
            app.manage(DownloadCancelFlag(Arc::new(AtomicBool::new(false))));

            // Initialise the file watcher for auto-indexing.
            let handle = app.handle().clone();
            let app_state: tauri::State<'_, AppState> = app.state();
            commands::init_watcher(handle, &app_state.db);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Sources
            commands::add_source,
            commands::list_sources,
            commands::get_source,
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
            // Watcher
            commands::start_watching,
            commands::stop_watching,
            commands::get_watcher_status,
            // Projects
            commands::create_project_cmd,
            commands::list_projects_cmd,
            commands::get_project_cmd,
            commands::update_project_cmd,
            commands::delete_project_cmd,
            commands::move_conversation_to_project_cmd,
            commands::remove_conversation_from_project_cmd,
            // Conversations
            commands::create_conversation_cmd,
            commands::list_conversations_cmd,
            commands::get_conversation_cmd,
            commands::get_conversation_turns_cmd,
            commands::get_agent_task_runs_cmd,
            commands::get_agent_task_run_events_cmd,
            commands::update_conversation_collection_context_cmd,
            commands::update_conversation_model_cmd,
            commands::delete_conversation_cmd,
            commands::delete_conversations_batch_cmd,
            commands::delete_all_conversations_cmd,
            commands::rename_conversation_cmd,
            commands::generate_title_cmd,
            commands::update_conversation_system_prompt_cmd,
            // Conversation maintenance
            commands::get_conversation_stats_cmd,
            commands::cleanup_empty_conversations_cmd,
            commands::compact_conversation_cmd,
            commands::search_conversations_cmd,
            // Conversation checkpoints
            commands::list_checkpoints_cmd,
            commands::restore_checkpoint_cmd,
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
            // Agent scratchpad
            commands::get_agent_scratchpad_cmd,
            // Agent configs
            commands::list_agent_configs_cmd,
            commands::save_agent_config_cmd,
            commands::delete_agent_config_cmd,
            commands::set_default_agent_config_cmd,
            commands::test_agent_connection_cmd,
            commands::list_provider_presets_cmd,
            // Agent chat
            commands::agent_chat_cmd,
            commands::agent_steer_cmd,
            commands::agent_stop_cmd,
            // Model info
            commands::get_model_context_window,
            // Image attachment
            commands::prepare_image_attachment,
            // App Config
            commands::get_app_config_cmd,
            commands::save_app_config_cmd,
            commands::check_office_runtime_cmd,
            commands::prepare_office_runtime_cmd,
            // Setup Wizard
            commands::get_wizard_state_cmd,
            commands::set_wizard_completed_cmd,
            commands::reset_wizard_cmd,
            // OCR
            commands::get_ocr_config_cmd,
            commands::save_ocr_config_cmd,
            commands::check_ocr_models_cmd,
            commands::download_ocr_models_cmd,
            // Video
            #[cfg(feature = "video")]
            commands::get_video_config_cmd,
            #[cfg(feature = "video")]
            commands::save_video_config_cmd,
            #[cfg(feature = "video")]
            commands::check_whisper_model_cmd,
            #[cfg(feature = "video")]
            commands::download_whisper_model_cmd,
            #[cfg(feature = "video")]
            commands::delete_whisper_model_cmd,
            #[cfg(feature = "video")]
            commands::transcribe_audio_buffer_cmd,
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
            commands::export_skill_to_md_cmd,
            commands::scan_skill_content_cmd,
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
            // Knowledge compilation
            commands::compile_document_cmd,
            commands::compile_pending_documents_cmd,
            commands::get_compile_stats_cmd,
            commands::get_knowledge_map_cmd,
            commands::run_knowledge_health_check_cmd,
            commands::compile_after_scan_cmd,
            // Scan errors
            commands::get_scan_errors_cmd,
            commands::clear_scan_errors_cmd,
            commands::clear_scan_error_cmd,
            // Knowledge loop
            commands::get_knowledge_gaps_cmd,
            commands::suggest_explorations_cmd,
            // Tool approval
            commands::approve_tool_call_cmd,
            commands::list_tool_approval_policies_cmd,
            commands::delete_tool_approval_policy_cmd,
            commands::clear_tool_approval_policies_cmd,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            // Shutdown MCP manager: kill all managed processes
            if let Some(mcp_state) = app_handle.try_state::<McpManagerState>() {
                tauri::async_runtime::block_on(async {
                    let mut manager = mcp_state.manager.lock().await;
                    manager.shutdown().await;
                });
            }
        }
    });
}
