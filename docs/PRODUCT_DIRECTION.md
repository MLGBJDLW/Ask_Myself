# Product Direction

## Summary

Nexa has formally moved from being primarily a local-first personal knowledge recall engine to being a broader local-first desktop assistant with knowledge recall as a core capability.

The assistant should still excel at evidence-first recall over the user's own files, but it should also become genuinely useful for everyday desktop work:

- finding and explaining information
- creating and updating office-style documents
- helping with planning, drafting, summarizing, and comparison
- staying grounded in the user's local context instead of behaving like a generic chatbot

## Product Positioning

Nexa should feel like:

- a trustworthy desktop assistant
- a local knowledge investigator
- a practical office helper for normal users, not only technical users

Nexa should not feel like:

- a developer-only coding console
- a generic web chatbot with local files bolted on
- a raw model-debug surface that exposes internal agent mechanics as the main experience

## Primary Users

The product should work for:

- office workers
- students and researchers
- operations and project coordinators
- founders and general knowledge workers
- personal knowledge users who are not programmers

This means the default UX must optimize for clarity, confidence, and usefulness over technical power-user aesthetics.

## Core Pillars

### 1. Local-first trust

- Sources, indexing, search, collections, and conversation persistence stay local by default.
- The UI should make scope and data boundaries obvious.
- Users should understand what information is being used and where it came from.

### 2. Evidence-first answers

- Factual answers should be grounded in evidence from the user's data.
- Citations, evidence strength, and scope boundaries should be visible.
- If evidence is weak or absent, the product should say so clearly.

### 3. Useful desktop assistance

- The product should help create documents, summarize content, compare files, draft materials, and assist common office workflows.
- File and document operations should feel safe, understandable, and reversible where possible.
- The assistant should help with ordinary work, not just retrieval.

### 4. Consumer-grade usability

- Low jargon by default
- Clear status, clear next step, clear scope
- No confusing split between what is “live” and what is “final”
- Strong defaults that work without technical setup knowledge

### 5. Reusable working sets

- Collections are not just bookmarks.
- They should evolve into reusable investigation packs and working contexts.
- Search -> collect -> ask should feel like one workflow.

## Product Principles

- Chat is an entry point, not the entire product.
- Investigation is more important than spectacle.
- Evidence is more important than chain-of-thought theater.
- Source scope must always be understandable.
- Consumer comprehension beats internal cleverness.
- File/document help should be practical, not abstract.

## Non-goals

These are explicitly lower priority than the pillars above:

- maximizing raw agent autonomy at the cost of clarity
- developer-centric terminal-first interaction models
- exposing large raw thinking traces by default
- copying IDE agent products too closely
- adding advanced controls before the default workflow is excellent

## Priority Themes

### Near-term

- make Chat feel like an investigation workspace
- strengthen scope/evidence visibility
- improve document and office-assistance workflows
- improve collection continuity across pages
- keep all UI strings fully internationalized

### Mid-term

- dedicated recall mode for vague-memory lookup
- stronger collection-as-workspace behavior
- consumer-friendly workflow templates
- guided office tasks and output helpers

### Long-term

- richer desktop-assistant actions
- structured task flows for common office work
- safer mutation/review patterns for document generation and editing

## Shipping Heuristic

A new feature is aligned only if it improves at least one of these without meaningfully harming the others:

- trust
- evidence quality
- consumer usability
- desktop usefulness
- local-first clarity
