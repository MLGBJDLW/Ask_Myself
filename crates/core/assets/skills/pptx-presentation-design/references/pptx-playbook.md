## PPTX Playbook

### Story And Structure
- When source material is loose, use the deck planner to create message titles, slide count, layout choices, notes, and source-link preservation before rendering.
- Build a narrative arc: title, agenda or framing, problem, evidence, options, recommendation, summary, and Q&A only when useful.
- Use section dividers for major shifts in the story.
- Prefer comparison layouts for tradeoffs, timeline layouts for plans, and dashboard layouts for metrics.
- Keep one decision or message per slide.
- Use native editable charts for numeric evidence. Avoid screenshot charts unless the source visual itself is the point.
- Use process, timeline, comparison, and matrix slides to replace dense bullet lists.
- Establish a visual contract before rendering: audience, industry, palette, type, background motif, icon language, page rhythm, and asset plan. Reuse that contract across slides instead of choosing a new look per page.
- Start from a renderer theme preset when no brand template is supplied: `consulting-clean`, `executive-midnight`, `editorial-ink`, `product-energy`, `healthcare-trust`, `finance-precision`, `education-bright`, or `industrial-contrast` provide stronger visual direction than the neutral defaults.
- Preserve a compact design brief in renderer metadata so later edits can recover why the theme, backgrounds, typography, and image approach were chosen.
- Assign each slide a page rhythm role: `anchor` for cover/section/close, `breathing` for visual-led pages, and `dense` for data or process pages.

### Template Workflow
- Inspect slide size, theme colors, layouts, placeholders, and existing style conventions before editing.
- Run the template profiler before using a `.pptx` as a template; choose layouts from its recommendations instead of guessing from names alone.
- Run the style profiler and map its `renderer_theme` tokens into the generated spec instead of copying a deck's private design wholesale.
- For reusable templates, run the template binder against the renderer spec. Render the bound spec with `--template` so slides use native layout indices and placeholders rather than approximating the template on blank slides. Template sample slides are cleared by default; use `preserve_template_slides: true` only for intentional append workflows.
- Reuse existing layouts when possible. When a template placeholder is not used, remove it rather than leaving an empty text box.
- Preserve speaker notes, comments, slide masters, custom layouts, and relationships unless the task requires changes.
- Use OOXML unpack/pack for master edits, media swaps, relationship fixes, or notes that `python-pptx` cannot express directly.

### Rewrite And Beautify
- Start with audit, then create a rewrite plan for existing decks that are too long, too dense, or visually inconsistent.
- For narrative changes, use the semantic rewriter instead of only reshuffling original slides. It should classify source content into context, problem, evidence, options, recommendation, plan, risk, and appendix, then rebuild the deck as a decision story.
- Preserve source facts and links, but rebuild weak slides into editable charts, tables, timelines, comparisons, processes, or matrices.
- For executive condensation, keep the decision path: context, evidence, options, recommendation, risks, and next step.
- Treat full-slide image decks as a red flag unless the user explicitly asked for poster-style output.

### Assets And Links
- Run the asset pack before delivery to inventory embedded media, external links, local image dependencies, and missing assets.
- Use the asset pack's image catalog output to decide image roles: landscape assets usually fit hero/background treatment, portrait assets usually need inline/side-by-side treatment, and declared roles override heuristics.
- Keep source URLs as clickable slide links or notes. Do not replace a real citation with a search-results URL.
- Use native editable charts and tables for data. Keep screenshots only when the original visual form matters.
- Use slide-level `background` when an image or branded backdrop improves the slide. For non-photo slides, use native editable background motifs such as `background_style: "soft_geometry"`, `"gradient_mesh"`, `"diagonal"`, `"blueprint_grid"`, `"paper_texture"`, `"clinical_grid"`, `"data_grid"`, or `"spotlight"`. Avoid long runs of flat solid backgrounds.
- Put supplied images in top-level `images` and reference them with `image_id`, `background_image_id`, or `@alias` so asset validation, replacement, and design reasoning stay consistent.
- Put repeated symbolic marks in top-level `icons` and reference them with `icon_id` or `@alias`. Use built-in editable icon names for simple symbols and image-backed aliases for brand marks.
- Treat SVG as a design source, not a guaranteed direct picture format in the editable renderer. For exact SVG-heavy aesthetics, prefer template-bound decks or rasterize only the background while keeping foreground text, charts, and shapes editable.

### Visual QA
- Render slides and inspect for overlap, overflow, low contrast, clipped text, missing images, wrong aspect ratios, and excessive density.
- Run the visual QA script even when rendered images are not available; it catches geometry-level overlap, edge, contrast, text-capacity risks, and spec-level design issues from OOXML/JSON.
- When rendered slide images exist, pass `--render-dir`; flat or nearly blank renders are flagged so a generated deck gets at least one screenshot sanity pass.
- Every content slide should have a visual anchor: chart, image, icon, process, timeline, comparison, or stat callout.
- Every slide should have a background decision: image, template chrome, native motif, or a deliberate flat utility background. A default solid fill is a warning sign, not a design style.
- Keep editable content editable. Use full-slide images only for explicitly requested visual mockups or poster-style deliverables.
- Cite sources on slides for non-obvious data claims.
- Run the quality gate for generated decks. A production deck should pass with zero audit warnings, visual anchors on content slides, no empty placeholders, no low-editability full-slide images, and speaker notes when presenter-ready output is expected.

### Regression And Delivery
- Use the regression suite after renderer or QA changes to generate canonical executive brief, dashboard, and roadmap samples.
- Package final decks with the delivery pack so the PPTX, audit, visual QA, asset manifest, quality gate result, and manifest travel together.
