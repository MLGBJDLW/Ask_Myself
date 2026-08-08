# Workspace runtime primary-source research

Status: proposed architecture guidance

Date checked: 2026-08-08

Scope: provider-hosted web search, Project workspace/event-claim continuity, and event-driven companion UI

## Executive decision

Nexa should build these features on two shared foundations rather than as three unrelated UI additions:

1. a provider-capability registry plus dialect adapters for hosted web search; and
2. a durable, project-scoped event store whose reviewed projections feed Project brief, claim graph, task UI, recovery UI, and an optional companion.

The provider adapter must preserve each wire contract. `web_search` is not one portable OpenAI-compatible feature:

- OpenAI Responses, Anthropic Messages, Gemini `generateContent`, Gemini Interactions, and DeepSeek Responses expose different request controls, streaming shapes, result blocks, and citation guarantees;
- DeepSeek documents server-side search on `/responses` as of this review, while `/chat/completions` still documents function tools only;
- Nexa's existing `SearchPlanItem::Native` means direct public search-engine access, not model-provider-hosted search. The new capability must not reuse that name.

For Project continuity, append immutable observations first, derive reviewable events/claims second, and materialize concise read models last. A background model must never silently rewrite the Project brief or accepted facts. The companion is only a projection of durable lifecycle events; it is not another agent and must never replace the primary approval/input UI.

## Research method and source policy

Provider protocol claims below come from current official API documentation. Repository implementation claims use immutable GitHub commit URLs. No upstream asset, prompt, or implementation is proposed for copying. Licenses are recorded to establish the integration boundary, not to replace Nexa's normal legal review.

Reviewed repository revisions:

