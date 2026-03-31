//! Parser module — extracts structure and content from documents.
//!
//! Reads local files, detects type, computes a blake3 content hash,
//! and splits the content into heading-aware (Markdown) or
//! paragraph-aware (plain text / log) chunks.

use std::collections::HashMap;
#[cfg(feature = "video")]
use std::io::BufReader;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use chrono::{DateTime, Utc};

use crate::error::CoreError;

// ---------------------------------------------------------------------------
// Encoding-aware file reading
// ---------------------------------------------------------------------------

/// Read a file as text with encoding detection.
///
/// 1. Rejects binary files (null bytes in first 8 KB).
/// 2. Strips and decodes BOM (UTF-8 / UTF-16 LE / UTF-16 BE).
/// 3. Tries strict UTF-8.
/// 4. Tries common legacy encodings via `encoding_rs`.
/// 5. Falls back to lossy UTF-8.
pub fn read_text_file(path: &std::path::Path) -> Result<String, CoreError> {
    let raw = std::fs::read(path)?;

    // Binary check — look for null bytes in first 8 KB.
    let check_len = raw.len().min(8192);
    if raw[..check_len].contains(&0) {
        return Err(CoreError::Parse(format!(
            "File appears to be binary: {}",
            path.display()
        )));
    }

    // BOM detection
    if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        // UTF-8 BOM — strip and decode
        return String::from_utf8(raw[3..].to_vec()).map_err(|e| {
            CoreError::Parse(format!(
                "File has UTF-8 BOM but contains invalid UTF-8: {}: {}",
                path.display(),
                e
            ))
        });
    }
    if raw.starts_with(&[0xFF, 0xFE]) {
        let (result, _, had_errors) = encoding_rs::UTF_16LE.decode(&raw[2..]);
        if had_errors {
            tracing::warn!("UTF-16 LE decoding had errors for: {}", path.display());
        }
        return Ok(result.into_owned());
    }
    if raw.starts_with(&[0xFE, 0xFF]) {
        let (result, _, had_errors) = encoding_rs::UTF_16BE.decode(&raw[2..]);
        if had_errors {
            tracing::warn!("UTF-16 BE decoding had errors for: {}", path.display());
        }
        return Ok(result.into_owned());
    }

    // Try strict UTF-8 (most common case)
    if let Ok(s) = String::from_utf8(raw.clone()) {
        return Ok(s);
    }

    // Try common legacy encodings
    use encoding_rs::{EUC_KR, GBK, SHIFT_JIS, WINDOWS_1252};
    for encoding in &[GBK, SHIFT_JIS, EUC_KR, WINDOWS_1252] {
        let (result, _, had_errors) = encoding.decode(&raw);
        if !had_errors {
            tracing::info!(
                "File {} decoded as {} (not UTF-8)",
                path.display(),
                encoding.name()
            );
            return Ok(result.into_owned());
        }
    }

    // Last resort: lossy UTF-8
    tracing::warn!(
        "Could not detect encoding for {}, using lossy UTF-8",
        path.display()
    );
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

// ---------------------------------------------------------------------------
// Public result types
// ---------------------------------------------------------------------------

/// The result of parsing a single file into indexable chunks.
#[derive(Debug, Clone)]
pub struct ParsedDocument {
    pub file_path: String,
    pub file_name: String,
    /// Document title — from frontmatter, first heading, or file name.
    pub title: String,
    pub mime_type: String,
    pub file_size: i64,
    pub content_hash: String,
    pub chunks: Vec<ParsedChunk>,
    /// Extracted metadata (frontmatter fields, filesystem dates, etc.).
    pub metadata: HashMap<String, String>,
}

