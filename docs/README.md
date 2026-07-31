# Documentation Index

This folder captures the durable product and engineering memory for Nexa, which is now positioned as a local-first desktop assistant with a personal knowledge base at its core.

## Product

- [PRODUCT_DIRECTION.md](./PRODUCT_DIRECTION.md)
  Defines the assistant-first product positioning, target audience, core pillars, and non-goals.
- [ROADMAP.md](./ROADMAP.md)
  Captures the active product priorities and sequencing so the team can keep shipping in the intended direction.
- [UX_QUALITY_BAR.md](./UX_QUALITY_BAR.md)
  Defines the front-end quality bar for a consumer-facing desktop assistant.
- [I18N_GUIDELINES.md](./I18N_GUIDELINES.md)
  Defines translation discipline, key naming, and rollout expectations for all shipped locales.

## Ecosystem and Workflows

- [ECOSYSTEM_ARCHITECTURE.md](./ECOSYSTEM_ARCHITECTURE.md)
  Defines the Nexa extension ecosystem: capability packages, connectors, skills,
  workflows, adapters, host surfaces, and future native plugins.
- [CAPABILITY_PACKAGES.md](./CAPABILITY_PACKAGES.md)
  Defines the `.nexa/capabilities/*/capability.yaml` package layout, supported
  surfaces, validation rules, and built-in bridge.
- [MCP_CONNECTORS.md](./MCP_CONNECTORS.md)
  Defines MCP connectors as the first external ecosystem lane and documents
  their lifecycle, trust model, and product language.
- [SKILL_PACKAGES.md](./SKILL_PACKAGES.md)
  Defines portable skill packages, bundled resources, dependency metadata, and
  import safety expectations.
- [WORKFLOW_PACKAGES.md](./WORKFLOW_PACKAGES.md)
  Defines user-facing workflow packages and how they compose tools, skills, and
  connectors.
- [PROTOCOL_EXITS.md](./PROTOCOL_EXITS.md)
  Defines how Nexa should expose selected local capabilities to other agents,
  starting with scoped MCP server mode.
- [NATIVE_PLUGIN_RUNTIME.md](./NATIVE_PLUGIN_RUNTIME.md)
  Defines when native plugin runtime is allowed and the safety rules it must
  satisfy.
- [TOOLS.md](./TOOLS.md)
  Reference for built-in agent tools and their intended routing.

## Runtime and Architecture

- [architecture/terminal-agent-bridge.md](./architecture/terminal-agent-bridge.md)
  Documents terminal copy/paste behavior, selection-to-prompt handoff,
  conversation binding, agent permissions, output limits, and stop semantics.
- [architecture/live-file-tool-streaming.md](./architecture/live-file-tool-streaming.md)
  Documents live file-tool projection and the frontend/backend event contract.
- [architecture/orchestration-runtime.md](./architecture/orchestration-runtime.md)
  Defines the MoA, Nexus Workflow IR, quality-profile, checkpoint, verification,
  privacy, cost, and evaluation contracts.
- [computer-use-integration.md](./computer-use-integration.md)
  Describes the desktop automation boundary and supported interaction paths.
- [security-and-architecture-audit-2026-07.md](./security-and-architecture-audit-2026-07.md)
  Captures the July 2026 security and architecture audit and its follow-up
  constraints.
