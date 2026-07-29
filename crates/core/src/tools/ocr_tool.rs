//! OCR tool for extracting text from local image files.

use std::path::PathBuf;
use std::sync::OnceLock;

use async_trait::async_trait;
use rusqlite::params;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;
use crate::{media, privacy};

use super::path_utils::resolve_existing_file_for_file_access;
use super::{ensure_source_in_scope, file_access_policy, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/extract_image_text.json");
const DEFAULT_MAX_CHARS: usize = 12_000;
const MAX_CHARS_LIMIT: usize = 50_000;

pub struct ExtractImageTextTool;

#[derive(Deserialize)]
struct ExtractImageTextArgs {
    path: Option<String>,
    #[serde(default, alias = "documentId")]
    document_id: Option<String>,
    #[serde(default, alias = "maxChars")]
    max_chars: Option<usize>,
}

struct ImageTarget {
    path: PathBuf,
    document_id: Option<String>,
    document_title: Option<String>,
    media_type: String,
}

#[async_trait]
impl Tool for ExtractImageTextTool {
    fn name(&self) -> &str {
        "extract_image_text"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::DocumentAnalysis]
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments,
            db,
            source_scope,
            ..
        } = context;
        let args: ExtractImageTextArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid extract_image_text arguments: {e}"))
        })?;

        let db = db.clone();
        let call_id = call_id.to_string();
        let source_scope = source_scope.to_vec();
        tokio::task::spawn_blocking(move || {
            let max_chars = args
                .max_chars
                .unwrap_or(DEFAULT_MAX_CHARS)
                .clamp(500, MAX_CHARS_LIMIT);
            let target = resolve_image_target(&db, &source_scope, &args)?;

            if !media::is_supported_image(&target.media_type) {
                return Ok(ToolResult {
                    call_id,
                    content: format!(
                        "Error: '{}' is not a supported OCR image type (detected {}). Supported image types are JPEG, PNG, GIF, and WebP.",
                        target.path.display(),
                        target.media_type
                    ),
                    is_error: true,
                    artifacts: Some(serde_json::json!({
                        "code": "unsupportedImageType",
                        "path": target.path.to_string_lossy(),
                        "mediaType": target.media_type,
                    })),
                });
            }

            let ocr_config = db.load_ocr_config()?;
            if !ocr_config.enabled {
                return Ok(ToolResult {
                    call_id,
                    content:
                        "Error: OCR is disabled in Settings. Enable OCR and download the OCR models before using extract_image_text."
                            .to_string(),
                    is_error: true,
                    artifacts: Some(serde_json::json!({
                        "code": "ocrDisabled",
                        "path": target.path.to_string_lossy(),
                    })),
                });
            }

            let bytes = std::fs::read(&target.path)?;
            let result = match crate::ocr::extract_text_from_image(
                &bytes,
                &target.media_type,
                &ocr_config,
                None,
            ) {
                Ok(result) => result,
                Err(error) => {
                    return Ok(ToolResult {
                        call_id,
                        content: format!(
                            "Error: OCR failed for '{}': {error}\n\nCheck that OCR models are downloaded in Settings > Models > OCR.",
                            target.path.display()
                        ),
                        is_error: true,
                        artifacts: Some(serde_json::json!({
                            "code": "ocrFailed",
                            "path": target.path.to_string_lossy(),
                            "mediaType": target.media_type,
                            "error": error.to_string(),
                        })),
                    });
                }
            };

            let privacy_config = db.load_privacy_config().unwrap_or_default();
            let redacted_text = if privacy_config.enabled {
                privacy::redact_content(&result.full_text, &privacy_config.redact_patterns)
            } else {
                result.full_text.clone()
            };
            let (display_text, truncated) = truncate_chars(&redacted_text, max_chars);
            let source = ocr_source_label(&result.source);

            let mut content = format!(
                "Image OCR result\nPath: {}\n",
                target.path.display()
            );
            if let Some(document_id) = &target.document_id {
                content.push_str(&format!("Document ID: {document_id}\n"));
            }
            if let Some(title) = &target.document_title {
                content.push_str(&format!("Title: {title}\n"));
            }
            content.push_str(&format!(
                "Media type: {}\nSource: {}\nAverage confidence: {:.2}\nRegions: {}\n",
                target.media_type,
                source,
                result.avg_confidence,
                result.regions.len()
            ));

            if result.full_text.trim().is_empty() {
                content.push_str("---\nNo visible text was extracted from this image.");
            } else {
                if truncated {
                    content.push_str(&format!(
                        "Text truncated to {max_chars} characters for context.\n"
                    ));
                }
                content.push_str("---\n");
                content.push_str(&display_text);
            }

            Ok(ToolResult {
                call_id,
                content,
                is_error: false,
                artifacts: Some(serde_json::json!({
                    "path": target.path.to_string_lossy(),
                    "documentId": target.document_id,
                    "documentTitle": target.document_title,
                    "mediaType": target.media_type,
                    "source": source,
                    "avgConfidence": result.avg_confidence,
                    "regionCount": result.regions.len(),
                    "textLength": redacted_text.chars().count(),
                    "truncated": truncated,
                    "maxChars": max_chars,
                })),
            })
        })
        .await
        .map_err(|e| CoreError::Internal(format!("OCR task join failed: {e}")))?
    }
}

