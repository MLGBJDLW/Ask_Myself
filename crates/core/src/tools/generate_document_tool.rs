//! GenerateDocumentTool — creates Office documents (DOCX, XLSX, PPTX).

use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::create_file_tool::{has_path_traversal, resolve_and_validate};
use super::{Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/generate_document.json");

pub struct GenerateDocumentTool;

#[derive(Deserialize)]
struct GenerateDocArgs {
    format: String,
    path: String,
    content: serde_json::Value,
}

// ---------------------------------------------------------------------------
// DOCX generation (via docx-rs)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct DocxContent {
    title: Option<String>,
    sections: Vec<DocxSection>,
}

#[derive(Deserialize)]
struct DocxSection {
    heading: Option<String>,
    body: String,
    bullet_items: Option<Vec<String>>,
}

fn generate_docx(path: &std::path::Path, content: &serde_json::Value) -> Result<u64, String> {
    use docx_rs::*;

    let content: DocxContent = serde_json::from_value(content.clone())
        .map_err(|e| format!("Invalid DOCX content: {e}"))?;

    let calibri = RunFonts::new()
        .ascii("Calibri")
        .hi_ansi("Calibri")
        .east_asia("Calibri")
        .cs("Calibri");
    let calibri_light = RunFonts::new()
        .ascii("Calibri Light")
        .hi_ansi("Calibri Light")
        .east_asia("Calibri Light")
        .cs("Calibri Light");

    // 1 inch = 1440 twips; page margins
    let mut doc = Docx::new().page_margin(
        PageMargin::new()
            .top(1440)
            .bottom(1440)
            .left(1440)
            .right(1440)
            .header(720)
            .footer(720),
    );

    // Set up bullet list numbering: abstract numbering 1, numbering 1
    let bullet_abstract = AbstractNumbering::new(1).add_level(
        Level::new(
            0,
            Start::new(1),
            NumberFormat::new("bullet"),
            LevelText::new("\u{2022}"),
            LevelJc::new("left"),
        )
        .indent(Some(720), Some(SpecialIndentType::Hanging(360)), None, None)
        .fonts(RunFonts::new().ascii("Symbol").hi_ansi("Symbol")),
    );
    let bullet_num = Numbering::new(1, 1);
    doc = doc
        .add_abstract_numbering(bullet_abstract)
        .add_numbering(bullet_num);

    // Title
    if let Some(title) = &content.title {
        doc = doc.add_paragraph(
            Paragraph::new()
                .align(AlignmentType::Center)
                .line_spacing(LineSpacing::new().after(200))
                .add_run(
                    Run::new()
                        .add_text(title)
                        .bold()
                        .size(56) // 28pt = 56 half-points
                        .color("1F4E79")
                        .fonts(calibri_light.clone()),
                ),
        );
        // Bottom border separator line (thin horizontal rule using an empty paragraph with bottom border)
        doc = doc.add_paragraph(
            Paragraph::new()
                .line_spacing(LineSpacing::new().after(200).before(0))
                .add_run(Run::new().size(4)),
        );
    }

    for section in &content.sections {
        // Heading
        if let Some(heading) = &section.heading {
            doc = doc.add_paragraph(
                Paragraph::new()
                    .line_spacing(LineSpacing::new().before(240).after(120)) // 12pt before, 6pt after
                    .add_run(
                        Run::new()
                            .add_text(heading)
                            .bold()
                            .size(32) // 16pt
                            .color("2E75B6")
                            .fonts(calibri_light.clone()),
                    ),
            );
        }

        // Body text — handle bullet lines (starting with "- " or "• ")
        for line in section.body.split('\n') {
            let trimmed = line.trim();
            if let Some(bullet_text) = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("• "))
            {
                doc = doc.add_paragraph(
                    Paragraph::new()
                        .numbering(NumberingId::new(1), IndentLevel::new(0))
                        .line_spacing(
                            LineSpacing::new()
                                .line(276) // 1.15 line spacing (240 * 1.15 = 276)
                                .line_rule(LineSpacingType::Auto)
                                .after(120), // 6pt
                        )
                        .add_run(
                            Run::new()
                                .add_text(bullet_text)
                                .size(22) // 11pt
                                .fonts(calibri.clone()),
                        ),
                );
            } else {
                doc = doc.add_paragraph(
                    Paragraph::new()
                        .align(AlignmentType::Both) // justified
                        .line_spacing(
                            LineSpacing::new()
                                .line(276)
                                .line_rule(LineSpacingType::Auto)
                                .after(120),
                        )
                        .add_run(
                            Run::new()
                                .add_text(line)
                                .size(22) // 11pt
                                .fonts(calibri.clone()),
                        ),
                );
            }
        }

        // Explicit bullet_items list
        if let Some(items) = &section.bullet_items {
            for item in items {
                doc = doc.add_paragraph(
                    Paragraph::new()
                        .numbering(NumberingId::new(1), IndentLevel::new(0))
                        .line_spacing(
                            LineSpacing::new()
                                .line(276)
                                .line_rule(LineSpacingType::Auto)
                                .after(120),
                        )
                        .add_run(Run::new().add_text(item).size(22).fonts(calibri.clone())),
                );
            }
        }
    }

    let file = std::fs::File::create(path).map_err(|e| format!("Failed to create file: {e}"))?;

    doc.build()
        .pack(file)
        .map_err(|e| format!("Failed to write DOCX: {e}"))?;

    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Ok(size)
}

