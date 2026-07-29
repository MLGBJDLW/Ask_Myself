//! DownloadAssetTool — saves supported public image assets to the workspace.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use async_trait::async_trait;
use image::GenericImageView;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE};
use serde::Deserialize;
use uuid::Uuid;

use crate::db::Database;
use crate::error::CoreError;

use super::fetch_url_tool::{
    build_http_client, read_limited_body, send_with_safe_redirects, validate_url_for_fetch,
};
use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/download_asset.json");
const DEFAULT_MAX_ASSET_BYTES: usize = 10 * 1024 * 1024;
const HARD_MAX_ASSET_BYTES: usize = 25 * 1024 * 1024;

pub struct DownloadAssetTool;

#[derive(Debug, Deserialize)]
struct DownloadAssetArgs {
    url: String,
    output_dir: Option<String>,
    filename: Option<String>,
    #[serde(default = "default_max_bytes")]
    max_bytes: usize,
}

fn default_max_bytes() -> usize {
    DEFAULT_MAX_ASSET_BYTES
}

fn content_type_extension(content_type: Option<&str>) -> Option<&'static str> {
    let media_type = content_type?
        .split(';')
        .next()
        .map(str::trim)?
        .to_ascii_lowercase();
    match media_type.as_str() {
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/webp" => Some("webp"),
        "image/gif" => Some("gif"),
        _ => None,
    }
}

fn is_supported_image_content_type(content_type: Option<&str>) -> bool {
    content_type_extension(content_type).is_some()
}

fn sanitize_filename(raw: &str) -> String {
    let trimmed = raw.trim();
    let mut safe: String = trimmed
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            ch if ch.is_control() => '-',
            _ => ch,
        })
        .collect();
    safe = safe.trim_matches(['.', ' ']).to_string();
    if safe.is_empty() {
        format!("downloaded-asset-{}", Uuid::new_v4())
    } else {
        safe
    }
}

fn filename_from_url(url: &reqwest::Url, extension: &str) -> String {
    let name = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.trim().is_empty())
        .map(sanitize_filename)
        .unwrap_or_else(|| format!("downloaded-asset-{}", Uuid::new_v4()));
    ensure_extension(name, extension)
}

fn ensure_extension(mut filename: String, extension: &str) -> String {
    if Path::new(&filename).extension().is_none() {
        filename.push('.');
        filename.push_str(extension);
    }
    filename
}

fn default_output_dir(db: &Database) -> Result<PathBuf, CoreError> {
    let sources = db.list_sources()?;
    if let Some(source) = sources.first() {
        return Ok(PathBuf::from(&source.root_path).join("downloaded-assets"));
    }
    Ok(std::env::current_dir()?.join("downloaded-assets"))
}

fn resolve_output_path(
    db: &Database,
    args: &DownloadAssetArgs,
    final_url: &reqwest::Url,
    extension: &str,
) -> Result<PathBuf, CoreError> {
    let filename = args
        .filename
        .as_deref()
        .map(sanitize_filename)
        .map(|name| ensure_extension(name, extension))
        .unwrap_or_else(|| filename_from_url(final_url, extension));

    let dir = if let Some(output_dir) = args
        .output_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let requested = PathBuf::from(output_dir);
        if requested.is_absolute() {
            requested
        } else {
            default_output_dir(db)?.join(requested)
        }
    } else {
        default_output_dir(db)?
    };

    let path = unique_path(dir.join(filename));
    validate_output_path(db, &path)?;
    Ok(path)
}

fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("downloaded-asset");
    let extension = path.extension().and_then(|value| value.to_str());
    for index in 2..1000 {
        let filename = match extension {
            Some(extension) => format!("{stem}-{index}.{extension}"),
            None => format!("{stem}-{index}"),
        };
        let candidate = parent.join(filename);
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!(
        "{stem}-{}.{}",
        Uuid::new_v4(),
        extension.unwrap_or("bin")
    ))
}

fn validate_output_path(db: &Database, path: &Path) -> Result<(), CoreError> {
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(CoreError::InvalidInput(
            "Asset output path must not contain '..'.".into(),
        ));
    }

    let parent = path.parent().ok_or_else(|| {
        CoreError::InvalidInput("Asset output path has no parent directory.".into())
    })?;
    std::fs::create_dir_all(parent)?;
    let canonical_parent = std::fs::canonicalize(parent)?;
    let target = canonical_parent.join(
        path.file_name()
            .ok_or_else(|| CoreError::InvalidInput("Asset output path has no filename.".into()))?,
    );

    let sources = db.list_sources()?;
    if sources.is_empty() {
        let current = std::fs::canonicalize(std::env::current_dir()?)?;
        if target.starts_with(&current) {
            return Ok(());
        }
        return Err(CoreError::InvalidInput(format!(
            "Asset output path must stay under the current directory when no sources are registered: {}",
            current.display()
        )));
    }

    for source in sources {
        let root = PathBuf::from(source.root_path);
        if let Ok(canonical_root) = std::fs::canonicalize(root) {
            if target.starts_with(canonical_root) {
                return Ok(());
            }
        }
    }

    Err(CoreError::InvalidInput(
        "Asset output path must stay inside a registered source root.".into(),
    ))
}

