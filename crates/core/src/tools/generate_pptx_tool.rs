//! GeneratePptxTool — creates professional PPTX presentations via hand-crafted OOXML + `zip`.

use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::create_file_tool::{has_path_traversal, resolve_and_validate};
use super::{scoped_sources, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/generate_pptx.json");

pub struct GeneratePptxTool;

#[derive(Deserialize)]
struct GeneratePptxArgs {
    path: String,
    content: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

fn normalized_hex(input: Option<&str>, fallback: &str) -> String {
    input
        .map(str::trim)
        .map(|value| value.trim_start_matches('#'))
        .filter(|value| value.len() == 6 && value.chars().all(|ch| ch.is_ascii_hexdigit()))
        .map(|value| value.to_ascii_uppercase())
        .unwrap_or_else(|| fallback.to_string())
}

#[derive(Deserialize, Clone, Default)]
struct PptxTheme {
    primary_color: Option<String>,
    accent_color: Option<String>,
    background_color: Option<String>,
    title_color: Option<String>,
    text_color: Option<String>,
    title_font: Option<String>,
    body_font: Option<String>,
}

#[derive(Deserialize)]
struct PptxContent {
    theme: Option<PptxTheme>,
    slides: Vec<PptxSlide>,
}

#[derive(Deserialize)]
struct PptxSlide {
    title: Option<String>,
    subtitle: Option<String>,
    body: Option<String>,
    bullet_items: Option<Vec<String>>,
    layout: Option<String>,
    notes: Option<String>,
    table: Option<PptxTable>,
    left_title: Option<String>,
    left_body: Option<String>,
    left_bullet_items: Option<Vec<String>>,
    right_title: Option<String>,
    right_body: Option<String>,
    right_bullet_items: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct PptxTable {
    headers: Option<Vec<String>>,
    rows: Vec<Vec<String>>,
}

#[derive(Clone)]
struct ResolvedPptxTheme {
    primary_color: String,
    accent_color: String,
    background_color: String,
    title_color: String,
    text_color: String,
    muted_text_color: String,
    title_font: String,
    body_font: String,
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn resolve_pptx_theme(theme: Option<&PptxTheme>) -> ResolvedPptxTheme {
    let theme = theme.cloned().unwrap_or_default();
    ResolvedPptxTheme {
        primary_color: normalized_hex(theme.primary_color.as_deref(), "1F4E79"),
        accent_color: normalized_hex(theme.accent_color.as_deref(), "2E75B6"),
        background_color: normalized_hex(theme.background_color.as_deref(), "FFFFFF"),
        title_color: normalized_hex(theme.title_color.as_deref(), "FFFFFF"),
        text_color: normalized_hex(theme.text_color.as_deref(), "333333"),
        muted_text_color: "D6DCE4".to_string(),
        title_font: theme
            .title_font
            .unwrap_or_else(|| "Calibri Light".to_string()),
        body_font: theme.body_font.unwrap_or_else(|| "Calibri".to_string()),
    }
}

fn build_pptx_body_parts(
    body: Option<&str>,
    bullet_items: Option<&[String]>,
    theme: &ResolvedPptxTheme,
) -> String {
    let mut body_parts = String::new();

    if let Some(body_raw) = body {
        for line in body_raw.split('\n') {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                body_parts.push_str(r#"<a:p><a:endParaRPr lang="en-US" sz="2000"/></a:p>"#);
                continue;
            }
            let (text, is_bullet) = if let Some(t) = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("\u{2022} "))
            {
                (t, true)
            } else {
                (trimmed, false)
            };
            let escaped = xml_escape(text);
            if is_bullet {
                body_parts.push_str(&format!(
                    r#"<a:p><a:pPr marL="342900" indent="-342900"><a:buChar char="&#x2022;"/></a:pPr><a:r><a:rPr lang="en-US" sz="2000" dirty="0"><a:solidFill><a:srgbClr val="{text_color}"/></a:solidFill><a:latin typeface="{body_font}"/></a:rPr><a:t>{escaped}</a:t></a:r></a:p>"#,
                    text_color = theme.text_color,
                    body_font = xml_escape(&theme.body_font)
                ));
            } else {
                body_parts.push_str(&format!(
                    r#"<a:p><a:r><a:rPr lang="en-US" sz="2000" dirty="0"><a:solidFill><a:srgbClr val="{text_color}"/></a:solidFill><a:latin typeface="{body_font}"/></a:rPr><a:t>{escaped}</a:t></a:r></a:p>"#,
                    text_color = theme.text_color,
                    body_font = xml_escape(&theme.body_font)
                ));
            }
        }
    }

    if let Some(items) = bullet_items {
        for item in items {
            let escaped = xml_escape(item);
            body_parts.push_str(&format!(
                r#"<a:p><a:pPr marL="342900" indent="-342900"><a:buChar char="&#x2022;"/></a:pPr><a:r><a:rPr lang="en-US" sz="2000" dirty="0"><a:solidFill><a:srgbClr val="{text_color}"/></a:solidFill><a:latin typeface="{body_font}"/></a:rPr><a:t>{escaped}</a:t></a:r></a:p>"#,
                text_color = theme.text_color,
                body_font = xml_escape(&theme.body_font)
            ));
        }
    }

    if body_parts.is_empty() {
        body_parts = r#"<a:p><a:endParaRPr lang="en-US"/></a:p>"#.into();
    }

    body_parts
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

// ---------------------------------------------------------------------------
// Table XML builder
// ---------------------------------------------------------------------------

fn build_pptx_table_xml(table: &PptxTable, theme: &ResolvedPptxTheme) -> String {
    let num_cols = table
        .headers
        .as_ref()
        .map(|h| h.len())
        .unwrap_or_else(|| table.rows.iter().map(|r| r.len()).max().unwrap_or(0));
    if num_cols == 0 {
        return String::new();
    }

    let table_w = SLIDE_W - 2 * MARGIN;
    let col_w = table_w / num_cols as i64;
    let num_rows = table.rows.len() + if table.headers.is_some() { 1 } else { 0 };
    let table_h = num_rows as i64 * 370840; // ~0.4in per row
    let table_y = HEADER_H + MARGIN;
    let body_font = xml_escape(&theme.body_font);

    let mut xml = String::new();
    xml.push_str(&format!(
        r#"<p:graphicFrame>
  <p:nvGraphicFramePr><p:cNvPr id="100" name="Table"/><p:cNvGraphicFramePr><a:graphicFrameLocks noGrp="1"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr>
  <p:xfrm><a:off x="{margin}" y="{table_y}"/><a:ext cx="{table_w}" cy="{table_h}"/></p:xfrm>
  <a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table">
  <a:tbl>
  <a:tblPr firstRow="1" bandRow="1"><a:noFill/></a:tblPr>
  <a:tblGrid>"#,
        margin = MARGIN,
    ));

    for _ in 0..num_cols {
        xml.push_str(&format!("<a:gridCol w=\"{col_w}\"/>"));
    }
    xml.push_str("</a:tblGrid>");

    // Header row
    if let Some(headers) = &table.headers {
        xml.push_str("<a:tr h=\"370840\">");
        for header in headers {
            let escaped = xml_escape(header);
            xml.push_str(&format!(
                r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="1400" b="1" dirty="0"><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill><a:latin typeface="{body_font}"/></a:rPr><a:t>{escaped}</a:t></a:r></a:p></a:txBody><a:tcPr><a:solidFill><a:srgbClr val="{primary}"/></a:solidFill></a:tcPr></a:tc>"#,
                primary = theme.primary_color,
            ));
        }
        xml.push_str("</a:tr>");
    }

    // Data rows
    for (row_idx, row) in table.rows.iter().enumerate() {
        let bg = if row_idx % 2 == 0 { "F2F2F2" } else { "FFFFFF" };
        xml.push_str("<a:tr h=\"370840\">");
        for (col_idx, cell) in row.iter().enumerate().take(num_cols) {
            let escaped = xml_escape(cell);
            xml.push_str(&format!(
                r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="1400" dirty="0"><a:solidFill><a:srgbClr val="{text_color}"/></a:solidFill><a:latin typeface="{body_font}"/></a:rPr><a:t>{escaped}</a:t></a:r></a:p></a:txBody><a:tcPr><a:solidFill><a:srgbClr val="{bg}"/></a:solidFill></a:tcPr></a:tc>"#,
                text_color = theme.text_color,
            ));
            let _ = col_idx; // silence unused
        }
        // Fill remaining columns if row is short
        for _ in row.len()..num_cols {
            xml.push_str(&format!(
                r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang="en-US"/></a:p></a:txBody><a:tcPr><a:solidFill><a:srgbClr val="{bg}"/></a:solidFill></a:tcPr></a:tc>"#,
            ));
        }
        xml.push_str("</a:tr>");
    }

    xml.push_str("</a:tbl></a:graphicData></a:graphic></p:graphicFrame>");
    xml
}

// ---------------------------------------------------------------------------
// Speaker notes XML
// ---------------------------------------------------------------------------

fn build_notes_xml(_slide_num: usize, notes_text: &str, theme: &ResolvedPptxTheme) -> String {
    let escaped = xml_escape(notes_text);
    let body_font = xml_escape(&theme.body_font);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:notes xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
 xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
 xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<p:cSld>
<p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr/>
<p:sp>
  <p:nvSpPr><p:cNvPr id="2" name="Slide Image"/><p:cNvSpPr><a:spLocks noGrp="1" noRot="1" noChangeAspect="1"/></p:cNvSpPr><p:nvPr><p:ph type="sldImg"/></p:nvPr></p:nvSpPr>
  <p:spPr/>
</p:sp>
<p:sp>
  <p:nvSpPr><p:cNvPr id="3" name="Notes"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr>
  <p:spPr/>
  <p:txBody>
    <a:bodyPr/>
    <a:lstStyle/>
    <a:p><a:r><a:rPr lang="en-US" sz="1200" dirty="0"><a:latin typeface="{body_font}"/></a:rPr><a:t>{escaped}</a:t></a:r></a:p>
  </p:txBody>
</p:sp>
</p:spTree>
</p:cSld>
</p:notes>"#
    )
}

// ---------------------------------------------------------------------------
// Slide builders
// ---------------------------------------------------------------------------

fn build_title_slide(slide: &PptxSlide, slide_num: usize, theme: &ResolvedPptxTheme) -> String {
    let title_text = xml_escape(slide.title.as_deref().unwrap_or(""));
    let subtitle_text = xml_escape(
        slide
            .subtitle
            .as_deref()
            .or(slide.body.as_deref())
            .unwrap_or(""),
    );
    let title_font = xml_escape(&theme.title_font);
    let body_font = xml_escape(&theme.body_font);

    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
 xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
 xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<p:cSld>
<p:bg><p:bgPr><a:solidFill><a:srgbClr val="{primary_color}"/></a:solidFill><a:effectLst/></p:bgPr></p:bg>
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
      <a:r><a:rPr lang="en-US" sz="4400" b="1" dirty="0"><a:solidFill><a:srgbClr val="{title_color}"/></a:solidFill><a:latin typeface="{title_font}"/></a:rPr><a:t>{title_text}</a:t></a:r>
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
      <a:r><a:rPr lang="en-US" sz="2000" dirty="0"><a:solidFill><a:srgbClr val="{muted_text_color}"/></a:solidFill><a:latin typeface="{body_font}"/></a:rPr><a:t>{subtitle_text}</a:t></a:r>
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
      <a:r><a:rPr lang="en-US" sz="1000" dirty="0"><a:solidFill><a:srgbClr val="{muted_text_color}"/></a:solidFill><a:latin typeface="{body_font}"/></a:rPr><a:t>{slide_num}</a:t></a:r>
    </a:p>
  </p:txBody>
</p:sp>
</p:spTree>
</p:cSld>
</p:sld>"#,
        primary_color = theme.primary_color,
        title_color = theme.title_color,
        muted_text_color = theme.muted_text_color,
        title_font = title_font,
        body_font = body_font,
        margin = MARGIN,
        content_w = SLIDE_W - 2 * MARGIN,
    )
}

fn build_section_slide(slide: &PptxSlide, slide_num: usize, theme: &ResolvedPptxTheme) -> String {
    build_title_slide(slide, slide_num, theme)
}

fn build_content_slide(slide: &PptxSlide, slide_num: usize, theme: &ResolvedPptxTheme) -> String {
    let title_text = xml_escape(slide.title.as_deref().unwrap_or(""));
    let title_font = xml_escape(&theme.title_font);
    let body_font = xml_escape(&theme.body_font);

    let body_parts =
        build_pptx_body_parts(slide.body.as_deref(), slide.bullet_items.as_deref(), theme);

    // Table XML (placed after body text area if present)
    let table_xml = slide
        .table
        .as_ref()
        .map(|t| build_pptx_table_xml(t, theme))
        .unwrap_or_default();

    let content_y = HEADER_H;
    let content_h = if table_xml.is_empty() {
        SLIDE_H - HEADER_H - MARGIN
    } else {
        // Shrink body area to make room for table
        (SLIDE_H - HEADER_H - MARGIN) / 2
    };

    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
 xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
 xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<p:cSld>
<p:bg><p:bgPr><a:solidFill><a:srgbClr val="{background_color}"/></a:solidFill><a:effectLst/></p:bgPr></p:bg>
<p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr/>
<p:sp>
  <p:nvSpPr><p:cNvPr id="2" name="HeaderBar"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
  <p:spPr>
    <a:xfrm><a:off x="0" y="0"/><a:ext cx="{slide_w}" cy="{header_h}"/></a:xfrm>
    <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
    <a:solidFill><a:srgbClr val="{primary_color}"/></a:solidFill>
  </p:spPr>
  <p:txBody>
    <a:bodyPr anchor="ctr" lIns="457200"/>
    <a:p><a:pPr algn="l"/>
      <a:r><a:rPr lang="en-US" sz="3600" b="1" dirty="0"><a:solidFill><a:srgbClr val="{title_color}"/></a:solidFill><a:latin typeface="{title_font}"/></a:rPr><a:t>{title_text}</a:t></a:r>
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
{table_xml}
<p:sp>
  <p:nvSpPr><p:cNvPr id="4" name="SlideNum"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
  <p:spPr>
    <a:xfrm><a:off x="8229600" y="6400800"/><a:ext cx="685800" cy="365125"/></a:xfrm>
    <a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/>
  </p:spPr>
  <p:txBody><a:bodyPr anchor="ctr"/>
    <a:p><a:pPr algn="r"/>
      <a:r><a:rPr lang="en-US" sz="1000" dirty="0"><a:solidFill><a:srgbClr val="888888"/></a:solidFill><a:latin typeface="{body_font}"/></a:rPr><a:t>{slide_num}</a:t></a:r>
    </a:p>
  </p:txBody>
</p:sp>
</p:spTree>
</p:cSld>
</p:sld>"#,
        background_color = theme.background_color,
        slide_w = SLIDE_W,
        header_h = HEADER_H,
        primary_color = theme.primary_color,
        title_color = theme.title_color,
        title_font = title_font,
        margin = MARGIN,
        content_w = SLIDE_W - 2 * MARGIN,
        body_font = body_font,
    )
}

fn build_comparison_slide(
    slide: &PptxSlide,
    slide_num: usize,
    theme: &ResolvedPptxTheme,
) -> String {
    let title_text = xml_escape(slide.title.as_deref().unwrap_or(""));
    let left_title = xml_escape(slide.left_title.as_deref().unwrap_or("Option A"));
    let right_title = xml_escape(slide.right_title.as_deref().unwrap_or("Option B"));
    let left_body = build_pptx_body_parts(
        slide.left_body.as_deref(),
        slide.left_bullet_items.as_deref(),
        theme,
    );
    let right_body = build_pptx_body_parts(
        slide.right_body.as_deref(),
        slide.right_bullet_items.as_deref(),
        theme,
    );
    let title_font = xml_escape(&theme.title_font);
    let body_font = xml_escape(&theme.body_font);
    let column_gap = 182880;
    let usable_w = SLIDE_W - (2 * MARGIN);
    let column_w = (usable_w - column_gap) / 2;
    let left_title_y = HEADER_H + 182880;
    let body_y = HEADER_H + 640080;
    let body_h = SLIDE_H - HEADER_H - 914400;
    let right_x = MARGIN + column_w + column_gap;
    let divider_x = MARGIN + column_w + (column_gap / 2);

    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
 xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
 xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<p:cSld>
<p:bg><p:bgPr><a:solidFill><a:srgbClr val="{background_color}"/></a:solidFill><a:effectLst/></p:bgPr></p:bg>
<p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr/>
<p:sp>
  <p:nvSpPr><p:cNvPr id="2" name="HeaderBar"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
  <p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{slide_w}" cy="{header_h}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="{primary_color}"/></a:solidFill></p:spPr>
  <p:txBody><a:bodyPr anchor="ctr" lIns="457200"/><a:p><a:pPr algn="l"/><a:r><a:rPr lang="en-US" sz="3400" b="1" dirty="0"><a:solidFill><a:srgbClr val="{title_color}"/></a:solidFill><a:latin typeface="{title_font}"/></a:rPr><a:t>{title_text}</a:t></a:r></a:p></p:txBody>
</p:sp>
<p:sp>
  <p:nvSpPr><p:cNvPr id="3" name="LeftTitle"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
  <p:spPr><a:xfrm><a:off x="{margin}" y="{left_title_y}"/><a:ext cx="{column_w}" cy="457200"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr>
  <p:txBody><a:bodyPr anchor="ctr"/><a:p><a:r><a:rPr lang="en-US" sz="2200" b="1" dirty="0"><a:solidFill><a:srgbClr val="{accent_color}"/></a:solidFill><a:latin typeface="{title_font}"/></a:rPr><a:t>{left_title}</a:t></a:r></a:p></p:txBody>
</p:sp>
<p:sp>
  <p:nvSpPr><p:cNvPr id="4" name="RightTitle"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
  <p:spPr><a:xfrm><a:off x="{right_x}" y="{left_title_y}"/><a:ext cx="{column_w}" cy="457200"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr>
  <p:txBody><a:bodyPr anchor="ctr"/><a:p><a:r><a:rPr lang="en-US" sz="2200" b="1" dirty="0"><a:solidFill><a:srgbClr val="{accent_color}"/></a:solidFill><a:latin typeface="{title_font}"/></a:rPr><a:t>{right_title}</a:t></a:r></a:p></p:txBody>
</p:sp>
<p:sp>
  <p:nvSpPr><p:cNvPr id="5" name="LeftBody"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
  <p:spPr><a:xfrm><a:off x="{margin}" y="{body_y}"/><a:ext cx="{column_w}" cy="{body_h}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr>
  <p:txBody><a:bodyPr anchor="t" lIns="91440" tIns="91440" rIns="91440" bIns="45720"/><a:lstStyle/>{left_body}</p:txBody>
</p:sp>
<p:sp>
  <p:nvSpPr><p:cNvPr id="6" name="RightBody"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
  <p:spPr><a:xfrm><a:off x="{right_x}" y="{body_y}"/><a:ext cx="{column_w}" cy="{body_h}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr>
  <p:txBody><a:bodyPr anchor="t" lIns="91440" tIns="91440" rIns="91440" bIns="45720"/><a:lstStyle/>{right_body}</p:txBody>
</p:sp>
<p:sp>
  <p:nvSpPr><p:cNvPr id="7" name="Divider"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
  <p:spPr><a:xfrm><a:off x="{divider_x}" y="{body_y}"/><a:ext cx="12700" cy="{body_h}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="{accent_color}"/></a:solidFill></p:spPr>
</p:sp>
<p:sp>
  <p:nvSpPr><p:cNvPr id="8" name="SlideNum"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
  <p:spPr><a:xfrm><a:off x="8229600" y="6400800"/><a:ext cx="685800" cy="365125"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr>
  <p:txBody><a:bodyPr anchor="ctr"/><a:p><a:pPr algn="r"/><a:r><a:rPr lang="en-US" sz="1000" dirty="0"><a:solidFill><a:srgbClr val="888888"/></a:solidFill><a:latin typeface="{body_font}"/></a:rPr><a:t>{slide_num}</a:t></a:r></a:p></p:txBody>
</p:sp>
</p:spTree>
</p:cSld>
</p:sld>"#,
        background_color = theme.background_color,
        slide_w = SLIDE_W,
        header_h = HEADER_H,
        primary_color = theme.primary_color,
        title_color = theme.title_color,
        accent_color = theme.accent_color,
        title_font = title_font,
        body_font = body_font,
        title_text = title_text,
        margin = MARGIN,
        left_title_y = left_title_y,
        body_y = body_y,
        body_h = body_h,
        column_w = column_w,
        right_x = right_x,
        divider_x = divider_x,
        left_title = left_title,
        right_title = right_title,
        left_body = left_body,
        right_body = right_body,
    )
}

