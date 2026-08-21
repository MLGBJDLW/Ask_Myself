//! OfficeArtifactTool — transactional DOCX/XLSX/PPTX candidate lifecycle.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::Database;
use crate::error::CoreError;
use crate::office_runtime;

use super::path_utils::resolve_existing_directory_for_file_access;
use super::{file_access_policy, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/office_artifact.json");

pub struct OfficeArtifactTool;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum OfficeArtifactAction {
    Capabilities,
    Inspect,
    Assess,
    Execute,
    Decide,
    Restore,
    LiveStatus,
    LivePairing,
    LiveExecute,
}

impl OfficeArtifactAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Capabilities => "capabilities",
            Self::Inspect => "inspect",
            Self::Assess => "assess",
            Self::Execute => "execute",
            Self::Decide => "decide",
            Self::Restore => "restore",
            Self::LiveStatus => "live_status",
            Self::LivePairing => "live_pairing",
            Self::LiveExecute => "live_execute",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum OfficeFormat {
    Docx,
    Xlsx,
    Pptx,
}

impl OfficeFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::Docx => "docx",
            Self::Xlsx => "xlsx",
            Self::Pptx => "pptx",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum OfficeIntent {
    Create,
    Modify,
    Verify,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum OfficeQuality {
    Draft,
    Standard,
    Publish,
    Native,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum OfficePreservation {
    Strict,
    Balanced,
    Replace,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum OfficeCalculation {
    NotRequired,
    Static,
    Compatible,
    Native,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum OfficeRender {
    None,
    ImportantSurfaces,
    All,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct OfficeGuarantees {
    #[serde(skip_serializing_if = "Option::is_none")]
    quality: Option<OfficeQuality>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preservation: Option<OfficePreservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    calculation: Option<OfficeCalculation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    render: Option<OfficeRender>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct OfficePreconditions {
    #[serde(skip_serializing_if = "Option::is_none")]
    source_sha256: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct OfficeDelivery {
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    manifest: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
enum OfficeValidation {
    Path(String),
    Contract(Box<OfficeValidationContract>),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct OfficeValidationContract {
    #[serde(rename = "contractVersion")]
    contract_version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    required_text: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    forbidden_text: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    required_sheets: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    required_named_ranges: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    require_formula_cache: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    min_paragraphs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    min_tables: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    required_styles: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    require_alt_text: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    min_comments: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    require_tracked_changes: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    require_no_tracked_changes: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    min_slides: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_slides: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    required_slide_titles: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    require_speaker_notes: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    no_heading_level_skips: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    require_table_header_rows: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    require_fixed_table_layout: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    required_language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    no_numeric_hardcodes_in: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    min_rows: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sentinels: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tie_outs: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reconciliations: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    formula_patterns: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    required_provenance: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum OfficeOperation {
    Create {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spec: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subtitle: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "inputMd")]
        input_md: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        template: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "authorEngine"
        )]
        author_engine: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "htmlFirst")]
        html_first: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outdir: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        screenshot: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        font: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        footer: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        author: Option<String>,
    },
    Replace(ReplaceOperation),
    Redact(ReplaceOperation),
    SecureRedact(SecureRedactOperation),
    AddComment(CommentOperation),
    TrackedReplace(TrackedReplaceOperation),
    AddBookmark(BookmarkOperation),
    InsertField(FieldOperation),
    WrapContentControl(ContentControlOperation),
    SetProtection {
        mode: String,
    },
    StripComments {},
    AcceptChanges {},
    RejectChanges {},
    SetValue(CellValueOperation),
    SetFormula(FormulaOperation),
    SetRange(RangeValueOperation),
    ClearRange(RangeOperation),
    SetStyle(StyleOperation),
    RenameSheet {
        sheet: String,
        #[serde(rename = "newName")]
        new_name: String,
    },
    SetDefinedName(DefinedNameOperation),
    SetDataValidation(DataValidationOperation),
    CreateTable(TableOperation),
    SetNumberFormat(NumberFormatOperation),
    SetChartTitle {
        #[serde(rename = "chartPart")]
        chart_part: String,
        title: String,
    },
    Recalculate {},
    InsertSlide(InsertSlideOperation),
    SetText(SetTextOperation),
    CloneSlide(SlideTargetOperation),
    ReorderSlides {
        order: Vec<Value>,
    },
    SetTransition(TransitionOperation),
    SetAltText(AltTextOperation),
    SetSpeakerNotes(SpeakerNotesOperation),
    Validate {},
    Render {},
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplaceOperation {
    find: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    replace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_matches: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scope: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    occurrence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    allow_style_merge: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SecureRedactOperation {
    find: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    replace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_matches: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    privacy_scrub: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommentOperation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    find: Option<String>,
    comment: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    initials: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    occurrence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    slide_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    slide_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    x: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    y: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct TrackedReplaceOperation {
    find: String,
    replace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    occurrence: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BookmarkOperation {
    find: String,
    bookmark_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    occurrence: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FieldOperation {
    find: String,
    instruction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    display_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    occurrence: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ContentControlOperation {
    find: String,
    tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lock: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    occurrence: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CellValueOperation {
    sheet: String,
    cell: String,
    value: Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FormulaOperation {
    sheet: String,
    cell: String,
    formula: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cached_value: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RangeValueOperation {
    sheet: String,
    range: String,
    values: Vec<Vec<Value>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RangeOperation {
    sheet: String,
    range: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StyleOperation {
    sheet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cell: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    range: Option<String>,
    style_id: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DefinedNameOperation {
    name: String,
    formula: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scope_sheet: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DataValidationOperation {
    sheet: String,
    range: String,
    validation_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    operator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    formula1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    formula2: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    allow_blank: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    show_error_message: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct TableOperation {
    sheet: String,
    range: String,
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    columns: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    style_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct NumberFormatOperation {
    sheet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cell: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    range: Option<String>,
    format_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_style_id: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct InsertSlideOperation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SetTextOperation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    slide_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    slide_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shape_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shape_name: Option<String>,
    text: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SlideTargetOperation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    slide_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    slide_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after_index: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct TransitionOperation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    slide_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    slide_index: Option<u64>,
    transition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    speed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    direction: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AltTextOperation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    slide_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    slide_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shape_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shape_name: Option<String>,
    alt_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SpeakerNotesOperation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    slide_id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    slide_index: Option<u64>,
    text: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OfficeArtifactRequest {
    request_version: u8,
    format: OfficeFormat,
    intent: OfficeIntent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    destination: Option<String>,
    operations: Vec<OfficeOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    guarantees: Option<OfficeGuarantees>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preconditions: Option<OfficePreconditions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    validation: Option<OfficeValidation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delivery: Option<OfficeDelivery>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum OfficeDecision {
    Publish,
    Discard,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum OfficeLiveOperation {
    WordReplaceText {
        search: String,
        replacement: String,
        #[serde(default, rename = "matchCase")]
        match_case: bool,
        #[serde(default, rename = "matchWholeWord")]
        match_whole_word: bool,
    },
    WordInsertText {
        location: String,
        text: String,
    },
    WordAddComment {
        search: String,
        comment: String,
    },
    ExcelSetRange {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<String>,
        address: String,
        values: Vec<Vec<Value>>,
    },
    ExcelSetFormula {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<String>,
        address: String,
        formulas: Vec<Vec<String>>,
    },
    ExcelFormatRange {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<String>,
        address: String,
        format: Value,
    },
    PowerpointSetText {
        #[serde(default, rename = "slideId", skip_serializing_if = "Option::is_none")]
        slide_id: Option<String>,
        #[serde(
            default,
            rename = "slideIndex",
            skip_serializing_if = "Option::is_none"
        )]
        slide_index: Option<u64>,
        #[serde(default, rename = "shapeId", skip_serializing_if = "Option::is_none")]
        shape_id: Option<String>,
        #[serde(default, rename = "shapeName", skip_serializing_if = "Option::is_none")]
        shape_name: Option<String>,
        text: String,
    },
    PowerpointAddSlide {
        #[serde(default, rename = "layoutId", skip_serializing_if = "Option::is_none")]
        layout_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
}

impl OfficeLiveOperation {
    fn host(&self) -> &'static str {
        match self {
            Self::WordReplaceText { .. }
            | Self::WordInsertText { .. }
            | Self::WordAddComment { .. } => "Word",
            Self::ExcelSetRange { .. }
            | Self::ExcelSetFormula { .. }
            | Self::ExcelFormatRange { .. } => "Excel",
            Self::PowerpointSetText { .. } | Self::PowerpointAddSlide { .. } => "PowerPoint",
        }
    }

    fn capability(&self) -> &'static str {
        match self {
            Self::WordReplaceText { .. } => "word.replace-text",
            Self::WordInsertText { .. } => "word.insert-text",
            Self::WordAddComment { .. } => "word.add-comment",
            Self::ExcelSetRange { .. } => "excel.set-range",
            Self::ExcelSetFormula { .. } => "excel.set-formula",
            Self::ExcelFormatRange { .. } => "excel.format-range",
            Self::PowerpointSetText { .. } => "powerpoint.set-text",
            Self::PowerpointAddSlide { .. } => "powerpoint.add-slide",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OfficeBlocker {
    code: String,
    message: String,
    #[serde(flatten)]
    details: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind")]
enum OfficeEngineResponse {
    #[serde(rename = "officeArtifactCapabilities")]
    Capabilities {
        #[serde(rename = "requestVersion")]
        request_version: u8,
        #[serde(rename = "adapterContractVersion")]
        adapter_contract_version: u8,
        formats: BTreeMap<String, Value>,
        lifecycle: Vec<String>,
        #[serde(flatten)]
        evidence: BTreeMap<String, Value>,
    },
    #[serde(rename = "officeArtifactInspection")]
    Inspection {
        format: String,
        source: String,
        sha256: String,
        #[serde(flatten)]
        evidence: BTreeMap<String, Value>,
    },
    #[serde(rename = "officeArtifactAssessment")]
    Assessment {
        status: String,
        format: String,
        ready: bool,
        #[serde(default)]
        blockers: Vec<OfficeBlocker>,
        #[serde(flatten)]
        evidence: BTreeMap<String, Value>,
    },
    #[serde(rename = "officeArtifactOutcome")]
    Outcome {
        status: String,
        #[serde(default, rename = "candidateId")]
        candidate_id: Option<String>,
        #[serde(default, rename = "candidatePath")]
        candidate_path: Option<String>,
        #[serde(default, rename = "candidateSha256")]
        candidate_sha256: Option<String>,
        #[serde(default, rename = "receiptId")]
        receipt_id: Option<String>,
        #[serde(default)]
        path: Option<String>,
        #[serde(flatten)]
        evidence: BTreeMap<String, Value>,
    },
    #[serde(rename = "officeArtifactError")]
    Error {
        code: String,
        message: String,
        stage: String,
        retryable: bool,
        #[serde(default, rename = "evidencePaths")]
        evidence_paths: Vec<String>,
        #[serde(default)]
        details: BTreeMap<String, Value>,
    },
}

#[derive(Debug, Deserialize)]
struct OfficeArtifactArgs {
    action: OfficeArtifactAction,
    workspace_root: String,
    #[serde(default)]
    request: Option<OfficeArtifactRequest>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    format: Option<OfficeFormat>,
    #[serde(default)]
    candidate_id: Option<String>,
    #[serde(default)]
    decision: Option<OfficeDecision>,
    #[serde(default)]
    receipt_id: Option<String>,
    #[serde(default)]
    live_session_id: Option<String>,
    #[serde(default)]
    live_operation: Option<OfficeLiveOperation>,
}

fn app_data_dir_from_db(db: &Database) -> Result<PathBuf, CoreError> {
    if let Some(parent) = db.db_path().and_then(|path| path.parent()) {
        return Ok(parent.to_path_buf());
    }
    let data_dir = dirs::data_dir()
        .ok_or_else(|| CoreError::Internal("Could not resolve app data directory".to_string()))?;
    Ok(data_dir.join(crate::APP_DIR))
}

fn engine_arguments(args: &OfficeArtifactArgs) -> Result<(Vec<String>, String), CoreError> {
    let action = args.action.as_str();
    let mut command = vec!["--action".to_string(), action.to_string()];
    let mut request_json = String::new();
    match action {
        "capabilities" => {}
        "inspect" => {
            let source = args.source.as_deref().ok_or_else(|| {
                CoreError::InvalidInput("office_artifact inspect requires source".to_string())
            })?;
            command.extend(["--source".to_string(), source.to_string()]);
            if let Some(format) = args.format {
                command.extend(["--format".to_string(), format.as_str().to_string()]);
            }
        }
        "assess" | "execute" => {
            let request = args.request.as_ref().ok_or_else(|| {
                CoreError::InvalidInput(format!("office_artifact {action} requires request"))
            })?;
            request_json = serde_json::to_string(request).map_err(|error| {
                CoreError::InvalidInput(format!("Invalid Office artifact request: {error}"))
            })?;
            command.extend(["--request".to_string(), "-".to_string()]);
        }
        "decide" => {
            let candidate_id = args.candidate_id.as_deref().ok_or_else(|| {
                CoreError::InvalidInput("office_artifact decide requires candidate_id".to_string())
            })?;
            let decision = args.decision.ok_or_else(|| {
                CoreError::InvalidInput("office_artifact decide requires decision".to_string())
            })?;
            command.extend([
                "--candidate-id".to_string(),
                candidate_id.to_string(),
                "--decision".to_string(),
                match decision {
                    OfficeDecision::Publish => "publish".to_string(),
                    OfficeDecision::Discard => "discard".to_string(),
                },
            ]);
        }
        "restore" => {
            let receipt_id = args.receipt_id.as_deref().ok_or_else(|| {
                CoreError::InvalidInput("office_artifact restore requires receipt_id".to_string())
            })?;
            command.extend(["--receipt-id".to_string(), receipt_id.to_string()]);
        }
        "live_status" | "live_pairing" | "live_execute" => {
            return Err(CoreError::Internal(
                "Office live actions must be handled in-process".to_string(),
            ));
        }
        other => {
            return Err(CoreError::InvalidInput(format!(
                "Unknown office_artifact action: {other}"
            )))
        }
    }
    Ok((command, request_json))
}

fn resolve_workspace(
    requested: &str,
    db: &Database,
    source_scope: &[String],
) -> Result<PathBuf, CoreError> {
    let policy = file_access_policy(db, source_scope)?;
    resolve_existing_directory_for_file_access(
        Path::new(requested),
        &policy.sources,
        policy.allow_unregistered_absolute_paths,
    )
    .map_err(CoreError::InvalidInput)
}

async fn execute_live_action(
    call_id: &str,
    args: &OfficeArtifactArgs,
) -> Result<ToolResult, CoreError> {
    let bridge = crate::office_live_bridge::ensure_office_live_bridge()?;
    let payload = match args.action {
        OfficeArtifactAction::LiveStatus => serde_json::to_value(bridge.status(false)),
        OfficeArtifactAction::LivePairing => serde_json::to_value(bridge.status(true)),
        OfficeArtifactAction::LiveExecute => {
            let session_id = args.live_session_id.as_deref().ok_or_else(|| {
                CoreError::InvalidInput(
                    "office_artifact live_execute requires live_session_id".to_string(),
                )
            })?;
            let operation = args.live_operation.as_ref().ok_or_else(|| {
                CoreError::InvalidInput(
                    "office_artifact live_execute requires live_operation".to_string(),
                )
            })?;
            let status = bridge.status(false);
            let session = status
                .sessions
                .iter()
                .find(|session| session.session_id == session_id)
                .ok_or_else(|| {
                    CoreError::InvalidInput(format!(
                        "Office.js host session is not connected: {session_id}"
                    ))
                })?;
            if session.host != operation.host() {
                return Err(CoreError::InvalidInput(format!(
                    "Office.js operation requires {}, but session host is {}",
                    operation.host(),
                    session.host
                )));
            }
            if !session
                .capabilities
                .iter()
                .any(|capability| capability == operation.capability())
            {
                return Err(CoreError::InvalidInput(format!(
                    "Office.js session does not advertise required capability: {}",
                    operation.capability()
                )));
            }
            let operation_id = bridge.enqueue(
                session_id,
                serde_json::to_value(operation).map_err(|error| {
                    CoreError::InvalidInput(format!("Invalid Office.js operation: {error}"))
                })?,
            )?;
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5 * 60);
            loop {
                if let Some(result) = bridge.take_result(&operation_id) {
                    let mut value = serde_json::to_value(result).map_err(|error| {
                        CoreError::Internal(format!(
                            "Office live result serialization failed: {error}"
                        ))
                    })?;
                    if let Some(object) = value.as_object_mut() {
                        object.insert(
                            "kind".to_string(),
                            Value::String("officeLiveOperationResult".to_string()),
                        );
                    }
                    break Ok(value);
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(CoreError::Internal(format!(
                        "Office.js operation timed out waiting for the authorized host: {operation_id}"
                    )));
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        }
        _ => unreachable!("non-live action passed to execute_live_action"),
    }
    .map_err(|error| {
        CoreError::Internal(format!("Office live bridge response serialization failed: {error}"))
    })?;
    let content = serde_json::to_string_pretty(&payload).map_err(|error| {
        CoreError::Internal(format!(
            "Office live bridge response serialization failed: {error}"
        ))
    })?;
    let is_error = payload
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| status != "ok" && status != "ready");
    Ok(ToolResult {
        call_id: call_id.to_string(),
        content,
        is_error,
        artifacts: Some(payload),
    })
}

#[async_trait]
impl Tool for OfficeArtifactTool {
    fn name(&self) -> &str {
        "office_artifact"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::FileSystem, ToolCategory::Process]
    }

    fn requires_confirmation(&self, args: &Value) -> bool {
        args.get("action")
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|action| {
                action.eq_ignore_ascii_case("execute")
                    || action.eq_ignore_ascii_case("decide")
                    || action.eq_ignore_ascii_case("restore")
                    || action.eq_ignore_ascii_case("live_pairing")
                    || action.eq_ignore_ascii_case("live_execute")
            })
    }

    fn confirmation_message(&self, args: &Value) -> Option<String> {
        self.requires_confirmation(args).then(|| {
            let action = args
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("update");
            format!("Run transactional Office artifact action: {action}")
        })
    }

    fn is_read_only(&self, args: &Value) -> bool {
        args.get("action")
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|action| {
                action.eq_ignore_ascii_case("capabilities")
                    || action.eq_ignore_ascii_case("inspect")
                    || action.eq_ignore_ascii_case("assess")
                    || action.eq_ignore_ascii_case("live_status")
            })
    }

    fn resource_keys(&self, args: &Value) -> Vec<String> {
        let mut keys = Vec::new();
        if let Some(root) = args.get("workspace_root").and_then(Value::as_str) {
            keys.push(format!("file:{}", root.trim().replace('\\', "/")));
        }
        if let Some(request) = args.get("request") {
            for field in ["source", "destination"] {
                if let Some(path) = request.get(field).and_then(Value::as_str) {
                    keys.push(format!("file:{}", path.trim().replace('\\', "/")));
                }
            }
        }
        if let Some(source) = args.get("source").and_then(Value::as_str) {
            keys.push(format!("file:{}", source.trim().replace('\\', "/")));
        }
        if let Some(id) = args
            .get("candidate_id")
            .or_else(|| args.get("receipt_id"))
            .or_else(|| args.get("live_session_id"))
            .and_then(Value::as_str)
        {
            keys.push(format!("office-artifact:{id}"));
        }
        keys.sort();
        keys.dedup();
        keys
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments,
            db,
            source_scope,
            ..
        } = context;
        let args: OfficeArtifactArgs = serde_json::from_str(arguments).map_err(|error| {
            CoreError::InvalidInput(format!("Invalid office_artifact arguments: {error}"))
        })?;
        let workspace = resolve_workspace(&args.workspace_root, db, source_scope)?;
        if matches!(
            args.action,
            OfficeArtifactAction::LiveStatus
                | OfficeArtifactAction::LivePairing
                | OfficeArtifactAction::LiveExecute
        ) {
            let _ = workspace;
            return execute_live_action(call_id, &args).await;
        }
        let app_data = app_data_dir_from_db(db)?;
        let (engine_args, request_json) = engine_arguments(&args)?;
        let execution = office_runtime::execute_office_artifact_engine(
            &app_data,
            &workspace,
            &engine_args,
            &request_json,
        )
        .await?;
        let artifacts = if execution.stdout.is_empty() {
            serde_json::to_value(&execution).ok()
        } else {
            let response: OfficeEngineResponse =
                serde_json::from_str(&execution.stdout).map_err(|error| {
                    CoreError::Internal(format!(
                        "Office artifact engine returned an invalid typed response: {error}"
                    ))
                })?;
            Some(serde_json::to_value(response).map_err(|error| {
                CoreError::Internal(format!(
                    "Office artifact response serialization failed: {error}"
                ))
            })?)
        };
        let content = if execution.stdout.is_empty() {
            execution.stderr.clone()
        } else if execution.stderr.is_empty() {
            execution.stdout.clone()
        } else {
            format!("{}\n\nDiagnostics:\n{}", execution.stdout, execution.stderr)
        };
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content,
            is_error: !execution.success,
            artifacts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_definition_exposes_candidate_lifecycle() {
        let tool = OfficeArtifactTool;
        let schema = tool.parameters_schema();
        let actions = schema["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum");
        assert!(actions.contains(&json!("execute")));
        assert!(actions.contains(&json!("inspect")));
        assert!(actions.contains(&json!("decide")));
        assert!(actions.contains(&json!("restore")));
        assert!(actions.contains(&json!("live_status")));
        assert!(actions.contains(&json!("live_execute")));
    }

    #[test]
    fn lifecycle_actions_have_correct_mutability() {
        let tool = OfficeArtifactTool;
        assert!(tool.is_read_only(&json!({"action": "assess"})));
        assert!(tool.is_read_only(&json!({"action": "inspect"})));
        assert!(!tool.requires_confirmation(&json!({"action": "capabilities"})));
        assert!(tool.requires_confirmation(&json!({"action": "execute"})));
        assert!(tool.requires_confirmation(&json!({"action": "decide"})));
        assert!(tool.requires_confirmation(&json!({"action": " DECIDE "})));
        assert!(tool.requires_confirmation(&json!({"action": "RESTORE"})));
        assert!(tool.requires_confirmation(&json!({"action": "live_pairing"})));
        assert!(tool.requires_confirmation(&json!({"action": "LIVE_EXECUTE"})));
        assert!(tool.is_read_only(&json!({"action": "live_status"})));
        assert!(!tool.is_read_only(&json!({"action": "RESTORE"})));
    }

    #[test]
    fn command_builder_requires_action_specific_identifiers() {
        let error = engine_arguments(&OfficeArtifactArgs {
            action: OfficeArtifactAction::Decide,
            workspace_root: ".".to_string(),
            request: None,
            source: None,
            format: None,
            candidate_id: None,
            decision: None,
            receipt_id: None,
            live_session_id: None,
            live_operation: None,
        })
        .unwrap_err();
        assert!(error.to_string().contains("candidate_id"));
    }

    #[test]
    fn rust_boundary_rejects_coerced_versions_and_operation_types() {
        let invalid_version = serde_json::from_value::<OfficeArtifactArgs>(json!({
            "action": "execute",
            "workspace_root": ".",
            "request": {
                "requestVersion": 2.9,
                "format": "xlsx",
                "intent": "modify",
                "operations": []
            }
        }));
        assert!(invalid_version.is_err());

        let invalid_boolean = serde_json::from_value::<OfficeArtifactArgs>(json!({
            "action": "execute",
            "workspace_root": ".",
            "request": {
                "requestVersion": 2,
                "format": "docx",
                "intent": "modify",
                "operations": [{
                    "op": "replace",
                    "find": "A",
                    "replace": "B",
                    "allowStyleMerge": "false"
                }]
            }
        }));
        assert!(invalid_boolean.is_err());
    }

    #[test]
    fn engine_responses_are_kind_discriminated_and_typed() {
        let outcome = serde_json::from_value::<OfficeEngineResponse>(json!({
            "kind": "officeArtifactOutcome",
            "status": "candidate",
            "candidateId": "aabbcc",
            "candidatePath": "/tmp/artifact.docx",
            "candidateSha256": "00",
            "manifest": {"status": "pass"}
        }));
        assert!(matches!(outcome, Ok(OfficeEngineResponse::Outcome { .. })));

        let malformed_error = serde_json::from_value::<OfficeEngineResponse>(json!({
            "kind": "officeArtifactError",
            "message": "missing stable code",
            "stage": "execute",
            "retryable": false
        }));
        assert!(malformed_error.is_err());
    }
}
