//! Ask-core — knowledge-base engine with embedding, search, and OCR.
//!
//! This crate provides the core functionality for ingesting, parsing,
//! embedding, and searching personal knowledge documents.  An optional
//! OCR module (feature-gated) adds ONNX-based PaddleOCR for extracting
//! text from images and scanned PDFs.

pub mod agent;
pub mod app_settings;
pub mod cache;
pub mod compile;
pub mod conversation;
pub mod crypto;
pub mod db;
pub mod embed;
pub mod error;
pub mod feedback;
pub mod index;
pub mod ingest;
pub mod knowledge_graph;
pub mod knowledge_loop;
pub mod lint;
pub mod llm;
pub mod mcp;
pub mod media;
pub mod migrations;
pub mod models;
#[cfg(feature = "ocr")]
pub mod ocr;
pub mod parse;
pub mod personalization;
pub mod playbook;
pub mod privacy;
pub mod project;
pub mod search;
pub mod skills;
pub mod sources;
pub mod tools;
pub mod trace;
#[cfg(feature = "video")]
pub mod video;
pub mod watcher;
pub mod wiki;
