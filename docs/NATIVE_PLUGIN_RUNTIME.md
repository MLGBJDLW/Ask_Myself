# Native Plugin Runtime

Native plugins are the last ecosystem surface Nexa should open. They allow
third-party code, hooks, or UI that cannot be represented by connectors, skills,
workflows, adapters, or capability package metadata.

## Gate Before Building

Do not start native plugin runtime work until these are stable:

- capability package manifests
- MCP connector lifecycle
- skill package import/export and scanning
- workflow package catalog
- protocol exports, starting with scoped MCP server mode

If a requested extension can be built with one of those surfaces, it should not
be a native plugin.

## Runtime Rules

Native plugins must:

- run isolated from core whenever possible
- declare permissions before activation
- declare host targets such as server, UI, or hook
- be disabled by default unless bundled by Nexa
- register tools, hooks, settings, and UI through generic host interfaces
- include compatibility version constraints
- include tests or validation metadata
- never patch core files
- never register hidden high-risk behavior

If a plugin needs a capability the host does not expose, Nexa should expand the
generic host interface. Do not add plugin-specific logic to core.

## Manifest Direction

```yaml
id: example-native-plugin
name: Example Native Plugin
surface: native_plugin
version: 1
compatibility:
  nexa: ">=0.7 <0.8"
targets:
  - server
permissions:
  read: true
  write: false
  execute: false
  network: false
  nativeCode: true
tools:
  - example_tool
hooks: []
settingsSurfaces: []
```

Native code permission is valid only for `surface: native_plugin`. The
capability manifest validator rejects native code on safer surfaces.
