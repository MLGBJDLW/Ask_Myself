//! EditDocumentTool — edits existing DOCX / PPTX / XLSX files via ZIP-level XML patching.

use std::io::{Cursor, Read, Write};
use std::path::PathBuf;
use std::sync::OnceLock;

use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::create_file_tool::{has_path_traversal, resolve_and_validate};
use super::document_utils::generated_document_mime;
use super::{scoped_sources, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/edit_document.json");

/// Maximum file size we will process (50 MB).
const MAX_FILE_SIZE: u64 = 50 * 1024 * 1024;

pub struct EditDocumentTool;

#[derive(Deserialize)]
struct EditDocumentArgs {
    path: String,
    replacements: Vec<Replacement>,
}

#[derive(Deserialize, Clone)]
struct Replacement {
    old_text: String,
    new_text: String,
}

// ---------------------------------------------------------------------------
// Document format detection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocFormat {
    Docx,
    Pptx,
    Xlsx,
}

fn detect_format(path: &std::path::Path) -> Option<DocFormat> {
    match generated_document_mime(path) {
        Some("docx") => Some(DocFormat::Docx),
        Some("pptx") => Some(DocFormat::Pptx),
        Some("xlsx") => Some(DocFormat::Xlsx),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// XML text replacement (handles split runs)
// ---------------------------------------------------------------------------

/// Escape text for insertion into XML content.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Apply text replacements to XML content, handling the split-runs problem.
///
/// Office XML often splits a single visible word across multiple text-element
/// runs. For example, "Hello World" might become:
///   `<w:t>Hel</w:t></w:r><w:r>...<w:t>lo World</w:t>`
///
/// Strategy: for each paragraph-level container, we collect the text content
/// of all text elements, concatenate them, apply the replacement on the
/// concatenated text, then redistribute among the runs. The first run gets
/// the full replacement text; subsequent runs that were part of the match
/// are emptied.
///
/// `text_tag` is the local tag name containing text, e.g. `w:t`, `a:t`, or `t`.
fn apply_replacements_to_xml(
    xml: &str,
    replacements: &[Replacement],
    text_tag: &str,
) -> (String, Vec<ReplacementOutcome>) {
    let mut result = xml.to_string();
    let mut outcomes: Vec<ReplacementOutcome> = Vec::new();

    for replacement in replacements {
        let old_text = &replacement.old_text;
        let new_text = &replacement.new_text;

        if old_text.is_empty() {
            outcomes.push(ReplacementOutcome {
                old_text: old_text.clone(),
                count: 0,
                warning: Some("old_text is empty, skipped".to_string()),
            });
            continue;
        }

        // Strategy 1: Try direct replacement on text element content.
        // This handles the common case where text is not split across runs.
        let escaped_old = xml_escape(old_text);
        let escaped_new = xml_escape(new_text);

        // Count simple matches (text fully contained in a single tag).
        let simple_count = result.matches(&escaped_old).count();
        if simple_count > 0 {
            result = result.replace(&escaped_old, &escaped_new);
            outcomes.push(ReplacementOutcome {
                old_text: old_text.clone(),
                count: simple_count,
                warning: None,
            });
            continue;
        }

        // Strategy 2: Handle split-runs by working at the paragraph level.
        let count = apply_split_run_replacement(&mut result, old_text, new_text, text_tag);
        if count > 0 {
            outcomes.push(ReplacementOutcome {
                old_text: old_text.clone(),
                count,
                warning: None,
            });
        } else {
            outcomes.push(ReplacementOutcome {
                old_text: old_text.clone(),
                count: 0,
                warning: Some("text not found in document".to_string()),
            });
        }
    }

    (result, outcomes)
}

/// Stable equivalent of nightly `str::floor_char_boundary`.
/// Returns the largest index `<= idx` that is a valid char boundary in `s`.
/// If `idx >= s.len()`, returns `s.len()`.
fn floor_char_boundary(s: &str, idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    let mut i = idx;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Returns the smallest index `>= idx` that is a valid char boundary in `s`.
/// If `idx >= s.len()`, returns `s.len()`.
fn ceil_char_boundary(s: &str, idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    let mut i = idx;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Handle split-run replacement by collecting text across adjacent text
/// elements and redistributing after replacement.
fn apply_split_run_replacement(
    xml: &mut String,
    old_text: &str,
    new_text: &str,
    text_tag: &str,
) -> usize {
    // Build a regex that matches text element content: <tag ...>content</tag>
    // We need to capture the content including the tags so we can rebuild.
    let pattern = format!(
        r"(<{tag}[^>]*>)([^<]*?)(</{tag}>)",
        tag = regex::escape(text_tag)
    );
    let re = match Regex::new(&pattern) {
        Ok(r) => r,
        Err(_) => return 0,
    };

    let escaped_old = xml_escape(old_text);
    let escaped_new = xml_escape(new_text);

    let mut total_replaced = 0;

    // We process the XML in a loop until no more replacements are found.
    loop {
        // Collect all text element positions and their content.
        let text_elements: Vec<TextElement> = re
            .captures_iter(xml)
            .map(|cap| {
                let full_match = cap.get(0).unwrap();
                TextElement {
                    start: full_match.start(),
                    end: full_match.end(),
                    open_tag: cap[1].to_string(),
                    content: cap[2].to_string(),
                    close_tag: cap[3].to_string(),
                }
            })
            .collect();

        if text_elements.is_empty() {
            break;
        }

        // Try to find the old_text spanning consecutive text elements.
        let mut found = false;
        let concatenated: String = text_elements.iter().map(|e| e.content.as_str()).collect();

        if let Some(pos) = concatenated.find(&escaped_old) {
            // Find which text elements are involved.
            let mut char_offset = 0;
            let mut start_elem = None;
            let mut end_elem = None;

            for (i, elem) in text_elements.iter().enumerate() {
                let elem_end = char_offset + elem.content.len();
                if start_elem.is_none() && pos < elem_end {
                    start_elem = Some(i);
                }
                if start_elem.is_some() && pos + escaped_old.len() <= elem_end {
                    end_elem = Some(i);
                    break;
                }
                char_offset = elem_end;
            }

            if let (Some(si), Some(ei)) = (start_elem, end_elem) {
                // Build new content for the involved elements.
                let mut char_offset = 0;
                for elem in text_elements.iter().take(si) {
                    char_offset += elem.content.len();
                }

                // Calculate the position within the first involved element.
                // `pos` is a valid char boundary in `concatenated`, and
                // `char_offset` is the exact sum of preceding element byte
                // lengths, so `offset_in_first` should already be a valid
                // char boundary in `elem.content[si]`. We clamp defensively
                // using `floor_char_boundary` to guarantee no panic even if
                // upstream escaping logic drifts.
                let first_elem = &text_elements[si];
                let raw_offset_in_first = pos.saturating_sub(char_offset);
                let offset_in_first =
                    floor_char_boundary(&first_elem.content, raw_offset_in_first);
                debug_assert!(
                    first_elem.content.is_char_boundary(offset_in_first),
                    "offset_in_first must be a valid char boundary"
                );

                // Build replacement: put everything into the first element,
                // empty subsequent matched elements.
                let mut new_xml = xml.clone();
                // We need to replace from the last element to the first to
                // preserve byte offsets.
                for i in (si..=ei).rev() {
                    let elem = &text_elements[i];
                    let new_content = if i == si {
                        // First element: prefix + replacement + suffix
                        let mut c = String::new();
                        c.push_str(&elem.content[..offset_in_first]);
                        c.push_str(&escaped_new);
                        if i == ei {
                            // Same element: add the suffix after the match.
                            let raw_suffix_start = offset_in_first + escaped_old.len();
                            let suffix_start =
                                ceil_char_boundary(&elem.content, raw_suffix_start);
                            if suffix_start <= elem.content.len() {
                                debug_assert!(elem.content.is_char_boundary(suffix_start));
                                c.clear();
                                c.push_str(&elem.content[..offset_in_first]);
                                c.push_str(&escaped_new);
                                c.push_str(&elem.content[suffix_start..]);
                            }
                        }
                        c
                    } else if i == ei {
                        // Last element: keep suffix after the match
                        let mut consumed = 0;
                        for e in text_elements.iter().take(i).skip(si) {
                            consumed += e.content.len();
                        }
                        let raw_match_end = (pos + escaped_old.len())
                            .saturating_sub(char_offset + consumed);
                        let match_end_in_this =
                            ceil_char_boundary(&elem.content, raw_match_end);
                        if match_end_in_this <= elem.content.len() {
                            debug_assert!(elem.content.is_char_boundary(match_end_in_this));
                            elem.content[match_end_in_this..].to_string()
                        } else {
                            String::new()
                        }
                    } else {
                        // Middle element: empty it
                        String::new()
                    };

                    let rebuilt = format!("{}{}{}", elem.open_tag, new_content, elem.close_tag);
                    new_xml.replace_range(elem.start..elem.end, &rebuilt);
                }

                *xml = new_xml;
                total_replaced += 1;
                found = true;
            }
        }

        if !found {
            break;
        }
    }

    total_replaced
}

struct TextElement {
    start: usize,
    end: usize,
    open_tag: String,
    content: String,
    close_tag: String,
}

struct ReplacementOutcome {
    old_text: String,
    count: usize,
    warning: Option<String>,
}

// ---------------------------------------------------------------------------
// ZIP-level patching
// ---------------------------------------------------------------------------

/// Read the file, patch relevant XML parts inside the ZIP, write back.
fn patch_office_file(
    path: &std::path::Path,
    format: DocFormat,
    replacements: &[Replacement],
) -> Result<String, String> {
    let file_bytes = std::fs::read(path).map_err(|e| format!("Cannot read file: {e}"))?;

    let reader = Cursor::new(&file_bytes);
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| format!("Cannot open ZIP archive: {e}"))?;

    // Determine which entries to patch and the text tag to use.
    #[allow(clippy::type_complexity)]
    let (entry_filter, text_tag): (Box<dyn Fn(&str) -> bool>, &str) = match format {
        DocFormat::Docx => (Box::new(|name: &str| name == "word/document.xml"), "w:t"),
        DocFormat::Pptx => (
            Box::new(|name: &str| name.starts_with("ppt/slides/slide") && name.ends_with(".xml")),
            "a:t",
        ),
        DocFormat::Xlsx => (
            Box::new(|name: &str| {
                name == "xl/sharedStrings.xml"
                    || (name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml"))
            }),
            "t",
        ),
    };

    // Collect all entry names and their raw bytes.
    let entry_count = archive.len();
    let mut entries: Vec<(String, Vec<u8>, zip::CompressionMethod)> =
        Vec::with_capacity(entry_count);
    for i in 0..entry_count {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("Cannot read ZIP entry {i}: {e}"))?;
        let name = entry.name().to_string();
        let method = entry.compression();
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut buf)
            .map_err(|e| format!("Cannot read ZIP entry '{}': {e}", name))?;
        entries.push((name, buf, method));
    }

    // Apply replacements to matching XML entries.
    let mut all_outcomes: Vec<ReplacementOutcome> = Vec::new();
    for (name, buf, _method) in &mut entries {
        if !entry_filter(name) {
            continue;
        }
        let xml = String::from_utf8_lossy(buf).to_string();
        let (patched, outcomes) = apply_replacements_to_xml(&xml, replacements, text_tag);
        all_outcomes.extend(outcomes);
        *buf = patched.into_bytes();
    }

    // Write the modified ZIP back to a buffer.
    let mut out_buf = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut out_buf);
        for (name, buf, method) in &entries {
            let options = zip::write::SimpleFileOptions::default().compression_method(*method);
            writer
                .start_file(name, options)
                .map_err(|e| format!("Cannot write ZIP entry '{}': {e}", name))?;
            writer
                .write_all(buf)
                .map_err(|e| format!("Cannot write ZIP entry '{}': {e}", name))?;
        }
        writer
            .finish()
            .map_err(|e| format!("Cannot finalize ZIP: {e}"))?;
    }

    // Write back to the original path.
    std::fs::write(path, out_buf.into_inner()).map_err(|e| format!("Cannot write file: {e}"))?;

    // Build summary.
    build_summary(path, &all_outcomes, replacements)
}

