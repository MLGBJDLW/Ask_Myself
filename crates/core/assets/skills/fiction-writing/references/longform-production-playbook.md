# Longform Production Playbook

Use this reference when the user wants a full novel, web novel, serial project,
chapter-by-chapter continuation, imported manuscript, or a reusable writing
workspace. The goal is to keep the story writable over time without losing
voice, continuity, or reader momentum.

## Contents

- Operating Principle
- Phase Workflow
- Resuming Work
- Importing Existing Fiction
- Parallel Or Batched Writing
- Web Serial And Chinese-Language Considerations

## Operating Principle

Do not keep a long book only in conversation memory. Use a small set of durable
artifacts, load only the current context needed, and update state after each
drafting unit.

## Phase Workflow

### 0. Detect Project State

Before asking new questions, check whether the user supplied an existing
manuscript, project folder, outline, prior chapters, or reference materials.

- Existing manuscript: import facts first, then diagnose or continue.
- Existing outline but no prose: create chapter cards before drafting.
- Existing prose and tracking: resume from the next missing chapter or the
  chapter the user names.
- No project: perform staged intake.

### 1. Staged Intake

Ask only enough to move forward. Prefer defaults when the user wants speed.

Core intake:

- Story seed or genre.
- Protagonist, central relationship, or ensemble focus.
- Main pressure: danger, desire, mystery, revenge, ascent, love, survival, repair.
- Emotional promise: what the reader should feel repeatedly.

Optional intake:

- Setting/world rules.
- POV, tense, tone, and language.
- Target reader/platform or shelf.
- Desired length, chapter count, chapter word range, and update cadence.
- Reference works, tropes to include, tropes to avoid.
- Ending preference: happy, tragic, bittersweet, open, twist, series hook.

If the user asks for random generation, generate 3-5 viable directions and pick
the strongest unless they want to choose.

### 2. Planning Artifacts

For file-based projects, create or update this minimal structure unless the repo
already has a better convention:

```text
story/
  brief.md
  bible/
    world.md
    cast.md
    relationships.md
  outline/
    arc-map.md
    chapter-cards.md
  manuscript/
    ch001-title.md
  tracking/
    continuity.md
    timeline.md
    promises.md
    character-state.md
```

Keep the structure proportional. A short story may need only a brief, beat list,
and draft. A long serial benefits from one file per major character, faction,
world rule, volume, and chapter.

### 3. Arc And Chapter Planning

Lock the large direction, not every detail.

- Whole-story arc: premise, dramatic question, ending pressure, major reversals.
- Volume/act arc: emotional purpose, conflict escalation, payoff schedule.
- First batch chapter cards: usually 5-10 chapters for an ongoing serial.
- Rolling outline: after several chapters, update later cards based on earned
  consequences rather than forcing stale plans.

Chapter card fields:

- Chapter number and working title.
- POV and location/time.
- Chapter purpose.
- Opening disturbance.
- Scene sequence with causal turns.
- Emotional target.
- Reveal, reversal, or payoff.
- Character state changes.
- Continuity changes: clues, injuries, powers, resources, location, reputation.
- Ending hook.
- Word target or length band if requested.

### 4. Draft Loop

For each chapter:

1. Load a context pack:
   - Previous chapter or last relevant scene.
   - Current chapter card.
   - Active character states.
   - Active promises/foreshadowing.
   - Relevant timeline and world rules.
   - Reference material only if it affects this chapter.
2. State the chapter intent in one sentence.
3. Draft around the intent:
   - Disturbance early.
   - Escalating attempts and obstacles.
   - One clear payoff, reversal, or state change.
   - Ending pressure that makes the next chapter feel necessary.
4. Run the chapter gate.
5. Update tracking files immediately if working on disk.

### 5. Validation And Repair

Check completed chapters before moving on:

- Length: if the user gave a target, verify with tooling where possible.
- Continuity: names, timeline, rules, injuries, inventory, locations, powers.
- Promises: new, advanced, paid off, delayed, or dropped.
- Reader promise: whether the target emotion landed.
- Prose: remove generic summaries, repeated phrasing, and translated cadence.

Repair hierarchy:

1. Missing plot movement: add a decision, obstacle, reveal, or consequence.
2. Weak emotion: add a sharper pressure point or reaction with behavioral detail.
3. Thin word count: add causal beats, tactical shifts, dialogue pressure, or
   subplot movement rather than explanation.
4. Continuity conflict: fix the newest text unless the older canon is clearly
   marked for retcon.

## Resuming Work

When asked to continue:

1. Find the last complete chapter and the next planned chapter.
2. Read the latest tracking summary, character state, promises, and timeline.
3. Check whether the next chapter card exists. If not, create it from the arc map
   and recent consequences before drafting.
4. Continue in the established POV, tense, naming, paragraph style, and voice.
5. Update the same tracking artifacts after the new chapter.

## Importing Existing Fiction

When the user provides an existing manuscript:

1. Identify chapter boundaries, titles, POVs, and timeline anchors.
2. Extract canon facts without judging them yet.
3. Build a compact story bible: cast, world rules, unresolved promises, major arcs.
4. Diagnose structural issues only after the source-of-truth exists.
5. For continuation, preserve current voice and unresolved setup unless the user
   explicitly asks for a rewrite.

## Parallel Or Batched Writing

Use parallel drafting only when the outline and context package are stable.
Separate workers or passes should receive the same story bible, chapter cards,
voice profile, and continuity constraints. Reconcile afterward for voice,
timeline, repeated reveals, duplicated hooks, and contradictory character state.

## Web Serial And Chinese-Language Considerations

- Name the reader payoff clearly before planning chapters.
- Keep the first chapter externally active; avoid long solo introspection before
  the story has earned attention.
- Build a repeatable progress ladder: status, power, clue, romance intimacy,
  revenge list, money, reputation, territory, rank, or survival margin.
- Use chapter endings to open forward pressure, but pay off smaller promises often
  enough that the reader trusts the story.
- In Chinese prose, prefer natural spoken rhythm, concise paragraphs, concrete
  behavior, and culturally fitting punctuation over translated English syntax.
- Remove "AI polish" by making reactions asymmetric, imperfect, and grounded in
  scene-specific objects or actions.
