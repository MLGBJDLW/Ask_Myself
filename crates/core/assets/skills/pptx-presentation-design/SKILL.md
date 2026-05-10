---
name: pptx-presentation-design
description: Create, edit, inspect, and validate PowerPoint PPTX presentations with Python-backed workflows. Activate for PPTX files, PowerPoint, slide decks, slides, presentations, pitch decks, speaker notes, slide templates, visual QA, editable deck generation, or deck extraction; use with `doc-script-editor`, python-pptx, and OOXML unpack/pack.
---

## Workflow
1. For source material, run `scripts/pptx_deck_planner.py --input <notes.md> --out <spec.json>` when a renderer spec is not already available.
2. For a new editable deck, use `scripts/pptx_renderer.py --path <file> --spec <json>` or the `doc-script-editor create_pptx` compatibility command, which delegates to this renderer.
3. Use `doc-script-editor` for cross-format file operations: `check`, `insert_slide`, `replace`, `extract`, `version`, `unpack`, `pack`, `render`, `convert`, and `validate`.
4. For a template deck, run `scripts/pptx_template_bind.py --template <template.pptx> --spec <spec.json> --out <bound-spec.json>` before rendering; this profiles layouts/style and annotates each slide with template layout bindings.
5. For an existing deck rewrite, run `scripts/pptx_semantic_rewriter.py --path <file> --out <spec.json>` for a new semantic story, or `scripts/pptx_rewrite_plan.py --path <file> --out-spec <spec.json>` when you also need slide-level remediation actions.
6. Before rendering, run `scripts/pptx_asset_pack.py --spec <spec.json> --pretty` to catch missing local assets and preserve source links.
7. After writing, run `scripts/pptx_audit.py --path <file> --pretty` and `scripts/pptx_visual_qa.py --path <file> --pretty`; if rendered slide images exist, pass `--render-dir`.
8. Run `scripts/pptx_quality_gate.py --path <file> --visual-qa <visual.json> --pretty` on finished decks; use `--strict` or `--require-notes` for presenter-ready or production decks.
9. For final delivery, run `scripts/pptx_delivery_pack.py --path <file> --out-dir <dir> --pretty` to save the deck, audit, visual QA, asset manifest, and quality gate result together.
10. Use OOXML unpack/pack for speaker notes, media replacement, relationship repair, master/layout work, or precise template surgery.

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
Use `scripts/pptx_renderer.py` to create editable decks from JSON specs. Supported layouts are `title`, `agenda`, `body`, `two_column`, `stat`, `quote`, `section`, `image_full`, `table`, `timeline`, `process`, `comparison`, `matrix`, and `chart`; specs may include `theme`, `slide_size`, `footer`, slide-level `notes`, top-level `notes_per_slide`, `images`, `icons`, slide-level `background`, slide-level `icon`, and template reuse.

Prefer advanced editable layouts for non-trivial decks: `timeline` for roadmaps, `process` for workflows, `comparison` for tradeoffs, `matrix` for prioritization, and `chart` for native editable bar/column/line/area/stacked/pie/doughnut charts. `table` accepts either `string[][]` or an object with `headers`, `rows`, `column_widths`, `number_format`, `banded_rows`, and `caption`. Slides may include `links` or `citations`; the renderer writes them as small clickable source links.

Use `scripts/pptx_audit.py` for a deterministic PPTX JSON inventory: slide count, size, layouts, masters, themes, per-slide text, shapes, pictures, chart/image/notes relationships, empty placeholders, editability warnings, and speaker-note coverage. It uses only Python stdlib and reads OOXML directly.

Use `scripts/pptx_visual_qa.py` after rendering or generation for geometry-based visual checks: overlap, text overflow risk, edge margin risk, low contrast, missing rendered slide images, and nearly blank rendered screenshots. With `--spec`, it also produces a spec-level design review for visual anchors, background decisions, page rhythm, and dense text; with `--out-spec`, it can split over-dense body slides and preserve full titles in notes.

Use `scripts/pptx_quality_gate.py` after audit or generation to produce a pass/fail publishability signal. It checks warning budget, visual anchors, text density, empty placeholders, low-editability full-slide images, speaker notes coverage, visual QA failures, and native visual metrics; it exits non-zero when the deck fails.

Use `scripts/pptx_template_profile.py` when the user supplies a template or existing deck to adapt. It inventories slide layouts, placeholder types, chart/media relationships, and recommends layout indices for title, body, section, table, chart, and comparison use.

Use `scripts/pptx_style_profile.py` to learn a deck's theme colors, sampled colors, and title/body fonts, then feed its `renderer_theme` into new specs.

Use `scripts/pptx_template_bind.py` to turn a normal renderer spec into a template-bound spec. It uses the template profile and style profile to add `template_layout_index`, `template_layout_name`, and `template_binding` per slide, plus reusable renderer theme tokens. Rendering that bound spec with `--template` clears template sample slides by default, fills native template placeholders, and removes unused placeholders. Set top-level `preserve_template_slides: true` only when intentionally appending to an existing deck.

