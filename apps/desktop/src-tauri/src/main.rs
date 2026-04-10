#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod subagent_tool;

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use ask_core::db::Database;
use commands::{AgentState, AppState, DownloadCancelFlag, McpManagerState};
use tauri::Manager;
use tokio::sync::Mutex as TokioMutex;

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("failed to resolve app data directory");
            std::fs::create_dir_all(&data_dir).expect("failed to create app data directory");

            let db_path = data_dir.join("ask-myself.db");
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
                manager: TokioMutex::new(ask_core::mcp::McpManager::new()),
            });
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
            // Conversations
            commands::create_conversation_cmd,
            commands::list_conversations_cmd,
            commands::get_conversation_cmd,
            commands::get_conversation_turns_cmd,
            commands::update_conversation_collection_context_cmd,
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
            // Conversation sources
            commands::set_conversation_sources_cmd,
            commands::get_conversation_sources_cmd,
            // User memories
            commands::list_user_memories_cmd,
            commands::create_user_memory_cmd,
            commands::update_user_memory_cmd,
            commands::delete_user_memory_cmd,
            // Agent configs
            commands::list_agent_configs_cmd,
            commands::save_agent_config_cmd,
            commands::delete_agent_config_cmd,
            commands::set_default_agent_config_cmd,
            commands::test_agent_connection_cmd,
            // Agent chat
            commands::agent_chat_cmd,
            commands::agent_stop_cmd,
            // Model info
            commands::get_model_context_window,
            // Image attachment
            commands::prepare_image_attachment,
            // App Config
            commands::get_app_config_cmd,
            commands::save_app_config_cmd,
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
