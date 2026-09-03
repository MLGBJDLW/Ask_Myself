# Skill Packages

Skill packages are Nexa's lightweight external contribution lane. A skill
package teaches the agent a reusable method, domain procedure, style, or tool
usage pattern without adding new host code.

Skills are not native plugins. They may include instructions, references,
metadata, scripts, and assets, but they must run through existing Nexa tools and
approval policy.

## Package Shape

The portable shape is:

```text
<canonical-name>/
  SKILL.md
  agents/
    openai.yaml
  references/
  scripts/
  assets/
```

`SKILL.md` is the entry point. It should include YAML frontmatter:

```yaml
---
name: research-synthesis
description: Use when synthesizing evidence from multiple local or web sources.
---
```

Installed personal skills use `~/.nexa/skills/<canonical-name>/`. The canonical
name is the portable invocation identity: 1-64 lowercase ASCII letters, digits,
or single hyphens, with no leading, trailing, or consecutive hyphen. It must
match both the parent folder and `SKILL.md` frontmatter. Nexa keeps its immutable
database UUID and localized/human display name separately, so neither appears
as the portable folder identity and changing display text does not move files.

Startup and the explicit Reload skill files action synchronize valid edits for
already-registered canonical names. Legacy UUID folders are recognized only for
a guarded, same-filesystem rename; the database id remains unchanged and all
unknown files move with the package. An existing target, symlink/reparse point,
or invalid canonical name fails closed and leaves the source untouched. Unknown
folders remain unregistered until the user chooses the normal import flow,
including security-warning acknowledgement. A disabled skill keeps its files;
only an explicit Delete removes its folder. Nexa preserves unmodeled files such
as `README.md`, `LICENSE`, and repository metadata. When an approved update
removes or renames a previously modeled resource, Nexa deletes only that exact
former resource path.

The optional `agents/openai.yaml` file describes agent-facing metadata and
dependencies:

```yaml
interface:
  displayName: Research Synthesis
  shortDescription: Turns retrieved evidence into a grounded synthesis.
dependencies:
  tools:
    - type: builtin
      value: search_knowledge_base
      description: Finds local evidence.
    - type: mcp
      value: github.search_issues
      transport: streamable_http
      description: Optional connector-provided issue search.
policy:
  allowImplicitInvocation: true
```

## Resource Policy

Resources are classified by path:

| Directory | Resource kind | Purpose |
| --- | --- | --- |
| `scripts/` | `script` | Helper scripts invoked through approved tools |
| `references/` | `reference` | Longer examples, checklists, schemas, or playbooks |
| `agents/` | `metadata` | Agent interface and dependency metadata |
| `assets/` | `asset` | Images, templates, examples, or binary resources |

The frontend receives resource metadata only. Full resource contents stay on the
core side until a tool or skill explicitly needs them.

## Safety Model

Skill package scanning is advisory but strict enough to surface dangerous
imports before activation. The scanner checks for:

- missing `name` or `description` frontmatter
- unusually large `SKILL.md`
- wildcard `allowed-tools`
- shell tool grants
- recursive force deletion patterns
- remote script execution patterns such as `curl ... | sh`
- dynamic code execution or obfuscated payload indicators

Blocked findings should require explicit user review before import or proposal
apply. Skills still cannot bypass tool approvals or source scope.

## Dependency Model

A skill can depend on:

- built-in tools by stable tool name
- MCP connector tools by connector/tool identity
- optional runtime capabilities described in its references

A skill does not own those tools. If a skill needs a new external tool, the
right path is to add or enable a connector. If a skill needs a new host ability,
that request should become a capability package or adapter discussion.

## Built-In Status

Nexa's bundled skills are represented by the `builtin-skills` package manifest
with surface `skill_package`. Each built-in skill also appears as a component
entry under:

```text
.nexa/capabilities/builtin-skills/skills/<skill-id>/SKILL.md
```

This is the migration bridge from today's bundled assets to the long-term
capability package layout.

Project-local skill package manifests can use the same
`.nexa/capabilities/*/capability.yaml` discovery path as other capability
packages.
