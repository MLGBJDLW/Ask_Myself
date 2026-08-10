# Streaming terminal, retry, tool lifecycle, and loop guards: primary-source research

Date: 2026-08-10 (Asia/Shanghai)
Scope: OpenAI Responses contract, `openai/codex`, `badlogic/pi-mono`, `NousResearch/hermes-agent`, and `sst/opencode`.
Purpose: define the non-negotiable runtime invariants for Nexa's DeepSeek/OpenAI-Responses streaming, fallback, all tool-card lifecycles, and repeated-answer prevention.

## Executive conclusion

The strongest common pattern is a monotonic terminal latch:

```text
OPEN -> COMPLETED | INCOMPLETE | FAILED | INTERRUPTED
```

The first semantic terminal event wins. After that transition, socket EOF, `[DONE]`, a drain error, a stale timer, or a close callback is cleanup only. It must not reopen the request, select a fallback, or start another model round.

For Nexa, the safe retry boundary is stricter than several upstream implementations:

1. Retry a transient connection failure only before a semantic terminal and before any visible output, completed hosted-tool event, or locally executed side effect.
2. After visible output, return a typed partial/interrupted outcome and offer an explicit user retry or provider-native resume. Never transparently resend the original turn.
3. Project provider-hosted tools into the same start/result UI lifecycle as client tools, with a durable stable ID and an explicit `provider_executed` owner bit. Do not locally execute them and do not treat them as an unresolved client-tool obligation.
4. Stop the agent loop before another provider request when the model round is terminal and no unresolved client tool call exists.
5. Add Nexa-owned finite round and repetition guards. Neither Codex nor Pi is evidence that an unbounded generic loop is safe.

## Source identity and snapshots

All repository links below are pinned to full commits.

