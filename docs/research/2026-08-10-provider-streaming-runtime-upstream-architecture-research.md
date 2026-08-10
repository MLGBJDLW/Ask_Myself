# Provider streaming runtime architecture: Codex, Pi, and Hermes primary-source review

Date: 2026-08-10 (Asia/Shanghai)
Status: dated implementation research; not normative architecture
Scope: OpenAI Responses contract, OpenAI Codex CLI and TypeScript SDK, the Pi coding-agent repository identified below, and Nous Research Hermes Agent.
Non-scope: this note does not audit or change Nexa product code, and it does not claim that every line of any upstream repository was reviewed.

This note complements the same-day terminal/retry study. It goes deeper on event ordering, incremental function-call arguments, completion gates, backpressure, render projection, provider layering, execution-time validation, and test strategy. Every implementation link is pinned to a full commit. Statements marked **Fact** are directly supported by primary source. Statements marked **Nexa inference** are design conclusions, not claims about upstream intent.

## Executive conclusion

No examined upstream is a complete blueprint. The strongest safe design is a deliberate composition:

- Take Codex's authoritative `response.output_item.done` tool boundary, bounded async channel, completed-item persistence, and coalesced frame scheduler.
- Take Pi's clean provider/API/agent separation, `output_index`-keyed partial projection, centralized pre-execution TypeBox/JSON-Schema validation, `message_end` persistence boundary, and 16 ms coalesced rendering.
- Take Hermes' stream-attempt fencing, no-retry-after-visible-output default, post-terminal drain rule, structured error taxonomy, and explicit provider-profile/transport split.
- Do **not** copy Pi's tolerant streaming JSON parser as the final execution parser, Hermes' acceptance of a Responses stream with completed output items but no semantic terminal frame, Hermes' repair of malformed Chat-Completions tool arguments, or any automatic replay after user-visible output.

The reported `Responses function_call contained incomplete arguments` failure is an architectural boundary violation, not merely a JSON exception. Incremental argument bytes are a preview projection. A local tool becomes executable only after all of the following are true:

```text
authoritative provider completion
  + exact raw JSON parse to an object
  + tool lookup
  + schema validation
  + authorization/approval
  + single execution owner
```

The official Responses protocol also supplies `sequence_number`, yet none of the three reviewed Responses consumers enforces monotonic continuity in the examined path. Therefore sequence-gap detection, duplicate suppression, and recovery must be a Nexa-owned invariant rather than something assumed from an upstream SDK.

## Source identity, snapshots, and licenses