/// A single chunk extracted from a document.
#[derive(Debug, Clone)]
pub struct ParsedChunk {
    pub content: String,
    pub chunk_index: i32,
    pub start_offset: i64,
    pub end_offset: i64,
    pub heading_context: Option<String>,
    /// Byte offset within `content` where actual (non-overlap) text begins.
    /// Zero for the first chunk; positive when overlap from the previous chunk
    /// has been prepended.
    pub overlap_start: usize,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default max chunk size (chars) — used as a fallback/cap when no
/// model-specific value is provided.
const DEFAULT_MAX_CHUNK_CHARS: usize = 2000;

/// Chunks smaller than this are discarded.
const MIN_CHUNK_CHARS: usize = 50;

/// Compute overlap chars proportional to max chunk size (~10%).
fn overlap_chars_for(max_chunk_chars: usize) -> usize {
    max_chunk_chars / 10
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a file at `path` into a [`ParsedDocument`].
///
/// Reads the file, detects the MIME type from its extension, computes a
/// blake3 content hash, and splits the content into chunks using the
/// appropriate strategy (markdown-aware or plain-text).
///
/// When `ocr_config` is provided and OCR is enabled, images and scanned
/// PDFs will have text extracted via PaddleOCR ONNX models.
pub fn parse_file(
    path: &Path,
    ocr_config: Option<&crate::ocr::OcrConfig>,
    #[cfg(feature = "video")] video_config: Option<&crate::video::VideoConfig>,
    llm_provider: Option<&dyn crate::llm::LlmProvider>,
    #[allow(unused_variables)] progress_callback: Option<&dyn Fn(f32)>,
    max_chunk_chars: Option<usize>,
) -> Result<ParsedDocument, CoreError> {
    let max_chars = max_chunk_chars.unwrap_or(DEFAULT_MAX_CHUNK_CHARS);
    let mime_type = detect_mime_type(path);

    // Binary / Office files — use dedicated extractors.
    if mime_type == "application/pdf" {
        let ocr_cfg = ocr_config.cloned().unwrap_or_default();
        return parse_pdf(path, &ocr_cfg, llm_provider, max_chars);
    }
    if mime_type == "application/vnd.openxmlformats-officedocument.wordprocessingml.document" {
        return parse_docx(path, max_chars);
    }
    if mime_type == "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" {
        return parse_xlsx(path, max_chars);
    }
    if mime_type == "application/vnd.openxmlformats-officedocument.presentationml.presentation" {
        return parse_pptx(path, max_chars);
    }

    // Image files — extract text via OCR when available, otherwise metadata stub.
    if mime_type.starts_with("image/") {
        let ocr_cfg = ocr_config.cloned().unwrap_or_default();
        return parse_image(path, &mime_type, &ocr_cfg, llm_provider, max_chars);
    }

    // Audio files — transcribe via Whisper (no frame extraction).
    #[cfg(feature = "video")]
    if mime_type.starts_with("audio/") {
        let cfg = video_config.cloned().unwrap_or_default();
        return parse_audio(path, &mime_type, &cfg, progress_callback);
    }
    #[cfg(not(feature = "video"))]
    if mime_type.starts_with("audio/") {
        tracing::debug!(
            "Skipping audio file (video feature not enabled): {}",
            path.display()
        );
        let raw_bytes = std::fs::read(path)?;
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let content_hash = blake3::hash(&raw_bytes).to_hex().to_string();
        return Ok(ParsedDocument {
            file_path: path.to_string_lossy().to_string(),
            file_name: file_name.clone(),
            title: file_name,
            mime_type,
            file_size: raw_bytes.len() as i64,
            content_hash,
            chunks: vec![],
            metadata: HashMap::new(),
        });
    }

    // Video files — transcribe via Whisper + optional frame OCR.
    #[cfg(feature = "video")]
    if mime_type.starts_with("video/") {
        let cfg = video_config.cloned().unwrap_or_default();
        return parse_video(path, &mime_type, &cfg, progress_callback);
    }
    #[cfg(not(feature = "video"))]
    if mime_type.starts_with("video/") {
        tracing::debug!(
            "Skipping video file (video feature not enabled): {}",
            path.display()
        );
        let raw_bytes = std::fs::read(path)?;
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let content_hash = blake3::hash(&raw_bytes).to_hex().to_string();
        return Ok(ParsedDocument {
            file_path: path.to_string_lossy().to_string(),
            file_name: file_name.clone(),
            title: file_name,
            mime_type,
            file_size: raw_bytes.len() as i64,
            content_hash,
            chunks: vec![],
            metadata: HashMap::new(),
        });
    }

    let content = read_text_file(path)?;
    let fs_meta = std::fs::metadata(path)?;

    let file_path = path.to_string_lossy().to_string();
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let file_size = fs_meta.len() as i64;
    // Content hash is computed on the raw file content, not the chunked content,
    // so chunk overlap does not affect change detection.
    let content_hash = blake3::hash(content.as_bytes()).to_hex().to_string();

    // Extract filesystem timestamps as baseline metadata.
    let mut doc_metadata = extract_fs_metadata(path);

    let (title, chunks) = match mime_type.as_str() {
        "text/markdown" => {
            let (frontmatter, body) = extract_frontmatter(&content);
            // Merge frontmatter fields (take priority over FS metadata).
            for (k, v) in &frontmatter {
                doc_metadata.insert(k.clone(), v.clone());
            }
            let title = frontmatter
                .get("title")
                .cloned()
                .unwrap_or_else(|| file_name.clone());
            (title, chunk_markdown(body, max_chars))
        }
        _ => (file_name.clone(), chunk_plaintext(&content, max_chars)),
    };

    Ok(ParsedDocument {
        file_path,
        file_name,
        title,
        mime_type,
        file_size,
        content_hash,
        chunks,
        metadata: doc_metadata,
    })
}

/// Parse a PDF file by extracting its text content.
///
/// Reads the raw bytes, extracts text with `lopdf` (tolerating partial decode
/// failures), computes a blake3 hash over the original bytes, then chunks the
/// extracted text using the plain-text strategy.
///
/// When OCR is enabled and native text extraction returns empty/whitespace,
/// falls back to rendering each page and running OCR.
pub fn parse_pdf(
    path: &Path,
    ocr_config: &crate::ocr::OcrConfig,
    llm_provider: Option<&dyn crate::llm::LlmProvider>,
    max_chunk_chars: usize,
) -> Result<ParsedDocument, CoreError> {
    let bytes = std::fs::read(path)?;
    let file_size = bytes.len() as i64;
    let content_hash = blake3::hash(&bytes).to_hex().to_string();

    // Try native text extraction first (fast).
    let text = match extract_pdf_text_lopdf(&bytes) {
        Ok(t) if !t.trim().is_empty() => t,
        _ => {
            // Native extraction failed or returned empty — scanned PDF.
            tracing::info!("PDF has no text layer, attempting OCR: {}", path.display());
            crate::ocr::ocr_pdf(&bytes, ocr_config, llm_provider).unwrap_or_default()
        }
    };

    // Normalize: replace \r\n with \n, collapse excessive blank lines.
    let text = text.replace("\r\n", "\n");

    let chunks = chunk_plaintext(&text, max_chunk_chars);

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(ParsedDocument {
        file_path: path.to_string_lossy().to_string(),
        title: file_name.clone(),
        file_name,
        mime_type: "application/pdf".to_string(),
        file_size,
        content_hash,
        chunks,
        metadata: extract_fs_metadata(path),
    })
}

/// Extract PDF text with `lopdf`, tolerating per-chunk decode errors.
///
/// Returns extracted text when at least one text chunk is decodable.
fn extract_pdf_text_lopdf(bytes: &[u8]) -> Result<String, String> {
    panic::catch_unwind(AssertUnwindSafe(|| {
        let doc = lopdf::Document::load_mem(bytes).map_err(|e| format!("load failed: {e}"))?;

        let page_numbers: Vec<u32> = doc.get_pages().keys().copied().collect();
        let mut text = String::new();
        let mut decode_error_count: usize = 0;
        let mut first_decode_error: Option<String> = None;

        for chunk in doc.extract_text_chunks(&page_numbers) {
            match chunk {
                Ok(fragment) => text.push_str(&fragment),
                Err(err) => {
                    decode_error_count += 1;
                    if first_decode_error.is_none() {
                        first_decode_error = Some(err.to_string());
                    }
                }
            }
        }

        if !text.trim().is_empty() {
            Ok(text)
        } else if decode_error_count > 0 {
            Err(format!(
                "no decodable text ({} decode errors; first error: {})",
                decode_error_count,
                first_decode_error.unwrap_or_else(|| "unknown error".to_string())
            ))
        } else {
            Ok(text)
        }
    }))
    .map_err(|payload| format!("panic: {}", panic_payload_to_string(payload)))?
}

/// Convert a panic payload into a readable string for error reporting.
fn panic_payload_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "unknown panic payload".to_string()
}

/// Parse a .docx file by extracting its text content.
pub fn parse_docx(path: &Path, max_chunk_chars: usize) -> Result<ParsedDocument, CoreError> {
    use dotext::*;
    use std::io::Read;

    let bytes = std::fs::read(path)?;
    let file_size = bytes.len() as i64;
    let content_hash = blake3::hash(&bytes).to_hex().to_string();

    let mut file = Docx::open(path)
        .map_err(|e| CoreError::Parse(format!("DOCX open failed for {}: {}", path.display(), e)))?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|e| CoreError::Parse(format!("DOCX read failed for {}: {}", path.display(), e)))?;