// ---------------------------------------------------------------------------
// XLSX generation (via rust_xlsxwriter)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct XlsxContent {
    sheets: Vec<XlsxSheet>,
}

#[derive(Deserialize)]
struct XlsxSheet {
    name: String,
    headers: Option<Vec<String>>,
    rows: Vec<Vec<serde_json::Value>>,
    column_widths: Option<Vec<f64>>,
}

fn generate_xlsx(path: &std::path::Path, content: &serde_json::Value) -> Result<u64, String> {
    use rust_xlsxwriter::{Color, Format, FormatAlign, FormatBorder};

    let content: XlsxContent = serde_json::from_value(content.clone())
        .map_err(|e| format!("Invalid XLSX content: {e}"))?;

    let mut workbook = rust_xlsxwriter::Workbook::new();

    // Shared formats
    let header_fmt = Format::new()
        .set_bold()
        .set_font_name("Calibri")
        .set_font_size(11)
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0x2E75B6))
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0x1F4E79))
        .set_align(FormatAlign::Center);

    let data_fmt_white = Format::new()
        .set_font_name("Calibri")
        .set_font_size(11)
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0xD9D9D9));

    let data_fmt_gray = Format::new()
        .set_font_name("Calibri")
        .set_font_size(11)
        .set_background_color(Color::RGB(0xF2F2F2))
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0xD9D9D9));

    let num_fmt_white = Format::new()
        .set_font_name("Calibri")
        .set_font_size(11)
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0xD9D9D9))
        .set_num_format("#,##0.##");

    let num_fmt_gray = Format::new()
        .set_font_name("Calibri")
        .set_font_size(11)
        .set_background_color(Color::RGB(0xF2F2F2))
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0xD9D9D9))
        .set_num_format("#,##0.##");

    for sheet in &content.sheets {
        let worksheet = workbook.add_worksheet();
        worksheet
            .set_name(&sheet.name)
            .map_err(|e| format!("Failed to set sheet name: {e}"))?;

        let num_cols = sheet
            .headers
            .as_ref()
            .map(|h| h.len())
            .unwrap_or_else(|| sheet.rows.first().map(|r| r.len()).unwrap_or(0));

        // Column widths: use explicit, else auto-estimate from headers
        for col in 0..num_cols {
            let width = sheet
                .column_widths
                .as_ref()
                .and_then(|w| w.get(col).copied())
                .unwrap_or_else(|| {
                    let header_len = sheet
                        .headers
                        .as_ref()
                        .and_then(|h| h.get(col))
                        .map(|s| s.len())
                        .unwrap_or(8);
                    (header_len as f64 + 4.0).clamp(10.0, 30.0)
                });
            let _ = worksheet.set_column_width(col as u16, width);
        }

        let mut row_idx: u32 = 0;

        if let Some(headers) = &sheet.headers {
            for (col, header) in headers.iter().enumerate() {
                worksheet
                    .write_string_with_format(row_idx, col as u16, header, &header_fmt)
                    .map_err(|e| format!("Failed to write header: {e}"))?;
            }
            row_idx += 1;

            // Freeze header row
            let _ = worksheet.set_freeze_panes(1, 0);
        }

        for row in &sheet.rows {
            let is_alt = (row_idx % 2) == 0; // alternating; row 0 is header, so data rows 1,2,3...
            let cell_fmt = if is_alt {
                &data_fmt_gray
            } else {
                &data_fmt_white
            };
            let n_fmt = if is_alt {
                &num_fmt_gray
            } else {
                &num_fmt_white
            };

            for (col, value) in row.iter().enumerate() {
                match value {
                    serde_json::Value::Number(n) => {
                        if let Some(f) = n.as_f64() {
                            worksheet
                                .write_number_with_format(row_idx, col as u16, f, n_fmt)
                                .map_err(|e| format!("Failed to write number: {e}"))?;
                        }
                    }
                    serde_json::Value::Bool(b) => {
                        worksheet
                            .write_boolean_with_format(row_idx, col as u16, *b, cell_fmt)
                            .map_err(|e| format!("Failed to write boolean: {e}"))?;
                    }
                    _ => {
                        let s = match value {
                            serde_json::Value::String(s) => s.clone(),
                            _ => value.to_string(),
                        };
                        worksheet
                            .write_string_with_format(row_idx, col as u16, &s, cell_fmt)
                            .map_err(|e| format!("Failed to write string: {e}"))?;
                    }
                }
            }
            row_idx += 1;
        }
    }

    workbook
        .save(path)
        .map_err(|e| format!("Failed to save XLSX: {e}"))?;

    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Ok(size)
}

