//! GenerateDocxTool — creates professional DOCX documents via `docx-rs`.

use std::path::PathBuf;
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::db::Database;
use crate::error::CoreError;

use super::create_file_tool::{has_path_traversal, resolve_and_validate};
use super::{scoped_sources, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/generate_docx.json");

pub struct GenerateDocxTool;

#[derive(Deserialize)]
struct GenerateDocxArgs {
    path: String,
    content: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Data structures
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
    header: Option<String>,
    footer: Option<String>,
    markdown: Option<String>,
    #[serde(default)]
    sections: Vec<DocxSection>,
}

#[derive(Deserialize)]
struct DocxSection {
    heading: Option<String>,
    body: Option<String>,
    bullet_items: Option<Vec<String>>,
    numbered_items: Option<Vec<String>>,
    links: Option<Vec<DocxLink>>,
    callout: Option<DocxCallout>,
    table: Option<DocxTable>,
    page_break_before: Option<bool>,
}

#[derive(Deserialize)]
struct DocxLink {
    text: String,
    url: String,
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

#[derive(Default)]
struct MarkdownDocx {
    title: Option<String>,
    sections: Vec<DocxSection>,
}

#[derive(Default)]
struct MarkdownSectionBuilder {
    heading: Option<String>,
    body_lines: Vec<String>,
    bullet_items: Vec<String>,
    numbered_items: Vec<String>,
    callout: Option<DocxCallout>,
    table: Option<DocxTable>,
}

impl MarkdownSectionBuilder {
    fn has_content(&self) -> bool {
        self.heading
            .as_ref()
            .map(|h| !h.trim().is_empty())
            .unwrap_or(false)
            || self.body_lines.iter().any(|line| !line.trim().is_empty())
            || !self.bullet_items.is_empty()
            || !self.numbered_items.is_empty()
            || self.callout.is_some()
            || self.table.is_some()
    }
}

fn clean_markdown_text(input: &str) -> String {
    input
        .trim()
        .replace("**", "")
        .replace("__", "")
        .replace('`', "")
}

fn markdown_heading(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }

    let level = trimmed.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&level) || !trimmed.chars().nth(level).is_some_and(char::is_whitespace) {
        return None;
    }

    let heading = clean_markdown_text(&trimmed[level..]);
    if heading.is_empty() {
        None
    } else {
        Some((level, heading))
    }
}

fn numbered_item_text(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let dot_index = trimmed.find('.')?;
    if dot_index == 0 || !trimmed[..dot_index].chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let rest = trimmed[dot_index + 1..].trim_start();
    if rest.is_empty() {
        None
    } else {
        Some(clean_markdown_text(rest))
    }
}

fn is_markdown_table_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.matches('|').count() >= 2
}

fn markdown_table_cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(clean_markdown_text)
        .collect()
}

fn is_separator_row(row: &[String]) -> bool {
    !row.is_empty()
        && row.iter().all(|cell| {
            let trimmed = cell.trim();
            !trimmed.is_empty()
                && trimmed.contains('-')
                && trimmed
                    .chars()
                    .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
        })
}

fn parse_markdown_table(lines: &[&str]) -> Option<DocxTable> {
    let rows: Vec<Vec<String>> = lines
        .iter()
        .map(|line| markdown_table_cells(line))
        .filter(|row| !row.is_empty())
        .collect();
    if rows.len() < 2 {
        return None;
    }

    let has_separator = rows.get(1).is_some_and(|row| is_separator_row(row));
    let headers = if has_separator {
        rows.first().cloned()
    } else {
        None
    };
    let start = if has_separator { 2 } else { 0 };
    let data_rows = rows
        .into_iter()
        .skip(start)
        .filter(|row| !is_separator_row(row))
        .collect::<Vec<_>>();

    Some(DocxTable {
        title: None,
        headers,
        rows: data_rows,
        column_widths: None,
    })
}

