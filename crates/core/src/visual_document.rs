//! Lightweight visual artifact extraction for documents.
//!
//! This module focuses on deterministic, local extraction that can run during
//! indexing: Office chart XML, embedded Office media metadata, standalone image
//! metadata, and PDF embedded-image OCR. Full page rendering and vision-model
//! interpretation can be layered on top without changing the stored artifact
//! shape.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use zip::ZipArchive;

use crate::error::CoreError;

const MAX_VISUAL_ARTIFACTS_PER_DOCUMENT: usize = 32;
const MAX_SERIES_ITEMS: usize = 12;
const MAX_SUMMARY_CHARS: usize = 4_000;

#[derive(Debug, Clone)]
pub struct ParsedVisualArtifact {
    pub artifact_index: i32,
    pub kind: String,
    pub source: String,
    pub location: Option<String>,
    pub title: Option<String>,
    pub summary: String,
    pub extracted_text: Option<String>,
    pub chart_type: Option<String>,
    pub confidence: f32,
    pub metadata: HashMap<String, String>,
}

impl ParsedVisualArtifact {
    pub fn to_chunk_content(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!("[Visual artifact: {}]", self.kind));
        lines.push(format!("Source: {}", self.source));
        if let Some(location) = self.location.as_deref().filter(|value| !value.is_empty()) {
            lines.push(format!("Location: {location}"));
        }
        if let Some(title) = self.title.as_deref().filter(|value| !value.is_empty()) {
            lines.push(format!("Title: {title}"));
        }
        if let Some(chart_type) = self.chart_type.as_deref().filter(|value| !value.is_empty()) {
            lines.push(format!("Chart type: {chart_type}"));
        }
        lines.push(format!("Confidence: {:.2}", self.confidence));
        lines.push(String::new());
        lines.push(self.summary.clone());
        if let Some(text) = self
            .extracted_text
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            lines.push(String::new());
            lines.push("Extracted visible text:".to_string());
            lines.push(text.to_string());
        }
        lines.join("\n")
    }
}

pub fn build_visual_artifact_metadata(
    artifact: &ParsedVisualArtifact,
) -> serde_json::Map<String, serde_json::Value> {
    let mut meta = serde_json::Map::new();
    meta.insert("visual_artifact".to_string(), serde_json::Value::Bool(true));
    meta.insert(
        "artifact_index".to_string(),
        serde_json::Value::Number(serde_json::Number::from(artifact.artifact_index)),
    );
    meta.insert(
        "kind".to_string(),
        serde_json::Value::String(artifact.kind.clone()),
    );
    meta.insert(
        "source".to_string(),
        serde_json::Value::String(artifact.source.clone()),
    );
    if let Some(location) = &artifact.location {
        meta.insert(
            "location".to_string(),
            serde_json::Value::String(location.clone()),
        );
    }
    if let Some(title) = &artifact.title {
        meta.insert(
            "title".to_string(),
            serde_json::Value::String(title.clone()),
        );
    }
    if let Some(chart_type) = &artifact.chart_type {
        meta.insert(
            "chart_type".to_string(),
            serde_json::Value::String(chart_type.clone()),
        );
    }
    if let Some(number) = serde_json::Number::from_f64(artifact.confidence as f64) {
        meta.insert("confidence".to_string(), serde_json::Value::Number(number));
    }
    for (key, value) in &artifact.metadata {
        meta.insert(
            format!("artifact_{key}"),
            serde_json::Value::String(value.clone()),
        );
    }
    meta
}

