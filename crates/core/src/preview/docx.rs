use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use zip::ZipArchive;

use super::model::{
    PreviewAsset, PreviewBlock, PreviewListItem, PreviewRun, PreviewTableCell, PreviewTableRow,
    StructuredPreview,
};
use super::PreviewBuildOptions;

#[derive(Debug, Clone, Default)]
struct XmlNode {
    name: String,
    attrs: HashMap<String, String>,
    children: Vec<XmlNode>,
    text: String,
}

impl XmlNode {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.get(name).map(String::as_str)
    }

    fn child(&self, name: &str) -> Option<&XmlNode> {
        self.children.iter().find(|child| child.name == name)
    }

    fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a XmlNode> + 'a {
        self.children.iter().filter(move |child| child.name == name)
    }

    fn text_content(&self) -> String {
        let mut out = self.text.clone();
        for child in &self.children {
            out.push_str(&child.text_content());
        }
        out
    }
}

#[derive(Debug, Clone, Default)]
struct Relationship {
    target: String,
    target_mode: Option<String>,
    rel_type: String,
}

#[derive(Debug, Clone, Default)]
struct Numbering {
    num_to_format: HashMap<String, String>,
}

#[derive(Debug, Clone, Default)]
struct DocxContext {
    relationships: HashMap<String, Relationship>,
    numbering: Numbering,
    styles: HashMap<String, String>,
    assets: HashMap<String, PreviewAsset>,
}

#[derive(Debug, Clone, Default)]
struct RunStyle {
    bold: bool,
    italic: bool,
    underline: bool,
    color: Option<String>,
    background_color: Option<String>,
    font_size: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct ParsedParagraph {
    runs: Vec<PreviewRun>,
    alignment: Option<String>,
    heading_level: Option<u8>,
    list: Option<ListInfo>,
    images: Vec<String>,
    page_breaks: usize,
}

#[derive(Debug, Clone)]
struct ListInfo {
    ordered: bool,
    level: u8,
}

pub fn preview_docx(
    path: &Path,
    content_hash: &str,
    options: &PreviewBuildOptions,
) -> Result<StructuredPreview, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let document_xml = read_zip_text(&mut archive, "word/document.xml")?;
    let document = parse_xml(&document_xml)?;

    let rels = read_zip_text(&mut archive, "word/_rels/document.xml.rels")
        .ok()
        .map(|xml| parse_relationships(&xml))
        .transpose()?
        .unwrap_or_default();
    let numbering = read_zip_text(&mut archive, "word/numbering.xml")
        .ok()
        .map(|xml| parse_numbering(&xml))
        .transpose()?
        .unwrap_or_default();
    let styles = read_zip_text(&mut archive, "word/styles.xml")
        .ok()
        .map(|xml| parse_styles(&xml))
        .transpose()?
        .unwrap_or_default();

    let mut ctx = DocxContext {
        relationships: rels,
        numbering,
        styles,
        assets: HashMap::new(),
    };

    extract_image_assets(&mut archive, content_hash, options, &mut ctx)?;

    let body = document
        .child("document")
        .and_then(|node| node.child("body"))
        .or_else(|| document.child("body"))
        .ok_or_else(|| "DOCX missing word/document.xml body".to_string())?;
    let mut blocks = parse_blocks(body, &ctx);
    if blocks.is_empty() {
        blocks.push(PreviewBlock::Unsupported {
            message: "No structured DOCX content was found.".to_string(),
        });
    }

    let mut assets: Vec<PreviewAsset> = ctx.assets.values().cloned().collect();
    assets.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(StructuredPreview::Document { blocks, assets })
}

