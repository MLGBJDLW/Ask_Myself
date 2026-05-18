# Persona Patterns

## Layers
- Core system prompt: non-negotiable app behavior, safety, evidence, tools, privacy.
- Soul/global identity: durable assistant identity and working style.
- Persona: selected role, voice, operating principles, workflow emphasis.
- Skills: reusable procedures, frameworks, templates, checklists, scripts.
- Project memory: local facts, canon, decisions, naming, conventions.
- Conversation mode: temporary emphasis for one task or session.

## BMAD-Style Fields
- Role: professional function and scope.
- Identity: how the agent understands its purpose and relationship to the user.
- Communication style: tone, level of directness, format preference.
- Principles: 3-6 distinctive operating beliefs that change behavior.

## SOUL-Style Identity
Use for global assistant identity, not per-task methodology:
- Who the assistant is.
- How it speaks.
- What it avoids.
- How it handles uncertainty, disagreement, and ambiguity.

## Anti-Patterns
- Giant persona that contains every method and genre checklist.
- Persona that repeats core system rules.
- Persona that includes project facts likely to become stale.
- Persona that claims credentials, legal/medical authority, or direct experience.
- Persona that conflicts with evidence-first or local-first behavior.

## Skill Binding Heuristic
Bind a default skill when:
- The persona repeatedly uses a specialized workflow.
- The method is too detailed for persona instructions.
- The skill can be reused by other personas or direct user requests.

Do not bind a skill when it only adds tone or identity.
