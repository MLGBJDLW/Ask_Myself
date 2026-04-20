//! Data model structs for the nexa-core crate.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Represents a content source (directory or file root).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub id: String,
    pub kind: String,
    pub root_path: String,
    pub include_globs: Vec<String>,
    pub exclude_globs: Vec<String>,
    pub watch_enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Supported file types for ingestion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FileType {
    Markdown,
    PlainText,
    Log,
    Pdf,
    Docx,
    Excel,
    Pptx,
    Image,
    Video,
    Audio,
}

/// An evidence card surfaced by a search query.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceCard {
    pub chunk_id: Uuid,
    pub document_id: Uuid,
    pub source_id: Uuid,
    pub source_name: String,
    pub document_path: String,
    pub document_title: String,
    pub content: String,
    pub heading_path: Vec<String>,
    pub score: f64,
    pub highlights: Vec<Highlight>,
    /// Short excerpt (first 150 chars) for preview display.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// Document date from metadata (frontmatter `date`/`created` or filesystem).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_date: Option<String>,
    /// Source credibility score (0.0–1.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credibility: Option<f64>,
    /// Document age in days (computed from `document_date`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub freshness_days: Option<i64>,
}

/// A highlighted span within content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Highlight {
    pub start: usize,
    pub end: usize,
    pub term: String,
}

/// A search query with optional filters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    pub text: String,
    pub filters: SearchFilters,
    pub limit: u32,
    pub offset: u32,
}

/// Filters applied to narrow search results.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFilters {
    pub source_ids: Vec<Uuid>,
    pub file_types: Vec<FileType>,
    pub date_from: Option<DateTime<Utc>>,
    pub date_to: Option<DateTime<Utc>>,
}

/// A playbook is a saved, composable set of evidence cards.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playbook {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_text: Option<String>,
    pub citations: Vec<PlaybookCitation>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A citation within a playbook, referencing a specific chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybookCitation {
    pub id: Uuid,
    pub playbook_id: Uuid,
    pub chunk_id: Uuid,
    pub annotation: String,
    pub order: u32,
}

/// A persistent record of a file that failed to scan/ingest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanError {
    pub source_id: String,
    pub path: String,
    pub error_message: String,
    pub error_count: i64,
    pub first_failed_at: String,
    pub last_failed_at: String,
}