// ---------------------------------------------------------------------------
// PPTX generation (professional OOXML via zip)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PptxContent {
    slides: Vec<PptxSlide>,
}

#[derive(Deserialize)]
struct PptxSlide {
    title: Option<String>,
    body: Option<String>,
    bullet_items: Option<Vec<String>>,
    layout: Option<String>,
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// EMU constants: 1 inch = 914400 EMU, 1 pt = 12700 EMU
const SLIDE_W: i64 = 9144000; // 10 in
const SLIDE_H: i64 = 6858000; // 7.5 in
const HEADER_H: i64 = 1371600; // 1.5 in header bar height
const MARGIN: i64 = 457200; // 0.5 in

fn pptx_theme_xml() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Professional">
<a:themeElements>
<a:clrScheme name="Professional">
  <a:dk1><a:srgbClr val="000000"/></a:dk1>
  <a:lt1><a:srgbClr val="FFFFFF"/></a:lt1>
  <a:dk2><a:srgbClr val="1F4E79"/></a:dk2>
  <a:lt2><a:srgbClr val="F2F2F2"/></a:lt2>
  <a:accent1><a:srgbClr val="2E75B6"/></a:accent1>
  <a:accent2><a:srgbClr val="4BACC6"/></a:accent2>
  <a:accent3><a:srgbClr val="F79646"/></a:accent3>
  <a:accent4><a:srgbClr val="9BBB59"/></a:accent4>
  <a:accent5><a:srgbClr val="8064A2"/></a:accent5>
  <a:accent6><a:srgbClr val="4F81BD"/></a:accent6>
  <a:hlink><a:srgbClr val="0563C1"/></a:hlink>
  <a:folHlink><a:srgbClr val="954F72"/></a:folHlink>
</a:clrScheme>
<a:fontScheme name="Professional">
  <a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont>
  <a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont>
</a:fontScheme>
<a:fmtScheme name="Professional">
  <a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst>
  <a:lnStyleLst><a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst>
  <a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst>
  <a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst>
</a:fmtScheme>
</a:themeElements>
</a:theme>"#
}