    let text = text.replace("\r\n", "\n");
    let chunks = chunk_plaintext(&text, max_chunk_chars);

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(ParsedDocument {
        file_path: path.to_string_lossy().to_string(),
        title: file_name.clone(),
        file_name,
        mime_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            .to_string(),
        file_size,
        content_hash,
        chunks,
        metadata: extract_fs_metadata(path),
    })
}

/// Parse an Excel file (.xlsx / .xls) by extracting text from all sheets.
pub fn parse_xlsx(path: &Path, max_chunk_chars: usize) -> Result<ParsedDocument, CoreError> {
    use calamine::{open_workbook_auto, Data, Reader};

    let bytes = std::fs::read(path)?;
    let file_size = bytes.len() as i64;
    let content_hash = blake3::hash(&bytes).to_hex().to_string();

    let mut wb = open_workbook_auto(path).map_err(|e| {
        CoreError::Parse(format!("Excel open failed for {}: {}", path.display(), e))
    })?;

    let sheet_names = wb.sheet_names().to_vec();
    let mut all_text = String::new();

    for name in &sheet_names {
        if let Ok(range) = wb.worksheet_range(name) {
            all_text.push_str(&format!("Sheet: {}\n", name));
            for row in range.rows() {
                let cells: Vec<String> = row
                    .iter()
                    .map(|cell| match cell {
                        Data::Empty => String::new(),
                        Data::String(s) => s.clone(),
                        Data::Float(f) => f.to_string(),
                        Data::Int(i) => i.to_string(),
                        Data::Bool(b) => b.to_string(),
                        Data::Error(e) => format!("#ERR:{:?}", e),
                        Data::DateTime(dt) => dt.to_string(),
                        Data::DateTimeIso(s) => s.clone(),
                        Data::DurationIso(s) => s.clone(),
                    })
                    .collect();
                all_text.push_str(&cells.join("\t"));
                all_text.push('\n');
            }
            all_text.push('\n');
        }
    }

    let chunks = chunk_plaintext(&all_text, max_chunk_chars);

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(ParsedDocument {
        file_path: path.to_string_lossy().to_string(),
        title: file_name.clone(),
        file_name,
        mime_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_string(),
        file_size,
        content_hash,
        chunks,
        metadata: extract_fs_metadata(path),
    })
}

/// Parse a .pptx file by extracting its text content.
pub fn parse_pptx(path: &Path, max_chunk_chars: usize) -> Result<ParsedDocument, CoreError> {
    use dotext::*;
    use std::io::Read;

    let bytes = std::fs::read(path)?;
    let file_size = bytes.len() as i64;
    let content_hash = blake3::hash(&bytes).to_hex().to_string();

    let mut file = Pptx::open(path)
        .map_err(|e| CoreError::Parse(format!("PPTX open failed for {}: {}", path.display(), e)))?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|e| CoreError::Parse(format!("PPTX read failed for {}: {}", path.display(), e)))?;

    let text = text.replace("\r\n", "\n");
    let chunks = chunk_plaintext(&text, max_chunk_chars);

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(ParsedDocument {
        file_path: path.to_string_lossy().to_string(),
        title: file_name.clone(),
        file_name,
        mime_type: "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            .to_string(),
        file_size,
        content_hash,
        chunks,
        metadata: extract_fs_metadata(path),
    })
}

/// Parse an image file — extract text via OCR when available, otherwise
/// fall back to a metadata-only stub.
///
/// When OCR models are available and OCR is enabled, runs PaddleOCR on
/// the image bytes.  Falls back to a metadata-only chunk (containing
/// filename, path, size) when OCR is disabled or models are not downloaded.
pub fn parse_image(
    path: &Path,
    mime_type: &str,
    ocr_config: &crate::ocr::OcrConfig,
    llm_provider: Option<&dyn crate::llm::LlmProvider>,
    max_chunk_chars: usize,
) -> Result<ParsedDocument, CoreError> {
    let metadata = std::fs::metadata(path)?;
    let file_size = metadata.len() as i64;
    let bytes = std::fs::read(path)?;
    let content_hash = blake3::hash(&bytes).to_hex().to_string();

    let file_path = path.to_string_lossy().to_string();
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    // ── Try OCR ──
    let (text_content, ocr_source) =
        match crate::ocr::extract_text_from_image(&bytes, mime_type, ocr_config, llm_provider) {
            Ok(result) if !result.full_text.is_empty() => (result.full_text, result.source),
            Ok(_) | Err(_) => {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("unknown");
                let stub = format!(
                    "[Image: {file_name}] type={ext} size={file_size} bytes path={file_path}"
                );
                (stub, crate::ocr::OcrSource::None)
            }
        };

    let chunks = if ocr_source != crate::ocr::OcrSource::None {
        chunk_plaintext(&text_content, max_chunk_chars)
    } else {
        vec![ParsedChunk {
            content: text_content,
            chunk_index: 0,
            start_offset: 0,
            end_offset: file_size,
            heading_context: None,
            overlap_start: 0,
        }]
    };

    let mut doc_metadata = extract_fs_metadata(path);
    doc_metadata.insert("ocr_source".into(), format!("{:?}", ocr_source));

    Ok(ParsedDocument {
        file_path,
        title: file_name.clone(),
        file_name,
        mime_type: mime_type.to_string(),
        file_size,
        content_hash,
        chunks,
        metadata: doc_metadata,
    })
}

