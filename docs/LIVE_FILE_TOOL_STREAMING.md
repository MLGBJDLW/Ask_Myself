# Live file-tool streaming and long-write contract

## Scope

This document defines the runtime contract for plain-text file tools that receive model-generated JSON arguments incrementally. It covers `create_file`, `edit_file`, `multi_edit`, and `write_note`, with `create_file` also providing the resumable long-write path.

The design separates three concerns that must not be conflated:

1. **Provider argument transport** — an incomplete JSON byte stream that is not yet safe to execute.
2. **Semantic UI preview** — bounded, best-effort file-change artifacts derived from the partial arguments.
3. **Authoritative execution** — one complete, schema-valid JSON argument object and its final tool result.

A preview is never an execution acknowledgement. A completed tool result is never reconstructed from preview state.

## Problems addressed

### Misdiagnosed “encoding limits”

The plain-text writer accepts UTF-8 Rust strings and writes their bytes. The practical limits are upstream model output, function-call payload size, runtime argument guards, and frontend event compaction—not a special UTF-8 encoding limit.

The previous contract told the model to avoid giant payloads but offered no general append/resume operation for ordinary text. That gap encouraged the model to abandon the file deliverable and claim an encoding limitation. `create_file` now supports ordered `append` chunks with an optimistic byte precondition.

### Lost preparing-state events

The backend already emitted canonical `ToolRunStarted` and `ToolRunUpdated` items while arguments were being assembled. The frontend replaced preparing runs with a delayed legacy placeholder and discarded the richer run payload. This prevented live diff artifacts from reaching the existing diff ticker and preview components.

Canonical tool-run events now update the state tree immediately. The delayed `toolCallPreparing` placeholder is retained only as a compatibility fallback for producers that do not emit canonical runs.

### Partial JSON cannot be parsed normally

Provider argument deltas commonly end inside a JSON string. A strict `serde_json::from_str` therefore returns no object until the closing quote and brace arrive—usually after almost all file content has already streamed.

The runtime now uses a display-only tolerant parser that extracts complete top-level keys and the safe prefix of an unfinished top-level string. It decodes complete JSON escapes, including Unicode surrogate pairs, and stops before incomplete or invalid escapes. The strict parser remains the only execution parser.

## Runtime invariants

### Execution safety

- Only complete JSON is sent to a tool implementation.
- Partial parsing must never trigger filesystem mutation, approval, or scheduling decisions that grant broader access.
- Preview artifacts are marked with `preview: true`.
- The completed tool-run event replaces preview state and remains authoritative.
- Path scoping, Office/PDF guards, approval policy, checkpointing, and tool argument size guards continue to apply.

### Bounded streaming

- Provider assembly accepts at most 1 MiB for one model-authored tool argument,
  and file-mutation validation uses the same shared limit.
- The Tool input session emits the first semantic preview and then one preview
  per 2 KiB cumulative-input bucket; provider assembly itself remains lossless.
- Semantic file diffs include at most the shared diff-line cap.
- Preparing-state parsing and raw arguments are bounded to a 32 KiB diagnostic
  prefix before diff construction or frontend transport.
- `inputProgress.receivedBytes` reports the cumulative provider argument size even when the displayed raw arguments are truncated.
- The desktop Tool preview journal replaces snapshots by `callId` and publishes
  only after another 2 KiB of growth or a two-second heartbeat.
- Preparing updates use sequenced ephemeral Run Events and are never written to
  the durable SQLite ledger.
- UI state is patched by `callId`; preparing updates must not create duplicate cards.

### Event lifecycle

```text
provider argument delta
        │
        ▼
accumulate complete argument buffer
        │
        ├─ complete JSON seals execution ──────┐
        │                                      │
        └─ 2 KiB Tool input session bucket     │
                 │                             │
                 ▼                             │
       bounded tolerant parse + diff            │
                 │                             │
                 ▼                             │
 ToolRunStarted + ephemeral ToolRunUpdated       │
                 │                             │
                 ▼                             │
 frontend upsert by callId                     │
                 │                             │
                 ▼                             │
 live +/- ticker and optional diff details     │
                                               ▼
                                ToolCallStart + authoritative execution
                                               │
                                               ▼
                                  ToolRunCompleted(final artifacts)
```

