//! ListDirTool — lists directory contents within registered source roots.

use std::path::Path;
use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::db::Database;
use crate::error::CoreError;

use super::path_utils::resolve_existing_directory_in_sources;
use super::{scoped_sources, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/list_dir.json");

/// Tool that lists directory contents, optionally recursively with glob filtering.
pub struct ListDirTool;

#[derive(Deserialize)]
struct ListDirArgs {
    path: String,
    #[serde(default)]
    recursive: bool,
    #[serde(default = "default_max_depth")]
    max_depth: u32,
    pattern: Option<String>,
}

fn default_max_depth() -> u32 {
    3
}

/// A single directory entry in the result.
#[derive(serde::Serialize)]
struct DirEntry {
    name: String,
    path: String,
    #[serde(rename = "type")]
    entry_type: &'static str,
    size_bytes: u64,
    modified: String,
}

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::Core, ToolCategory::FileSystem]
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: ListDirArgs = serde_json::from_str(arguments)
            .map_err(|e| CoreError::InvalidInput(format!("Invalid list_dir arguments: {e}")))?;

        let db = db.clone();
        let call_id = call_id.to_string();
        let source_scope = source_scope.to_vec();
        tokio::task::spawn_blocking(move || {
            let sources = scoped_sources(&db, &source_scope)?;
            if sources.is_empty() {
                return Ok(ToolResult {
                    call_id: call_id.clone(),
                    content: format!(
                        "Access denied: '{}' is not within any directory available in the current source scope.",
                        args.path
                    ),
                    is_error: true,
                    artifacts: None,
                });
            }
            let canonical = resolve_existing_directory_in_sources(Path::new(&args.path), &sources)
                .map_err(CoreError::InvalidInput)?;

            // Collect entries.
            let mut entries = Vec::new();
            let max_depth = if args.recursive { args.max_depth } else { 1 };
            collect_entries(
                &canonical,
                &canonical,
                max_depth,
                0,
                &args.pattern,
                &mut entries,
            )?;

            // Format response.
            let mut text = format!("Directory: {}\n", canonical.display());
            text.push_str(&format!("{} entries found.\n\n", entries.len()));
            for e in &entries {
                let type_marker = if e.entry_type == "dir" { "/" } else { "" };
                text.push_str(&format!(
                    "{}{type_marker}  ({} bytes, modified {})\n",
                    e.path, e.size_bytes, e.modified
                ));
            }

            Ok(ToolResult {
                call_id,
                content: text,
                is_error: false,
                artifacts: Some(json!(entries)),
            })
        })
        .await
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?
    }
}

/// Recursively collect directory entries up to `max_depth`.
fn collect_entries(
    base: &Path,
    dir: &Path,
    max_depth: u32,
    current_depth: u32,
    pattern: &Option<String>,
    entries: &mut Vec<DirEntry>,
) -> Result<(), CoreError> {
    if current_depth >= max_depth {
        return Ok(());
    }

    let read_dir = std::fs::read_dir(dir).map_err(CoreError::Io)?;

    for entry_result in read_dir {
        let entry = entry_result.map_err(CoreError::Io)?;
        let metadata = entry.metadata().map_err(CoreError::Io)?;
        let file_name = entry.file_name().to_string_lossy().to_string();
        let is_dir = metadata.is_dir();

        // Apply pattern filter (only to files).
        if !is_dir {
            if let Some(ref pat) = pattern {
                if !matches_glob_simple(pat, &file_name) {
                    continue;
                }
            }
        }

        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| {
                chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_default()
            })
            .unwrap_or_else(|| "unknown".to_string());

        let relative = entry
            .path()
            .strip_prefix(base)
            .unwrap_or(&entry.path())
            .to_string_lossy()
            .to_string();

        entries.push(DirEntry {
            name: file_name,
            path: relative,
            entry_type: if is_dir { "dir" } else { "file" },
            size_bytes: metadata.len(),
            modified,
        });

        // Recurse into subdirectories.
        if is_dir {
            collect_entries(
                base,
                &entry.path(),
                max_depth,
                current_depth + 1,
                pattern,
                entries,
            )?;
        }
    }
    Ok(())
}

/// Simple glob matching: supports `*` as wildcard prefix (e.g., `*.md`).
fn matches_glob_simple(pattern: &str, filename: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix('*') {
        filename.ends_with(suffix)
    } else if let Some(prefix) = pattern.strip_suffix('*') {
        filename.starts_with(prefix)
    } else if pattern.contains('*') {
        // Fallback: split on '*' and check parts appear in order.
        let parts: Vec<&str> = pattern.split('*').collect();
        let mut remaining = filename;
        for part in parts {
            if part.is_empty() {
                continue;
            }
            if let Some(pos) = remaining.find(part) {
                remaining = &remaining[pos + part.len()..];
            } else {
                return false;
            }
        }
        true
    } else {
        filename == pattern
    }
}