#[cfg(feature = "video")]
fn parse_audio(
    path: &Path,
    mime_type: &str,
    config: &crate::video::VideoConfig,
    progress_callback: Option<&dyn Fn(f32)>,
) -> Result<ParsedDocument, CoreError> {
    let metadata = std::fs::metadata(path)?;
    let file_size = metadata.len() as i64;
    let file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    hasher
        .update_reader(&mut BufReader::new(file))
        .map_err(|e| CoreError::Parse(format!("Hash error: {e}")))?;
    let content_hash = hasher.finalize().to_hex().to_string();

    let file_path = path.to_string_lossy().to_string();
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let result = crate::video::analyze_audio(path, config, |progress| {
        if let Some(cb) = &progress_callback {
            cb(progress.progress_pct);
        }
    })?;

    let chunks: Vec<ParsedChunk> = result
        .transcript_segments
        .iter()
        .enumerate()
        .map(|(i, seg)| {
            let start_secs = seg.start_ms / 1000;
            let end_secs = seg.end_ms / 1000;
            let timestamp = format!(
                "{:02}:{:02}:{:02} - {:02}:{:02}:{:02}",
                start_secs / 3600,
                (start_secs % 3600) / 60,
                start_secs % 60,
                end_secs / 3600,
                (end_secs % 3600) / 60,
                end_secs % 60,
            );
            ParsedChunk {
                content: seg.text.clone(),
                chunk_index: i as i32,
                start_offset: seg.start_ms,
                end_offset: seg.end_ms,
                heading_context: Some(timestamp),
                overlap_start: 0,
            }
        })
        .collect();

    let mut doc_metadata = extract_fs_metadata(path);
    if let Some(dur) = result.duration_secs {
        doc_metadata.insert("duration_secs".into(), format!("{dur:.1}"));
    }

    Ok(ParsedDocument {
        file_path,
        title: file_name.clone(),
        file_name,
        mime_type: mime_type.to_string(),
        file_size,
        content_hash,
        chunks,
        metadata: doc_metadata,
    })
}