pub fn annotate_document_metadata(
    metadata: &mut HashMap<String, String>,
    artifacts: &[ParsedVisualArtifact],
) {
    if artifacts.is_empty() {
        return;
    }
    metadata.insert(
        "visual_artifact_count".to_string(),
        artifacts.len().to_string(),
    );
    let kinds = artifacts
        .iter()
        .map(|artifact| artifact.kind.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(",");
    metadata.insert("visual_artifact_kinds".to_string(), kinds);
}

pub fn redact_visual_artifacts(
    artifacts: &mut [ParsedVisualArtifact],
    redact: impl Fn(&str) -> String,
) {
    for artifact in artifacts {
        artifact.summary = redact(&artifact.summary);
        if let Some(text) = artifact.extracted_text.as_mut() {
            *text = redact(text);
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum OoxmlPackageKind {
    Docx,
    Pptx,
    Xlsx,
}

impl OoxmlPackageKind {
    fn chart_prefix(self) -> &'static str {
        match self {
            Self::Docx => "word/charts/",
            Self::Pptx => "ppt/charts/",
            Self::Xlsx => "xl/charts/",
        }
    }

    fn media_prefix(self) -> &'static str {
        match self {
            Self::Docx => "word/media/",
            Self::Pptx => "ppt/media/",
            Self::Xlsx => "xl/media/",
        }
    }

    fn source_label(self) -> &'static str {
        match self {
            Self::Docx => "docx_ooxml",
            Self::Pptx => "pptx_ooxml",
            Self::Xlsx => "xlsx_ooxml",
        }
    }
}

pub fn extract_ooxml_visual_artifacts(
    bytes: &[u8],
    package_kind: OoxmlPackageKind,
) -> Vec<ParsedVisualArtifact> {
    let mut artifacts = Vec::new();
    let mut archive = match ZipArchive::new(Cursor::new(bytes)) {
        Ok(archive) => archive,
        Err(err) => {
            tracing::debug!("OOXML visual extraction skipped: {err}");
            return artifacts;
        }
    };

    for i in 0..archive.len() {
        if artifacts.len() >= MAX_VISUAL_ARTIFACTS_PER_DOCUMENT {
            break;
        }
        let mut entry = match archive.by_index(i) {
            Ok(entry) => entry,
            Err(err) => {
                tracing::debug!("OOXML visual entry open failed: {err}");
                continue;
            }
        };
        let name = entry.name().replace('\\', "/");
        if name.starts_with(package_kind.chart_prefix()) && name.ends_with(".xml") {
            let mut xml = String::new();
            if let Err(err) = entry.read_to_string(&mut xml) {
                tracing::debug!("OOXML chart read failed for {name}: {err}");
                continue;
            }
            if let Some(mut artifact) =
                parse_ooxml_chart_artifact(&xml, package_kind.source_label(), &name)
            {
                artifact.artifact_index = artifacts.len() as i32;
                artifacts.push(artifact);
            }
            continue;
        }

        if name.starts_with(package_kind.media_prefix()) {
            let mut media_bytes = Vec::new();
            if let Err(err) = entry.read_to_end(&mut media_bytes) {
                tracing::debug!("OOXML media read failed for {name}: {err}");
                continue;
            }
            if let Some(mut artifact) =
                media_artifact_from_bytes(&name, package_kind.source_label(), &media_bytes)
            {
                artifact.artifact_index = artifacts.len() as i32;
                artifacts.push(artifact);
            }
        }
    }

    artifacts
}

pub fn image_file_visual_artifact(
    file_name: &str,
    mime_type: &str,
    bytes: &[u8],
    extracted_text: Option<String>,
    confidence: f32,
) -> ParsedVisualArtifact {
    let mut metadata = HashMap::new();
    metadata.insert("mime_type".to_string(), mime_type.to_string());
    metadata.insert("byte_size".to_string(), bytes.len().to_string());
    let dimensions = image::load_from_memory(bytes)
        .ok()
        .map(|image| (image.width(), image.height()));
    if let Some((width, height)) = dimensions {
        metadata.insert("width".to_string(), width.to_string());
        metadata.insert("height".to_string(), height.to_string());
    }

    let mut summary = format!("Standalone image file '{file_name}'");
    if let Some((width, height)) = dimensions {
        summary.push_str(&format!(" with dimensions {width}x{height}px"));
    }
    summary.push('.');
    if let Some(text) = extracted_text
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        summary.push_str(" OCR/vision extracted visible text from the image.");
        metadata.insert("has_extracted_text".to_string(), "true".to_string());
        metadata.insert("extracted_text_chars".to_string(), text.len().to_string());
    } else {
        summary.push_str(" No visible text was extracted.");
        metadata.insert("has_extracted_text".to_string(), "false".to_string());
    }

    ParsedVisualArtifact {
        artifact_index: 0,
        kind: "image".to_string(),
        source: "image_file".to_string(),
        location: Some(file_name.to_string()),
        title: Some(file_name.to_string()),
        summary,
        extracted_text,
        chart_type: None,
        confidence,
        metadata,
    }
}

