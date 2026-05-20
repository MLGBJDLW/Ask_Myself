use super::*;

/// An image attachment prepared for LLM submission.
///
/// Re-uses [`nexa_core::conversation::ImageAttachment`] so that the same
/// serialized shape (camelCase JSON) can be persisted alongside a user
/// message and round-tripped back to the frontend.
#[tauri::command]
pub async fn prepare_image_attachment(path: String) -> Result<ImageAttachment, String> {
    let file_path = std::path::Path::new(&path);
    if !file_path.exists() {
        return Err(format!("File not found: {path}"));
    }

    let bytes = std::fs::read(file_path).map_err(|e| format!("Failed to read file: {e}"))?;
    let mime = nexa_core::parse::detect_mime_type(file_path);

    if !nexa_core::media::is_supported_image(&mime) {
        return Err(format!("Unsupported image type: {mime}"));
    }

    let (base64_data, media_type) =
        nexa_core::media::prepare_image_for_llm(&bytes, &mime).map_err(|e| e.to_string())?;

    let original_name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "image".to_string());

    Ok(ImageAttachment {
        base64_data,
        media_type,
        original_name,
    })
}

// ── File Commands ───────────────────────────────────────────────────────

#[tauri::command]
pub fn open_file_in_default_app(path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(format!("File not found: {path}"));
    }

    #[cfg(target_os = "windows")]
    std::process::Command::new("cmd")
        .args(["/c", "start", "", &path])
        .spawn()
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub fn show_in_file_explorer(path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(format!("File not found: {path}"));
    }

    #[cfg(target_os = "windows")]
    std::process::Command::new("explorer")
        .args(["/select,", &path])
        .spawn()
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .args(["-R", &path])
        .spawn()
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "linux")]
    {
        let parent = p.parent().unwrap_or(p).to_str().unwrap_or(&path);
        std::process::Command::new("xdg-open")
            .arg(parent)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveGeneratedImageInput {
    output_path: String,
    source_path: Option<String>,
    data_url: Option<String>,
    media_type: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveGeneratedImageResult {
    path: String,
    bytes_written: u64,
}

const GENERATED_IMAGE_MAX_BYTES: u64 = 50 * 1024 * 1024;

#[tauri::command]
pub async fn read_generated_image_data_url_cmd(
    path: String,
    media_type: Option<String>,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let (bytes, detected_media_type) =
            read_supported_image_bytes(&path, media_type.as_deref())?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        Ok(format!("data:{detected_media_type};base64,{encoded}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn save_generated_image_cmd(
    input: SaveGeneratedImageInput,
) -> Result<SaveGeneratedImageResult, String> {
    tokio::task::spawn_blocking(move || {
        let (bytes, media_type) = if let Some(source_path) = input
            .source_path
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            read_supported_image_bytes(source_path, input.media_type.as_deref())?
        } else if let Some(data_url) = input
            .data_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            decode_image_data_url(data_url)?
        } else {
            return Err("No generated image payload was available to save.".to_string());
        };

        if !nexa_core::media::is_supported_image(&media_type) {
            return Err(format!("Unsupported generated image type: {media_type}"));
        }

        let output_path = PathBuf::from(input.output_path.trim());
        if output_path.as_os_str().is_empty() {
            return Err("Choose a file path before saving the image.".to_string());
        }
        let extension = output_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif") {
            return Err(
                "Generated images can only be saved as PNG, JPEG, WEBP, or GIF.".to_string(),
            );
        }

        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&output_path, &bytes).map_err(|e| e.to_string())?;

        Ok(SaveGeneratedImageResult {
            path: output_path.to_string_lossy().to_string(),
            bytes_written: bytes.len() as u64,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

fn read_supported_image_bytes(
    path: &str,
    media_type_hint: Option<&str>,
) -> Result<(Vec<u8>, String), String> {
    let file_path = Path::new(path);
    if !file_path.exists() {
        return Err(format!("Generated image preview not found: {path}"));
    }
    let metadata = std::fs::metadata(file_path).map_err(|e| e.to_string())?;
    if metadata.len() > GENERATED_IMAGE_MAX_BYTES {
        return Err("Generated image is too large to preview or save.".to_string());
    }

    let bytes = std::fs::read(file_path).map_err(|e| e.to_string())?;
    let media_type = media_type_hint
        .map(str::trim)
        .filter(|value| nexa_core::media::is_supported_image(value))
        .map(str::to_string)
        .unwrap_or_else(|| nexa_core::parse::detect_mime_type(file_path));
    if !nexa_core::media::is_supported_image(&media_type) {
        return Err(format!("Unsupported generated image type: {media_type}"));
    }
    Ok((bytes, media_type))
}

fn decode_image_data_url(data_url: &str) -> Result<(Vec<u8>, String), String> {
    let (header, payload) = data_url
        .split_once(',')
        .ok_or_else(|| "Generated image data URL is malformed.".to_string())?;
    if !header.to_ascii_lowercase().starts_with("data:image/") {
        return Err("Generated image data URL must contain an image media type.".to_string());
    }
    if !header.to_ascii_lowercase().contains(";base64") {
        return Err("Generated image data URL must be base64 encoded.".to_string());
    }
    let media_type = header
        .trim_start_matches("data:")
        .split(';')
        .next()
        .unwrap_or("image/png")
        .to_string();
    if !nexa_core::media::is_supported_image(&media_type) {
        return Err(format!("Unsupported generated image type: {media_type}"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|e| format!("Generated image data URL is not valid base64: {e}"))?;
    if bytes.len() as u64 > GENERATED_IMAGE_MAX_BYTES {
        return Err("Generated image is too large to preview or save.".to_string());
    }
    Ok((bytes, media_type))
}

const PREVIEW_TEXT_BYTES_LIMIT: u64 = 2 * 1024 * 1024;
const PREVIEW_PARSE_BYTES_LIMIT: u64 = 25 * 1024 * 1024;
const PREVIEW_RELATIVE_SEARCH_LIMIT: usize = 5_000;
const PREVIEW_RELATIVE_SEARCH_MAX_DEPTH: usize = 8;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderedPreviewPage {
    pub page: usize,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderedPreview {
    pub kind: String,
    pub dpi: u32,
    pub page_count: usize,
    pub truncated: bool,
    pub pages: Vec<RenderedPreviewPage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePreview {
    pub path: String,
    pub display_name: String,
    pub source_id: String,
    pub source_name: String,
    pub extension: String,
    pub mime_type: String,
    pub kind: String,
    pub language: Option<String>,
    pub content: Option<String>,
    pub encoding: Option<String>,
    pub editable: bool,
    pub size_bytes: u64,
    pub modified_at: Option<String>,
    pub hash: String,
    pub line_count: usize,
    pub truncated: bool,
    pub warning: Option<String>,
    pub structured_preview: Option<nexa_core::preview::StructuredPreview>,
    pub rendered_preview: Option<RenderedPreview>,
    pub capabilities: nexa_core::preview::PreviewCapabilities,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSaveResult {
    pub preview: FilePreview,
    pub checkpoint_id: String,
    pub bytes_written: u64,
    pub reindex_status: String,
    pub reindex_detail: Option<String>,
}

pub(crate) struct ResolvedSourceFile {
    pub(crate) canonical: PathBuf,
    pub(crate) source: Source,
}

fn source_display_name(source: &Source) -> String {
    Path::new(&source.root_path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| source.root_path.clone())
}

fn should_skip_preview_search_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(".git")
            | Some(".hg")
            | Some(".svn")
            | Some("node_modules")
            | Some("target")
            | Some("dist")
            | Some("build")
            | Some(".venv")
            | Some("venv")
            | Some("__pycache__")
    )
}

fn relative_preview_candidate_matches(root: &Path, candidate: &Path, requested: &Path) -> bool {
    let Ok(relative) = candidate.strip_prefix(root) else {
        return false;
    };
    if requested.components().count() <= 1 {
        return relative.file_name() == requested.file_name();
    }
    relative.ends_with(requested)
}

fn collect_relative_preview_matches(
    root: &Path,
    dir: &Path,
    requested: &Path,
    depth: usize,
    seen: &mut usize,
    matches: &mut Vec<PathBuf>,
) {
    if *seen >= PREVIEW_RELATIVE_SEARCH_LIMIT || depth > PREVIEW_RELATIVE_SEARCH_MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        if *seen >= PREVIEW_RELATIVE_SEARCH_LIMIT {
            return;
        }
        *seen += 1;
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if !should_skip_preview_search_dir(&path) {
                collect_relative_preview_matches(root, &path, requested, depth + 1, seen, matches);
            }
            continue;
        }
        if file_type.is_file() && relative_preview_candidate_matches(root, &path, requested) {
            if let Ok(canonical) = std::fs::canonicalize(&path) {
                matches.push(canonical);
            }
        }
    }
}

fn find_relative_preview_matches(root: &Path, requested: &Path) -> Vec<PathBuf> {
    let mut seen = 0usize;
    let mut matches = Vec::new();
    collect_relative_preview_matches(root, root, requested, 0, &mut seen, &mut matches);
    matches.sort();
    matches.dedup();
    matches
}

pub(crate) fn resolve_source_file(
    db: &Database,
    raw_path: &str,
) -> Result<ResolvedSourceFile, String> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return Err("Path must not be empty.".to_string());
    }
    if trimmed.contains('\0') {
        return Err("Path must not contain null bytes.".to_string());
    }

    let requested = PathBuf::from(trimmed);
    if !requested.is_absolute()
        && requested
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("Path must not contain '..' traversal sequences.".to_string());
    }

    let sources = db.list_sources().map_err(|e| e.to_string())?;
    if sources.is_empty() {
        return Err("No source directories are registered.".to_string());
    }

    if requested.is_absolute() {
        let canonical = std::fs::canonicalize(&requested)
            .map_err(|e| format!("Cannot resolve path '{}': {e}", requested.display()))?;
        if !canonical.is_file() {
            return Err(format!("'{}' is not a file.", requested.display()));
        }
        for source in sources {
            let Ok(root) = std::fs::canonicalize(Path::new(&source.root_path)) else {
                continue;
            };
            if canonical.starts_with(&root) {
                return Ok(ResolvedSourceFile { canonical, source });
            }
        }
        return Err(format!(
            "Access denied: '{}' is not within any registered source directory.",
            requested.display()
        ));
    }

    let mut matches: Vec<ResolvedSourceFile> = Vec::new();
    let mut source_roots: Vec<(Source, PathBuf)> = Vec::new();
    for source in &sources {
        let Ok(root) = std::fs::canonicalize(Path::new(&source.root_path)) else {
            continue;
        };
        source_roots.push((source.clone(), root.clone()));
        let candidate = root.join(&requested);
        let Ok(canonical) = std::fs::canonicalize(&candidate) else {
            continue;
        };
        if canonical.is_file()
            && canonical.starts_with(&root)
            && !matches.iter().any(|entry| entry.canonical == canonical)
        {
            matches.push(ResolvedSourceFile {
                canonical,
                source: source.clone(),
            });
        }
    }

    if matches.is_empty() {
        for (source, root) in &source_roots {
            for canonical in find_relative_preview_matches(root, &requested) {
                if canonical.is_file()
                    && canonical.starts_with(root)
                    && !matches.iter().any(|entry| entry.canonical == canonical)
                {
                    matches.push(ResolvedSourceFile {
                        canonical,
                        source: source.clone(),
                    });
                }
            }
        }
    }

    match matches.len() {
        1 => Ok(matches.remove(0)),
        n if n > 1 => {
            let mut message = format!(
                "Path '{}' is ambiguous across multiple source directories. Use an absolute path.",
                requested.display()
            );
            for entry in matches {
                message.push_str(&format!("\n- {}", entry.canonical.display()));
            }
            Err(message)
        }
        _ => Err(format!(
            "Cannot resolve path '{}' within any registered source directory.",
            requested.display()
        )),
    }
}

fn extension_lower(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| format!(".{}", ext.to_ascii_lowercase()))
        .unwrap_or_default()
}

fn language_for_extension(ext: &str) -> Option<&'static str> {
    match ext {
        ".md" | ".markdown" => Some("markdown"),
        ".json" => Some("json"),
        ".jsonl" => Some("json"),
        ".toml" => Some("toml"),
        ".yaml" | ".yml" => Some("yaml"),
        ".ts" | ".tsx" => Some("typescript"),
        ".js" | ".jsx" | ".mjs" | ".cjs" => Some("javascript"),
        ".rs" => Some("rust"),
        ".py" => Some("python"),
        ".go" => Some("go"),
        ".java" => Some("java"),
        ".c" | ".h" => Some("c"),
        ".cpp" | ".cc" | ".cxx" | ".hpp" => Some("cpp"),
        ".cs" => Some("csharp"),
        ".css" | ".scss" | ".sass" | ".less" => Some("css"),
        ".html" | ".htm" => Some("html"),
        ".xml" => Some("xml"),
        ".sql" => Some("sql"),
        ".sh" | ".bash" | ".zsh" => Some("bash"),
        ".ps1" => Some("powershell"),
        ".bat" | ".cmd" => Some("batch"),
        ".csv" => Some("csv"),
        _ => None,
    }
}

fn is_editable_extension(ext: &str) -> bool {
    matches!(
        ext,
        ".md"
            | ".markdown"
            | ".txt"
            | ".log"
            | ".json"
            | ".jsonl"
            | ".toml"
            | ".yaml"
            | ".yml"
            | ".ts"
            | ".tsx"
            | ".js"
            | ".jsx"
            | ".mjs"
            | ".cjs"
            | ".rs"
            | ".py"
            | ".go"
            | ".java"
            | ".c"
            | ".h"
            | ".cpp"
            | ".cc"
            | ".cxx"
            | ".hpp"
            | ".cs"
            | ".css"
            | ".scss"
            | ".sass"
            | ".less"
            | ".html"
            | ".htm"
            | ".xml"
            | ".sql"
            | ".sh"
            | ".bash"
            | ".zsh"
            | ".ps1"
            | ".bat"
            | ".cmd"
            | ".csv"
    )
}

fn preview_kind(ext: &str, mime_type: &str, binary: bool) -> String {
    if ext == ".md" || ext == ".markdown" {
        "markdown".to_string()
    } else if mime_type == "application/pdf"
        || mime_type.contains("officedocument")
        || mime_type == "application/msword"
        || mime_type == "application/vnd.ms-powerpoint"
        || mime_type.starts_with("application/vnd.oasis.opendocument.")
    {
        "document".to_string()
    } else if binary {
        "binary".to_string()
    } else if language_for_extension(ext).is_some() {
        "code".to_string()
    } else {
        "text".to_string()
    }
}

fn is_document_preview_type(mime_type: &str) -> bool {
    mime_type == "application/pdf"
        || mime_type.contains("officedocument")
        || mime_type == "application/msword"
        || mime_type == "application/vnd.ms-powerpoint"
        || mime_type == "application/epub+zip"
        || mime_type.starts_with("application/vnd.oasis.opendocument.")
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn read_prefix(path: &Path, max_bytes: u64) -> Result<Vec<u8>, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut limited = file.by_ref().take(max_bytes);
    let mut bytes = Vec::new();
    limited.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn decode_preview_text(bytes: &[u8]) -> (String, String, bool) {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        if let Ok(text) = String::from_utf8(bytes[3..].to_vec()) {
            return (text, "utf-8-bom".to_string(), true);
        }
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let (text, _, _) = encoding_rs::UTF_16LE.decode(&bytes[2..]);
        return (text.into_owned(), "utf-16le".to_string(), false);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let (text, _, _) = encoding_rs::UTF_16BE.decode(&bytes[2..]);
        return (text.into_owned(), "utf-16be".to_string(), false);
    }
    match String::from_utf8(bytes.to_vec()) {
        Ok(text) => (text, "utf-8".to_string(), true),
        Err(_) => {
            let (text, _, had_errors) = encoding_rs::GBK.decode(bytes);
            if !had_errors {
                return (text.into_owned(), "gbk".to_string(), false);
            }
            (
                String::from_utf8_lossy(bytes).into_owned(),
                "lossy".to_string(),
                false,
            )
        }
    }
}

fn flatten_parsed_document(parsed: &nexa_core::parse::ParsedDocument) -> String {
    let mut out = String::new();
    for chunk in &parsed.chunks {
        let visible = chunk
            .content
            .get(chunk.overlap_start..)
            .unwrap_or(chunk.content.as_str());
        if !visible.trim().is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(visible);
        }
    }
    if out.trim().is_empty() {
        format!("[No extractable text found in {}]", parsed.file_name)
    } else {
        out
    }
}

