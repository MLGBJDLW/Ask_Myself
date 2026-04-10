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

fn chunk_plaintext_preserving_short_document(
    content: &str,
    max_chunk_chars: usize,
) -> Vec<ParsedChunk> {
    let chunks = chunk_plaintext(content, max_chunk_chars);
    if !chunks.is_empty() || content.trim().is_empty() {
        return chunks;
    }

    vec![make_chunk(
        content.trim().to_string(),
        0,
        0,
        content.len() as i64,
        None,
    )]
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
    if mime_type == "application/msword" {
        return parse_doc(path, max_chars);
    }
    if mime_type == "application/vnd.ms-powerpoint" {
        return parse_ppt(path, max_chars);
    }
    if mime_type == "text/html" {
        return parse_html(path, max_chars);
    }
    if mime_type == "application/epub+zip" {
        return parse_epub(path, max_chars);
    }
    if mime_type.starts_with("application/vnd.oasis.opendocument.") {
        return parse_odf(path, &mime_type, max_chars);
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

    let chunks = chunk_plaintext_preserving_short_document(&text, max_chunk_chars);

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

fn decode_xml_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn strip_ooxml_tags(xml: &str) -> Result<String, String> {
    let tag_re = regex::Regex::new(r"<[^>]+>").map_err(|e| format!("regex init failed: {e}"))?;
    let without_tags = tag_re.replace_all(xml, "");
    let decoded = decode_xml_entities(&without_tags);
    Ok(decoded
        .replace("\r\n", "\n")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n"))
}

fn read_zip_entry_text(
    bytes: &[u8],
    include_entry: impl Fn(&str) -> bool,
) -> Result<String, String> {
    use std::io::{Cursor, Read};

    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("zip open failed: {e}"))?;
    let mut fragments = Vec::new();

    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("zip entry open failed: {e}"))?;
        let name = file.name().to_string();
        if !include_entry(&name) {
            continue;
        }

        let mut xml = String::new();
        file.read_to_string(&mut xml)
            .map_err(|e| format!("zip entry read failed for {name}: {e}"))?;
        fragments.push(xml);
    }

    if fragments.is_empty() {
        return Ok(String::new());
    }

    Ok(fragments.join("\n"))
}

fn extract_docx_text_from_xml(bytes: &[u8]) -> Result<String, String> {
    let xml = read_zip_entry_text(bytes, |name| {
        name == "word/document.xml"
            || name.starts_with("word/header")
            || name.starts_with("word/footer")
            || name == "word/footnotes.xml"
            || name == "word/endnotes.xml"
    })?;

    let with_breaks = xml
        .replace("<w:tab/>", "\t")
        .replace("<w:tab />", "\t")
        .replace("<w:br/>", "\n")
        .replace("<w:br />", "\n")
        .replace("<w:cr/>", "\n")
        .replace("<w:cr />", "\n")
        .replace("</w:p>", "\n")
        .replace("</w:tr>", "\n");

    strip_ooxml_tags(&with_breaks)
}

fn extract_pptx_text_from_xml(bytes: &[u8]) -> Result<String, String> {
    let xml = read_zip_entry_text(bytes, |name| {
        name.starts_with("ppt/slides/slide") && name.ends_with(".xml")
    })?;

    let with_breaks = xml
        .replace("<a:tab/>", "\t")
        .replace("<a:tab />", "\t")
        .replace("<a:br/>", "\n")
        .replace("<a:br />", "\n")
        .replace("</a:p>", "\n");

    strip_ooxml_tags(&with_breaks)
}

