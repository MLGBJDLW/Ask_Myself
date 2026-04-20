# Product Roadmap

## Active Direction

Nexa is moving toward a local-first desktop assistant for everyday work.

The product should combine:

- evidence-first recall
- investigation over personal files
- collections as reusable working sets
- document and office assistance
- consumer-grade usability for non-programmers

## Current Priorities

### P0

- Keep source scope, evidence strength, and route clarity visible in Chat
- Make Search, Collections, and Chat feel like one continuous workflow
- Maintain full i18n coverage for all shipped locales
- Preserve streaming and completed-state consistency

### P1

- Recall Mode for vague memory lookup
- Consumer-friendly language across helper/status surfaces
- Better collection-as-workspace behavior
- Stronger office assistance flows

### P2

- More guided desktop-assistant workflows
- Better document output templates and review patterns
- Richer consumer onboarding and task suggestions

## Near-term Build Sequence

1. Investigation workspace in Chat
2. Recall Mode in Search
3. Collections as sustained working sets
4. Consumerize advanced agent labels and helper flows
5. Office/document workflows that feel guided and safe

## Shipped Foundations

Already in progress or landed:

- investigation header in Chat with clearer scope / route / evidence visibility
- Search to Chat source-scope handoff
- collection-context handoff into Chat
- Recall Mode entry in Search for vague-memory lookup
- first-pass collection workspace actions for investigation, briefs, reports, and slide outlines
- stronger product, UX, roadmap, and i18n documentation in `docs/`

## Guardrails

- Do not sacrifice trust for agent spectacle.
- Do not add technical UI just because it is possible.
- Do not leave new user-facing strings outside i18n.
- Do not ship workflows that look clever but feel confusing.