Legacy `ToolCallPreparing` remains useful when only a tool name, call ID, and argument byte count are available. It must not overwrite a canonical preparing run.

## Resumable plain-text writes

`create_file` supports three explicit modes:

| Mode | Existing path | Required precondition | Effect |
| --- | --- | --- | --- |
| `create` | must not exist | none | creates a new file |
| `overwrite` | may exist | none | replaces the whole file under a checkpoint |
| `append` | must be a regular file | `expected_bytes` | appends one UTF-8 chunk under a checkpoint |

Recommended long-write sequence:

1. Write the first coherent section with `mode: "create"`.
2. Read `writeProgress.nextExpectedBytes` from the successful result.
3. Send one coherent continuation with `mode: "append"` and `expected_bytes` equal to that returned value.
4. Repeat from the latest successful result until complete.
5. On a precondition mismatch, inspect the current file state. Do not blindly replay the chunk.

The byte precondition makes retries fail closed: a chunk that already succeeded changes the file size, so resending it with the old offset is rejected. It also prevents dependent chunks from being applied out of order. Checkpoints make each successful append reversible.

For Unicode content, callers should use the returned byte offset rather than calculating character counts. A model should normally issue one dependent append per tool-result round; parallel calls are appropriate only for independent paths.

## UI projection

The frontend already has two useful surfaces:

- a compact diff-stat ticker in the tool trace header;
- an expandable file diff preview with addition/deletion lines.

Live behavior depends on preserving the canonical preparing run and its artifacts. The reducer therefore:

1. clears any pending legacy placeholder timer for the same `callId`;
2. applies every canonical started/updated/completed run;
3. patches both global tool-call state and the owning stream round;
4. keeps the final status transition authoritative.

## Failure handling

- Invalid final JSON remains a tool-contract error; a partial preview does not make it executable.
- An append without `expected_bytes` fails and reports the current file size.
- A mismatched byte precondition fails before checkpoint creation or mutation.
- A failed append attempts to restore the previous file length; the checkpoint is an additional recovery mechanism.
- Existing path traversal, source-scope, generated-document, and approval checks remain in force.
- Stream terminal events clear pending legacy timers so late placeholders cannot reappear.

## Test matrix

Backend tests cover:

- incomplete JSON strings producing a live diff;
- escaped newlines and Unicode surrogate pairs;
- stopping before incomplete escapes;
- bounded raw preparing arguments with unbounded semantic counts;
- create, overwrite, ordered append, precondition mismatch, and checkpoint restore.

Frontend contract tests cover:

- immediate insertion of a canonical preparing file run;
- preservation of `fileChangePreview` artifacts;
- in-place updates of additions and arguments;
- absence of duplicate tool cards.

## Known boundaries and follow-up metrics

The current policy intentionally favors deterministic byte buckets over
content-aware parsing of every fragment. Track these counters before changing
the bucket or heartbeat policy:

- preparing updates per call;
- cumulative input bytes versus emitted preview bytes;
- parser time and diff-build time;
- frontend reducer updates and rendered frames;
- time to first semantic preview;
- time from final delta to authoritative start;
- ephemeral previews delivered versus dropped under queue pressure;
- durable Run Event rows per file-tool call.

Do not promote previews back into durable history or use preview state to infer
that a tool executed. The durable started boundary and final authoritative
lifecycle state must remain observable even if intermediate snapshots are
dropped.

## Prompt ownership

The active runtime prompt is composed from the core kernel, route scaffolding, skills, and enabled tool definitions. Long-write instructions belong in the `create_file` tool definition because that schema is injected only when the capability is available and stays synchronized with executable parameters.

The older standalone `crates/core/prompts/system.md` contains broader historical routing text and is not the default prompt kernel. Treat differences there as prompt-documentation drift rather than relying on it to define executable file-tool behavior.
