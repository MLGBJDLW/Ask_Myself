---
name: ppt-generation
description: Generate beautiful, structured PowerPoint (.pptx) decks via the Nexa `ppt_generate` tool. Activates when the user asks for slides, decks, presentations, "make a PPT", "turn this into a deck", or provides content that would be much clearer in slide form (reports, pitches, summaries, roadmaps).
---

# PPT Generation Skill

You are composing a slide deck for the Nexa desktop app. Your output is a JSON spec passed to the `ppt_generate` tool, which renders a `.pptx` file on disk. Treat this as **authoring**, not prompting: every slide is a deliberate choice of layout, copy, and pacing.

## When to Use

Trigger this skill when the user:
- Explicitly asks for "slides", "a deck", "a presentation", "PPT", "PPTX", or "PowerPoint"
- Asks to "summarize this as slides" or "turn this into a presentation"
- Requests content that would be dramatically clearer as a deck (board updates, quarterly reports, pitch decks, training material, roadmaps)
- Has long-form content (>500 words) that would benefit from chunked visual delivery

Do NOT trigger for:
- Simple Q&A or conversational responses
- Single-page documents → use `generate_docx`
- Tabular data analysis → use `generate_xlsx`
- Markdown articles / blog posts → return the markdown directly

## Mental Model

The `ppt_generate` tool takes a **structured JSON spec** and renders it to `.pptx` in the user's chosen folder. You (the LLM) are the *storyteller*; the renderer is the *designer*. Your job is to compose a spec that:
1. **Opens strong** — a `title` slide, possibly followed by `agenda`
2. **Uses the right layout for each idea** — don't force everything into `body`
3. **Closes deliberately** — `quote` or `section` works well for a finale

Aim for **density of meaning, not density of text**. A 10-slide deck with one clear idea per slide beats a 5-slide deck crammed with bullets.

## Tool Signature

Invoke the tool with exactly two arguments:

| Arg    | Type     | Required | Notes |
|--------|----------|----------|-------|
| `path` | string   | yes      | Absolute path ending in `.pptx`. Must live inside a registered source directory (the user sees these in the Sources panel). On Windows use forward slashes or escaped backslashes. |
| `spec` | object   | yes      | See schema below. |

The renderer itself lives in the desktop frontend — the Rust tool hands the spec to the Tauri layer, which writes the `.pptx` using the bundled PPT engine. The file appears in the user's source library and opens with a single click.

`spec` top-level fields:

| Field             | Type                   | Required | Purpose |
|-------------------|------------------------|----------|---------|
| `title`           | string                 | yes      | Deck title (also metadata) |
| `subtitle`        | string                 | no       | Deck subtitle (shown on cover if no `title` slide supplies one) |
| `author`          | string                 | no       | Author / team — shown on cover, stored in metadata |
| `theme`           | string \| object       | no       | `"nexa-light"` (default), `"nexa-dark"`, or a custom theme object |
| `slides`          | Slide[]                | yes      | Ordered array; one entry = one rendered slide |
| `notes_per_slide` | string[]               | no       | Parallel array to `slides`; each string becomes speaker notes for the slide at the same index. Use `""` to skip a slide. |

## The Eight Layouts — Choose Wisely

Each slide object has a `layout` discriminator plus layout-specific fields. Every layout also accepts an optional `background` override (hex color) — use sparingly.

### 1. `title` — Cover

Fields: `title` (string), `subtitle?`, `author?`, `date?`, `image_url?` (background photo).

Use for: slide 1 only, in nearly every deck. `image_url` turns the cover into an image-backed hero.

Do: keep `title` ≤ 6 words; pair with a short `subtitle` that sets up the "why".
Don't: use `title` mid-deck as a filler — that's what `section` is for.

### 2. `agenda` — Numbered Outline

Fields: `title?` (defaults to "Agenda"), `items` (string[], 3–6 entries).

Use for: slide 2 of any deck longer than 6 slides. Sets expectations, earns trust.

Do: phrase items as short noun phrases ("Revenue", "Product", "Road ahead").
Don't: use full sentences or more than 6 items (use a second agenda slide if needed).

