---
name: pptx-presentation-design
description: Create, edit, inspect, and validate PowerPoint PPTX presentations with Python-backed workflows. Activate for PPTX files, PowerPoint, slide decks, slides, presentations, pitch decks, speaker notes, slide templates, visual QA, editable deck generation, or deck extraction; use with `doc-script-editor`, python-pptx, and OOXML unpack/pack.
---

## Workflow
1. Use `doc-script-editor` for file operations: `check`, `create_pptx`, `insert_slide`, `replace`, `extract`, `version`, `unpack`, `pack`, `render`, `convert`, and `validate`.
2. Run `scripts/pptx_audit.py --path <file> --pretty` before adapting an existing deck/template and after generating decks that need QA.
3. For a new deck, create a concise JSON spec or a short Python script using `python-pptx`; keep text, charts, and shapes editable.
4. For an existing or template deck, inventory layouts/placeholders first, snapshot before edits, then preserve theme, masters, notes, and slide size.
5. Use OOXML unpack/pack for speaker notes, media replacement, relationship repair, master/layout work, or precise template surgery.
6. Validate after writing and render slides for visual QA when the backends are available.

## Quality Rules
1. One idea per slide. Put the message in the title, not only the body.
2. Use visual structure on content slides: chart, image, icon, timeline, comparison, process, stat callout, or diagram.
3. Avoid text-only slide runs and keep body bullets to six or fewer.
4. Include speaker notes when the user asks for presenter-ready output or when the deck tells a story.
5. Remove unused placeholders and empty shapes in template decks.
6. Do not use deleted native Office generators. Do not make a full-slide-image deck unless the user explicitly wants non-editable poster-style slides.

## Reference
Read `references/pptx-playbook.md` for template workflow, slide design rules, and QA checks.

## Script
Use `scripts/pptx_audit.py` for a deterministic PPTX JSON inventory: slide count, size, layouts, masters, themes, per-slide text, shapes, pictures, chart/image/notes relationships, empty placeholders, and warnings. It uses only Python stdlib and reads OOXML directly.