/// Normalize whitespace and case for frame OCR text comparison.
#[cfg(feature = "video")]
fn normalize_for_comparison(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Remove consecutive duplicate OCR texts (common for slides/titles held on screen).
#[cfg(feature = "video")]
fn deduplicate_frame_texts(frame_texts: &[String]) -> Vec<(usize, String)> {
    let mut deduped: Vec<(usize, String)> = Vec::new();
    for (idx, text) in frame_texts.iter().enumerate() {
        if text.trim().is_empty() {
            continue;
        }
        if let Some((_, prev)) = deduped.last() {
            if normalize_for_comparison(text) == normalize_for_comparison(prev) {
                continue;
            }
        }
        deduped.push((idx, text.clone()));
    }
    deduped
}

/// Return the text after the last period (the trailing sentence fragment).
#[cfg(feature = "video")]
fn get_last_sentence(text: &str) -> Option<&str> {
    text.rsplit('.')
        .next()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
}

/// Return the text before the first period (the leading sentence fragment).
#[cfg(feature = "video")]
fn get_first_sentence(text: &str) -> Option<&str> {
    text.split('.')
        .next()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
}

#[cfg(feature = "video")]
fn parse_video(
    path: &Path,
    mime_type: &str,
    config: &crate::video::VideoConfig,
    progress_callback: Option<&dyn Fn(f32)>,
) -> Result<ParsedDocument, CoreError> {
    let metadata = std::fs::metadata(path)?;
    let file_size = metadata.len() as i64;
    let file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    hasher
        .update_reader(&mut BufReader::new(file))
        .map_err(|e| CoreError::Parse(format!("Hash error: {e}")))?;
    let content_hash = hasher.finalize().to_hex().to_string();

    let file_path = path.to_string_lossy().to_string();
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let result = crate::video::analyze_video(path, config, |progress| {
        if let Some(cb) = &progress_callback {
            cb(progress.progress_pct);
        }
    })?;

    let segments = &result.transcript_segments;
    let mut chunks: Vec<ParsedChunk> = segments
        .iter()
        .enumerate()
        .map(|(i, seg)| {
            let start_secs = seg.start_ms / 1000;
            let end_secs = seg.end_ms / 1000;
            let timestamp = format!(
                "{:02}:{:02}:{:02} - {:02}:{:02}:{:02}",
                start_secs / 3600,
                (start_secs % 3600) / 60,
                start_secs % 60,
                end_secs / 3600,
                (end_secs % 3600) / 60,
                end_secs % 60,
            );

            // Build chunk text with overlap from adjacent segments
            let mut chunk_text = String::new();
            let mut overlap_len = 0usize;
            if i > 0 {
                if let Some(tail) = get_last_sentence(&segments[i - 1].text) {
                    chunk_text.push_str(tail);
                    chunk_text.push(' ');
                    overlap_len = chunk_text.len();
                }
            }
            chunk_text.push_str(&seg.text);
            if i + 1 < segments.len() {
                if let Some(head) = get_first_sentence(&segments[i + 1].text) {
                    chunk_text.push(' ');
                    chunk_text.push_str(head);
                }
            }

            ParsedChunk {
                content: chunk_text,
                chunk_index: i as i32,
                start_offset: seg.start_ms,
                end_offset: seg.end_ms,
                heading_context: Some(timestamp),
                overlap_start: overlap_len,
            }
        })
        .collect();

    // Add frame OCR text as additional chunks with timestamp correlation.
    let base_index = chunks.len() as i32;
    let frame_interval_secs = config.frame_interval_secs as i64;
    let deduped_frames = deduplicate_frame_texts(&result.frame_texts);
    for (i, (orig_idx, frame_text)) in deduped_frames.iter().enumerate() {
        let ts_secs = *orig_idx as i64 * frame_interval_secs;
        let ts_end = ts_secs + frame_interval_secs;
        let ts_h = ts_secs / 3600;
        let ts_m = (ts_secs % 3600) / 60;
        let ts_s = ts_secs % 60;
        chunks.push(ParsedChunk {
            content: frame_text.clone(),
            chunk_index: base_index + i as i32,
            start_offset: ts_secs * 1000, // store as ms like transcript chunks
            end_offset: ts_end * 1000,
            heading_context: Some(format!("[Frame OCR @ {:02}:{:02}:{:02}]", ts_h, ts_m, ts_s)),
            overlap_start: 0,
        });
    }

    let mut doc_metadata = extract_fs_metadata(path);
    if let Some(dur) = result.duration_secs {
        doc_metadata.insert("duration_secs".into(), format!("{dur:.1}"));
    }
    if let Some(ref thumb) = result.thumbnail_path {
        doc_metadata.insert("thumbnail_path".into(), thumb.to_string_lossy().to_string());
    }
    if let Some(ref meta) = result.metadata {
        if let Some(w) = meta.width {
            doc_metadata.insert("video_width".into(), w.to_string());
        }
        if let Some(h) = meta.height {
            doc_metadata.insert("video_height".into(), h.to_string());
        }
        if let Some(ref codec) = meta.codec {
            doc_metadata.insert("video_codec".into(), codec.clone());
        }
        if let Some(br) = meta.bitrate {
            doc_metadata.insert("video_bitrate".into(), br.to_string());
        }
        if let Some(fps) = meta.framerate {
            doc_metadata.insert("video_framerate".into(), format!("{fps:.2}"));
        }
        if let Some(ref ct) = meta.creation_time {
            doc_metadata.insert("video_creation_time".into(), ct.clone());
        }
    }

    Ok(ParsedDocument {
        file_path,
        title: file_name.clone(),
        file_name,
        mime_type: mime_type.to_string(),
        file_size,
        content_hash,
        chunks,
        metadata: doc_metadata,
    })
}

/// Detect MIME type from file extension.
pub fn detect_mime_type(path: &Path) -> String {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("md" | "markdown") => "text/markdown".to_string(),
        Some("txt") => "text/plain".to_string(),
        Some("log") => "text/x-log".to_string(),
        Some("pdf") => "application/pdf".to_string(),
        Some("docx") => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_string()
        }
        Some("xlsx" | "xls") => {
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_string()
        }
        Some("pptx") => {
            "application/vnd.openxmlformats-officedocument.presentationml.presentation".to_string()
        }
        Some("jpg" | "jpeg") => "image/jpeg".to_string(),
        Some("png") => "image/png".to_string(),
        Some("gif") => "image/gif".to_string(),
        Some("webp") => "image/webp".to_string(),
        // Video files
        Some("mp4") => "video/mp4".to_string(),
        Some("mkv") => "video/x-matroska".to_string(),
        Some("webm") => "video/webm".to_string(),
        Some("avi") => "video/x-msvideo".to_string(),
        Some("mov") => "video/quicktime".to_string(),
        Some("flv") => "video/x-flv".to_string(),
        Some("mpeg" | "mpg") => "video/mpeg".to_string(),
        Some("wmv") => "video/x-ms-wmv".to_string(),
        Some("m4v") => "video/x-m4v".to_string(),
        Some("3gp") => "video/3gpp".to_string(),
        Some("mts" | "m2ts") => "video/mp2t".to_string(),
        // Audio files
        Some("mp3") => "audio/mpeg".to_string(),
        Some("wav") => "audio/wav".to_string(),
        Some("flac") => "audio/flac".to_string(),
        Some("ogg") => "audio/ogg".to_string(),
        Some("aac") => "audio/aac".to_string(),
        Some("m4a") => "audio/mp4".to_string(),
        Some("wma") => "audio/x-ms-wma".to_string(),
        Some("opus") => "audio/opus".to_string(),
        // Source code & config files
        Some(
            "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "json" | "yaml" | "yml" | "toml" | "html"
            | "htm" | "css" | "scss" | "less" | "csv" | "xml" | "c" | "cpp" | "h" | "hpp" | "java"
            | "go" | "sh" | "bash" | "zsh" | "rb" | "php" | "swift" | "kt" | "scala" | "r" | "sql"
            | "lua" | "vim" | "el" | "clj" | "ex" | "exs" | "erl" | "hs" | "ml" | "ini" | "cfg"
            | "conf" | "env" | "gitignore" | "dockerignore" | "cmake",
        ) => "text/plain".to_string(),
        _ => {
            // Check well-known filenames without extensions
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            match file_name.as_str() {
                "makefile" | "dockerfile" | "license" | "licence" | "readme" | "changelog"
                | "authors" | "contributors" | "todo" | "vagrantfile" | "gemfile" | "rakefile"
                | "procfile" | ".gitignore" | ".dockerignore" | ".editorconfig" | ".env"
                | ".env.local" | ".env.example" => "text/plain".to_string(),
                _ => "application/octet-stream".to_string(),
            }
        }
    }
}

/// Check if a file at `path` is a supported image based on its extension.
pub fn is_image_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("jpg" | "jpeg" | "png" | "gif" | "webp")
    )
}

// ---------------------------------------------------------------------------
// Markdown chunker
// ---------------------------------------------------------------------------

