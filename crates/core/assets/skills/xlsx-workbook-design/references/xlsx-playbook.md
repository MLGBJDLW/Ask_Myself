## XLSX Playbook

### Workbook Structure
- Use a summary/dashboard sheet first for KPIs, charts, and key decisions.
- Separate raw data, assumptions, calculations, and outputs when the workbook needs auditability.
- Use tables or well-labeled ranges for structured data; freeze header rows and enable filters.
- Set widths, number formats, date formats, and print orientation deliberately.
- Follow a capability ladder similar to OfficeCLI: inspect first, use structured workbook APIs for normal edits, and drop to raw OOXML only for advanced controls or relationship repair.
- For dashboards, organize inputs, staging/calculation, pivots or pivot-style summaries, visuals, and notes on separate sheets. Keep hidden helper sheets only when they make the workbook easier to use, not to hide fragile logic.
- Use named ranges for assumptions and scenario inputs so formulas are readable and reusable.

### Formula Safety
- Use formulas for derived values. Do not hardcode calculated totals, rates, deltas, or scenario outputs.
- Put model assumptions in explicit input cells and reference them from formulas.
- Avoid saving files opened with `data_only=True`; it strips or hides formula intent.
- Run `lint_xlsx` and `validate` after writing formulas. The default QA path does not use LibreOffice; it checks formula structure, missing sheet references, external workbook links, structured table references, cached error values, and `#REF!` tokens.
- Mark generated workbooks for automatic recalculation on open so Excel refreshes formula results when the user opens the file.
- For financial models, build assumptions, drivers, calculations, scenarios, sensitivity analysis, outputs, and checks as separate zones or sheets. Add balance/check rows where a broken model would otherwise look plausible.
- Prefer formulas, tables, and named ranges to pasted derived values. If a value is intentionally static, label it as an input or assumption.

### Presentation Quality
- Use conditional formatting sparingly and only where it improves scanning.
- Use positive/negative/neutral colors consistently, but follow existing templates if present.
- Keep charts near the source summary they explain and label axes clearly.
- Validate the workbook and render or convert for visual QA when layout matters.
- Use charts for trend, comparison, distribution, and composition; use sparklines for row-level trends; use conditional formatting for exceptions and thresholds.
- Freeze panes, filters, print areas, page orientation, and column widths are part of the deliverable, not optional polish.
- For user-facing dashboards, preview the workbook after generation and correct clipped headers, unreadable labels, empty charts, broken filters, and excessive blank regions.

### Cleanup
- Delete temporary scripts, CSV staging files, rendered sheet images, conversion outputs, and unpacked OOXML directories after a successful run unless they are requested deliverables or needed to diagnose a failure.
- If cleanup removes important debug artifacts after an error would hide the cause, keep them in a clearly named diagnostic folder and mention it.
