//! GenerateXlsxTool — creates professional XLSX spreadsheets via `rust_xlsxwriter`.

use std::path::PathBuf;
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::create_file_tool::{has_path_traversal, resolve_and_validate};
use super::{scoped_sources, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/generate_xlsx.json");

pub struct GenerateXlsxTool;

#[derive(Deserialize)]
struct GenerateXlsxArgs {
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

fn value_to_display_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(v) => v.to_string(),
        serde_json::Value::Number(v) => v.to_string(),
        other => other.to_string(),
    }
}

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
    charts: Option<Vec<ChartSpec>>,
}

#[derive(Deserialize)]
struct ChartSpec {
    #[serde(rename = "type")]
    chart_type: String,
    title: Option<String>,
    data_range: String,
    categories_range: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    position: Option<ChartPosition>,
}

#[derive(Deserialize)]
struct ChartPosition {
    row: Option<u32>,
    col: Option<u16>,
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

// ---------------------------------------------------------------------------
// XLSX generation
// ---------------------------------------------------------------------------

pub(crate) fn generate_xlsx(
    path: &std::path::Path,
    content: &serde_json::Value,
) -> Result<u64, String> {
    use rust_xlsxwriter::{Chart, ChartType, Color, Format, FormatAlign, FormatBorder};

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
                                            value_to_display_text(cell_value),
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
                                value_to_display_text(value),
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

        // Charts
        if let Some(charts) = &sheet.charts {
            for chart_spec in charts {
                let chart_type = match chart_spec.chart_type.to_lowercase().as_str() {
                    "bar" => ChartType::Bar,
                    "column" => ChartType::Column,
                    "line" => ChartType::Line,
                    "pie" => ChartType::Pie,
                    "scatter" => ChartType::Scatter,
                    "area" => ChartType::Area,
                    "doughnut" => ChartType::Doughnut,
                    "radar" => ChartType::Radar,
                    _ => ChartType::Column,
                };

                let mut chart = Chart::new(chart_type);

                if let Some(title) = &chart_spec.title {
                    chart.title().set_name(title);
                }

                // Parse data range: "Sheet1!$B$2:$B$10" or simplified "B2:B10"
                let (r1, c1, r2, c2) = parse_range(&chart_spec.data_range);
                let series = chart.add_series();
                series.set_values((&*sheet.name, r1, c1, r2, c2));

                if let Some(cat_range) = &chart_spec.categories_range {
                    let (cr1, cc1, cr2, cc2) = parse_range(cat_range);
                    series.set_categories((&*sheet.name, cr1, cc1, cr2, cc2));
                }

                let width = chart_spec.width.unwrap_or(480);
                let height = chart_spec.height.unwrap_or(288);
                chart.set_width(width);
                chart.set_height(height);

                let pos_row = chart_spec
                    .position
                    .as_ref()
                    .and_then(|p| p.row)
                    .unwrap_or(row_idx + 1);
                let pos_col = chart_spec
                    .position
                    .as_ref()
                    .and_then(|p| p.col)
                    .unwrap_or(0);

                worksheet
                    .insert_chart(pos_row, pos_col, &chart)
                    .map_err(|e| format!("Failed to insert chart: {e}"))?;
            }
        }
    }

    workbook
        .save(path)
        .map_err(|e| format!("Failed to save XLSX: {e}"))?;

    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Ok(size)
}

/// Parse a simplified cell range like "B2:B10" into (first_row, first_col, last_row, last_col).
/// Supports both "B2:B10" and "$B$2:$B$10" formats.
fn parse_range(range: &str) -> (u32, u16, u32, u16) {
    // Strip the "Sheet1!" prefix if present
    let range = range
        .split('!')
        .last()
        .unwrap_or(range)
        .replace('$', "");

    let parts: Vec<&str> = range.split(':').collect();
    if parts.len() == 2 {
        let (r1, c1) = parse_cell_ref(parts[0]);
        let (r2, c2) = parse_cell_ref(parts[1]);
        (r1, c1, r2, c2)
    } else {
        (0, 0, 0, 0)
    }
}

/// Parse a cell reference like "B2" into (row, col) zero-indexed.
fn parse_cell_ref(cell: &str) -> (u32, u16) {
    let cell = cell.trim();
    let mut col_str = String::new();
    let mut row_str = String::new();
    for ch in cell.chars() {
        if ch.is_ascii_alphabetic() {
            col_str.push(ch.to_ascii_uppercase());
        } else if ch.is_ascii_digit() {
            row_str.push(ch);
        }
    }
    let col = col_str
        .chars()
        .fold(0u16, |acc, c| acc * 26 + (c as u16 - b'A' as u16 + 1))
        .saturating_sub(1);
    let row = row_str.parse::<u32>().unwrap_or(1).saturating_sub(1);
    (row, col)
}

// ---------------------------------------------------------------------------
// Tool implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Tool for GenerateXlsxTool {
    fn name(&self) -> &str {
        "generate_xlsx"
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
        Some(format!("Generate XLSX spreadsheet: {path}"))
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: GenerateXlsxArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid generate_xlsx arguments: {e}"))
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

            match generate_xlsx(&canonical, &content) {
                Ok(size) => Ok(ToolResult {
                    call_id,
                    content: format!(
                        "Generated XLSX spreadsheet '{}' ({size} bytes).\nPath: {}",
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
