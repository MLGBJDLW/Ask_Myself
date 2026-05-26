# Capability Packages

Capability packages are Nexa's manifest-backed package format. They group a
coherent ability under one package root so tools, skills, workflows, commands,
hooks, tests, settings, runtime checks, and permissions can be described through
one interface.

Capability packages are not automatically native plugins. A package can describe
a core platform ability, a built-in capability, a connector, a skill package, a
workflow package, an adapter, a host surface, or a future native plugin.

## Standard Layout

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

`capability.yaml` is the package manifest:

```yaml
id: office-documents
name: Office Documents
surface: capability_package
description: Works with PPT, DOCX, XLSX, PDF, and HTML document flows.
version: 1
tools:
  - prepare_document_tools
  - get_document_info
settingsSurfaces:
  - office-runtime
workflows:
  - generate-presentation
permissions:
  read: true
  write: false
  execute: false
  network: false
  nativeCode: false
```

## Supported Surfaces

The `surface` field must be one of:

- `core_platform`
- `capability_package`
- `connector`
- `skill_package`
- `workflow_package`
- `adapter`
- `host_surface`
- `native_plugin`

Native code is valid only for `surface: native_plugin`. The manifest validator
rejects `nativeCode: true` on every safer surface.

## Discovery

Nexa can now discover project-local manifests from:

```text
.nexa/capabilities/*/capability.yaml
```

Discovery parses YAML, validates the manifest, and returns manifests sorted by
package id. Invalid manifests fail discovery instead of being silently ignored.
Duplicate package ids are rejected by the combined ecosystem catalog.

This gives the ecosystem a real package boundary without enabling arbitrary
third-party code execution.

## Built-In Bridge

The existing built-in manifest API is still named `PluginManifest` for desktop
compatibility, but each built-in manifest now exposes `ecosystemSurface` and can
be converted into a capability package manifest.

Bundled skills and workflows also expose package manifests:

- `builtin-skills` uses `surface: skill_package`
- `builtin-workflows` uses `surface: workflow_package`

The core ecosystem catalog has two layers:

- `builtin_ecosystem_manifests()` returns bundled capability, connector, skill,
  and workflow manifests.
- `ecosystem_manifests(project_root)` merges bundled manifests with
  `.nexa/capabilities/*/capability.yaml` and rejects duplicate package ids.

These bridge today's bundled assets to the long-term package layout.
