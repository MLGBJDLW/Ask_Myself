---
name: fiction-writing
description: Use this skill when the user asks to write, plan, revise, continue, import, or diagnose fiction, including novels, Chinese-language fiction, web novels, short stories, serial chapters, scenes, characters, plot, canon, worldbuilding, prose style, genre beats, outlines, chapter cards, cliffhangers, continuity, or fiction revision. Also use for longform fiction project workflows, story bibles, serialized chapter production, reader-promise design, low-dash human prose, de-generic/de-AI polishing, and narrative quality checks.
---

# Fiction Writing

## Core Rule
Treat the user's existing draft, canon, naming, POV, tense, and intended genre as source-of-truth. Add craft structure only where it helps the current decision; do not force a formula onto material that is already working.

For Chinese-language fiction and web-serial work, optimize for the reader experience the user is trying to deliver: curiosity, dread, romance tension, catharsis, wonder, progression, face-slapping release, grief, warmth, or another named emotional promise. Structure, prose, hooks, and pacing serve that promise.

## First Pass
1. Identify the task type: discovery, premise, title, outline, longform project setup, chapter plan, scene/chapter draft, continuation, imported manuscript, character, worldbuilding, continuity, style, critique, or revision.
2. Extract the current constraints: language, genre, target reader/platform, emotional promise, POV, tense, tone, setting, canon facts, chapter/scene position, desired length, and whether output should be chat text or files.
3. Use minimum viable intake. For a new story, collect only the missing essentials first: core idea/genre, protagonist or relationship, central pressure, desired tone/POV, target length, and any reference works. Ask one focused question when missing canon would create continuity risk; otherwise make a labeled assumption and proceed.
4. Choose the smallest workflow that can safely satisfy the request:
   - Single scene/chapter: build a context pack, state the scene intent, draft, then run a quality gate.
   - Outline/planning: move from reader promise to premise, conflict engine, cast, structure, chapter cards, and continuity promises.
   - Longform/serial: use a project workflow with story bible, chapter cards, manuscript files, and tracking ledgers when the user wants an ongoing book.
   - Revision/critique: diagnose structure and reader effect before line edits.
5. Choose one default working frame:
   - Whole story: premise -> central dramatic question -> protagonist want/need -> opposition -> stakes -> ending pressure.
   - Act/chapter outline: turning points, reversals, reveals, emotional escalation, and unresolved hooks.
   - Chapter/scene: POV character goal, obstacle, tactic shift, conflict turn, consequence, next pressure.
   - Revision: diagnose structure first, then line-level prose.

## Longform Workflow
Use this when the user asks for a novel, web novel, chapter-by-chapter story, continuation, or a manuscript project.

1. Establish the reader promise before designing plot: what the reader should keep returning for.
2. Create or update reusable artifacts when working in files: premise/brief, cast, world/canon, outline, chapter cards, manuscript, and continuity tracking.
3. Plan progressively. Lock the whole-story direction and the first batch of chapter cards; avoid pretending every later chapter is fixed if the story will evolve.
4. Before drafting a chapter, load only the context needed to avoid errors: previous chapter, current chapter card, active cast states, active promises/foreshadowing, timeline, and relevant world rules.
5. Draft the chapter around a clear delivery target: opening disturbance, escalating pressure, payoff or reversal, and an earned ending hook.
6. After drafting, update continuity: changed facts, relationship state, timeline, unresolved promises, clues, injuries, abilities, locations, and emotional commitments.
7. When the user asks for a complete autonomous run and the plan is confirmed, keep producing within the agreed scope without repeated confirmation unless canon, ethics, or file safety would be at risk.

## Deliverable Patterns
- For planning: provide a compact beat list with cause-and-effect links, not isolated events.
- For chapter cards: include chapter purpose, POV, conflict, scene sequence, reveal/reversal, emotional target, continuity changes, and ending hook.
- For drafting: separate "Draft" from "Notes" so the user can keep prose without explanations mixed in.
- For critique: lead with the highest-leverage issue, then give specific fix options.
- For continuity: list canonical facts, conflicts, assumptions, open questions, and active promises.
- For style imitation: infer a style profile from the user's supplied text; do not claim to copy an absent style.
- For Chinese prose: write in the user's requested Chinese register when asked, keep punctuation and paragraphing natural for Chinese fiction, and avoid translated English cadence.