fn build_title_slide(slide: &PptxSlide, slide_num: usize) -> String {
    let title_text = xml_escape(slide.title.as_deref().unwrap_or(""));
    let subtitle_text = xml_escape(slide.body.as_deref().unwrap_or(""));

    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
 xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
 xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<p:cSld>
<p:bg><p:bgPr><a:solidFill><a:srgbClr val="1F4E79"/></a:solidFill><a:effectLst/></p:bgPr></p:bg>
<p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr/>
<p:sp>
  <p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
  <p:spPr>
    <a:xfrm><a:off x="{margin}" y="1714500"/><a:ext cx="{content_w}" cy="1371600"/></a:xfrm>
    <a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/>
  </p:spPr>
  <p:txBody>
    <a:bodyPr anchor="ctr"/>
    <a:p><a:pPr algn="ctr"/>
      <a:r><a:rPr lang="en-US" sz="4400" b="1" dirty="0"><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill><a:latin typeface="Calibri Light"/></a:rPr><a:t>{title_text}</a:t></a:r>
    </a:p>
  </p:txBody>
</p:sp>
<p:sp>
  <p:nvSpPr><p:cNvPr id="3" name="Subtitle"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
  <p:spPr>
    <a:xfrm><a:off x="{margin}" y="3200400"/><a:ext cx="{content_w}" cy="800100"/></a:xfrm>
    <a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/>
  </p:spPr>
  <p:txBody>
    <a:bodyPr anchor="t"/>
    <a:p><a:pPr algn="ctr"/>
      <a:r><a:rPr lang="en-US" sz="2000" dirty="0"><a:solidFill><a:srgbClr val="D6DCE4"/></a:solidFill><a:latin typeface="Calibri"/></a:rPr><a:t>{subtitle_text}</a:t></a:r>
    </a:p>
  </p:txBody>
</p:sp>
<p:sp>
  <p:nvSpPr><p:cNvPr id="4" name="SlideNum"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
  <p:spPr>
    <a:xfrm><a:off x="8229600" y="6400800"/><a:ext cx="685800" cy="365125"/></a:xfrm>
    <a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/>
  </p:spPr>
  <p:txBody><a:bodyPr anchor="ctr"/>
    <a:p><a:pPr algn="r"/>
      <a:r><a:rPr lang="en-US" sz="1000" dirty="0"><a:solidFill><a:srgbClr val="D6DCE4"/></a:solidFill><a:latin typeface="Calibri"/></a:rPr><a:t>{slide_num}</a:t></a:r>
    </a:p>
  </p:txBody>
</p:sp>
</p:spTree>
</p:cSld>
</p:sld>"#,
        margin = MARGIN,
        content_w = SLIDE_W - 2 * MARGIN,
    )
}

fn build_content_slide(slide: &PptxSlide, slide_num: usize) -> String {
    let title_text = xml_escape(slide.title.as_deref().unwrap_or(""));

    // Build body paragraphs from body text and/or bullet_items
    let mut body_parts = String::new();

    if let Some(body_raw) = &slide.body {
        for line in body_raw.split('\n') {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                body_parts.push_str(r#"<a:p><a:endParaRPr lang="en-US" sz="2000"/></a:p>"#);
                continue;
            }
            let (text, is_bullet) = if let Some(t) = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("• "))
            {
                (t, true)
            } else {
                (trimmed, false)
            };
            let escaped = xml_escape(text);
            if is_bullet {
                body_parts.push_str(&format!(
                    r#"<a:p><a:pPr marL="342900" indent="-342900"><a:buChar char="&#x2022;"/></a:pPr><a:r><a:rPr lang="en-US" sz="2000" dirty="0"><a:solidFill><a:srgbClr val="333333"/></a:solidFill><a:latin typeface="Calibri"/></a:rPr><a:t>{escaped}</a:t></a:r></a:p>"#
                ));
            } else {
                body_parts.push_str(&format!(
                    r#"<a:p><a:r><a:rPr lang="en-US" sz="2000" dirty="0"><a:solidFill><a:srgbClr val="333333"/></a:solidFill><a:latin typeface="Calibri"/></a:rPr><a:t>{escaped}</a:t></a:r></a:p>"#
                ));
            }
        }
    }

    if let Some(items) = &slide.bullet_items {
        for item in items {
            let escaped = xml_escape(item);
            body_parts.push_str(&format!(
                r#"<a:p><a:pPr marL="342900" indent="-342900"><a:buChar char="&#x2022;"/></a:pPr><a:r><a:rPr lang="en-US" sz="2000" dirty="0"><a:solidFill><a:srgbClr val="333333"/></a:solidFill><a:latin typeface="Calibri"/></a:rPr><a:t>{escaped}</a:t></a:r></a:p>"#
            ));
        }
    }

    if body_parts.is_empty() {
        body_parts = r#"<a:p><a:endParaRPr lang="en-US"/></a:p>"#.into();
    }

    let content_y = HEADER_H;
    let content_h = SLIDE_H - HEADER_H - MARGIN;

    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
 xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
 xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<p:cSld>
