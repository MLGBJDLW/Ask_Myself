---
name: docx-document-design
description: Create, edit, review, and validate Microsoft Word DOCX documents with Python-backed workflows. Activate for DOCX or Word files, reports, proposals, memos, contracts, tables, comments, tracked-change-aware edits, template-preserving document work, polished document generation, or conversion involving .docx output; use with `doc-script-editor`, python-docx, and OOXML unpack/pack.
---

## Workflow
1. Use `doc-script-editor` for file operations: `check`, `create_docx`, `replace`, `redact`, `extract`, `version`, `unpack`, `pack`, `render`, `convert`, and `validate`.
2. Run `scripts/docx_audit.py --path <file> --pretty` before editing existing DOCX files or after generating layout-sensitive documents.
3. For a new document, first create a reviewable Markdown or JSON blueprint with `create_file`/`edit_file`, then run `create_docx` or a file-backed Python renderer. Keep the blueprint unless the user asks for only the binary.
4. For complex layout, write a short workspace script with `create_file`/`edit_file`, pass input/output paths to it via `run_shell`, validate the DOCX, and delete temporary scratch scripts only after the output is verified.
5. For an existing document, create a version snapshot before risky edits, then preserve the original template, margins, headers, footers, styles, and tables.
6. For comments, tracked changes, relationship repair, embedded media, or template-sensitive surgery, unpack the DOCX, edit OOXML, repack, and validate.
7. After writing, validate the DOCX. Render or convert to PDF for visual QA when layout matters and the backends are available.

## Quality Rules
1. Use clear hierarchy: cover/title block, heading levels, short sections, tables for comparable data, and callouts for decisions, risks, or recommendations.
2. Keep body text editable, not flattened into images.
3. Use tables when there are three or more comparable rows. Include header rows and explicit widths when possible.
4. Keep bullet lists short and grouped under meaningful subheadings.
5. Use topic-appropriate theme colors, but let an existing template override generic styling.
6. Do not save or rebuild a template document from scratch unless the user asks for a redesign.

## Reference
Read `references/docx-playbook.md` for detailed layout, OOXML, and validation guidance.

## Script
Use `scripts/docx_audit.py` for a deterministic DOCX JSON inventory: paragraphs, tables, sections, images, headers, footers, comments, tracked changes, styles, relationships, and warnings. It uses only Python stdlib and reads OOXML directly.
