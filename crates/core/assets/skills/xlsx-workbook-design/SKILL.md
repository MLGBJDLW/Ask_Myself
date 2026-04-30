---
name: xlsx-workbook-design
description: Create, edit, analyze, recalculate, and validate Excel XLSX workbooks with Python-backed workflows. Activate for XLSX files, Excel spreadsheets, workbooks, dashboards, financial models, formulas, charts, tables, pivot-style summaries, data cleaning, or spreadsheet QA; use with `doc-script-editor`, openpyxl, pandas, and LibreOffice recalculation when available.
---

## Workflow
1. Use `doc-script-editor` for file operations: `check`, `create_xlsx`, `replace`, `extract`, `version`, `unpack`, `pack`, `recalc_xlsx`, `render`, `convert`, and `validate`.
2. Run `scripts/xlsx_audit.py --path <file> --pretty` before editing existing workbooks and after generating formula-heavy files.
3. For a new workbook, prefer a JSON spec or a short Python script using `openpyxl`; use pandas only for data loading/transforms, then format with `openpyxl`.
4. For financial or scenario models, put assumptions in input cells and formulas in calculation cells. Do not hardcode derived numbers.
5. After writing formulas, run `recalc_xlsx` when LibreOffice is available, then validate and treat formula errors as blocking.
6. For existing workbooks, snapshot first and preserve formulas, named ranges, charts, styles, filters, freeze panes, and sheet visibility.

## Quality Rules
1. Put an executive summary or dashboard first when the workbook is user-facing.
2. Keep raw data, assumptions, calculations, and outputs on separate sheets when the file will be audited.
3. Use formulas for derived metrics; include clear labels and number formats.
4. Freeze header rows, enable filters, and set column widths explicitly.
5. Add charts only when they improve trend, comparison, or distribution reading.
6. Never save a workbook loaded with `data_only=True`; that can destroy formulas.

## Reference
Read `references/xlsx-playbook.md` for formula safety, layout, recalculation, and QA guidance.

## Script
Use `scripts/xlsx_audit.py` for a deterministic XLSX JSON inventory: sheets, dimensions, rows, cells, formulas, formula errors, tables, drawings, autofilters, frozen panes, calculation metadata, and warnings. It uses only Python stdlib and reads OOXML directly.