fn resolve_image_target(
    db: &Database,
    source_scope: &[String],
    args: &ExtractImageTextArgs,
) -> Result<ImageTarget, CoreError> {
    let path = args
        .path
        .as_ref()
        .map(|value| value.trim())
        .filter(|v| !v.is_empty());
    let document_id = args
        .document_id
        .as_ref()
        .map(|value| value.trim())
        .filter(|v| !v.is_empty());

    match (path, document_id) {
        (Some(_), Some(_)) => Err(CoreError::InvalidInput(
            "Provide either path or document_id, not both.".to_string(),
        )),
        (None, None) => Err(CoreError::InvalidInput(
            "extract_image_text requires either path or document_id.".to_string(),
        )),
        (Some(path), None) => {
            let requested = PathBuf::from(path);
            let file_policy = file_access_policy(db, source_scope)?;
            if file_policy.sources.is_empty()
                && !(file_policy.allow_unregistered_absolute_paths && requested.is_absolute())
            {
                return Err(CoreError::InvalidInput(format!(
                    "Access denied: '{path}' is not within any directory available in the current source scope."
                )));
            }
            let canonical = resolve_existing_file_for_file_access(
                &requested,
                &file_policy.sources,
                file_policy.allow_unregistered_absolute_paths,
            )
            .map_err(CoreError::InvalidInput)?;
            let media_type = crate::parse::detect_mime_type(&canonical);
            Ok(ImageTarget {
                path: canonical,
                document_id: None,
                document_title: None,
                media_type,
            })
        }
        (None, Some(document_id)) => {
            let conn = db.conn();
            let (source_id, path, title, mime_type): (String, String, Option<String>, String) =
                conn.query_row(
                    "SELECT source_id, path, title, mime_type FROM documents WHERE id = ?1",
                    params![document_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .map_err(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => CoreError::InvalidInput(format!(
                        "No indexed document found for document_id '{document_id}'."
                    )),
                    other => CoreError::Database(other),
                })?;
            ensure_source_in_scope(&source_id, source_scope).map_err(CoreError::InvalidInput)?;
            Ok(ImageTarget {
                path: PathBuf::from(path),
                document_id: Some(document_id.to_string()),
                document_title: title,
                media_type: mime_type,
            })
        }
    }
}

fn truncate_chars(text: &str, max_chars: usize) -> (String, bool) {
    let mut iter = text.chars();
    let truncated: String = iter.by_ref().take(max_chars).collect();
    let was_truncated = iter.next().is_some();
    (truncated, was_truncated)
}

fn ocr_source_label(source: &crate::ocr::OcrSource) -> &'static str {
    match source {
        crate::ocr::OcrSource::PaddleOcr => "paddleOcr",
        crate::ocr::OcrSource::LlmVision => "llmVision",
        crate::ocr::OcrSource::None => "none",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_settings::{AppConfig, ShellAccessMode};
    use crate::ocr::OcrConfig;
    use crate::sources::CreateSourceInput;
    use std::io::Write;

    fn setup_db_with_source(root: &std::path::Path) -> Database {
        let db = Database::open_memory().expect("open in-memory db");
        db.add_source(CreateSourceInput {
            root_path: root.to_string_lossy().to_string(),
            include_globs: vec!["**/*".into()],
            exclude_globs: vec![],
            watch_enabled: false,
        })
        .expect("add source");
        db
    }

    #[tokio::test]
    async fn ocr_tool_reports_disabled_ocr_without_loading_models() {
        let dir = tempfile::tempdir().expect("tempdir");
        let image_path = dir.path().join("sample.png");
        std::fs::File::create(&image_path)
            .expect("create image")
            .write_all(b"not really a png")
            .expect("write image");

        let db = setup_db_with_source(dir.path());
        let mut app_config = AppConfig::default();
        app_config.shell_access_mode = ShellAccessMode::Restricted;
        db.save_app_config(&app_config).expect("save app config");
        db.save_ocr_config(&OcrConfig {
            enabled: false,
            ..OcrConfig::default()
        })
        .expect("save ocr config");

        let tool = ExtractImageTextTool;
        let result = tool
            .execute(crate::tools::ToolExecutionContext::new(
                "call-1",
                &serde_json::json!({ "path": image_path.to_string_lossy() }).to_string(),
                &db,
                &[],
            ))
            .await
            .expect("tool result");

        assert!(result.is_error);
        assert!(result.content.contains("OCR is disabled"));
        assert_eq!(result.artifacts.unwrap()["code"], "ocrDisabled");
    }
}
