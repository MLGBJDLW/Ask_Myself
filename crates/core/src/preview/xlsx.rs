use std::path::Path;

use calamine::{open_workbook, Data, Reader, Xlsx};

use super::model::{
    PreviewCell, PreviewMergedRange, PreviewSheet, StructuredPreview, WorkbookPreviewLimits,
    WORKBOOK_MAX_COLUMNS, WORKBOOK_MAX_ROWS, WORKBOOK_MAX_SHEETS,
};

pub fn preview_xlsx(path: &Path) -> Result<StructuredPreview, String> {
    let mut workbook: Xlsx<_> =
        open_workbook(path).map_err(|e| format!("XLSX open failed for {}: {e}", path.display()))?;
    let sheet_names = workbook.sheet_names();
    let mut sheets = Vec::new();
    let mut truncated = sheet_names.len() > WORKBOOK_MAX_SHEETS;

    for (index, name) in sheet_names
        .into_iter()
        .take(WORKBOOK_MAX_SHEETS)
        .enumerate()
    {
        let range = workbook
            .worksheet_range(&name)
            .map_err(|e| format!("XLSX sheet '{name}' read failed: {e}"))?;
        let formulas = workbook.worksheet_formula(&name).ok();
        let merged_ranges = workbook
            .worksheet_merge_cells(&name)
            .and_then(Result::ok)
            .unwrap_or_default();

        let row_count = range.height();
        let column_count = range.width();
        let preview_row_count = row_count.min(WORKBOOK_MAX_ROWS);
        let preview_column_count = column_count.min(WORKBOOK_MAX_COLUMNS);
        let sheet_truncated = row_count > WORKBOOK_MAX_ROWS || column_count > WORKBOOK_MAX_COLUMNS;
        truncated |= sheet_truncated;

        let start = range.start().unwrap_or((0, 0));
        let mut cells = Vec::new();
        for row_offset in 0..preview_row_count {
            for col_offset in 0..preview_column_count {
                let row = start.0 + row_offset as u32;
                let column = start.1 + col_offset as u32;
                let value = range
                    .get_value((row, column))
                    .cloned()
                    .unwrap_or(Data::Empty);
                let formula = formulas
                    .as_ref()
                    .and_then(|formula_range| formula_range.get_value((row, column)))
                    .filter(|formula| !formula.trim().is_empty())
                    .cloned();
                if matches!(value, Data::Empty) && formula.is_none() {
                    continue;
                }
                cells.push(PreviewCell {
                    row: row_offset as u32,
                    column: col_offset as u32,
                    value: value.to_string(),
                    data_type: data_type_name(&value).to_string(),
                    formula,
                });
            }
        }

        let merged_ranges = merged_ranges
            .into_iter()
            .filter_map(|range| {
                let start_row = range.start.0.saturating_sub(start.0) as usize;
                let start_column = range.start.1.saturating_sub(start.1) as usize;
                if start_row >= preview_row_count || start_column >= preview_column_count {
                    return None;
                }
                let end_row = (range.end.0.saturating_sub(start.0) as usize)
                    .min(preview_row_count.saturating_sub(1));
                let end_column = (range.end.1.saturating_sub(start.1) as usize)
                    .min(preview_column_count.saturating_sub(1));
                Some(PreviewMergedRange {
                    start_row: start_row as u32,
                    start_column: start_column as u32,
                    end_row: end_row as u32,
                    end_column: end_column as u32,
                })
            })
            .collect();

        sheets.push(PreviewSheet {
            name,
            index,
            row_count,
            column_count,
            preview_row_count,
            preview_column_count,
            cells,
            merged_ranges,
            truncated: sheet_truncated,
        });
    }

    Ok(StructuredPreview::Workbook {
        sheets,
        limits: WorkbookPreviewLimits::default(),
        truncated,
    })
}

