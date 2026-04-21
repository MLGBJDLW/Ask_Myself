---
name: office-document-design
description: Activate when generating DOCX, XLSX, or PPTX files via the generate_docx, generate_xlsx, or ppt_generate tools. Trigger on requests to create reports, spreadsheets, presentations, slides, decks, workbooks, or any polished office document output.
---

## Trigger
When creating DOCX, XLSX, or PPTX files via generate_docx/generate_xlsx/ppt_generate tools.

## Rules

### DOCX — Professional Documents
1. ALWAYS include: theme colors, title font, body font
2. Start with a cover page (title, subtitle, date/author note)
3. Use section rhythm: heading → 1-2 paragraphs → callout or table → next section
4. Insert callout boxes for key takeaways (tone: info for facts, warning for risks, success for wins)
5. Tables: use for any data with 3+ items. Always include header row
6. Bullet lists: max 7 items per list. Prefer grouped bullets with sub-headings

### XLSX — Data Workbooks
1. Sheet 1 = Summary dashboard (title banner, KPIs, key metrics)
2. Sheet 2+ = Detail data (raw data, calculations)
3. ALWAYS add charts when showing trends, comparisons, or distributions
4. Use formulas for derived values — never hardcode calculated numbers
5. Freeze header rows. Enable auto-filter. Set column widths explicitly
6. Use color coding: green for positive, red for negative, blue for neutral

### PPTX — Presentations
1. Max 6 bullets per slide. One message per slide
2. Storyboard: Title slide → Agenda → Content (3-7 slides) → Summary → Q&A
3. Use section divider slides between major topics
4. Comparison layout for pros/cons, before/after, option A vs B
5. Every data claim needs a source citation on the slide
6. Speaker notes: include detailed talking points (2-3 sentences per slide)

## Common Rules (All Formats)
- Choose colors that match the topic: blue for corporate, green for nature/health, orange for energy/startup
- Never use default black-and-white. Always set a theme
- Information hierarchy: most important info first, details second
- If user doesn't specify design, use professional blue theme: primary #2B579A, accent #217346
