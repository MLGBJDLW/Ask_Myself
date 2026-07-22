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

- Keep source scope, evidence strength, route, model, and reasoning state clear in Chat
- Preserve streaming, persisted replay, archive, restore, and delete consistency
- Make live desktop surfaces such as the terminal useful to the agent without weakening user control or approval boundaries
- Maintain full i18n coverage and critical browser regression coverage for all shipped UI changes

### P1

- Better collection-as-workspace behavior and cross-surface handoff
- Consumer-friendly language across helper, approval, trace, and status surfaces
- Stronger office assistance flows with verifiable output
- Reduce provider drift by keeping the shared catalog and adapter contracts synchronized

### P2

- More guided desktop-assistant workflows
- Better document output templates and review patterns
- Richer consumer onboarding and task suggestions

## Near-term Build Sequence

1. Harden conversation lifecycle and replay across active and archived states
2. Deepen Search, Collections, and Chat as one sustained working set
3. Expand user-controlled desktop context bridges beyond the terminal where justified
4. Consumerize advanced agent labels, approvals, and recovery flows
5. Ship office/document workflows with validation and review built in

## Shipped Foundations

Landed foundations include:

- investigation header in Chat with clearer scope / route / evidence visibility
- Search to Chat source-scope handoff
- collection-context handoff into Chat
- Recall Mode entry in Search for vague-memory lookup
- first-pass collection workspace actions for investigation, briefs, reports, and slide outlines
- persisted turn traces, checkpoints, context accounting, and per-turn navigation
- visible archived-conversation browsing with read-only replay and restore/delete lifecycle
- a split model and reasoning selector backed by the shared provider catalog
- a conversation-linked terminal with selection-to-prompt and approval-gated agent interaction
- critical Playwright coverage for the custom window frame, conversation lifecycle, terminal bridge, Mermaid rendering, plan capsule, and turn timeline
- stronger product, UX, roadmap, and i18n documentation in `docs/`

## Guardrails

- Do not sacrifice trust for agent spectacle.
- Do not add technical UI just because it is possible.
- Do not leave new user-facing strings outside i18n.
- Do not ship workflows that look clever but feel confusing.