| Project | Revision | License | Why reviewed |
| --- | --- | --- | --- |
| OpenHands | [`4470813`](https://github.com/All-Hands-AI/OpenHands/tree/4470813ce58f5ac384e3d367d34518e10106526b) | [MIT](https://github.com/All-Hands-AI/OpenHands/blob/4470813ce58f5ac384e3d367d34518e10106526b/LICENSE) | persistent/live event separation, pagination, event-store isolation, workspace binding |
| LangGraph | [`fde3068`](https://github.com/langchain-ai/langgraph/tree/fde3068970679184b68d3d068a92c83c966a4888) | [MIT](https://github.com/langchain-ai/langgraph/blob/fde3068970679184b68d3d068a92c83c966a4888/LICENSE) | checkpoint identity, pending writes, hierarchical project stores |
| Microsoft GraphRAG | [`14a00ad`](https://github.com/microsoft/graphrag/tree/14a00ad88fc33cf2b52f4f113f25807556f8e25e) | [MIT](https://github.com/microsoft/graphrag/blob/14a00ad88fc33cf2b52f4f113f25807556f8e25e/LICENSE) | evidence-linked claims/covariates and temporal claim extraction |
| OpenAI Codex | [`e734a1a`](https://github.com/openai/codex/tree/e734a1a5c1c6e51d7a28ec8ec6381d7ffb18e23b) | [Apache-2.0](https://github.com/openai/codex/blob/e734a1a5c1c6e51d7a28ec8ec6381d7ffb18e23b/LICENSE) | lifecycle-to-companion state projection and untrusted asset-pack validation |

## 1. Provider-hosted web search

### 1.1 Primary-source protocol matrix

| Provider surface | Request and controls | Output and citations | Streaming and orchestration |
| --- | --- | --- | --- |
| OpenAI Responses | Official [web-search guide](https://developers.openai.com/api/docs/guides/tools-web-search): `tools: [{"type":"web_search"}]`; supports allowed/blocked domain filters, approximate user location, external-web/cache control, and optional included source/result data. `web_search_preview` is a legacy surface with fewer controls. | A `web_search_call` output item records `search`, `open_page`, or `find_in_page`; assistant `output_text.annotations` carries `url_citation` ranges. `web_search_call.action.sources` is the consulted set and may be broader than citations. | Official [Responses streaming events](https://developers.openai.com/api/reference/resources/responses/streaming-events) include `response.web_search_call.in_progress`, `.searching`, `.completed`, output-text deltas, annotation-added events, and terminal completed/incomplete/failed events. `max_tool_calls` is request-wide across hosted tools. |
| Anthropic Messages | Official [web-search tool](https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool): current versioned types include `web_search_20250305`, `web_search_20260209`, and `web_search_20260318`. Controls include `max_uses`, exactly one of allowed/blocked domains, approximate location, callers, and version-dependent response inclusion. | The response sequence contains `server_tool_use`, `web_search_tool_result`, `web_search_result`, then cited text. A result retains `encrypted_content`; cited text uses `web_search_result_location`. In-body tool errors can arrive with HTTP 200. | Uses ordinary [Messages SSE](https://platform.claude.com/docs/en/build-with-claude/streaming). Tool input arrives as partial JSON; the server result begins as a complete content block. `pause_turn` requires replaying the paused assistant message unchanged. A parallel client tool can defer server search until the next request. |
| Gemini `generateContent` | Official [Google Search grounding guide](https://ai.google.dev/gemini-api/docs/generate-content/google-search?hl=en): `tools: [{"google_search": {}}]`. The [GenerateContent schema](https://ai.google.dev/api/generate-content) documents search types and a time-range filter; it does not document Nexa-style domain or location filters. | Candidate text is separate from `groundingMetadata`: queries, required-display search entry point, `groundingChunks[].web`, and `groundingSupports[]` map byte ranges to chunk indices. | `streamGenerateContent?alt=sse` emits `GenerateContentResponse` chunks, not typed search lifecycle events. A chunk may add grounding chunks while support indices remain global, so adapters must accumulate in order. |
| Gemini Interactions | Official [Interactions Google Search guide](https://ai.google.dev/gemini-api/docs/google-search): `tools: [{"type":"google_search"}]`; the [Interactions schema](https://ai.google.dev/api/interactions-api) documents web/image/enterprise search types, but no domain, time, or location controls on this surface. | Steps include `google_search_call` with queries, a call-linked `google_search_result`, then model output with byte-indexed `url_citation` annotations. | The [streaming guide](https://ai.google.dev/gemini-api/docs/streaming) uses interaction lifecycle plus `step.start`, `step.delta`, and `step.stop`; text and annotation deltas are assembled client-side. The [tool-combination guide](https://ai.google.dev/gemini-api/docs/tool-combination) places built-in plus custom-tool combinations behind model/API restrictions that the registry must encode. |
| DeepSeek Responses | Official [Responses reference](https://api-docs.deepseek.com/api/create-response/) and [guide](https://api-docs.deepseek.com/guides/responses_api) document hosted `web_search` and `web_search_2025_08_26` for the supported Responses model surface. The documented search context/location options are ignored and domain/time filters are not documented. | A `web_search_call` has `search`, `open_page`, or `find_in_page` action semantics. The current documentation does **not** contractually define OpenAI-equivalent structured URL citations or an included source list; Nexa must not synthesize that guarantee. | Semantic events include response lifecycle, item/content deltas, and `response.web_search_call.in_progress`, `.searching`, `.completed`. Function and hosted search may coexist, but ignored limits/options must be represented as unsupported rather than sent optimistically. |
| DeepSeek Chat / Anthropic compatibility | The [Chat Completions reference](https://api-docs.deepseek.com/api/create-chat-completion/) documents only `function` tools. Separately, the [Anthropic compatibility guide](https://api-docs.deepseek.com/guides/anthropic_api) and [Claude Code integration](https://api-docs.deepseek.com/zh-cn/quick_start/agent_integrations/claude_code/) describe compatible server-tool/search blocks. | Compatibility does not document parity with every Anthropic version, filter, ZDR, encrypted-result, or citation field. | Treat DeepSeek Chat, DeepSeek Responses, and DeepSeek Anthropic compatibility as distinct endpoint dialects selected by exact endpoint/model capability—not by provider display name. |

Google's developer documentation states that page prose is CC BY 4.0 and code samples are Apache 2.0. The other provider rows above cite hosted official documentation, not copied repository code; no repository-license claim is made for those pages.

### 1.2 Canonical Nexa boundary

The normalized contract should express user intent and observed evidence without pretending all providers support the same knobs:

```rust
enum HostedSearchDialect {
    OpenAiResponses,
    AnthropicMessages { tool_version: String },
    GeminiGenerateContent,
    GeminiInteractions,
    DeepSeekResponses,
    DeepSeekAnthropicCompat,
}

struct HostedSearchCapability {
    dialect: HostedSearchDialect,
    supported_models: Vec<ModelSelector>,
    supports_domain_allowlist: bool,
    supports_domain_blocklist: bool,
    supports_time_range: bool,
    supports_location: bool,
    supports_structured_citations: bool,
    supports_source_inventory: bool,
    can_mix_client_tools: ToolMixingRule,
}

struct WebSearchIntent {
    mode: SearchMode, // Auto | ProviderHosted | NexaRouter | Hybrid
    allowed_domains: Vec<String>,
    blocked_domains: Vec<String>,
    time_range: Option<TimeRange>,
    approximate_location: Option<ApproximateLocation>,
    max_uses: Option<u32>,
}

struct SearchEvidence {
    provider: String,
    dialect: HostedSearchDialect,
    query: Option<String>,
    url: Option<String>,
    title: Option<String>,
    cited_range: Option<TextRange>,
    raw_reference: Option<RawProviderReference>,
    observed_at: DateTime<Utc>,
}
```

`RawProviderReference` is essential for Anthropic encrypted content and for forward-compatible unknown blocks. Normalization must be additive: preserve raw events, emit semantic lifecycle events, and add normalized evidence only when supported.

### 1.3 Mapping to the current Nexa runtime

- [`web_search_tool.rs`](../../crates/core/src/tools/web_search_tool.rs#L1) calls its public, no-key engine path “native” and creates `SearchPlanItem::Native`. Rename the new concept to **provider-hosted** or **server search**; otherwise telemetry, settings, and fallback rules will be ambiguous.
- [`web_search/model.rs`](../../crates/core/src/web_search/model.rs) is the existing Nexa-router request/response health model. Do not force provider response items into this engine result schema.
- Provider capability selection belongs beside the shared provider/model catalog and exact configured endpoint. Unknown endpoints must default to no hosted-search capability.
- Provider stream parsers should emit durable `search.started`, `search.query`, `search.source`, `search.citation`, `search.completed`, and `search.failed` events with the untouched provider item attached where policy permits.

### 1.4 Do / do not

Do:

- select dialect by exact endpoint, API family, model capability, and tool version;
- validate mutually exclusive filters locally and show which controls a provider ignores;
- render only citations actually returned by the provider, with correct byte/character range conversion;
- retain source inventory separately from cited sources;
- resume Anthropic `pause_turn` and preserve required opaque result content;
- execute one route in Auto/ProviderHosted/NexaRouter modes; make duplicate search an explicit Hybrid choice.

Do not:

- infer hosted search from “OpenAI compatible” or “Anthropic compatible” branding;
- send Nexa's local `web_search` client tool and a provider-hosted tool under the same logical name;
- turn DeepSeek's empty/undocumented annotation shape into invented structured citations;
- treat HTTP 200 as Anthropic tool success without inspecting result error blocks;
- flatten Gemini grounding metadata before support indices have been resolved;
- drop unknown provider event fields during normalization.

## 2. Project workspace and durable runtime events

### 2.1 Current Nexa seams

The current code already contains most storage primitives, but they are not one Project runtime:

- [`Project`](../../crates/core/src/project.rs#L16) stores presentation fields, `system_prompt`, and `source_scope`.
- Desktop turn assembly loads ranked Project memory at [`desktop_agent_session.rs`](../../apps/desktop/src-tauri/src/desktop_agent_session.rs#L1453), but builds the base prompt from `conversation.system_prompt` at [line 1636](../../apps/desktop/src-tauri/src/desktop_agent_session.rs#L1636). The stored Project system prompt is therefore not visibly part of this assembly path.
- [`project_memory.rs`](../../crates/core/src/project_memory.rs#L11) caps injection at eight memories and 350 estimated tokens. This is a useful bounded hint channel, not a complete brief/decision/task history.
- [`AgentTaskRunEvent`](../../crates/core/src/conversation/mod.rs#L276) is an append-oriented execution event, while [`AgentExecutionGraph`](../../crates/core/src/conversation/mod.rs#L312) derives task/subtask structure.
- [`knowledge_graph.rs`](../../crates/core/src/knowledge_graph.rs#L24) represents entity links/nodes/edges with evidence snippets and confidence. It does not yet encode the full temporal, contradictory, superseding, or review lifecycle requested by the Project Event/Claim Graph.

The recommended move is to connect and extend these seams, not introduce a second runtime database.

### 2.2 Primary-source precedents

#### OpenHands: persistent history is not the live runtime

OpenHands' pinned [`event-service.api.ts`](https://github.com/All-Hands-AI/OpenHands/blob/4470813ce58f5ac384e3d367d34518e10106526b/src/api/event-service/event-service.api.ts) explicitly separates persistent conversation history on its App API from live runtime-sandbox endpoints. Its [`event-service.types.ts`](https://github.com/All-Hands-AI/OpenHands/blob/4470813ce58f5ac384e3d367d34518e10106526b/src/api/event-service/event-service.types.ts) defines timestamp order/filtering and an opaque `next_page_id`; the caller must paginate rather than mistake one page for complete history.

The pinned [`use-event-store.ts`](https://github.com/All-Hands-AI/OpenHands/blob/4470813ce58f5ac384e3d367d34518e10106526b/src/stores/use-event-store.ts) deduplicates by event ID, merges streaming deltas, re-sorts out-of-order timestamps, and records `loadedConversationId` so one conversation's history cannot masquerade as another's. [`conversation-metadata-store.ts`](https://github.com/All-Hands-AI/OpenHands/blob/4470813ce58f5ac384e3d367d34518e10106526b/src/api/conversation-metadata-store.ts) binds selected workspace/mode to the conversation and deliberately stores only plugin coordinates because plugin parameters can contain secrets.

Nexa implication: live UI events and durable Project events may share IDs/types, but persistence acknowledgement, pagination completeness, and active-project identity must be explicit.

#### LangGraph: checkpoint identity and scoped stores are separate concepts

The pinned [`checkpoint/base/__init__.py`](https://github.com/langchain-ai/langgraph/blob/fde3068970679184b68d3d068a92c83c966a4888/libs/checkpoint/langgraph/checkpoint/base/__init__.py) treats `thread_id` as the primary checkpoint key, adds checkpoint namespace/ID, parent metadata, and pending writes. The pinned [`store/base/__init__.py`](https://github.com/langchain-ai/langgraph/blob/fde3068970679184b68d3d068a92c83c966a4888/libs/checkpoint/langgraph/store/base/__init__.py) separately exposes hierarchical namespace + key + value storage, structured filters, semantic search, and pagination.

Nexa implication: a turn checkpoint answers “where can execution resume?”; a Project knowledge item answers “what durable fact applies in this scope?”. Do not overload Project memory rows as resumable execution checkpoints or vice versa.

#### GraphRAG: claims remain linked to source text and time

GraphRAG's pinned [`Covariate`](https://github.com/microsoft/graphrag/blob/14a00ad88fc33cf2b52f4f113f25807556f8e25e/packages/graphrag/graphrag/data_model/covariate.py) retains subject, covariate type, source text-unit IDs, and arbitrary attributes. Its pinned [claim extraction prompt](https://github.com/microsoft/graphrag/blob/14a00ad88fc33cf2b52f4f113f25807556f8e25e/packages/graphrag/graphrag/prompts/index/extract_claims.py) asks for subject, object, type, TRUE/FALSE/SUSPECTED status, start/end dates, description, and source text.

Nexa implication: an extracted claim is not a memory sentence. It is a scoped, temporal assertion with provenance and review state. GraphRAG is useful evidence for the shape; Nexa should not copy its prompt or treat model-assigned TRUE as human verification.

### 2.3 Proposed shared data model

Keep immutable observations and editable projections distinct:

```text
project_event
  id, project_id, conversation_id?, turn_id?, task_run_id?
  kind, schema_version, payload_json
  occurred_at, recorded_at, actor_kind, actor_id?
  source_refs[], confidence?, review_state

claim
  id, project_id, subject_ref?, predicate, object_json
  valid_from?, valid_to?, asserted_at
  confidence, review_state, superseded_by?

claim_evidence
  claim_id, source_id, document_id?, chunk_id?, span_start?, span_end?
  relation = supports | contradicts | mentions

project_projection
  project_id, projection_kind, revision, derived_through_event_id
  content_json, updated_at
```

Recommended node kinds: Project, Source, Document, Chunk, Entity, Event, Claim, Decision, Constraint, Task, ConversationEpisode, Artifact, Topic. Recommended edge kinds: mentions, supports, contradicts, supersedes, caused_by, before, after, depends_on, decided_in, derived_from, produced, belongs_to, same_as, related_to.

Every derived claim/edge must retain Project scope, source evidence, extractor/model version, confidence, valid time where known, review state, and supersession. Use SQLite foreign keys, existing FTS/vector retrieval, and recursive CTE/query expansion first; a graph server is not justified until measured workloads require it.

### 2.4 Turn and Project lifecycle

1. Persist raw provider/tool/runtime events with stable IDs and the active Project/conversation/turn identity.
2. Commit the final turn and its source/artifact references.
3. Extract candidate Project events/claims in a bounded post-turn job.
4. Store candidates as `proposed`; never mutate accepted brief/claims directly.
5. Let deterministic policies auto-accept only low-risk facts such as produced artifact identity; route decisions, constraints, contradictions, and deletions to review.
6. Rebuild materialized brief/timeline/task/claim projections through an idempotent reducer.
7. Bootstrap a new Project chat from instruction hierarchy, accepted brief/constraints/decisions, open tasks, relevant episodes/claims, and source scope—with provenance labels and a token budget.

Correction and deletion must be events too. Supersede accepted claims rather than editing their historical rows. Privacy deletion must remove or tombstone source-derived projections consistently and schedule index cleanup.

### 2.5 Project do / do not

Do:

- make `project_id + event_id` uniqueness and idempotent reducer checkpoints explicit;
- expose history completeness and persistence acknowledgement to the UI;
- keep raw observation, extracted claim, reviewed fact, and current brief as different layers;
- require source spans for claims whenever the source has stable text offsets;
- isolate active Project/conversation state exactly as OpenHands isolates loaded conversation history;
- make model/extractor revision part of derivation provenance;
- use valid time and recorded time separately for temporal queries.

Do not:

- silently inject a stored Project prompt at a higher priority than the UI communicates;
- call eight short memory bullets a complete Project runtime;
- overwrite a decision when a later turn disagrees—record contradiction/supersession;
- let a dreaming/background pass directly rewrite accepted Project state;
- store plugin/provider secrets in event payloads or workspace metadata;
- add Neo4j before SQLite retrieval/projection performance is measured.

## 3. Event-driven companion safety

### 3.1 What Codex actually demonstrates

Codex's companion is a lifecycle projection, not an autonomous agent. In the pinned source:

- [`ambient.rs`](https://github.com/openai/codex/blob/e734a1a5c1c6e51d7a28ec8ec6381d7ffb18e23b/codex-rs/tui/src/pets/ambient.rs) maps semantic notifications `Running`, `Waiting`, `Review`, and `Failed` to animation/label/fallback text, expires stale states, and falls back to `idle` when an animation is missing.
- [`turn_runtime.rs`](https://github.com/openai/codex/blob/e734a1a5c1c6e51d7a28ec8ec6381d7ffb18e23b/codex-rs/tui/src/chatwidget/turn_runtime.rs) derives running/review/failure from turn lifecycle.
- [`tool_requests.rs`](https://github.com/openai/codex/blob/e734a1a5c1c6e51d7a28ec8ec6381d7ffb18e23b/codex-rs/tui/src/chatwidget/tool_requests.rs) derives waiting from approvals, permissions, user input, and MCP elicitation—not from model reasoning text.
- [`model.rs`](https://github.com/openai/codex/blob/e734a1a5c1c6e51d7a28ec8ec6381d7ffb18e23b/codex-rs/tui/src/pets/model.rs) caps custom packs at 256 frames and 60 FPS, rejects invalid geometry/frame references/fallbacks, and rejects absolute or parent-traversing sprite paths.
- [`asset_pack.rs`](https://github.com/openai/codex/blob/e734a1a5c1c6e51d7a28ec8ec6381d7ffb18e23b/codex-rs/tui/src/pets/asset_pack.rs) downloads built-ins over HTTPS with timeout and byte cap, validates the final URL and image, stages the download, then atomically renames it into a versioned cache.
- [`chatwidget/pets.rs`](https://github.com/openai/codex/blob/e734a1a5c1c6e51d7a28ec8ec6381d7ffb18e23b/codex-rs/tui/src/chatwidget/pets.rs) ignores stale asynchronous preview results and turns load failures into local UI errors rather than failing the chat.

The pinned manifest does not establish a reusable public `schemaVersion` contract. Nexa should define and version its own format. Nexa must also create original artwork; the Codex sprites/branding are outside the architectural comparison.

### 3.2 Nexa contract

The companion subscribes to the same durable semantic event bus as Task Center and recovery UI:

```text
turn.started / tool.started                  -> Running
input.requested / approval.requested         -> Waiting
turn.completed / artifact.ready              -> Review
turn.failed / recovery.required               -> Failed
event expired / run superseded / conversation changed -> Idle
```

State selection should be a deterministic reducer using conversation ID, run ID, sequence/time, and priority. A late event from an old conversation must not animate the active chat. The primary modal, toast, task row, or error panel remains authoritative; the companion is a compact secondary cue.

Recommended Nexa pack rules:

- `pet.json` with Nexa-owned `schemaVersion`, ID/display name, relative spritesheet path, frame geometry, named animations, FPS, loop, and fallback;
- JSON plus inert image files only—no JavaScript, WASM, native code, HTML, or remote URLs in custom packs;
- canonicalize the package root; reject absolute paths, `..`, Windows prefixes, links/reparse points escaping the root, duplicate IDs, decompression bombs, excessive dimensions/frames/FPS, and invalid indices;
- parse and decode off the render path, cache by content hash, publish only after complete validation, and isolate one pack's failure;
- use a fixed-size layout slot so loading/failure cannot move the composer;
- respect `prefers-reduced-motion` and a Nexa animation-off setting by selecting a stable representative frame while keeping text/status accessible;
- expose accessible name/status text and never rely on color or motion alone.

### 3.3 Companion do / do not

Do:

- derive state from durable lifecycle/input/approval events;
- expire or supersede stale states deterministically;
- keep keyboard focus on the actual control requiring action;
- make the feature optional, disable-able, and harmless when assets fail;
- reuse Project/run identity and event ordering already needed by Task Center.

Do not:

- parse chain-of-thought or assistant prose to decide the animation;
- allow a pet pack to execute code or fetch arbitrary runtime content;
- let the companion become the only indication that input or approval is required;
- copy Codex assets, names, or timing values as a Nexa visual identity;
- run continuous animation under reduced-motion preference.

## 4. Cross-feature implementation order

1. Define provider-hosted search capabilities and dialect parser tests; keep existing Nexa Router behavior unchanged.
2. Add durable semantic event envelopes with Project/conversation/run identity and idempotent append semantics.
3. Use those events for provider-search progress and the existing task/recovery surfaces.
4. Add Project event candidates, claim provenance, review, and deterministic projections.
5. Inject the accepted Project projection into new chats with visible provenance and budget controls.
6. Add the optional companion as a read-only event projection after the event bus is authoritative.

This order prevents three competing state machines and lets the companion exercise the same stale-event and recovery paths that Project and search UI require.

## 5. Verification plan

Provider contract tests:

- golden request/response/SSE fixtures per exact dialect and tool version;
- unknown endpoint/model refuses ProviderHosted mode and falls back only according to explicit Auto policy;
- domain XOR validation, ignored-control visibility, and correct byte-to-text citation ranges;
- Anthropic HTTP-200 error blocks, empty results, `pause_turn`, encrypted result replay, and mixed client/server tools;
- Gemini chunk accumulation with global grounding indices and Interactions step/annotation assembly;
- DeepSeek Responses search lifecycle without invented citations; DeepSeek Chat rejects hosted-search configuration;
- exactly one search route outside explicit Hybrid mode.

Project/event tests:

- duplicate append and reducer retry are idempotent;
- out-of-order live/durable events converge on the same ordered projection;
- incomplete pagination is never reported as complete history;
- active Project/conversation guards reject stale events;
- claim evidence survives projection rebuild; contradiction and supersession preserve history;
- source deletion/tombstone removes retrieval and projection exposure;
- Project prompt/brief injection order, token budget, and provenance are visible and tested;
- migration rollback preserves existing Project memory and task-run events.

Companion tests:

- event-to-state reducer covers running/waiting/review/failed/idle, expiry, supersession, and conversation switches;
- input modal remains focusable and primary while Waiting is displayed;
- reduced motion produces a stable frame with accessible status text;
- malformed JSON, oversized images, invalid geometry/FPS/index/fallback, traversal, symlink/reparse escape, and decompression limits fail closed;
- stale preview/load completion cannot replace the newly selected pack;
- asset failure never blocks message composition or the active agent run.

## 6. Acceptance guardrails

The feature is ready only when:

- every enabled provider-hosted search dialect has fixtures for its real wire/events and an exact capability entry;
- unknown/custom endpoints cannot inherit trusted hosted-search credentials or capabilities;
- a Project brief can be regenerated deterministically from reviewed events while preserving evidence and superseded history;
- background extraction cannot directly alter accepted Project state;
- the companion can be disabled, reduced to a static frame, or fail entirely without hiding an actionable request or disturbing the chat runtime;
- no upstream visual assets or prompts are copied, and all borrowed source-code ideas remain within the licenses recorded above.
