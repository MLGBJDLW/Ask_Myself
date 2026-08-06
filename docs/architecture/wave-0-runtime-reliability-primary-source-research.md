# Wave 0 Runtime Reliability: Primary-source Research

This note records the primary-source review for the Wave 0 work described in
`D:\Nexa.txt`. It was prepared on 2026-08-06 against immutable upstream commits.
It is a design input, not a claim that Nexa implements another project's wire
protocol or that all providers accept the same message shape.

## Executive decision

Wave 0 should establish five runtime contracts before adding new UI or media
capabilities:

1. Validate a provider-neutral conversation model, then project it into each
   provider's wire format and validate that projection again.
2. Compute a subagent's effective capabilities as a narrowing intersection and
   reject invalid runs in a typed preflight phase before model execution.
3. Treat tool-argument deltas as an assembly protocol, not a user-interface
   protocol. Only dedicated diff and terminal streams should update live.
4. Represent connection and retry progress as versioned state events, separate
   from assistant text and reasoning.
5. Move audio across the webview/native boundary as bounded binary chunks and
   spool behind a bounded queue. Never serialize bytes through `Array.from`.

These contracts intentionally stop short of the complete Wave 3 recorder
rewrite. AudioWorklet migration, pause/resume UX, and the full recording dock
remain follow-up work.

## Reviewed Nexa seams

The current implementation has the right integration points but does not yet
enforce the contracts end to end:

- `crates/core/src/llm/` owns the provider-neutral `Message` type and is the
  correct home for shared message validation.
- `apps/desktop/src-tauri/src/commands.rs` currently repairs request history in
  `sanitize_tool_call_history` and substitutes `[Empty assistant message]` for
  an empty assistant record. Repair at this late call site can hide provenance
  and does not protect every provider or subagent path.
- `apps/desktop/src-tauri/src/subagent_tool.rs` builds delegated history,
  selects a model, narrows tools, and applies deadlines. It is the natural
  orchestration boundary for one typed preflight result.
- `apps/desktop/src-tauri/src/agent_stream.rs` forwards
  `ToolCallArgsDelta`; `apps/desktop/src/lib/streamStore.ts` then places live
  stream state in React-facing storage. Assembly and presentation should split
  before that boundary.
- `apps/desktop/src/features/voice/useVoiceRecorder.ts` records through a
  `ScriptProcessorNode` and retains PCM chunks until stop.
- `apps/desktop/src/features/voice/voiceInputRuntime.ts` sends both final WAV
  and realtime PCM with `Array.from`, while the Tauri commands accept JSON
  number arrays. This is the first Wave 0 audio transport seam.

## 1. Provider-safe conversation normalization

### Evidence