### 3. `body` — Title + Bullets or Paragraph

Fields: `title` (string), `bullets?` (string[]) **or** `paragraph?` (string), `image_url?` (optional right-side image).

Use for: the default workhorse slide. Pick **either** bullets **or** paragraph — not both.

Do: keep bullets ≤ 10 words each, max 5 bullets; lead with a noun or verb, not "The".
Don't: dump a paragraph into `bullets` as a single entry; don't combine prose + bullets on one slide.

### 4. `two_column` — Side-by-Side

Fields: `title` (string), `left` and `right`, each a `ColCont` with `heading?`, `bullets?` or `paragraph?`, `image_url?`.

Use for: compare/contrast, before/after, problem/solution, shipped/next, pros/cons.

Do: keep both columns balanced in length; use parallel grammar ("Build X" / "Ship Y").
Don't: force three ideas into two columns — split into two slides instead.

### 5. `stat` — Big Numbers

Fields: `title?`, `stats` (array of 1–4 items, each `{ value, label, caption? }`).

Use for: KPIs, revenue, user counts, growth rates, market size.

Do: make `value` the hero — short and bold (`"$4.2M"`, `"97%"`, `"12"`); use `label` for the tight descriptor ("ARR", "Net retention"); use `caption` for context ("+38% YoY").
Don't: put more than 4 stats on one slide; don't pad `value` with units that belong in `label`.

### 6. `quote` — Pull Quote

Fields: `text` (string), `attribution?` (string), `image_url?` (optional portrait).

Use for: testimonials, customer voices, founder vision, industry quotes that anchor a section.

Do: keep the quote under 25 words; trim to the punchline.
Don't: use marketing fluff or paraphrased quotes — use real words from real people, or omit.

### 7. `section` — Chapter Break

Fields: `title` (string), `subtitle?`.

Use for: signaling transitions in decks longer than 8 slides ("Part 2: Product"); also a strong closer ("Let's build this").

Do: minimal words — one bold statement. Inverted full-color background by design.
Don't: use `section` more than every 3–4 slides; it loses meaning.

### 8. `image_full` — Full-Bleed Image

Fields: `image_url` (string, required), `title?`, `caption?`.

Use for: emotional opens/closes, product hero shots, a single photo that tells the whole story.

Do: use when the image IS the message; overlay text only if it adds meaning.
Don't: use for decorative filler. `image_url` MUST be a direct image URL (see Image URL Discipline).

## Themes

Two built-in themes matched to the Nexa app tokens:

- `"nexa-light"` — white background, dark text, teal accent (default; use for most professional decks)
- `"nexa-dark"` — near-black background, light text, teal accent (use for tech/modern/product topics)

Custom theme object (all fields optional; omitted fields fall back to `nexa-light`):

| Token             | Purpose                                 | Example       |
|-------------------|-----------------------------------------|---------------|
| `background`      | Default slide background                | `"#0B0F14"`   |
| `surface`         | Card / column backgrounds               | `"#131821"`   |
| `text_primary`    | Titles, stat values                     | `"#F4F6F8"`   |
| `text_secondary`  | Body, captions, labels                  | `"#9BA7B4"`   |
| `accent`          | Numbers, dividers, agenda markers       | `"#2DD4BF"`   |
| `accent_soft`     | Highlighted fills, section backgrounds  | `"#0F3B37"`   |
| `font_heading`    | Title typeface name                     | `"Inter"`     |
| `font_body`       | Body typeface name                      | `"Inter"`     |

Rule: **pick one theme per deck**. Don't switch mid-deck — it breaks visual coherence.

## Design Patterns That Work

### 5-Slide Pitch

1. `title` — Product name + tagline
2. `body` — Problem (3 bullets)
3. `stat` — Market size / traction
4. `body` — Solution (3 bullets)
5. `section` — "Let's build this"

### 10-Slide Board Update