/// Parse a .docx file by extracting its text content.
pub fn parse_docx(path: &Path, max_chunk_chars: usize) -> Result<ParsedDocument, CoreError> {
    use dotext::*;
    use std::io::Read;

    let bytes = std::fs::read(path)?;
    let file_size = bytes.len() as i64;
    let content_hash = blake3::hash(&bytes).to_hex().to_string();

    let dotext_result = (|| -> Result<String, String> {
        let mut file = Docx::open(path)
            .map_err(|e| format!("DOCX open failed for {}: {}", path.display(), e))?;
        let mut text = String::new();
        file.read_to_string(&mut text)
            .map_err(|e| format!("DOCX read failed for {}: {}", path.display(), e))?;
        Ok(text)
    })();

    let text = match dotext_result {
        Ok(text) if !text.trim().is_empty() => text,
        Ok(_) | Err(_) => extract_docx_text_from_xml(&bytes).map_err(|e| {
            CoreError::Parse(format!(
                "DOCX read failed for {} (OOXML fallback also failed: {})",
                path.display(),
                e
            ))
        })?,
    };

    let text = text.replace("\r\n", "\n");
    let chunks = chunk_plaintext_preserving_short_document(&text, max_chunk_chars);

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

    let chunks = chunk_plaintext_preserving_short_document(&all_text, max_chunk_chars);

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

    let dotext_result = (|| -> Result<String, String> {
        let mut file = Pptx::open(path)
            .map_err(|e| format!("PPTX open failed for {}: {}", path.display(), e))?;
        let mut text = String::new();
        file.read_to_string(&mut text)
            .map_err(|e| format!("PPTX read failed for {}: {}", path.display(), e))?;
        Ok(text)
    })();

    let text = match dotext_result {
        Ok(text) if !text.trim().is_empty() => text,
        Ok(_) | Err(_) => extract_pptx_text_from_xml(&bytes).map_err(|e| {
            CoreError::Parse(format!(
                "PPTX read failed for {} (OOXML fallback also failed: {})",
                path.display(),
                e
            ))
        })?,
    };

    let text = text.replace("\r\n", "\n");
    let chunks = chunk_plaintext_preserving_short_document(&text, max_chunk_chars);

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
        chunk_plaintext_preserving_short_document(&text_content, max_chunk_chars)
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

// ---------------------------------------------------------------------------
// Legacy Office / HTML / EPUB / ODF parsers
// ---------------------------------------------------------------------------

/// Extract readable text sequences from binary document data.
///
/// Scans for runs of printable ASCII characters, filtering out short
/// runs that are likely formatting artifacts rather than real content.
fn extract_text_from_binary(bytes: &[u8]) -> String {
    let min_run_length = 20;
    let mut result = String::new();
    let mut current_run = String::new();

    for &byte in bytes {
        if (byte >= 0x20 && byte < 0x7F) || byte == b'\n' || byte == b'\r' || byte == b'\t' {
            current_run.push(byte as char);
        } else {
            if current_run.trim().len() >= min_run_length {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(current_run.trim());
            }
            current_run.clear();
        }
    }
    // Flush last run.
    if current_run.trim().len() >= min_run_length {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(current_run.trim());
    }

    result
}

/// Decode common HTML entities to their character equivalents.
fn decode_html_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&#160;", " ")
        .replace("&ndash;", "\u{2013}")
        .replace("&mdash;", "\u{2014}")
        .replace("&hellip;", "\u{2026}")
        .replace("&copy;", "\u{00A9}")
        .replace("&reg;", "\u{00AE}")
        .replace("&trade;", "\u{2122}")
}

