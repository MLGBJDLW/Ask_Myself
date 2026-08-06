//! Nexa-core — knowledge-base engine with embedding, search, and OCR.
//!
//! This crate provides the core functionality for ingesting, parsing,
//! embedding, and searching personal knowledge documents.  An optional
//! OCR module (feature-gated) adds ONNX-based PaddleOCR for extracting
//! text from images and scanned PDFs.

/// Application directory name used under the OS data dir.
/// Changed from "ask-myself" to "nexa" in the v0.x.x rebrand.
pub const APP_DIR: &str = "nexa";

/// User-agent string for outbound HTTP requests.
pub const USER_AGENT: &str = "nexa/1.0";

pub mod activity;
pub mod agent;
pub mod agent_run;
pub mod agent_session;
pub mod app_settings;
pub mod approval;
mod background_process;
pub mod behavioral_eval;
pub mod browser_runtime;
pub mod cache;
pub mod capability_package;
pub mod capability_registry;
pub mod compile;
pub mod context_maintenance;
pub mod context_pack;
pub mod conversation;
pub mod crypto;
pub mod db;
pub mod db_executor;
pub mod dreaming;
pub mod dreaming_scope;
pub mod ecosystem;
pub mod embed;
pub mod embedding_provider_catalog;
pub mod error;
pub mod eval_harness;
pub mod evidence_verifier;
pub mod evolution;
pub mod execution_environment;
pub mod feedback;
pub mod file_checkpoint;
pub mod graph_retrieval;
pub mod image_provider_catalog;
pub mod index;
pub mod ingest;
pub mod intelligence;
pub mod interaction;
pub mod knowledge_graph;
pub mod knowledge_loop;
pub mod learning;
pub mod lint;
pub mod llm;
pub mod managed_assets;
pub mod mcp;
pub mod media;
pub mod migrations;
pub mod mixture_of_agents;
pub mod model_catalog;
pub mod models;
#[cfg(feature = "ocr")]
pub mod ocr;
#[cfg(not(feature = "ocr"))]
#[path = "ocr_disabled.rs"]
pub mod ocr;
pub mod office_runtime;
pub mod package_host;
pub mod parse;
pub mod persona;
pub mod personalization;
pub mod playbook;
pub mod plugins;
pub mod policy_engine;
pub mod preview;
pub mod privacy;
pub mod project;
pub mod project_memory;
pub mod protocol_exports;
pub mod provider_catalog;
pub mod provider_registry;
pub mod quality_eval;
pub mod quality_profile;
pub mod rag;
pub mod runtime;
pub mod search;
pub mod settings_schema_v2;
pub mod skills;
pub mod source_tree;
pub mod sources;
pub mod speech_to_text;
pub mod task_orchestrator;
pub mod task_run;
pub mod task_timeline;
pub mod tool_access;
pub mod tool_visibility_policy;
pub mod tools;
pub mod trace;
pub mod trajectory;
pub mod tts_provider_catalog;
pub mod usage_analytics;
pub mod usage_snapshot;
#[cfg(feature = "video")]
pub mod video;
pub mod vision_router;
pub mod visual_document;
pub mod watcher;
pub mod web_search;
pub mod wiki;
pub mod work_plan;
pub mod workflow_automation;
pub mod workflow_catalog;
pub mod workflow_ir;

#[cfg(test)]
mod architecture_fitness;
