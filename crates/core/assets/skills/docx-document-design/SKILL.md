---
name: docx-document-design
description: Create, edit, review, and validate Microsoft Word DOCX documents with Python-backed workflows. Activate for DOCX or Word files, reports, proposals, memos, contracts, tables, comments, tracked-change-aware edits, template-preserving document work, polished document generation, or conversion involving .docx output; use with `doc-script-editor`, python-docx, and OOXML unpack/pack.
---

## Workflow
1. Prefer the `office_artifact` requestVersion 2 lifecycle for DOCX create/modify/verify work: assess guarantees, execute to a candidate, inspect evidence, then publish/discard and retain the receipt. Use `doc-script-editor` direct commands for focused compatibility operations, OOXML surgery, rendering, and conversion.
2. Run `scripts/docx_audit.py --path <file> --pretty` before editing existing DOCX files or after generating layout-sensitive documents.
3. For a new professional document, create a reviewable DOCX Spec v2 JSON blueprint and pass it as the create operation's `spec`. The deterministic renderer provides `executive`, `technical`, `proposal`, and `memo` presets, numeric page/style tokens, fixed table geometry, repeated table headers, sections, headers/footers, PAGE fields, links, images with required alt text, captions, and callouts. Keep the blueprint unless the user asks for only the binary.
4. For complex layout, write a short workspace script with `create_file`/`edit_file`, pass input/output paths to it via `run_shell`, validate the DOCX, and delete temporary scratch scripts only after the output is verified.
5. For an existing document, use the Office artifact candidate lifecycle so risky edits are staged and validated before publication; pair strict preservation with sensitive-part hash evidence, and preserve the original template, margins, headers, footers, styles, and tables.
6. Use typed `add_comment`, `strip_comments`, `tracked_replace`, `accept_changes`, and `reject_changes` operations for the basic review lifecycle. Use OOXML surgery only for replies/resolution extensions, complex fields/content controls, relationship repair, embedded media, or template-sensitive work that the capability assessment explicitly supports.
7. Put required text/structure rules in `validation`; they execute before publication. Validate DOCX relationships, Content Types, XML, and backend open. For `quality: publish`, require rendered evidence and inspect every page. If the render backend is unavailable, report the blocker rather than claiming visual QA passed.
8. `redact` is visible-story replacement and must never be described as secure erasure. Use `secure_redact` for package-wide textual removal; it verifies UTF-8/UTF-16 absence and fails closed when media or embedded objects make proof impossible. Use `privacyScrub` when author/custom metadata must also be removed.

## Quality Rules
1. Use clear hierarchy: cover/title block, heading levels, short sections, tables for comparable data, and callouts for decisions, risks, or recommendations.
2. Keep body text editable, not flattened into images.
3. Use tables when there are three or more comparable rows. Include header rows and explicit widths when possible.
4. Keep bullet lists short and grouped under meaningful subheadings.
5. Use topic-appropriate theme colors, but let an existing template override generic styling.
6. Do not save or rebuild a template document from scratch unless the user asks for a redesign.

## Reference
Read `references/docx-playbook.md` for detailed layout, OOXML, and validation guidance.
Use `references/docx-spec-v2.schema.json` as the versioned creation contract; unknown root/block fields fail instead of being ignored.

## Script
Use `scripts/docx_audit.py` for a deterministic DOCX JSON inventory: paragraphs, tables, sections, images, headers, footers, comments, tracked changes, styles, relationships, and warnings. It uses only Python stdlib and reads OOXML directly.

Use `scripts/docx_renderer.py` for DOCX Spec v2 creation and `scripts/docx_review_editor.py` for typed comments/tracked-change operations. These are internal format adapters behind `office_artifact`; direct CLI use is a compatibility/debug path.