The OpenAI OpenAPI schema permits assistant content to be null only when a tool
call or legacy function call is present; an array content value must contain at
least one item. A tool result requires content and a `tool_call_id` that names
the call it answers
([assistant schema](https://github.com/openai/openai-openapi/blob/dc708bbe9a149bc35132c567ef3a3fdd7a24ab49/openapi.yaml#L30876-L30944),
[tool message schema](https://github.com/openai/openai-openapi/blob/dc708bbe9a149bc35132c567ef3a3fdd7a24ab49/openapi.yaml#L31215-L31244)).
Therefore `content: null` plus a complete non-empty tool call is valid OpenAI
input; empty content with no call is not.

The matching requirement is also explicit in LangGraph's prebuilt tool node:
every model tool call in history must have a corresponding tool message
([source](https://github.com/langchain-ai/langgraph/blob/fb3d5f0399222504e015fe959e0e79fdc6e00a65/libs/prebuilt/langgraph/prebuilt/tool_node.py#L1571-L1578)).
OpenAI Codex applies a stronger fork boundary: it carries system, developer,
and user messages but only assistant items whose phase is `FinalAnswer`, rather
than inheriting partial calls, outputs, or reasoning
([source](https://github.com/openai/codex/blob/aac9f842473ac6a05d417dd76ce8b89bdb3b707d/codex-rs/core/src/agent/control/spawn.rs#L47-L75)).
Its context normalization separately repairs missing call outputs and removes
orphan outputs before sending history
([source](https://github.com/openai/codex/blob/aac9f842473ac6a05d417dd76ce8b89bdb3b707d/codex-rs/core/src/context_manager/normalize.rs#L20-L129)).

The provider projections are not interchangeable. Anthropic's generated
official SDK represents both `tool_use` and `tool_result` as content blocks and
links a result through required `tool_use_id`
([message blocks](https://github.com/anthropics/anthropic-sdk-python/blob/f5c30d0490fb7bcd8e0b65d8d8e63c0e7d1bfe59/src/anthropic/types/message_param.py#L30-L59),
[tool result](https://github.com/anthropics/anthropic-sdk-python/blob/f5c30d0490fb7bcd8e0b65d8d8e63c0e7d1bfe59/src/anthropic/types/tool_result_block_param.py#L22-L32)).
Google's official GenAI SDK example resends the model's function-call content
and follows it with `role='tool'` function-response content
([source](https://github.com/googleapis/python-genai/blob/a8ec86eab28c2806205fc8ec746b492110113c44/README.md#L775-L819)).

### Nexa contract

Introduce one provider-neutral validation result that preserves provenance:

```rust
enum MessageSource {
    Persisted,
    Stream,
    Recovery,
    SubagentHandoff,
}

enum HistoryIssueKind {
    EmptyAssistant,
    IncompleteToolCall,
    OrphanToolResult,
    MissingToolResult,
    DuplicateToolResult,
    InvalidRoleSequence,
}
```

The canonical history invariant is:

- An assistant item has nonblank visible content, at least one complete tool
  call, or another explicitly supported non-text output form.
- Every tool call has a stable non-empty id, a name, and complete parseable
  arguments before it may be persisted as executable or dispatched.
- Every tool result refers to one preceding call exactly once unless a provider
  explicitly permits multiple result blocks for that call.
- Transient stream fragments are never promoted directly to durable history.
- Thought/reasoning is not copied into visible content to make an otherwise
  invalid assistant item pass validation.

Normalization must be policy-driven rather than always inserting placeholder
text:

- a known interrupted stream suffix may be dropped or marked interrupted;
- a missing result for a known aborted call may receive a typed synthetic
  aborted result;
- an unexplained persisted invalid record must be quarantined with a structured
  issue, not silently rewritten into user-visible assistant speech;
- a valid assistant tool-call-only item must remain tool-call-only.

Run the invariant at four boundaries: stream finalization, persistence,
subagent snapshot/fork, and immediately before provider serialization. After
serialization, each adapter performs a provider-specific sequence check. This
keeps the shared validator independent from OpenAI, Anthropic, Gemini, Ollama,
or future wire-role conventions.

### Required tests

- Text only, tool call only, text plus tool call, and OpenAI `content: null`
  with a valid tool call remain valid.
- Empty text with no calls, empty call arrays, incomplete JSON arguments,
  orphan results, missing results, and duplicate results are rejected or
  repaired only under the documented provenance policy.
- Interrupted parent turns never produce an empty assistant item in a child.
- The same canonical fixture projects successfully through OpenAI-compatible,
  Anthropic, Gemini, and Ollama adapters with provider-specific golden tests.
- A malformed durable record reports conversation id, turn id, message index,
  source, provider, model, and issue kind without logging raw sensitive text or
  arguments.

## 2. Subagent preflight and least-privilege inheritance

### Evidence

OpenAI Codex checks spawn depth before constructing and starting the child,
then builds an explicit child configuration and applies role/model/runtime
overrides before reserving execution
([handler](https://github.com/openai/codex/blob/aac9f842473ac6a05d417dd76ce8b89bdb3b707d/codex-rs/core/src/tools/handlers/multi_agents/spawn.rs#L44-L140),
[capacity reservation](https://github.com/openai/codex/blob/aac9f842473ac6a05d417dd76ce8b89bdb3b707d/codex-rs/core/src/agent/control/spawn.rs#L382-L420)).
OpenAI Agents exposes static allow-then-block filtering and dynamic filters that
receive the active agent and run context
([source](https://github.com/openai/openai-agents-python/blob/36d50b014a92d09c9f667bf95bfc26c2f22920ca/docs/mcp.md#L382-L416)).
Its sandbox runtime preparation also validates capability dependencies before
copying a prepared agent with an explicit tool list
([source](https://github.com/openai/openai-agents-python/blob/36d50b014a92d09c9f667bf95bfc26c2f22920ca/src/agents/sandbox/runtime_agent_preparation.py#L65-L110)).

MCP annotations cannot be used as an authorization oracle. The MCP schema calls
`readOnlyHint`, `destructiveHint`, `idempotentHint`, and `openWorldHint` hints,
and explicitly says clients must not make tool-use decisions from annotations
received from untrusted servers
([source](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/2de0727d3c2d6f2f32b3fefbba0bf8395b2e7324/schema/2025-06-18/schema.ts#L869-L923)).

### Nexa contract

Compute the child tool set monotonically:

```text
registered tools
  intersect parent effective tools
  intersect workspace policy
  intersect role allowlist
  intersect task-requested tools
  minus trusted hard deny rules
  = child effective tools
```

No child prompt, model response, MCP annotation, role profile, or stored task may
widen the parent's effective authority. High-risk categories such as credential
access, external-account actions, computer use, destructive writes, and nested
delegation require explicit trusted policy and the normal approval path.

Preflight returns a typed result before starting a provider stream:

```rust
enum SubagentPreflightStage {
    History,
    Provider,
    Policy,
    Budget,
    Capacity,
    Timeout,
    SourceScope,
    Recursion,
}
```

The checks, in deterministic order, are:

1. Normalize and validate the inherited history.
2. Resolve provider credentials, endpoint, model availability, calling mode,
   and tool-calling support.
3. Calculate effective tools and verify the selected role's required
   capabilities remain available.
4. Reserve context and output budgets, including reasoning/tool overhead.
5. Check recursion depth, concurrent capacity, and queue deadline.
6. Resolve readable/writable source scope and environment roots.
7. Materialize connect, first-token, idle, and total-run deadlines.

Because credentials, policy, and capacity can change after preflight, the
executor revalidates the cheap security-critical subset at dispatch. Failures
carry `stage`, stable `code`, retryability, role, provider/model identity, and a
sanitized explanation. The UI may then offer retry only when the code is
actually retryable.

### Required tests

- Each intersection term can narrow but never widen the effective set.
- A role-required tool blocked by workspace policy fails in `Policy`, rather
  than starting an agent that cannot fulfill its role.
- MCP hints cannot bypass a trusted destructive/open-world classification.
- Bad inherited history fails in `History` before provider or capacity use.
- Missing credentials/model/tool support fail in `Provider`; exhausted context
  in `Budget`; queue/connect/first-token/run expiry in `Timeout` with distinct
  codes.
- Parallel children and restart recovery preserve the same effective-policy
  snapshot or perform an explicit, auditable revalidation.

## 3. Tool-input presentation policy

### Evidence

OpenAI's streaming schema distinguishes an argument `delta` (a JSON string
fragment) from the `done` event containing final arguments
([delta](https://github.com/openai/openai-openapi/blob/dc708bbe9a149bc35132c567ef3a3fdd7a24ab49/openapi.yaml#L48628-L48678),
[done](https://github.com/openai/openai-openapi/blob/dc708bbe9a149bc35132c567ef3a3fdd7a24ab49/openapi.yaml#L48679-L48723)).
OpenAI Agents similarly separates raw response events from higher-level events
emitted when an item is fully generated
([source](https://github.com/openai/openai-agents-python/blob/36d50b014a92d09c9f667bf95bfc26c2f22920ca/docs/streaming.md#L68-L86)).

Codex gives `apply_patch` a dedicated, buffered partial-input diff consumer,
while the default tool implementation has no diff consumer
([default](https://github.com/openai/codex/blob/aac9f842473ac6a05d417dd76ce8b89bdb3b707d/codex-rs/core/src/tools/registry.rs#L148-L163),
[apply-patch consumer](https://github.com/openai/codex/blob/aac9f842473ac6a05d417dd76ce8b89bdb3b707d/codex-rs/core/src/tools/handlers/apply_patch.rs#L56-L99)).
Its app protocol streams command stdout/stderr separately and sends parsed file
change snapshots while the legacy raw patch-output delta is no longer emitted
([source](https://github.com/openai/codex/blob/aac9f842473ac6a05d417dd76ce8b89bdb3b707d/codex-rs/app-server/README.md#L1580-L1588)).
VS Code likewise lets tools request `Hidden` or `HiddenAfterComplete`
presentation instead of assuming all invocations belong in chat
([source](https://github.com/microsoft/vscode/blob/ae29e2dd05bc35c3f35a5d09819c996eae85e278/src/vs/workbench/contrib/chat/common/tools/languageModelToolsService.ts#L396-L420)).

### Nexa contract

Use an explicit presentation policy independent of execution permission:

```ts
type ToolInputPresentation =
  | "hidden"
  | "summary_when_complete"
  | "live_diff"
  | "live_terminal";
```

- A non-React assembler keyed by call id receives raw argument deltas, enforces
  a byte limit, and validates the final payload.
- Search, read, fetch, retrieval, and delegation remain hidden while assembling;
  after completion they show one sanitized semantic summary.
- Shell JSON remains hidden. Once the command is complete, show the command once
  and stream stdout/stderr through the terminal lifecycle.
- Edit, patch, and multi-edit may feed only parsed, path-scoped snapshots to a
  dedicated live-diff renderer.
- Approval and user-input tools use dedicated interaction cards. Credential or
  secret fields never enter the ordinary tool summary.
- Development diagnostics may retain a redacted, size-limited final payload,
  but generic per-delta React updates are prohibited.

Gate the new policy and record argument bytes, assembler duration, React commit
count, long tasks, and dropped/truncated diagnostic bytes. This distinguishes a
real performance improvement from a merely quieter card.

### Required tests

- Fragmented and invalid JSON never appears in the visible card.
- Generic tools do not publish React-facing updates for each argument delta.
- Shell output remains live after the complete command becomes available.
- Edit tools receive structured diff snapshots; non-edit tools cannot select
  `live_diff` through model-controlled metadata.
- Redaction covers secrets, credentials, headers, tokens, and configured
  sensitive keys in both summaries and telemetry.

## 4. Structured connection and retry events

### Evidence

Codex's app protocol reports separate typed categories for HTTP connection
failure, response-stream connection failure, midstream disconnection, and too
many failed attempts, preserving an upstream status code when known
([source](https://github.com/openai/codex/blob/aac9f842473ac6a05d417dd76ce8b89bdb3b707d/codex-rs/app-server/README.md#L1590-L1611)).
It uses bounded ingress queues and tells clients to treat overload as retryable
with exponential backoff and jitter
([source](https://github.com/openai/codex/blob/aac9f842473ac6a05d417dd76ce8b89bdb3b707d/codex-rs/app-server/README.md#L51-L55)).
Realtime lifecycle also has distinct started, transcript delta/done, error, and
closed notifications
([source](https://github.com/openai/codex/blob/aac9f842473ac6a05d417dd76ce8b89bdb3b707d/codex-rs/app-server/README.md#L1496-L1507)).

### Nexa contract

```ts
interface ConnectionStateEvent {
  schemaVersion: 1;
  sequence: number;
  generation: number;
  connectionId: string;
  conversationId?: string;
  turnId?: string;
  scope: "model_stream" | "mcp" | "stt" | "realtime" | "environment";
  state: "connecting" | "connected" | "reconnecting" |
         "degraded" | "failed" | "closed";
  attempt: number;
  maxAttempts?: number;
  backoffMs?: number;
  nextRetryAt?: string;
  reason: ConnectionReason;
  retryable: boolean;
  statusCode?: number;
  occurredAt: string;
}
```

Interim connection events and the terminal turn result are different records.
Reducers reject stale sequence numbers and handle duplicate delivery
idempotently. A new socket/transport increments `generation`; callbacks from an
older generation cannot mutate the replacement connection. OpenAI Agents JS
uses the same current-socket identity guard to ignore stale WebSocket callbacks
([source](https://github.com/openai/openai-agents-js/blob/9b97dd2b5420b9241aeb29ee55af6f4261b2febb/packages/agents-realtime/src/openaiRealtimeWebsocket.ts#L132-L171)).
Reconnect state renders in a status banner/card and telemetry; it must not be
converted into assistant content or ordinary thinking text. A successful
reconnect closes the active degraded interval and preserves attempt count for
diagnostics.

Reason values should be a stable closed enum with an `other` fallback, including
at least authentication, rate limit, overload, HTTP connect, stream connect,
midstream disconnect, provider timeout, user cancellation, offline, and
protocol error. Human copy is derived at the UI boundary, never parsed back
into runtime state.

### Required tests

- reconnecting -> connected, reconnecting -> failed, cancellation, offline,
  overload, and midstream disconnect have deterministic reducer fixtures;
- duplicate and out-of-order events cannot regress state;
- callbacks from a stale connection generation are ignored;
- retry metadata reaches the banner and telemetry without entering transcript
  or reasoning history;
- HTTP status and retryability survive provider-to-core-to-Tauri-to-UI mapping.

## 5. STT binary transport, backpressure, and spooling

### Evidence

Tauri documents that normal serializable values use JSON and can be slow for
large data. It supports optimized array-buffer responses and, in the other
direction, a raw `ArrayBuffer` or `Uint8Array` invoke body received as
`InvokeBody::Raw`, with headers available for metadata
([binary response](https://github.com/tauri-apps/tauri-docs/blob/05a224e96fdb1c10be8526be2a11fef690bc3f4f/src/content/docs/develop/calling-rust.mdx#L194-L206),
[raw request](https://github.com/tauri-apps/tauri-docs/blob/05a224e96fdb1c10be8526be2a11fef690bc3f4f/src/content/docs/develop/calling-rust.mdx#L522-L549)).
Tauri channels are the recommended native-to-frontend streaming mechanism
([source](https://github.com/tauri-apps/tauri-docs/blob/05a224e96fdb1c10be8526be2a11fef690bc3f4f/src/content/docs/develop/calling-rust.mdx#L404-L425));
frontend-to-native audio should use raw binary invokes rather than JSON events.
They are not the capture backpressure queue: Tauri's JavaScript channel keeps
an unbounded pending-message array for reordering, while the Rust side uses an
unbounded map for large payloads
([JavaScript source](https://github.com/tauri-apps/tauri/blob/0aeadb6b2674ecd43f15b5dd6fcace3232f74b8a/packages/api/src/core.ts#L77-L130),
[Rust source](https://github.com/tauri-apps/tauri/blob/0aeadb6b2674ecd43f15b5dd6fcace3232f74b8a/crates/tauri/src/ipc/channel.rs#L138-L180)).
Reserve channels/events for low-rate pressure, transcript, and connection-state
notifications.

Tokio's bounded MPSC waits when the configured message capacity is full and
preserves send order
([source](https://github.com/tokio-rs/tokio/blob/108d6d3dc038332af2af83957748333091e35b3f/tokio/src/sync/mpsc/bounded.rs#L111-L124)).
Its unbounded variant explicitly warns that a slow receiver can arbitrarily
buffer until the process runs out of memory
([source](https://github.com/tokio-rs/tokio/blob/108d6d3dc038332af2af83957748333091e35b3f/tokio/src/sync/mpsc/unbounded.rs#L85-L95)).
The `tempfile` crate's `SpooledTempFile` keeps bytes in memory until a configured
size and then moves subsequent I/O to a temporary file
([source](https://github.com/Stebalien/tempfile/blob/889f7bfbd8a61cfa87cb0886c4dbfbca1ef08919/src/spooled.rs#L7-L25)).

The Web Audio specification marks `ScriptProcessorNode` deprecated in favor of
`AudioWorkletNode`
([source](https://github.com/WebAudio/web-audio-api/blob/ad5d2ed145e43f1a818bfe29a792365e9386eb6a/index.bs#L9891-L9898)).
That supports the Wave 3 direction, but replacing the capture node is not
required to remove Wave 0's `Array.from` and unbounded buffering risks.

### Nexa contract

Use a recording-session protocol:

```text
start_audio_ingest(metadata) -> recording_id
append_audio_chunk(recording_id, sequence, raw Uint8Array)
finish_audio_ingest(recording_id) -> transcript/job
cancel_audio_ingest(recording_id)
```

The raw Tauri request body carries only bytes; recording id, sequence, sample
format, and integrity metadata travel in validated headers or another small
control command. Enforce per-chunk, total-byte, duration, inactivity, concurrent
session, and disk-space limits before accepting more data.

The native session owns a bounded Tokio MPSC queue consumed by an encoder/spool
worker. Because MPSC capacity is message-count based, either use a fixed maximum
chunk size or pair it with byte permits so the real bound is bytes/audio
duration. Never use an unbounded channel for PCM.

The renderer awaits each append or keeps only a small fixed in-flight window.
An audio callback must not block on native I/O; when the window is full, apply
an explicit product policy:

- final dictation/archive mode preserves audio by spooling and reports pressure;
- realtime partial transcription may enter `degraded` only under a documented
  drop policy, while final capture remains complete when enabled;
- no mode silently grows a promise chain or drops chunks without a sequence-gap
  counter and structured state event.

The native consumer writes into a configurable spool. `SpooledTempFile` is a
suitable implementation option for ephemeral recordings, but it is only a
transitive dependency in the current lockfile; adding it directly requires a
manifest, lockfile, and third-party-notice audit. If restart recovery is a
requirement, use Nexa-managed application storage with explicit lifecycle and
deletion rather than relying on an anonymous temporary file.

The memory-to-disk threshold is not a total-size limit: after rollover the file
continues growing
([source](https://github.com/Stebalien/tempfile/blob/889f7bfbd8a61cfa87cb0886c4dbfbca1ef08919/src/spooled.rs#L199-L238)).
Hard per-session byte/time/free-space quotas therefore remain mandatory.

On finish, stream/read the spooled source into encoding or the provider upload;
do not reconstruct the entire recording as a JavaScript number array or a
second renderer-resident PCM/WAV copy. If a provider protocol requires Base64
JSON, conversion occurs once inside that provider adapter, after the internal
binary queue/spool boundary; OpenAI Agents JS follows that pattern for Realtime
audio
([source](https://github.com/openai/openai-agents-js/blob/9b97dd2b5420b9241aeb29ee55af6f4261b2febb/packages/agents-realtime/src/openaiRealtimeBase.ts#L936-L957)).

### Telemetry and tests

Record only numeric/structural audio diagnostics:

- `queuedBytes`, `inFlightChunks`, `maxQueueDepth`, `enqueueWaitMs`, and IPC
  latency;
- in-memory/spooled bytes, spool transition count, disk-write latency, and
  available-disk failure;
- recording duration, chunk size, sequence gaps, rejected/dropped chunks, and
  cancellation cleanup;
- renderer long tasks, event-loop lag, renderer/native memory high-water marks,
  and time from stop to first transcription progress/final result.

Tests must cover byte-for-byte raw IPC round trips, chunk reordering/replay,
slow-consumer backpressure, provider stall, quota breach, cancellation,
low-disk/write failure, window close, and long-recording memory plateaus. Logs
must not contain PCM, transcripts, credentials, or raw provider payloads.

## Rollout order and acceptance

Keep independently reversible feature flags for history enforcement, tool
presentation, structured connection events, and binary audio transport. A safe
Wave 0 sequence is:

1. Land shared history validation plus provider projection tests.
2. Reuse it from subagent preflight, then add capability intersection and typed
   stage errors.
3. Split tool assembly from presentation and enable hidden generic deltas.
4. Introduce the versioned connection reducer and migrate reconnect UI.
5. Add audio telemetry, then move final and realtime audio to the bounded raw
   binary ingest protocol.

The Wave exits only when no provider receives an invalid empty assistant item,
subagent failures identify the correct stage, ordinary tool argument fragments
never render, reconnect state does not enter assistant/reasoning history, and a
long recording has a demonstrably bounded renderer/native memory profile.

## License and integration boundaries

| Upstream | License at reviewed commit | Permitted use in this design | Boundary |
| --- | --- | --- | --- |
| OpenAI OpenAPI | [MIT](https://github.com/openai/openai-openapi/blob/dc708bbe9a149bc35132c567ef3a3fdd7a24ab49/LICENSE) | Contract reference and independent tests | Do not vendor the full schema |
| OpenAI Codex | [Apache-2.0](https://github.com/openai/codex/blob/aac9f842473ac6a05d417dd76ce8b89bdb3b707d/LICENSE) | Protocol and normalization concepts | Prefer independent implementation; retain notices if code is copied |
| OpenAI Agents Python | [MIT](https://github.com/openai/openai-agents-python/blob/36d50b014a92d09c9f667bf95bfc26c2f22920ca/LICENSE) | Tool-filter and semantic-event concepts | No Python runtime dependency |
| LangGraph | [MIT](https://github.com/langchain-ai/langgraph/blob/fb3d5f0399222504e015fe959e0e79fdc6e00a65/LICENSE) | Call/result matching invariant | No Python dependency or copied implementation |
| Anthropic Python SDK | [MIT](https://github.com/anthropics/anthropic-sdk-python/blob/f5c30d0490fb7bcd8e0b65d8d8e63c0e7d1bfe59/LICENSE) | Generated wire-shape reference | Adapter reference only |
| Google GenAI Python SDK | [Apache-2.0](https://github.com/googleapis/python-genai/blob/a8ec86eab28c2806205fc8ec746b492110113c44/LICENSE) | Function-call sequence reference | Adapter reference only; no Python dependency |
| VS Code | [MIT](https://github.com/microsoft/vscode/blob/ae29e2dd05bc35c3f35a5d09819c996eae85e278/LICENSE.txt) | Presentation-policy precedent | Do not copy workbench implementation |
| MCP schema | [mixed MIT/Apache-2.0 transition; documentation CC-BY-4.0](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/2de0727d3c2d6f2f32b3fefbba0bf8395b2e7324/LICENSE) | Tool-annotation trust boundary | Implement against Nexa's negotiated MCP version |
| Tauri docs/runtime | [docs MIT](https://github.com/tauri-apps/tauri-docs/blob/05a224e96fdb1c10be8526be2a11fef690bc3f4f/LICENSE); runtime already integrated under its applicable license | Raw IPC and channel APIs | Use installed Tauri API; do not invent wire compatibility |
| Tokio | [MIT](https://github.com/tokio-rs/tokio/blob/108d6d3dc038332af2af83957748333091e35b3f/LICENSE); already a direct Nexa dependency | Bounded MPSC | Keep byte/duration bound in addition to message count |
| `tempfile` | [MIT](https://github.com/Stebalien/tempfile/blob/889f7bfbd8a61cfa87cb0886c4dbfbca1ef08919/LICENSE-MIT) OR [Apache-2.0](https://github.com/Stebalien/tempfile/blob/889f7bfbd8a61cfa87cb0886c4dbfbca1ef08919/LICENSE-APACHE); currently transitive | Optional ephemeral spool | Make direct and audit notices before relying on its API |
| Web Audio specification | [W3C Software and Document License](https://github.com/WebAudio/web-audio-api/blob/ad5d2ed145e43f1a818bfe29a792365e9386eb6a/LICENSE.md) | Browser API behavior reference | Use platform API; do not copy specification text/source |

All links above point to upstream primary code, generated official API schema,
or official specifications. No implementation should copy substantial upstream
code merely because its behavior informed this contract.
