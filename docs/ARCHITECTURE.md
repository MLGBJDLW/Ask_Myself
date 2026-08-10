# Nexa Architecture

Status: canonical architecture entry point.

Nexa is a local-first desktop assistant composed of a React user interface in a
Tauri shell, a Rust application and agent runtime, and SQLite-backed durable
state. This document is the single architecture index. Detailed contracts live
in the focused documents linked below; research notes and dated audits do not
become architecture merely by being stored in the repository.

## System boundaries

| Boundary | Responsibility | Primary implementation |
| --- | --- | --- |
| Desktop presentation | Navigation, chat, settings, task state, previews, accessibility, and user-controlled interaction | `apps/desktop/src` |
| Native desktop bridge | Tauri commands, window and tray ownership, operating-system integration, and event projection | `apps/desktop/src-tauri` |
| Core runtime | Agent turns, provider adapters, tools, retrieval, persistence, workflows, and local media/document capabilities | `crates/core/src` |
| Shared catalogs | Provider, model, modality, and capability descriptors consumed by both frontend and backend | `shared/` |
| Durable state | Conversations, turns, tool results, checkpoints, settings, sources, and indexes | SQLite migrations and stores under `crates/core/src` |

The UI is a projection of runtime state, not a second source of truth. Provider
payloads and operating-system events enter through explicit adapters. Durable
records authorize replay and recovery; transient UI state must not invent a
successful tool execution or model response.

## Run Event publication boundary

The core runtime owns one Run Event outbox per Agent Run. It is the sole
authority for Run Event sequencing, batching, Run Event-derived task
projection, and terminal acceptance. Producers submit unsequenced events
through a bounded, non-blocking interface; the native desktop bridge supplies
only the post-commit delivery adapter. Consequently, a missing main window
cannot block durable run completion, and presentation code cannot race the core
ledger with a second sequence or terminal decision.

Resumable phases such as paused and awaiting user input keep the same outbox
open. True terminal outcomes close it permanently, and finalization crosses the
completion barrier only after both the terminal event and task projection are
durable. Per-run lifecycle serialization also covers durable continuation
claims, executor spawn, session registration, pause, and stop. Startup recovery
uses the Run Event ledger to restore suspensions or terminalize interrupted
work instead of letting task rows invent a second outcome. The detailed
batching, failure, recovery, and wire rules are normative in the
[Agent Streaming Protocol](./AGENT_STREAMING_PROTOCOL.md).

## Cross-cutting invariants

1. **Local-first ownership.** Indexes, conversation history, settings, and
   project state remain local. External providers receive only the scoped input
   required for the selected request.
2. **Durability before continuation.** State required by the next model request
   or recovery path must commit successfully before it is appended to the live
   context or exposed as completed.
3. **Provider-boundary fidelity.** Provider-native transcripts are replayed once
   and validated at the final wire boundary. Display projections never replace
   opaque reasoning signatures, tool-call IDs, or provider ordering rules.
4. **One capability catalog.** Shared catalog descriptors and the configured
   endpoint define model capabilities. UI and Rust adapters must not maintain
   divergent provider facts or grant trusted credentials to unknown endpoints.
5. **Explicit authority.** Read, write, desktop-control, terminal, connector,
   and native-plugin operations retain their permission and workspace
   boundaries. Tool output and external content are untrusted input.
6. **Recoverable interaction.** Long-running work, user questions, approvals,
   cancellations, and checkpoints use durable lifecycle records rather than
   prompt-only state.
7. **Bounded presentation.** Streaming and trace surfaces stay responsive,
   preserve reduced-motion behavior, and avoid turning internal diagnostics into
   normal chat content.

## Normative runtime contracts

- [Agent Streaming Protocol](./AGENT_STREAMING_PROTOCOL.md) defines the core
  Run Event outbox, versioned wire schema, commit-before-delivery ordering,
  batching, terminal completion barrier, fail-closed recovery, block offsets,
  and window-scoped projection channels.
- [Terminal and Agent Bridge](./TERMINAL_AGENT_BRIDGE.md) defines the boundary
  between the user-owned PTY and approval-gated agent interaction.
- [Live File-Tool Streaming](./LIVE_FILE_TOOL_STREAMING.md) separates partial
  previews from complete schema-valid execution and durable results.
- [Orchestration Runtime](./ORCHESTRATION_RUNTIME.md) defines workflow IR,
  fan-out, checkpoints, verification, and quality-profile behavior.
- [Ecosystem Architecture](./ECOSYSTEM_ARCHITECTURE.md) defines capability,
  connector, skill, workflow, adapter, and native-plugin lanes.
- [Computer Use Integration](./computer-use-integration.md) defines the desktop
  automation trust boundary.

## Change discipline

Architecture changes must update the smallest relevant normative document and
add regression coverage at the affected boundary. A primary-source investigation
may inform a change, but it belongs in an Issue, PR discussion, or the ignored
`docs/research/` workspace. Stable contracts should explain Nexa's behavior and
invariants, not mirror a particular upstream version or preserve a dated source
dump.

See [README.md](./README.md) for the full documentation index.
