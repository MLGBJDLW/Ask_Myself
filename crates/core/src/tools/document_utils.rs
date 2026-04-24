use std::path::Path;

use crate::error::CoreError;

pub(crate) fn is_binary_file_error(err: &CoreError) -> bool {
    matches!(err, CoreError::Parse(msg) if msg.starts_with("File appears to be binary:"))
}

pub(crate) fn generated_document_mime(path: &Path) -> Option<&'static str> {
    let mime = crate::parse::detect_mime_type(path);
    match mime.as_str() {
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => Some("docx"),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => Some("xlsx"),
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => Some("pptx"),
        _ => None,
    }
}

pub(crate) fn supports_document_fallback(path: &Path) -> bool {
    let mime = crate::parse::detect_mime_type(path);
    matches!(
        mime.as_str(),
        "application/pdf"
            | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
    ) || mime.starts_with("image/")
}

pub(crate) fn edit_guidance_for_path(path: &Path) -> Option<String> {
    if let Some(format) = generated_document_mime(path) {
        return Some(format!(
            "Office documents are not plain-text editable with edit_file/create_file. Prefer run_shell + doc-script-editor (python scripts/edit_doc.py) for robust create/edit flows, use edit_document for quick text replacement, or use generate_docx/generate_xlsx/ppt_generate to regenerate '{}'. Avoid generate_document except legacy compatibility routing.",
            format
        ));
    }

    let mime = crate::parse::detect_mime_type(path);
    if mime == "application/pdf" {
        return Some(
            "PDF files are not editable via edit_file/create_file. Use the 'doc-script-editor' skill: run_shell with `python <SKILL_DIR>/scripts/edit_doc.py --path <abs-path> <replace|extract|redact>` to modify, extract text, or redact."
                .to_string(),
        );
    }
    if mime.starts_with("image/") {
        return Some(
            "This file can be inspected with read_file, but it is not editable via edit_file/create_file."
                .to_string(),
        );
    }

    None
}

pub(crate) fn flatten_parsed_document_text(parsed: &crate::parse::ParsedDocument) -> String {
    let mut out = String::new();
    for chunk in &parsed.chunks {
        let visible = chunk
            .content
            .get(chunk.overlap_start..)
            .unwrap_or(chunk.content.as_str());
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(visible);
    }

    if out.trim().is_empty() {
        format!(
            "[No extractable text found in document: {}]",
            parsed.file_name
        )
    } else {
        out
    }
}

pub(crate) fn read_supported_file_content(path: &Path) -> Result<String, CoreError> {
    match crate::parse::read_text_file(path) {
        Ok(raw) => Ok(raw),
        Err(err) if is_binary_file_error(&err) && supports_document_fallback(path) => {
            let parsed = crate::parse::parse_file(
                path,
                None,
                #[cfg(feature = "video")]
                None,
                None,
                None,
                None,
            )?;
            Ok(flatten_parsed_document_text(&parsed))
        }
        Err(err) => Err(err),
    }
}
