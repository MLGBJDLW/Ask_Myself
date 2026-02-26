#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use std::collections::HashMap;
use std::sync::Arc;

use ask_core::db::Database;
use commands::{AgentState, AppState};
use tauri::Manager;
use tokio::sync::Mutex as TokioMutex;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("failed to resolve app data directory");
            std::fs::create_dir_all(&data_dir)
                .expect("failed to create app data directory");

            let db_path = data_dir.join("ask-myself.db");
            let db =
                Database::new(&db_path).expect("failed to initialize database");
            let db = Arc::new(db);

            app.manage(AppState { db: db.clone() });
            app.manage(AgentState {
                running: TokioMutex::new(HashMap::new()),
            });

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
            commands::delete_conversation_cmd,
            commands::rename_conversation_cmd,
            commands::update_conversation_system_prompt_cmd,
            // Conversation maintenance
            commands::get_conversation_stats_cmd,
            commands::cleanup_empty_conversations_cmd,
            // Conversation sources
            commands::set_conversation_sources_cmd,
            commands::get_conversation_sources_cmd,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