| Source | Snapshot | License |
| --- | --- | --- |
| OpenAI Responses API | Official [streaming event reference](https://platform.openai.com/docs/api-reference/responses-streaming/response/completed) | API contract, not an open-source implementation |
| `openai/codex` | [`c0ad3ab014a27d66d1631fb00f7a70b035f46f0d`](https://github.com/openai/codex/commit/c0ad3ab014a27d66d1631fb00f7a70b035f46f0d) | [Apache-2.0](https://github.com/openai/codex/blob/c0ad3ab014a27d66d1631fb00f7a70b035f46f0d/LICENSE) |
| `badlogic/pi-mono` | [`936aff00918de1187f085f123c2812d8f2d67745`](https://github.com/badlogic/pi-mono/commit/936aff00918de1187f085f123c2812d8f2d67745) | [MIT](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/LICENSE) |
| `NousResearch/hermes-agent` | [`3bd844edf1777a680115f88a68474b4fb434092f`](https://github.com/NousResearch/hermes-agent/commit/3bd844edf1777a680115f88a68474b4fb434092f) | [MIT](https://github.com/NousResearch/hermes-agent/blob/3bd844edf1777a680115f88a68474b4fb434092f/LICENSE) |
| `sst/opencode` | [`0bff28de09105088ff5bdefab91413d55c28dff1`](https://github.com/sst/opencode/commit/0bff28de09105088ff5bdefab91413d55c28dff1) | [MIT](https://github.com/sst/opencode/blob/0bff28de09105088ff5bdefab91413d55c28dff1/LICENSE) |

## 1. The protocol boundary: terminal is semantic, EOF is transport

The official Responses reference defines distinct semantic events including `response.completed`, `response.incomplete`, and `response.failed`; those must be interpreted independently of how the HTTP/SSE transport later closes ([official streaming events](https://platform.openai.com/docs/api-reference/responses-streaming/response/completed)).

DeepSeek now documents the same contract for V4 Flash rather than merely claiming generic OpenAI compatibility. Its [Responses guide](https://api-docs.deepseek.com/guides/responses_api/) says the SSE stream ends with `response.completed`, `response.incomplete`, or `response.failed`, carries monotonically increasing `sequence_number` values, and does **not** use `data: [DONE]`. The 2026-07-31 [change log](https://api-docs.deepseek.com/updates/) and [Codex integration guide](https://api-docs.deepseek.com/quick_start/agent_integrations/codex/) explicitly state that V4 Flash natively supports Responses and is adapted for Codex. Therefore the Nexa incident must not be dismissed as use of an unsupported endpoint; it is a concrete incompatibility or state-machine defect in Nexa's handling of the documented DeepSeek stream.

Codex implements that distinction directly. Its SSE decoder returns immediately on `response.completed`; reaching stream EOF first is an error ([Responses SSE reader](https://github.com/openai/codex/blob/c0ad3ab014a27d66d1631fb00f7a70b035f46f0d/codex-rs/codex-api/src/sse/responses.rs#L525-L617)). The turn consumer likewise treats `None` before completion as failure, while the `Completed` event flushes the aggregator, emits completion, and breaks successfully ([EOF branch](https://github.com/openai/codex/blob/c0ad3ab014a27d66d1631fb00f7a70b035f46f0d/codex-rs/core/src/session/turn.rs#L2231-L2250), [completed branch](https://github.com/openai/codex/blob/c0ad3ab014a27d66d1631fb00f7a70b035f46f0d/codex-rs/core/src/session/turn.rs#L2504-L2548)). Its integration test proves that an early EOF is retryable specifically because `response.completed` was absent ([early-EOF regression](https://github.com/openai/codex/blob/c0ad3ab014a27d66d1631fb00f7a70b035f46f0d/codex-rs/core/tests/suite/stream_no_completed.rs#L20-L103)).

Pi keeps an explicit `sawTerminalResponseEvent`. It maps completed/incomplete terminal frames to stop reasons, rejects EOF without a terminal, and emits success only for a non-pending/non-error stop ([Responses event state machine](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/api/openai-responses-shared.ts#L430-L593), [finalization](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/api/openai-responses-shared.ts#L595-L756)). Its tests explicitly show that a provisional `final_answer` does not override the authoritative incomplete/completed terminal status ([terminal regression tests](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/test/openai-responses-terminal-event.test.ts#L206-L293)).

Hermes stops its event loop on the first terminal frame ([terminal latch](https://github.com/NousResearch/hermes-agent/blob/3bd844edf1777a680115f88a68474b4fb434092f/agent/codex_runtime.py#L1171-L1202)). More importantly for the reported Nexa symptom, after it has assembled a terminal response, a transport failure while draining the remaining iterator is logged as a non-fatal finalization warning and the completed response is returned instead of retried ([post-terminal drain](https://github.com/NousResearch/hermes-agent/blob/3bd844edf1777a680115f88a68474b4fb434092f/agent/codex_runtime.py#L1398-L1426)).

OpenCode's WebSocket bridge sets `completed`, emits exactly one synthetic `[DONE]`, closes the consumer stream, and ignores later close/error callbacks after a Responses terminal event. A socket close is an error only while `completed` is still false ([WebSocket terminal latch](https://github.com/sst/opencode/blob/0bff28de09105088ff5bdefab91413d55c28dff1/packages/opencode/src/plugin/openai/ws.ts#L145-L278)). Its tests distinguish terminal completion from close-before-terminal failure ([WebSocket regressions](https://github.com/sst/opencode/blob/0bff28de09105088ff5bdefab91413d55c28dff1/packages/opencode/test/plugin/openai-ws.test.ts#L80-L122)).

**Direct Nexa conclusion:** a fallback/reconnect that fires after a parsed `response.completed` is not resilience; it is an illegal state transition.

## 2. Retry ownership and partial delivery

Pi disables hidden SDK retries with `maxRetries: 0`, then puts request establishment behind one explicit bounded and abortable retry helper ([single retry owner](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/api/openai-responses.ts#L145-L190), [provider retry helper](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/utils/provider-retry.ts#L97-L124)). Its higher-level retry returns immediately for every non-error assistant result and retries only classified errors within a finite attempt count ([assistant retry](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/utils/retry.ts#L145-L211)). This prevents a successful terminal from falling through into another retry owner.

Codex only enters retry/fallback on `Err`, using ordinary retry limits and then WebSocket-to-HTTPS fallback ([Responses retry](https://github.com/openai/codex/blob/c0ad3ab014a27d66d1631fb00f7a70b035f46f0d/codex-rs/core/src/responses_retry.rs#L38-L113)). The same source has an intentionally unbounded `ConnectionFailed` branch, so it is not a safe limit to copy into Nexa.

Hermes records whether any deltas reached the user. Its normal rule is not to retry after partial visible delivery because replay would duplicate text ([partial-delivery guard](https://github.com/NousResearch/hermes-agent/blob/3bd844edf1777a680115f88a68474b4fb434092f/agent/chat_completion_helpers.py#L4044-L4103)). It makes one narrow exception for a transient drop while a tool call is still being generated and no tool has executed; the source explicitly accepts a duplicated preamble, emits a reconnect marker, clears per-attempt buffers, and fences the previous stream before a bounded retry ([mid-tool exception](https://github.com/NousResearch/hermes-agent/blob/3bd844edf1777a680115f88a68474b4fb434092f/agent/chat_completion_helpers.py#L4104-L4148), [attempt fencing](https://github.com/NousResearch/hermes-agent/blob/3bd844edf1777a680115f88a68474b4fb434092f/agent/chat_completion_helpers.py#L3104-L3198)).

That Hermes exception is useful evidence about side-effect safety, but its duplicate-output UX is not acceptable for Nexa. Nexa should convert a mid-tool drop into an explicit interrupted tool card and require deliberate resume/retry, unless the provider offers an idempotent resume token that continues the same response without replaying already rendered bytes.

## 3. Provider-hosted tools must still produce cards

Codex treats provider-hosted web search as a first-class turn item. `response.output_item.added` produces an item start, `response.output_item.done` completes it, and `WebSearchCall` is explicitly mapped to `TurnItem::WebSearch` rather than being flattened into invisible provider metadata ([stream item lifecycle](https://github.com/openai/codex/blob/c0ad3ab014a27d66d1631fb00f7a70b035f46f0d/codex-rs/core/src/session/turn.rs#L2371-L2447), [hosted web-search item](https://github.com/openai/codex/blob/c0ad3ab014a27d66d1631fb00f7a70b035f46f0d/codex-rs/core/src/stream_events_utils.rs#L328-L350), [event projection](https://github.com/openai/codex/blob/c0ad3ab014a27d66d1631fb00f7a70b035f46f0d/codex-rs/core/src/event_mapping.rs#L223-L233)).

OpenCode generalizes this pattern. It maps `web_search_call`, file search, code interpreter, computer use, image generation, MCP, and local shell items to a uniform `tool-call` + `tool-result` pair, preserves the provider item ID, and marks both events `providerExecuted: true` ([hosted-tool registry and projection](https://github.com/sst/opencode/blob/0bff28de09105088ff5bdefab91413d55c28dff1/packages/llm/src/protocols/openai-responses.ts#L533-L603)). Its output-item handler emits that pair when the hosted item is done ([hosted item completion](https://github.com/sst/opencode/blob/0bff28de09105088ff5bdefab91413d55c28dff1/packages/llm/src/protocols/openai-responses.ts#L808-L844)). The session processor then persists a single tool part through pending/running/completed/error states, including provider-executed ownership ([tool-part state machine](https://github.com/sst/opencode/blob/0bff28de09105088ff5bdefab91413d55c28dff1/packages/opencode/src/session/processor.ts#L216-L253), [tool stream events](https://github.com/sst/opencode/blob/0bff28de09105088ff5bdefab91413d55c28dff1/packages/opencode/src/session/processor.ts#L315-L419)).

Hermes' Codex app-server bridge independently demonstrates the same UI boundary: even Codex-internal `webSearch` receives stable start/completed callbacks and a deterministic call ID, while text and reasoning remain separate streams ([tool type mapping](https://github.com/NousResearch/hermes-agent/blob/3bd844edf1777a680115f88a68474b4fb434092f/agent/codex_runtime.py#L276-L354), [stable card lifecycle](https://github.com/NousResearch/hermes-agent/blob/3bd844edf1777a680115f88a68474b4fb434092f/agent/codex_runtime.py#L431-L545), [event dispatch](https://github.com/NousResearch/hermes-agent/blob/3bd844edf1777a680115f88a68474b4fb434092f/agent/codex_runtime.py#L589-L615)).

Pi provides a strong client-tool lifecycle (`toolcall_start/delta/end`, then execution start/update/end and a tool-result message), but its examined Responses slot does not project hosted web search. It is therefore a client-tool reference, not the model for Nexa's native-search card ([provider tool-call stream](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/ai/src/api/openai-responses-shared.ts#L461-L524), [agent execution events](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/agent/src/agent-loop.ts#L489-L553)).

**Direct Nexa conclusion:** native DeepSeek search is not “no tool call.” It is a provider-owned tool call that needs a card but no local dispatch.

### Nexa's generic tool-card visibility failure is a separate layer

The live Nexa database proves that ordinary client tools are not missing from the backend protocol: recent `run_shell`, `read_file`, `grep_files`, `edit_file`, and `web_research_context` starts are durable `toolStarted` events with `visibility=user` and `display_kind=tool`. The current frontend nevertheless routes every normal `ToolCallCard` through `ThinkingBlock`; `collapseOnFinish` then removes the cards from the visible DOM when the phase completes, and persisted traces start collapsed. The E2E contract explicitly expects the completed thinking toggle to be `aria-expanded=false` before clicking it. This is why the product appears to hide *all* completed cards, not just native web search.

The product decision is to keep that information architecture: ToolCards stay inside Thinking, the live trace opens while work is streaming, and the completed trace collapses by default. Therefore collapse itself is not a defect. The defect boundary is a missing live/durable tool lifecycle or a completed Thinking control that cannot reveal its cards when clicked; the fix must preserve both live visibility and deterministic replay without moving cards outside Thinking.

There is also a real durable-wire mismatch hidden by synthetic tests. Rust serializes the enum variant field as `tool_call` in `traceTimeline` / `turnTrace` JSON, while `extractPersistedTraceItems` reads only `toolCall`. The frontend unit fixtures hand-author camelCase `toolCall`, so they pass without exercising the JSON produced by the backend. Message-history reconstruction can mask this for ordinary function calls, but provider-hosted items and any path relying on the durable trace lose the tool item outright.

Nexa also has explicit presentation suppressions for `update_plan` / plan-rendered calls, `tool_search`, `prepare_document_tools`, and successful generated-image calls. Some have alternate surfaces, but these exceptions reinforce why a single generic “tool cards exist” test is insufficient. Acceptance must cover each tool ownership and presentation class: client, provider-hosted, internal/discovery, plan, image/artifact, live, settled, restart replay, and compacted history.

## 4. Stopping and repetition guards

OpenCode checks the latest durable assistant finish before creating another provider round. A terminal finish with no unresolved client tool part exits the loop; provider-executed tools are excluded from that unresolved-client check ([pre-request stop gate](https://github.com/sst/opencode/blob/0bff28de09105088ff5bdefab91413d55c28dff1/packages/opencode/src/session/prompt.ts#L1081-L1130)). It also detects repeated identical tool name/input sequences and asks through a dedicated `doom_loop` permission instead of executing silently forever ([repeated-tool breaker](https://github.com/sst/opencode/blob/0bff28de09105088ff5bdefab91413d55c28dff1/packages/opencode/src/session/processor.ts#L331-L380)). Its general step limit is configurable but defaults to infinity, so this is not a complete generic answer-repetition guard ([optional step limit](https://github.com/sst/opencode/blob/0bff28de09105088ff5bdefab91413d55c28dff1/packages/opencode/src/session/prompt.ts#L1170-L1182)).

Pi exposes `shouldStopAfterTurn` before the next provider call and supports a terminating tool-result batch, but has no examined global hard turn counter ([turn stop hook](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/agent/src/agent-loop.ts#L169-L275), [termination result type](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/agent/src/types.ts#L212-L222), [batch termination](https://github.com/badlogic/pi-mono/blob/936aff00918de1187f085f123c2812d8f2d67745/packages/agent/src/agent-loop.ts#L550-L584)). Codex's main turn loop is also not a generic finite repetition detector ([turn loop](https://github.com/openai/codex/blob/c0ad3ab014a27d66d1631fb00f7a70b035f46f0d/codex-rs/core/src/session/turn.rs#L430-L469)).

Hermes enforces both a per-turn API-call limit and a shared iteration budget ([bounded conversation loop](https://github.com/NousResearch/hermes-agent/blob/3bd844edf1777a680115f88a68474b4fb434092f/agent/conversation_loop.py#L1634-L1669)). It also has opt-in semantic continuation policies such as verification-on-stop. Those policies explicitly persist the candidate as interim, clear the final response, increment a separate nudge counter, and start a new semantic round ([verification continuation](https://github.com/NousResearch/hermes-agent/blob/3bd844edf1777a680115f88a68474b4fb434092f/agent/conversation_loop.py#L7462-L7501)). This is materially different from a transport reconnect. Nexa should keep such product policy off the transport path and make any continuation reason visible and bounded.

## 5. Required Nexa invariants

1. **One terminal latch per physical response.** The first of completed/incomplete/failed/interrupted atomically seals the attempt. Later events are ignored except for diagnostics.
2. **Terminal-before-EOF precedence.** EOF after a terminal is normal cleanup. EOF before a terminal is an interrupted transport outcome, never a fabricated completion.
3. **No post-terminal fallback.** Retry/fallback eligibility must include `terminal_seen == false`; this check belongs at the only retry owner.
4. **No transparent replay after visible delivery.** Any non-empty reasoning, answer, tool-card, or hosted-tool event suppresses automatic resend. Return a typed partial result instead.
5. **Attempt fencing.** Every projected event carries `(run_id, round_id, attempt_id, sequence)`. Only the current attempt may mutate UI or durable state; late chunks from superseded attempts are discarded.
6. **Idempotent text reconciliation.** Terminal aggregate text may emit only the suffix not already streamed. It must never re-emit the full final answer.
7. **Hosted-tool projection.** Map provider item start/done to tool start/completed/error using a stable provider item ID and `provider_executed=true`.
8. **Execution ownership.** Client tools require one local result before continuation. Hosted tools require no local dispatch and cannot by themselves trigger another model round.
9. **Strict loop stop.** `terminal && unresolved_client_calls == 0` stops before building or sending the next provider request.
10. **Explicit continuation only.** Length continuation, verification, or another product policy must create a separately named, user-visible, bounded semantic transition. It is never called “reconnect” or “fallback.”
11. **Finite round budget.** Cap model sampling rounds per user turn, independent of HTTP retry count.
12. **Repetition fingerprint.** Abort when consecutive rounds repeat the same provider response ID, terminal content hash, unresolved call-ID set, and tool name/input fingerprint without new durable evidence. Record which dimension repeated.

## 6. Regression matrix for the DeepSeek Flash path

| Case | Required observation |
| --- | --- |
| text deltas -> `response.completed` -> EOF | One answer, one terminal, zero reconnects, one provider request |
| text deltas -> `response.completed` containing the same full text | Terminal reconciliation emits an empty suffix; no duplicate final reply |
| `response.completed` -> delayed stale chunk/close callback | Delayed data ignored; terminal state unchanged |
| EOF before terminal, zero visible events | Bounded transient retry may occur under a new attempt ID |
| EOF before terminal after any visible delta | Partial/interrupted result; no transparent resend |
| hosted `web_search_call` done -> final answer | One provider-owned tool card and one answer; no local execution; no extra sampling round solely for the hosted tool |
| client function call -> local result -> terminal answer | One card lifecycle, one durable matching result, then one continuation round |
| terminal `stop`, no client calls | The agent loop exits before another request body is built |
| same completed payload delivered twice | Exactly-once terminal projection and exactly-once persisted reply |
| same answer/call fingerprint repeats across rounds | Repetition guard terminalizes the turn with diagnostics before the hard round cap |

## Final recommendation

Nexa should use four separate state machines rather than one broad “stream failed, reconnect” fallback:

```text
transport attempt: connecting -> open -> closed
provider response: created -> streaming -> terminal
tool item: pending -> running -> completed | error
agent turn: sampling -> client-tools -> sampling -> final
```

Only the transport machine may reconnect, and only while the provider-response machine has not reached a terminal and the turn has not delivered observable output. This separation is the core design shared by the strongest examined paths and directly prevents the reported “already completed, then reconnect and answer again” failure.
