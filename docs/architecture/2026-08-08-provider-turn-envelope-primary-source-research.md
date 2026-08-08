# Provider-native Turn Envelopes for Reasoning and Tool Replay: Primary-source Research

Date: 2026-08-08
Status: implementation input for the Provider Turn Envelope PR
Scope: DeepSeek, Anthropic, OpenAI Responses, and Gemini provider-native reasoning/tool-call replay

## Purpose and boundary

Nexa currently has a useful safety boundary around replaying reasoning, but its durable representation still collapses provider state to a generic string. This note establishes the provider-native wire units that must be captured, validated, and persisted before tool execution.

The companion note, [Companion Reasoning Graph: Primary-source Research](./2026-08-08-companion-reasoning-graph-primary-source-research.md), already establishes the DeepSeek Chat Completions baseline and the rule that unknown or custom endpoints must not inherit a trusted provider dialect. This document does not repeat that research. It extends it across native Anthropic Messages, native OpenAI Responses, Gemini `generateContent`, and DeepSeek's distinct compatibility surfaces.

Only first-party API documentation, first-party SDK source, and their licenses are used below. SDK source links are pinned to revisions observed on 2026-08-08:

| Project | Pinned revision | License relevant to source reuse |
| --- | --- | --- |
| OpenAI Python SDK | [`0c09a3f`](https://github.com/openai/openai-python/tree/0c09a3fe815184f0a46fbf18b1aba84a467c854e) | [Apache-2.0](https://github.com/openai/openai-python/blob/0c09a3fe815184f0a46fbf18b1aba84a467c854e/LICENSE) |
| Anthropic Python SDK | [`009b035`](https://github.com/anthropics/anthropic-sdk-python/tree/009b035305e0724ce108ebd796935f91711fc6e1) | [MIT](https://github.com/anthropics/anthropic-sdk-python/blob/009b035305e0724ce108ebd796935f91711fc6e1/LICENSE) |
| Google Gen AI Python SDK | [`66e224c`](https://github.com/googleapis/python-genai/tree/66e224c39c9527e0fef3a4f049ac33ec941e2f99) | [Apache-2.0](https://github.com/googleapis/python-genai/blob/66e224c39c9527e0fef3a4f049ac33ec941e2f99/LICENSE) |
| DeepSeek API documentation | live official documentation checked 2026-08-08 | protocol reference; no SDK source is copied by this proposal |

The implementation should remain a clean-room mapping of documented wire contracts. License notices become relevant only if SDK code is copied rather than used as protocol evidence.

## Executive decisions

1. **There is no safe provider-neutral reasoning string.** The replay unit is a DeepSeek assistant message tuple, an ordered Anthropic content-block list, a sequence of OpenAI Responses items, or an ordered Gemini `Content.parts` list.
2. **Route identity is protocol identity.** `provider_family + api_style + API version + endpoint identity + model/profile capability revision` selects the replay decoder. Provider name alone does not.
3. **DeepSeek's three documented API styles are separate dialects.** OpenAI Chat Completions uses `reasoning_content`; the Anthropic-compatible endpoint uses Anthropic blocks; the Responses-compatible endpoint uses Responses items but intentionally differs from OpenAI's native state and encrypted-reasoning semantics.
4. **Signed, encrypted, and opaque data is replay state, not display reasoning.** Anthropic `signature`/`redacted_thinking.data`, OpenAI `encrypted_content`, and Gemini `thoughtSignature` must be stored losslessly, never reconstructed from a summary, and excluded from normal logs/UI.
5. **Streaming is an assembly transport, not another durable shape.** A stream becomes replayable only when its provider-defined blocks/items/parts and tool arguments are complete. A non-stream retry is a new `sample_id`; its content must never be merged with a partial stream.
6. **The assistant payload and tool links form one validation unit.** Nexa must validate and persist them atomically before any tool side effect is allowed.
7. **Missing required native replay state fails closed.** Do not invent signatures, encrypted fields, reasoning text, IDs, or sentinel blocks. Mark the sample non-replayable and start a new provider turn only from a safe pre-tool boundary.
8. **Legacy strings are not generally migratable.** A string may be treated as DeepSeek `reasoning_content` only when the historical route is positively identified as that exact Chat Completions dialect. Otherwise insert a replay boundary.

## Current Nexa seam

The present model is intentionally narrower than the provider contracts:

- `crates/core/src/llm/mod.rs` stores `Message.reasoning_content: Option<String>` and a single `ToolCallRequest.thought_signature: Option<String>`.
- `crates/core/src/agent/assistant_turn.rs` writes the durable reasoning envelope as `serde_json::Value::String`.
- `crates/core/src/conversation/mod.rs` reads that payload with `as_str()`.
- `crates/core/src/llm/reasoning_replay.rs` correctly omits an unsafe assistant/tool replay chain when required content is absent, but it can only project a provider-neutral string.

That shape cannot losslessly represent ordered Anthropic blocks, OpenAI response items, Gemini signed parts, or DeepSeek Responses items. It also places Gemini's signature on a normalized tool call rather than preserving the signed part position.

## Route identity is part of the envelope

| Provider surface | Canonical request identity | Native assistant/reasoning replay shape | Tool result correlation | Route distinction that Nexa must retain |
| --- | --- | --- | --- | --- |
| DeepSeek OpenAI-compatible Chat Completions | `POST /chat/completions`, official base URL `https://api.deepseek.com` | assistant message containing `content`, complete `reasoning_content`, and `tool_calls` | tool message `tool_call_id` -> assistant tool call `id` | `deepseek + chat_completions`; exact trusted endpoint/capability profile |
| DeepSeek Anthropic-compatible Messages | official base URL `https://api.deepseek.com/anthropic` through Anthropic SDK/format | Anthropic ordered content blocks | Anthropic `tool_use.id` -> `tool_result.tool_use_id` | separate from DeepSeek Chat and native Anthropic endpoint identity |
| DeepSeek Responses-compatible API | official base URL `https://api.deepseek.com` | typed Responses-style `reasoning`, `function_call`, and other items | `function_call.call_id` -> `function_call_output.call_id` | `deepseek + responses_compat`; not OpenAI-native Responses semantics |
| Anthropic Messages API | `POST /v1/messages` with required `anthropic-version` | ordered `thinking`, `redacted_thinking`, text, and `tool_use` blocks | `tool_use.id` -> following user `tool_result.tool_use_id` | endpoint/backend, Messages dialect, version header, beta/features, model |
| OpenAI native Responses API | `POST https://api.openai.com/v1/responses` | ordered typed output/input items, including opaque `reasoning` items | `function_call.call_id` -> `function_call_output.call_id` | native Responses vs Chat Completions/compatibility routes; stored vs manual state |
| Gemini Developer API `generateContent` | `POST https://generativelanguage.googleapis.com/v1beta/{model=models/*}:generateContent` | ordered model `Content.parts`, preserving `thoughtSignature` on its exact part | optional call/response `id`, plus name and ordered parts | Developer API vs Vertex/backend; `v1beta generateContent` vs any later Interactions surface |

Sources: [DeepSeek Chat Completions reference](https://api-docs.deepseek.com/api/create-chat-completion), [DeepSeek Anthropic format](https://api-docs.deepseek.com/guides/anthropic_api), [DeepSeek Responses format](https://api-docs.deepseek.com/guides/responses_api/), [Anthropic authentication and Messages route](https://platform.claude.com/docs/en/manage-claude/authentication), [Anthropic versioning](https://platform.claude.com/docs/en/api/versioning), [OpenAI Responses migration guide](https://developers.openai.com/api/docs/guides/migrate-to-responses), and [Gemini `generateContent` API](https://ai.google.dev/api/generate-content).

`RouteSnapshot` should therefore include at least:

- stable provider endpoint ID and a credential-free effective endpoint identity;
- `provider_family`, explicit `api_style`, and API/version surface;
- model ID, reasoning profile ID/version, and provider capability revision;
- request-affecting feature/beta set or a stable digest of it;
- state mode where applicable, for example OpenAI stored response, Conversations state, or stateless/manual items.

Unknown endpoints, edited base URLs, routers, and non-standard ports must use an explicit custom capability record. They must not inherit any trusted replay codec based on a provider label or model-name substring.

## DeepSeek

### OpenAI-compatible Chat Completions

DeepSeek's official thinking-mode guide returns `reasoning_content` beside `content`. In a tool loop, the official example appends the complete assistant message and then appends tool messages, preserving the assistant tuple `{content, reasoning_content, tool_calls}`. When `tools` are present, DeepSeek requires complete `reasoning_content` in subsequent requests; omitting it produces HTTP 400. See [Thinking Mode](https://api-docs.deepseek.com/guides/thinking_mode/) and the [Chat Completions schema](https://api-docs.deepseek.com/api/create-chat-completion).

Consequences:

- A `DeepSeekReasoningContent(String)` variant alone is still underspecified. The replay payload must retain the whole assistant message tuple, including tool call IDs, names, and exact arguments.
- Tool results are `role: "tool"` messages correlated by `tool_call_id`.
- The reasoning string may be shown separately only under product policy; its durable replay copy must not be normalized or reconstructed from the UI text.

For streaming, the official guide assembles `delta.reasoning_content` and `delta.content`; Chat Completions uses data-only SSE and terminates with `data: [DONE]`. Nexa should additionally require a terminal choice/finish state and complete tool-call arguments before accepting a replayable sample. An EOF before those boundaries is an abandoned partial sample, not a candidate for merging with a non-stream retry. See [DeepSeek streaming fields](https://api-docs.deepseek.com/guides/thinking_mode/) and [streaming response format](https://api-docs.deepseek.com/api/create-chat-completion).

### Anthropic-compatible endpoint

DeepSeek documents a distinct Anthropic-compatible base URL, `https://api.deepseek.com/anthropic`, using Anthropic SDK and message format. The route must therefore select the Anthropic block codec described below; it must not project an OpenAI-style `reasoning_content` field merely because the provider family is DeepSeek. See [Using the Anthropic API Format](https://api-docs.deepseek.com/guides/anthropic_api).

### Responses-compatible endpoint

DeepSeek also documents a Responses-compatible API. As checked on 2026-08-08, the official compatibility page says:

- only `deepseek-v4-flash` is currently supported on this surface; support for `deepseek-v4-pro` is described as planned, so Nexa must use live route capabilities rather than assume it;
- `previous_response_id`, `conversation`, and stored state are unsupported; `store` is unsupported/always false;
- an input `reasoning` item is supported as plain-text `content`, while `summary` and `encrypted_content` are unsupported;
- function-call and function-call-output items are supported;
- unsupported parameters can be silently ignored;
- semantic streaming events include output-item, reasoning-text, and function-call-argument delta/done events, and terminate with `response.completed`, `response.incomplete`, or `response.failed`, without Chat Completions' `[DONE]` marker.

These are first-party compatibility guarantees from [DeepSeek Responses API](https://api-docs.deepseek.com/guides/responses_api/). They prove that “Responses” is not itself a complete replay identity. DeepSeek Responses and OpenAI native Responses should have different payload variants/capability revisions even if some item names match.

Safe missing-state policy: if DeepSeek Chat requires reasoning replay and the exact assistant tuple is absent, do not execute its tool calls. If a DeepSeek Responses item sequence is incomplete, do not synthesize OpenAI `encrypted_content`, a prior response ID, or a summary; mark it non-replayable and restart from a safe pre-tool boundary.

## Anthropic Messages

Anthropic's replay unit is the complete ordered assistant `content` block list. The native types include:

- `thinking`: `{type, thinking, signature}`; the signature is opaque/encrypted;
- `redacted_thinking`: `{type, data}`; the data is opaque/encrypted;
- `tool_use`: `{type, id, name, input}`.

The first-party SDK definitions confirm the exact fields: [`ThinkingBlock`](https://github.com/anthropics/anthropic-sdk-python/blob/009b035305e0724ce108ebd796935f91711fc6e1/src/anthropic/types/thinking_block.py), [`RedactedThinkingBlock`](https://github.com/anthropics/anthropic-sdk-python/blob/009b035305e0724ce108ebd796935f91711fc6e1/src/anthropic/types/redacted_thinking_block.py), and [`ToolUseBlock`](https://github.com/anthropics/anthropic-sdk-python/blob/009b035305e0724ce108ebd796935f91711fc6e1/src/anthropic/types/tool_use_block.py).

During tool use, Anthropic requires the complete sequence of `thinking` and `redacted_thinking` blocks from the last assistant message to be passed back unchanged and in order. Tool use is a pause within the same assistant response; thinking cannot be toggled midway through that assistant/tool loop. If thinking display is omitted, the visible thinking text can be empty while the signature still carries encrypted reasoning state. See [Extended thinking with tool use](https://platform.claude.com/docs/en/build-with-claude/extended-thinking) and [tool-use troubleshooting](https://platform.claude.com/docs/en/agents-and-tools/tool-use/troubleshooting-tool-use).

Tool results are user-message `tool_result` blocks. Each `tool_result.tool_use_id` references a prior assistant `tool_use.id`; results must immediately follow the tool-use assistant message and precede other user content. The SDK parameter type is [`ToolResultBlockParam`](https://github.com/anthropics/anthropic-sdk-python/blob/009b035305e0724ce108ebd796935f91711fc6e1/src/anthropic/types/tool_result_block_param.py), and the ordering rules are in [Handle tool calls](https://platform.claude.com/docs/en/agents-and-tools/tool-use/handle-tool-calls).

Streaming preserves the same final message shape:

1. `message_start` opens the message.
2. Each indexed block has `content_block_start`, one or more deltas, and `content_block_stop`.
3. Tool input is a partial JSON string until its block stops.
4. Thinking uses `thinking_delta`; `signature_delta` arrives immediately before its block stops. With thinking display omitted, a signature may arrive without thinking deltas.
5. `message_delta` and `message_stop` close the message.

Anthropic's SDK accumulator yields the same completed object as the non-stream call. The official recovery rules say thinking and tool-use blocks are not partially recoverable, so Nexa must not dispatch a tool from a truncated `input_json_delta` or persist a signature before its block is complete. See [Streaming Messages](https://platform.claude.com/docs/en/build-with-claude/streaming).

Safe missing-state policy: a missing, modified, or reordered `thinking`/`redacted_thinking` block makes the same tool loop non-replayable. Preserve the original blocks byte-for-byte in their original order; do not replace them with display text, an empty signature, or a generic reasoning string.

## OpenAI native Responses

The Responses API uses typed Items rather than Chat Completions messages. The durable unit for a reasoning/tool loop is the ordered sequence of returned reasoning and function-call items plus function-call-output inputs. OpenAI's official guides require reasoning items returned with function calls to be passed back along with the tool outputs. With multiple consecutive calls, all reasoning, function-call, and function-call-output items since the last user message must remain intact. See [Reasoning models](https://developers.openai.com/api/docs/guides/reasoning), [Function calling](https://developers.openai.com/api/docs/guides/function-calling), and [Migrating to Responses](https://developers.openai.com/api/docs/guides/migrate-to-responses).

The official SDK types make the wire boundaries concrete:

- [`ResponseReasoningItem`](https://github.com/openai/openai-python/blob/0c09a3fe815184f0a46fbf18b1aba84a467c854e/src/openai/types/responses/response_reasoning_item.py) carries `id`, `summary`, optional `content`, optional opaque `encrypted_content`, and a completion status;
- [`ResponseFunctionToolCall`](https://github.com/openai/openai-python/blob/0c09a3fe815184f0a46fbf18b1aba84a467c854e/src/openai/types/responses/response_function_tool_call.py) carries `id`, `call_id`, name, arguments, and status;
- [`ResponseFunctionCallOutputItemParam`](https://github.com/openai/openai-python/blob/0c09a3fe815184f0a46fbf18b1aba84a467c854e/src/openai/types/responses/response_function_call_output_item_param.py) links the output by `call_id`.

`summary` is display/audit content and is not a substitute for opaque replay state. OpenAI supports three materially different continuation modes:

- a stored response referenced by `previous_response_id`;
- a durable Conversations object;
- manual item replay, including stateless/Zero Data Retention cases. In stateless mode the reasoning item can contain `encrypted_content`, which must be replayed unchanged.

See [Conversation state](https://developers.openai.com/api/docs/guides/conversation-state) and the stateless/encrypted-content rules in [Reasoning models](https://developers.openai.com/api/docs/guides/reasoning). Because stored response state has a finite retention period, Nexa should persist the route, response ID, state mode, and native items required by its durability policy instead of treating a response ID as permanent local replay state.

Responses streaming is semantic SSE. Item completion is represented by `response.output_item.done`; function arguments have a dedicated `response.function_call_arguments.done`; the terminal successful response is `response.completed`. The pinned SDK event types are [`ResponseOutputItemDoneEvent`](https://github.com/openai/openai-python/blob/0c09a3fe815184f0a46fbf18b1aba84a467c854e/src/openai/types/responses/response_output_item_done_event.py), [`ResponseFunctionCallArgumentsDoneEvent`](https://github.com/openai/openai-python/blob/0c09a3fe815184f0a46fbf18b1aba84a467c854e/src/openai/types/responses/response_function_call_arguments_done_event.py), and [`ResponseCompletedEvent`](https://github.com/openai/openai-python/blob/0c09a3fe815184f0a46fbf18b1aba84a467c854e/src/openai/types/responses/response_completed_event.py). The event lifecycle is documented in [Streaming Responses](https://developers.openai.com/api/docs/guides/streaming-responses).

Nexa should persist a provider item only at its `done` boundary and authorize tools only after the relevant call item/arguments and the response envelope are validated. Deltas and reasoning summaries can drive UI, but they are not durable replay state by themselves.

Safe missing-state policy: if server-side state is still resolvable, continuing by its exact response/conversation reference is provider-supported. Otherwise, a missing native reasoning item or `encrypted_content` in a stateless tool loop cannot be reconstructed. Do not dispatch the associated tools or synthesize state from the summary; start a new sample from a safe boundary.

## Gemini `generateContent`

Gemini thought signatures are encrypted, opaque metadata attached to a particular `Part`. The official rule is positional: if a signature is returned, send it back exactly in the same part and position. Official SDKs handle this when the full response is appended to history; custom history managers must do the same. See the legacy-`generateContent` [Thought signatures](https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures) guide and the [`generateContent` API](https://ai.google.dev/api/generate-content).

The Gemini 3 rules are especially important:

- a function-calling turn requires a thought signature or the API returns HTTP 400;
- for one function call the signature is on that `functionCall` part;
- for parallel calls, only the first `functionCall` part may carry the signature, and all calls must remain in their original order;
- for sequential calls, the first function call of each model step carries that step's signature;
- without function calls, the signature can be on the last part;
- in a streamed response without function calls, the signature can arrive in an empty-text part near the end, so the full response must be consumed through the finish reason;
- signed and unsigned parts must never be coalesced, and two signed parts must never be merged.

Gemini 2.5 is more permissive about returning signatures, but any signature that is returned still must be preserved. These current model-family rules and examples are all in the first-party legacy-`generateContent` [Thought signatures guide](https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures).

The pinned Google SDK represents the protocol without flattening it:

- [`FunctionCall`](https://github.com/googleapis/python-genai/blob/66e224c39c9527e0fef3a4f049ac33ec941e2f99/google/genai/types.py#L1756-L1780) has optional `id`, name, and arguments;
- [`FunctionResponse`](https://github.com/googleapis/python-genai/blob/66e224c39c9527e0fef3a4f049ac33ec941e2f99/google/genai/types.py#L1958-L1988) can link by optional `id` and retains name/response;
- [`Part`](https://github.com/googleapis/python-genai/blob/66e224c39c9527e0fef3a4f049ac33ec941e2f99/google/genai/types.py#L2208-L2263) retains `thought`, byte-valued `thought_signature`, and the function-call/response union.

Because call IDs are optional, Nexa must preserve the complete ordered parts and all provider-returned IDs; it must not fabricate an ID or rely only on a normalized name. JSON transports encode the signature bytes, but storage should preserve the exact decoded bytes or their exact canonical base64 representation.

`generateContent` and `streamGenerateContent` are distinct transport methods for the same final `Content`/`Part` structure. Streaming chunks are not individually safe history entries. Nexa must assemble every part, retain empty signed parts, observe the terminal finish reason, and only then validate a replayable model turn. See the [GenerateContent API reference](https://ai.google.dev/api/generate-content).

The guide also documents two explicit dummy signatures for deliberately injected, client-authored function-call parts. That escape hatch is for context engineering when no provider-generated signature ever existed; it does **not** authenticate or repair a damaged provider-native turn. Nexa should model such injected calls as a distinct origin/policy, never silently place a dummy value into a captured turn whose real signature was lost.

Safe missing-state policy: if a required provider-generated Gemini 3 signature is missing, moved, or associated with a merged/reordered part, the model/tool turn is non-replayable. Do not execute the function calls, invent a replacement signature, or fall back to an unsigned reconstruction of the same turn.

## Recommended Nexa representation

The envelope should keep a provider-neutral lifecycle around provider-owned replay variants. One possible Rust-level direction is:

```rust
struct ProviderTurnEnvelope {
    schema_version: u32,
    turn_item_id: String,
    sample_id: String,
    route: RouteSnapshot,
    visible_content: Option<String>,
    provider_items: Vec<ProviderReplayItem>,
    replay_payload: ProviderReplayPayload,
    tool_calls: Vec<ProviderToolLink>,
    capture_status: CaptureStatus,
    request_id: Option<String>,
    response_id: Option<String>,
    raw_response_digest: Option<String>,
}

enum ProviderReplayPayload {
    DeepSeekChatCompletions {
        assistant_message: DeepSeekAssistantMessage,
    },
    DeepSeekResponsesCompat {
        items: Vec<DeepSeekResponseItem>,
    },
    AnthropicMessages {
        assistant_blocks: Vec<AnthropicContentBlock>,
    },
    OpenAiResponses {
        state: OpenAiResponseState,
        items: Vec<OpenAiResponseItem>,
    },
    GeminiGenerateContent {
        model_content: GeminiContent,
    },
    None,
}
```

This is deliberately deeper than `DeepSeekReasoningContent(String)`, `AnthropicThinkingBlocks`, or `GeminiThoughtSignatures`: the assistant/tool relationship and ordering live around the opaque field and are part of what providers validate.

Each provider variant should:

- have its own schema version and reject a route/payload variant mismatch;
- preserve item/block/part order and provider-returned identifiers;
- preserve bounded unknown fields needed for forward-compatible replay without allowing arbitrary unbounded response capture;
- separate display-safe reasoning/summary from the exact replay payload;
- treat signatures, encrypted content, and redacted blocks as secret-adjacent values excluded from telemetry, diagnostics, search indexing, and UI serialization;
- carry a digest for integrity diagnostics, but never use a digest as a substitute for the raw replay value.

## Persistence and transaction boundary

Attaching replay JSON only to a visible message is insufficient for multiple samples, retries, and tool-side-effect auditing. The durable model should represent:

- one envelope row per `sample_id`, including immutable route snapshot, capture/validation status, request/response IDs, and digest;
- ordered provider items or one versioned envelope blob whose order is canonical and independently size-bounded;
- tool links containing provider call ID, tool name/arguments, result link, validation state, and a side-effect execution/idempotency record;
- an explicit relation from the accepted sample to the visible conversation turn, without merging rejected samples into it.

The required state transition is:

```text
Receiving -> Assembling -> Complete -> Validated -> Persisted -> ToolDispatchAllowed
                   \-> Abandoned / NonReplayable
```

The accepted envelope, route snapshot, provider items, and tool-call ledger must commit atomically. Tool execution begins only after that commit. A stream failure or a non-stream fallback creates a new `sample_id`; it cannot backfill missing bytes into the failed sample. Steering, compaction, retry, restart, and subagent resume must all reconstruct from the accepted envelope rather than from visible text.

If a tool may have executed despite a crash, recovery must consult the persisted tool ledger/idempotency record before either replaying the result or allowing another execution. The provider replay envelope prevents protocol corruption; it does not by itself make external side effects idempotent.

## Legacy boundary

Legacy `reasoning_content: String` can be upgraded only when stored evidence identifies a trusted DeepSeek-style Chat Completions route and preserves the matching assistant tool-call tuple. It must not be upcast into:

- an Anthropic `thinking` block, because no authentic `signature` or redacted-block order exists;
- an OpenAI Responses reasoning item, because no item ID/status/`encrypted_content` or server-state reference exists;
- a Gemini signed part, because no authentic signature bytes or part position exists;
- a DeepSeek Responses item merely because the text field is called reasoning.

For every ambiguous legacy history, retain display content, insert an explicit replay boundary, and start the next provider request with a route-supported safe context. Never generate a sentinel or empty opaque field to make a historical turn appear native.

## Required provider test matrix

Every row below should run in streaming and non-streaming form, through both live capture and persistence/restart reconstruction. Where live credentials are unavailable, provider-wire fixtures must reproduce the cited official shapes, and at least one contract test per provider should use the authoritative API in CI or a documented release gate.

| Provider/style | Valid acceptance cases | Required rejection cases | Tool safety assertion |
| --- | --- | --- | --- |
| DeepSeek Chat Completions | complete assistant tuple; single, parallel, and sequential tool loops | missing/blank required `reasoning_content`; incomplete tool args; assistant/tool ID mismatch; partial stream + retry merge | zero tool executions before validated envelope commit |
| DeepSeek Anthropic compatibility | ordered Anthropic-shaped blocks under the exact compatibility route | OpenAI string projected onto this route; block/signature mismatch | zero executions on dialect mismatch |
| DeepSeek Responses compatibility | complete supported plaintext reasoning/function item sequence; terminal completed event | assuming `previous_response_id`, `encrypted_content`, or unsupported model; incomplete/done mismatch | zero executions from incomplete items |
| Anthropic Messages | thinking + redacted blocks; omitted-display signature-only case; single/parallel results | missing/modified/reordered block; partial JSON tool input; result not immediately after tool-use assistant message; wrong `tool_use_id` | zero executions until message/block completion and atomic persistence |
| OpenAI native Responses, stored | exact `previous_response_id`/conversation continuation and response status | unresolved/expired state treated as valid; compatibility response ID accepted as native | safe boundary or complete manual fallback, never synthetic reasoning |
| OpenAI native Responses, stateless | ordered reasoning/function-call/output items with `encrypted_content`; consecutive calls | missing item/encrypted content; summary substituted; `call_id` mismatch; incomplete item status | zero executions from delta-only or incomplete calls |
| Gemini 3 `generateContent` | single, parallel, sequential calls; signature-only empty part; optional IDs | missing/moved/changed signature; merged/reordered parts; signature added to every parallel call; stream ended before finish | zero function executions before signed ordered content is persisted |
| Gemini 2.5 `generateContent` | no signature returned; signature returned and preserved | returned signature discarded or moved | received signatures remain exact even when optional |

Cross-cutting permutations:

- endpoint trust: official exact endpoint, trusted catalog endpoint, custom HTTP endpoint, edited host, and non-standard port;
- route mismatch: payload captured under one `api_style` resumed under another;
- retry: partial stream followed by non-stream retry, proving two sample IDs and no byte merging;
- lifecycle: app restart, conversation reopen, compaction, steering, cancellation, subagent handoff, and provider/model fallback;
- durability fault injection: database failure after validation but before commit, commit success followed by process crash, and crash after external tool side effect;
- secrecy: logs, UI events, exports, and search indices contain no opaque/encrypted/signature payloads;
- forward compatibility: an unknown item/block/event is preserved or safely rejects replay according to the provider capability revision, never silently coerced to a string.

## Acceptance criteria for the Provider Turn Envelope PR

The PR is ready only when:

1. Route snapshots unambiguously select all documented dialects above, including the three separate DeepSeek surfaces.
2. Provider replay payloads preserve exact native order, IDs, signatures/encrypted fields, and completion status.
3. Display reasoning and replay state are different fields with different logging/serialization policies.
4. Streaming and non-streaming normalize only after provider-native completion; retries always create new samples.
5. Envelope persistence and tool ledger persistence are atomic and precede tool dispatch.
6. Missing or mismatched payloads cannot reach tool dispatch, and tests assert zero side effects.
7. Legacy strings cross an explicit replay boundary unless exact route provenance proves a safe DeepSeek Chat migration.
8. The provider matrix above covers primary chats and subagent/tool paths rather than testing only the top-level conversation.

## Primary sources

### DeepSeek

- [Thinking Mode](https://api-docs.deepseek.com/guides/thinking_mode/)
- [Create Chat Completion](https://api-docs.deepseek.com/api/create-chat-completion)
- [Using the Anthropic API Format](https://api-docs.deepseek.com/guides/anthropic_api)
- [Responses API](https://api-docs.deepseek.com/guides/responses_api/)
- [Models and Pricing / official API base URLs](https://api-docs.deepseek.com/quick_start/pricing)

### Anthropic

- [Extended thinking](https://platform.claude.com/docs/en/build-with-claude/extended-thinking)
- [Streaming Messages](https://platform.claude.com/docs/en/build-with-claude/streaming)
- [Handle tool calls](https://platform.claude.com/docs/en/agents-and-tools/tool-use/handle-tool-calls)
- [Troubleshooting tool use](https://platform.claude.com/docs/en/agents-and-tools/tool-use/troubleshooting-tool-use)
- [API authentication](https://platform.claude.com/docs/en/manage-claude/authentication)
- [API versioning](https://platform.claude.com/docs/en/api/versioning)
- [Pinned official Python SDK types and MIT license](https://github.com/anthropics/anthropic-sdk-python/tree/009b035305e0724ce108ebd796935f91711fc6e1)

### OpenAI

- [Reasoning models](https://developers.openai.com/api/docs/guides/reasoning)
- [Function calling](https://developers.openai.com/api/docs/guides/function-calling)
- [Migrating to the Responses API](https://developers.openai.com/api/docs/guides/migrate-to-responses)
- [Conversation state](https://developers.openai.com/api/docs/guides/conversation-state)
- [Streaming Responses](https://developers.openai.com/api/docs/guides/streaming-responses)
- [Pinned official Python SDK Responses types and Apache-2.0 license](https://github.com/openai/openai-python/tree/0c09a3fe815184f0a46fbf18b1aba84a467c854e)

### Google Gemini

- [`generateContent` thought signatures](https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures)
- [`generateContent` and `streamGenerateContent` API](https://ai.google.dev/api/generate-content)
- [Pinned official Google Gen AI Python SDK types and Apache-2.0 license](https://github.com/googleapis/python-genai/tree/66e224c39c9527e0fef3a4f049ac33ec941e2f99)
