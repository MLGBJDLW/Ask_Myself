# Continuity State Playbook

Use this reference when continuing a manuscript, importing an existing story,
revising long fiction, or managing a serial with active canon. The purpose is to
avoid accidental contradictions while keeping only the necessary context loaded.

## Contents

- Source Of Truth
- Minimal Context Pack
- State Ledgers
- Import Workflow
- Continuation Workflow
- Revision Workflow
- Conflict Handling
- Snapshot Cadence
- Templates

## Source Of Truth

Prefer explicit project files over chat memory. When sources conflict, resolve in
this order unless the user says otherwise:

1. User's newest instruction.
2. Manuscript text already accepted as canon.
3. Story bible or tracking ledgers.
4. Outline or chapter cards.
5. Assistant assumptions.

Label assumptions when they affect canon. Never silently invent major facts such
as parentage, powers, culprit identity, timeline jumps, relationship status,
world rules, or ending direction.

## Minimal Context Pack

Before drafting or diagnosing a chapter, load:

- The previous chapter or the last relevant scene.
- The current chapter card or user request.
- Active character state for involved characters.
- Active promises, clues, secrets, and unresolved hooks.
- Timeline constraints and location.
- World/power rules directly used in the chapter.
- Voice sample if matching style matters.

Skip unrelated lore. A bloated context pack causes more contradictions, not fewer.

## State Ledgers

Use ledgers when working on disk. Keep them compact and current.

### Character State

Track only state that can affect future scenes:

- Location and availability.
- Physical condition: injuries, illness, fatigue, pregnancy, scars, disability.
- Emotional state: grief, attraction, distrust, fear, guilt, resolve.
- Public status: rank, reputation, accusation, disguise, legal/social position.
- Resources: money, weapon, artifact, evidence, allies, access, information.
- Ability/power state: rank, cooldown, cost, limitation, learned technique.
- Relationship changes: debt, trust, betrayal, intimacy, leverage.
- Secrets known, hidden, suspected, or misunderstood.

### Promises And Foreshadowing

Track every promise that creates reader expectation:

- Mystery question.
- Clue.
- Prophecy, omen, image, repeated phrase.
- Threat, vow, warning, deadline.
- Relationship promise.
- Power-system limitation.
- Object or document introduced as meaningful.

Statuses:

- Open: introduced but not advanced.
- Advanced: new information added.
- Paid: resolved on page.
- Delayed: intentionally postponed with new pressure.
- Retconned: changed with user approval.
- Dropped risk: likely forgotten; needs repair or payoff.

### Timeline

Track story time, not just chapter order:

- Date/time or relative sequence.
- Travel time.
- Injuries and recovery.
- School/work/legal deadlines.
- Ages, anniversaries, seasons, festivals, pregnancies, training durations.
- Simultaneous events in other POVs.

### World Rules

Track rules that constrain scenes:

- Magic/power/technology limits.
- Costs and cooldowns.
- Social hierarchy, law, taboo, institutions.
- Geography and travel.
- Naming conventions and address forms.
- Economy, resources, and material constraints.

## Import Workflow

For an existing manuscript:

1. Detect chapter boundaries and POV pattern.
2. Extract cast with aliases and relationships.
3. Extract world rules and timeline anchors.
4. List open promises and unanswered questions.
5. Build a chapter summary table.
6. Only then diagnose, continue, or rewrite.

Do not overwrite the user's voice during import. The first task is canonical
mapping, not improvement.

## Continuation Workflow

When the user says continue:

1. Identify the last accepted manuscript unit.
2. Read the last ending and current unresolved pressures.
3. Build a context pack from ledgers.
4. Draft the next unit in established POV, tense, naming, paragraph rhythm, and
   emotional temperature.
5. Update state ledgers immediately after drafting.

If no next chapter card exists, create a provisional card from the arc map and
recent consequences. Mark it as provisional if user confirmation may be needed.

## Revision Workflow

When revising older chapters:

1. Determine whether the change is local or canon-altering.
2. If canon-altering, list downstream files/chapters affected before rewriting.
3. Preserve accepted later payoffs where possible.
4. After rewriting, update ledgers and flag any remaining mismatch.

Local changes:

- Prose polish.
- Dialogue sharpening.
- Scene order within the same event.
- Detail correction that does not affect later events.

Canon-altering changes:

- Character death, injury, power, identity, motivation, relationship, clue, or
  timeline.
- World rule or institution change.
- Culprit, antagonist, ending, or major reveal.
- Removing a promise that later chapters rely on.

## Conflict Handling

When a contradiction appears:

1. Quote or summarize both conflicting facts with locations.
2. Classify severity:
   - S1: blocks current drafting or breaks core plot.
   - S2: visible reader-facing contradiction.
   - S3: minor timeline/state mismatch.
   - S4: style or terminology drift.
3. Propose the smallest repair:
   - Adjust newest text.
   - Add bridging explanation.
   - Update tracking ledger.
   - Ask user to choose if it changes canon.

## Snapshot Cadence

For long runs:

- After each chapter: update character state, promises, and timeline.
- After every 3-5 chapters: write a compact context summary.
- At volume/act boundary: archive resolved promises, summarize active ones, and
  refresh cast/world state.
- Before context compaction: preserve current chapter target, last accepted
  ending, open promises, and any user-specific style instruction.

## Templates

### Character State Row

```text
Character:
Location:
Physical state:
Emotional state:
Public status:
Resources / powers:
Secrets known:
Relationship changes:
Last changed in:
Next pressure:
```

### Promise Row

```text
Promise:
Type:
Opened in:
Current status:
Evidence planted:
Next advancement:
Planned payoff:
Risk if forgotten:
```

### Chapter Context Pack

```text
Chapter:
Intent:
Previous ending:
POV:
Active characters:
Must preserve:
Must advance:
Promises to pay or deepen:
World rules in play:
Ending pressure:
```