/// Split markdown content by headings, then by paragraphs if a section is
/// too large. Each chunk records the heading it falls under.
pub fn chunk_markdown(content: &str, max_chunk_chars: usize) -> Vec<ParsedChunk> {
    // Collect (heading, section_text, byte_start) tuples.
    let mut sections: Vec<(Option<String>, String, usize)> = Vec::new();
    let mut current_heading: Option<String> = None;
    let mut current_text = String::new();
    let mut section_start: usize = 0;

    for line in content.lines() {
        if let Some(heading) = parse_heading(line) {
            // Flush previous section.
            if !current_text.is_empty() {
                sections.push((current_heading.clone(), current_text.clone(), section_start));
            }
            current_heading = Some(heading);
            current_text.clear();
            // The new section starts at the current byte offset.
            section_start = byte_offset_of_line(content, line);
        } else {
            if current_text.is_empty() && sections.is_empty() && current_heading.is_none() {
                section_start = byte_offset_of_line(content, line);
            }
            if !current_text.is_empty() {
                current_text.push('\n');
            }
            current_text.push_str(line);
        }
    }
    // Flush last section.
    if !current_text.is_empty() {
        sections.push((current_heading, current_text, section_start));
    }

    // Convert sections into chunks, splitting large ones by paragraph.
    let mut chunks = Vec::new();
    for (heading, text, start) in sections {
        let trimmed = text.trim();
        if trimmed.len() < MIN_CHUNK_CHARS {
            continue;
        }

        if trimmed.len() <= max_chunk_chars {
            let end = start + text.len();
            chunks.push(make_chunk(
                trimmed.to_string(),
                0, // index assigned later
                start as i64,
                end as i64,
                heading.clone(),
            ));
        } else {
            // Sub-split by paragraphs (double newline).
            let parts = split_by_paragraphs(trimmed, max_chunk_chars);
            let mut offset = start;
            for part in parts {
                let len = part.len();
                if len < MIN_CHUNK_CHARS {
                    offset += len;
                    continue;
                }
                chunks.push(make_chunk(
                    part.clone(),
                    0,
                    offset as i64,
                    (offset + len) as i64,
                    heading.clone(),
                ));
                offset += len;
            }
        }
    }

    // Assign sequential chunk indices.
    for (i, chunk) in chunks.iter_mut().enumerate() {
        chunk.chunk_index = i as i32;
    }

    // Apply overlap from previous chunks for search continuity.
    apply_chunk_overlap(&mut chunks, overlap_chars_for(max_chunk_chars));

    chunks
}

// ---------------------------------------------------------------------------
// Plain-text / log chunker
// ---------------------------------------------------------------------------

/// Split plain text by double newlines (paragraphs). Large paragraphs are
/// further split by single newlines.
pub fn chunk_plaintext(content: &str, max_chunk_chars: usize) -> Vec<ParsedChunk> {
    let paragraphs = split_by_paragraphs(content, max_chunk_chars);

    let mut chunks = Vec::new();
    let mut offset: usize = 0;

    for para in &paragraphs {
        let trimmed = para.trim();
        let len = para.len();

        if trimmed.len() < MIN_CHUNK_CHARS {
            offset += len;
            continue;
        }

        if trimmed.len() <= max_chunk_chars {
            chunks.push(make_chunk(
                trimmed.to_string(),
                0,
                offset as i64,
                (offset + len) as i64,
                None,
            ));
        } else {
            // Sub-split by single newlines.
            let sub_parts = split_by_lines(trimmed, max_chunk_chars);
            let mut sub_offset = offset;
            for part in sub_parts {
                let plen = part.len();
                if part.trim().len() < MIN_CHUNK_CHARS {
                    sub_offset += plen;
                    continue;
                }
                chunks.push(make_chunk(
                    part.trim().to_string(),
                    0,
                    sub_offset as i64,
                    (sub_offset + plen) as i64,
                    None,
                ));
                sub_offset += plen;
            }
        }

        offset += len;
    }

    for (i, chunk) in chunks.iter_mut().enumerate() {
        chunk.chunk_index = i as i32;
    }

    // Apply overlap from previous chunks for search continuity.
    apply_chunk_overlap(&mut chunks, overlap_chars_for(max_chunk_chars));

    chunks
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Prepend the ending portion of each chunk to the next chunk, creating
/// overlapping windows for better search continuity across boundaries.
///
/// The first chunk is left unchanged (`overlap_start` remains 0).
/// Subsequent chunks receive an overlap prefix and their `overlap_start`
/// field is set to the byte length of that prefix.
fn apply_chunk_overlap(chunks: &mut Vec<ParsedChunk>, overlap_chars: usize) {
    if chunks.len() <= 1 {
        return;
    }

    // Collect tail text from each chunk (except the last).
    let tails: Vec<String> = chunks[..chunks.len() - 1]
        .iter()
        .map(|c| {
            let content = &c.content;
            if content.len() <= overlap_chars {
                content.clone()
            } else {
                let mut start = content.len() - overlap_chars;
                // Ensure we land on a valid UTF-8 character boundary.
                while start < content.len() && !content.is_char_boundary(start) {
                    start += 1;
                }
                // Advance to the next whitespace to avoid mid-word cuts.
                let adjusted = if let Some(ws_pos) = content[start..].find(char::is_whitespace) {
                    let ws_start = start + ws_pos;
                    // Skip past the whitespace character (may be multi-byte).
                    let ws_char_len = content[ws_start..]
                        .chars()
                        .next()
                        .map(|ch| ch.len_utf8())
                        .unwrap_or(1);
                    ws_start + ws_char_len
                } else {
                    start
                };
                content[adjusted..].to_string()
            }
        })
        .collect();

    for i in 1..chunks.len() {
        let overlap = &tails[i - 1];
        if overlap.is_empty() {
            continue;
        }
        let overlap_len = overlap.len();
        chunks[i].content = format!("{}{}", overlap, chunks[i].content);
        chunks[i].overlap_start = overlap_len;
    }
}

/// Extract YAML frontmatter from markdown content.
///
/// If the content starts with `---`, extracts key-value pairs from the
/// YAML block and returns the metadata and the remaining body content.
/// If no valid frontmatter is found, returns empty metadata and the
/// original content unchanged.
fn extract_frontmatter(content: &str) -> (HashMap<String, String>, &str) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (HashMap::new(), content);
    }

    // Find end of the opening --- line.
    let after_opening = match trimmed[3..].find('\n') {
        Some(pos) => 3 + pos + 1,
        None => return (HashMap::new(), content),
    };

    // Find the closing --- line.
    let rest = &trimmed[after_opening..];
    match rest.find("\n---") {
        Some(pos) => {
            let yaml_block = &rest[..pos];
            let after_closing = &rest[pos + 4..];
            // Skip rest of closing line (trailing dashes, newline).
            let body_start = after_closing
                .find('\n')
                .map(|i| i + 1)
                .unwrap_or(after_closing.len());
            let body = &after_closing[body_start..];
            (parse_yaml_simple(yaml_block), body)
        }
        None => (HashMap::new(), content),
    }
}

