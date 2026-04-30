## XLSX Playbook

### Workbook Structure
- Use a summary/dashboard sheet first for KPIs, charts, and key decisions.
- Separate raw data, assumptions, calculations, and outputs when the workbook needs auditability.
- Use tables or well-labeled ranges for structured data; freeze header rows and enable filters.
- Set widths, number formats, date formats, and print orientation deliberately.

### Formula Safety
- Use formulas for derived values. Do not hardcode calculated totals, rates, deltas, or scenario outputs.
- Put model assumptions in explicit input cells and reference them from formulas.
- Avoid saving files opened with `data_only=True`; it strips or hides formula intent.
- Recalculate with LibreOffice when available, then scan for formula errors such as `#REF!`, `#VALUE!`, `#DIV/0!`, `#NAME?`, and `#N/A`.

### Presentation Quality
- Use conditional formatting sparingly and only where it improves scanning.
- Use positive/negative/neutral colors consistently, but follow existing templates if present.
- Keep charts near the source summary they explain and label axes clearly.
- Validate the workbook and render or convert for visual QA when layout matters.