fn modified_at_iso(path: &Path) -> Option<String> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let duration = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(
        chrono::DateTime::<Utc>::from(UNIX_EPOCH + duration)
            .to_rfc3339_opts(SecondsFormat::Secs, true),
    )
}

pub(crate) fn append_preview_warning(warning: &mut Option<String>, message: impl Into<String>) {
    let message = message.into();
    if message.trim().is_empty() {
        return;
    }
    match warning {
        Some(existing) if !existing.trim().is_empty() => {
            existing.push('\n');
            existing.push_str(&message);
        }
        _ => *warning = Some(message),
    }
}

pub(crate) fn build_file_preview(
    db: &Database,
    raw_path: &str,
    app_data_dir: Option<&Path>,
) -> Result<FilePreview, String> {
    let resolved = resolve_source_file(db, raw_path)?;
    let metadata = std::fs::metadata(&resolved.canonical).map_err(|e| e.to_string())?;
    let size_bytes = metadata.len();
    let ext = extension_lower(&resolved.canonical);
    let mime_type = nexa_core::parse::detect_mime_type(&resolved.canonical);
    let display_name = resolved
        .canonical
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file")
        .to_string();
    let hash = hash_file(&resolved.canonical)?;

    let mut content: Option<String> = None;
    let mut encoding: Option<String> = None;
    let mut encoding_editable = false;
    let mut binary = false;
    let mut truncated = false;
    let mut warning: Option<String> = None;
    let mut structured_preview: Option<nexa_core::preview::StructuredPreview> = None;
    let rendered_preview: Option<RenderedPreview> = None;
    let mut capabilities = nexa_core::preview::PreviewCapabilities::default();

    if is_document_preview_type(&mime_type) && size_bytes <= PREVIEW_PARSE_BYTES_LIMIT {
        match nexa_core::parse::parse_file(
            &resolved.canonical,
            None,
            #[cfg(feature = "video")]
            None,
            None,
            None,
            Some(2000),
        ) {
            Ok(parsed) => {
                content = Some(flatten_parsed_document(&parsed));
                encoding = Some("extracted-text".to_string());
                capabilities.can_extract_text = true;
            }
            Err(err) => {
                warning = Some(format!("Could not extract document text: {err}"));
            }
        }
    } else if size_bytes <= PREVIEW_PARSE_BYTES_LIMIT {
        let read_limit = size_bytes.min(PREVIEW_TEXT_BYTES_LIMIT);
        let bytes = read_prefix(&resolved.canonical, read_limit)?;
        binary = bytes[..bytes.len().min(8192)].contains(&0);
        truncated = size_bytes > PREVIEW_TEXT_BYTES_LIMIT;
        if binary {
            warning = Some(
                "This looks like a binary file, so inline text preview is disabled.".to_string(),
            );
        } else {
            let (text, enc, editable_encoding) = decode_preview_text(&bytes);
            content = Some(text);
            encoding = Some(enc);
            encoding_editable = editable_encoding;
            capabilities.can_extract_text = true;
            if truncated {
                warning = Some(format!(
                    "Preview is truncated to {} MB. Large files are read-only here.",
                    PREVIEW_TEXT_BYTES_LIMIT / 1024 / 1024
                ));
            } else if !encoding_editable {
                warning = Some(
                    "Preview decoded a non-UTF-8 encoding; edit externally to preserve encoding."
                        .to_string(),
                );
            }
        }
    } else {
        warning = Some(format!(
            "File is larger than {} MB, so inline preview is disabled.",
            PREVIEW_PARSE_BYTES_LIMIT / 1024 / 1024
        ));
    }

    if size_bytes <= PREVIEW_PARSE_BYTES_LIMIT {
        let options = nexa_core::preview::PreviewBuildOptions {
            asset_cache_dir: app_data_dir
                .map(|data_dir| data_dir.join("preview-cache").join("assets")),
        };
        match nexa_core::preview::build_structured_preview(
            &resolved.canonical,
            &mime_type,
            &hash,
            &options,
        ) {
            Ok(Some(structured)) => {
                structured_preview = Some(structured);
                capabilities.can_render_structured = true;
            }
            Ok(None) => {}
            Err(err) => {
                capabilities.structured_unavailable_reason = Some(err.clone());
                append_preview_warning(
                    &mut warning,
                    format!("Structured preview unavailable: {err}"),
                );
            }
        }
    }

    let kind = preview_kind(&ext, &mime_type, binary);
    let line_count = content
        .as_deref()
        .map(|text| {
            text.lines()
                .count()
                .max(if text.is_empty() { 0 } else { 1 })
        })
        .unwrap_or(0);
    let editable = content.is_some()
        && !truncated
        && encoding_editable
        && is_editable_extension(&ext)
        && kind != "document"
        && kind != "binary";

    Ok(FilePreview {
        path: resolved.canonical.to_string_lossy().to_string(),
        display_name,
        source_id: resolved.source.id.clone(),
        source_name: source_display_name(&resolved.source),
        extension: ext.clone(),
        mime_type,
        kind,
        language: language_for_extension(&ext).map(str::to_string),
        content,
        encoding,
        editable,
        size_bytes,
        modified_at: modified_at_iso(&resolved.canonical),
        hash,
        line_count,
        truncated,
        warning,
        structured_preview,
        rendered_preview,
        capabilities,
    })
}

