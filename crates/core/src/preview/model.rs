use serde::Serialize;

pub const WORKBOOK_MAX_SHEETS: usize = 20;
pub const WORKBOOK_MAX_ROWS: usize = 500;
pub const WORKBOOK_MAX_COLUMNS: usize = 60;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewCapabilities {
    pub can_render_structured: bool,
    pub can_extract_text: bool,
    pub needs_external_runtime: bool,
    pub structured_unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StructuredPreview {
    Document {
        blocks: Vec<PreviewBlock>,
        assets: Vec<PreviewAsset>,
    },
    Workbook {
        sheets: Vec<PreviewSheet>,
        limits: WorkbookPreviewLimits,
        truncated: bool,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PreviewBlock {
    Heading {
        level: u8,
        runs: Vec<PreviewRun>,
        alignment: Option<String>,
    },
    Paragraph {
        runs: Vec<PreviewRun>,
        alignment: Option<String>,
    },
    List {
        ordered: bool,
        level: u8,
        items: Vec<PreviewListItem>,
    },
    Table {
        rows: Vec<PreviewTableRow>,
    },
    Image {
        asset_id: String,
        alt: Option<String>,
    },
    PageBreak,
    Unsupported {
        message: String,
    },
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRun {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub color: Option<String>,
    pub background_color: Option<String>,
    pub font_size: Option<String>,
    pub hyperlink: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewListItem {
    pub runs: Vec<PreviewRun>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewTableRow {
    pub cells: Vec<PreviewTableCell>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewTableCell {
    pub blocks: Vec<PreviewBlock>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAsset {
    pub id: String,
    pub kind: String,
    pub mime_type: String,
    pub path: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkbookPreviewLimits {
    pub max_sheets: usize,
    pub max_rows: usize,
    pub max_columns: usize,
}

impl Default for WorkbookPreviewLimits {
    fn default() -> Self {
        Self {
            max_sheets: WORKBOOK_MAX_SHEETS,
            max_rows: WORKBOOK_MAX_ROWS,
            max_columns: WORKBOOK_MAX_COLUMNS,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewSheet {
    pub name: String,
    pub index: usize,
    pub row_count: usize,
    pub column_count: usize,
    pub preview_row_count: usize,
    pub preview_column_count: usize,
    pub cells: Vec<PreviewCell>,
    pub merged_ranges: Vec<PreviewMergedRange>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewCell {
    pub row: u32,
    pub column: u32,
    pub value: String,
    pub data_type: String,
    pub formula: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewMergedRange {
    pub start_row: u32,
    pub start_column: u32,
    pub end_row: u32,
    pub end_column: u32,
}