fn data_type_name(value: &Data) -> &'static str {
    match value {
        Data::Int(_) => "number",
        Data::Float(_) => "number",
        Data::String(_) => "string",
        Data::Bool(_) => "boolean",
        Data::DateTime(_) => "date",
        Data::DateTimeIso(_) => "date",
        Data::DurationIso(_) => "duration",
        Data::Error(_) => "error",
        Data::Empty => "empty",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
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

    fn write_xlsx(path: &Path) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        write_entry(
            &mut zip,
            "[Content_Types].xml",
            br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>
</Types>"#,
        );
        write_entry(
            &mut zip,
            "_rels/.rels",
            br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#,
        );
        write_entry(
            &mut zip,
            "xl/_rels/workbook.xml.rels",
            br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>
</Relationships>"#,
        );
        write_entry(
            &mut zip,
            "xl/workbook.xml",
            br#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets>
</workbook>"#,
        );
        write_entry(
            &mut zip,
            "xl/sharedStrings.xml",
            br#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="2" uniqueCount="2">
<si><t>Name</t></si><si><t>Total</t></si>
</sst>"#,
        );
        write_entry(
            &mut zip,
            "xl/worksheets/sheet1.xml",
            br#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<dimension ref="A1:B3"/>
<sheetData>
<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>
<row r="2"><c r="A2" t="str"><v>Q1</v></c><c r="B2"><f>1+2</f><v>3</v></c></row>
</sheetData>
<mergeCells count="1"><mergeCell ref="A1:B1"/></mergeCells>
</worksheet>"#,
        );
        zip.finish().unwrap();
    }

    fn write_truncated_xlsx(path: &Path) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        write_entry(
            &mut zip,
            "[Content_Types].xml",
            br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
</Types>"#,
        );
        write_entry(
            &mut zip,
            "_rels/.rels",
            br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#,
        );
        write_entry(
            &mut zip,
            "xl/_rels/workbook.xml.rels",
            br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
</Relationships>"#,
        );
        write_entry(
            &mut zip,
            "xl/workbook.xml",
            br#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets><sheet name="Large" sheetId="1" r:id="rId1"/></sheets>
</workbook>"#,
        );
        write_entry(
            &mut zip,
            "xl/worksheets/sheet1.xml",
            br#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<dimension ref="A1:A501"/>
<sheetData>
<row r="1"><c r="A1"><v>1</v></c></row>
<row r="501"><c r="A501"><v>9</v></c></row>
</sheetData>
</worksheet>"#,
        );
        zip.finish().unwrap();
    }

    #[test]
    fn preview_xlsx_extracts_cells_formulas_and_merges() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sample.xlsx");
        write_xlsx(&path);

        let preview = preview_xlsx(&path).unwrap();

        let StructuredPreview::Workbook {
            sheets, truncated, ..
        } = preview
        else {
            panic!("expected workbook preview");
        };
        assert!(!truncated);
        assert_eq!(sheets.len(), 1);
        assert_eq!(sheets[0].name, "Sheet1");
        assert!(sheets[0]
            .cells
            .iter()
            .any(|cell| cell.value == "Name" && cell.data_type == "string"));
        assert!(sheets[0]
            .cells
            .iter()
            .any(|cell| cell.formula.as_deref() == Some("1+2") && cell.value == "3"));
        assert_eq!(sheets[0].merged_ranges.len(), 1);
    }

    #[test]
    fn preview_xlsx_marks_large_sheets_truncated() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("large.xlsx");
        write_truncated_xlsx(&path);

        let preview = preview_xlsx(&path).unwrap();

        let StructuredPreview::Workbook {
            sheets, truncated, ..
        } = preview
        else {
            panic!("expected workbook preview");
        };
        assert!(truncated);
        assert_eq!(sheets[0].row_count, 501);
        assert_eq!(sheets[0].preview_row_count, WORKBOOK_MAX_ROWS);
        assert!(sheets[0].truncated);
        assert!(sheets[0].cells.iter().any(|cell| cell.value == "1"));
        assert!(!sheets[0].cells.iter().any(|cell| cell.value == "9"));
    }
}
