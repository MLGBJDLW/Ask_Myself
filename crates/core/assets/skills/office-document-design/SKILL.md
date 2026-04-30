---
name: office-document-design
description: Route polished Microsoft Office work to the correct Python-backed workflow. Activate when a request mentions DOCX, Word, reports, XLSX, Excel, spreadsheets, workbooks, PPTX, PowerPoint, slides, decks, presentations, or multi-format Office deliverables; pair this with the format-specific Office skill and `doc-script-editor`.
---

## Role
Use this skill as the Office router. It should keep routing concise, then rely on the specific format skill for design rules and `doc-script-editor` for disk operations.

## Routing
1. DOCX or Word report/memo/proposal: use `docx-document-design` plus `doc-script-editor`.
2. PPTX, slides, presentation, deck, speaker notes, template deck: use `pptx-presentation-design` plus `doc-script-editor`.
3. XLSX, Excel, spreadsheet, workbook, dashboard, financial model, formulas: use `xlsx-workbook-design` plus `doc-script-editor`.
4. Mixed deliverables: make a short artifact plan, then execute each file with the relevant format skill.
5. Existing Office file with exact layout preservation: use `doc-script-editor` versioning and OOXML unpack/pack before broad regeneration.

## Common Rules
1. Produce real editable Office files through Python-backed libraries or `doc-script-editor`; do not route Office work through deleted native generator tools.
2. Keep file bytes on disk. Never paste binary content or base64 Office blobs into tool arguments.
3. Validate after write with `edit_doc.py --path <file> validate` when possible.
4. Render or convert layout-sensitive outputs for visual QA when LibreOffice/Poppler are available.
5. For XLSX formulas, recalculate and scan for formula errors before completion.
6. Preserve existing templates unless the user explicitly asks for a redesign.

## Planning Resource
Read `scripts/outline-blueprint.md` when the request needs multiple Office files, a deck/report/workbook structure, or a quick content-to-format plan.