fn build_summary(
    path: &std::path::Path,
    outcomes: &[ReplacementOutcome],
    replacements: &[Replacement],
) -> Result<String, String> {
    let mut summary = String::new();
    let total_requested = replacements.len();
    let mut total_applied = 0usize;
    let mut warnings: Vec<String> = Vec::new();

    // Deduplicate outcomes by old_text (multiple XML parts may report the same replacement).
    let mut seen: std::collections::HashMap<String, (usize, Option<String>)> =
        std::collections::HashMap::new();
    for outcome in outcomes {
        let entry = seen.entry(outcome.old_text.clone()).or_insert((0, None));
        entry.0 += outcome.count;
        if outcome.warning.is_some() && entry.0 == 0 {
            entry.1 = outcome.warning.clone();
        } else if entry.0 > 0 {
            // Clear warning if replacement was found in another XML part.
            entry.1 = None;
        }
    }

    for (old_text, (count, warning)) in &seen {
        total_applied += count;
        if let Some(w) = warning {
            warnings.push(format!("'{}': {}", truncate_text(old_text, 40), w));
        }
    }

    summary.push_str(&format!(
        "Edited '{}': {}/{} replacements applied.",
        path.display(),
        total_applied,
        total_requested
    ));

    if !warnings.is_empty() {
        summary.push_str("\n\nWarnings:");
        for w in &warnings {
            summary.push_str(&format!("\n  - {w}"));
        }
    }

    Ok(summary)
}