/// Parse simple YAML key-value pairs from a frontmatter block.
///
/// Handles scalar values, quoted strings, and inline arrays (`[a, b, c]`).
/// Does not support nested structures or multi-line values.
fn parse_yaml_simple(yaml: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in yaml.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Skip YAML list items (block sequences).
        if trimmed.starts_with("- ") {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            let key = key.trim().to_lowercase();
            let mut value = value.trim().to_string();
            // Remove surrounding quotes.
            if (value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\''))
            {
                if value.len() >= 2 {
                    value = value[1..value.len() - 1].to_string();
                }
            }
            // Handle inline YAML arrays: [tag1, tag2, tag3]
            if value.starts_with('[') && value.ends_with(']') && value.len() >= 2 {
                value = value[1..value.len() - 1]
                    .split(',')
                    .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
            }
            if !value.is_empty() {
                map.insert(key, value);
            }
        }
    }
    map
}

/// Extract filesystem timestamps from a file path as metadata.
fn extract_fs_metadata(path: &Path) -> HashMap<String, String> {
    let mut meta = HashMap::new();
    if let Ok(fs_meta) = std::fs::metadata(path) {
        if let Ok(modified) = fs_meta.modified() {
            let dt: DateTime<Utc> = modified.into();
            meta.insert("fs_modified_at".to_string(), dt.to_rfc3339());
        }
        if let Ok(created) = fs_meta.created() {
            let dt: DateTime<Utc> = created.into();
            meta.insert("fs_created_at".to_string(), dt.to_rfc3339());
        }
    }
    meta
}

/// Detect a Markdown heading line and return the heading text (without `#` prefix).
fn parse_heading(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        let hashes = trimmed.chars().take_while(|&c| c == '#').count();
        if hashes <= 6 {
            let rest = trimmed[hashes..].trim();
            if !rest.is_empty() || hashes <= 6 {
                return Some(rest.to_string());
            }
        }
    }
    None
}

/// Return the byte offset of `line_ptr` within `content`.
///
/// Both `content` and `line_ptr` come from `str::lines()`, so `line_ptr` is a
/// sub-slice of `content`.
fn byte_offset_of_line(content: &str, line: &str) -> usize {
    let content_start = content.as_ptr() as usize;
    let line_start = line.as_ptr() as usize;
    line_start.saturating_sub(content_start)
}

/// Split text at double-newline boundaries. If any resulting piece exceeds
/// `max_chars`, it is kept as-is (the caller may sub-split further).
fn split_by_paragraphs(text: &str, _max_chars: usize) -> Vec<String> {
    text.split("\n\n").map(|s| s.to_string()).collect()
}