Use `scripts/pptx_deck_planner.py` to convert notes or source material into a renderer-ready spec with message titles, layout selection, speaker notes, preserved source links, slide-level `design_role`, industry-aware `background_style`, slide icons, and a metadata `design_brief`. The design brief mirrors PPT Master's Strategist discipline: canvas, page count, audience, industry, style objective, color scheme, icon approach, typography, and image usage are decided before rendering.

Theme presets: use `nexa-light` for neutral reports, `nexa-dark` for modern technical decks, `consulting-clean` for executive consulting pages, `executive-midnight` for high-contrast leadership briefs, `editorial-ink` for narrative/report decks, `product-energy` for product or launch stories, `healthcare-trust` for patient/clinical decks, `finance-precision` for finance/risk/market decks, `education-bright` for training/learning decks, and `industrial-contrast` for operations/manufacturing decks. Each preset carries a repeatable background motif so generated decks do not default to flat fills.

Use `scripts/pptx_semantic_rewriter.py` for existing PPTX files or plain source notes that need a new narrative, not only polishing. It classifies content into context, problem, evidence, options, recommendation, plan, risk, and appendix, then creates an editable decision-story spec with charts, comparisons, timelines, section breaks, notes, and preserved source links.

Use `scripts/pptx_rewrite_plan.py` for existing PPTX files that need condensation, executive rewrite, or visual restructuring. It turns audit findings into slide-level actions and a semantic rewrite spec.

Use `scripts/pptx_asset_pack.py` to inventory embedded media, external links, and renderer-spec image dependencies before generation or delivery. For top-level `images`, it reports local dimensions, orientation, recommended usage, and crop guidance so a landscape hero can become a background while a portrait image stays inline.

Use `scripts/pptx_regression_suite.py` to write or render canonical PPT samples for smoke testing renderer changes.

Use `scripts/pptx_delivery_pack.py` to assemble a final delivery directory with the PPTX, audit, visual QA, asset manifest, quality gate output, and manifest.

### Backgrounds And Visual Motifs

Do not let generated decks fall back to plain solid backgrounds unless the slide is intentionally utilitarian. For each deck, pick a repeatable visual motif before rendering: full-bleed photography, soft native geometry, diagonal section bands, template chrome, or a chart/diagram language. The renderer supports slide-level backgrounds:

```json
{
  "layout": "body",
  "title": "Market Shift",
  "bullets": ["Demand is fragmenting", "Premium segments are resilient"],
  "background": {
    "image_path": "/abs/workspace/assets/city.jpg",
    "fit": "cover",
    "overlay_color": "background_color",
    "overlay_transparency": 28,
    "style": "none"
  }
}
```

For editable abstract backdrops, use `background_style: "soft_geometry"`, `"gradient_mesh"`, `"diagonal"`, `"blueprint_grid"`, `"paper_texture"`, `"clinical_grid"`, `"data_grid"`, or `"spotlight"` without an image. The deck planner adds these motifs automatically; override them slide-by-slide when the content needs a stronger image, chart, or template treatment. Direct SVG files are not embedded by the python-pptx renderer; for exact SVG art, use a template-bound deck or convert the SVG to a raster preview while keeping foreground text, charts, and shapes editable.

### User Images And Asset Aliases

When a user supplies images, add them to top-level `images` and reference them by role instead of repeating paths. This keeps the design plan stable and lets `pptx_asset_pack.py` validate dependencies before rendering:

```json
{
  "images": {
    "hero": { "path": "/abs/assets/hero.jpg" },
    "diagram": "/abs/assets/architecture.png"
  },
  "slides": [
    { "layout": "title", "title": "Launch Plan", "background_image_id": "hero" },
    { "layout": "body", "title": "Architecture", "image_id": "diagram", "bullets": ["Editable text remains editable"] }
  ]
}
```

Use `background_image_id` for full-bleed or atmospheric backgrounds, `image_id` for normal foreground images, and `background: { "image_id": "hero", "fit": "cover", "overlay_transparency": 30 }` when overlay/crop control is needed. Prefix aliases with `@` if using them directly in `image` or `background.image`, e.g. `"image": "@diagram"`.

### Icons And Design Language

Use top-level `icons` when the deck needs a consistent symbolic language. Aliases may point to built-in editable icon names (`shield`, `trend`, `network`, `spark`, `check`) or to local/remote image files:

```json
{
  "icons": {
    "risk": "shield",
    "growth": "trend"
  },
  "slides": [
    { "layout": "body", "title": "Risk Controls", "icon_id": "risk", "background_style": "data_grid" },
    { "layout": "process", "title": "Growth Loop", "steps": [{ "title": "Signal", "icon_id": "growth" }] }
  ]
}
```

Prefer planner output for source notes: it infers an industry profile, chooses a theme/background set, assigns page rhythm, and adds icons. For hand-written specs, keep this same discipline: explicit theme, explicit backgrounds, stable image aliases, stable icon aliases, and a final `pptx_visual_qa.py --spec` pass before delivery.