// ---------------------------------------------------------------------------
// PPTX generation
// ---------------------------------------------------------------------------

pub(crate) fn generate_pptx(
    path: &std::path::Path,
    content: &serde_json::Value,
) -> Result<u64, String> {
    let content: PptxContent = serde_json::from_value(content.clone())
        .map_err(|e| format!("Invalid PPTX content: {e}"))?;
    let theme = resolve_pptx_theme(content.theme.as_ref());

    if content.slides.is_empty() {
        return Err("PPTX content must have at least one slide.".into());
    }

    let file = std::fs::File::create(path).map_err(|e| format!("Failed to create file: {e}"))?;

    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // Collect which slides have notes
    let slides_with_notes: Vec<usize> = content
        .slides
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            if s.notes.as_ref().is_some_and(|n| !n.is_empty()) {
                Some(i + 1)
            } else {
                None
            }
        })
        .collect();

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
    for &slide_num in &slides_with_notes {
        content_types.push_str(&format!(
            "<Override PartName=\"/ppt/notesSlides/notesSlide{slide_num}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml\"/>\n"
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
            i + 2
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

    // Individual slides + notes
    for (i, slide) in content.slides.iter().enumerate() {
        let slide_num = i + 1;
        let is_title_layout = slide
            .layout
            .as_deref()
            .map(|l| l == "title")
            .unwrap_or(i == 0 && content.slides.len() > 1);

        let slide_xml = if is_title_layout {
            build_title_slide(slide, slide_num, &theme)
        } else if matches!(slide.layout.as_deref(), Some("section")) {
            build_section_slide(slide, slide_num, &theme)
        } else if matches!(slide.layout.as_deref(), Some("comparison")) {
            build_comparison_slide(slide, slide_num, &theme)
        } else {
            build_content_slide(slide, slide_num, &theme)
        };

        zip.start_file(format!("ppt/slides/slide{slide_num}.xml"), options)
            .map_err(|e| format!("ZIP error: {e}"))?;
        zip.write_all(slide_xml.as_bytes())
            .map_err(|e| format!("Write error: {e}"))?;

        // Slide .rels for notes relationship
        if slide.notes.as_ref().is_some_and(|n| !n.is_empty()) {
            let slide_rels = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
                 <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n\
                 <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide\" Target=\"../notesSlides/notesSlide{slide_num}.xml\"/>\n\
                 </Relationships>"
            );
            zip.start_file(
                format!("ppt/slides/_rels/slide{slide_num}.xml.rels"),
                options,
            )
            .map_err(|e| format!("ZIP error: {e}"))?;
            zip.write_all(slide_rels.as_bytes())
                .map_err(|e| format!("Write error: {e}"))?;

            // Notes slide
            let notes_xml =
                build_notes_xml(slide_num, slide.notes.as_deref().unwrap_or(""), &theme);
            zip.start_file(
                format!("ppt/notesSlides/notesSlide{slide_num}.xml"),
                options,
            )
            .map_err(|e| format!("ZIP error: {e}"))?;
            zip.write_all(notes_xml.as_bytes())
                .map_err(|e| format!("Write error: {e}"))?;

            // Notes slide .rels (back-reference to slide)
            let notes_rels = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
                 <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n\
                 <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"../slides/slide{slide_num}.xml\"/>\n\
                 </Relationships>"
            );
            zip.start_file(
                format!("ppt/notesSlides/_rels/notesSlide{slide_num}.xml.rels"),
                options,
            )
            .map_err(|e| format!("ZIP error: {e}"))?;
            zip.write_all(notes_rels.as_bytes())
                .map_err(|e| format!("Write error: {e}"))?;
        }
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
impl Tool for GeneratePptxTool {
    fn name(&self) -> &str {
        "generate_pptx"
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
        Some(format!("Generate PPTX presentation: {path}"))
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: GeneratePptxArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid generate_pptx arguments: {e}"))
        })?;

        if has_path_traversal(&args.path) {
            return Ok(ToolResult {
                call_id: call_id.to_string(),
                content: "Path must not contain '..' traversal sequences.".into(),
                is_error: true,
                artifacts: None,
            });
        }

        let db = db.clone();
        let call_id = call_id.to_string();
        let content = args.content;
        let path_str = args.path;
        let source_scope = source_scope.to_vec();

        tokio::task::spawn_blocking(move || {
            let sources = scoped_sources(&db, &source_scope)?;
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

            if let Some(parent) = canonical.parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent).map_err(CoreError::Io)?;
                }
            }

            match generate_pptx(&canonical, &content) {
                Ok(size) => Ok(ToolResult {
                    call_id,
                    content: format!(
                        "Generated PPTX presentation '{}' ({size} bytes).\nPath: {}",
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