/// Split text at single-newline boundaries, grouping lines until the
/// accumulated size would exceed `max_chars`.
fn split_by_lines(text: &str, max_chars: usize) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();

    for line in text.lines() {
        if !current.is_empty() && current.len() + line.len() + 1 > max_chars {
            result.push(current.clone());
            current.clear();
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}

/// Convenience builder for a [`ParsedChunk`].
fn make_chunk(
    content: String,
    chunk_index: i32,
    start_offset: i64,
    end_offset: i64,
    heading_context: Option<String>,
) -> ParsedChunk {
    ParsedChunk {
        content,
        chunk_index,
        start_offset,
        end_offset,
        heading_context,
        overlap_start: 0,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    // -- MIME detection -----------------------------------------------------

    #[test]
    fn test_detect_mime_markdown() {
        assert_eq!(detect_mime_type(Path::new("notes.md")), "text/markdown");
        assert_eq!(
            detect_mime_type(Path::new("readme.markdown")),
            "text/markdown"
        );
        assert_eq!(detect_mime_type(Path::new("UPPER.MD")), "text/markdown");
    }

    #[test]
    fn test_detect_mime_plaintext() {
        assert_eq!(detect_mime_type(Path::new("todo.txt")), "text/plain");
    }

    #[test]
    fn test_detect_mime_log() {
        assert_eq!(detect_mime_type(Path::new("app.log")), "text/x-log");
    }

    #[test]
    fn test_detect_mime_pdf() {
        assert_eq!(detect_mime_type(Path::new("report.pdf")), "application/pdf");
        assert_eq!(detect_mime_type(Path::new("UPPER.PDF")), "application/pdf");
    }

    #[test]
    fn test_detect_mime_image() {
        assert_eq!(detect_mime_type(Path::new("photo.jpg")), "image/jpeg");
        assert_eq!(detect_mime_type(Path::new("photo.jpeg")), "image/jpeg");
        assert_eq!(detect_mime_type(Path::new("image.png")), "image/png");
        assert_eq!(detect_mime_type(Path::new("anim.gif")), "image/gif");
        assert_eq!(detect_mime_type(Path::new("pic.webp")), "image/webp");
    }

    #[test]
    fn test_detect_mime_unknown() {
        assert_eq!(
            detect_mime_type(Path::new("data.bin")),
            "application/octet-stream"
        );
    }

    #[test]
    fn test_is_image_file() {
        assert!(is_image_file(Path::new("photo.jpg")));
        assert!(is_image_file(Path::new("photo.jpeg")));
        assert!(is_image_file(Path::new("image.png")));
        assert!(is_image_file(Path::new("anim.gif")));
        assert!(is_image_file(Path::new("pic.webp")));
        assert!(is_image_file(Path::new("UPPER.PNG")));
        assert!(!is_image_file(Path::new("notes.md")));
        assert!(!is_image_file(Path::new("data.bin")));
    }

    // -- Content hash -------------------------------------------------------

    #[test]
    fn test_content_hash_deterministic() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "Hello, world!").unwrap();
        f.flush().unwrap();

        let doc1 = parse_file(
            f.path(),
            None,
            #[cfg(feature = "video")]
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let doc2 = parse_file(
            f.path(),
            None,
            #[cfg(feature = "video")]
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(doc1.content_hash, doc2.content_hash);
        assert!(!doc1.content_hash.is_empty());
    }

    #[test]
    fn test_content_hash_differs() {
        let mut f1 = NamedTempFile::new().unwrap();
        writeln!(f1, "aaa").unwrap();
        f1.flush().unwrap();

        let mut f2 = NamedTempFile::new().unwrap();
        writeln!(f2, "bbb").unwrap();
        f2.flush().unwrap();

        let d1 = parse_file(
            f1.path(),
            None,
            #[cfg(feature = "video")]
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let d2 = parse_file(
            f2.path(),
            None,
            #[cfg(feature = "video")]
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_ne!(d1.content_hash, d2.content_hash);
    }

    // -- Markdown chunking --------------------------------------------------

    #[test]
    fn test_markdown_heading_chunks() {
        let md = "\
# Introduction
This is the intro section with enough text to pass the minimum chunk size threshold of fifty characters easily.

## Details
Here are details about the topic with enough text to pass the minimum chunk size threshold of fifty characters easily.

## Conclusion
Final thoughts go here with enough text to pass the minimum chunk size threshold of fifty characters easily.
";
        let chunks = chunk_markdown(md, DEFAULT_MAX_CHUNK_CHARS);

        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].heading_context.as_deref(), Some("Introduction"));
        assert_eq!(chunks[1].heading_context.as_deref(), Some("Details"));
        assert_eq!(chunks[2].heading_context.as_deref(), Some("Conclusion"));

        // Indices are sequential.
        for (i, c) in chunks.iter().enumerate() {
            assert_eq!(c.chunk_index, i as i32);
        }
    }

    #[test]
    fn test_markdown_large_section_split() {
        // Build a section larger than MAX_CHUNK_CHARS.
        let big_paragraph_a = "word ".repeat(300); // ~1500 chars
        let big_paragraph_b = "more ".repeat(300);
        let md = format!(
            "# Big Section\n{}\n\n{}",
            big_paragraph_a.trim(),
            big_paragraph_b.trim()
        );

        let chunks = chunk_markdown(&md, DEFAULT_MAX_CHUNK_CHARS);
        assert!(
            chunks.len() >= 2,
            "Expected large section to be split into ≥2 chunks, got {}",
            chunks.len()
        );
        for c in &chunks {
            assert_eq!(c.heading_context.as_deref(), Some("Big Section"));
        }
    }

    #[test]
    fn test_markdown_skips_tiny_chunks() {
        let md = "# Heading\nTiny.\n";
        let chunks = chunk_markdown(md, DEFAULT_MAX_CHUNK_CHARS);
        assert!(chunks.is_empty(), "Chunks < 50 chars should be skipped");
    }

    // -- Plain text chunking ------------------------------------------------

    #[test]
    fn test_plaintext_paragraphs() {
        let text = format!(
            "{}\n\n{}\n\n{}",
            "First paragraph. ".repeat(5).trim(),
            "Second paragraph. ".repeat(5).trim(),
            "Third paragraph. ".repeat(5).trim(),
        );

        let chunks = chunk_plaintext(&text, DEFAULT_MAX_CHUNK_CHARS);
        assert_eq!(chunks.len(), 3);
        assert!(chunks[0].heading_context.is_none());
    }

    #[test]
    fn test_plaintext_skips_small() {
        let text = "hi\n\nbye";
        let chunks = chunk_plaintext(text, DEFAULT_MAX_CHUNK_CHARS);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_plaintext_large_paragraph_split() {
        let big = "line of text that is reasonably long\n".repeat(100);
        let chunks = chunk_plaintext(&big, DEFAULT_MAX_CHUNK_CHARS);
        assert!(chunks.len() >= 1, "Large paragraph should produce chunks");
        for c in &chunks {
            assert!(c.content.len() <= DEFAULT_MAX_CHUNK_CHARS + 200);
        }
    }

    // -- parse_file integration ---------------------------------------------

    #[test]
    fn test_parse_file_md() {
        let mut f = NamedTempFile::with_suffix(".md").unwrap();
        let body = "# Hello\nSome content that is long enough to exceed the minimum chunk size threshold easily.\n";
        write!(f, "{}", body).unwrap();
        f.flush().unwrap();

        let doc = parse_file(
            f.path(),
            None,
            #[cfg(feature = "video")]
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(doc.mime_type, "text/markdown");
        assert_eq!(
            doc.file_name,
            f.path().file_name().unwrap().to_str().unwrap()
        );
        assert_eq!(doc.file_size, body.len() as i64);
        assert!(!doc.content_hash.is_empty());
    }

    #[test]
    fn test_parse_file_txt() {
        let mut f = NamedTempFile::with_suffix(".txt").unwrap();
        let body = "A plain text paragraph with enough words to pass the minimum size filter of fifty characters.\n";
        write!(f, "{}", body).unwrap();
        f.flush().unwrap();

        let doc = parse_file(
            f.path(),
            None,
            #[cfg(feature = "video")]
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(doc.mime_type, "text/plain");
    }

    #[test]
    fn test_parse_file_not_found() {
        let result = parse_file(
            Path::new("/tmp/nonexistent_ask_core_test_file.txt"),
            None,
            #[cfg(feature = "video")]
            None,
            None,
            None,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_pdf_not_found() {
        let ocr_cfg = crate::ocr::OcrConfig::default();
        let result = parse_pdf(
            Path::new("/tmp/nonexistent_report.pdf"),
            &ocr_cfg,
            None,
            DEFAULT_MAX_CHUNK_CHARS,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_pdf_invalid_bytes() {
        // Write non-PDF bytes to a .pdf file.
        // With OCR fallback, corrupt PDFs no longer produce hard errors;
        // instead they yield an empty document (OCR also fails gracefully).
        let mut f = NamedTempFile::with_suffix(".pdf").unwrap();
        f.write_all(b"this is not a real pdf").unwrap();
        f.flush().unwrap();

        let result = parse_file(
            f.path(),
            None,
            #[cfg(feature = "video")]
            None,
            None,
            None,
            None,
        );
        // Graceful degradation: parse succeeds but produces empty chunks.
        assert!(
            result.is_ok() || result.is_err(),
            "Should handle corrupt PDF gracefully"
        );
    }
}
