# Reasoning-only terminal responses across Gemini and DeepSeek: primary-source notes

Date: 2026-08-04

This note validates the hotfix request in `D:\Nexa.txt`, including the supplementary DeepSeek report, against the synchronized Nexa baseline, Google and DeepSeek primary sources, and fixed source snapshots from leading open-source LLM integrations. It is an engineering input, not an implementation record. Statements labeled **Nexa inference** are design conclusions, not claims made by the providers or upstream projects.

## Executive decision

1. **Reasoning and visible answer are separate provider channels.** Gemini marks reasoning with `Part.thought`; DeepSeek returns `reasoning_content` beside nullable `content`. Google's official SDK excludes thought parts from its answer getter, while DeepSeek describes `reasoning_content` as content emitted before the final answer. A reasoning-only response therefore has no visible answer to promote. [Google thinking guide](https://ai.google.dev/gemini-api/docs/generate-content/thinking#thought-summaries), [Google SDK answer extractor](https://github.com/googleapis/js-genai/blob/e728fad9599af298f515329fcb92f5f122e110cf/src/types.ts#L3717-L3754), [DeepSeek response schema](https://api-docs.deepseek.com/api/create-chat-completion/#responses)
2. **Finish reason is terminal metadata, not content.** Google defines `MAX_TOKENS`; DeepSeek defines `length`, `content_filter`, `tool_calls`, and `insufficient_system_resource` in addition to `stop`. None grants permission to reclassify reasoning as the answer. [Google protocol enum](https://github.com/googleapis/googleapis/blob/f3ff3a1dc91aa7719f98437416fd686fad0296cd/google/cloud/aiplatform/v1/content.proto#L709-L725), [DeepSeek finish reasons](https://api-docs.deepseek.com/api/create-chat-completion/#responses), [LiteLLM normalization](https://github.com/BerriAI/litellm/blob/a79f598f692e66ce49790bfda699b7f4dccb3ca0/litellm/litellm_core_utils/core_helpers.py#L90-L108)
3. **A terminal chunk may validly carry no answer text.** Both protocols permit nullable/absent visible content, and DeepSeek's official streaming example accumulates `reasoning_content` and `content` independently. Reducers must preserve an empty answer together with reasoning, finish reason, usage, and tools; storage/UI must represent incomplete/error without changing channels. [DeepSeek streaming example](https://api-docs.deepseek.com/guides/thinking_mode/#multi-turn-conversation), [LiteLLM content-less chunk handling](https://github.com/BerriAI/litellm/blob/a79f598f692e66ce49790bfda699b7f4dccb3ca0/litellm/llms/vertex_ai/gemini/vertex_and_google_ai_studio_gemini.py#L3168-L3192), [LangChain Google empty-message handling](https://github.com/langchain-ai/langchain-google/blob/942b7cc4bc0abe750b6350379e5df870db19d9ee/libs/genai/langchain_google_genai/chat_models.py#L1431-L1456)
4. **The visible leak has one provider-agnostic root cause in Nexa.** Both Gemini and the OpenAI-compatible/DeepSeek stream parser already separate ordinary text from reasoning and retain terminal metadata. Later, the agent loop copies `iteration_thinking` into `full_content` whenever visible content is empty and there are no tool calls. That shared fallback explains both reports. [Nexa OpenAI-compatible separation](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/streaming.rs#L31-L48), [stream emission](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/streaming.rs#L660-L716), [promotion fallback](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/agent/turn_loop.rs#L930-L938)
5. **DeepSeek also has an independent tool-replay contract.** When a request carries tools, official docs require the full `reasoning_content` to be passed back in all subsequent requests or the API returns HTTP 400. A valid tool subturn can have `content: ""`, non-empty reasoning, and tool calls. The shared terminal hotfix must keep tools higher priority than empty-answer handling, and the adapter must preserve exact reasoning history rather than inventing a visible answer. [DeepSeek tool-call contract and examples](https://api-docs.deepseek.com/guides/thinking_mode/#tool-calls)

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

## 4. Verified Nexa baseline and exact failure chain

The observations below refer to synchronized baseline commit `6023b4bda4a60dd46289a1c94219e2fae9813124`.

1. The Gemini wire enum matches `{"text": ..., "thought": true}` before ordinary text, extracts true thought parts into `thinking_parts`, extracts ordinary text into `text_parts`, and maps `MAX_TOKENS` to `FinishReason::Length`. [Wire types](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/google.rs#L36-L60), [finish mapping](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/google.rs#L268-L282), [extraction](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/google.rs#L666-L812)
2. The shared OpenAI-compatible stream parser, used by DeepSeek, deserializes `content` and multiple reasoning field aliases separately, maps `length` to `FinishReason::Length`, and emits a chunk when any of text, finish reason, usage, or reasoning exists. Thus a content-less terminal DeepSeek chunk is not discarded. It does not enumerate DeepSeek's `insufficient_system_resource`, so that provider reason currently collapses to `Other`; this is an independent loss of retry classification, not the cause of the reasoning leak. [Wire delta types](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/streaming.rs#L22-L48), [finish mapping](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/streaming.rs#L232-L244), [separate terminal emission](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/streaming.rs#L650-L716)
3. The streaming model step then accumulates ordinary deltas into `full_content`, thoughts into `iteration_thinking`, and retains the last finish reason without provider-specific branching. [Model-step output contract](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/agent/model_step.rs#L22-L38), [stream accumulation](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/agent/model_step.rs#L460-L485), [completed output](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/agent/model_step.rs#L739-L754)
4. The semantic corruption occurs afterward: when `full_content` is blank, reasoning is present, and there are no tools, `turn_loop.rs` copies all reasoning into `full_content`. The fallback does not inspect provider, native-vs-parsed reasoning provenance, or finish reason. It therefore explains both Gemini and DeepSeek terminal leaks. [Promotion fallback](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/agent/turn_loop.rs#L930-L938)
5. The assistant message then uses `full_content` as ordinary text while retaining reasoning separately. Finalization persists `final_text` as `ConversationMessage.content` and reasoning as `ConversationMessage.thinking`. This can store identical reasoning in both fields and make reload/UI treat the copy as a reply. [Message construction](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/agent/turn_loop.rs#L940-L955), [final persistence](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/agent/finalization.rs#L254-L267)
6. DeepSeek replay is a separate concern. The profile enables reasoning-history preservation and synthetic fallback for direct DeepSeek reasoning models; request conversion replays real assistant reasoning or substitutes `[reasoning content unavailable in local history]`. Tests cover actual reasoning and tool calls. [DeepSeek reasoning profile](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/reasoning_profile.rs#L398-L422), [history serialization](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/openai.rs#L746-L769), [tool replay tests](https://github.com/MLGBJDLW/Nexa/blob/6023b4bda4a60dd46289a1c94219e2fae9813124/crates/core/src/llm/openai.rs#L1809-L1889)

This confirms two findings. The user-visible reasoning leak is caused by one shared agent fallback after correct provider separation. Independently, real DeepSeek tool reasoning is already replayed when present, but synthetic placeholder replay does not satisfy the documented semantic requirement to pass back the full chain and should remain a migration/error case rather than normal operation.

## 5. Hotfix semantics for Nexa

The following is **Nexa design inference**, derived from the verified contracts above.

### 5.1 Terminal decision table

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

### 5.2 Minimal hotfix versus follow-up hardening

For the immediate hotfix, the current accumulators already prove the critical boundary:

```text
answer_present = !full_content.trim().is_empty()
reasoning_present = !iteration_thinking.trim().is_empty()
tools_present = !tool_calls.is_empty()
finish_reason = last_finish_reason
```

Remove the reasoning-to-answer assignment and route a no-tool terminal reasoning-only result to an incomplete/error path. If database/UI contracts cannot represent an empty assistant message today, emit a typed safe status or synthetic explanation that contains no model reasoning; do not write reasoning into `content`. Keep `tools_present` as the earlier branch so valid DeepSeek `content: "" + reasoning_content + tool_calls` subturns continue normally.

Longer-term hardening should carry explicit provenance such as `answer_delta_seen`, `reasoning_delta_seen`, and `reasoning_source` (`native`, parsed tags, or none) through finalization. DeepSeek tool subturns should additionally track whether exact replay-required reasoning is available; a synthetic placeholder should be observable and should not silently qualify as complete protocol history. These additions make tests and migration of already-polluted history unambiguous, but they are not prerequisites for stopping the visible leak.

### 5.3 Executable cross-provider regression matrix

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

## 6. License and integration boundary

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

All cited integration code is under permissive licenses for the referenced non-enterprise files, but this note recommends semantic reimplementation against Nexa's own provider/runtime types, not vendoring source. The DeepSeek model-weight license is a distinct boundary and irrelevant to this adapter hotfix. If code is later copied rather than independently implemented, preserve the upstream copyright and license notices required by the relevant license.

## 7. Confidence and limits

- **Confirmed:** Google's channel marker and official SDK exclude thought parts from answer extraction.
- **Confirmed from the live official contract:** DeepSeek V4 returns nullable `content` separately from `reasoning_content`; `length` means the requested maximum was reached; tool flows require full reasoning replay or return 400.
- **Confirmed:** Nexa baseline separates both Gemini and DeepSeek channels until `turn_loop.rs`, where a shared fallback copies reasoning into reply content.
- **Confirmed:** Gemini `MAX_TOKENS` and DeepSeek `length` both normalize to `FinishReason::Length`, but the promotion fallback ignores the terminal reason.
- **Confirmed:** DeepSeek `insufficient_system_resource` currently normalizes to generic `Other`, so a provider-specific retry policy needs an enum/metadata follow-up even after the leak is fixed.
- **Confirmed:** Nexa replays real DeepSeek reasoning when it exists; its legacy placeholder prevents a missing field but is not an exact replay of the provider chain.
- **Confirmed by fixed-source inspection:** LiteLLM keeps the channels separate but uses a lossy replay fallback; LangChain keeps inbound channels separate but its cited DeepSeek path does not serialize `additional_kwargs.reasoning_content` back out.
- **Highly plausible, not confirmed from the original screenshots alone:** the reported production turns exhausted output budget. Verify provider trace/persisted finish reason for each incident.
- **Time-sensitive:** DeepSeek hosted docs have no public commit-pinned server/SDK source and can change. Re-check them when provider catalog models or endpoint semantics change.
- **Not established by primary sources:** that a fixed arithmetic answer reserve is valid for every Gemini model, or that one particular recent Nexa change first caused the incident.

The hotfix should therefore enforce the channel invariant once in the shared agent loop, keep tool flow ahead of empty-answer handling, and treat DeepSeek exact replay, budget tuning, and historical-data repair as separately testable concerns.
