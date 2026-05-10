## PPTX Playbook

### Story And Structure
- When source material is loose, use the deck planner to create message titles, slide count, layout choices, notes, and source-link preservation before rendering.
- Build a narrative arc: title, agenda or framing, problem, evidence, options, recommendation, summary, and Q&A only when useful.
- Use section dividers for major shifts in the story.
- Prefer comparison layouts for tradeoffs, timeline layouts for plans, and dashboard layouts for metrics.
- Keep one decision or message per slide.
- Use native editable charts for numeric evidence. Avoid screenshot charts unless the source visual itself is the point.
- Use process, timeline, comparison, and matrix slides to replace dense bullet lists.

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
- Keep source URLs as clickable slide links or notes. Do not replace a real citation with a search-results URL.
- Use native editable charts and tables for data. Keep screenshots only when the original visual form matters.

### Visual QA
- Render slides and inspect for overlap, overflow, low contrast, clipped text, missing images, wrong aspect ratios, and excessive density.
- Run the visual QA script even when rendered images are not available; it catches geometry-level overlap, edge, contrast, and text-capacity risks from OOXML.
- Every content slide should have a visual anchor: chart, image, icon, process, timeline, comparison, or stat callout.
- Keep editable content editable. Use full-slide images only for explicitly requested visual mockups or poster-style deliverables.
- Cite sources on slides for non-obvious data claims.
- Run the quality gate for generated decks. A production deck should pass with zero audit warnings, visual anchors on content slides, no empty placeholders, no low-editability full-slide images, and speaker notes when presenter-ready output is expected.

### Regression And Delivery
- Use the regression suite after renderer or QA changes to generate canonical executive brief, dashboard, and roadmap samples.
- Package final decks with the delivery pack so the PPTX, audit, visual QA, asset manifest, quality gate result, and manifest travel together.
