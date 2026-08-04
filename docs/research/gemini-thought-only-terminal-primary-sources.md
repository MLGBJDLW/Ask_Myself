# Provider-neutral output exhaustion and reasoning-only terminals: primary-source notes

Date: 2026-08-04

This note validates the hotfix request in `D:\Nexa.txt`, including the Gemini and DeepSeek reports, against the synchronized Nexa baseline, provider primary sources, and fixed snapshots of OpenAI Codex and Pi Agent as the main runtime references. Provider SDKs supply adapter-level evidence; OpenCode and Goose are secondary cross-checks only. It is an engineering input, not an implementation record. Statements labeled **Nexa inference** are design conclusions, not claims made by the providers or upstream projects.

## Executive decision

1. **Reasoning and visible answer are separate provider channels.** Gemini marks reasoning with `Part.thought`; DeepSeek returns `reasoning_content` beside nullable `content`. Google's official SDK excludes thought parts from its answer getter, while DeepSeek describes `reasoning_content` as content emitted before the final answer. A reasoning-only response therefore has no visible answer to promote. [Google thinking guide](https://ai.google.dev/gemini-api/docs/generate-content/thinking#thought-summaries), [Google SDK answer extractor](https://github.com/googleapis/js-genai/blob/e728fad9599af298f515329fcb92f5f122e110cf/src/types.ts#L3717-L3754), [DeepSeek response schema](https://api-docs.deepseek.com/api/create-chat-completion/#responses)
2. **Finish reason is terminal metadata, not content.** Google defines `MAX_TOKENS`; DeepSeek defines `length`, `content_filter`, `tool_calls`, and `insufficient_system_resource` in addition to `stop`. None grants permission to reclassify reasoning as the answer. [Google protocol enum](https://github.com/googleapis/googleapis/blob/f3ff3a1dc91aa7719f98437416fd686fad0296cd/google/cloud/aiplatform/v1/content.proto#L709-L725), [DeepSeek finish reasons](https://api-docs.deepseek.com/api/create-chat-completion/#responses), [LiteLLM normalization](https://github.com/BerriAI/litellm/blob/a79f598f692e66ce49790bfda699b7f4dccb3ca0/litellm/litellm_core_utils/core_helpers.py#L90-L108)
3. **A terminal chunk may validly carry no answer text.** Both protocols permit nullable/absent visible content, and DeepSeek's official streaming example accumulates `reasoning_content` and `content` independently. Reducers must preserve an empty answer together with reasoning, finish reason, usage, and tools; storage/UI must represent incomplete/error without changing channels. [DeepSeek streaming example](https://api-docs.deepseek.com/guides/thinking_mode/#multi-turn-conversation), [LiteLLM content-less chunk handling](https://github.com/BerriAI/litellm/blob/a79f598f692e66ce49790bfda699b7f4dccb3ca0/litellm/llms/vertex_ai/gemini/vertex_and_google_ai_studio_gemini.py#L3168-L3192), [LangChain Google empty-message handling](https://github.com/langchain-ai/langchain-google/blob/942b7cc4bc0abe750b6350379e5df870db19d9ee/libs/genai/langchain_google_genai/chat_models.py#L1431-L1456)
4. **The visible leak has one provider-agnostic root cause in Nexa.** Both Gemini and the OpenAI-compatible/DeepSeek stream parser already separate ordinary text from reasoning and retain terminal metadata. Later, the agent loop copies `iteration_thinking` into `full_content` whenever visible content is empty and there are no tool calls. That shared fallback explains both reports. [Nexa OpenAI-compatible separation](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/streaming.rs#L31-L48), [stream emission](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/streaming.rs#L660-L716), [promotion fallback](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/agent/turn_loop.rs#L930-L938)
5. **DeepSeek also has an independent tool-replay contract.** When a request carries tools, official docs require the full `reasoning_content` to be passed back in all subsequent requests or the API returns HTTP 400. A valid tool subturn can have `content: ""`, non-empty reasoning, and tool calls. The shared terminal hotfix must keep tools higher priority than empty-answer handling, and the adapter must preserve exact reasoning history rather than inventing a visible answer. [DeepSeek tool-call contract and examples](https://api-docs.deepseek.com/guides/thinking_mode/#tool-calls)
6. **The primary agent-runtime references support a typed, multi-step lifecycle, but neither is a complete answer to output exhaustion.** OpenAI Codex persists completed response items, executes tools, and follows up until no tool or server continuation remains; Pi Agent preserves `stopReason`, refuses to execute token-truncated tool calls, and asks the model to re-issue them. Yet Codex maps `response.incomplete` to a generic stream error, and Pi stops after a no-tool `length` message without regenerating visible text. Nexa should borrow their separation and continuation mechanics, not copy those terminal gaps. [Codex turn loop](https://github.com/openai/codex/blob/49b0aebd6fba2fc590abbe16882cefd048524228/codex-rs/core/src/session/turn.rs#L135-L147), [Codex incomplete mapping](https://github.com/openai/codex/blob/49b0aebd6fba2fc590abbe16882cefd048524228/codex-rs/codex-api/src/sse/responses.rs#L426-L445), [Pi truncated-tool handling](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/agent/src/agent-loop.ts#L202-L224)

The required hotfix invariant is:

```text
Only ordinary provider answer/text parts may populate the user-visible reply.
Reasoning may be retained, folded, persisted, or protocol-required replayed,
but never promoted into reply content.
```

## 1. Verified Google contract

### 1.1 `thought` is an explicit channel marker

Google's versioned Vertex AI protocol declares `Part.thought` separately from `Part.text` and describes it as indicating that the part is thought from the model. `ThinkingConfig.include_thoughts` controls whether thoughts are returned when available. [Google `Part` protocol](https://github.com/googleapis/googleapis/blob/f3ff3a1dc91aa7719f98437416fd686fad0296cd/google/cloud/aiplatform/v1/content.proto#L117-L173), [Google `ThinkingConfig`](https://github.com/googleapis/googleapis/blob/f3ff3a1dc91aa7719f98437416fd686fad0296cd/google/cloud/aiplatform/v1/content.proto#L377-L412)

The public thinking guide is even more explicit: it calls these values **thought summaries**, says they summarize raw thoughts, and demonstrates branching on `part.thought` into separate `thoughts` and `answer` accumulators in both non-streaming and streaming examples. [Non-streaming separation](https://ai.google.dev/gemini-api/docs/generate-content/thinking#thought-summaries), [streaming separation](https://ai.google.dev/gemini-api/docs/generate-content/thinking#streaming)

Two distinctions matter for Nexa:

- `thought: true` marks reasoning/thought-summary content.
- `thoughtSignature` is opaque reasoning-state metadata for later turns and can also appear on non-thought or function-call parts. It is not a channel classifier. [Official thought-signature rules](https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures), [official SDK fields](https://github.com/googleapis/js-genai/blob/e728fad9599af298f515329fcb92f5f122e110cf/src/types.ts#L2068-L2078)

### 1.2 The official SDK does not turn thought text into answer text

At fixed commit `e728fad9599af298f515329fcb92f5f122e110cf`, Google's JavaScript SDK implements `GenerateContentResponse.text` by concatenating text only from parts for which `part.thought` is not true. If all text parts are thoughts, its `anyTextPartText` flag remains false and the getter returns `undefined`. [SDK implementation](https://github.com/googleapis/js-genai/blob/e728fad9599af298f515329fcb92f5f122e110cf/src/types.ts#L3717-L3754)

This is the strongest direct upstream precedent for the hotfix: a thought-only response is structurally representable, but it has no convenience-level answer string.

### 1.3 `MAX_TOKENS` and usage fields must remain distinct facts

Google's protocol says:

- `STOP` means a natural stop point or configured stop sequence.
- `MAX_TOKENS` means generation reached the configured maximum output tokens.
- `finish_reason` is output-only terminal metadata and may be absent while generation is still active. [Protocol source](https://github.com/googleapis/googleapis/blob/f3ff3a1dc91aa7719f98437416fd686fad0296cd/google/cloud/aiplatform/v1/content.proto#L709-L769)
- Usage reports candidate tokens and thought tokens separately, and total tokens include both. [Protocol usage fields](https://github.com/googleapis/googleapis/blob/f3ff3a1dc91aa7719f98437416fd686fad0296cd/google/cloud/aiplatform/v1/prediction_service.proto#L823-L840), [official SDK usage fields](https://github.com/googleapis/js-genai/blob/e728fad9599af298f515329fcb92f5f122e110cf/src/types.ts#L3636-L3657)

The thinking guide says `thinkingBudget` guides the number of raw thinking tokens and warns that a model may overflow or underflow that budget. It also says displayed thought summaries are not the raw thoughts to which the budget applies. [Thinking-budget guidance](https://ai.google.dev/gemini-api/docs/generate-content/thinking#thinking-budgets), [summary distinction](https://ai.google.dev/gemini-api/docs/generate-content/thinking#thought-summaries)

**Nexa inference:** the screenshot/token estimate described in `D:\Nexa.txt` is consistent with output-budget exhaustion but cannot prove it. UI-estimated summary tokens, provider `thoughtsTokenCount`, `candidatesTokenCount`, and `maxOutputTokens` are not interchangeable. The trace or a deterministic fixture must show all of:

```text
finishReason = MAX_TOKENS (normalized by Nexa to length)
ordinary answer chars = 0
thought chars > 0
tool calls = 0
```

Likewise, `thinkingBudget + answerReserve <= maxOutputTokens` is a possible Nexa product policy, not a Google protocol invariant. A model-aware headroom policy may reduce recurrence, but it cannot replace correct channel handling and should not assume that `thinkingBudget` is a hard allocation.

## 2. Verified DeepSeek contract

DeepSeek's hosted API documentation is the authoritative source for this section and was checked live on 2026-08-04. It is not backed by a public, commit-pinnable hosted-API server or DeepSeek SDK repository, so the links below are intentionally date-qualified rather than presented as immutable source snapshots. DeepSeek's public model/inference repository is not evidence for the hosted Chat Completions wire contract.

### 2.1 `reasoning_content` and `content` are sibling channels

DeepSeek says thinking mode emits chain-of-thought before the final answer and returns it in `reasoning_content` at the same level as `content`. The Chat Completions schema defines assistant `content` as required but nullable and `reasoning_content` as nullable; the streaming delta schema likewise exposes both independently. Its official streaming example accumulates reasoning and visible content in separate variables. [Thinking-mode contract](https://api-docs.deepseek.com/guides/thinking_mode/#input-and-output-parameters), [non-streaming and streaming schema](https://api-docs.deepseek.com/api/create-chat-completion/#responses), [official streaming example](https://api-docs.deepseek.com/guides/thinking_mode/#multi-turn-conversation)

The same schema defines terminal reasons as:

- `stop`: natural stop or configured stop sequence;
- `length`: the requested maximum token count was reached;
- `content_filter`: content was omitted by a content filter;
- `tool_calls`: the model called a tool;
- `insufficient_system_resource`: inference was interrupted because resources were insufficient.

[DeepSeek finish-reason schema](https://api-docs.deepseek.com/api/create-chat-completion/#responses)

**Nexa inference:** `reasoning_content != empty + content null/empty + finish_reason = length + no tools` is an incomplete answer, not a successful response. `stop` with the same empty visible shape is a provider empty-final anomaly. Both are representable by the wire schema; neither authorizes exposing the reasoning. `tool_calls` is different: it is an intermediate control-flow result even when `content` is empty.

### 2.2 Tool calls impose a stronger replay requirement

DeepSeek draws an important boundary between ordinary turns and tool-bearing turns:

- With no tool call between two user messages, an intermediate assistant's `reasoning_content` need not be included in later context; if included, the API ignores it.
- Once a request carries `tools`, the full `reasoning_content` must be passed back in all subsequent requests. The official guide says omission causes HTTP 400.

[DeepSeek context rules](https://api-docs.deepseek.com/guides/thinking_mode/#input-and-output-parameters), [tool replay requirement](https://api-docs.deepseek.com/guides/thinking_mode/#tool-calls)

The official sample appends the complete assistant object containing `content`, `reasoning_content`, and `tool_calls`, then appends each tool result. It demonstrates a valid intermediate subturn with `content: ""`, non-empty reasoning, and a tool call, followed later by an answer-bearing subturn. [DeepSeek tool-call sample and output](https://api-docs.deepseek.com/guides/thinking_mode/#tool-calls)

**Nexa inference:** tools must take precedence over terminal empty-answer handling. The agent should execute the tools, preserve the exact assistant reasoning for replay, and only evaluate whether a visible final exists after a no-tool terminal result. A placeholder may avoid validation failure but is not equivalent to the provider's requirement to replay the full reasoning chain.

### 2.3 Version boundary

Do not apply older `deepseek-reasoner`/R1 summaries mechanically to current DeepSeek V4. The current documentation describes `deepseek-v4-flash` and `deepseek-v4-pro`, thinking enabled by default, optional omission of reasoning only for no-tool ordinary continuation, and mandatory full replay for tool flows. The live contract above is the integration target for these catalog models. [Current model and thinking parameters](https://api-docs.deepseek.com/api/create-chat-completion/#request)

## 3. Fixed-source open-source comparison

### 3.1 LiteLLM Gemini: strongest positive implementation reference

Fixed commit: `a79f598f692e66ce49790bfda699b7f4dccb3ca0`.

LiteLLM returns `(content, reasoning_content)` from the Gemini parts. Only `part.thought is True` enters `reasoning_content`; ordinary non-empty text enters `content`. With a thought-only part, visible `content` remains `None`. [Parser](https://github.com/BerriAI/litellm/blob/a79f598f692e66ce49790bfda699b7f4dccb3ca0/litellm/llms/vertex_ai/gemini/vertex_and_google_ai_studio_gemini.py#L1364-L1403), [separation test](https://github.com/BerriAI/litellm/blob/a79f598f692e66ce49790bfda699b7f4dccb3ca0/tests/test_litellm/llms/vertex_ai/gemini/test_vertex_and_google_ai_studio_gemini.py#L407-L441)

Its streaming regression test makes the boundary executable: a `thought: true` chunk yields `delta.content is None` and populated `delta.reasoning_content`. [Thought-only streaming test](https://github.com/BerriAI/litellm/blob/a79f598f692e66ce49790bfda699b7f4dccb3ca0/tests/test_litellm/llms/vertex_ai/gemini/test_vertex_and_google_ai_studio_gemini.py#L675-L712)

LiteLLM separately maps Google's `MAX_TOKENS` to OpenAI-compatible `length`. A final chunk with `finishReason` but no content is converted into an empty delta choice carrying the mapped finish reason; metadata-only chunks similarly produce a choice with no finish reason, preserving reducer safety without fabricating text. [Finish mapping](https://github.com/BerriAI/litellm/blob/a79f598f692e66ce49790bfda699b7f4dccb3ca0/litellm/litellm_core_utils/core_helpers.py#L90-L108), [content-less chunk handling](https://github.com/BerriAI/litellm/blob/a79f598f692e66ce49790bfda699b7f4dccb3ca0/litellm/llms/vertex_ai/gemini/vertex_and_google_ai_studio_gemini.py#L3168-L3192), [terminal-chunk tests](https://github.com/BerriAI/litellm/blob/a79f598f692e66ce49790bfda699b7f4dccb3ca0/tests/test_litellm/llms/vertex_ai/gemini/test_gemini_streaming_tool_call_finish_reason.py#L24-L141), [metadata-only test](https://github.com/BerriAI/litellm/blob/a79f598f692e66ce49790bfda699b7f4dccb3ca0/tests/test_litellm/llms/vertex_ai/gemini/test_gemini_streaming_tool_call_finish_reason.py#L307-L340)

**Nexa inference:** adopt LiteLLM's data boundary directly: answer, reasoning, finish reason, usage, and tools are orthogonal facts. Nexa's runtime can then apply stricter agent completion semantics on top.

### 3.2 LangChain Google: preserve structure and let the runtime decide completion

Fixed commit: `942b7cc4bc0abe750b6350379e5df870db19d9ee`.

LangChain converts `part.thought` into a typed `thinking` block. Only non-empty ordinary text becomes a `text` block/string. If no usable part exists, it leaves content as `[]` for Gemini 3+ or `""` for older models, then still builds an `AIMessage`/`AIMessageChunk`; it does not synthesize an answer. [Part conversion](https://github.com/langchain-ai/langchain-google/blob/942b7cc4bc0abe750b6350379e5df870db19d9ee/libs/genai/langchain_google_genai/chat_models.py#L1080-L1130), [empty content and message construction](https://github.com/langchain-ai/langchain-google/blob/942b7cc4bc0abe750b6350379e5df870db19d9ee/libs/genai/langchain_google_genai/chat_models.py#L1258-L1288)

It preserves `candidate.finish_reason.name` in generation metadata, including `MAX_TOKENS`. If there are no candidates at all, it logs a warning and returns an empty message rather than borrowing content from another channel. [Finish reason](https://github.com/langchain-ai/langchain-google/blob/942b7cc4bc0abe750b6350379e5df870db19d9ee/libs/genai/langchain_google_genai/chat_models.py#L1351-L1368), [no-candidate path](https://github.com/langchain-ai/langchain-google/blob/942b7cc4bc0abe750b6350379e5df870db19d9ee/libs/genai/langchain_google_genai/chat_models.py#L1431-L1456)

**Nexa inference:** LangChain is an adapter precedent, not a sufficient agent-state policy. Nexa must still decide that `no ordinary text + no tools + terminal reason` is incomplete/anomalous rather than successful.

### 3.3 Continue Gemini: useful counterexample, not a template

Fixed commit: `5522c6f44ca0ac3528b37244818fbfa39b5af470`.

Continue's current openai-adapters Gemini stream iterates every part with `part.text` and emits it as ordinary assistant content without checking `part.thought`. Its non-streaming wrapper concatenates those chunks and hard-codes `finish_reason: "stop"`, rather than retaining provider `finishReason`. [Stream conversion](https://github.com/continuedev/continue/blob/5522c6f44ca0ac3528b37244818fbfa39b5af470/packages/openai-adapters/src/apis/Gemini.ts#L330-L370), [non-streaming reconstruction](https://github.com/continuedev/continue/blob/5522c6f44ca0ac3528b37244818fbfa39b5af470/packages/openai-adapters/src/apis/Gemini.ts#L283-L320)

Its older Gemini parser explicitly notes that a max-token response may contain no parts, but only skips/warns and does not provide a durable completion state. [Older parser](https://github.com/continuedev/continue/blob/5522c6f44ca0ac3528b37244818fbfa39b5af470/core/llm/llms/Gemini.ts#L386-L452)

**Nexa inference:** this path can lose both the thought boundary and `MAX_TOKENS` signal. It demonstrates why project popularity is not enough; Nexa should not copy this behavior.

### 3.4 LiteLLM DeepSeek: correct channel separation, defensive but lossy replay fallback

Fixed commit: `956d5177d1d915adc8084c142d9d2babad1ff7af`.

LiteLLM's OpenAI-compatible response path extracts DeepSeek `reasoning_content` separately and returns the original nullable/empty `content` unchanged. Its stream builder independently joins non-null answer chunks and reasoning chunks. A thought-only DeepSeek response therefore remains `content = None/""` plus reasoning; it is not promoted. [Non-streaming extraction](https://github.com/BerriAI/litellm/blob/956d5177d1d915adc8084c142d9d2babad1ff7af/litellm/litellm_core_utils/prompt_templates/common_utils.py#L1487-L1504), [response transformation](https://github.com/BerriAI/litellm/blob/956d5177d1d915adc8084c142d9d2babad1ff7af/litellm/llms/openai/chat/gpt_transformation.py#L516-L585), [stream collection](https://github.com/BerriAI/litellm/blob/956d5177d1d915adc8084c142d9d2babad1ff7af/litellm/main.py#L8527-L8562), [independent joins](https://github.com/BerriAI/litellm/blob/956d5177d1d915adc8084c142d9d2babad1ff7af/litellm/litellm_core_utils/streaming_chunk_builder_utils.py#L352-L369)

For request replay, LiteLLM restores `reasoning_content` from the assistant field or `provider_specific_fields`. If both are missing it injects one space and warns that the blank chain may silently reduce answer quality. The repair is guarded by model capability plus explicitly enabled thinking. [DeepSeek replay transformation](https://github.com/BerriAI/litellm/blob/956d5177d1d915adc8084c142d9d2babad1ff7af/litellm/llms/deepseek/chat/transformation.py#L63-L101), [guard and request transform](https://github.com/BerriAI/litellm/blob/956d5177d1d915adc8084c142d9d2babad1ff7af/litellm/llms/deepseek/chat/transformation.py#L128-L137), [sync and async application](https://github.com/BerriAI/litellm/blob/956d5177d1d915adc8084c142d9d2babad1ff7af/litellm/llms/deepseek/chat/transformation.py#L208-L250), [replay tests](https://github.com/BerriAI/litellm/blob/956d5177d1d915adc8084c142d9d2babad1ff7af/tests/llm_translation/test_deepseek_completion.py#L181-L287)

**Nexa inference:** copy the channel boundary, not the placeholder policy. DeepSeek V4 currently enables thinking by default, so an explicit-enable-only guard can miss real tool flows. More importantly, a single space is not the required full prior reasoning. Nexa should retain the actual value at ingestion and fail explicitly if a mandatory tool replay cannot be reconstructed.

### 3.5 LangChain DeepSeek: correct ingestion, outbound replay gap

Fixed commit: `1a43a6e14ab2c8ff6e4fac250941e9568926e1b4`.

LangChain's DeepSeek partner adapter stores non-streaming and streaming `reasoning_content` in `AIMessage.additional_kwargs`; its OpenAI parent normalizes nullable answer content to an empty string. This retains a thought-only result as empty visible content plus separate reasoning rather than promoting it. [DeepSeek result and stream conversion](https://github.com/langchain-ai/langchain/blob/1a43a6e14ab2c8ff6e4fac250941e9568926e1b4/libs/partners/deepseek/langchain_deepseek/chat_models.py#L310-L372), [nullable content conversion](https://github.com/langchain-ai/langchain/blob/1a43a6e14ab2c8ff6e4fac250941e9568926e1b4/libs/partners/openai/langchain_openai/chat_models/base.py#L201-L241), [stream delta conversion](https://github.com/langchain-ai/langchain/blob/1a43a6e14ab2c8ff6e4fac250941e9568926e1b4/libs/partners/openai/langchain_openai/chat_models/base.py#L470-L505), [tests](https://github.com/langchain-ai/langchain/blob/1a43a6e14ab2c8ff6e4fac250941e9568926e1b4/libs/partners/deepseek/tests/unit_tests/test_chat_models.py#L116-L238)

The outbound parent converter serializes content, tool calls/function calls, and audio from an `AIMessage`, but not arbitrary `additional_kwargs.reasoning_content`. The DeepSeek subclass then only normalizes list content; it does not restore the reasoning field. [Outbound message conversion](https://github.com/langchain-ai/langchain/blob/1a43a6e14ab2c8ff6e4fac250941e9568926e1b4/libs/partners/openai/langchain_openai/chat_models/base.py#L388-L467), [DeepSeek request payload](https://github.com/langchain-ai/langchain/blob/1a43a6e14ab2c8ff6e4fac250941e9568926e1b4/libs/partners/deepseek/langchain_deepseek/chat_models.py#L275-L308)

**Nexa inference from those fixed sources:** this snapshot can read DeepSeek reasoning yet can omit it when serializing a later tool request, violating the current V4 contract and risking HTTP 400. It is a useful warning that correct display separation does not prove correct request replay.

## 4. Fixed-source agent-runtime comparison

Provider adapters answer only whether a chunk is text, reasoning, a tool call, usage, or terminal metadata. Coding agents add a second layer: repeated model turns, tool execution, loop/budget limits, context compaction, persistence, and user-visible completion. OpenAI Codex and Pi Agent are the primary implementation references below; OpenCode and Goose are secondary cross-checks. The question is whether each runtime actually recovers a final answer after truncation, not merely whether it preserves metadata or shows a warning.

### 4.1 OpenAI Codex: authoritative rollout lifecycle, structured continuation, incomplete-output gap

Official repository: `openai/codex`; fixed commit `49b0aebd6fba2fc590abbe16882cefd048524228` (2026-08-04); Apache-2.0. [LICENSE](https://github.com/openai/codex/blob/49b0aebd6fba2fc590abbe16882cefd048524228/LICENSE#L1-L18)

Codex documents its core turn as repeated sampling: tool requests are executed and their outputs go into the next model request, while an assistant-only response is recorded and completes the turn. The live loop preserves one turn-scoped model client across steps, records pending inputs before the next sample, and decides continuation from locally observed tool work, pending user input, or a server `end_turn: false`. [Turn contract](https://github.com/openai/codex/blob/49b0aebd6fba2fc590abbe16882cefd048524228/codex-rs/core/src/session/turn.rs#L135-L157), [multi-step loop](https://github.com/openai/codex/blob/49b0aebd6fba2fc590abbe16882cefd048524228/codex-rs/core/src/session/turn.rs#L260-L383), [server continuation flag](https://github.com/openai/codex/blob/49b0aebd6fba2fc590abbe16882cefd048524228/codex-rs/core/src/session/turn.rs#L2505-L2549)

Completed tool items are recorded before execution; tool futures return protocol outputs that are also recorded, and `needs_follow_up` forces another sampling step. This is the right separation between a model response boundary and an agent turn boundary. [Tool dispatch and follow-up](https://github.com/openai/codex/blob/49b0aebd6fba2fc590abbe16882cefd048524228/codex-rs/core/src/stream_events_utils.rs#L288-L359), [tool-result persistence](https://github.com/openai/codex/blob/49b0aebd6fba2fc590abbe16882cefd048524228/codex-rs/core/src/session/turn.rs#L2101-L2125)

Visible final text is derived only from a completed agent-message turn item. Whitespace-only text produces no `last_agent_message`; nevertheless, if there are no tools, pending inputs, or server continuation, the outer loop exits. Reasoning is a separate response item, so the cited path does not promote it, but it also does not regenerate a missing visible final. [Final-message extraction](https://github.com/openai/codex/blob/49b0aebd6fba2fc590abbe16882cefd048524228/codex-rs/core/src/stream_events_utils.rs#L236-L284), [completion decision](https://github.com/openai/codex/blob/49b0aebd6fba2fc590abbe16882cefd048524228/codex-rs/core/src/session/turn.rs#L460-L511)

For output exhaustion, the Responses SSE reducer extracts `incomplete_details.reason` but converts every `response.incomplete` into a generic stream error string. The typed `ResponseEvent` exposes `Completed` with usage and `end_turn`, but no structured incomplete reason. Thus `max_output_tokens` is surfaced as failure, not mistaken for an answer, yet the cited runtime does not preserve a normalized `length` state or run a concise final-answer recovery step. [Response event contract](https://github.com/openai/codex/blob/49b0aebd6fba2fc590abbe16882cefd048524228/codex-rs/codex-api/src/common.rs#L75-L105), [incomplete/completed parsing](https://github.com/openai/codex/blob/49b0aebd6fba2fc590abbe16882cefd048524228/codex-rs/codex-api/src/sse/responses.rs#L390-L445)

Codex proactively and mid-turn compacts when its active context reaches the configured threshold, then resumes the interrupted model/tool continuation before accepting new steering input. It also has a shared rollout-token budget that persists weighted usage, injects remaining-budget reminders, and raises `SessionBudgetExceeded` once the limit is crossed. The core `run_turn` loop has no independent generic step counter; its hard boundaries are no-follow-up, cancellation/error, context handling, stop hooks, and the optional rollout budget. [Context rollover and resume](https://github.com/openai/codex/blob/49b0aebd6fba2fc590abbe16882cefd048524228/codex-rs/core/src/session/turn.rs#L371-L457), [rollout budget accounting](https://github.com/openai/codex/blob/49b0aebd6fba2fc590abbe16882cefd048524228/codex-rs/core/src/rollout_budget.rs#L34-L64), [budget stop](https://github.com/openai/codex/blob/49b0aebd6fba2fc590abbe16882cefd048524228/codex-rs/core/src/session/rollout_budget.rs#L8-L35)

Local compaction retries transport failures and progressively removes oldest input on context-window failure. After a completed summary response it takes the last non-empty assistant message, builds replacement history, persists compaction metadata, and warns the user about accuracy loss. The limitation is transactional validation: streamed summary items are recorded into the live conversation before `response.completed`; the accepted summary is not separately checked for output exhaustion because that became a generic stream error, and an absent visible assistant message becomes an empty summary suffix. [Compaction retry](https://github.com/openai/codex/blob/49b0aebd6fba2fc590abbe16882cefd048524228/codex-rs/core/src/compact.rs#L241-L345), [replacement commit](https://github.com/openai/codex/blob/49b0aebd6fba2fc590abbe16882cefd048524228/codex-rs/core/src/compact.rs#L348-L393), [stream item recording](https://github.com/openai/codex/blob/49b0aebd6fba2fc590abbe16882cefd048524228/codex-rs/core/src/compact.rs#L698-L755)

**Borrow:** response-item persistence; tool-result-driven continuation; explicit server continuation; turn-scoped client reuse; context rollover before the next step; durable weighted budget and explicit budget error.

**Do not copy:** collapsing all incomplete reasons to an untyped stream error; treating “no follow-up” as sufficient completion when no visible final exists; recording compaction output into live history before completeness validation.

### 4.2 Pi Agent: safest truncated-tool recovery, but no no-tool final recovery

Official repository: `badlogic/pi-mono`; fixed commit `f119b01cb122ea55e17905caff62d4523f6cce1d` (2026-08-04); MIT. [LICENSE](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/LICENSE#L1-L20)

Pi Agent's core loop streams a typed `AssistantMessage`, persists its final version in context, and treats `stopReason` as distinct metadata. It continues while tool calls or queued steering/follow-up messages exist; hosts can optionally stop after a turn through `shouldStopAfterTurn`, but the core loop itself has no built-in step counter. [Agent loop](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/agent/src/agent-loop.ts#L153-L200), [continuation and host stop hook](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/agent/src/agent-loop.ts#L202-L274), [stream finalization](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/agent/src/agent-loop.ts#L282-L371)

The provider-neutral assistant type preserves `pending`, `stop`, `length`, `toolUse`, `error`, `aborted`, and `deferred` together with `rawStopReason`. Its OpenAI-compatible adapters normalize Chat Completions `length` and Responses `incomplete/max_output_tokens` into that typed `length` state, while retaining unknown provider reasons for diagnostics. [Assistant stop contract](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/ai/src/types.ts#L387-L422), [Chat Completions mapping](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/ai/src/api/openai-completions.ts#L1386-L1408), [Responses incomplete mapping](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/ai/src/api/openai-responses-shared.ts#L742-L769)

Its strongest pattern is output-length handling for tools. If a `length` response contains tool calls, Pi refuses to execute any possibly truncated arguments, creates an error tool result asking the model to re-issue complete calls, and leaves the tool loop active for another sampling step. This is safer than either executing salvaged JSON or terminating the turn. [Length-aware tool branch](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/agent/src/agent-loop.ts#L202-L224), [synthetic error results](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/agent/src/agent-loop.ts#L375-L405)

The no-tool case is different. A no-tool `length` is not `error` or `aborted`, leaves `hasMoreToolCalls` false, and exits after `turn_end`; there is no core tools-disabled request for a concise visible final. The coding-agent layer permits one compact-and-retry only for a recoverable overflow shape, including the condition that reported output is below the model maximum; a genuine output-budget exhaustion therefore does not qualify. [No-tool exit path](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/agent/src/agent-loop.ts#L193-L224), [output-overflow classification](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/ai/src/utils/overflow.ts#L165-L173), [single compaction retry](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/coding-agent/src/core/agent-session.ts#L1985-L2018)

Final visibility and durability are not one atomic boundary across Pi's surfaces. The core replaces the streamed partial with the final assistant object before `message_end`; the newer harness appends that event before notifying its own listeners, while the production coding-agent path notifies extensions/UI before appending the session entry. A retry can remove the failed assistant only from active model context while leaving the historical entry present. Consequently a later successful answer can visually cover a persisted truncated response; UI success alone does not prove clean recovery. [Core finalization](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/agent/src/agent-loop.ts#L314-L370), [harness append order](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/agent/src/harness/agent-harness.ts#L580-L604), [coding-agent append order](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/coding-agent/src/core/agent-session.ts#L633-L657), [retry history gap](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/coding-agent/src/core/agent-session.ts#L2011-L2018)

Pi's compaction path selects a retained tail, summarizes older history, and appends a compaction entry only after the compaction function returns success. The coding-agent defaults reserve 16,384 tokens and retain 20,000 recent tokens, triggering before the context-window remainder is exhausted. The newer harness additionally caps summary output at 80% of `reserveTokens` and keeps explicit file-operation facts beside the summary. [Production trigger defaults](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/coding-agent/src/core/compaction/compaction.ts#L126-L136), [production trigger](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/coding-agent/src/core/compaction/compaction.ts#L232-L238), [cut point and retained tail](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/agent/src/harness/compaction/compaction.ts#L639-L713), [summary budget](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/agent/src/harness/compaction/compaction.ts#L551-L599), [durable compaction append](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/agent/src/harness/agent-harness.ts#L783-L833)

The summary validator still has a critical gap: it rejects only `aborted`/`error` (or only `error` in the production path). A `length` summary, including empty visible content with only thinking, is returned as success and then can be appended as the authoritative compaction checkpoint. Split-turn summaries have the same omission. [Harness summary handling](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/agent/src/harness/compaction/compaction.ts#L592-L614), [split-turn handling](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/agent/src/harness/compaction/compaction.ts#L827-L879), [production summary handling](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/packages/coding-agent/src/core/compaction/compaction.ts#L637-L685)

**Borrow:** typed `stopReason`; never execute token-truncated tool arguments; feed a safe error tool result back so the model can re-issue the call; append durable messages/compaction entries at explicit lifecycle points; reserve summary output budget.

**Do not copy:** allowing a no-tool `length` turn to settle without a visible-final recovery/status; relying on an optional host hook as the only loop bound; accepting summary text without rejecting `length` or empty-visible output.

### 4.3 OpenCode cross-check: strong tool/compaction loop, no output-length final recovery

Official repository: `anomalyco/opencode`; fixed commit `f0afb6750e63ee0a60b052914531bde0afb9bc2b` (2026-08-04); MIT. [LICENSE](https://github.com/anomalyco/opencode/blob/f0afb6750e63ee0a60b052914531bde0afb9bc2b/LICENSE#L1-L20)

OpenCode stores reasoning parts and visible text parts independently. On step completion it persists the normalized provider finish reason and usage, while context overflow is tracked separately. [Reasoning events](https://github.com/anomalyco/opencode/blob/f0afb6750e63ee0a60b052914531bde0afb9bc2b/packages/opencode/src/session/processor.ts#L278-L313), [text events](https://github.com/anomalyco/opencode/blob/f0afb6750e63ee0a60b052914531bde0afb9bc2b/packages/opencode/src/session/processor.ts#L486-L531), [finish and overflow](https://github.com/anomalyco/opencode/blob/f0afb6750e63ee0a60b052914531bde0afb9bc2b/packages/opencode/src/session/processor.ts#L435-L483)

Its outer loop correctly gives tool parts precedence over an unreliable provider finish reason: even when a provider says `stop`, locally present tool calls keep the loop running so results can be sent back. It stops only after a finished assistant has no remaining tool parts. [Tool-aware exit condition](https://github.com/anomalyco/opencode/blob/f0afb6750e63ee0a60b052914531bde0afb9bc2b/packages/opencode/src/session/prompt.ts#L1096-L1130)

However, the normal completion path treats every finish other than `tool-calls` and `unknown` as finished, special-casing only `content-filter`. A no-tool `length` response therefore reaches the next loop iteration and exits whether or not visible text exists. There is no branch that requests a final answer or labels reasoning-only `length` as incomplete. [Completion branch](https://github.com/anomalyco/opencode/blob/f0afb6750e63ee0a60b052914531bde0afb9bc2b/packages/opencode/src/session/prompt.ts#L1288-L1335), [next-iteration exit](https://github.com/anomalyco/opencode/blob/f0afb6750e63ee0a60b052914531bde0afb9bc2b/packages/opencode/src/session/prompt.ts#L1100-L1130)

OpenCode does have an explicit long-turn step-limit prompt: tools are disabled and the model must return a text-only progress summary, remaining tasks, and next steps. This is a good safe-status pattern, but the resulting summary call still has no second guard if it itself terminates with `length`. [Maximum-step final prompt](https://github.com/anomalyco/opencode/blob/f0afb6750e63ee0a60b052914531bde0afb9bc2b/packages/core/src/session/runner/max-steps.ts#L1-L16), [injection at the last step](https://github.com/anomalyco/opencode/blob/f0afb6750e63ee0a60b052914531bde0afb9bc2b/packages/opencode/src/session/prompt.ts#L1178-L1286)

For context pressure, OpenCode reserves output headroom, detects context overflow independently of finish reason, selects recent tail turns, generates a no-tool compaction assistant, then replays the interrupted user turn or injects a hidden continuation instruction. [Headroom calculation](https://github.com/anomalyco/opencode/blob/f0afb6750e63ee0a60b052914531bde0afb9bc2b/packages/opencode/src/session/overflow.ts#L8-L34), [tail selection](https://github.com/anomalyco/opencode/blob/f0afb6750e63ee0a60b052914531bde0afb9bc2b/packages/opencode/src/session/compaction.ts#L188-L239), [overflow replay and auto-continue](https://github.com/anomalyco/opencode/blob/f0afb6750e63ee0a60b052914531bde0afb9bc2b/packages/opencode/src/session/compaction.ts#L289-L348), [continuation](https://github.com/anomalyco/opencode/blob/f0afb6750e63ee0a60b052914531bde0afb9bc2b/packages/opencode/src/session/compaction.ts#L404-L510)

The gap is summary validation: a completed compaction accepts any truthy finish reason, including `length`, and allows extracted summary text to be absent. [Completed-compaction selection](https://github.com/anomalyco/opencode/blob/f0afb6750e63ee0a60b052914531bde0afb9bc2b/packages/opencode/src/session/compaction.ts#L46-L77) Thus OpenCode solves context continuation and tool precedence, but it does not solve reasoning-only output exhaustion or prove that a truncated compaction summary is usable.

**Borrow:** separate context overflow from output truncation; tools override provider `stop`; replay the interrupted user work after successful compaction; use an explicit safe status at a step cap.

**Do not copy:** treating `length` as ordinary completion, or accepting a summary solely because the summarizer emitted some finish reason.

### 4.4 Goose limitation cross-check: warning is not final recovery

Official repository: `aaif-goose/goose`; fixed commit `fe49eb389e62e0dcedf0f138f2295c10fc762c06` (2026-08-04); Apache-2.0. [LICENSE](https://github.com/aaif-goose/goose/blob/fe49eb389e62e0dcedf0f138f2295c10fc762c06/LICENSE#L1-L18)

Goose maps OpenAI-compatible `finish_reason: "length"` into message metadata while keeping `reasoning_content` as a thinking block and cleaned `content` as visible text. [OpenAI response conversion](https://github.com/aaif-goose/goose/blob/fe49eb389e62e0dcedf0f138f2295c10fc762c06/crates/goose-provider-types/src/formats/openai.rs#L682-L730), [metadata preservation](https://github.com/aaif-goose/goose/blob/fe49eb389e62e0dcedf0f138f2295c10fc762c06/crates/goose-provider-types/src/formats/openai.rs#L769-L844)

The agent loop tracks provider content, tool calls, and the output-limit flag separately. A terminal metadata-only message is emitted rather than dropped, and tool requests make the loop continue. [Per-turn state](https://github.com/aaif-goose/goose/blob/fe49eb389e62e0dcedf0f138f2295c10fc762c06/crates/goose/src/agents/agent.rs#L2226-L2242), [content and terminal emission](https://github.com/aaif-goose/goose/blob/fe49eb389e62e0dcedf0f138f2295c10fc762c06/crates/goose/src/agents/agent.rs#L2259-L2339), [tool completion resumes work](https://github.com/aaif-goose/goose/blob/fe49eb389e62e0dcedf0f138f2295c10fc762c06/crates/goose/src/agents/agent.rs#L2680-L2719)

Goose deliberately excludes output-limit responses from its bounded empty-turn retry. With no tools, that path falls through to normal exit rather than requesting a visible final. The CLI renders a warning that the response may be incomplete. [Empty-turn classification](https://github.com/aaif-goose/goose/blob/fe49eb389e62e0dcedf0f138f2295c10fc762c06/crates/goose/src/agents/agent.rs#L2869-L2887), [retry/exit branch](https://github.com/aaif-goose/goose/blob/fe49eb389e62e0dcedf0f138f2295c10fc762c06/crates/goose/src/agents/agent.rs#L2889-L3011), [CLI warning](https://github.com/aaif-goose/goose/blob/fe49eb389e62e0dcedf0f138f2295c10fc762c06/crates/goose-cli/src/session/output.rs#L29-L30), [warning rendering](https://github.com/aaif-goose/goose/blob/fe49eb389e62e0dcedf0f138f2295c10fc762c06/crates/goose-cli/src/session/output.rs#L385-L400)

This is honest status preservation, not final-answer recovery. It avoids channel promotion and a silent blank UI, but reasoning-only `length` still ends without a user-facing answer. The warning must not be mistaken for a recovered model final.

Goose's long-turn cap similarly emits “Would you like me to continue?” after a configurable maximum. In this path the status is yielded as a UI event and assigned to telemetry text, but it is not added to the session conversation in the cited branch; reload durability should not be inferred. [Turn cap](https://github.com/aaif-goose/goose/blob/fe49eb389e62e0dcedf0f138f2295c10fc762c06/crates/goose/src/agents/agent.rs#L2067-L2183), [post-loop handling](https://github.com/aaif-goose/goose/blob/fe49eb389e62e0dcedf0f138f2295c10fc762c06/crates/goose/src/agents/agent.rs#L3130-L3149)

For context-length failure, Goose compacts and retries, with a bounded second failure. Compaction progressively removes tool responses, generates a summary, hides old messages from the agent, adds an agent-only continuation instruction, and resumes either natural conversation or tool work. [Proactive compaction](https://github.com/aaif-goose/goose/blob/fe49eb389e62e0dcedf0f138f2295c10fc762c06/crates/goose/src/agents/agent.rs#L1874-L1959), [recovery compaction](https://github.com/aaif-goose/goose/blob/fe49eb389e62e0dcedf0f138f2295c10fc762c06/crates/goose/src/agents/agent.rs#L2723-L2781), [summary and continuation layout](https://github.com/aaif-goose/goose/blob/fe49eb389e62e0dcedf0f138f2295c10fc762c06/crates/goose/src/context_mgmt/mod.rs#L36-L49), [history replacement](https://github.com/aaif-goose/goose/blob/fe49eb389e62e0dcedf0f138f2295c10fc762c06/crates/goose/src/context_mgmt/mod.rs#L78-L177)

Its structured-summary parser intentionally refuses to repair cut-off JSON and falls back to raw response text so late continuation-critical content is not silently discarded. That is safer than inventing missing fields, but `do_compact` does not reject output-limit metadata or an empty visible summary before replacing history. [Lossless structured fallback](https://github.com/aaif-goose/goose/blob/fe49eb389e62e0dcedf0f138f2295c10fc762c06/crates/goose/src/context_mgmt/structured.rs#L128-L207), [summarizer result handling](https://github.com/aaif-goose/goose/blob/fe49eb389e62e0dcedf0f138f2295c10fc762c06/crates/goose/src/context_mgmt/mod.rs#L319-L418)

**Borrow:** persist an explicit output-limit bit through adapters and UI; keep thinking/text distinct; bounded context-error compaction; lossless fallback instead of repairing truncated structured summaries.

**Do not copy:** using a warning as the only resolution for reasoning-only `length`, accepting truncated/empty compaction output as usable context, or relying on an unpersisted turn-cap event.

## 5. Verified Nexa baseline and exact failure chain

The observations below refer to synchronized baseline commit `6023b4bda4a60dd46289a1c94219e2fae9813124`.

1. The Gemini wire enum matches `{"text": ..., "thought": true}` before ordinary text, extracts true thought parts into `thinking_parts`, extracts ordinary text into `text_parts`, and maps `MAX_TOKENS` to `FinishReason::Length`. [Wire types](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/google.rs#L36-L60), [finish mapping](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/google.rs#L268-L282), [extraction](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/google.rs#L666-L812)
2. The shared OpenAI-compatible stream parser, used by DeepSeek, deserializes `content` and multiple reasoning field aliases separately, maps `length` to `FinishReason::Length`, and emits a chunk when any of text, finish reason, usage, or reasoning exists. Thus a content-less terminal DeepSeek chunk is not discarded. It does not enumerate DeepSeek's `insufficient_system_resource`, so that provider reason currently collapses to `Other`; this is an independent loss of retry classification, not the cause of the reasoning leak. [Wire delta types](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/streaming.rs#L22-L48), [finish mapping](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/streaming.rs#L232-L244), [separate terminal emission](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/streaming.rs#L650-L716)
3. The streaming model step then accumulates ordinary deltas into `full_content`, thoughts into `iteration_thinking`, and retains the last finish reason without provider-specific branching. [Model-step output contract](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/agent/model_step.rs#L22-L38), [stream accumulation](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/agent/model_step.rs#L460-L485), [completed output](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/agent/model_step.rs#L739-L754)
4. The semantic corruption occurs afterward: when `full_content` is blank, reasoning is present, and there are no tools, `turn_loop.rs` copies all reasoning into `full_content`. The fallback does not inspect provider, native-vs-parsed reasoning provenance, or finish reason. It therefore explains both Gemini and DeepSeek terminal leaks. [Promotion fallback](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/agent/turn_loop.rs#L930-L938)
5. The assistant message then uses `full_content` as ordinary text while retaining reasoning separately. Finalization persists `final_text` as `ConversationMessage.content` and reasoning as `ConversationMessage.thinking`. This can store identical reasoning in both fields and make reload/UI treat the copy as a reply. [Message construction](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/agent/turn_loop.rs#L940-L955), [final persistence](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/agent/finalization.rs#L254-L267)
6. DeepSeek replay is a separate concern. The profile enables reasoning-history preservation and synthetic fallback for direct DeepSeek reasoning models; request conversion replays real assistant reasoning or substitutes `[reasoning content unavailable in local history]`. Tests cover actual reasoning and tool calls. [DeepSeek reasoning profile](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/reasoning_profile.rs#L398-L422), [history serialization](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/openai.rs#L746-L769), [tool replay tests](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/openai.rs#L1809-L1889)

This confirms two findings. The user-visible reasoning leak is caused by one shared agent fallback after correct provider separation. Independently, real DeepSeek tool reasoning is already replayed when present, but synthetic placeholder replay does not satisfy the documented semantic requirement to pass back the full chain and should remain a migration/error case rather than normal operation.

## 6. Hotfix semantics for Nexa

The following is **Nexa design inference**, derived from the verified contracts above.

### 6.1 Terminal decision table

| Provider shape | Ordinary answer | Reasoning | Tools | Finish reason | Runtime result |
| --- | --- | --- | --- | --- | --- |
| Any | Non-empty | Any | None | Natural stop | Complete normally; persist answer and reasoning separately. |
| DeepSeek/Gemini | Any | Any | Present | Tool-call reason or call parts | Continue the tool loop; do not apply empty-answer handling. Preserve any provider replay state. |
| Gemini | Empty | Non-empty | None | `MAX_TOKENS` | Incomplete because the output budget ended before an answer. Preserve reasoning and finish metadata; never promote. |
| DeepSeek | Null/empty | Non-empty | None | `length` | Same incomplete state as Gemini after normalization; never promote. |
| Any | Empty | Non-empty | None | `STOP` / `stop` | Provider empty-final anomaly. At most one bounded recovery attempt; otherwise explicit incomplete/error. Never promote. |
| Any | Empty | Any | None | Content filter | Preserve the filter reason and surface a safe filter/error state. Do not retry as an ordinary answer. |
| DeepSeek | Empty | Any | None | `insufficient_system_resource` | Explicit retryable provider failure under bounded policy; never treat reasoning as completion. |
| Any | Empty | Empty | None | Other/unknown | Explicit incomplete/error with provider metadata; never fabricate content. |

### 6.2 Cross-provider turn state machine

The runtime should persist completion state explicitly rather than infer it from whether a string exists. The minimum state carried across provider, agent, persistence, and UI boundaries is:

```text
visible_answer       ordinary answer text only
reasoning            native reasoning/thought or parsed tags, with provenance
tool_calls           pending/completed calls plus provider replay metadata
finish_reason_raw    immutable provider value
finish_reason        normalized value
usage                including output/reasoning counts when available
context_state        within_limit | overflow | compacting | compacted
completion_state     streaming | tool_pending | output_continuation |
                     complete | incomplete_length | provider_empty |
                     filtered | retryable_provider_error | needs_user_continue
recovery_progress    visible prefix plus bounded anomaly counters by class
```

The transition order matters:

1. **Tool calls first.** If executable tool calls exist, persist the assistant subturn and exact provider replay state, execute tools, append results, and start the next model step. A provider `stop` must not override locally observed calls, following the primary Codex/Pi continuation pattern. If the same assistant subturn ended with `length`, do not execute possibly truncated arguments; append a safe error tool result and let the model re-issue the call, as Pi does.
2. **Context failure second.** Context overflow may trigger one transactional compaction/retry. Do not confuse it with output `length` merely because both mention tokens.
3. **Visible natural final.** With no tools, non-empty visible answer plus natural stop becomes `complete`.
4. **Output limit.** Any `length` response enters `output_continuation`; it does not end the user turn. Preserve partial answer and reasoning separately, reserve the answer channel on every later model step, and join visible continuation fragments into one durable reply. Do not impose an output-limit-specific retry count: cancellation, the user-configured turn iteration boundary, context handling, and verified forward progress remain the enclosing safety controls. Tools may still be used while the task is unfinished, but a tool call emitted by the same `length` response is untrusted and must be rejected so the model can re-issue complete arguments.
5. **Empty natural stop.** No answer plus `stop` is `provider_empty`, eligible for one bounded retry. Reasoning does not change that classification.
6. **Filters/resources.** Content filters are terminal safe errors. Resource exhaustion is a separately bounded retryable provider failure.
7. **Step cap.** Persist a runtime-owned `needs_user_continue` status containing accomplished work, remaining work, and a continue action. It must be durable and typed, not a transient UI event or model reasoning copied into content.

Compaction needs its own two-phase commit:

```text
summarize old context
  -> validate: visible summary non-empty, no tools, no error,
               finish is natural stop, output_limit is false
  -> commit: hide/archive old context and install summary
  -> inject hidden continuation/replay original interrupted request
```

If validation fails, keep the original history authoritative and surface a compaction failure. Codex and Pi both provide useful compaction/continuation mechanics, while OpenCode and Goose corroborate the pattern; none of the cited paths imposes all of these summary-completeness and atomic-commit checks.

### 6.3 Minimal hotfix versus follow-up hardening

For the immediate hotfix, the current accumulators already prove the critical boundary:

```text
answer_present = !full_content.trim().is_empty()
reasoning_present = !iteration_thinking.trim().is_empty()
tools_present = !tool_calls.is_empty()
finish_reason = last_finish_reason
```

Remove the reasoning-to-answer assignment and route an output-limited response into a provider-neutral continuation state. Keep that state active across later tool rounds until a non-empty ordinary answer reaches a natural terminal; partial answer fragments must stream and persist as one reply. If the configured turn boundary is reached first, emit a typed safe status containing no model reasoning. Normal DeepSeek `content: "" + reasoning_content + tool_calls` subturns continue, while tool calls attached to a `length` terminal are rejected as potentially truncated.

Longer-term hardening should carry explicit provenance such as `answer_delta_seen`, `reasoning_delta_seen`, and `reasoning_source` (`native`, parsed tags, or none) through finalization. DeepSeek tool subturns should additionally track whether exact replay-required reasoning is available; a synthetic placeholder should be observable and should not silently qualify as complete protocol history. These additions make tests and migration of already-polluted history unambiguous, but they are not prerequisites for stopping the visible leak.

### 6.4 Executable cross-provider regression matrix

Fixtures should run at adapter/reducer, agent state, request replay, persistence, and UI boundaries. The wire snippets below are intentionally small enough for deterministic mock-SSE or deserialization tests.

| ID | Input fixture | Required assertions |
| --- | --- | --- |
| G1 | Gemini non-stream/stream parts contain only `{"text":"r","thought":true}` and terminal `MAX_TOKENS`. | Reasoning is `r`; visible delta/content is empty; reason normalizes to `length`; no tools; agent result is incomplete; persisted `content` is never `r`. |
| G2 | Gemini thought part `r`, ordinary text part `answer`, terminal `STOP`. | Reasoning is `r`; reply is exactly `answer`; both remain separated after database reload. |
| G3 | Gemini thought-only plus terminal `STOP`. | Provider empty-final anomaly, not success; no promotion and at most the configured bounded recovery. |
| G4 | Gemini thought plus function-call part, no visible answer. | Tool call is emitted and executed; empty-answer handling is not entered; reasoning and thought signature remain non-visible protocol state. |
| D1 | DeepSeek non-stream `{"message":{"content":null,"reasoning_content":"r"},"finish_reason":"length"}`. | Deserialization accepts nullable content; reasoning is `r`; reply is empty; result is incomplete/length; no promotion. |
| D2 | Same as D1 with `content:""`. | Identical semantics to null; no accidental branch difference. |
| D3 | DeepSeek SSE reasoning deltas `r1`, `r2`, followed by a content-less terminal chunk with `finish_reason:"length"` and usage. | Reasoning joins to `r1r2`; visible content remains empty; terminal reason and usage reach the reducer; result is incomplete. |
| D4 | DeepSeek SSE reasoning followed by visible `answer` and terminal `stop`. | Channels remain separate; reply is exactly `answer`; no duplicated reasoning. |
| D5 | DeepSeek reasoning-only, no tools, terminal `stop`. | Explicit provider empty-final anomaly; not a completed reply. |
| D6 | DeepSeek assistant subturn `{"content":"","reasoning_content":"r","tool_calls":[...]}` with `finish_reason:"tool_calls"`. | Tool execution continues. The next request contains that assistant's exact empty content, exact `reasoning_content`, and exact tool calls before the tool result. |
| D7 | D6 replay with `reasoning_content` deleted. | Mock server/contract validator rejects the request as 400-equivalent; test proves the adapter cannot silently omit mandatory history. |
| D8 | D6 replay with a fabricated placeholder instead of `r`. | Test marks replay as degraded/invalid for normal operation; it must not pass the exact-history assertion merely because a real server may accept the field syntactically. |
| D9 | No-tool ordinary DeepSeek turn followed by a new user message, with prior reasoning omitted. | Request remains valid under the documented no-tool rule; omission is not treated as an error. |
| D10 | DeepSeek thinking disabled. | No reasoning replay requirement is imposed and no placeholder is synthesized. |
| X1 | Any provider emits finish/usage metadata with no text and no reasoning. | Reducer retains metadata without `choices[0]`/empty-array failure and without fabricating reply text. |
| X2 | Local or OpenAI-compatible `<think>r</think>` with no ordinary final and no tools. | Parsed reasoning follows the same shared no-promotion rule, while provenance records that it came from tags rather than a native field. |
| X3 | Live reasoning block -> terminal event -> persistence -> reload/UI. | Reasoning never becomes ordinary reply content, TTS input, quote text, copy target, or duplicated transcript content. |
| A1 | No-tool turn returns partial visible answer plus `length`, then a second response returns the remainder with natural stop. | Continue the same turn, stream both fragments once, and persist their concatenation as one complete reply. |
| A2 | No-tool turn returns reasoning only plus `length`, then uses a tool before returning a visible natural-stop answer. | Neither reasoning value is promoted; answer-channel reservation stays active through the tool round and ends only when the visible final arrives. |
| A3 | Provider reports `stop`, but the same assistant turn contains executable tool parts. | Tool parts take precedence over finish metadata; the exact assistant/tool history is persisted and execution continues. |
| A4 | One or more tool subturns are followed by non-empty visible text and natural stop. | Only the last visible text is a complete final; earlier reasoning/tool protocol state remains non-visible and replayable. |
| A5 | Agent reaches its configured step cap. | Persist a runtime-owned `needs_user_continue` status with accomplished and remaining work; reload preserves the status; no reasoning or transient UI toast is treated as the final. |
| A6 | Assistant contains tool calls but terminates with `length`. | Do not execute any possibly truncated arguments; append one error result per call, preserve IDs/order, and let the next model step re-issue complete calls. |
| A7 | Responses-style provider emits `response.incomplete` with `reason: max_output_tokens`. | Preserve the raw reason and normalize to `incomplete_length`; do not collapse it to a generic transport error or mark the turn complete. |
| C1 | Context overflow; compactor returns non-empty visible summary, no tools, natural stop, and no output-limit metadata. | Commit the summary transactionally, retain an audit link to archived history, inject hidden continuation, and replay the interrupted work exactly once. |
| C2 | Compactor returns visible text with `length`/output-limit metadata. | Reject the summary as authoritative; the original history stays active and no hidden continuation is injected. |
| C3 | Compactor returns reasoning-only, empty visible text, or a tool request. | Reject compaction; never promote reasoning or execute summarizer tools. |
| C4 | Context retry/compaction fails again after the configured bound. | Persist an explicit compaction failure and stop; do not loop or silently drop old turns. |
| C5 | Structured compaction JSON is cut off mid-object. | Raw text may be retained for diagnostics, but it cannot replace history unless the enclosing response also passes the completeness checks. |
| R1 | Process exits during `tool_pending`, `output_continuation`, or `needs_user_continue`, then reloads. | The same typed state, visible prefix, and anomaly counters resume without duplicate tool execution, duplicate continuation, or loss of the user-visible status. |

For D6, the request-history assertion should compare a structured object, not just string containment:

```json
{
  "role": "assistant",
  "content": "",
  "reasoning_content": "r",
  "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "lookup", "arguments": "{}"}}]
}
```

The final tool-flow success fixture should then return a later assistant message with non-empty `content` and no tool calls, proving that the shared hotfix does not prematurely terminate a valid DeepSeek chain.

## 7. License and integration boundary

| Source | Fixed revision | License verified | How it is used here |
| --- | --- | --- | --- |
| `googleapis/googleapis` protocol | `f3ff3a1dc91aa7719f98437416fd686fad0296cd` | Apache-2.0 in source headers and repository [LICENSE](https://github.com/googleapis/googleapis/blob/f3ff3a1dc91aa7719f98437416fd686fad0296cd/LICENSE#L1-L7) | Authoritative wire-field and finish-reason semantics. |
| Google Gen AI JS SDK | `e728fad9599af298f515329fcb92f5f122e110cf` | Apache-2.0 [LICENSE](https://github.com/googleapis/js-genai/blob/e728fad9599af298f515329fcb92f5f122e110cf/LICENSE#L1-L13) | Authoritative first-party answer extraction behavior. |
| DeepSeek hosted API docs | Live page checked 2026-08-04; no public source revision | Public documentation, not source imported into Nexa | Authoritative hosted V4 wire and replay contract; cited, not copied into runtime. |
| `deepseek-ai/DeepSeek-V3` | `9b4e9788e4a3a731f7567338ed15d3ec549ce03b` | Code is MIT under [LICENSE-CODE](https://github.com/deepseek-ai/DeepSeek-V3/blob/9b4e9788e4a3a731f7567338ed15d3ec549ce03b/LICENSE-CODE#L1-L9); weights have a separate [DeepSeek Model License](https://github.com/deepseek-ai/DeepSeek-V3/blob/9b4e9788e4a3a731f7567338ed15d3ec549ce03b/LICENSE-MODEL#L1-L12) | Scope check only. This model/inference repository does not prove hosted Chat API behavior and is not integrated. |
| LiteLLM Gemini snapshot | `a79f598f692e66ce49790bfda699b7f4dccb3ca0` | MIT outside `enterprise/`; boundary stated in [LICENSE](https://github.com/BerriAI/litellm/blob/a79f598f692e66ce49790bfda699b7f4dccb3ca0/LICENSE#L1-L15) | Positive Gemini adapter/reducer precedent; no source copied. |
| LiteLLM DeepSeek snapshot | `956d5177d1d915adc8084c142d9d2babad1ff7af` | Same MIT/non-enterprise boundary [LICENSE](https://github.com/BerriAI/litellm/blob/956d5177d1d915adc8084c142d9d2babad1ff7af/LICENSE#L1-L15) | Positive channel separation plus replay-fallback comparison; no source copied. |
| LangChain Google | `942b7cc4bc0abe750b6350379e5df870db19d9ee` | MIT [LICENSE](https://github.com/langchain-ai/langchain-google/blob/942b7cc4bc0abe750b6350379e5df870db19d9ee/LICENSE#L1-L18) | Structured empty/thinking-message precedent; no source copied. |
| LangChain DeepSeek | `1a43a6e14ab2c8ff6e4fac250941e9568926e1b4` | MIT [LICENSE](https://github.com/langchain-ai/langchain/blob/1a43a6e14ab2c8ff6e4fac250941e9568926e1b4/LICENSE#L1-L18) | Ingestion precedent and outbound replay-gap analysis; no source copied. |
| Continue | `5522c6f44ca0ac3528b37244818fbfa39b5af470` | Apache-2.0 [LICENSE](https://github.com/continuedev/continue/blob/5522c6f44ca0ac3528b37244818fbfa39b5af470/LICENSE#L1-L7) | Counterexample only; do not copy its current Gemini terminal behavior. |
| OpenAI Codex | `49b0aebd6fba2fc590abbe16882cefd048524228` | Apache-2.0 [LICENSE](https://github.com/openai/codex/blob/49b0aebd6fba2fc590abbe16882cefd048524228/LICENSE#L1-L18) | Primary rollout, tool continuation, persistence, compaction, and token-budget reference; no source copied. |
| Pi Agent (`badlogic/pi-mono`) | `f119b01cb122ea55e17905caff62d4523f6cce1d` | MIT [LICENSE](https://github.com/badlogic/pi-mono/blob/f119b01cb122ea55e17905caff62d4523f6cce1d/LICENSE#L1-L20) | Primary typed stop-reason, truncated-tool recovery, session, and compaction reference; no source copied. |
| OpenCode | `f0afb6750e63ee0a60b052914531bde0afb9bc2b` | MIT [LICENSE](https://github.com/anomalyco/opencode/blob/f0afb6750e63ee0a60b052914531bde0afb9bc2b/LICENSE#L1-L20) | Agent-loop, tool precedence, step-cap, and context-compaction precedent; no source copied. |
| Goose | `fe49eb389e62e0dcedf0f138f2295c10fc762c06` | Apache-2.0 [LICENSE](https://github.com/aaif-goose/goose/blob/fe49eb389e62e0dcedf0f138f2295c10fc762c06/LICENSE#L1-L18) | Output-limit metadata, warning, turn-cap, and compaction comparison; no source copied. |

All cited integration code is under permissive licenses for the referenced non-enterprise files, but this note recommends semantic reimplementation against Nexa's own provider/runtime types, not vendoring source. The DeepSeek model-weight license is a distinct boundary and irrelevant to this adapter hotfix. If code is later copied rather than independently implemented, preserve the upstream copyright and license notices required by the relevant license.

## 8. Confidence and limits

- **Confirmed:** Google's channel marker and official SDK exclude thought parts from answer extraction.
- **Confirmed from the live official contract:** DeepSeek V4 returns nullable `content` separately from `reasoning_content`; `length` means the requested maximum was reached; tool flows require full reasoning replay or return 400.
- **Confirmed:** Nexa baseline separates both Gemini and DeepSeek channels until `turn_loop.rs`, where a shared fallback copies reasoning into reply content.
- **Confirmed:** Gemini `MAX_TOKENS` and DeepSeek `length` both normalize to `FinishReason::Length`, but the promotion fallback ignores the terminal reason.
- **Confirmed:** DeepSeek `insufficient_system_resource` currently normalizes to generic `Other`, so a provider-specific retry policy needs an enum/metadata follow-up even after the leak is fixed.
- **Confirmed:** Nexa replays real DeepSeek reasoning when it exists; its legacy placeholder prevents a missing field but is not an exact replay of the provider chain.
- **Confirmed by fixed-source inspection:** LiteLLM keeps the channels separate but uses a lossy replay fallback; LangChain keeps inbound channels separate but its cited DeepSeek path does not serialize `additional_kwargs.reasoning_content` back out.
- **Confirmed by fixed-source inspection:** Codex records response/tool items and continues on tool or server follow-up, but maps `response.incomplete` to an untyped stream error and can end with no non-empty `last_agent_message`; it does not promote reasoning or recover a visible final in the cited paths.
- **Confirmed by fixed-source inspection:** Pi preserves typed and raw stop reasons and safely re-issues length-truncated tool calls, but a genuine no-tool output-budget exhaustion does not qualify for its one compaction retry and settles without final regeneration.
- **Confirmed lifecycle limitation:** Pi's production coding-agent path may notify extensions/UI before the assistant message is durably appended, and its retry path can leave a truncated historical entry even when active model context removes it.
- **Confirmed by secondary fixed-source inspection:** OpenCode and Goose separate thinking, ordinary text, tools, and terminal/context metadata, but neither cited runtime automatically regenerates a visible final after a no-tool output-limit response.
- **Confirmed, but only UI/status behavior:** Goose warns that the response may be incomplete; that warning is not a recovered model final. Its cited maximum-turn prompt is emitted to the active UI path, not persisted as a conversation message.
- **Confirmed compaction gap across all four agents:** Codex, Pi, OpenCode, and Goose have useful resume/retained-tail mechanics, but their cited paths do not reject every `length`, output-limited, empty-visible, or pre-commit partial summary before accepting or recording continuation context.
- **Highly plausible, not confirmed from the original screenshots alone:** the reported production turns exhausted output budget. Verify provider trace/persisted finish reason for each incident.
- **Time-sensitive:** DeepSeek hosted docs have no public commit-pinned server/SDK source and can change. Re-check them when provider catalog models or endpoint semantics change.
- **Not established by primary sources:** that a fixed arithmetic answer reserve is valid for every Gemini model, or that one particular recent Nexa change first caused the incident.

The hotfix should therefore enforce the channel invariant once in the shared agent loop, keep tool flow ahead of empty-answer handling, and treat DeepSeek exact replay, budget tuning, and historical-data repair as separately testable concerns.
