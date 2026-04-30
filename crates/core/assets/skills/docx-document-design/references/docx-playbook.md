## DOCX Playbook

### Generation
- Start with a cover or title block: title, subtitle or scope, author/date line, and a short executive summary when appropriate.
- Use a repeating rhythm for long reports: heading, short framing paragraph, evidence block or table, then implication or next action.
- Use `python-docx` for normal sections, headings, paragraphs, tables, images, headers, footers, and page breaks.
- Use a template document when the user provides one; copy the template and edit inside it instead of recreating styles.

### Editing Existing DOCX
- Take a `version` snapshot before destructive or broad edits.
- Use text replacement only for small formatting-preserving changes.
- Use OOXML unpack/pack when the task involves comments, tracked changes, fields, relationships, embedded images, or precise template repair.
- Preserve headers, footers, numbering, section breaks, margins, and named styles unless asked to redesign.

### Visual Quality
- Turn any comparable list with three or more rows into a table with a header row.
- Highlight decisions, risks, and recommendations with visually distinct callouts.
- Avoid dense pages with uninterrupted paragraphs; break them with headings, tables, or callouts.
- Validate the file, then render or convert to PDF for a layout pass when page fidelity matters.
