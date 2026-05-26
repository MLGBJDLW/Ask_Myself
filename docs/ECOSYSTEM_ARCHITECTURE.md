# Nexa Ecosystem Architecture

This document defines how Nexa exposes extension points without turning every
extension point into a plugin. It is the durable reference for capability
packages, connectors, skills, workflows, adapters, host surfaces, and future
native plugins.

## Product Constraint

Nexa is a local-first desktop assistant for everyday knowledge and office work.
The ecosystem must preserve:

- local-first trust
- evidence-first answers
- consumer-grade usability
- practical desktop assistance
- reusable working sets

The default product experience should not require users to understand agent
internals. Advanced extension surfaces must sit behind clear trust and
installation boundaries.

## Core Rule

Do not use `plugin` as the default word for every extension.

Use the smallest ecosystem surface that provides the needed leverage:

1. Use a `Skill Package` for instructions, references, examples, and bundled
   helper scripts that are invoked by existing tools.
2. Use a `Connector` for external tools, services, and data sources. MCP is the
   first supported connector interface.
3. Use a `Workflow Package` for product-facing task templates that compose
   existing tools, skills, and connectors.
4. Use an `Adapter` for replaceable backends behind a stable host interface,
   such as model, image, search, or document runtimes.
5. Use a `Capability Package` for a coherent built-in or installable Nexa
   ability that owns tools, settings surfaces, workflows, checks, and tests.
6. Use a `Native Plugin` only when the extension needs code, hooks, or UI that
   cannot be represented by the safer surfaces above.

## Ecosystem Surfaces

| Surface | Purpose | External by default | Native code | Examples |
| --- | --- | --- | --- | --- |
| Core Platform | Required host runtime and trust model | No | Owned by Nexa | indexing, search, source scope, approvals, task runs, MCP client |
| Capability Package | Coherent Nexa ability with tools/settings/workflows | Built-in first, external later | Only after package host exists | Office Documents, File Workspace, Delegation |
| Connector | External service or process access | Yes | Out of process | MCP connectors, browser automation connector, SaaS tools |
| Skill Package | Portable working instructions and resources | Yes | No direct host code | research synthesis, document editing, persona design |
| Workflow Package | User-facing task template | Yes, after catalog format stabilizes | No | meeting summary, document compare, report brief |
| Adapter | Replaceable backend implementation | Usually internal first | Maybe, behind host interface | LLM provider, web search provider, image provider |
| Host Surface | Product shell | No | Host-owned | Desktop, CLI, IDE extension, browser extension |
| Native Plugin | Last-resort isolated extension | Later | Yes, isolated | custom tool implementation, UI panel, hook bundle |

## Non-Plugin Surfaces

The following are not native plugins:

- MCP endpoints and processes: expose them to users as MCP connectors.
- Skills: they are skill packages.
- Workflow templates: they are workflow packages.
- Provider choices: they are adapters behind a capability package.
- Desktop, CLI, IDE, or browser products: they are host surfaces.
- Knowledge indexing, source scope, approval policy, task runs, and local
  persistence: they are core platform responsibilities.

## Built-In Package Classification

The current built-in manifest API is still named `PluginManifest` for
compatibility, but the manifests describe ecosystem surfaces. New work should
treat the name as legacy compatibility and avoid expanding "plugin" language in
user-facing docs.

| Manifest id | Surface | Rationale |
| --- | --- | --- |
| `core-agent` | Core Platform | Orchestration, planning, and verification are host runtime responsibilities. |
| `knowledge-base` | Core Platform | Local evidence retrieval is a product pillar, not an optional plugin. |
| `office-documents` | Capability Package | Coherent document ability with tools, runtime checks, settings, and workflows. |
| `image-generation` | Adapter | Provider-backed generation behind a stable Nexa interface. |
| `web-research` | Adapter | Search/fetch providers cross the network through controlled backend adapters. |
| `file-workspace` | Capability Package | Source-scoped file inspection and edits are a coherent built-in ability. |
| `desktop-automation` | Host Surface | Desktop and shell actions depend on host permissions and approvals. |
| `agent-memory` | Capability Package | Playbooks, skills, personas, scratchpads, and feedback form a reusable context ability. |
| `agent-evaluation` | Core Platform | Quality previews protect the agent runtime itself. |
| `delegation` | Capability Package | Subagent work composes bounded agent tools and policies. |
| `mcp-connectors` | Connector | MCP exposes external server-defined tools behind approval policy. |