#[async_trait]
impl Tool for DownloadAssetTool {
    fn name(&self) -> &str {
        "download_asset"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::Web]
    }

    fn requires_confirmation(&self, _args: &serde_json::Value) -> bool {
        true
    }

    fn confirmation_message(&self, args: &serde_json::Value) -> Option<String> {
        let url = args
            .get("url")
            .and_then(|value| value.as_str())
            .unwrap_or("the remote asset");
        Some(format!(
            "Download image asset from {url} into the workspace?"
        ))
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments,
            db,
            source_scope: _source_scope,
            ..
        } = context;
        let args: DownloadAssetArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid download_asset arguments: {e}"))
        })?;

        let parsed_url = match validate_url_for_fetch(&args.url).await {
            Ok(url) => url,
            Err(msg) => {
                return Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: msg,
                    is_error: true,
                    artifacts: None,
                });
            }
        };
        let max_bytes = if args.max_bytes == 0 {
            DEFAULT_MAX_ASSET_BYTES
        } else {
            args.max_bytes.min(HARD_MAX_ASSET_BYTES)
        };

        let client = build_http_client()
            .map_err(|e| CoreError::InvalidInput(format!("Failed to build HTTP client: {e}")))?;
        let (response, final_url, redirect_count) =
            match send_with_safe_redirects(&client, parsed_url).await {
                Ok(result) => result,
                Err(e) => {
                    return Ok(ToolResult {
                        call_id: call_id.to_string(),
                        content: e,
                        is_error: true,
                        artifacts: None,
                    });
                }
            };

        let status = response.status();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let content_length = response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok());

        if !status.is_success() {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: format!("HTTP {status} downloading {}", final_url),
                is_error: true,
                artifacts: None,
            });
        }
        if !is_supported_image_content_type(content_type.as_deref()) {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: format!(
                    "download_asset only supports JPEG, PNG, WebP, and GIF images. Content type was {}.",
                    content_type.as_deref().unwrap_or("unknown")
                ),
                is_error: true,
                artifacts: Some(serde_json::json!({
                    "kind": "downloadAsset",
                    "url": args.url,
                    "finalUrl": final_url.as_str(),
                    "contentType": content_type,
                    "redirectCount": redirect_count,
                    "saved": false,
                })),
            });
        }
        if let Some(content_length) = content_length {
            if content_length > max_bytes {
                return Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!(
                        "Asset is too large: {content_length} bytes exceeds the {max_bytes} byte limit."
                    ),
                    is_error: true,
                    artifacts: None,
                });
            }
        }

        let (bytes, truncated) = match read_limited_body(response, max_bytes).await {
            Ok(body) => body,
            Err(e) => {
                return Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: e,
                    is_error: true,
                    artifacts: None,
                });
            }
        };
        if truncated {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: format!("Asset exceeded the {max_bytes} byte download limit."),
                is_error: true,
                artifacts: None,
            });
        }

        let image = match image::load_from_memory(&bytes) {
            Ok(image) => image,
            Err(e) => {
                return Ok(ToolResult {
                    call_id: call_id.to_string(),
                    content: format!("Downloaded bytes were not a decodable image: {e}"),
                    is_error: true,
                    artifacts: None,
                });
            }
        };
        let (width, height) = image.dimensions();
        let extension = content_type_extension(content_type.as_deref()).unwrap_or("img");
        let output_path = resolve_output_path(db, &args, &final_url, extension)?;
        std::fs::write(&output_path, &bytes)?;

        Ok(ToolResult {
            call_id: call_id.to_string(),
            content: format!(
                "Downloaded image asset.\nURL: {}\nFinal URL: {}\nPath: {}\nContent type: {}\nSize: {} bytes\nDimensions: {}x{}",
                args.url,
                final_url,
                output_path.display(),
                content_type.as_deref().unwrap_or("unknown"),
                bytes.len(),
                width,
                height
            ),
            is_error: false,
            artifacts: Some(serde_json::json!({
                "kind": "downloadAsset",
                "url": args.url,
                "finalUrl": final_url.as_str(),
                "path": output_path,
                "contentType": content_type,
                "bytes": bytes.len(),
                "width": width,
                "height": height,
                "redirectCount": redirect_count,
                "saved": true,
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_filename_removes_path_and_windows_forbidden_chars() {
        assert_eq!(sanitize_filename(r#"..\bad:name?.png"#), "-bad-name-.png");
    }

    #[test]
    fn filename_from_url_adds_extension_when_missing() {
        let url = reqwest::Url::parse("https://example.com/assets/chart").unwrap();
        assert!(filename_from_url(&url, "webp").ends_with(".webp"));
    }

    #[test]
    fn content_type_allowlist_is_image_only() {
        assert!(is_supported_image_content_type(Some("image/png")));
        assert!(is_supported_image_content_type(Some(
            "image/webp; charset=binary"
        )));
        assert!(!is_supported_image_content_type(Some("application/pdf")));
        assert!(!is_supported_image_content_type(Some("text/html")));
    }
}
