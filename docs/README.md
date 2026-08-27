# Nexa Documentation

This directory contains durable product and engineering contracts for Nexa. The
canonical architecture entry point is [ARCHITECTURE.md](./ARCHITECTURE.md).

## Product

- [PRODUCT_DIRECTION.md](./PRODUCT_DIRECTION.md) defines the assistant-first
  product position, audience, pillars, and non-goals.
- [ROADMAP.md](./ROADMAP.md) records active product priorities without treating
  temporary implementation research as architecture.
- [UX_QUALITY_BAR.md](./UX_QUALITY_BAR.md) defines the desktop experience bar.
- [I18N_GUIDELINES.md](./I18N_GUIDELINES.md) defines translation and locale
  maintenance rules.

## Architecture and runtime

- [ARCHITECTURE.md](./ARCHITECTURE.md) is the single normative architecture
  index and records the system boundaries and invariants shared by all modules.
- [TERMINAL_AGENT_BRIDGE.md](./TERMINAL_AGENT_BRIDGE.md) defines terminal
  ownership, conversation binding, permissions, and stop semantics.
- [LIVE_FILE_TOOL_STREAMING.md](./LIVE_FILE_TOOL_STREAMING.md) defines partial
  argument previews, authoritative file-tool execution, and resumable writes.
- [ORCHESTRATION_RUNTIME.md](./ORCHESTRATION_RUNTIME.md) defines MoA, Nexus,
  workflow checkpoints, verification, privacy, cost, and evaluation contracts.
- [SCHEDULED_TASKS.md](./SCHEDULED_TASKS.md) defines recurrence, occurrence
  claiming, execution policy, permissions, migration, and operator guidance for
  unattended workflows.
- [computer-use-integration.md](./computer-use-integration.md) defines the
  desktop automation boundary.
- [computer-use-landscape-2026-08-23.md](./computer-use-landscape-2026-08-23.md)
  compares 28 primary-source computer-use systems and records the upgrade
  rationale, licenses, risks, evaluation ladder, and verified source links.
- [agent-liveness-and-tool-progress-2026-08-23.md](./agent-liveness-and-tool-progress-2026-08-23.md)
  compares 22 agent/runtime families and records the long-reasoning,
  browser/computer handoff, subagent, cancellation, and deadlock findings.
- [security-and-architecture-audit-2026-07.md](./security-and-architecture-audit-2026-07.md)
  records the retained constraints from the July 2026 audit.

## Ecosystem and tools

- [ECOSYSTEM_ARCHITECTURE.md](./ECOSYSTEM_ARCHITECTURE.md) defines the extension
  ecosystem and its trust boundaries.
- [CAPABILITY_PACKAGES.md](./CAPABILITY_PACKAGES.md),
  [MCP_CONNECTORS.md](./MCP_CONNECTORS.md),
  [SKILL_PACKAGES.md](./SKILL_PACKAGES.md), and
  [WORKFLOW_PACKAGES.md](./WORKFLOW_PACKAGES.md) define the supported package
  surfaces.
- [PROTOCOL_EXITS.md](./PROTOCOL_EXITS.md) and
  [NATIVE_PLUGIN_RUNTIME.md](./NATIVE_PLUGIN_RUNTIME.md) define higher-risk
  integration exits.
- [TOOLS.md](./TOOLS.md) is the built-in tool reference.

## Documentation lifecycle

Stable contracts belong directly under `docs/` and must be linked from this
index or from `ARCHITECTURE.md`. Dated investigations, source dumps,
implementation research, and temporary roadmaps are not normative architecture.
Keep that material in the relevant Issue or PR discussion, or in the ignored
`docs/local/` and `docs/research/` work areas. The repository ignores the former
`docs/architecture/` tree and research-style Markdown filenames so temporary
evidence cannot silently become a permanent documentation surface.