fn read_zip_text<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> Result<String, String> {
    let mut entry = archive
        .by_name(name)
        .map_err(|e| format!("zip entry {name} unavailable: {e}"))?;
    let mut xml = String::new();
    entry
        .read_to_string(&mut xml)
        .map_err(|e| format!("zip entry {name} read failed: {e}"))?;
    Ok(xml)
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
            Ok(Event::Start(event)) => {
                stack.push(XmlNode {
                    name: local_name(event.local_name().as_ref()),
                    attrs: attrs_for(&reader, &event)?,
                    ..XmlNode::default()
                });
            }
            Ok(Event::Empty(event)) => {
                let node = XmlNode {
                    name: local_name(event.local_name().as_ref()),
                    attrs: attrs_for(&reader, &event)?,
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
        .ok_or_else(|| "XML parser stack ended empty".to_string())
}

fn parse_relationships(xml: &str) -> Result<HashMap<String, Relationship>, String> {
    let root = parse_xml(xml)?;
    let mut rels = HashMap::new();
    for rel in descendants_named(&root, "Relationship") {
        let Some(id) = rel.attr("Id").or_else(|| rel.attr("id")) else {
            continue;
        };
        rels.insert(
            id.to_string(),
            Relationship {
                target: rel.attr("Target").unwrap_or_default().to_string(),
                target_mode: rel.attr("TargetMode").map(str::to_string),
                rel_type: rel.attr("Type").unwrap_or_default().to_string(),
            },
        );
    }
    Ok(rels)
}

fn parse_styles(xml: &str) -> Result<HashMap<String, String>, String> {
    let root = parse_xml(xml)?;
    let mut styles = HashMap::new();
    for style in descendants_named(&root, "style") {
        let Some(style_id) = style.attr("styleId") else {
            continue;
        };
        if let Some(name) = style.child("name").and_then(|node| node.attr("val")) {
            styles.insert(style_id.to_string(), name.to_string());
        }
    }
    Ok(styles)
}

fn parse_numbering(xml: &str) -> Result<Numbering, String> {
    let root = parse_xml(xml)?;
    let mut abstract_formats = HashMap::new();
    for abstract_num in descendants_named(&root, "abstractNum") {
        let Some(abstract_id) = abstract_num.attr("abstractNumId") else {
            continue;
        };
        let fmt = abstract_num
            .children_named("lvl")
            .find(|lvl| lvl.attr("ilvl").unwrap_or("0") == "0")
            .and_then(|lvl| lvl.child("numFmt"))
            .and_then(|fmt| fmt.attr("val"))
            .unwrap_or("bullet");
        abstract_formats.insert(abstract_id.to_string(), fmt.to_string());
    }

    let mut num_to_format = HashMap::new();
    for num in descendants_named(&root, "num") {
        let Some(num_id) = num.attr("numId") else {
            continue;
        };
        let Some(abstract_id) = num.child("abstractNumId").and_then(|node| node.attr("val")) else {
            continue;
        };
        if let Some(fmt) = abstract_formats.get(abstract_id) {
            num_to_format.insert(num_id.to_string(), fmt.clone());
        }
    }

    Ok(Numbering { num_to_format })
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

fn extract_image_assets<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    content_hash: &str,
    options: &PreviewBuildOptions,
    ctx: &mut DocxContext,
) -> Result<(), String> {
    let Some(cache_root) = options.asset_cache_dir.as_ref() else {
        return Ok(());
    };
    let asset_dir = cache_root.join(content_hash);
    fs::create_dir_all(&asset_dir).map_err(|e| e.to_string())?;

    for (rid, rel) in ctx.relationships.clone() {
        if !rel.rel_type.ends_with("/image") {
            continue;
        }
        let part = resolve_part_path("word/document.xml", &rel.target);
        let Ok(mut entry) = archive.by_name(&part) else {
            continue;
        };
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        let file_name = Path::new(&part)
            .file_name()
            .and_then(|name| name.to_str())
            .map(sanitize_file_name)
            .unwrap_or_else(|| format!("{rid}.bin"));
        let out_path = asset_dir.join(file_name);
        fs::write(&out_path, &bytes).map_err(|e| e.to_string())?;
        let mime_type = mime_for_asset_path(&part);
        ctx.assets.insert(
            rid.clone(),
            PreviewAsset {
                id: rid,
                kind: "image".to_string(),
                mime_type,
                path: out_path.to_string_lossy().to_string(),
                width: None,
                height: None,
            },
        );
    }
    Ok(())
}

fn resolve_part_path(base_part: &str, target: &str) -> String {
    if target.starts_with('/') {
        return target.trim_start_matches('/').replace('\\', "/");
    }
    let mut base = PathBuf::from(base_part.replace('\\', "/"));
    base.pop();
    base.push(target);
    normalize_zip_path(&base)
}

fn normalize_zip_path(path: &Path) -> String {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
            Component::ParentDir => {
                parts.pop();
            }
            _ => {}
        }
    }
    parts.join("/")
}