fn flush_markdown_section(sections: &mut Vec<DocxSection>, builder: &mut MarkdownSectionBuilder) {
    if !builder.has_content() {
        return;
    }

    sections.push(DocxSection {
        heading: builder.heading.take(),
        body: if builder.body_lines.is_empty() {
            None
        } else {
            Some(builder.body_lines.join("\n"))
        },
        bullet_items: if builder.bullet_items.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut builder.bullet_items))
        },
        numbered_items: if builder.numbered_items.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut builder.numbered_items))
        },
        links: None,
        callout: builder.callout.take(),
        table: builder.table.take(),
        page_break_before: None,
    });
    builder.body_lines.clear();
}

fn markdown_to_docx(markdown: &str) -> MarkdownDocx {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut parsed = MarkdownDocx::default();
    let mut builder = MarkdownSectionBuilder::default();
    let mut index = 0usize;

    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim();

        if trimmed.is_empty() || trimmed == "---" {
            if !builder
                .body_lines
                .last()
                .is_some_and(|line| line.is_empty())
            {
                builder.body_lines.push(String::new());
            }
            index += 1;
            continue;
        }

        if let Some((level, heading)) = markdown_heading(line) {
            if level == 1 && parsed.title.is_none() && !builder.has_content() {
                parsed.title = Some(heading);
            } else {
                flush_markdown_section(&mut parsed.sections, &mut builder);
                builder.heading = Some(heading);
            }
            index += 1;
            continue;
        }

        if is_markdown_table_line(line) {
            let start = index;
            while index < lines.len() && is_markdown_table_line(lines[index]) {
                index += 1;
            }
            if let Some(table) = parse_markdown_table(&lines[start..index]) {
                if builder.table.is_some() {
                    flush_markdown_section(&mut parsed.sections, &mut builder);
                }
                builder.table = Some(table);
                continue;
            }
            index = start;
        }

        if let Some(text) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("• "))
        {
            builder.bullet_items.push(clean_markdown_text(text));
            index += 1;
            continue;
        }

        if let Some(text) = numbered_item_text(trimmed) {
            builder.numbered_items.push(text);
            index += 1;
            continue;
        }

        if let Some(text) = trimmed.strip_prefix("> ") {
            if builder.callout.is_some() {
                flush_markdown_section(&mut parsed.sections, &mut builder);
            }
            builder.callout = Some(DocxCallout {
                title: Some("Note".to_string()),
                body: clean_markdown_text(text),
                tone: Some("info".to_string()),
            });
            index += 1;
            continue;
        }

        builder.body_lines.push(clean_markdown_text(line));
        index += 1;
    }

    flush_markdown_section(&mut parsed.sections, &mut builder);
    parsed
}

fn normalized_hex(input: Option<&str>, fallback: &str) -> String {
    input
        .map(str::trim)
        .map(|value| value.trim_start_matches('#'))
        .filter(|value| value.len() == 6 && value.chars().all(|ch| ch.is_ascii_hexdigit()))
        .map(|value| value.to_ascii_uppercase())
        .unwrap_or_else(|| fallback.to_string())
}

// ---------------------------------------------------------------------------
// DOCX generation
// ---------------------------------------------------------------------------