pub fn extract_pdf_visual_artifacts(
    pdf_bytes: &[u8],
    ocr_config: &crate::ocr::OcrConfig,
    llm_provider: Option<&dyn crate::llm::LlmProvider>,
) -> Vec<ParsedVisualArtifact> {
    let mut artifacts = Vec::new();
    let doc = match lopdf::Document::load_mem(pdf_bytes) {
        Ok(doc) => doc,
        Err(err) => {
            tracing::debug!("PDF visual extraction skipped: {err}");
            return artifacts;
        }
    };

    let pages: Vec<(u32, lopdf::ObjectId)> = doc.get_pages().into_iter().collect();
    for (page_number, page_id) in pages {
        if artifacts.len() >= MAX_VISUAL_ARTIFACTS_PER_DOCUMENT {
            break;
        }
        let images = crate::ocr::extract_images_from_pdf_page(&doc, page_id);
        for image in images {
            if artifacts.len() >= MAX_VISUAL_ARTIFACTS_PER_DOCUMENT {
                break;
            }
            let width = image.width();
            let height = image.height();
            let mut png = Cursor::new(Vec::new());
            let ocr_result = if image.write_to(&mut png, image::ImageFormat::Png).is_ok() {
                crate::ocr::extract_text_from_image(
                    png.get_ref(),
                    "image/png",
                    ocr_config,
                    llm_provider,
                )
                .ok()
            } else {
                None
            };
            let extracted_text = ocr_result
                .as_ref()
                .map(|result| result.full_text.trim().to_string())
                .filter(|text| !text.is_empty());
            let confidence = ocr_result
                .as_ref()
                .map(|result| result.avg_confidence)
                .unwrap_or(0.45);
            let mut metadata = HashMap::new();
            metadata.insert("page".to_string(), page_number.to_string());
            metadata.insert("width".to_string(), width.to_string());
            metadata.insert("height".to_string(), height.to_string());
            metadata.insert(
                "has_extracted_text".to_string(),
                extracted_text.is_some().to_string(),
            );
            if let Some(result) = &ocr_result {
                metadata.insert("ocr_source".to_string(), format!("{:?}", result.source));
            }

            let summary = if extracted_text.is_some() {
                format!(
                    "PDF page {page_number} contains an embedded image ({width}x{height}px). Text was extracted from this visual region."
                )
            } else {
                format!(
                    "PDF page {page_number} contains an embedded image ({width}x{height}px). No text was extracted, so chart or diagram semantics may require page rendering or a vision model."
                )
            };

            artifacts.push(ParsedVisualArtifact {
                artifact_index: artifacts.len() as i32,
                kind: "embedded_image".to_string(),
                source: "pdf_embedded_image".to_string(),
                location: Some(format!("page {page_number}")),
                title: Some(format!("PDF page {page_number} image")),
                summary,
                extracted_text,
                chart_type: None,
                confidence,
                metadata,
            });
        }
    }

    artifacts
}

fn media_artifact_from_bytes(
    path: &str,
    source_label: &str,
    bytes: &[u8],
) -> Option<ParsedVisualArtifact> {
    let mime_type = mime_type_for_media_path(path)?;
    let dimensions = image::load_from_memory(bytes)
        .ok()
        .map(|image| (image.width(), image.height()));
    let mut metadata = HashMap::new();
    metadata.insert("mime_type".to_string(), mime_type.to_string());
    metadata.insert("byte_size".to_string(), bytes.len().to_string());
    if let Some((width, height)) = dimensions {
        metadata.insert("width".to_string(), width.to_string());
        metadata.insert("height".to_string(), height.to_string());
    }
    let summary = if let Some((width, height)) = dimensions {
        format!("Embedded image asset at {path} ({mime_type}, {width}x{height}px).")
    } else {
        format!(
            "Embedded image asset at {path} ({mime_type}, {} bytes).",
            bytes.len()
        )
    };

    Some(ParsedVisualArtifact {
        artifact_index: 0,
        kind: "embedded_image".to_string(),
        source: source_label.to_string(),
        location: Some(path.to_string()),
        title: path.rsplit('/').next().map(str::to_string),
        summary,
        extracted_text: None,
        chart_type: None,
        confidence: 0.5,
        metadata,
    })
}

fn mime_type_for_media_path(path: &str) -> Option<&'static str> {
    match path.rsplit('.').next()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "tif" | "tiff" => Some("image/tiff"),
        "emf" => Some("image/emf"),
        "wmf" => Some("image/wmf"),
        _ => None,
    }
}

#[derive(Debug, Clone, Default)]
struct XmlNode {
    name: String,
    _attrs: HashMap<String, String>,
    children: Vec<XmlNode>,
    text: String,
}

impl XmlNode {
    fn child(&self, name: &str) -> Option<&XmlNode> {
        self.children.iter().find(|child| child.name == name)
    }

    fn text_content(&self) -> String {
        let mut out = self.text.clone();
        for child in &self.children {
            out.push_str(&child.text_content());
        }
        out
    }
}