fn sanitize_file_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "asset.bin".to_string()
    } else {
        out
    }
}

fn mime_for_asset_path(path: &str) -> String {
    match Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn parse_blocks(parent: &XmlNode, ctx: &DocxContext) -> Vec<PreviewBlock> {
    let mut blocks = Vec::new();
    for child in &parent.children {
        match child.name.as_str() {
            "p" => push_paragraph_blocks(&mut blocks, parse_paragraph(child, ctx)),
            "tbl" => blocks.push(parse_table(child, ctx)),
            _ => {}
        }
    }
    blocks
}

fn push_paragraph_blocks(blocks: &mut Vec<PreviewBlock>, paragraph: ParsedParagraph) {
    let has_text = paragraph.runs.iter().any(|run| !run.text.trim().is_empty());
    if has_text {
        if let Some(list) = paragraph.list {
            if let Some(PreviewBlock::List {
                ordered,
                level,
                items,
            }) = blocks.last_mut()
            {
                if *ordered == list.ordered && *level == list.level {
                    items.push(PreviewListItem {
                        runs: paragraph.runs,
                    });
                } else {
                    blocks.push(PreviewBlock::List {
                        ordered: list.ordered,
                        level: list.level,
                        items: vec![PreviewListItem {
                            runs: paragraph.runs,
                        }],
                    });
                }
            } else {
                blocks.push(PreviewBlock::List {
                    ordered: list.ordered,
                    level: list.level,
                    items: vec![PreviewListItem {
                        runs: paragraph.runs,
                    }],
                });
            }
        } else if let Some(level) = paragraph.heading_level {
            blocks.push(PreviewBlock::Heading {
                level,
                runs: paragraph.runs,
                alignment: paragraph.alignment,
            });
        } else {
            blocks.push(PreviewBlock::Paragraph {
                runs: paragraph.runs,
                alignment: paragraph.alignment,
            });
        }
    }

    for asset_id in paragraph.images {
        blocks.push(PreviewBlock::Image {
            asset_id,
            alt: None,
        });
    }
    for _ in 0..paragraph.page_breaks {
        blocks.push(PreviewBlock::PageBreak);
    }
}

fn parse_paragraph(node: &XmlNode, ctx: &DocxContext) -> ParsedParagraph {
    let ppr = node.child("pPr");
    let style_id = ppr
        .and_then(|ppr| ppr.child("pStyle"))
        .and_then(|style| style.attr("val"));
    let heading_level = style_id.and_then(|id| heading_level_for(id, ctx));
    let alignment = ppr
        .and_then(|ppr| ppr.child("jc"))
        .and_then(|jc| jc.attr("val"))
        .map(str::to_string);
    let list = ppr.and_then(|ppr| parse_list_info(ppr, ctx));

    let mut paragraph = ParsedParagraph {
        alignment,
        heading_level,
        list,
        ..ParsedParagraph::default()
    };
    for child in &node.children {
        match child.name.as_str() {
            "r" | "hyperlink" => collect_runs(child, ctx, None, &mut paragraph),
            _ => {}
        }
    }
    paragraph
}

fn heading_level_for(style_id: &str, ctx: &DocxContext) -> Option<u8> {
    let compact = style_id.to_ascii_lowercase().replace(' ', "");
    if let Some(rest) = compact.strip_prefix("heading") {
        return rest
            .parse::<u8>()
            .ok()
            .filter(|level| (1..=6).contains(level));
    }
    let style_name = ctx
        .styles
        .get(style_id)
        .map(|name| name.to_ascii_lowercase().replace(' ', ""));
    style_name
        .as_deref()
        .and_then(|name| name.strip_prefix("heading"))
        .and_then(|level| level.parse::<u8>().ok())
        .filter(|level| (1..=6).contains(level))
}

fn parse_list_info(ppr: &XmlNode, ctx: &DocxContext) -> Option<ListInfo> {
    let num_pr = ppr.child("numPr")?;
    let num_id = num_pr.child("numId").and_then(|node| node.attr("val"))?;
    let level = num_pr
        .child("ilvl")
        .and_then(|node| node.attr("val"))
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(0);
    let fmt = ctx
        .numbering
        .num_to_format
        .get(num_id)
        .map(String::as_str)
        .unwrap_or("bullet");
    Some(ListInfo {
        ordered: !fmt.eq_ignore_ascii_case("bullet"),
        level,
    })
}

fn collect_runs(
    node: &XmlNode,
    ctx: &DocxContext,
    hyperlink: Option<String>,
    paragraph: &mut ParsedParagraph,
) {
    match node.name.as_str() {
        "hyperlink" => {
            let href = node
                .attr("id")
                .and_then(|id| ctx.relationships.get(id))
                .filter(|rel| rel.target_mode.as_deref() == Some("External"))
                .map(|rel| rel.target.clone())
                .or_else(|| node.attr("anchor").map(|anchor| format!("#{anchor}")))
                .or(hyperlink);
            for child in &node.children {
                collect_runs(child, ctx, href.clone(), paragraph);
            }
        }
        "r" => {
            let style = parse_run_style(node.child("rPr"));
            let mut text = String::new();
            for child in &node.children {
                match child.name.as_str() {
                    "t" => text.push_str(&child.text_content()),
                    "tab" => text.push('\t'),
                    "cr" => text.push('\n'),
                    "br" => {
                        if child.attr("type") == Some("page") {
                            paragraph.page_breaks += 1;
                        } else {
                            text.push('\n');
                        }
                    }
                    "drawing" | "pict" => {
                        collect_image_refs(child, ctx, paragraph);
                    }
                    _ => {}
                }
            }
            if !text.is_empty() {
                paragraph.runs.push(PreviewRun {
                    text,
                    bold: style.bold,
                    italic: style.italic,
                    underline: style.underline,
                    color: style.color,
                    background_color: style.background_color,
                    font_size: style.font_size,
                    hyperlink,
                });
            }
        }
        _ => {
            for child in &node.children {
                collect_runs(child, ctx, hyperlink.clone(), paragraph);
            }
        }
    }
}

fn parse_run_style(rpr: Option<&XmlNode>) -> RunStyle {
    let Some(rpr) = rpr else {
        return RunStyle::default();
    };
    RunStyle {
        bold: rpr.child("b").is_some(),
        italic: rpr.child("i").is_some(),
        underline: rpr.child("u").is_some(),
        color: rpr
            .child("color")
            .and_then(|node| node.attr("val"))
            .filter(|value| !value.eq_ignore_ascii_case("auto"))
            .map(|value| format!("#{value}")),
        background_color: rpr
            .child("highlight")
            .and_then(|node| node.attr("val"))
            .map(str::to_string),
        font_size: rpr
            .child("sz")
            .and_then(|node| node.attr("val"))
            .and_then(font_size_bucket),
    }
}

fn font_size_bucket(value: &str) -> Option<String> {
    let half_points = value.parse::<u32>().ok()?;
    let points = half_points / 2;
    Some(
        match points {
            0..=10 => "small",
            11..=13 => "normal",
            14..=18 => "large",
            _ => "xlarge",
        }
        .to_string(),
    )
}

fn collect_image_refs(node: &XmlNode, ctx: &DocxContext, paragraph: &mut ParsedParagraph) {
    for blip in descendants_named(node, "blip") {
        let Some(id) = blip.attr("embed").or_else(|| blip.attr("link")) else {
            continue;
        };
        if ctx.assets.contains_key(id) {
            paragraph.images.push(id.to_string());
        }
    }
}

fn parse_table(node: &XmlNode, ctx: &DocxContext) -> PreviewBlock {
    let rows = node
        .children_named("tr")
        .map(|row| PreviewTableRow {
            cells: row
                .children_named("tc")
                .map(|cell| PreviewTableCell {
                    blocks: parse_blocks(cell, ctx),
                })
                .collect(),
        })
        .collect();
    PreviewBlock::Table { rows }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;
    use zip::write::FileOptions;

    fn write_entry<W: Write + std::io::Seek>(
        zip: &mut zip::ZipWriter<W>,
        name: &str,
        value: &[u8],
    ) {
        zip.start_file(name, FileOptions::<()>::default()).unwrap();
        zip.write_all(value).unwrap();
    }

    fn write_docx(path: &Path) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        write_entry(&mut zip, "[Content_Types].xml", br#"<Types/>"#);
        write_entry(
            &mut zip,
            "word/_rels/document.xml.rels",
            br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rIdHyper" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com" TargetMode="External"/>
<Relationship Id="rIdImage" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>
</Relationships>"#,
        );
        write_entry(
            &mut zip,
            "word/styles.xml",
            br#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/></w:style>
</w:styles>"#,
        );
        write_entry(
            &mut zip,
            "word/numbering.xml",
            br#"<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:abstractNum w:abstractNumId="1"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum>
<w:num w:numId="7"><w:abstractNumId w:val="1"/></w:num>
</w:numbering>"#,
        );
        write_entry(
            &mut zip,
            "word/document.xml",
            br#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Executive Summary</w:t></w:r></w:p>
<w:p><w:r><w:rPr><w:b/><w:i/><w:u/><w:color w:val="FF0000"/></w:rPr><w:t>Important</w:t></w:r><w:hyperlink r:id="rIdHyper"><w:r><w:t> link</w:t></w:r></w:hyperlink></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="7"/></w:numPr></w:pPr><w:r><w:t>First item</w:t></w:r></w:p>
<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Cell A</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Cell B</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
<w:p><w:r><w:drawing><a:blip r:embed="rIdImage"/></w:drawing></w:r></w:p>
<w:p><w:r><w:br w:type="page"/></w:r></w:p>
</w:body></w:document>"#,
        );
        write_entry(&mut zip, "word/media/image1.png", b"png");
        zip.finish().unwrap();
    }

    #[test]
    fn preview_docx_extracts_structured_blocks_and_assets() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sample.docx");
        write_docx(&path);
        let options = PreviewBuildOptions {
            asset_cache_dir: Some(dir.path().join("assets")),
        };

        let preview = preview_docx(&path, "hash", &options).unwrap();

        let StructuredPreview::Document { blocks, assets } = preview else {
            panic!("expected document preview");
        };
        assert!(matches!(blocks[0], PreviewBlock::Heading { level: 1, .. }));
        assert!(blocks
            .iter()
            .any(|block| matches!(block, PreviewBlock::List { ordered: true, .. })));
        assert!(blocks
            .iter()
            .any(|block| matches!(block, PreviewBlock::Table { rows } if rows.len() == 1)));
        assert!(blocks.iter().any(
            |block| matches!(block, PreviewBlock::Image { asset_id, .. } if asset_id == "rIdImage")
        ));
        assert!(blocks
            .iter()
            .any(|block| matches!(block, PreviewBlock::PageBreak)));
        assert_eq!(assets.len(), 1);
        assert!(Path::new(&assets[0].path).exists());
    }
}