| Source | Identity evidence | Snapshot | License |
| --- | --- | --- | --- |
| OpenAI Responses | Official [streaming-event reference](https://platform.openai.com/docs/api-reference/responses-streaming/response/refusal/delta?lang=curl) | Live API contract reviewed 2026-08-10 | API contract; no repository license asserted |
| OpenAI Codex CLI/SDK | The repository calls Codex CLI an OpenAI local coding agent ([README](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/README.md#L1-L8)) | [`89a335ed50258dc9dc5b3d7f410db61b431244f9`](https://github.com/openai/codex/commit/89a335ed50258dc9dc5b3d7f410db61b431244f9) | [Apache-2.0](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/LICENSE#L1-L10) |
| Pi coding agent | The requested name `Pi` is otherwise ambiguous. This review uses `badlogic/pi-mono` because its own README identifies the repository as the Pi Agent Harness and its coding-agent package as the interactive coding-agent CLI ([README](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/README.md#L13-L34), [coding-agent README](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/coding-agent/README.md#L15-L19)) | [`936aff00918de1187f085f123c2812d8f2d67745`](https://github.com/badlogic/pi-mono/commit/936aff00918de1187f085f123c2812d8f2d67745) | [MIT](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/LICENSE#L1-L10) |
| Nous Research Hermes Agent | The official repository identifies Hermes Agent as built by Nous Research ([README](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/README.md#L5-L21)) | [`8359e760be499fd8e804242e7606d81dde931abb`](https://github.com/NousResearch/hermes-agent/commit/8359e760be499fd8e804242e7606d81dde931abb) | [MIT](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/LICENSE#L1-L13) |

Pi identity caveat: the pinned checkout is reached through the official `badlogic/pi-mono` remote, while its current README uses `@earendil-works/*` package names. The source identity as the Pi coding-agent project is confirmed; this note does not infer organization ownership beyond the repository and website statements. No unresolved license ambiguity was found for the three source repositories at the pinned snapshots.

## 1. Normative Responses contract

**Fact.** The official Responses streaming reference defines separate `response.function_call_arguments.delta` and `response.function_call_arguments.done` events. The delta carries a partial string; the done event carries the finalized `arguments` string and function name. Both include `item_id`, `output_index`, and `sequence_number`. The same reference says `sequence_number` is used to order streaming events. Function tool definitions use a JSON Schema `parameters` object and `strict` defaults to `true` ([official streaming events](https://platform.openai.com/docs/api-reference/responses-streaming/response/refusal/delta?lang=curl)).

**Nexa inference.** The safest interpretation is:

1. `delta` may update an ephemeral preview buffer only.
2. `function_call_arguments.done` may finalize the argument string for the item, but it does not by itself authorize a side effect.
3. `response.output_item.done` is the best item-level authority because it closes the complete function-call item, including ID, name, and arguments.
4. Tool dispatch must still independently parse exact JSON and validate the tool's local schema. Provider `strict` is generation assistance, not a local trust boundary.
5. A gap in `sequence_number` is a protocol-integrity failure. Advancing durable or executable state across the gap is unsafe.

## 2. Four separate state machines and their ownership

The clearest boundary is four cooperating machines, not one broad “streaming request” state:

| Layer | Owner | Legal terminal/transition evidence | Must not own |
| --- | --- | --- | --- |
| Transport attempt | HTTP/SSE/WebSocket adapter | socket open/EOF/error/timeout | tool dispatch, turn continuation, semantic success |
| Provider response | wire-protocol codec | `response.completed`, `response.incomplete`, `response.failed`; item added/done; ordered deltas | local tool side effects, UI frame rate |
| Tool lifecycle | tool runtime | authoritative completed call + exact parse + schema + approval; then result/error | provider reconnect, assistant-turn completion |
| Agent turn | agent loop | assistant result, tool obligations, stop policy, finite round budget | raw SSE parsing, provider-specific repair rules |

### Codex boundary

**Fact.** Codex's SSE layer parses wire events into `ResponseEvent` values. It emits `OutputItemDone` from `response.output_item.done`, classifies terminal failures, emits `Completed` from `response.completed`, and explicitly ignores function-argument delta/done frames ([decoder](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/codex-api/src/sse/responses.rs#L330-L481)). The session layer receives those normalized events. On `OutputItemDone`, it finalizes the item and may queue tool execution; on `Completed`, it records usage and ends that sampling request ([turn event loop](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/core/src/session/turn.rs#L2243-L2370), [completed branch](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/core/src/session/turn.rs#L2525-L2548)). The tool boundary first persists the completed call item, then queues the tool future ([completed-item handler](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/core/src/stream_events_utils.rs#L288-L327)).

**Nexa inference.** This is a strong separation: item completion authorizes creation of a tool obligation; response completion ends the physical sampling response; the agent turn can continue after tool results. A terminal latch in the transport must not itself decide whether the agent turn has further client-tool work.

### Pi boundary

**Fact.** Pi's provider codec constructs partial tool-call slots keyed by `output_index`, emits `toolcall_start/delta/end`, and requires a Responses terminal event before the stream returns ([Responses codec](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/api/openai-responses-shared.ts#L423-L500), [argument and terminal branches](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/api/openai-responses-shared.ts#L650-L756)). The agent loop waits for the final assistant message before extracting tool calls. Error/abort ends the agent; a length-truncated response fails every tool call without executing it; otherwise calls go through the tool executor and results can trigger another turn ([agent loop](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/agent/src/agent-loop.ts#L155-L224), [truncation gate](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/agent/src/agent-loop.ts#L374-L425)).

**Nexa inference.** Pi has the cleanest separation between previewed tool-call state and execution. Tool execution happens only after the complete assistant result is known, which makes terminal status such as `length` available as an execution veto.

### Hermes boundary

**Fact.** Hermes explicitly declares that a `ProviderTransport` owns message/tool conversion, request kwargs, and response normalization, but not client construction, streaming, credential refresh, interrupt, or retry ([transport contract](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/agent/transports/base.py#L1-L73)). Its Responses runtime assembles content from `response.output_item.done`, stops at completed/incomplete/failed, and does not use terminal `response.output` for reconstruction ([Responses rationale](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/agent/codex_runtime.py#L893-L920), [event consumer](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/agent/codex_runtime.py#L1024-L1202)). Its Chat-Completions path separately accumulates function names and arguments by tool index until the stream ends ([tool delta assembly](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/agent/chat_completion_helpers.py#L3492-L3567)).

**Fact, important exception.** Hermes' Responses consumer returns collected output items as status `completed` when the stream ends without any semantic terminal, provided usable output exists ([consumer EOF behavior](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/agent/codex_runtime.py#L1204-L1243)); a regression test explicitly preserves that behavior ([missing-terminal test](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/tests/run_agent/test_run_agent_codex_responses.py#L411-L444)).

**Nexa inference.** The Hermes transport/profile split is useful; the missing-terminal completion rule is not. Nexa should fail closed or return a typed partial outcome when a semantic terminal is missing, even if one or more output items look complete.

## 3. Incremental function/tool-call arguments

### Codex: ignore incremental JSON; trust the completed item

**Fact.** Codex ignores both `response.function_call_arguments.delta` and `.done` in its Responses decoder and uses the typed function call embedded in `response.output_item.done` ([decoder branches](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/codex-api/src/sse/responses.rs#L330-L340), [ignored events](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/codex-api/src/sse/responses.rs#L470-L481)). Its unit test includes a function-argument delta and proves it is not projected, while a custom-tool input delta is projected ([test](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/codex-api/src/sse/responses.rs#L956-L985)).

**Fact.** Tool handlers parse the completed argument string into a typed Rust structure via `serde_json::from_str`; parse failure becomes a `RespondToModel` tool error rather than execution ([typed argument parser](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/core/src/tools/handlers/mod.rs#L83-L89)). This provides exact JSON and serde type/required-field enforcement for handlers that use the helper, but it is not a single generic JSON-Schema validator for every tool.

**Nexa inference.** Codex supplies the safest completion rule, but it gives up live structured argument previews for ordinary function calls. Nexa can keep previews if the preview parser is explicitly non-authoritative and cannot reach dispatch.

### Pi: good preview state, strong schema gate, overly tolerant final parse

**Fact.** Pi appends function-argument deltas to `partialJson`, parses a preview, and emits the raw suffix. When `.done` arrives it replaces the scratch string with the authoritative `event.arguments`; when `output_item.done` arrives it replaces the argument object from the completed item, emits exactly one `toolcall_end`, removes the scratch field, and clears the slot ([argument assembly](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/api/openai-responses-shared.ts#L650-L722)). The cleanup test verifies that `partialJson` is absent from the persisted tool call ([cleanup regression](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/test/openai-responses-partial-json-cleanup.test.ts#L27-L104)).

**Fact.** `parseStreamingJson` repairs malformed strings, invokes a partial-JSON parser, and returns `{}` if all parsing fails; its contract says it always returns a valid object for potentially incomplete JSON ([streaming parser](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/utils/json-parse.ts#L85-L123)). The final `output_item.done` branch still calls this tolerant function rather than exact `JSON.parse` ([final branch](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/api/openai-responses-shared.ts#L706-L722)).

**Fact.** Before execution, Pi finds the local tool and validates/coerces arguments against its TypeBox or JSON Schema, producing path-specific validation errors ([validator](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/utils/validation.ts#L263-L316)). The agent calls optional argument preparation, schema validation, and `beforeToolCall` before returning a prepared executable call; any error becomes an immediate error tool result ([execution preparation](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/agent/src/agent-loop.ts#L586-L665)).

**Nexa inference.** Copy the split, not the exact parser: permissive parsing is acceptable for preview only. At `output_item.done`, retain the exact raw string, require an exact object parse, then schema-validate. A repaired or partial object must never be executable merely because it satisfies a permissive schema after missing fields were dropped.

### Hermes: strict dispatch boundary conflicts with earlier repair

**Fact.** In the Chat-Completions stream, Hermes concatenates argument fragments. At stream end it first tries `json.loads`; on failure it attempts repair and can replace malformed arguments with a repaired string. Unrepairable or empty-without-finish-reason calls are marked as truncated/mid-tool drops ([assembly finalization](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/agent/chat_completion_helpers.py#L3616-L3718)).

**Fact.** The dispatch boundary itself is fail-closed for syntax: `_parse_tool_arguments` uses `json.loads`, accepts only a dictionary, performs no repair/coercion, and returns a structured “tool was not executed” error otherwise ([dispatch parser](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/agent/tool_executor.py#L141-L157)). Both concurrent and sequential paths call this before dispatch ([concurrent path](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/agent/tool_executor.py#L807-L829), [sequential path](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/agent/tool_executor.py#L1653-L1683)). Tests cover malformed, scalar, list, empty, and truncated input and verify that a valid sibling call still executes ([malformed-argument test](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/tests/run_agent/test_malformed_tool_arguments.py#L49-L97)).

**Fact.** No universal tool-schema validation step equivalent to Pi's `validateToolArguments` was found in the reviewed generic Hermes dispatch path; individual tools and special bridges can validate their own inputs.

**Nexa inference.** The strict dispatch parser is worth copying, but upstream repair before that boundary weakens provenance: the executor can no longer distinguish model-authored valid JSON from runtime-repaired JSON. Nexa should preserve raw authoritative bytes and prohibit repair on the executable path.

## 4. Ordering, continuity, and duplicate projection

**Fact.** The official Responses event schema includes `sequence_number`. Pi keys simultaneous outputs by `output_index`, Codex consumes transport order, and Hermes consumes iterator order. Searches of the reviewed Responses paths found no monotonic `sequence_number` check, no gap buffer, and no duplicate-event suppression keyed by sequence.

**Fact.** Pi's `outputSlots` map prevents cross-item argument concatenation when multiple output indexes are active ([slot map](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/api/openai-responses-shared.ts#L423-L461)). Hermes' Chat-Completions path additionally detects providers that reuse a raw tool index with a different call ID and redirects the new call to a fresh slot ([index/ID handling](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/agent/chat_completion_helpers.py#L3492-L3543)).

**Nexa inference.** A canonical provider event should carry at least `(run_id, round_id, attempt_id, provider_response_id, sequence_number, output_index, item_id)`. The projection rule should be:

```text
sequence == expected       -> apply once, advance
sequence > expected        -> buffer; start bounded gap recovery/timeout
sequence < expected        -> duplicate/stale; ignore with diagnostics
different attempt/run      -> reject before projection
```

`output_index` solves item multiplexing; it does not solve missing or duplicated transport events.

## 5. Backpressure, batching, rendering, and persistence

### Codex

**Fact.** Codex places SSE results in a bounded Tokio MPSC channel of capacity 1600, and every send is awaited. Once full, the decoder task is backpressured by its consumer ([bounded channel](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/codex-api/src/sse/responses.rs#L50-L84), [awaited sends](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/codex-api/src/sse/responses.rs#L572-L617)). The capacity absorbs bursts but is not a substitute for UI coalescing.

**Fact.** Text deltas are projected as client events, while completed response items are the persistence boundary. A completed tool item is recorded before its future is queued; completed non-tool items are finalized and recorded once ([delta projection](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/core/src/session/turn.rs#L2550-L2599), [completed-item persistence](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/core/src/stream_events_utils.rs#L288-L357)).

**Fact.** Codex TUI uses a dedicated frame-scheduler task. Frame requests are coalesced into one draw, and a pure rate limiter caps redraw notifications at 120 FPS (about 8.33 ms minimum interval) ([frame scheduler](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/tui/src/tui/frame_requester.rs#L1-L127), [rate limiter](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/tui/src/tui/frame_rate_limiter.rs#L1-L37)). The request channel itself is unbounded, but the scheduler collapses pending deadlines before drawing.

### Pi

**Fact.** Pi's generic `EventStream` uses an unbounded in-memory array when no consumer is waiting; `push` is synchronous and has no maximum length ([event stream](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/utils/event-stream.ts#L3-L67)). That is not transport backpressure and can grow if a consumer is slow.

**Fact.** Pi mutates one partial assistant-message projection as deltas arrive and emits `message_update`, but session persistence occurs only on `message_end`, where the final message is appended once ([partial projection](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/agent/src/agent-loop.ts#L277-L371), [persistence boundary](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/coding-agent/src/core/agent-session.ts#L633-L658)).

**Fact.** Pi TUI coalesces repeated render requests and enforces a 16 ms minimum interval, while forced/user-input rendering bypasses the throttle to preserve interaction latency ([render state](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/tui/src/tui.ts#L335-L344), [scheduler](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/tui/src/tui.ts#L757-L816)).

### Hermes

**Fact.** Hermes' model-delta path invokes registered display/TTS callbacks synchronously per accepted delta and fences callbacks from superseded stream writers ([delta callback](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/run_agent.py#L6467-L6523)). The interactive CLI separately throttles background invalidation (250 ms default), uses a 150 ms invalidation cadence while waiting for an agent thread so `StdoutProxy` flushes, and bypasses the throttle for user-blocking modal prompts ([invalidation contract](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/cli.py#L4857-L4889), [agent-thread flush cadence](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/cli.py#L14347-L14357)).

**Fact.** No bounded model-event queue was found in that reviewed Hermes path. This is a scoped negative finding, not a repository-wide claim.

**Nexa inference.** Backpressure, projection, persistence, and rendering need independent budgets:

- transport decoder: bounded event/byte queue with awaited send;
- semantic projector: in-memory per-item accumulation, bounded by item count and bytes;
- durable store: append authoritative item/terminal/tool milestones, never every token;
- renderer: 16–33 ms coalescing is a practical desktop target; terminal/user-input/tool-state transitions can force an immediate frame;
- diagnostics: count queue depth, coalesced events, dropped stale attempts, gap waits, render frames, and durable writes separately.

An overly coarse 150–250 ms renderer can look visibly “chunky”; an unbounded event queue can look smooth until memory/GC pressure causes stalls. Both must be measured independently.

## 6. Disconnect recovery, idempotency, retry, and fallback ownership

### Codex

**Fact.** EOF before `response.completed` is a stream error, malformed JSON SSE frames are logged and skipped, and a parsed `Completed` event is sent before the decoder returns ([SSE loop](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/codex-api/src/sse/responses.rs#L514-L624)). An integration test proves that early EOF can be retried under the configured stream budget ([early-EOF regression](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/core/tests/suite/stream_no_completed.rs#L20-L105)). The client chooses Responses WebSocket or HTTP under one wire API and falls back to HTTP when the WebSocket path is unhealthy ([transport selection](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/core/src/client.rs#L1847-L1906)).

**Nexa inference.** Skipping malformed SSE JSON without creating a protocol-gap error is unsafe when the skipped frame could be a tool argument or terminal. Also, an early-EOF retry test alone is not evidence that replay after visible output is idempotent.

### Pi

**Fact.** Pi disables hidden OpenAI SDK retries with `maxRetries: 0` and wraps request establishment in one bounded, abortable helper ([Responses request](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/api/openai-responses.ts#L129-L165)). The helper owns status classification, `Retry-After`, exponential jitter, attempt count, and abortable sleep ([provider retry owner](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/utils/provider-retry.ts#L22-L124)). Its higher-level assistant retry restarts only results classified with `stopReason === "error"` under a finite policy ([assistant retry](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/utils/retry.ts#L145-L211)).

**Nexa inference.** The explicit single retry owner is strong. The higher-level retry helper does not itself inspect whether deltas were already shown, so it is not sufficient evidence for safe transparent replay after visible output.

### Hermes

**Fact.** Hermes records whether deltas were delivered. Its default is not to retry after partial delivery because replay duplicates visible text. It has a narrow transient mid-tool exception before tool execution, emits a reconnect marker, resets attempt buffers, and starts another bounded attempt ([partial-delivery policy](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/agent/chat_completion_helpers.py#L4025-L4148)). It gives every stream attempt a monotonically increasing ID and discards chunks from cancelled/superseded attempts ([attempt fencing](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/agent/chat_completion_helpers.py#L3104-L3198)). A second single-writer token prevents a previous attempt from interleaving late deltas into the active result ([writer fence](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/run_agent.py#L6392-L6465)).

**Fact.** After a semantic terminal has already been assembled, a drain-time transport failure is only a warning; Hermes returns the completed response and does not open another physical request. A test verifies exactly one physical request ([runtime](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/agent/codex_runtime.py#L1398-L1426), [regression](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/tests/run_agent/test_run_agent_codex_responses.py#L611-L675)).

**Nexa inference.** Nexa's only transparent retry owner should require all of:

```text
semantic_terminal_seen == false
visible_output_seen == false
durable_tool_milestone_seen == false
local_side_effect_started == false
retryable_transport_class == true
attempt_budget_remaining == true
```

After any visible output, return a typed partial/interrupted state or use a provider-native resume mechanism that proves continuity of the same response. A new physical request is not idempotent merely because no local tool executed: it can duplicate text, billing, hosted-provider tools, or remote side effects.

## 7. Provider adapter layering

### Codex

**Fact.** Codex's core selects by provider `wire_api`, then chooses WebSocket Responses or HTTP Responses within that protocol ([client selection](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/core/src/client.rs#L1847-L1906)). The TypeScript SDK is a process wrapper: `runStreamed` launches the CLI and parses its JSONL events; it is not an independent provider-protocol implementation ([SDK README](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/sdk/typescript/README.md#L1-L48), [thread stream](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/sdk/typescript/src/thread.ts#L65-L110)).

**Nexa inference.** SDK/application consumers should receive canonical turn events, not raw provider frames. One process should own provider semantics.

### Pi

**Fact.** Pi separates provider catalog/auth/base URL from API implementation. `openaiProvider()` supplies provider identity, models, auth, base URL, and the lazily loaded Responses API codec ([OpenAI provider](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/providers/openai.ts#L1-L15)); the shared provider registry composes many provider definitions without adding their wire parsing to the agent loop ([provider registry](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/providers/all.ts#L1-L45)).

**Nexa inference.** Provider identity/catalog, API codec, agent loop, and renderer are distinct seams. “OpenAI-compatible” should select a tested protocol profile, not enable a growing matrix of unrelated flags inside the turn loop.

### Hermes

**Fact.** Hermes separates declarative `ProviderProfile` metadata/quirks from wire `ProviderTransport`. Profiles explicitly do not own client construction, credential rotation, or streaming ([profile contract](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/providers/base.py#L1-L10)); transports own conversion/normalization but not retry/streaming ([transport contract](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/agent/transports/base.py#L1-L73)). A registry discovers transport implementations by `api_mode`, while allowing a `None` result for gradual migration to a legacy path ([registry](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/agent/transports/__init__.py#L17-L68)).

**Nexa inference.** The profile/transport split is excellent. The gradual legacy fallback is technical debt by design; Nexa should avoid retaining both a formal adapter and an undocumented legacy switch path for the same model/provider after conformance is proven.

## 8. Error classification and user-visible semantics

### Codex

**Fact.** Codex maps `response.failed` into context-window, quota, usage-not-included, cyber-policy, invalid-request, overloaded, or retryable errors; `response.incomplete` becomes a stream error carrying the incomplete reason; malformed completed payloads become stream errors ([terminal classification](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/codex-api/src/sse/responses.rs#L390-L451)). Malformed nonterminal SSE JSON is debug-logged and skipped ([SSE parse branch](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/codex-api/src/sse/responses.rs#L553-L566)).

### Pi

**Fact.** Pi rejects EOF without a terminal, throws the provider failure code/message for `response.failed`, and maps incomplete max-output-tokens to `length` while other incomplete reasons become errors ([terminal handling](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/api/openai-responses-shared.ts#L738-L782)). Its wrapper emits a final error message with `stopReason: "error"` when the protocol parser rejects early EOF; tests assert the user-visible text ([terminal tests](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/test/openai-responses-terminal-event.test.ts#L206-L239)).

### Hermes

**Fact.** Hermes centralizes a broad error taxonomy—auth, billing, rate limit, upstream rate limit, overload, server, timeout, certificate, context/payload, policy, format, replay state, provider-specific cases—and attaches recovery actions such as retry, rotate, fallback, compress, or abort ([taxonomy](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/agent/error_classifier.py#L1-L72)). Its tool parser produces a structured model-visible `invalid_tool_arguments` result instead of executing or crashing ([tool error path](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/agent/tool_executor.py#L1653-L1683)).

**Nexa inference.** Nexa should expose at least these orthogonal dimensions rather than one nested `LLM error` string:

| Dimension | Examples | User-visible meaning |
| --- | --- | --- |
| Transport | connect timeout, EOF, malformed SSE | connection interrupted; whether any partial output was kept |
| Protocol integrity | sequence gap, unknown terminal, item done missing, invalid completed item | provider returned an incompatible/incomplete stream; no tool executed |
| Provider terminal | failed, incomplete/length, quota, policy | provider-declared reason and recommended action |
| Tool arguments | invalid JSON, non-object, schema mismatch, unknown tool | call rejected before execution; exact field/path when safe |
| Side-effect certainty | not started, started/unknown, completed | whether automatic retry is safe |
| Projection/UI | stale attempt, duplicate event, renderer backlog | diagnostics; never reclassify semantic success as failure |

The message should name the failed layer once. `LLM error: LLM error:` is itself evidence that error wrapping lacks a canonical typed boundary.

## 9. Tests, fixtures, stress, and fuzzing

### What upstreams actually test

- **Codex facts:** deterministic SSE unit tests cover event projection, ignored function-argument deltas, and completion; an integration server covers close-before-completed retry ([decoder test](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/codex-api/src/sse/responses.rs#L956-L995), [early EOF](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/core/tests/suite/stream_no_completed.rs#L20-L105)). The frame limiter has isolated timing tests ([frame tests](https://github.com/openai/codex/blob/89a335ed50258dc9dc5b3d7f410db61b431244f9/codex-rs/tui/src/tui/frame_rate_limiter.rs#L39-L61)).
- **Pi facts:** deterministic fixtures cover missing terminal, terminal reason precedence, partial-JSON scratch cleanup, and malformed/truncated tool handling ([terminal tests](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/test/openai-responses-terminal-event.test.ts#L206-L293), [argument cleanup](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/test/openai-responses-partial-json-cleanup.test.ts#L27-L104), [length gate](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/agent/src/agent-loop.ts#L374-L405)).
- **Hermes facts:** regressions cover missing Responses terminal (permissive behavior), post-terminal drain failure, malformed argument rejection in concurrent/sequential execution, partial-stream finish reasons, and stale-attempt fencing ([Responses tests](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/tests/run_agent/test_run_agent_codex_responses.py#L411-L444), [post-terminal test](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/tests/run_agent/test_run_agent_codex_responses.py#L611-L675), [malformed arguments](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/tests/run_agent/test_malformed_tool_arguments.py#L49-L97)). Hermes has randomized property testing elsewhere, but the reviewed property-fuzz file targets Kanban database invariants, not provider streaming ([property test scope](https://github.com/NousResearch/hermes-agent/blob/8359e760be499fd8e804242e7606d81dde931abb/tests/stress/test_property_fuzzing.py#L1-L35)).

**Scoped negative finding.** No protocol-specific property/fuzz target for Responses event ordering, arbitrary SSE/chunk boundaries, or incremental tool JSON was found in the reviewed Codex, Pi, or Hermes paths. This does not assert that no such test exists anywhere outside the examined directories/history.

### Required Nexa conformance matrix

The minimum useful suite should combine golden fixtures, chunk-split property tests, randomized event mutations, and renderer/load tests:

1. Split every UTF-8 code point and every JSON token boundary across arbitrary network chunks; canonical events must be identical to the unsplit fixture.
2. Split function arguments at every byte boundary; preview may vary, final exact JSON and schema result may not.
3. Delete, duplicate, reorder, and delay each `sequence_number`; assert buffering, timeout, duplicate suppression, and no side effect across a gap.
4. Interleave two `output_index`/`item_id` tool calls; arguments and completion events must never cross.
5. Deliver `.done` with arguments different from accumulated deltas; authoritative done wins for preview reconciliation, then exact parse/schema decides executability.
6. EOF before terminal with zero visible output: one bounded retry under a new attempt ID.
7. EOF before terminal after one visible byte: no transparent new physical request.
8. Terminal then EOF/error/stale chunk: exactly one terminal, one persisted result, zero retry/fallback.
9. Malformed SSE frame at sequence N: protocol-integrity error, not silent skip past N.
10. Invalid JSON/object/schema/unknown tool: structured error result and zero tool invocation.
11. A 100k-delta synthetic stream: bounded memory, bounded durable writes, stable CPU, renderer frames capped, final content exact.
12. Slow renderer/consumer: transport queue applies backpressure without losing semantic milestones.
13. Cancel/supersede attempt A while A continues sending; only B can mutate UI or durable state.
14. Restart after completed call persisted but before result: replay identifies the obligation once and does not execute an uncertain side effect twice.

For every randomized failure, print the seed and minimized raw event sequence. Preserve provider-specific real-world fixtures separately from generated protocol fixtures.

## 10. Deletion tests for Nexa's audit

This section defines tests for deciding what can be deleted. It does **not** claim that a named Nexa component is currently redundant; that requires the parent audit to map these roles to actual files and call graphs.

| Candidate role | Deletion test | Keep only if deletion proves |
| --- | --- | --- |
| Hidden SDK retry layer | Set SDK retries to zero/remove wrapper; run connect/429/5xx/EOF/visible-output matrix | The SDK owns a capability the explicit retry owner cannot represent. Otherwise delete it. |
| Second retry/fallback state machine | Disable one owner and record physical request count, attempts, visible bytes, terminal count | Only one owner can satisfy every supported route. Delete/merge the other. |
| Duplicate Responses argument assembler | Remove the assembler outside the wire codec; replay interleaved delta/done fixtures | A distinct consumer needs a different canonical representation. Otherwise delete it. |
| Permissive JSON repair on executable path | Remove repair and run provider fixtures plus invalid/truncated cases | A formally documented provider emits recoverable malformed arguments and the product explicitly accepts semantic mutation. Otherwise delete repair; keep preview repair isolated. |
| Generic `OpenAI-compatible` flag matrix | Route each provider through a named conformance profile; remove a flag branch one at a time | The branch corresponds to a tested wire difference, not historical folklore. Otherwise delete it. |
| Legacy provider path behind a formal adapter | Turn off legacy fallback and run all models mapped to the adapter | A provider lacks adapter parity. If parity holds, delete the legacy path. |
| Per-token persistence writer | Disable it; crash at every item/terminal/tool boundary and replay | Durable correctness genuinely requires token granularity. Otherwise persist milestones only and delete per-token writes. |
| Extra UI projection hop/store | Bypass it and compare canonical event sequence, final UI, restart replay, and frame count | It owns a necessary normalization or durable boundary. Otherwise delete it. |
| Terminal aggregate re-emitter | Remove full-text terminal emission; reconcile against streamed prefix | A provider never streams deltas for that route. Otherwise emit suffix only or delete duplicate projection. |
| Tool dispatch fallback accepting `{}`/repaired args | Remove fallback; run missing/invalid/schema cases | A tool explicitly defines empty object as valid and the provider delivered exact `{}`. Otherwise delete the coercion. |
| Post-visible automatic retry | Disable it; simulate all transport failures after first visible byte | A provider-native resume proves same-response continuity and idempotency. Otherwise delete automatic replay. |
| Error-string wrapper layer | Remove one wrapper and snapshot typed error code, cause chain, and user message | It adds a distinct typed context. If it only prefixes `LLM error`, delete it. |

The decisive metric is not line count. A component is deletable when removing it reduces the number of semantic owners without losing a tested contract. Compatibility code that survives must be named, provider-scoped, measurable, and covered by a fixture.

## 11. Recommended Nexa target architecture

```text
ProviderProfile
  identity, endpoint, auth, advertised capabilities, tested quirks
        |
WireCodec (Responses / Chat Completions / Anthropic / ...)
  bytes -> canonical ordered events
  owns exact protocol terminal + per-item assembly + sequence continuity
        |
AttemptController
  one physical request at a time
  owns bounded retry/fallback before visible output only
  fences stale attempts
        |
TurnProjector
  ephemeral text/reasoning/tool previews
  coalesced client updates; milestone persistence only
        |
ToolRuntime
  completed raw call -> exact JSON -> schema -> approval -> execute once
        |
AgentLoop
  completed assistant result + tool results -> stop or next semantic round
        |
Renderer
  frame coalescing independent of provider chunk cadence
```

Required ownership rules:

1. One codec owns argument assembly per wire protocol.
2. One attempt controller owns retry and fallback.
3. One typed terminal latch owns semantic completion per physical response.
4. One tool runtime owns local execution and execution idempotency.
5. One durable projector owns canonical milestone persistence.
6. UI rendering consumes canonical projections and cannot reopen provider state.
7. Provider quirks are data/profile entries unless they truly require a different codec.

## Final assessment

The most important lesson from Codex, Pi, and Hermes is not “copy this repository.” It is to make completion authority and ownership explicit.

- Codex demonstrates that a completed output item can be the only function-call argument authority and that completed items, not token deltas, are the durable/tool boundary.
- Pi demonstrates that incremental previews, final messages, schema validation, persistence, and rendering can be separate stages.
- Hermes demonstrates that stale attempts and post-terminal drain errors need fencing and terminal-aware retry policy, while also showing why permissive missing-terminal or repaired-argument behavior should not be copied.

For Nexa, `incomplete arguments` must become a typed protocol/tool-validation outcome with zero side effects, not an exception discovered after dispatch. Streaming “卡顿” must be measured as four separate queues—network bytes, canonical events, durable writes, and render frames—because adjusting one throttle cannot repair a broken terminal or tool state machine.