pub(crate) fn generate_docx(
    path: &std::path::Path,
    content: &serde_json::Value,
) -> Result<u64, String> {
    use docx_rs::*;

    let mut content: DocxContent = serde_json::from_value(content.clone())
        .map_err(|e| format!("Invalid DOCX content: {e}"))?;
    if content.sections.is_empty() {
        if let Some(markdown) = content.markdown.as_deref() {
            let parsed = markdown_to_docx(markdown);
            if content.title.is_none() {
                content.title = parsed.title;
            }
            content.sections = parsed.sections;
        }
    }
    if content.sections.is_empty() {
        return Err("DOCX content must include at least one section or non-empty markdown.".into());
    }

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

    // Set up numbered list: abstract numbering 2, numbering 2
    let numbered_abstract = AbstractNumbering::new(2).add_level(
        Level::new(
            0,
            Start::new(1),
            NumberFormat::new("decimal"),
            LevelText::new("%1."),
            LevelJc::new("left"),
        )
        .indent(Some(720), Some(SpecialIndentType::Hanging(360)), None, None),
    );
    let numbered_num = Numbering::new(2, 2);

    doc = doc
        .add_abstract_numbering(bullet_abstract)
        .add_numbering(bullet_num)
        .add_abstract_numbering(numbered_abstract)
        .add_numbering(numbered_num);

    // Page header
    if let Some(header_text) = &content.header {
        let header = Header::new().add_paragraph(
            Paragraph::new().align(AlignmentType::Right).add_run(
                Run::new()
                    .add_text(header_text)
                    .size(18)
                    .color("888888")
                    .fonts(body_fonts.clone()),
            ),
        );
        doc = doc.header(header);
    }

    // Page footer
    if let Some(footer_text) = &content.footer {
        let footer = Footer::new().add_paragraph(
            Paragraph::new().align(AlignmentType::Center).add_run(
                Run::new()
                    .add_text(footer_text)
                    .size(18)
                    .color("888888")
                    .fonts(body_fonts.clone()),
            ),
        );
        doc = doc.footer(footer);
    }

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
                    .line_spacing(LineSpacing::new().before(240).after(120))
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
                                    .line(276)
                                    .line_rule(LineSpacingType::Auto)
                                    .after(120),
                            )
                            .add_run(
                                Run::new()
                                    .add_text(bullet_text)
                                    .size(22)
                                    .fonts(body_fonts.clone()),
                            ),
                    );
                } else {
                    doc = doc.add_paragraph(
                        Paragraph::new()
                            .align(AlignmentType::Both)
                            .line_spacing(
                                LineSpacing::new()
                                    .line(276)
                                    .line_rule(LineSpacingType::Auto)
                                    .after(120),
                            )
                            .add_run(Run::new().add_text(line).size(22).fonts(body_fonts.clone())),
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

        // Numbered items list
        if let Some(items) = &section.numbered_items {
            for item in items {
                doc = doc.add_paragraph(
                    Paragraph::new()
                        .numbering(NumberingId::new(2), IndentLevel::new(0))
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

        // Hyperlinks
        if let Some(links) = &section.links {
            for link in links {
                let hyperlink = Hyperlink::new(&link.url, HyperlinkType::External).add_run(
                    Run::new()
                        .add_text(&link.text)
                        .size(22)
                        .color("0563C1")
                        .underline("single")
                        .fonts(body_fonts.clone()),
                );
                doc = doc.add_paragraph(
                    Paragraph::new()
                        .line_spacing(
                            LineSpacing::new()
                                .line(276)
                                .line_rule(LineSpacingType::Auto)
                                .after(120),
                        )
                        .add_hyperlink(hyperlink),
                );
            }
        }

        // Callout box
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

        // Table
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
// Tool implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Tool for GenerateDocxTool {
    fn name(&self) -> &str {
        "generate_docx"
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
        Some(format!("Generate DOCX document: {path}"))
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: GenerateDocxArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid generate_docx arguments: {e}"))
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

            match generate_docx(&canonical, &content) {
                Ok(size) => Ok(ToolResult {
                    call_id,
                    content: format!(
                        "Generated DOCX document '{}' ({size} bytes).\nPath: {}",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_to_docx_extracts_common_blocks() {
        let parsed = markdown_to_docx(
            r#"# Product Plan

## Overview
This is **important**.
- First item
- Second item

> Keep this visible.

## Metrics
| Metric | Value |
| --- | --- |
| ARR | 1M |
"#,
        );

        assert_eq!(parsed.title.as_deref(), Some("Product Plan"));
        assert_eq!(parsed.sections.len(), 2);
        assert_eq!(parsed.sections[0].heading.as_deref(), Some("Overview"));
        assert_eq!(
            parsed.sections[0].bullet_items.as_deref(),
            Some(&["First item".to_string(), "Second item".to_string()][..])
        );
        assert_eq!(
            parsed.sections[0].callout.as_ref().map(|c| c.body.as_str()),
            Some("Keep this visible.")
        );
        let table = parsed.sections[1].table.as_ref().expect("table");
        assert_eq!(
            table.headers.as_deref(),
            Some(&["Metric".to_string(), "Value".to_string()][..])
        );
        assert_eq!(table.rows, vec![vec!["ARR".to_string(), "1M".to_string()]]);
    }
}
