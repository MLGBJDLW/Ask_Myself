## DOCX Playbook

### Generation
- Start with a cover or title block: title, subtitle or scope, author/date line, and a short executive summary when appropriate.
- Use a repeating rhythm for long reports: heading, short framing paragraph, evidence block or table, then implication or next action.
- Use `python-docx` for normal sections, headings, paragraphs, tables, images, headers, footers, and page breaks.
- Use a template document when the user provides one; copy the template and edit inside it instead of recreating styles.

### Editing Existing DOCX
- Take a `version` snapshot before destructive or broad edits.
- Use text replacement only for small formatting-preserving changes.
- Prefer the typed candidate lifecycle for comments, threaded replies, resolution state, tracked replacement, bookmarks, safe fields, content controls, protection, and template binding. Address a comment thread by its inspected `commentId`; do not guess IDs from visible order.
- Inspect `trackedChanges.unsupportedForAcceptReject` before accepting or rejecting revisions. Insertions and deletions are handled directly across Word story parts; moves, property/table revisions, conflicts, and custom-XML revisions must fail closed or use native Word automation.
- Use manual OOXML unpack/pack only for an explicitly inspected object that has no typed operation, and keep changed-part and relationship evidence.
- Preserve headers, footers, numbering, section breaks, margins, and named styles unless asked to redesign.

### Visual Quality
- Turn any comparable list with three or more rows into a table with a header row.
- Highlight decisions, risks, and recommendations with visually distinct callouts.
- Avoid dense pages with uninterrupted paragraphs; break them with headings, tables, or callouts.
- Validate the file, then render or convert to PDF for a layout pass when page fidelity matters.