## Style Discipline
- For Chinese web-serial prose, enforce mobile-readable paragraphing unless the user supplies a different house style: one beat per paragraph, a blank line between paragraphs in Markdown/chat output, one speaker per dialogue paragraph, and no dense wall-of-text blocks.
- Default Chinese fiction paragraph length is 15-80 Chinese characters. Split action, reaction, new information, dialogue, and emotional turns into separate paragraphs. Treat 120 Chinese characters as the normal hard cap; exceed it only for a deliberate slow descriptive passage that earns the density.
- Respect explicit length targets. If the user asks for a full web-serial chapter without specifying length, use 2,500-5,000 Chinese characters as the working range; if they ask for a short scene, use the requested scale rather than padding. When writing to files, verify substantial chapter length with tooling when practical.
- Default to zero em dashes in drafted prose. Avoid both `--` and Chinese dash forms such as `—` or `——` unless the user explicitly requests that punctuation or a source passage already requires it.
- In Chinese fiction, use comma, period, colon, semicolon, ellipsis, paragraph breaks, short sentences, action beats, and dialogue tags for rhythm. Reserve dashes only for a true interruption, sudden break in speech, or abrupt turn that cannot be cleaner another way.
- If a dash is unavoidable, keep it rare: at most one dash construction per 1,000 Chinese characters in finished prose, and only where it has a clear dramatic function.
- Replace dash-heavy sentences with one of these moves: split the sentence, make the consequence visible, add a physical beat, let a silence carry the turn, use direct dialogue, or put the reversal in the next paragraph.
- Avoid AI-ish prose patterns: denial-first phrasing such as "not X but Y," "unlike X, more like Y," "不是...而是...," or "不像...更像...," decorative abstractions, over-explained emotions, repeated sensory cliches, tidy summary paragraphs, exposition dumps, labeled feelings, generic epiphanies, and repeated "realized/knew/felt a surge" constructions.
- Run a trust-the-reader pass for substantial prose: remove explanation after facts, narrator self-Q&A, speculative inner-state menus, zero-information repetition, unsupported POV knowledge, naming regressions, abrupt unearned emotional turns, mechanical metaphors, and sentences that describe only what did not happen.
- Human prose should feel locally caused: concrete nouns and verbs, imperfect dialogue timing, subtext, specific physical behavior, asymmetry, pressure, consequence, and a little useful ambiguity.
- Before finalizing a substantial draft or polish, run a dash audit and an AI-prose audit. Rewrite any paragraph where punctuation is carrying drama that should come from action, choice, or consequence.

## Quality Bar
- Every scene or chapter needs a change: information, relationship, power, emotion, danger, status, location, ability, or commitment.
- Character choices should reveal motive under pressure.
- Plot beats should be connected by "because/therefore", not only "and then".
- Stakes should become more specific as the story narrows.
- Chapter openings must create disturbance quickly; avoid static weather, exposition dumps, routine wake-up sequences, and throat-clearing unless they are immediately weaponized by conflict or contrast.
- Chapter endings need forward pressure: a question, cost, reveal, deadline, threat, promise, emotional break, or irreversible choice.
- Keep prose concrete: action, sensory anchors, subtext, rhythm, consequence, and sentence-level pressure.
- Revise generic AI-sounding prose by adding specific behavior, asymmetry, silence, imperfect dialogue, concrete nouns/verbs, and scene-level stakes instead of decorative adjectives.
- Finished fiction should not read like a style demonstration. Remove over-neat parallel phrasing, sermon-like explanations, and ornamental punctuation that makes the narrator sound more impressed than the characters are.
- Chinese web-serial drafts should pass a format check before delivery: paragraphs separated by blank lines, no accidental ultra-long paragraphs, no repeated summary paragraphs, and the visible ending hook has forward pressure.

## Resources
- Read `references/story-craft-playbook.md` for structure lenses, genre architecture, character arcs, suspense systems, and revision passes.
- Read `references/longform-production-playbook.md` for novel/web-novel project workflows, staged intake, file artifacts, continuation, resumption, and validation loops.
- Read `references/chapter-drafting-playbook.md` for chapter openings, cliffhangers, scene expansion without padding, dialogue, Chinese prose naturalization, and chapter quality gates.
- Read `references/chinese-webnovel-playbook.md` for Chinese web novel mechanics, reader-payoff design, opening pressure, platform-shaped pacing, genre modules, and Chinese prose risks.
- Read `references/continuity-state-playbook.md` when continuing, importing, revising, or managing a long manuscript with active canon, character state, timeline, promises, clues, powers, inventory, or relationship changes.
- Read `references/quality-gate.md` before finalizing substantial fiction deliverables, especially full chapters, longform outlines, continuations, imported manuscript diagnoses, trust-the-reader line edits, or de-generic prose passes.
- Use `assets/fiction-outline-template.md` when the user wants a reusable novel/story planning template or a project bible starter.
- Use `scripts/check_chinese_fiction_format.py` when a Chinese prose draft is saved to a file and paragraph length or chapter word count needs a mechanical check.

## Resource Routing
- New Chinese web novel, web-serial concept, or chapter plan: use `chinese-webnovel-playbook.md` plus `story-craft-playbook.md`.
- Full novel setup or chapter-by-chapter project: use `longform-production-playbook.md`, then the outline template if files are useful.
- Chapter drafting, expansion, or polish: use `chapter-drafting-playbook.md`, then `quality-gate.md`.
- Continuation, import, or canon-heavy revision: use `continuity-state-playbook.md` before drafting.
- Broad structure diagnosis: use `story-craft-playbook.md` first, then only the narrower reference needed for the problem found.

## Avoid
- Do not overwrite the user's voice with generic "literary" prose.
- Do not invent major canon silently.
- Do not over-explain craft theory when the user asked for usable text.
- Do not present formula names as authority; use them as optional lenses.
- Do not continue a long manuscript from memory when files or prior chapters are available; load the relevant source-of-truth first.
- Do not pad word count with exposition, repeated internal monologue, or decorative description. Add new pressure, choices, reveals, tactics, or relationship movement.