<p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr/>
<p:sp>
  <p:nvSpPr><p:cNvPr id="2" name="HeaderBar"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
  <p:spPr>
    <a:xfrm><a:off x="0" y="0"/><a:ext cx="{slide_w}" cy="{header_h}"/></a:xfrm>
    <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
    <a:solidFill><a:srgbClr val="1F4E79"/></a:solidFill>
  </p:spPr>
  <p:txBody>
    <a:bodyPr anchor="ctr" lIns="457200"/>
    <a:p><a:pPr algn="l"/>
      <a:r><a:rPr lang="en-US" sz="3600" b="1" dirty="0"><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill><a:latin typeface="Calibri Light"/></a:rPr><a:t>{title_text}</a:t></a:r>
    </a:p>
  </p:txBody>
</p:sp>
<p:sp>
  <p:nvSpPr><p:cNvPr id="3" name="Content"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
  <p:spPr>
    <a:xfrm><a:off x="{margin}" y="{content_y}"/><a:ext cx="{content_w}" cy="{content_h}"/></a:xfrm>
    <a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/>
  </p:spPr>
  <p:txBody>
    <a:bodyPr anchor="t" lIns="91440" tIns="91440" rIns="91440" bIns="45720"/>
    <a:lstStyle/>
    {body_parts}
  </p:txBody>
</p:sp>
<p:sp>
  <p:nvSpPr><p:cNvPr id="4" name="SlideNum"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
  <p:spPr>
    <a:xfrm><a:off x="8229600" y="6400800"/><a:ext cx="685800" cy="365125"/></a:xfrm>
    <a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/>
  </p:spPr>
  <p:txBody><a:bodyPr anchor="ctr"/>
    <a:p><a:pPr algn="r"/>
      <a:r><a:rPr lang="en-US" sz="1000" dirty="0"><a:solidFill><a:srgbClr val="888888"/></a:solidFill><a:latin typeface="Calibri"/></a:rPr><a:t>{slide_num}</a:t></a:r>
    </a:p>
  </p:txBody>
</p:sp>
</p:spTree>
</p:cSld>
</p:sld>"#,
        slide_w = SLIDE_W,
        header_h = HEADER_H,
        margin = MARGIN,
        content_w = SLIDE_W - 2 * MARGIN,
    )
}

