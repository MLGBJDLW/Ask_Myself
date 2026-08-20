---
name: xlsx-workbook-design
description: Create, edit, analyze, lint, and validate Excel XLSX workbooks with Python-backed workflows. Activate for XLSX files, Excel spreadsheets, workbooks, dashboards, financial models, formulas, charts, tables, pivot-style summaries, data cleaning, or spreadsheet QA; use with `doc-script-editor`, openpyxl, pandas, and the skill-owned XLSX renderer.
---

## Workflow
1. Prefer the `office_artifact` requestVersion 2 lifecycle for XLSX create/modify/verify work; it separates candidates from publication and reports calculation evidence. Use `doc-script-editor` direct commands for focused compatibility operations and OOXML inspection.
2. For a new workbook, create or edit a JSON spec as a workspace file, then run `create_xlsx`; it delegates to `scripts/xlsx_model_renderer.py`.
3. Run `scripts/xlsx_audit.py --path <file> --pretty` before editing existing workbooks and after generating formula-heavy files.
4. Use pandas only for data loading/transforms, then format with `openpyxl`; do not use ad-hoc one-shot Python when the renderer spec covers the task.
5. For financial or scenario models, put assumptions in input cells and formulas in calculation cells. Do not hardcode derived numbers.
6. Express calculation truth explicitly: `static` means formula lint only, `compatible` means LibreOffice recalculated, and `native` means Excel COM recalculated. An empty formula cache is `not_calculated`, even if structural validation passes. Add a validation contract for required sheets/names, hardcode bans, minimum rows, sentinels, and `require_formula_cache` when cached values are mandatory.
7. For existing workbooks, inspect the preservation-risk inventory first. Text replacement uses precise OOXML part edits; broad library round-trips must not be used for complex workbooks. Transactional commands snapshot and publish only after validation.
8. For existing workbooks, prefer typed `set_value`, `set_formula`, `set_range`, `clear_range`, and `set_style` operations. They resolve sheets through workbook relationships, address names case-insensitively, treat values as literals, require formulas to be explicit, and patch only authorized worksheet parts. `set_style` reuses an existing `cellXfs` style id; creating new style records remains a creation-spec task.
9. Formula evidence includes a cache-independent definition fingerprint, formula kinds, dependency edges, cache coverage, and every OOXML `t=e` value including modern/unknown errors. Compatible recalculation is rejected if it changes the formula fingerprint. Dynamic arrays, data tables, spill references, or preservation-sensitive packages require the Excel-native adapter for strong calculation guarantees.
10. Use an OfficeCLI-style ladder: L1 read/audit, L2 structured workbook edits, and L3 raw OOXML only for features the normal writer cannot express. Prefer deterministic workbook state over ad-hoc cell poking.
11. For user-facing workbooks, render or preview important sheets after creation when tooling is available; fix clipped text, unusable widths, missing formats, and unreadable charts before delivery.
12. Remove temporary CSV extracts, Python conversion scratch files, rendered previews, and unpacked OOXML folders unless the user requested an audit/debug bundle.

## Quality Rules
1. Put an executive summary or dashboard first when the workbook is user-facing.
2. Keep raw data, assumptions, calculations, and outputs on separate sheets when the file will be audited.
3. Use formulas for derived metrics; include clear labels and number formats.
4. Freeze header rows, enable filters, and set column widths explicitly.
5. Add charts only when they improve trend, comparison, or distribution reading.
6. Never save a workbook loaded with `data_only=True`; that can destroy formulas.
7. Use Excel tables, named ranges, data validation, conditional formatting, sparklines, pivots or pivot-style summaries, and slicers only when they improve repeated use or scanning.
8. Financial models should separate assumptions, calculations, scenarios, sensitivity tables, and outputs. Every output number should trace back to inputs and formulas.

## Reference
Read `references/xlsx-playbook.md` for formula safety, layout, and QA guidance.

## Script
Use `scripts/xlsx_model_renderer.py` for structured workbook creation. It supports rows/records, formal Excel tables, formula fill-down/fill-right with translated references, named ranges, validations, conditional formatting, charts, column widths, calculation metadata, and internal formula QA without LibreOffice.

Minimal formula-model spec:

```json
{
  "title": "Revenue Model",
  "qa": { "min_formulas": 3 },
  "sheets": [
    {
      "name": "Model",
      "start_cell": "B4",
      "headers": ["Metric", "2024", "2025", "2026"],
      "rows": [["Revenue", 100, 120, 144], ["Margin", 0.4, 0.42, 0.45], ["Gross Profit", null, null, null]],
      "table": { "name": "ModelTable" },
      "formulas": [
        { "cell": "C6", "formula": "=C4*C5", "fill_down": { "to_cell": "E6" }, "number_format": "$#,##0" }
      ],
      "named_ranges": [{ "name": "BaseRevenue", "cell": "C4" }]
    }
  ]
}
```

Use `scripts/xlsx_audit.py` for a deterministic XLSX JSON inventory: sheets, dimensions, rows, cells, formulas, formula errors, tables, drawings, autofilters, frozen panes, calculation metadata, and warnings. It uses only Python stdlib and reads OOXML directly.
Use `scripts/xlsx_structured_editor.py` as the direct-OOXML typed edit adapter. It preserves every non-target package part byte-for-byte.