#[tauri::command]
pub async fn preview_file_cmd(
    state: tauri::State<'_, AppState>,
    app_handle: AppHandle,
    path: String,
) -> Result<FilePreview, String> {
    let db = state.db.clone();
    let data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data directory: {e}"))?;
    tokio::task::spawn_blocking(move || build_file_preview(&db, &path, Some(&data_dir)))
        .await
        .map_err(|e| e.to_string())?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveTextFileInput {
    path: String,
    content: String,
    expected_hash: Option<String>,
}

#[tauri::command]
pub async fn save_text_file_cmd(
    state: tauri::State<'_, AppState>,
    input: SaveTextFileInput,
) -> Result<FileSaveResult, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let resolved = resolve_source_file(&db, &input.path)?;
        let ext = extension_lower(&resolved.canonical);
        if !is_editable_extension(&ext) {
            return Err(format!(
                "Inline editing is not enabled for '{}' files.",
                ext.trim_start_matches('.')
            ));
        }

        let current_bytes = std::fs::read(&resolved.canonical).map_err(|e| e.to_string())?;
        if current_bytes[..current_bytes.len().min(8192)].contains(&0) {
            return Err("This file appears to be binary and cannot be edited inline.".to_string());
        }
        let current_hash = blake3::hash(&current_bytes).to_hex().to_string();
        if let Some(expected) = input.expected_hash.as_deref() {
            if expected != current_hash {
                return Err(
                    "File changed on disk after preview opened. Reload it before saving."
                        .to_string(),
                );
            }
        }

        let has_utf8_bom = current_bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
        let current_text_bytes = if has_utf8_bom {
            &current_bytes[3..]
        } else {
            &current_bytes[..]
        };
        if std::str::from_utf8(current_text_bytes).is_err() {
            return Err(
                "Inline editing only supports UTF-8 text files to avoid corrupting file encoding."
                    .to_string(),
            );
        }

        let checkpoint = db
            .create_file_checkpoint(nexa_core::file_checkpoint::CreateFileCheckpointInput {
                conversation_id: None,
                tool_call_id: &format!("ui-file-preview-{}", Uuid::new_v4()),
                tool_name: "ui_file_editor",
                operation: "manual_save",
                path: &input.path,
                absolute_path: &resolved.canonical,
            })
            .map_err(|e| e.to_string())?;

        let mut next_bytes = Vec::new();
        if has_utf8_bom {
            next_bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        next_bytes.extend_from_slice(input.content.as_bytes());
        std::fs::write(&resolved.canonical, &next_bytes).map_err(|e| e.to_string())?;

        let (reindex_status, reindex_detail) =
            match ingest::ingest_single_file(&db, &resolved.source.id, &resolved.canonical) {
                Ok(result) => ("ok".to_string(), Some(format!("{result:?}"))),
                Err(err) => ("error".to_string(), Some(err.to_string())),
            };

        let preview = build_file_preview(&db, &resolved.canonical.to_string_lossy(), None)?;
        Ok(FileSaveResult {
            preview,
            checkpoint_id: checkpoint.id,
            bytes_written: next_bytes.len() as u64,
            reindex_status,
            reindex_detail,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}