fn parse_ooxml_chart_artifact(
    xml: &str,
    source_label: &str,
    location: &str,
) -> Option<ParsedVisualArtifact> {
    let root = parse_xml(xml).ok()?;
    let chart_type = chart_type_for_tree(&root).unwrap_or("chart").to_string();
    let title = first_chart_title(&root);
    let series = chart_series(&root);
    let mut metadata = HashMap::new();
    metadata.insert("chart_type".to_string(), chart_type.clone());
    metadata.insert("series_count".to_string(), series.len().to_string());

    let mut lines = Vec::new();
    lines.push(format!(
        "OOXML chart at {location}; type: {}; title: {}.",
        chart_type,
        title.as_deref().unwrap_or("(untitled)")
    ));
    if series.is_empty() {
        lines.push("No cached chart series data was found in the OOXML package.".to_string());
    } else {
        lines.push("Cached series data:".to_string());
        for (idx, item) in series.iter().enumerate() {
            lines.push(format!(
                "- Series {}: {}",
                idx + 1,
                item.to_summary_line(MAX_SERIES_ITEMS)
            ));
        }
    }

    Some(ParsedVisualArtifact {
        artifact_index: 0,
        kind: "chart".to_string(),
        source: source_label.to_string(),
        location: Some(location.to_string()),
        title,
        summary: compact_chars(&lines.join("\n"), MAX_SUMMARY_CHARS),
        extracted_text: None,
        chart_type: Some(chart_type),
        confidence: 0.9,
        metadata,
    })
}

#[derive(Debug, Clone, Default)]
struct ChartSeries {
    name: Option<String>,
    categories: Vec<String>,
    values: Vec<String>,
}

impl ChartSeries {
    fn to_summary_line(&self, max_items: usize) -> String {
        let name = self.name.as_deref().unwrap_or("(unnamed)");
        let categories = compact_list(&self.categories, max_items);
        let values = compact_list(&self.values, max_items);
        match (categories.is_empty(), values.is_empty()) {
            (false, false) => format!("{name}; categories={categories}; values={values}"),
            (false, true) => format!("{name}; categories={categories}"),
            (true, false) => format!("{name}; values={values}"),
            (true, true) => name.to_string(),
        }
    }
}

fn compact_list(values: &[String], max_items: usize) -> String {
    if values.is_empty() {
        return String::new();
    }
    let mut shown = values.iter().take(max_items).cloned().collect::<Vec<_>>();
    if values.len() > max_items {
        shown.push(format!("... +{} more", values.len() - max_items));
    }
    format!("[{}]", shown.join(", "))
}

fn chart_series(root: &XmlNode) -> Vec<ChartSeries> {
    descendants_named(root, "ser")
        .into_iter()
        .map(|ser| ChartSeries {
            name: ser.child("tx").and_then(first_v_text),
            categories: ser.child("cat").map(collect_v_texts).unwrap_or_default(),
            values: ser.child("val").map(collect_v_texts).unwrap_or_default(),
        })
        .collect()
}

fn first_chart_title(root: &XmlNode) -> Option<String> {
    descendants_named(root, "title")
        .into_iter()
        .find_map(|node| {
            let text = descendants_named(node, "t")
                .into_iter()
                .map(XmlNode::text_content)
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string();
            (!text.is_empty()).then_some(text)
        })
}

fn first_v_text(node: &XmlNode) -> Option<String> {
    descendants_named(node, "v").into_iter().find_map(|node| {
        let text = node.text_content().trim().to_string();
        (!text.is_empty()).then_some(text)
    })
}

fn collect_v_texts(node: &XmlNode) -> Vec<String> {
    descendants_named(node, "v")
        .into_iter()
        .filter_map(|node| {
            let text = node.text_content().trim().to_string();
            (!text.is_empty()).then_some(text)
        })
        .collect()
}

fn chart_type_for_tree(root: &XmlNode) -> Option<&'static str> {
    const CHART_TYPES: &[(&str, &str)] = &[
        ("barChart", "bar"),
        ("bar3DChart", "bar_3d"),
        ("lineChart", "line"),
        ("line3DChart", "line_3d"),
        ("pieChart", "pie"),
        ("pie3DChart", "pie_3d"),
        ("doughnutChart", "doughnut"),
        ("areaChart", "area"),
        ("area3DChart", "area_3d"),
        ("scatterChart", "scatter"),
        ("bubbleChart", "bubble"),
        ("radarChart", "radar"),
        ("surfaceChart", "surface"),
        ("stockChart", "stock"),
    ];
    CHART_TYPES.iter().find_map(|(xml_name, label)| {
        (!descendants_named(root, xml_name).is_empty()).then_some(*label)
    })
}