fn truncate_text(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let cut = floor_char_boundary(s, max);
        format!("{}...", &s[..cut])
    }
}

// ---------------------------------------------------------------------------
// Tool implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Tool for EditDocumentTool {
    fn name(&self) -> &str {
        "edit_document"
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

    fn requires_confirmation(&self, _args: &serde_json::Value) -> bool {
        true
    }

    fn confirmation_message(&self, args: &serde_json::Value) -> Option<String> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("<unknown>");
        let count = args
            .get("replacements")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        Some(format!(
            "Edit document: {path} ({count} replacement{})",
            if count == 1 { "" } else { "s" }
        ))
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: EditDocumentArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid edit_document arguments: {e}"))
        })?;

        if args.replacements.is_empty() {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "No replacements specified.".to_string(),
                is_error: true,
                artifacts: None,
            });
        }

        if has_path_traversal(&args.path) {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "Path contains directory traversal sequences.".to_string(),
                is_error: true,
                artifacts: None,
            });
        }

        let db = db.clone();
        let call_id = call_id.to_string();
        let source_scope = source_scope.to_vec();
        tokio::task::spawn_blocking(move || {
            let sources = scoped_sources(&db, &source_scope)?;
            if sources.is_empty() {
                return Ok(ToolResult {
                    call_id: call_id.clone(),
                    content: "No sources registered. Add a source directory first.".to_string(),
                    is_error: true,
                    artifacts: None,
                });
            }

            let requested = PathBuf::from(&args.path);
            let canonical = match resolve_and_validate(&requested, &sources) {
                Ok(p) => p,
                Err(msg) => {
                    return Ok(ToolResult {
                        call_id: call_id.clone(),
                        content: msg,
                        is_error: true,
                        artifacts: None,
                    });
                }
            };

            if !canonical.is_file() {
                return Ok(ToolResult {
                    call_id: call_id.clone(),
                    content: format!("File not found: '{}'", args.path),
                    is_error: true,
                    artifacts: None,
                });
            }

            // Check file size.
            let meta = std::fs::metadata(&canonical)
                .map_err(CoreError::Io)?;
            if meta.len() > MAX_FILE_SIZE {
                return Ok(ToolResult {
                    call_id: call_id.clone(),
                    content: format!(
                        "File too large ({:.1} MB, limit is {} MB)",
                        meta.len() as f64 / (1024.0 * 1024.0),
                        MAX_FILE_SIZE / (1024 * 1024)
                    ),
                    is_error: true,
                    artifacts: None,
                });
            }

            let format = match detect_format(&canonical) {
                Some(f) => f,
                None => {
                    return Ok(ToolResult {
                        call_id: call_id.clone(),
                        content: format!(
                            "Unsupported file format. edit_document supports DOCX, PPTX, and XLSX files. File: '{}'",
                            args.path
                        ),
                        is_error: true,
                        artifacts: None,
                    });
                }
            };

            match patch_office_file(&canonical, format, &args.replacements) {
                Ok(summary) => Ok(ToolResult {
                    call_id,
                    content: summary,
                    is_error: false,
                    artifacts: None,
                }),
                Err(msg) => Ok(ToolResult {
                    call_id,
                    content: msg,
                    is_error: true,
                    artifacts: None,
                }),
            }
        })
        .await
        .map_err(|e| CoreError::InvalidInput(format!("Task join error: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xml_escape() {
        assert_eq!(xml_escape("a & b"), "a &amp; b");
        assert_eq!(xml_escape("<hello>"), "&lt;hello&gt;");
    }

    #[test]
    fn test_simple_replacement_in_docx_xml() {
        let xml = r#"<w:p><w:r><w:t>Hello World</w:t></w:r></w:p>"#;
        let replacements = vec![Replacement {
            old_text: "Hello World".to_string(),
            new_text: "Goodbye World".to_string(),
        }];
        let (result, outcomes) = apply_replacements_to_xml(xml, &replacements, "w:t");
        assert!(result.contains("Goodbye World"));
        assert_eq!(outcomes[0].count, 1);
        assert!(outcomes[0].warning.is_none());
    }

    #[test]
    fn test_simple_replacement_in_pptx_xml() {
        let xml = r#"<a:p><a:r><a:t>Slide Title</a:t></a:r></a:p>"#;
        let replacements = vec![Replacement {
            old_text: "Slide Title".to_string(),
            new_text: "New Title".to_string(),
        }];
        let (result, outcomes) = apply_replacements_to_xml(xml, &replacements, "a:t");
        assert!(result.contains("New Title"));
        assert_eq!(outcomes[0].count, 1);
    }

    #[test]
    fn test_simple_replacement_in_xlsx_xml() {
        let xml = r#"<sst><si><t>Cell Value</t></si></sst>"#;
        let replacements = vec![Replacement {
            old_text: "Cell Value".to_string(),
            new_text: "New Value".to_string(),
        }];
        let (result, outcomes) = apply_replacements_to_xml(xml, &replacements, "t");
        assert!(result.contains("New Value"));
        assert_eq!(outcomes[0].count, 1);
    }

    #[test]
    fn test_replacement_not_found() {
        let xml = r#"<w:p><w:r><w:t>Hello</w:t></w:r></w:p>"#;
        let replacements = vec![Replacement {
            old_text: "Nonexistent".to_string(),
            new_text: "Replacement".to_string(),
        }];
        let (_result, outcomes) = apply_replacements_to_xml(xml, &replacements, "w:t");
        assert_eq!(outcomes[0].count, 0);
        assert!(outcomes[0].warning.is_some());
    }

    #[test]
    fn test_empty_old_text_skipped() {
        let xml = r#"<w:p><w:r><w:t>Hello</w:t></w:r></w:p>"#;
        let replacements = vec![Replacement {
            old_text: String::new(),
            new_text: "Replacement".to_string(),
        }];
        let (result, outcomes) = apply_replacements_to_xml(xml, &replacements, "w:t");
        assert!(result.contains("Hello"));
        assert_eq!(outcomes[0].count, 0);
        assert!(outcomes[0].warning.is_some());
    }

    #[test]
    fn test_multiple_replacements() {
        let xml = r#"<w:p><w:r><w:t>Hello World</w:t></w:r><w:r><w:t>Foo Bar</w:t></w:r></w:p>"#;
        let replacements = vec![
            Replacement {
                old_text: "Hello World".to_string(),
                new_text: "Hi".to_string(),
            },
            Replacement {
                old_text: "Foo Bar".to_string(),
                new_text: "Baz".to_string(),
            },
        ];
        let (result, outcomes) = apply_replacements_to_xml(xml, &replacements, "w:t");
        assert!(result.contains("Hi"));
        assert!(result.contains("Baz"));
        assert!(!result.contains("Hello World"));
        assert!(!result.contains("Foo Bar"));
        assert_eq!(outcomes.len(), 2);
    }

    #[test]
    fn test_special_characters_in_replacement() {
        let xml = r#"<w:p><w:r><w:t>Price: $100</w:t></w:r></w:p>"#;
        let replacements = vec![Replacement {
            old_text: "Price: $100".to_string(),
            new_text: "Price: $200 & tax".to_string(),
        }];
        let (result, outcomes) = apply_replacements_to_xml(xml, &replacements, "w:t");
        assert!(result.contains("Price: $200 &amp; tax"));
        assert_eq!(outcomes[0].count, 1);
    }

    #[test]
    fn test_split_run_replacement() {
        // Simulates "Hello" split across two runs.
        let xml = r#"<w:p><w:r><w:t>Hel</w:t></w:r><w:r><w:t>lo</w:t></w:r></w:p>"#;
        let replacements = vec![Replacement {
            old_text: "Hello".to_string(),
            new_text: "Goodbye".to_string(),
        }];
        let (result, outcomes) = apply_replacements_to_xml(xml, &replacements, "w:t");
        // The concatenated text should be replaced.
        assert!(
            result.contains("Goodbye"),
            "Expected 'Goodbye' in result: {result}"
        );
        assert_eq!(outcomes[0].count, 1);
    }

    #[test]
    fn test_detect_format() {
        use std::path::Path;
        assert_eq!(detect_format(Path::new("test.docx")), Some(DocFormat::Docx));
        assert_eq!(detect_format(Path::new("test.pptx")), Some(DocFormat::Pptx));
        assert_eq!(detect_format(Path::new("test.xlsx")), Some(DocFormat::Xlsx));
        assert_eq!(detect_format(Path::new("test.txt")), None);
        assert_eq!(detect_format(Path::new("test.pdf")), None);
    }

    // ---------------------------------------------------------------------
    // UTF-8 / char-boundary safety (regression tests for the PPTX panic)
    // ---------------------------------------------------------------------

    #[test]
    fn test_floor_char_boundary_ascii() {
        let s = "hello";
        assert_eq!(floor_char_boundary(s, 0), 0);
        assert_eq!(floor_char_boundary(s, 3), 3);
        assert_eq!(floor_char_boundary(s, 5), 5);
        assert_eq!(floor_char_boundary(s, 100), 5);
    }

    #[test]
    fn test_floor_char_boundary_chinese() {
        // Each CJK char is 3 bytes in UTF-8. Boundaries: 0, 3, 6, 9, 12.
        let s = "你好世界";
        assert_eq!(floor_char_boundary(s, 0), 0);
        assert_eq!(floor_char_boundary(s, 1), 0);
        assert_eq!(floor_char_boundary(s, 2), 0);
        assert_eq!(floor_char_boundary(s, 3), 3);
        assert_eq!(floor_char_boundary(s, 4), 3);
        assert_eq!(floor_char_boundary(s, 12), 12);
        assert_eq!(floor_char_boundary(s, 100), 12);
    }

    #[test]
    fn test_ceil_char_boundary_chinese() {
        let s = "你好";
        assert_eq!(ceil_char_boundary(s, 0), 0);
        assert_eq!(ceil_char_boundary(s, 1), 3);
        assert_eq!(ceil_char_boundary(s, 2), 3);
        assert_eq!(ceil_char_boundary(s, 3), 3);
        assert_eq!(ceil_char_boundary(s, 6), 6);
        assert_eq!(ceil_char_boundary(s, 100), 6);
    }

    #[test]
    fn test_chinese_single_run_replacement() {
        let xml = r#"<a:p><a:r><a:t>你好世界，欢迎使用</a:t></a:r></a:p>"#;
        let replacements = vec![Replacement {
            old_text: "你好世界".to_string(),
            new_text: "Hello World".to_string(),
        }];
        let (result, outcomes) = apply_replacements_to_xml(xml, &replacements, "a:t");
        assert!(result.contains("Hello World"), "got: {}", result);
        assert!(!result.contains("你好世界"));
        assert_eq!(outcomes[0].count, 1);
    }

    #[test]
    fn test_chinese_spanning_runs() {
        // Chinese text split across two runs — the classic PPTX panic case.
        let xml = r#"<a:p><a:r><a:t>你好</a:t></a:r><a:r><a:t>世界</a:t></a:r></a:p>"#;
        let replacements = vec![Replacement {
            old_text: "你好世界".to_string(),
            new_text: "Hello".to_string(),
        }];
        let (result, outcomes) = apply_replacements_to_xml(xml, &replacements, "a:t");
        // Must not panic, and must replace.
        assert!(result.contains("Hello"), "got: {}", result);
        assert_eq!(outcomes[0].count, 1);
    }

    #[test]
    fn test_chinese_no_panic_on_boundary_spanning_match() {
        // Match that starts mid-run and ends mid-next-run with multi-byte
        // chars on both sides of the boundary.
        let xml = r#"<a:p><a:r><a:t>前缀中文</a:t></a:r><a:r><a:t>跨界文本后缀</a:t></a:r></a:p>"#;
        let replacements = vec![Replacement {
            old_text: "中文跨界".to_string(),
            new_text: "X".to_string(),
        }];
        // Must not panic regardless of whether the match is found.
        let (_result, _outcomes) =
            apply_replacements_to_xml(xml, &replacements, "a:t");
    }

    #[test]
    fn test_mixed_cjk_ascii_replacement() {
        let xml = r#"<w:p><w:r><w:t>Hello世界 2026年Q1报告</w:t></w:r></w:p>"#;
        let replacements = vec![Replacement {
            old_text: "2026年Q1".to_string(),
            new_text: "2026Q1工作".to_string(),
        }];
        let (result, outcomes) = apply_replacements_to_xml(xml, &replacements, "w:t");
        assert!(result.contains("2026Q1工作"), "got: {}", result);
        assert_eq!(outcomes[0].count, 1);
    }

    #[test]
    fn test_emoji_4byte_replacement() {
        // Emoji = 4-byte UTF-8 sequences.
        let xml = r#"<a:p><a:r><a:t>emoji 🎉 test</a:t></a:r></a:p>"#;
        let replacements = vec![Replacement {
            old_text: "🎉".to_string(),
            new_text: "✅".to_string(),
        }];
        let (result, _outcomes) =
            apply_replacements_to_xml(xml, &replacements, "a:t");
        // Must not panic.
        assert!(result.contains("✅") || result.contains("🎉"));
    }

    #[test]
    fn test_emoji_spanning_runs() {
        // Emoji split across runs — upstream Office writers may split anywhere.
        let xml = r#"<a:p><a:r><a:t>start 🎉party</a:t></a:r><a:r><a:t>🎊 end</a:t></a:r></a:p>"#;
        let replacements = vec![Replacement {
            old_text: "party🎊".to_string(),
            new_text: "OK".to_string(),
        }];
        // Must not panic.
        let _ = apply_replacements_to_xml(xml, &replacements, "a:t");
    }

    #[test]
    fn test_truncate_text_chinese_safe() {
        // truncate_text used to slice at arbitrary byte offsets; verify it
        // no longer panics on multi-byte boundaries.
        let s = "你好世界欢迎使用这个系统";
        let _ = truncate_text(s, 5); // would have panicked before the fix
        let _ = truncate_text(s, 7);
        let _ = truncate_text(s, 1000);
    }
}
