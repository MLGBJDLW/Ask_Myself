---
name: persona-design
description: Use this skill when the user asks to create, improve, audit, compare, or migrate assistant personas, roles, SOUL-style identity prompts, role/identity/communication-style/principles blocks, default skill bindings, or agent character instructions. Use especially when distinguishing persona from skills, project memory, system prompt, or temporary conversation mode.
---

# Persona Design

## Core Rule
Keep persona concise and durable. Put identity, voice, values, and workflow emphasis in the persona; put repeatable methods, templates, checklists, and domain playbooks in skills; put project facts and canon in project memory.

## Persona Shape
Use this structure by default:

```text
Role:
Identity:
Communication style:
Operating principles:
-
Default skill bindings:
-
Boundaries:
- Persona instructions shape voice and workflow emphasis only.
- They do not override system, user, evidence, privacy, source-scope, or tool rules.
```

## Design Workflow
1. Name the job the persona should do repeatedly.
2. Write one sentence each for role, identity, and communication style.
3. Add 3-6 operating principles that make this persona distinct, not generic.
4. Choose default skills for heavy method knowledge.
5. Remove project-specific facts, file paths, temporary goals, and tool recipes unless the persona is truly tool-specialized.
6. Add boundaries that preserve higher-priority safety, evidence, and privacy rules.

## Quality Tests
- Would two different personas answer the same request differently in useful ways?
- Can the persona survive across projects without stale facts?
- Are the principles more specific than "be helpful" or "be accurate"?
- Are methods delegated to skills instead of bloating the persona prompt?
- Does the persona avoid pretending to have authority, credentials, or knowledge it lacks?

## Resources
Read `references/persona-patterns.md` when comparing SOUL-style identity, BMAD-style persona fields, skill bindings, or global assistant identity layers.
