//! GenerateDocumentTool — creates Office documents (DOCX, XLSX, PPTX).

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
const DEF_JSON: &str = include_str!("../../prompts/tools/generate_document.json");

pub struct GenerateDocumentTool;

#[derive(Deserialize)]
struct GenerateDocArgs {
    format: String,
    path: String,
    content: serde_json::Value,
}

fn normalized_hex(input: Option<&str>, fallback: &str) -> String {
    input
        .map(str::trim)
        .map(|value| value.trim_start_matches('#'))
        .filter(|value| value.len() == 6 && value.chars().all(|ch| ch.is_ascii_hexdigit()))
        .map(|value| value.to_ascii_uppercase())
        .unwrap_or_else(|| fallback.to_string())
}

fn value_to_display_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(v) => v.to_string(),
        serde_json::Value::Number(v) => v.to_string(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// DOCX generation (via docx-rs)
// ---------------------------------------------------------------------------

#[derive(Deserialize, Clone, Default)]
struct DocxTheme {
    primary_color: Option<String>,
    accent_color: Option<String>,
    title_font: Option<String>,
    body_font: Option<String>,
    title_align: Option<String>,
}

#[derive(Deserialize)]
struct DocxContent {
    title: Option<String>,
    subtitle: Option<String>,
    cover_note: Option<String>,
    theme: Option<DocxTheme>,
    sections: Vec<DocxSection>,
}

#[derive(Deserialize)]
struct DocxSection {
    heading: Option<String>,
    body: Option<String>,
    bullet_items: Option<Vec<String>>,
    callout: Option<DocxCallout>,
    table: Option<DocxTable>,
    page_break_before: Option<bool>,
}

#[derive(Deserialize)]
struct DocxCallout {
    title: Option<String>,
    body: String,
    tone: Option<String>,
}

#[derive(Deserialize)]
struct DocxTable {
    title: Option<String>,
    headers: Option<Vec<String>>,
    rows: Vec<Vec<String>>,
    column_widths: Option<Vec<usize>>,
}

fn generate_docx(path: &std::path::Path, content: &serde_json::Value) -> Result<u64, String> {
    use docx_rs::*;

    let content: DocxContent = serde_json::from_value(content.clone())
        .map_err(|e| format!("Invalid DOCX content: {e}"))?;

    let theme = content.theme.clone().unwrap_or_default();
    let primary_color = normalized_hex(theme.primary_color.as_deref(), "1F4E79");
    let accent_color = normalized_hex(theme.accent_color.as_deref(), "2E75B6");
    let title_font_name = theme.title_font.as_deref().unwrap_or("Calibri Light");
    let body_font_name = theme.body_font.as_deref().unwrap_or("Calibri");
    let title_alignment = match theme
        .title_align
        .as_deref()
        .unwrap_or("center")
        .to_ascii_lowercase()
        .as_str()
    {
        "left" => AlignmentType::Left,
        "right" => AlignmentType::Right,
        _ => AlignmentType::Center,
    };

    let run_fonts = |family: &str| {
        RunFonts::new()
            .ascii(family)
            .hi_ansi(family)
            .east_asia(family)
            .cs(family)
    };

    let body_fonts = run_fonts(body_font_name);
    let title_fonts = run_fonts(title_font_name);

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
                .align(title_alignment)
                .line_spacing(LineSpacing::new().after(200))
                .add_run(
                    Run::new()
                        .add_text(title)
                        .bold()
                        .size(56) // 28pt = 56 half-points
                        .color(primary_color.clone())
                        .fonts(title_fonts.clone()),
                ),
        );
        if let Some(subtitle) = &content.subtitle {
            doc = doc.add_paragraph(
                Paragraph::new()
                    .align(title_alignment)
                    .line_spacing(LineSpacing::new().after(160))
                    .add_run(
                        Run::new()
                            .add_text(subtitle)
                            .italic()
                            .size(24)
                            .color(accent_color.clone())
                            .fonts(body_fonts.clone()),
                    ),
            );
        }
        if let Some(cover_note) = &content.cover_note {
            doc = doc.add_paragraph(
                Paragraph::new()
                    .align(title_alignment)
                    .line_spacing(LineSpacing::new().after(220))
                    .add_run(
                        Run::new()
                            .add_text(cover_note)
                            .size(20)
                            .color("6B7280")
                            .fonts(body_fonts.clone()),
                    ),
            );
        }
        doc = doc.add_paragraph(
            Paragraph::new()
                .line_spacing(LineSpacing::new().after(220).before(0))
                .add_run(
                    Run::new()
                        .add_text("________________________________________")
                        .size(16)
                        .color(accent_color.clone())
                        .fonts(body_fonts.clone()),
                ),
        );
    }

    for section in &content.sections {
        if section.page_break_before.unwrap_or(false) {
            doc =
                doc.add_paragraph(Paragraph::new().add_run(Run::new().add_break(BreakType::Page)));
        }

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
                            .color(accent_color.clone())
                            .fonts(title_fonts.clone()),
                    ),
            );
        }

        // Body text — handle bullet lines (starting with "- " or "• ")
        if let Some(body) = &section.body {
            for line in body.split('\n') {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    doc = doc.add_paragraph(Paragraph::new());
                    continue;
                }
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
                                    .fonts(body_fonts.clone()),
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
                                    .fonts(body_fonts.clone()),
                            ),
                    );
                }
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
                        .add_run(Run::new().add_text(item).size(22).fonts(body_fonts.clone())),
                );
            }
        }

        if let Some(callout) = &section.callout {
            let fill = match callout
                .tone
                .as_deref()
                .unwrap_or("info")
                .to_ascii_lowercase()
                .as_str()
            {
                "success" => "E8F5E9",
                "warning" => "FFF4D6",
                _ => "EAF3FB",
            };

            let callout_cell = TableCell::new()
                .width(9000, WidthType::Dxa)
                .shading(Shading::new().shd_type(ShdType::Clear).fill(fill))
                .add_paragraph(
                    Paragraph::new()
                        .line_spacing(LineSpacing::new().after(80))
                        .add_run(
                            Run::new()
                                .add_text(callout.title.as_deref().unwrap_or("Highlight"))
                                .bold()
                                .size(22)
                                .color(primary_color.clone())
                                .fonts(title_fonts.clone()),
                        ),
                )
                .add_paragraph(
                    Paragraph::new()
                        .line_spacing(LineSpacing::new().line(264).after(80))
                        .add_run(
                            Run::new()
                                .add_text(&callout.body)
                                .size(22)
                                .fonts(body_fonts.clone()),
                        ),
                );
            doc = doc.add_table(
                Table::without_borders(vec![TableRow::new(vec![callout_cell])])
                    .width(9000, WidthType::Dxa)
                    .set_grid(vec![9000]),
            );
        }

        if let Some(table) = &section.table {
            if let Some(table_title) = &table.title {
                doc = doc.add_paragraph(
                    Paragraph::new()
                        .line_spacing(LineSpacing::new().before(140).after(80))
                        .add_run(
                            Run::new()
                                .add_text(table_title)
                                .bold()
                                .size(22)
                                .color(accent_color.clone())
                                .fonts(title_fonts.clone()),
                        ),
                );
            }

            let column_count = table
                .headers
                .as_ref()
                .map(|headers| headers.len())
                .unwrap_or_else(|| table.rows.iter().map(|row| row.len()).max().unwrap_or(0));

            if column_count > 0 {
                let grid = if let Some(widths) = &table.column_widths {
                    let mut weights: Vec<usize> = widths
                        .iter()
                        .copied()
                        .take(column_count)
                        .map(|value| value.max(1))
                        .collect();
                    while weights.len() < column_count {
                        weights.push(1);
                    }
                    let sum = weights.iter().sum::<usize>().max(1);
                    weights
                        .iter()
                        .map(|value| (9000 * *value / sum).max(900))
                        .collect::<Vec<_>>()
                } else {
                    vec![(9000 / column_count).max(900); column_count]
                };

                let mut rows: Vec<TableRow> = Vec::new();
                if let Some(headers) = &table.headers {
                    rows.push(TableRow::new(
                        headers
                            .iter()
                            .enumerate()
                            .map(|(index, header)| {
                                TableCell::new()
                                    .width(*grid.get(index).unwrap_or(&2000), WidthType::Dxa)
                                    .shading(
                                        Shading::new()
                                            .shd_type(ShdType::Clear)
                                            .fill(primary_color.clone()),
                                    )
                                    .add_paragraph(
                                        Paragraph::new().add_run(
                                            Run::new()
                                                .add_text(header)
                                                .bold()
                                                .size(20)
                                                .color("FFFFFF")
                                                .fonts(body_fonts.clone()),
                                        ),
                                    )
                            })
                            .collect(),
                    ));
                }

                for (row_index, row) in table.rows.iter().enumerate() {
                    let shade = if row_index % 2 == 0 {
                        "FFFFFF"
                    } else {
                        "F7FAFC"
                    };
                    rows.push(TableRow::new(
                        (0..column_count)
                            .map(|index| {
                                TableCell::new()
                                    .width(*grid.get(index).unwrap_or(&2000), WidthType::Dxa)
                                    .shading(Shading::new().shd_type(ShdType::Clear).fill(shade))
                                    .add_paragraph(
                                        Paragraph::new().add_run(
                                            Run::new()
                                                .add_text(
                                                    row.get(index).cloned().unwrap_or_default(),
                                                )
                                                .size(20)
                                                .fonts(body_fonts.clone()),
                                        ),
                                    )
                            })
                            .collect(),
                    ));
                }

                doc = doc.add_table(
                    Table::new(rows)
                        .width(9000, WidthType::Dxa)
                        .set_grid(grid)
                        .margins(
                            TableCellMargins::new()
                                .margin_left(100, WidthType::Dxa)
                                .margin_right(100, WidthType::Dxa),
                        ),
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
    title: Option<String>,
    notes: Option<String>,
    headers: Option<Vec<String>>,
    rows: Vec<Vec<serde_json::Value>>,
    column_widths: Option<Vec<f64>>,
    tab_color: Option<String>,
    freeze_rows: Option<u32>,
    autofilter: Option<bool>,
}

#[derive(Deserialize, Default)]
struct XlsxCellStyle {
    value: Option<serde_json::Value>,
    formula: Option<String>,
    num_format: Option<String>,
    bold: Option<bool>,
    italic: Option<bool>,
    align: Option<String>,
    font_color: Option<String>,
    background_color: Option<String>,
    wrap: Option<bool>,
}

fn generate_xlsx(path: &std::path::Path, content: &serde_json::Value) -> Result<u64, String> {
    use rust_xlsxwriter::{Color, Format, FormatAlign, FormatBorder};

    let content: XlsxContent = serde_json::from_value(content.clone())
        .map_err(|e| format!("Invalid XLSX content: {e}"))?;

    let parse_color = |input: Option<&str>, fallback: u32| {
        let default_hex = format!("{fallback:06X}");
        let hex = normalized_hex(input, &default_hex);
        let value = u32::from_str_radix(&hex, 16).unwrap_or(fallback);
        Color::RGB(value)
    };

    let apply_alignment = |format: Format, align: Option<&str>| match align
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "left" => format.set_align(FormatAlign::Left),
        "right" => format.set_align(FormatAlign::Right),
        "center" => format.set_align(FormatAlign::Center),
        _ => format,
    };

    let apply_cell_style =
        |base: &Format, style: &XlsxCellStyle, default_num_format: Option<&str>| {
            let mut format = base.clone();
            if style.bold.unwrap_or(false) {
                format = format.set_bold();
            }
            if style.italic.unwrap_or(false) {
                format = format.set_italic();
            }
            if style.wrap.unwrap_or(false) {
                format = format.set_text_wrap();
            }
            if let Some(num_format) = style.num_format.as_deref().or(default_num_format) {
                format = format.set_num_format(num_format);
            }
            if let Some(color) = style.font_color.as_deref() {
                format = format.set_font_color(parse_color(Some(color), 0x1F2937));
            }
            if let Some(color) = style.background_color.as_deref() {
                format = format.set_background_color(parse_color(Some(color), 0xFFFFFF));
            }
            apply_alignment(format, style.align.as_deref())
        };

    let mut workbook = rust_xlsxwriter::Workbook::new();

    let title_fmt = Format::new()
        .set_bold()
        .set_font_name("Calibri")
        .set_font_size(15)
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0x1F4E79))
        .set_align(FormatAlign::Center)
        .set_align(FormatAlign::VerticalCenter);

    let notes_fmt = Format::new()
        .set_font_name("Calibri")
        .set_font_size(10)
        .set_font_color(Color::RGB(0x4B5563))
        .set_background_color(Color::RGB(0xEEF3F8))
        .set_text_wrap();

    let header_fmt = Format::new()
        .set_bold()
        .set_font_name("Calibri")
        .set_font_size(11)
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0x2E75B6))
        .set_border(FormatBorder::Thin)
        .set_border_color(Color::RGB(0x1F4E79))
        .set_align(FormatAlign::Center)
        .set_align(FormatAlign::VerticalCenter);

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

    for sheet in &content.sheets {
        let worksheet = workbook.add_worksheet();
        worksheet
            .set_name(&sheet.name)
            .map_err(|e| format!("Failed to set sheet name: {e}"))?;

        if let Some(tab_color) = sheet.tab_color.as_deref() {
            worksheet.set_tab_color(parse_color(Some(tab_color), 0x2E75B6));
        }

        let mut num_cols = sheet
            .headers
            .as_ref()
            .map(|h| h.len())
            .unwrap_or_else(|| sheet.rows.iter().map(|r| r.len()).max().unwrap_or(0));
        if num_cols == 0 && (sheet.title.is_some() || sheet.notes.is_some()) {
            num_cols = 1;
        }

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
                        .unwrap_or(10);
                    (header_len as f64 + 4.0).clamp(10.0, 30.0)
                });
            let _ = worksheet.set_column_width(col as u16, width);
        }

        let mut row_idx: u32 = 0;
        if let Some(title) = &sheet.title {
            if num_cols > 1 {
                worksheet
                    .merge_range(0, 0, 0, (num_cols - 1) as u16, title, &title_fmt)
                    .map_err(|e| format!("Failed to write merged title: {e}"))?;
            } else {
                worksheet
                    .write_string_with_format(0, 0, title, &title_fmt)
                    .map_err(|e| format!("Failed to write title: {e}"))?;
            }
            let _ = worksheet.set_row_height(0, 24.0);
            row_idx += 1;
        }
        if let Some(notes) = &sheet.notes {
            if num_cols > 1 {
                worksheet
                    .merge_range(
                        row_idx,
                        0,
                        row_idx,
                        (num_cols - 1) as u16,
                        notes,
                        &notes_fmt,
                    )
                    .map_err(|e| format!("Failed to write notes: {e}"))?;
            } else {
                worksheet
                    .write_string_with_format(row_idx, 0, notes, &notes_fmt)
                    .map_err(|e| format!("Failed to write notes: {e}"))?;
            }
            let _ = worksheet.set_row_height(row_idx, 36.0);
            row_idx += 1;
        }

        let header_row = if let Some(headers) = &sheet.headers {
            let current = row_idx;
            for (col, header) in headers.iter().enumerate() {
                worksheet
                    .write_string_with_format(current, col as u16, header, &header_fmt)
                    .map_err(|e| format!("Failed to write header: {e}"))?;
            }
            row_idx += 1;
            Some(current)
        } else {
            None
        };

        let data_start_row = row_idx;
        for row in &sheet.rows {
            let is_alt = ((row_idx - data_start_row) % 2) == 1;
            let base_fmt = if is_alt {
                &data_fmt_gray
            } else {
                &data_fmt_white
            };

            for (col, value) in row.iter().enumerate() {
                if let Ok(style) = serde_json::from_value::<XlsxCellStyle>(value.clone()) {
                    if style.value.is_some()
                        || style.formula.is_some()
                        || style.num_format.is_some()
                        || style.bold.is_some()
                        || style.italic.is_some()
                        || style.align.is_some()
                        || style.font_color.is_some()
                        || style.background_color.is_some()
                        || style.wrap.is_some()
                    {
                        let default_num_format = style
                            .value
                            .as_ref()
                            .filter(|v| matches!(v, serde_json::Value::Number(_)))
                            .map(|_| "#,##0.##");
                        let fmt = apply_cell_style(base_fmt, &style, default_num_format);
                        if let Some(formula) = style.formula.as_deref() {
                            worksheet
                                .write_formula_with_format(row_idx, col as u16, formula, &fmt)
                                .map_err(|e| format!("Failed to write formula: {e}"))?;
                        } else if let Some(cell_value) = style.value.as_ref() {
                            match cell_value {
                                serde_json::Value::Number(n) => {
                                    if let Some(f) = n.as_f64() {
                                        worksheet
                                            .write_number_with_format(row_idx, col as u16, f, &fmt)
                                            .map_err(|e| format!("Failed to write number: {e}"))?;
                                    }
                                }
                                serde_json::Value::Bool(b) => {
                                    worksheet
                                        .write_boolean_with_format(row_idx, col as u16, *b, &fmt)
                                        .map_err(|e| format!("Failed to write boolean: {e}"))?;
                                }
                                _ => {
                                    worksheet
                                        .write_string_with_format(
                                            row_idx,
                                            col as u16,
                                            &value_to_display_text(cell_value),
                                            &fmt,
                                        )
                                        .map_err(|e| format!("Failed to write string: {e}"))?;
                                }
                            }
                            continue;
                        }
                    }
                }

                match value {
                    serde_json::Value::Number(n) => {
                        if let Some(f) = n.as_f64() {
                            let fmt = base_fmt.clone().set_num_format("#,##0.##");
                            worksheet
                                .write_number_with_format(row_idx, col as u16, f, &fmt)
                                .map_err(|e| format!("Failed to write number: {e}"))?;
                        }
                    }
                    serde_json::Value::Bool(b) => {
                        worksheet
                            .write_boolean_with_format(row_idx, col as u16, *b, base_fmt)
                            .map_err(|e| format!("Failed to write boolean: {e}"))?;
                    }
                    _ => {
                        worksheet
                            .write_string_with_format(
                                row_idx,
                                col as u16,
                                &value_to_display_text(value),
                                base_fmt,
                            )
                            .map_err(|e| format!("Failed to write string: {e}"))?;
                    }
                }
            }
            row_idx += 1;
        }

        if let Some(header_row) = header_row {
            if sheet.autofilter.unwrap_or(true) && row_idx > header_row {
                let last_row = row_idx.saturating_sub(1);
                let last_col = num_cols.saturating_sub(1) as u16;
                let _ = worksheet.autofilter(header_row, 0, last_row, last_col);
            }
        }

        let freeze_rows = sheet.freeze_rows.unwrap_or_else(|| {
            if header_row.is_some() {
                row_idx.min(data_start_row)
            } else {
                0
            }
        });
        if freeze_rows > 0 {
            let _ = worksheet.set_freeze_panes(freeze_rows, 0);
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
    left_title: Option<String>,
    left_body: Option<String>,
    left_bullet_items: Option<Vec<String>>,
    right_title: Option<String>,
    right_body: Option<String>,
    right_bullet_items: Option<Vec<String>>,
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

fn generate_pptx(path: &std::path::Path, content: &serde_json::Value) -> Result<u64, String> {
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
        source_scope: &[String],
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