## Seven-Step Migration Path

### 1. Establish language and boundaries

Definition of done:

- `Capability Package`, `Connector`, `Skill Package`, `Workflow Package`,
  `Adapter`, `Host Surface`, and `Native Plugin` are defined in docs.
- Built-in manifests expose an ecosystem surface classification.
- Existing docs stop treating plugin as the umbrella term.

### 2. Manifest-driven built-in packages

Definition of done:

- Built-in package metadata has one source of truth.
- Tool-to-package mapping, settings surfaces, workflows, runtime checks, and
  provider catalogs are generated or validated from package metadata.
- Each built-in package has tests that verify every registered tool belongs to
  exactly one package surface.

### 3. MCP connectors as the first external lane

Definition of done:

- MCP connectors are documented separately from native plugins.
- Connector install/config/test/enable/disable lifecycle is explicit.
- Connector permissions are keyed by server and tool identity.
- The settings UI uses connector language for MCP.

### 4. Skill packages as the second external lane

Definition of done:

- Skill import/export documents the bundle structure, resource policy, and
  dependency model.
- Skill scanning blocks unsafe patterns before installation or proposal apply.
- Skills declare tool and connector dependencies without owning those tools.

### 5. Workflow packages for consumer-facing tasks

Definition of done:

- Workflow templates are documented as user-facing task packages.
- Workflows compose skills, tools, connectors, and approval requirements.
- The workflow catalog avoids internal agent jargon in user-facing text.

### 6. Protocol exits before native plugins

Definition of done:

- Nexa can expose selected local capabilities to other agents through a
  protocol surface, starting with MCP server mode.
- Protocol exports have explicit source-scope and approval policy.
- ACP or A2A is considered only after MCP server mode has stable tests.

### 7. Native plugin runtime last

Definition of done:

- Native plugins are process-isolated or otherwise constrained by a stable host
  interface.
- Native plugins cannot patch core files or register hidden high-risk behavior.
- Plugin packages declare permissions, hooks, settings, UI targets, tests, and
  compatibility versions.
- If a plugin needs a capability the host does not expose, the host interface is
  expanded generically rather than hard-coding that plugin into core.

## Capability Manifest Direction

The long-term package layout is:

```text
.nexa/capabilities/<package-id>/
  capability.yaml
  tools/
  skills/
  workflows/
  commands/
  hooks/
  tests/
```

The manifest should describe the package before implementation details:

```yaml
id: office-documents
name: Office Documents
surface: capability_package
description: Works with PPT, DOCX, XLSX, PDF, and HTML document flows.
version: 1
tools:
  - prepare_document_tools
  - get_document_info
  - compare_documents
settingsSurfaces:
  - office-runtime
workflows:
  - generate-presentation
runtimeChecks:
  - office-runtime
```

The package host should reject unknown high-risk permissions by default. The
manifest is a declaration; the runtime is still responsible for enforcing source
scope, approvals, network boundaries, and execution policy.

Project-local manifest discovery now reads:

```text
.nexa/capabilities/*/capability.yaml
```

Discovery parses YAML, validates the declared ecosystem surface and permissions,
and rejects native code unless the surface is `native_plugin`.

## External Contribution Guidance

For external contributors:

- Build an MCP connector when you need new external tools or service access.
- Build a skill package when you need reusable agent behavior over existing
  tools.
- Build a workflow package when you need a guided end-user task.
- Request an adapter interface when you need a new model, image, search, or
  document backend.
- Request native plugin support only when the above surfaces are insufficient.

This keeps the external path approachable while preserving the trust model that
the desktop assistant depends on.