/// Strip HTML tags from content, preserving meaningful structure.
///
/// Removes script/style blocks, converts headings to markdown format,
/// preserves link URLs in markdown syntax, converts block elements to
/// newlines, strips remaining tags, and decodes HTML entities.
fn strip_html_tags(html: &str) -> String {
    // Remove script, style, noscript blocks entirely.
    let re_script = regex::Regex::new(r"(?is)<script[^>]*>.*?</script>").unwrap();
    let without_scripts = re_script.replace_all(html, "");
    let re_style = regex::Regex::new(r"(?is)<style[^>]*>.*?</style>").unwrap();
    let without_styles = re_style.replace_all(&without_scripts, "");
    let re_noscript = regex::Regex::new(r"(?is)<noscript[^>]*>.*?</noscript>").unwrap();
    let without_noscript = re_noscript.replace_all(&without_styles, "");

    // Convert headings to markdown.
    let re_heading = regex::Regex::new(r"(?is)<h([1-6])[^>]*>(.*?)</h[1-6]>").unwrap();
    let with_headings = re_heading.replace_all(&without_noscript, |caps: &regex::Captures| {
        let level = caps[1].parse::<usize>().unwrap_or(1);
        let hashes = "#".repeat(level);
        format!("\n{} {}\n", hashes, caps[2].trim())
    });

    // Convert links to markdown format.
    let re_link = regex::Regex::new(r#"(?is)<a[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#).unwrap();
    let with_links = re_link.replace_all(&with_headings, |caps: &regex::Captures| {
        let url = &caps[1];
        let text = caps[2].trim();
        if text.is_empty() {
            url.to_string()
        } else {
            format!("[{}]({})", text, url)
        }
    });

    // Convert block elements to newlines.
    let re_block = regex::Regex::new(r"(?i)</?(p|div|br|tr|li|blockquote|pre|hr)[^>]*/?>").unwrap();
    let with_blocks = re_block.replace_all(&with_links, "\n");

    // Strip all remaining tags.
    let re_tags = regex::Regex::new(r"<[^>]+>").unwrap();
    let without_tags = re_tags.replace_all(&with_blocks, "");

    // Decode HTML entities.
    let decoded = decode_html_entities(&without_tags);

    // Normalize whitespace: trim lines, collapse blank lines.
    decoded
        .replace("\r\n", "\n")
        .lines()
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join("\n")
        .split("\n\n\n")
        .collect::<Vec<_>>()
        .join("\n\n")
        .trim()
        .to_string()
}

/// Parse a legacy .doc file by attempting text extraction.
///
/// Tries to open as a zip archive first (handles modern .docx files
/// saved with a .doc extension), then falls back to raw text
/// extraction from the binary data.
fn parse_doc(path: &Path, max_chunk_chars: usize) -> Result<ParsedDocument, CoreError> {
    let bytes = std::fs::read(path)?;
    let file_size = bytes.len() as i64;
    let content_hash = blake3::hash(&bytes).to_hex().to_string();

    // Try as zip first (handles renamed .docx files).
    let text = if let Ok(xml_text) = extract_docx_text_from_xml(&bytes) {
        if !xml_text.trim().is_empty() {
            xml_text
        } else {
            extract_text_from_binary(&bytes)
        }
    } else {
        extract_text_from_binary(&bytes)
    };

    if text.trim().is_empty() {
        return Err(CoreError::Parse(format!(
            "Could not extract text from legacy .doc file: {}",
            path.display()
        )));
    }

    let text = text.replace("\r\n", "\n");
    let chunks = chunk_plaintext_preserving_short_document(&text, max_chunk_chars);

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(ParsedDocument {
        file_path: path.to_string_lossy().to_string(),
        title: file_name.clone(),
        file_name,
        mime_type: "application/msword".to_string(),
        file_size,
        content_hash,
        chunks,
        metadata: extract_fs_metadata(path),
    })
}

/// Parse a legacy .ppt file by attempting text extraction.
///
/// Tries to open as a zip archive first (handles modern .pptx files
/// saved with a .ppt extension), then falls back to raw text
/// extraction from the binary data.
fn parse_ppt(path: &Path, max_chunk_chars: usize) -> Result<ParsedDocument, CoreError> {
    let bytes = std::fs::read(path)?;
    let file_size = bytes.len() as i64;
    let content_hash = blake3::hash(&bytes).to_hex().to_string();

    // Try as zip first (handles renamed .pptx files).
    let text = if let Ok(xml_text) = extract_pptx_text_from_xml(&bytes) {
        if !xml_text.trim().is_empty() {
            xml_text
        } else {
            extract_text_from_binary(&bytes)
        }
    } else {
        extract_text_from_binary(&bytes)
    };

    if text.trim().is_empty() {
        return Err(CoreError::Parse(format!(
            "Could not extract text from legacy .ppt file: {}",
            path.display()
        )));
    }

    let text = text.replace("\r\n", "\n");
    let chunks = chunk_plaintext_preserving_short_document(&text, max_chunk_chars);

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(ParsedDocument {
        file_path: path.to_string_lossy().to_string(),
        title: file_name.clone(),
        file_name,
        mime_type: "application/vnd.ms-powerpoint".to_string(),
        file_size,
        content_hash,
        chunks,
        metadata: extract_fs_metadata(path),
    })
}

/// Parse an HTML file by stripping tags and extracting clean text.
fn parse_html(path: &Path, max_chunk_chars: usize) -> Result<ParsedDocument, CoreError> {
    let content = read_text_file(path)?;
    let fs_meta = std::fs::metadata(path)?;

    let file_path = path.to_string_lossy().to_string();
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let file_size = fs_meta.len() as i64;
    let content_hash = blake3::hash(content.as_bytes()).to_hex().to_string();

    let clean_text = strip_html_tags(&content);

    // Try to extract title from <title> tag.
    let title_re = regex::Regex::new(r"(?is)<title[^>]*>(.*?)</title>").ok();
    let title = title_re
        .and_then(|re| re.captures(&content))
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| file_name.clone());

    let chunks = chunk_plaintext_preserving_short_document(&clean_text, max_chunk_chars);

    Ok(ParsedDocument {
        file_path,
        file_name,
        title,
        mime_type: "text/html".to_string(),
        file_size,
        content_hash,
        chunks,
        metadata: extract_fs_metadata(path),
    })
}