1. `title` — "Q{N} {Year} Review"
2. `agenda` — 4–5 items
3. `stat` — Headline KPIs (ARR, NRR, NPS)
4. `body` — Wins (3 bullets)
5. `body` — Misses (3 bullets, candid)
6. `two_column` — This quarter vs next quarter
7. `quote` — Customer or team voice
8. `stat` — Pipeline / forecast
9. `body` — Asks of the board
10. `section` — "Thank you"

### 3-Slide Flash Update

1. `title` — Topic + date
2. `stat` — The one number that matters
3. `body` — Next steps (3 bullets)

## Image URL Discipline

When you include an `image_url`:

- **Do:** use direct image URLs from `images.unsplash.com`, `images.pexels.com`, or URLs the user explicitly provided
- **Don't:** link to `google.com/search?...`, `bing.com/images/...`, or any HTML page
- **Don't:** use image URLs from private/authenticated CDNs (they'll 401 at render time)
- **Fallback:** if unsure, omit the image — leave the slide image-less and it will still look clean

A broken image will render as a placeholder and degrade the whole deck. When in doubt, leave it out.

## Anti-Patterns

| ❌ Don't | ✅ Do |
|---------|-------|
| 10 × `body` slides with 10 bullets each | Mix layouts; use `stat`, `quote`, `section` |
| Paragraphs as bullets ("The product empowers users by...") | Short noun-phrase bullets ("Enterprise SSO", "Audit log") |
| Slide titles that repeat the deck title | Each title advances the story |
| Fill every `image_url` with random stock | Only add images where they truly add context |
| Mix themes mid-deck | One theme per deck |
| Forget `notes_per_slide` | Add brief speaker notes; they help the user present |
| Over 20 slides for a routine update | Cap at 10–12; split into decks if needed |
| Put the conclusion only in speaker notes | The last visible slide must land the point |

## Full Example

```json
{
  "path": "/Users/me/Documents/sources/acme-q4.pptx",
  "spec": {
    "title": "Acme Q4 2025 Review",
    "subtitle": "Revenue, Product, Road Ahead",
    "author": "Finance & Product",
    "theme": "nexa-dark",
    "slides": [
      { "layout": "title", "title": "Acme Q4 2025", "subtitle": "Revenue, Product, Road Ahead", "author": "Finance & Product" },
      { "layout": "agenda", "items": ["Highlights", "Revenue", "Product", "Customers", "Road ahead"] },
      { "layout": "section", "title": "Highlights" },
      { "layout": "stat", "title": "Q4 at a glance", "stats": [
        { "value": "$4.2M", "label": "ARR", "caption": "+38% YoY" },
        { "value": "97%", "label": "Net retention" },
        { "value": "12", "label": "New markets" }
      ]},
      { "layout": "body", "title": "Revenue drivers", "bullets": ["Enterprise expansion (+62%)", "Mid-market self-serve unlock", "Two new reseller partnerships"] },
      { "layout": "quote", "text": "This is the best quarter of engagement we've shipped.", "attribution": "Head of Product" },
      { "layout": "two_column", "title": "Product: shipped vs next",
        "left":  { "heading": "Shipped", "bullets": ["Multi-agent flows", "Inline citations", "Encrypted vault"] },
        "right": { "heading": "Next",    "bullets": ["Shared workspaces", "Mobile companion", "Evals V2"] }
      },
      { "layout": "section", "title": "Let's keep going" }
    ],
    "notes_per_slide": [
      "Welcome — 30s intro, set the tone.",
      "",
      "",
      "Walk through each KPI; pause after retention.",
      "Call out the partnerships by name if audience is external.",
      "Let the quote breathe — 5 seconds of silence.",
      "Contrast past with future — this is the pivot.",
      "Close with asks; leave room for discussion."
    ]
  }
}
```

## Return Format

After the tool call succeeds, the frontend auto-renders and saves the deck. You'll receive a tool result; relay to the user with one short sentence:

> "Saved your deck to `~/Documents/sources/acme-q4.pptx`. Click Open to preview or Reveal to show it in the file explorer."

Don't paste the deck JSON back at the user unless they explicitly ask for it. If the tool errors (invalid path, unregistered source directory, malformed spec), surface the error message verbatim and offer one concrete fix.
