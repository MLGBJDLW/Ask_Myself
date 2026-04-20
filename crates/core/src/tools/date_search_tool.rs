//! DateSearchTool — browse documents by modification date range.

use std::sync::OnceLock;

use async_trait::async_trait;
use chrono::NaiveDate;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::{ensure_source_in_scope, scope_is_active, Tool, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/search_by_date.json");

pub struct DateSearchTool;

#[derive(Deserialize)]
struct DateSearchArgs {
    after: Option<String>,
    before: Option<String>,
    source_id: Option<String>,
    #[serde(default = "default_limit")]
    limit: u32,
    #[serde(default = "default_order")]
    order: String,
}

fn default_limit() -> u32 {
    50
}

fn default_order() -> String {
    "newest".to_string()
}

/// Normalize a user-supplied date string into an ISO 8601 datetime suitable
/// for SQLite TEXT comparison against `modified_at` (which stores full
/// datetime strings).  Accepts:
///   - Full datetime:  `2024-07-01T09:00:00Z`, `2024-07-01 09:00:00`
///   - Date only:      `2024-07-01`  → becomes `2024-07-01T00:00:00`
///
/// Returns the normalized string or a human-readable error.
fn normalize_date(input: &str) -> Result<String, String> {
    let trimmed = input.trim();

    // Already looks like a full datetime — pass through after basic validation.
    if trimmed.len() > 10 {
        // Strip trailing 'Z' for uniform comparison.
        let cleaned = trimmed.trim_end_matches('Z').replace('T', " ");
        // Quick sanity: must start with a valid date portion.
        let date_part = &cleaned[..10.min(cleaned.len())];
        NaiveDate::parse_from_str(date_part, "%Y-%m-%d")
            .map_err(|e| format!("Invalid date '{input}': {e}"))?;
        return Ok(cleaned);
    }

    // Date-only: YYYY-MM-DD
    let date = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d")
        .map_err(|e| format!("Invalid date '{input}': {e}"))?;
    Ok(format!("{date} 00:00:00"))
}

#[async_trait]
impl Tool for DateSearchTool {
    fn name(&self) -> &str {
        "search_by_date"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: DateSearchArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid search_by_date arguments: {e}"))
        })?;

        // Validate & normalize dates up-front (before entering blocking task).
        let after = args
            .after
            .as_deref()
            .map(normalize_date)
            .transpose()
            .map_err(CoreError::InvalidInput)?;

        let before = args
            .before
            .as_deref()
            .map(normalize_date)
            .transpose()
            .map_err(CoreError::InvalidInput)?;

        let order_desc = args.order != "oldest";
        let limit = args.limit.clamp(1, 200);
        let source_id = args.source_id.clone();

        let db = db.clone();
        let call_id = call_id.to_string();
        let source_scope = source_scope.to_vec();

        tokio::task::spawn_blocking(move || {
            // Build dynamic query.
            let mut conditions: Vec<String> = Vec::new();
            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

            if let Some(ref a) = after {
                conditions.push(format!("d.modified_at > ?{}", params.len() + 1));
                params.push(Box::new(a.clone()));
            }
            if let Some(ref b) = before {
                conditions.push(format!("d.modified_at < ?{}", params.len() + 1));
                params.push(Box::new(b.clone()));
            }
            if let Some(ref sid) = source_id {
                if let Err(message) = ensure_source_in_scope(sid, &source_scope) {
                    return Ok(ToolResult {
                        call_id,
                        content: message,
                        is_error: true,
                        artifacts: None,
                    });
                }
                conditions.push(format!("d.source_id = ?{}", params.len() + 1));
                params.push(Box::new(sid.clone()));
            } else if scope_is_active(&source_scope) {
                let placeholders: Vec<String> = source_scope
                    .iter()
                    .enumerate()
                    .map(|(idx, _)| format!("?{}", params.len() + idx + 1))
                    .collect();
                conditions.push(format!("d.source_id IN ({})", placeholders.join(", ")));
                for sid in &source_scope {
                    params.push(Box::new(sid.clone()));
                }
            }

            let where_clause = if conditions.is_empty() {
                String::new()
            } else {
                format!("WHERE {}", conditions.join(" AND "))
            };

            let order_dir = if order_desc { "DESC" } else { "ASC" };

            let sql = format!(
                "SELECT d.id, d.path, d.title, d.mime_type, d.file_size, d.modified_at, s.root_path
                 FROM documents d
                 JOIN sources s ON s.id = d.source_id
                 {where_clause}
                 ORDER BY d.modified_at {order_dir}
                 LIMIT ?{}",
                params.len() + 1,
            );
            params.push(Box::new(limit));

            let conn = db.conn();
            let mut stmt = conn.prepare(&sql)?;

            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();

            let rows: Vec<(String, String, Option<String>, String, i64, String, String)> = stmt
                .query_map(param_refs.as_slice(), |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;

            if rows.is_empty() {
                let mut msg = "No documents found".to_string();
                if after.is_some() || before.is_some() {
                    msg.push_str(" in the specified date range");
                }
                msg.push('.');
                return Ok(ToolResult {
                    call_id,
                    content: msg,
                    is_error: false,
                    artifacts: None,
                });
            }

            let mut text = format!("{} document(s) found", rows.len());
            if after.is_some() || before.is_some() {
                text.push_str(" in date range");
                if let Some(ref a) = after {
                    text.push_str(&format!(" after {a}"));
                }
                if let Some(ref b) = before {
                    text.push_str(&format!(" before {b}"));
                }
            }
            text.push_str(":\n\n");

            let mut artifacts = Vec::new();

            for (id, path, title, mime_type, file_size, modified_at, source_root) in &rows {
                let title_display = title.as_deref().unwrap_or("(untitled)");
                let size_display = if *file_size > 1024 * 1024 {
                    format!("{:.1} MB", *file_size as f64 / (1024.0 * 1024.0))
                } else if *file_size > 1024 {
                    format!("{:.1} KB", *file_size as f64 / 1024.0)
                } else {
                    format!("{file_size} B")
                };
                text.push_str(&format!(
                    "- [{modified_at}] {path}\n  Title: {title_display} | {mime_type} | {size_display}\n  Source: {source_root}\n\n",
                ));
                artifacts.push(serde_json::json!({
                    "id": id,
                    "path": path,
                    "title": title,
                    "mime_type": mime_type,
                    "file_size": file_size,
                    "modified_at": modified_at,
                }));
            }

            Ok(ToolResult {
                call_id,
                content: text,
                is_error: false,
                artifacts: Some(serde_json::Value::Array(artifacts)),
            })
        })
        .await
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_date_only() {
        let result = normalize_date("2024-07-01").unwrap();
        assert_eq!(result, "2024-07-01 00:00:00");
    }

    #[test]
    fn test_normalize_full_datetime_with_z() {
        let result = normalize_date("2024-07-01T09:30:00Z").unwrap();
        assert_eq!(result, "2024-07-01 09:30:00");
    }

    #[test]
    fn test_normalize_full_datetime_without_z() {
        let result = normalize_date("2024-07-01 14:00:00").unwrap();
        assert_eq!(result, "2024-07-01 14:00:00");
    }

    #[test]
    fn test_normalize_invalid_date() {
        assert!(normalize_date("not-a-date").is_err());
        assert!(normalize_date("2024-13-01").is_err());
        assert!(normalize_date("2024-02-30").is_err());
    }
}