fn local_name(bytes: &[u8]) -> String {
    let raw = String::from_utf8_lossy(bytes);
    raw.rsplit(':').next().unwrap_or(raw.as_ref()).to_string()
}

fn attrs_for(
    reader: &Reader<&[u8]>,
    event: &BytesStart<'_>,
) -> Result<HashMap<String, String>, String> {
    let mut attrs = HashMap::new();
    for attr in event.attributes().with_checks(false) {
        let attr = attr.map_err(|e| e.to_string())?;
        let key = local_name(attr.key.as_ref());
        let value = attr
            .decode_and_unescape_value(reader)
            .map_err(|e| e.to_string())?
            .to_string();
        attrs.insert(key, value);
    }
    Ok(attrs)
}

fn parse_xml(xml: &str) -> Result<XmlNode, String> {
    let mut reader = Reader::from_str(xml);
    reader.trim_text(false);
    let mut buffer = Vec::new();
    let mut stack = vec![XmlNode {
        name: "__root".to_string(),
        ..XmlNode::default()
    }];

    loop {
        buffer.clear();
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) => stack.push(XmlNode {
                name: local_name(event.local_name().as_ref()),
                _attrs: attrs_for(&reader, &event)?,
                ..XmlNode::default()
            }),
            Ok(Event::Empty(event)) => {
                let node = XmlNode {
                    name: local_name(event.local_name().as_ref()),
                    _attrs: attrs_for(&reader, &event)?,
                    ..XmlNode::default()
                };
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                }
            }
            Ok(Event::Text(event)) => {
                if let Some(node) = stack.last_mut() {
                    node.text
                        .push_str(&event.unescape().map_err(|e| e.to_string())?);
                }
            }
            Ok(Event::CData(event)) => {
                if let Some(node) = stack.last_mut() {
                    node.text.push_str(&String::from_utf8_lossy(event.as_ref()));
                }
            }
            Ok(Event::End(_)) if stack.len() > 1 => {
                let node = stack.pop().expect("stack has node");
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(e.to_string()),
            _ => {}
        }
    }

    stack
        .pop()
        .ok_or_else(|| "XML parse stack ended empty".to_string())
}

fn descendants_named<'a>(node: &'a XmlNode, name: &'a str) -> Vec<&'a XmlNode> {
    let mut out = Vec::new();
    collect_descendants_named(node, name, &mut out);
    out
}

fn collect_descendants_named<'a>(node: &'a XmlNode, name: &str, out: &mut Vec<&'a XmlNode>) {
    if node.name == name {
        out.push(node);
    }
    for child in &node.children {
        collect_descendants_named(child, name, out);
    }
}

fn compact_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out = String::with_capacity(max_chars + 3);
    for ch in value.chars().take(max_chars) {
        out.push(ch);
    }
    out.push_str("...");
    out
}

#[allow(dead_code)]
fn _core_error_for_visual_extraction(message: impl Into<String>) -> CoreError {
    CoreError::Parse(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ooxml_chart_series() {
        let xml = r#"
        <c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart">
          <c:chart>
            <c:title><c:tx><c:rich><a:p xmlns:a="a"><a:r><a:t>Quarterly Sales</a:t></a:r></a:p></c:rich></c:tx></c:title>
            <c:plotArea>
              <c:barChart>
                <c:ser>
                  <c:tx><c:strRef><c:strCache><c:pt idx="0"><c:v>Revenue</c:v></c:pt></c:strCache></c:strRef></c:tx>
                  <c:cat><c:strRef><c:strCache><c:pt idx="0"><c:v>Q1</c:v></c:pt><c:pt idx="1"><c:v>Q2</c:v></c:pt></c:strCache></c:strRef></c:cat>
                  <c:val><c:numRef><c:numCache><c:pt idx="0"><c:v>10</c:v></c:pt><c:pt idx="1"><c:v>14</c:v></c:pt></c:numCache></c:numRef></c:val>
                </c:ser>
              </c:barChart>
            </c:plotArea>
          </c:chart>
        </c:chartSpace>
        "#;

        let artifact = parse_ooxml_chart_artifact(xml, "xlsx_ooxml", "xl/charts/chart1.xml")
            .expect("chart artifact");

        assert_eq!(artifact.kind, "chart");
        assert_eq!(artifact.chart_type.as_deref(), Some("bar"));
        assert_eq!(artifact.title.as_deref(), Some("Quarterly Sales"));
        assert!(artifact.summary.contains("Revenue"));
        assert!(artifact.summary.contains("Q1"));
        assert!(artifact.summary.contains("14"));
    }
}