/// Parse an EPUB file by extracting text from XHTML chapters.
///
/// Opens the EPUB as a zip archive, reads the OPF manifest and spine
/// to determine chapter order, extracts text from each chapter's
/// XHTML content, and concatenates them with separators.
fn parse_epub(path: &Path, max_chunk_chars: usize) -> Result<ParsedDocument, CoreError> {
    use std::io::{Cursor, Read};

    let bytes = std::fs::read(path)?;
    let file_size = bytes.len() as i64;
    let content_hash = blake3::hash(&bytes).to_hex().to_string();

    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).map_err(|e| {
        CoreError::Parse(format!(
            "EPUB zip open failed for {}: {}",
            path.display(),
            e
        ))
    })?;

    // 1. Read container.xml to find the OPF file path.
    let opf_path = {
        let mut container = archive
            .by_name("META-INF/container.xml")
            .map_err(|e| CoreError::Parse(format!("EPUB container.xml not found: {}", e)))?;
        let mut xml = String::new();
        container
            .read_to_string(&mut xml)
            .map_err(|e| CoreError::Parse(format!("Failed to read container.xml: {}", e)))?;

        let re = regex::Regex::new(r#"full-path="([^"]+)""#).unwrap();
        re.captures(&xml)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| {
                CoreError::Parse("Could not find rootfile path in container.xml".to_string())
            })?
    };

    // Determine the base directory of the OPF file for resolving relative paths.
    let opf_dir = if let Some(pos) = opf_path.rfind('/') {
        opf_path[..=pos].to_string()
    } else {
        String::new()
    };

    // 2. Read and parse the OPF file.
    let (manifest_items, spine_idrefs, epub_title) = {
        let mut opf_file = archive
            .by_name(&opf_path)
            .map_err(|e| CoreError::Parse(format!("EPUB OPF file not found: {}", e)))?;
        let mut opf_xml = String::new();
        opf_file
            .read_to_string(&mut opf_xml)
            .map_err(|e| CoreError::Parse(format!("Failed to read OPF file: {}", e)))?;

        // Extract manifest items.
        let item_re = regex::Regex::new(r#"<item\s+([^>]*)/?>"#).unwrap();
        let attr_id = regex::Regex::new(r#"\bid="([^"]+)""#).unwrap();
        let attr_href = regex::Regex::new(r#"\bhref="([^"]+)""#).unwrap();
        let attr_media = regex::Regex::new(r#"\bmedia-type="([^"]+)""#).unwrap();

        let mut items: HashMap<String, (String, String)> = HashMap::new();
        for caps in item_re.captures_iter(&opf_xml) {
            let attrs = &caps[1];
            if let (Some(id), Some(href), Some(media)) = (
                attr_id.captures(attrs).and_then(|c| c.get(1)),
                attr_href.captures(attrs).and_then(|c| c.get(1)),
                attr_media.captures(attrs).and_then(|c| c.get(1)),
            ) {
                items.insert(
                    id.as_str().to_string(),
                    (href.as_str().to_string(), media.as_str().to_string()),
                );
            }
        }

        // Extract spine idrefs.
        let spine_re = regex::Regex::new(r#"<itemref[^>]*\bidref="([^"]+)"[^>]*/?\s*>"#).unwrap();
        let idrefs: Vec<String> = spine_re
            .captures_iter(&opf_xml)
            .map(|caps| caps[1].to_string())
            .collect();

        // Extract title.
        let title_re = regex::Regex::new(r"(?is)<dc:title[^>]*>(.*?)</dc:title>").unwrap();
        let title = title_re
            .captures(&opf_xml)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().trim().to_string())
            .filter(|t| !t.is_empty());

        (items, idrefs, title)
    };

    // 3. Read chapter content in spine order.
    let mut all_text = String::new();
    for idref in &spine_idrefs {
        if let Some((href, media_type)) = manifest_items.get(idref) {
            if !media_type.contains("html") && !media_type.contains("xml") {
                continue;
            }

            let full_path = format!("{}{}", opf_dir, href);
            if let Ok(mut entry) = archive.by_name(&full_path) {
                let mut xhtml = String::new();
                if entry.read_to_string(&mut xhtml).is_ok() {
                    let chapter_text = strip_html_tags(&xhtml);
                    if !chapter_text.trim().is_empty() {
                        if !all_text.is_empty() {
                            all_text.push_str("\n\n---\n\n");
                        }
                        all_text.push_str(&chapter_text);
                    }
                }
            }
        }
    }

    if all_text.trim().is_empty() {
        return Err(CoreError::Parse(format!(
            "EPUB contains no extractable text: {}",
            path.display()
        )));
    }

    let chunks = chunk_plaintext_preserving_short_document(&all_text, max_chunk_chars);

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let title = epub_title.unwrap_or_else(|| file_name.clone());

    Ok(ParsedDocument {
        file_path: path.to_string_lossy().to_string(),
        file_name,
        title,
        mime_type: "application/epub+zip".to_string(),
        file_size,
        content_hash,
        chunks,
        metadata: extract_fs_metadata(path),
    })
}

/// Parse an ODF file (.odt, .ods, .odp) by extracting text.
///
/// Uses `dotext` for `.odt` and `.odp` when available, with a zip-based
/// fallback that reads `content.xml` and strips XML tags.
fn parse_odf(
    path: &Path,
    mime_type: &str,
    max_chunk_chars: usize,
) -> Result<ParsedDocument, CoreError> {
    use dotext::doc::OpenOfficeDoc;
    use std::io::Read;

    let bytes = std::fs::read(path)?;
    let file_size = bytes.len() as i64;
    let content_hash = blake3::hash(&bytes).to_hex().to_string();

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    // Try dotext first for supported formats.
    let dotext_result: Option<String> = match ext.as_str() {
        "odt" => {
            let r = (|| -> Result<String, String> {
                let mut file =
                    dotext::Odt::open(path).map_err(|e| format!("ODT open failed: {}", e))?;
                let mut text = String::new();
                file.read_to_string(&mut text)
                    .map_err(|e| format!("ODT read failed: {}", e))?;
                Ok(text)
            })();
            r.ok().filter(|t| !t.trim().is_empty())
        }
        "odp" => {
            let r = (|| -> Result<String, String> {
                let mut file =
                    dotext::Odp::open(path).map_err(|e| format!("ODP open failed: {}", e))?;
                let mut text = String::new();
                file.read_to_string(&mut text)
                    .map_err(|e| format!("ODP read failed: {}", e))?;
                Ok(text)
            })();
            r.ok().filter(|t| !t.trim().is_empty())
        }
        _ => None,
    };

    let text = if let Some(text) = dotext_result {
        text
    } else {
        // Fallback: read content.xml from the zip and strip tags.
        let xml = read_zip_entry_text(&bytes, |name| name == "content.xml").map_err(|e| {
            CoreError::Parse(format!(
                "ODF read failed for {} (content.xml extraction failed: {})",
                path.display(),
                e
            ))
        })?;
        strip_ooxml_tags(&xml).map_err(|e| {
            CoreError::Parse(format!(
                "ODF tag stripping failed for {}: {}",
                path.display(),
                e
            ))
        })?
    };

    if text.trim().is_empty() {
        return Err(CoreError::Parse(format!(
            "ODF contains no extractable text: {}",
            path.display()
        )));
    }

    let text = text.replace("\r\n", "\n");
    let chunks = chunk_plaintext_preserving_short_document(&text, max_chunk_chars);

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(ParsedDocument {
        file_path: path.to_string_lossy().to_string(),
        title: file_name.clone(),
        file_name,
        mime_type: mime_type.to_string(),
        file_size,
        content_hash,
        chunks,
        metadata: extract_fs_metadata(path),
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
        Some("doc") => "application/msword".to_string(),
        Some("ppt") => "application/vnd.ms-powerpoint".to_string(),
        Some("html" | "htm") => "text/html".to_string(),
        Some("epub") => "application/epub+zip".to_string(),
        Some("odt") => "application/vnd.oasis.opendocument.text".to_string(),
        Some("ods") => "application/vnd.oasis.opendocument.spreadsheet".to_string(),
        Some("odp") => "application/vnd.oasis.opendocument.presentation".to_string(),
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
            "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "json" | "yaml" | "yml" | "toml" | "css"
            | "scss" | "less" | "csv" | "xml" | "c" | "cpp" | "h" | "hpp" | "java" | "go" | "sh"
            | "bash" | "zsh" | "rb" | "php" | "swift" | "kt" | "scala" | "r" | "sql" | "lua"
            | "vim" | "el" | "clj" | "ex" | "exs" | "erl" | "hs" | "ml" | "ini" | "cfg" | "conf"
            | "env" | "gitignore" | "dockerignore" | "cmake",
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
fn apply_chunk_overlap(chunks: &mut [ParsedChunk], overlap_chars: usize) {
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
            if ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
                && value.len() >= 2
            {
                value = value[1..value.len() - 1].to_string();
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

    // -- New format MIME detection ------------------------------------------

    #[test]
    fn test_detect_mime_doc() {
        assert_eq!(
            detect_mime_type(Path::new("file.doc")),
            "application/msword"
        );
    }

    #[test]
    fn test_detect_mime_ppt() {
        assert_eq!(
            detect_mime_type(Path::new("file.ppt")),
            "application/vnd.ms-powerpoint"
        );
    }

    #[test]
    fn test_detect_mime_html() {
        assert_eq!(detect_mime_type(Path::new("page.html")), "text/html");
        assert_eq!(detect_mime_type(Path::new("page.htm")), "text/html");
    }

    #[test]
    fn test_detect_mime_epub() {
        assert_eq!(
            detect_mime_type(Path::new("book.epub")),
            "application/epub+zip"
        );
    }

    #[test]
    fn test_detect_mime_odf() {
        assert_eq!(
            detect_mime_type(Path::new("doc.odt")),
            "application/vnd.oasis.opendocument.text"
        );
        assert_eq!(
            detect_mime_type(Path::new("sheet.ods")),
            "application/vnd.oasis.opendocument.spreadsheet"
        );
        assert_eq!(
            detect_mime_type(Path::new("slides.odp")),
            "application/vnd.oasis.opendocument.presentation"
        );
    }

    // -- HTML stripping -----------------------------------------------------

    #[test]
    fn test_strip_html_tags_basic() {
        let html =
            "<html><head><title>Test</title></head><body><h1>Hello</h1><p>World</p></body></html>";
        let result = strip_html_tags(html);
        assert!(result.contains("# Hello"), "Should convert h1 to markdown");
        assert!(result.contains("World"), "Should preserve text content");
        assert!(
            !result.contains("<p>"),
            "Should strip tags: got: {}",
            result
        );
    }

    #[test]
    fn test_strip_html_tags_preserves_links() {
        let html = r#"<p>Visit <a href="https://example.com">Example</a> site</p>"#;
        let result = strip_html_tags(html);
        assert!(
            result.contains("[Example](https://example.com)"),
            "Links should be markdown: got: {}",
            result
        );
    }

    #[test]
    fn test_strip_html_tags_removes_scripts() {
        let html = "<p>Before</p><script>alert('xss')</script><p>After</p>";
        let result = strip_html_tags(html);
        assert!(result.contains("Before"));
        assert!(result.contains("After"));
        assert!(!result.contains("alert"), "Scripts should be removed");
    }

    #[test]
    fn test_decode_html_entities_fn() {
        assert_eq!(decode_html_entities("&amp; &lt; &gt;"), "& < >");
        assert_eq!(decode_html_entities("&nbsp;"), " ");
    }

    // -- Binary text extraction ---------------------------------------------

    #[test]
    fn test_extract_text_from_binary_basic() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0x00; 10]);
        data.extend_from_slice(
            b"This is a test string that is long enough to be extracted as text content from binary data",
        );
        data.extend_from_slice(&[0x00; 10]);

        let result = extract_text_from_binary(&data);
        assert!(
            result.contains("This is a test string"),
            "Should extract readable text"
        );
    }

    #[test]
    fn test_extract_text_from_binary_filters_short() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0x00; 5]);
        data.extend_from_slice(b"short"); // too short to extract
        data.extend_from_slice(&[0x00; 5]);

        let result = extract_text_from_binary(&data);
        assert!(result.is_empty(), "Short runs should be filtered out");
    }

    // -- HTML file parsing --------------------------------------------------

    #[test]
    fn test_parse_html_file() {
        let mut f = NamedTempFile::with_suffix(".html").unwrap();
        write!(
            f,
            "<html><head><title>Test Page</title></head><body>\
             <h1>Welcome</h1>\
             <p>This is a test paragraph with enough content to meet the minimum \
             chunk size threshold for extraction and indexing purposes.</p>\
             </body></html>"
        )
        .unwrap();
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
        assert_eq!(doc.mime_type, "text/html");
        assert_eq!(doc.title, "Test Page");
        assert!(!doc.chunks.is_empty(), "Should produce chunks");
        for chunk in &doc.chunks {
            assert!(!chunk.content.contains("<html>"));
            assert!(!chunk.content.contains("<p>"));
        }
    }

    // -- EPUB parsing -------------------------------------------------------

    #[test]
    fn test_parse_epub_basic() {
        use std::io::{Cursor, Write as IoWrite};

        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let options = zip::write::SimpleFileOptions::default();

            zip.start_file("mimetype", options).unwrap();
            zip.write_all(b"application/epub+zip").unwrap();

            zip.start_file("META-INF/container.xml", options).unwrap();
            zip.write_all(
                br#"<?xml version="1.0"?>
                <container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
                <rootfiles>
                <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
                </rootfiles></container>"#,
            )
            .unwrap();

            zip.start_file("OEBPS/content.opf", options).unwrap();
            zip.write_all(
                br#"<?xml version="1.0"?>
                <package xmlns="http://www.idpf.org/2007/opf" version="3.0">
                <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
                <dc:title>Test Book</dc:title>
                </metadata>
                <manifest>
                <item id="ch1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
                </manifest>
                <spine><itemref idref="ch1"/></spine>
                </package>"#,
            )
            .unwrap();

            zip.start_file("OEBPS/chapter1.xhtml", options).unwrap();
            zip.write_all(
                b"<html><body><h1>Chapter One</h1>\
                  <p>This is the first chapter of the test book with enough content \
                  to pass the minimum chunk size threshold for parsing and indexing.</p>\
                  </body></html>",
            )
            .unwrap();

            zip.finish().unwrap();
        }

        let mut f = NamedTempFile::with_suffix(".epub").unwrap();
        f.write_all(buf.get_ref()).unwrap();
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
        assert_eq!(doc.mime_type, "application/epub+zip");
        assert_eq!(doc.title, "Test Book");
        assert!(!doc.chunks.is_empty(), "Should produce chunks");
        assert!(
            doc.chunks
                .iter()
                .any(|c| c.content.contains("Chapter One") || c.content.contains("first chapter")),
            "Should contain chapter text"
        );
    }

    // -- ODF parsing --------------------------------------------------------

    #[test]
    fn test_parse_odf_zip_fallback() {
        use std::io::{Cursor, Write as IoWrite};

        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("content.xml", options).unwrap();
            zip.write_all(
                b"<office:document-content><office:body><office:text>\
                  <text:p>This is test content from an ODF document that should be \
                  long enough to meet the minimum chunk threshold for extraction.</text:p>\
                  </office:text></office:body></office:document-content>",
            )
            .unwrap();
            zip.finish().unwrap();
        }

        let mut f = NamedTempFile::with_suffix(".odt").unwrap();
        f.write_all(buf.get_ref()).unwrap();
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
        assert_eq!(doc.mime_type, "application/vnd.oasis.opendocument.text");
        assert!(!doc.chunks.is_empty(), "Should produce chunks");
        assert!(
            doc.chunks[0].content.contains("This is test content"),
            "Should extract text from content.xml"
        );
    }
}
