use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use lopdf::Document;

use super::model::{PreviewBlock, PreviewRun, StructuredPreview};

const PDF_PREVIEW_MAX_PAGES: usize = 80;
const PDF_PREVIEW_MAX_PARAGRAPHS_PER_PAGE: usize = 80;
const PDF_PREVIEW_MAX_PARAGRAPH_CHARS: usize = 4_000;

pub fn preview_pdf(path: &Path) -> Result<Option<StructuredPreview>, String> {
    panic::catch_unwind(AssertUnwindSafe(|| preview_pdf_inner(path)))
        .map_err(|payload| format!("panic: {}", panic_payload_to_string(payload)))?
}

fn preview_pdf_inner(path: &Path) -> Result<Option<StructuredPreview>, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let doc = Document::load_mem(&bytes).map_err(|e| format!("PDF load failed: {e}"))?;
    let pages = doc.get_pages();
    if pages.is_empty() {
        return Ok(None);
    }

    let page_count = pages.len();
    let mut blocks = Vec::new();
    let mut extracted_any_text = false;
    let mut first_error: Option<String> = None;

    for (index, page_number) in pages.keys().take(PDF_PREVIEW_MAX_PAGES).enumerate() {
        if index > 0 {
            blocks.push(PreviewBlock::PageBreak);
        }
        blocks.push(heading(format!("Page {}", index + 1), 2));

        match extract_page_text(&doc, *page_number) {
            Ok(text) if !text.trim().is_empty() => {
                extracted_any_text = true;
                let mut paragraph_count = 0;
                for paragraph in pdf_paragraphs(&text) {
                    if paragraph_count >= PDF_PREVIEW_MAX_PARAGRAPHS_PER_PAGE {
                        blocks.push(PreviewBlock::Unsupported {
                            message: format!(
                                "Page {} preview is limited to the first {} paragraphs.",
                                index + 1,
                                PDF_PREVIEW_MAX_PARAGRAPHS_PER_PAGE
                            ),
                        });
                        break;
                    }
                    blocks.push(paragraph_block(paragraph));
                    paragraph_count += 1;
                }
            }
            Ok(_) => {
                blocks.push(PreviewBlock::Unsupported {
                    message: "No extractable text layer was found on this page.".to_string(),
                });
            }
            Err(err) => {
                if first_error.is_none() {
                    first_error = Some(err.clone());
                }
                blocks.push(PreviewBlock::Unsupported {
                    message: format!("Could not extract text from this page: {err}"),
                });
            }
        }
    }

    if page_count > PDF_PREVIEW_MAX_PAGES {
        blocks.push(PreviewBlock::Unsupported {
            message: format!(
                "PDF structured preview is limited to the first {} of {} pages.",
                PDF_PREVIEW_MAX_PAGES, page_count
            ),
        });
    }

    if !extracted_any_text {
        if let Some(err) = first_error {
            return Err(format!("Could not extract structured PDF text: {err}"));
        }
        return Ok(None);
    }

    Ok(Some(StructuredPreview::Document {
        blocks,
        assets: Vec::new(),
    }))
}

fn extract_page_text(doc: &Document, page_number: u32) -> Result<String, String> {
    let mut text = String::new();
    let mut decode_error_count = 0;
    let mut first_decode_error: Option<String> = None;

    for chunk in doc.extract_text_chunks(&[page_number]) {
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

    if !text.trim().is_empty() || decode_error_count == 0 {
        Ok(text)
    } else {
        Err(format!(
            "{} decode errors; first error: {}",
            decode_error_count,
            first_decode_error.unwrap_or_else(|| "unknown".to_string())
        ))
    }
}

fn pdf_paragraphs(text: &str) -> Vec<String> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut paragraphs = Vec::new();
    let mut current = String::new();

    for line in normalized.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            flush_paragraph(&mut paragraphs, &mut current);
            continue;
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    flush_paragraph(&mut paragraphs, &mut current);
    paragraphs
}

fn flush_paragraph(paragraphs: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        paragraphs.push(limit_paragraph(trimmed));
    }
    current.clear();
}

fn limit_paragraph(text: &str) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= PDF_PREVIEW_MAX_PARAGRAPH_CHARS {
            out.push_str("\n[truncated]");
            break;
        }
        out.push(ch);
    }
    out
}

fn heading(text: String, level: u8) -> PreviewBlock {
    PreviewBlock::Heading {
        level,
        runs: vec![plain_run(text)],
        alignment: None,
    }
}

fn paragraph_block(text: String) -> PreviewBlock {
    PreviewBlock::Paragraph {
        runs: vec![plain_run(text)],
        alignment: None,
    }
}

fn plain_run(text: String) -> PreviewRun {
    PreviewRun {
        text,
        ..PreviewRun::default()
    }
}

fn panic_payload_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::content::{Content, Operation};
    use lopdf::{dictionary, Object, Stream};

    #[test]
    fn preview_pdf_builds_document_blocks_per_page() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.pdf");
        write_test_pdf(&path, &["Executive Summary", "Second page details"]);

        let preview = preview_pdf(&path)
            .expect("preview pdf")
            .expect("structured");
        let StructuredPreview::Document { blocks, assets } = preview else {
            panic!("expected document preview");
        };

        assert!(assets.is_empty());
        assert!(matches!(
            &blocks[0],
            PreviewBlock::Heading { runs, .. } if runs[0].text == "Page 1"
        ));
        assert!(blocks.iter().any(|block| matches!(
            block,
            PreviewBlock::Paragraph { runs, .. } if runs[0].text.contains("Executive Summary")
        )));
        assert!(blocks
            .iter()
            .any(|block| matches!(block, PreviewBlock::PageBreak)));
        assert!(blocks.iter().any(|block| matches!(
            block,
            PreviewBlock::Paragraph { runs, .. } if runs[0].text.contains("Second page details")
        )));
    }

    fn write_test_pdf(path: &Path, page_texts: &[&str]) {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! {
                "F1" => font_id,
            },
        });

        let mut kids = Vec::new();
        for text in page_texts {
            let content = Content {
                operations: vec![
                    Operation::new("BT", vec![]),
                    Operation::new("Tf", vec!["F1".into(), 12.into()]),
                    Operation::new("Td", vec![72.into(), 720.into()]),
                    Operation::new("Tj", vec![Object::string_literal(*text)]),
                    Operation::new("ET", vec![]),
                ],
            };
            let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "Contents" => content_id,
                "Resources" => resources_id,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            });
            kids.push(page_id.into());
        }

        let pages = dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => page_texts.len() as i64,
        };
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        doc.compress();
        doc.save(path).unwrap();
    }
}