fn generate_pptx(path: &std::path::Path, content: &serde_json::Value) -> Result<u64, String> {
    let content: PptxContent = serde_json::from_value(content.clone())
        .map_err(|e| format!("Invalid PPTX content: {e}"))?;

    if content.slides.is_empty() {
        return Err("PPTX content must have at least one slide.".into());
    }

    let file = std::fs::File::create(path).map_err(|e| format!("Failed to create file: {e}"))?;

    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // [Content_Types].xml
    let mut content_types = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\n\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\n\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\n\
         <Override PartName=\"/ppt/presentation.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml\"/>\n\
         <Override PartName=\"/ppt/theme/theme1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.theme+xml\"/>\n",
    );
    for i in 1..=content.slides.len() {
        content_types.push_str(&format!(
            "<Override PartName=\"/ppt/slides/slide{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>\n"
        ));
    }
    content_types.push_str("</Types>");

    zip.start_file("[Content_Types].xml", options)
        .map_err(|e| format!("ZIP error: {e}"))?;
    zip.write_all(content_types.as_bytes())
        .map_err(|e| format!("Write error: {e}"))?;

    // _rels/.rels
    zip.start_file("_rels/.rels", options)
        .map_err(|e| format!("ZIP error: {e}"))?;
    zip.write_all(
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
          <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n\
          <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"ppt/presentation.xml\"/>\n\
          </Relationships>",
    )
    .map_err(|e| format!("Write error: {e}"))?;

    // ppt/theme/theme1.xml
    zip.start_file("ppt/theme/theme1.xml", options)
        .map_err(|e| format!("ZIP error: {e}"))?;
    zip.write_all(pptx_theme_xml().as_bytes())
        .map_err(|e| format!("Write error: {e}"))?;

    // ppt/presentation.xml
    let mut pres = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <p:presentation xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
         xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\">\n\
         <p:sldIdLst>\n",
    );
    for i in 1..=content.slides.len() {
        pres.push_str(&format!(
            "<p:sldId id=\"{}\" r:id=\"rId{}\"/>\n",
            255 + i,
            i + 2 // rId1=theme, rId2+=slides
        ));
    }
    pres.push_str(&format!(
        "</p:sldIdLst>\n\
         <p:sldSz cx=\"{SLIDE_W}\" cy=\"{SLIDE_H}\"/>\n\
         <p:notesSz cx=\"{SLIDE_H}\" cy=\"{SLIDE_W}\"/>\n\
         </p:presentation>"
    ));

    zip.start_file("ppt/presentation.xml", options)
        .map_err(|e| format!("ZIP error: {e}"))?;
    zip.write_all(pres.as_bytes())
        .map_err(|e| format!("Write error: {e}"))?;

    // ppt/_rels/presentation.xml.rels
    let mut pres_rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n\
         <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme\" Target=\"theme/theme1.xml\"/>\n",
    );
    for i in 1..=content.slides.len() {
        pres_rels.push_str(&format!(
            "<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide{i}.xml\"/>\n",
            i + 1
        ));
    }
    pres_rels.push_str("</Relationships>");

    zip.start_file("ppt/_rels/presentation.xml.rels", options)
        .map_err(|e| format!("ZIP error: {e}"))?;
    zip.write_all(pres_rels.as_bytes())
        .map_err(|e| format!("Write error: {e}"))?;

    // Individual slides
    for (i, slide) in content.slides.iter().enumerate() {
        let slide_num = i + 1;
        let is_title_layout = slide
            .layout
            .as_deref()
            .map(|l| l == "title")
            .unwrap_or(i == 0 && content.slides.len() > 1);

        let slide_xml = if is_title_layout {
            build_title_slide(slide, slide_num)
        } else {
            build_content_slide(slide, slide_num)
        };

        zip.start_file(format!("ppt/slides/slide{slide_num}.xml"), options)
            .map_err(|e| format!("ZIP error: {e}"))?;
        zip.write_all(slide_xml.as_bytes())
            .map_err(|e| format!("Write error: {e}"))?;
    }

    zip.finish()
        .map_err(|e| format!("ZIP finalize error: {e}"))?;

    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Ok(size)
}

// ---------------------------------------------------------------------------
// Tool implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Tool for GenerateDocumentTool {
    fn name(&self) -> &str {
        "generate_document"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::FileSystem]
    }

    fn requires_confirmation(&self, _args: &serde_json::Value) -> bool {
        true
    }

    fn confirmation_message(&self, args: &serde_json::Value) -> Option<String> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("<unknown>");
        let fmt = args.get("format").and_then(|v| v.as_str()).unwrap_or("?");
        Some(format!("Generate {fmt} document: {path}"))
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        _source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: GenerateDocArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid generate_document arguments: {e}"))
        })?;

        if has_path_traversal(&args.path) {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "Path must not contain '..' traversal sequences.".into(),
                is_error: true,
                artifacts: None,
            });
        }

        let format = args.format.to_lowercase();
        if !matches!(format.as_str(), "docx" | "xlsx" | "pptx") {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: format!("Unsupported format '{format}'. Use docx, xlsx, or pptx."),
                is_error: true,
                artifacts: None,
            });
        }

        let db = db.clone();
        let call_id = call_id.to_string();
        let content = args.content;
        let path_str = args.path;

        tokio::task::spawn_blocking(move || {
            let sources = db.list_sources()?;
            if sources.is_empty() {
                return Ok(ToolResult {
                    call_id: call_id.clone(),
                    content: "No sources registered. Add a source directory first.".into(),
                    is_error: true,
                    artifacts: None,
                });
            }

            let requested = PathBuf::from(&path_str);
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

            // Create parent directories if needed.
            if let Some(parent) = canonical.parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent).map_err(CoreError::Io)?;
                }
            }

            let result = match format.as_str() {
                "docx" => generate_docx(&canonical, &content),
                "xlsx" => generate_xlsx(&canonical, &content),
                "pptx" => generate_pptx(&canonical, &content),
                _ => unreachable!(),
            };

            match result {
                Ok(size) => Ok(ToolResult {
                    call_id,
                    content: format!(
                        "Generated {format} document '{}' ({size} bytes).\nPath: {}",
                        path_str,
                        canonical.display()
                    ),
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
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?
    }
}
