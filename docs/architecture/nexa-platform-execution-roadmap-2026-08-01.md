# Nexa Platform Execution Roadmap

**Roadmap date:** 2026-08-01  
**Repository:** `MLGBJDLW/Nexa`  
**Implementation branch:** `agent/platform-capability-audit`  
**Related audit:** `docs/architecture/agent-platform-capability-audit-2026-07-31.md`  
**Current delivery PR:** `#264`

## 1. Executive decision

Nexa should not continue growing by adding isolated settings, model names, or tool wrappers. The next stage requires a small set of shared runtime contracts that every capability uses:

1. **Model Catalog 2.0** — one lifecycle- and capability-aware catalog contract across text, image, video, speech, realtime, and embedding models.
2. **Durable Interaction Runtime** — explicit `waiting_for_user`, `waiting_for_approval`, pause, resume, and interaction events instead of UI heuristics.
3. **Artifact and Async Job Runtime** — a common runtime for generated images, videos, transcripts, exports, and long provider tasks.
4. **Package Host and Extensions Hub** — Skills, MCP connectors, apps, workflows, and future native plugins share installation and trust lifecycle while retaining separate execution formats.
5. **Deterministic Retrieval Router** — code, documents, graph, web, and media use different retrieval lanes selected by policy, not only by prompt wording.
6. **Capability-specific evaluations** — a provider or model is not “supported” until its request, response, streaming/job behavior, tool use, usage accounting, error mapping, and UI surface pass focused tests.

The order is intentional. Provider and UI work built before these contracts would create another round of duplicated catalogs, mismatched states, and settings that do not control the actual runtime.

## 2. Non-negotiable engineering rules

### 2.1 Catalog entry is not full support

A model has four distinct states:

- **Known:** verified model identity and official lifecycle are recorded.
- **Discoverable:** account-scoped live discovery can expose it.
- **Callable:** Nexa has a compatible adapter and validated request/response behavior.
- **Product-ready:** streaming or async jobs, tools, usage, errors, artifacts, UI, and tests are complete.

The UI must show these states. A model that is listed but cannot be called correctly must never appear as fully supported.

### 2.2 Do not hard-code unverified snapshot IDs

Stable aliases and officially verified snapshots may enter the curated overlay. Account- or region-specific model IDs must be discovered live whenever the provider exposes them. If an official product page names a family but does not publish an API ID, Nexa should display the family only after discovery rather than guessing an ID.

### 2.3 Preview and gated models never become automatic defaults

Preview, limited-preview, allowlisted, or region-restricted models must carry explicit lifecycle/access metadata. Their fallback must remain an active generally available model.

### 2.4 Separate modalities and execution styles

The following are not interchangeable provider “models”:

- chat-completions text models;
- Responses API agent models;
- realtime WebSocket omni models;
- synchronous image generation/editing;
- asynchronous video generation;
- TTS, ASR, voice cloning, and live interpretation;
- text and multimodal embedding.

They may share credentials and provider identity, but each needs its own adapter contract.

### 2.5 Every high-impact capability needs a replayable audit trail

Computer Use, Skills installation, connector authorization, model calls, media generation, and user-interaction resumes must emit durable events sufficient to explain what happened and reproduce failures without storing secrets.

---

## 3. Program dependency graph

```text
Model Descriptor v2 ──────────────┐
Provider Endpoint Registry ───────┼──> Qwen / Volcengine / future providers
Async Job + Artifact Runtime ─────┘             │
                                                ├──> Image / Video / Media generation
Durable Interaction Runtime ────────────────────┼──> Question Cards / approvals / pause-resume
                                                └──> Computer Use user takeover
Package Host v2 ────────────────────────────────┬──> Skills lifecycle
                                                ├──> MCP connector packages
                                                └──> Extensions Hub
Retrieval Router ───────────────────────────────┬──> grep / code intelligence
                                                ├──> document RAG
                                                └──> Graph local/global retrieval
Evaluation + telemetry ─────────────────────────> gates every workstream
```

No downstream workstream should invent a private replacement for these foundations.

---

# Part I — Model and provider program

## 4. Workstream MODEL-001: Model Catalog 2.0

### Objective

Replace modality-specific ad hoc preset shapes with a canonical model descriptor and a provider endpoint registry, while preserving the existing curated-plus-live-discovery behavior in `provider_catalog.rs`.

### Current strengths

The text model catalog already supports:

- curated overlays;
- account-scoped live discovery;
- lifecycle status;
- source provenance;
- regions;
- last verification date;
- modalities;
- tool and structured-output capabilities;
- reasoning effort metadata.

Image, TTS, STT, and embedding preset files do not yet expose the same lifecycle and capability model.

### Canonical data model

```text
ProviderDescriptor
  id
  display_name
  aliases[]
  credential_kind
  documentation_ref
  endpoints[]

ProviderEndpoint
  id
  provider_id
  region
  base_url_template
  api_style
  transport: http | sse | websocket | async_job
  auth_style
  workspace_required
  discovery_strategy
  health_probe

ModelDescriptor
  id
  aliases[]
  display_name
  provider_id
  family
  version
  lifecycle: active | preview | gated | legacy | deprecated | removed
  access: public | account_enablement | application | private_preview
  regions[]
  endpoint_kinds[]
  input_modalities[]
  output_modalities[]
  capabilities
  limits
  pricing_ref
  release_date
  deprecation_date
  replacement_model_id
  source: official | discovered | curated
  last_verified_at

ModelCapabilities
  reasoning
  vision
  audio_input
  audio_output
  video_input
  video_output
  tool_calling
  parallel_tool_calling
  structured_output
  image_generation
  image_editing
  multi_reference_editing
  realtime
  prompt_cache
  async_jobs
  batch

ModelLimits
  context_tokens
  max_output_tokens
  max_images
  max_input_bytes
  max_video_seconds
  max_audio_seconds
  supported_sizes[]
  output_formats[]
```

### Catalog sources and precedence

1. **Account live discovery** determines what credentials can use now.
2. **Official signed/curated overlay** supplies names, capability metadata, lifecycle, and aliases.
3. **Provider capability probe** verifies behavior not exposed by `/models`.
4. **Local cache** allows offline settings and preserves the last good catalog.
5. **Static fallback** is used only when discovery is unavailable.

Removed-model tombstones must override live or stale cache entries.

### Required code changes

- Add `crates/core/src/model_catalog/`:
  - `descriptor.rs`
  - `provider_endpoint.rs`
  - `merge.rs`
  - `discovery.rs`
  - `probe.rs`
  - `cache.rs`
  - `lifecycle.rs`
- Migrate or wrap:
  - `provider_catalog.rs`
  - `image_provider_catalog.rs`
  - embedding provider catalog;
  - TTS/STT catalogs.
- Replace duplicated TypeScript types with generated or shared JSON-schema-derived types.
- Add a catalog migration layer so existing saved provider IDs and model selections remain valid.
- Add provider aliases and model replacement mappings.

### Settings UX

Each model row should display:

- lifecycle badge;
- access requirement;
- region;
- input/output modalities;
- tools, reasoning, realtime, and async-job indicators;
- discovered versus curated source;
- last verification time;
- replacement model when deprecated;
- “available to this credential” status.

Default selection must exclude preview, gated, deprecated, removed, and unprobed entries unless the user explicitly selects them.

### Automated drift audit

Add a scheduled repository job that:

1. calls supported provider discovery endpoints using test credentials;
2. compares discovered IDs with the curated overlay;
3. emits a JSON and Markdown drift report;
4. opens or updates an issue for new, missing, deprecated, and capability-changed models;
5. never edits production presets automatically;
6. stores no model-provider secrets in artifacts.

### Acceptance criteria

- All model settings surfaces consume one descriptor projection.
- Existing saved configs continue to resolve.
- Preview and gated models cannot become implicit defaults.
- New live models appear without an app release when safe discovery is available.
- Removed models disappear through tombstones.
- Every visible capability has either official metadata or a passing probe.
- Catalog drift tests cover region and credential isolation.

---

## 5. Verified model gap audit — 2026-08-01

This table records verified gaps, not speculative names.

| Family | Current Nexa state | Required action | Product-ready gate |
|---|---|---|---|
| `qwen-image-3.0-pro` | Added to Beijing and Singapore image presets in this branch; 2.0 Pro remains recommended | Complete model metadata, workspace endpoint support, I2I/reference-image support, multi-output handling | T2I + 1–3 reference I2I tests, access error UX, artifact persistence |
| `qwen3.8-max-preview` | Not present | Add only to the Qwen Token Plan endpoint as preview after account discovery confirms the ID | Token Plan routing, reasoning/tool tests, preview badge |
| `qwen3.7-flash` and official snapshot | Not present in the repository search | Add to Alibaba Model Studio curated overlay and verify region availability | chat, thinking toggle, tools, vision, usage tests |
| `qwen3.5-omni-plus` / `qwen3.5-omni-flash` | Not present | Add to an omni catalog, not a text-only preset | audio/image/video input schema and text/audio output tests |
| Qwen 3.5 Omni Realtime variants | Not present | Build a WebSocket realtime adapter and session runtime | interruption, reconnect, session limits, audio/video streaming tests |
| `qwen3.5-livetranslate-flash-realtime` | Not present | Add a live-translation capability after realtime runtime exists | bidirectional audio/text output, language mapping, latency tests |
| Qwen Audio 3.0 TTS Plus/Flash | Already present in TTS presets | Add voice enrollment/cloning lifecycle and capability metadata | enrollment, consent, deletion, instruction-control tests |
| Qwen3 TTS instruct / voice design / voice cloning families | Partially absent | Add after the voice asset model exists | voice ownership, enrollment, provider-specific request tests |
| Qwen3 ASR Flash | Already present | Extend to timestamps, confidence, diarization integration | ASR contract and meeting-pipeline tests |
| Qwen 3.7 text embedding | Already present | Verify dimensions and region metadata in Model Descriptor v2 | dimension override and migration tests |
| Doubao Seed 2.1 Pro / Turbo / Evolving | Provider type exists, no usable preset/catalog entry | Add Volcengine Ark provider, Responses adapter, live discovery, and curated family metadata | streaming, thinking, tools, multimodal input, cache/usage tests |
| Doubao Seed 2.0 Pro / Lite / Mini / Code | Absent | Add as active/legacy fallback according to live provider lifecycle; keep exact snapshot IDs discovery-led | standard Ark and Coding Plan compatibility tests |
| `ark-code-latest` | Absent | Add as a routing alias under a separate Ark Coding Plan endpoint | provider-specific base URL, model switching, tool loop tests |
| Doubao Seed Vision / Character | Absent | Add capability-specific metadata after request behavior is verified | image input, persona/role behavior, safety tests |
| Seedream 5.0 / 5.0 Lite / 4.5 / 4.0 | Absent | Add Volcengine image provider and Seedream body/response adapter | text/image generation, editing, web-search tool option, output tests |
| Seedance 2.0 / Fast / Mini and supported older variants | No video generation runtime | Build async Video Generation capability; do not place in image settings | submit/poll/cancel, progress, multi-reference, 1080p, artifact tests |
| Doubao RealtimeVoice / TTS 2.0 / ICL 2.0 / LiveInterpret | Absent | Add separate realtime speech, TTS, voice cloning, and interpretation adapters | consent, realtime session, voice asset, billing/usage tests |
| Doubao Seed Embedding / Embedding Vision | Absent | Add text and multimodal embedding provider descriptors | dimension, image/video input, index-version migration tests |

### Models deliberately not hard-coded yet

- Unverified Doubao 2.1 snapshot IDs.
- Account-specific Ark endpoint IDs.
- Region-specific preview IDs not visible to live discovery.
- Video model IDs before the async generation API contract is implemented.

The provider UI may show family names from official metadata, but only credential-visible discovered IDs should be selectable for calls.

---

## 6. Workstream QWEN-IMG-300: Complete Qwen Image 3.0 support

### Immediate branch state

`qwen-image-3.0-pro` is now present in both Qwen image presets and is explicitly non-recommended during limited preview. The existing Qwen request shape is compatible with text-to-image calls, but the current tool does not expose the complete 3.0 capability surface.

### Stage A — catalog and access metadata

- Extend image model presets with lifecycle, access, regions, input/output modalities, editing support, max references, and supported parameter metadata.
- Mark Qwen Image 3.0 as:
  - lifecycle: preview;
  - access: application/limited preview;
  - inputs: text, image;
  - output: image/png;
  - image editing: true;
  - max reference images: 3.
- Preserve Qwen Image 2.0 Pro as the automatic default.
- Display an access-required badge and a direct diagnostic when the provider returns a permission error.

### Stage B — endpoint modernization

Support both:

- legacy DashScope Beijing/Singapore domains;
- workspace-specific Model Studio domains.

Add endpoint configuration fields:

```text
region
workspace_id
base_url_override
use_workspace_domain
```

Do not store API keys or workspace secrets in catalog files. Validate that region and credential origin match before the request.

### Stage C — request contract

Extend `GenerateImageArgs` into a provider-neutral contract:

```text
prompt
reference_images[]
size: auto | width*height
count
seed
negative_prompt
prompt_extend
watermark
output_format
filename
```

Reference images may originate from:

- current chat image attachments;
- generated-image artifacts;
- source-scoped local paths;
- HTTPS URLs;
- validated data URLs.

Rules:

- 1–3 references for Qwen Image 3.0 editing;
- reject unsupported MIME types and oversized images before sending;
- do not silently upload arbitrary local files;
- preserve reference order;
- omit `size` for model-selected automatic resolution;
- send `negative_prompt`, `seed`, and `n` only when supported.

### Stage D — multi-output artifacts

Replace the singular internal result with:

```text
GeneratedImageSet
  provider
  model
  request_id
  images[]
  usage
  prompt
  revised_prompt
  references[]
  transient_until
```

Each image receives its own media type, bytes, provider URL, suggested filename, and saved/transient state. Provider result URLs must be materialized immediately because they expire.

### Stage E — editing UI

- Add “Edit with AI” to generated images and source images.
- Allow 1–3 references with visible ordering.
- Provide preserve-subject, preserve-layout, and free-edit prompt helpers without hiding the actual prompt.
- Show generated variations as a gallery.
- Saving remains explicit; generated previews stay transient until saved.

### Required tests

- catalog parses and 2.0 remains default;
- workspace and legacy endpoint resolution;
- T2I request snapshot;
- one-, two-, and three-reference I2I requests;
- MIME, byte-size, and count validation;
- `size=auto` omission;
- negative prompt, seed, watermark, and count;
- multiple output parsing and immediate download;
- access-denied and region-mismatch errors;
- no reference bytes leak into logs or traces.

---

## 7. Workstream VOLC-001: Volcengine Ark / ByteDance capability suite

### Objective

Turn the already-declared `ProviderType::Doubao` into a real provider package spanning Ark text/agent models, Seedream image generation, Seedance video generation, speech, and embeddings.

### Provider identity and endpoints

Add aliases:

```text
doubao
volcengine
ark
bytedance
```

Define separate endpoint profiles:

1. **Ark Standard API** — `https://ark.cn-beijing.volces.com/api/v3`.
2. **Ark Coding Plan OpenAI-compatible endpoint** — separate configuration and entitlement.
3. **Ark Coding Plan Anthropic-compatible endpoint** — optional later adapter; do not reuse OpenAI auth/header assumptions.
4. Future international regions as separate endpoint records, not string substitutions.

Provider detection should recognize Ark hosts, but explicit provider configuration must win over URL inference.

### Text and Agent adapter

The current OpenAI-compatible adapter is Chat Completions centered. Add a `ResponsesCompatible` adapter with:

- input item serialization;
- output item/event normalization;
- reasoning/thinking controls;
- function calls and function outputs;
- parallel tools;
- text and multimodal inputs;
- streaming event parsing;
- usage and cache fields;
- refusal and content-filter mapping;
- idempotency and retry policy.

Expose provider API style in config:

```text
chat_completions
responses
coding_openai
coding_anthropic
```

Do not guess the style from the selected model alone.

### Model catalog strategy

- Seed 2.1 Pro, Turbo, and Evolving are recommended family entries after live verification.
- Seed 2.0 Pro, Lite, Mini, and Code remain selectable fallbacks when discovered.
- `ark-code-latest` is a routing alias, not a fixed model snapshot.
- Exact snapshot IDs enter the curated overlay only after official verification.
- Account-scoped live discovery decides availability.
- Capability probes verify tools, vision, thinking, structured output, and cache reporting.

### Multimodal request model

Extend `ContentPart` beyond text and base64 image:

```text
Text
ImageUrl / ImageBytes
AudioUrl / AudioBytes
VideoUrl / VideoAsset
FileReference
```

Provider adapters may downgrade or reject unsupported parts explicitly. No adapter should silently discard a modality.

### Seedream image package

Create `VolcengineImageProvider` rather than relying exclusively on generic OpenAI Images behavior. It should own:

- Seedream-specific request fields;
- reference-image/editing inputs;
- optional provider tools such as web search when officially supported;
- output format and resolution rules;
- multiple results;
- provider task/request IDs;
- URL materialization and expiration handling;
- model-specific safety errors.

Initial curated families:

- Seedream 5.0 / 5.0 Lite;
- Seedream 4.5;
- Seedream 4.0.

### Seedance video package

Create a new capability package, not an image model option:

```text
video-generation
  submit_video_generation
  observe_video_generation
  cancel_video_generation
  save_generated_video
```

Canonical request:

```text
prompt
reference_images[]
reference_videos[]
reference_audio[]
duration
resolution
aspect_ratio
camera_motion
seed
safety_authorizations[]
```

Canonical job:

```text
job_id
provider
model
state
progress
submitted_at
updated_at
expires_at
outputs[]
error
usage
```

Use the shared async job runtime for polling, cancellation, app restart recovery, and final artifact download. Seedance must include portrait/likeness authorization and copyright/safety metadata in the workflow rather than hiding these behind generic tool approval.

### Speech package

Separate adapters are required for:

- RealtimeVoice conversational sessions;
- TTS 2.0;
- ICL 2.0 voice cloning;
- LiveInterpret simultaneous interpretation;
- batch or streaming ASR where offered.

Voice cloning requires a durable `VoiceAsset` model with owner consent, source recording, provider enrollment ID, permitted scopes, expiration, and deletion propagation.

### Embedding package

Add:

- Doubao Seed text embedding;
- Doubao embedding-vision/multimodal embedding.

Multimodal vectors require index metadata that records model, dimensions, normalization, modality, and preprocessing version. Changing embedding models must create a new index version rather than mixing incompatible vectors.

### Acceptance criteria

- Ark Standard and Coding Plan credentials are not confused.
- Responses streaming and tool loops pass provider fixtures.
- Model availability is account-scoped and live-discovered.
- Seedream produces and saves image artifacts through a provider-specific adapter.
- Seedance jobs survive app restarts and support cancellation.
- Speech and voice cloning enforce consent and deletion lifecycle.
- Embedding model changes trigger safe reindexing.

---

# Part II — Agent runtime and UX

## 8. Workstream INTERACT-001: Durable user-interaction runtime

### Objective

Replace “tool call ended, then a new user bubble starts another turn” with an explicit continuation protocol for questions, approvals, forms, handoffs, and user takeover.

### State model

```text
queued
running
waiting_for_user
waiting_for_approval
paused
cancelling
completed
failed
cancelled
timed_out
```

### Event model

```text
interactionRequested
interactionUpdated
interactionResolved
interactionCancelled
runPaused
runResumed
```

`interactionRequested` contains an interaction ID, schema, card kind, source call ID, resumability token, expiration policy, and visibility. User answers are stored as interaction responses, not ordinary conversation turns.

### Runtime API

```text
continue_agent_run(run_id, interaction_id, response_artifact)
cancel_agent_interaction(run_id, interaction_id)
list_pending_interactions(conversation_id)
```

Resume must be idempotent. Duplicate card clicks return the existing resolution rather than starting another turn.

### UI invariants

Never fold or hide:

- a final answer;
- a pending question;
- a pending approval;
- a form or confirmation;
- a recoverable error requiring user action;
- a user-takeover request.

Completed interaction cards stay visible as compact, answered records. Pure choice cards submit on the final required selection. Text/multi-select forms keep an explicit submit action.

### Persistence and recovery

- Pending interactions survive app restart.
- A resumed run keeps the same run/turn identity.
- If provider state cannot resume, the runtime builds a deterministic continuation request with the original interaction context.
- Expired interactions show a restart action instead of failing silently.

### Acceptance criteria

- No extra user bubble for structured card responses.
- No duplicate send action for complete choice cards.
- A pending card remains visible across reload and restart.
- Duplicate submissions are harmless.
- Run history and trajectories can distinguish user content from control-plane interaction responses.

---

## 9. Workstream SHELL-001: Shared Shell Capability Service

### Objective

Make one shell preference control both interactive terminals and Agent `run_shell` execution.

### Service contract

```text
ShellCapability
  id
  executable
  dialect
  version
  source
  available
  supports_pty
  supports_noninteractive
  path_translation

ShellPreference
  terminal_shell
  agent_shell
  link_preferences
  explicit_wsl_distribution
  fallback_policy
```

### Detection order

Windows `auto`:

```text
pwsh
-> explicitly configured Git Bash/native bash
-> Windows PowerShell
-> cmd
```

macOS:

```text
$SHELL -> zsh -> bash -> sh
```

Linux:

```text
$SHELL -> bash -> zsh -> sh
```

WSL is opt-in because it changes paths, environment, permissions, and process ownership.

### Integration points

- `AppConfig` and migration defaults;
- Settings > Agent & Tools > Terminal & Shell;
- TerminalDock PTY creation;
- `run_shell` default selector;
- system prompt dialect hint;
- command preview and Tool Card executable label;
- verification environment;
- project tool execution where appropriate.

### Acceptance criteria

- Settings report the detected executable and version.
- Terminal and Agent use the same preference when linked.
- Missing preferred shells fall back predictably and visibly.
- WSL is never entered implicitly.
- Windows path quoting, CJK paths, environment stripping, and cancellation have tests per dialect.

---

## 10. Workstream RETRIEVE-001: Deterministic retrieval router

### Objective

Separate code intelligence from document RAG at the runtime-policy level and keep source code out of ordinary document embeddings by default.

### Retrieval lanes

```text
CodeExact
CodeSymbol
CodeHistory
DocumentExact
DocumentSemantic
DocumentMetadata
GraphLocal
GraphGlobal
MediaTimeline
WebEvidence
Mixed
```

### Routing policy

Examples:

- class/function/path/error/import/reference questions -> grep, LSP, tree-sitter, git;
- exact phrase in ordinary files -> exact text search first;
- semantic questions over PDF, Office, Markdown, notes -> hybrid RAG;
- entity relationship questions -> Graph Local plus source evidence;
- corpus-level theme/comparison -> Graph Global or document aggregation;
- meeting/video questions -> timestamped media timeline;
- code plus specification -> independent code and document lanes, then evidence merge.

### Index policy

- Code files are excluded from the general document embedding index by default.
- Optional code semantic search uses a separate symbol-level index with its own model/version.
- Binary/media sources use media-specific extraction and timeline indexes.
- Every retrieved item carries provenance, trust boundary, score components, and router reason.

### Runtime design

`RetrievalPlanner` produces a plan before tool selection:

```text
lanes[]
queries[]
filters
budgets
merge_strategy
required_evidence
fallbacks
```

The UI may show the selected lanes in developer mode and aggregate telemetry without exposing noisy internal routing text to ordinary users.

### Evaluation

Build benchmark sets for:

- symbol lookup;
- exact error search;
- natural-language code intent;
- document QA;
- cross-document synthesis;
- code/spec mixed tasks;
- graph relationship questions;
- media timestamp questions.

Track recall, precision, citation correctness, latency, context bytes, and answer success.

---

# Part III — Extensions and connectors

## 11. Workstream SKILLS-002: Universal skill package lifecycle

### Objective

Evolve existing safe local ZIP/`.skill`/directory import into source-aware installation, trust, dependency, update, and rollback.

### Acquisition sources

- local file/directory;
- ZIP, TAR, TAR.GZ, `.skill`, identified by content signature;
- HTTPS package;
- Git URL and repository subdirectory;
- GitHub/Gitee repository or release asset;
- curated registry;
- pasted `SKILL.md`.

### Pipeline

```text
acquire
-> quarantine
-> discover roots
-> normalize manifest
-> scan files/scripts
-> resolve dependencies/capabilities
-> show install plan
-> approval
-> immutable install
-> activate metadata
-> post-install validation
```

### Package and installation records

Add package identity, version, source URI, source revision, checksum, publisher, license, compatibility, dependencies, capability requirements, trust state, scope, update channel, granted capabilities, and rollback pointer.

### Runtime rules

- Progressive disclosure remains mandatory.
- Skill scripts receive no implicit shell/network rights.
- Skills refer to capabilities rather than bundling unrestricted executors.
- Unknown publishers install into quarantine and require explicit trust promotion.
- Updates show file, permission, dependency, and publisher diffs.

### Evaluation

- format corpus covering common Agent Skills layouts;
- malicious archive corpus;
- trigger precision/recall tests;
- dependency and rollback tests;
- cross-platform path and encoding tests;
- deterministic package checksum and lockfile tests.

---

## 12. Workstream MCP-002: Connector Package Host

### Objective

Represent each MCP server as an installable connector package with authentication, permissions, protocol negotiation, health, update, and UI lifecycle.

### Connector records

```text
ConnectorPackage
ConnectorInstallation
McpRuntimeEndpoint
SecretReference
CapabilityGrant
ConnectorHealth
ConnectorVersionState
```

### Protocol support

Maintain compatibility with existing stdio, legacy SSE, and Streamable HTTP while adding conformance fixtures for the current MCP protocol generation. Implement capability negotiation instead of assuming tools-only behavior.

### Product surfaces

- Tools;
- Resources;
- Prompts;
- sampling/elicitation where supported;
- MCP Apps with sandboxed UI;
- Tasks/long-running work mapped to Nexa async jobs;
- OAuth and secret references;
- per-tool/resource grants;
- health, logs, restart policy, schema hash, and update history.

### Runtime isolation

- Connector subprocess/network permissions derive from package grants.
- Secrets are injected by reference and redacted from events.
- Tool schemas are validated and normalized before entering the Agent registry.
- Schema changes invalidate cached tool definitions and require a capability-diff review when permissions expand.

### Acceptance criteria

- Every enabled MCP server appears as a Package Host record.
- Disabling/uninstalling a connector removes its runtime capabilities atomically.
- Protocol negotiation and reconnect pass fixtures.
- Rich content remains typed rather than flattened to text.
- Long tasks survive reconnect and app restart.
- Connector health is visible without opening developer logs.

---

## 13. Workstream EXT-HUB-001: Extensions Hub

### Objective

Provide one discovery and lifecycle UX for Skills, Connectors, Apps, Workflows, and future Native Plugins without pretending they share one execution format.

### Shared UX

- installed/discover views;
- source and publisher;
- version and update channel;
- permissions and secrets;
- health and compatibility;
- enable/disable;
- update, rollback, uninstall;
- global/workspace/project scope;
- usage and recent failures;
- package files and provenance.

### Type-specific UX

- Skills: instructions, triggers, scripts, references.
- Connectors: endpoints, OAuth, tools/resources/prompts, health.
- Apps: sandbox and UI permissions.
- Workflows: triggers, schedules, inputs, outputs.
- Native plugins: signatures, platform/architecture, elevated risk.

The existing Package Host becomes the sole runtime assembler. Settings pages must stop independently rediscovering extension state.

---

# Part IV — Media and desktop intelligence

## 14. Workstream MEDIA-002: Meeting-grade video and audio analysis

### Objective

Upgrade the existing Whisper/frame/OCR pipeline into a speaker-aware, evidence-linked media timeline.

### Canonical pipeline

```text
probe
-> channel-preserving decode
-> normalization
-> VAD
-> ASR
-> word alignment
-> diarization
-> overlap detection
-> reconciliation
-> turns/chapters
-> meeting intelligence
-> visual timeline fusion
```

### Provider interfaces

```text
AsrProvider
AlignmentProvider
DiarizationProvider
VisualUnderstandingProvider
MeetingSummarizer
```

Local and cloud providers implement the same contracts. A Python/ONNX media worker is acceptable only behind a versioned JSON protocol with health, model inventory, progress, cancellation, and pinned dependencies.

### Data model

Add speakers, words, speaker turns, overlap regions, language/confidence, scenes, visual events, chapters, decisions, action items, open questions, and evidence links.

Speaker identities start anonymous. Naming or voiceprint matching is a separate consented operation.

### UI

- speaker-attributed editable transcript;
- waveform and active-speaker lanes;
- frame/slide/OCR lane;
- click any summary claim to jump to evidence;
- rename speaker with propagation;
- export Markdown, JSON, SRT, VTT, and structured minutes;
- local-only and redaction controls.

### Evaluation

WER, timestamp MAE, DER/JER, speaker-attributed WER, overlap recall, silence hallucination rate, real-time factor, memory, and summary/action-item evidence faithfulness.

---

## 15. Workstream CUA-002: Computer Use 2.0

### Objective

Evolve the current guarded Windows window-control backend into a cross-platform desktop agent runtime with pixel/accessibility fusion, recovery, and measured success.

### Perception service

Platform backends:

- Windows Graphics Capture + UI Automation;
- ScreenCaptureKit + macOS Accessibility;
- PipeWire/portal + AT-SPI.

Observations contain virtual desktop, monitors, windows, screenshot/tile references, changed regions, cursor, focus, accessibility tree, redaction regions, timestamp, and a short-lived observation ID.

### Action service

Canonical actions:

```text
move, click, double_click, drag, scroll,
type_text, key_down, key_up, key_chord,
focus_window, invoke_element, set_value, wait
```

Prefer semantic accessibility actions, verify with pixels, and use coordinates as a fallback. Add bounded action batches with divergence checks and automatic observations.

### Driver loop

```text
observe -> locate -> act -> verify -> recover/continue
```

The driver owns action count, elapsed-time budget, retries, unexpected dialogs, focus loss, DPI changes, occlusion, application hangs, and user takeover.

### Risk tiers

- observation;
- reversible navigation;
- data entry;
- external communication;
- authentication/secrets;
- purchase/deletion/installation and other high impact.

High-impact actions always require confirmation. Add application allowlists, protected-window detection, secret-field redaction, clipboard restrictions, and immutable action replay.

### Evaluation

Create a fixed desktop task suite across browsers, editors, file managers, settings, and Office-like apps. Track task success, target accuracy, retries, recovery, action latency, observation bytes, and user takeovers.

---

# Part V — Knowledge and graph

## 16. Workstream GRAPH-002: Evidence-first Graph 2.0

### Objective

Move from extraction visualization to a versioned, conflict-aware knowledge graph that participates in retrieval and preserves source evidence.

### New domain entities

```text
CanonicalEntity
EntityMention
EntityMergeDecision
RelationAssertion
Claim
TemporalInterval
EvidenceSpan
ExtractionRun
Community
CommunityReport
GraphSnapshot
```

### Required capabilities

- entity resolution, merge, split, and undo history;
- cross-document coreference;
- relation ontology, domain/range, and inverse relations;
- claim versus relation assertion separation;
- valid time and observed time;
- negation, conflict, and superseded states;
- extraction model/prompt/run lineage;
- incremental invalidation after source changes;
- communities and community summaries;
- local, global, and DRIFT-style retrieval;
- graph answer evidence from original text;
- worker-based layout and Canvas/WebGL rendering for large graphs.

### Evaluation

- entity precision/recall;
- canonicalization accuracy;
- relation precision/recall;
- evidence-span correctness;
- temporal/conflict accuracy;
- local/global retrieval quality;
- answer citation faithfulness;
- incremental-update correctness;
- graph rendering performance.

Graph retrieval must use the Retrieval Router and never replace original source evidence with unsupported graph summaries.

---

# Part VI — Usage, quality, and delivery

## 17. Workstream USAGE-002: Complete contribution analytics

The current PR adds the rolling activity grid. Follow-up work:

- show the actual rolling date range rather than a calendar-year label;
- return contribution and detail series in one backend request;
- click a day to drill into requests, models, operations, cost, and failures;
- add cost, cache hit, successful runs, agent time, and tool time modes;
- annotate app releases, model switches, provider changes, and configuration migrations;
- compare providers/models on aligned rows;
- preserve local timezone while storing canonical UTC boundaries;
- virtualize large detail ranges;
- export the selected contribution projection.

Acceptance requires timezone boundary tests, DST tests, empty ranges, very large outliers, deletion refresh, and accessible keyboard navigation.

---

## 18. Workstream QUALITY-001: Evals, telemetry, fixtures, and release gates

### Provider fixture framework

Record sanitized request/response fixtures for:

- sync and streaming success;
- tool calls and partial arguments;
- thinking/reasoning;
- usage and cache metrics;
- rate limits and transient errors;
- permission/access errors;
- malformed and incomplete streams;
- async submit/poll/cancel;
- expired artifact URLs.

Fixtures must be versioned by provider, API style, and model family.

### Capability scorecards

- Model adapter readiness;
- Computer Use task success;
- Meeting WER/DER/evidence;
- Retrieval recall/citation;
- Skills install and trigger quality;
- MCP conformance and health;
- Graph extraction/retrieval;
- turn/interaction recovery;
- shell portability.

### Telemetry principles

- local-first by default;
- no prompts, secrets, screenshots, audio, or file contents in aggregate telemetry;
- explicit opt-in for diagnostic bundles;
- stable event schemas;
- distinguish provider failure, adapter failure, model refusal, user cancellation, and policy denial.

### CI gates

Each provider/capability PR must run:

- schema and preset parsing;
- request/response fixtures;
- TypeScript and Rust contract tests;
- settings and card E2E tests;
- migration tests;
- architecture boundary checks;
- security/path/archive tests where applicable;
- capability-specific smoke evaluation.

---

## 19. Ordered PR plan

The work should be delivered as reviewable PRs in this dependency order.

### PR 0 — Current draft completion

Scope:

- contribution heatmap;
- reply folding fix;
- Question Card interaction fixes;
- detailed video result API;
- Qwen Image 3.0 preview catalog entry and guard tests;
- audit and roadmap documents.

Before ready-for-review:

- use a rolling-date subtitle;
- ensure all CI checks pass;
- verify Qwen image catalog rendering in settings;
- update PR description and screenshots if available.

### PR 1 — Model Descriptor v2 and endpoint registry

Scope only catalog contracts, migrations, projections, and tests. No new heavy provider adapter.

### PR 2 — Durable interaction runtime

DB states/events, resume API, Question/Approval integration, restart recovery, and UI invariants.

### PR 3 — Qwen Image 3.0 full adapter

Workspace endpoints, reference images, editing, seed/count/negative prompt, multi-output artifacts, access UX.

### PR 4 — Volcengine Ark text/agent provider

Provider preset, endpoint identity, Responses adapter, live discovery, Seed 2.1/2.0 metadata, tool and streaming fixtures.

### PR 5 — Shared shell service

Detection, settings, PTY integration, `run_shell`, prompt dialect, and cross-platform tests.

### PR 6 — Retrieval Router

Deterministic lanes, separate code/document indexes, telemetry, and benchmark harness.

### PR 7 — Skills package lifecycle

Universal intake, provenance, immutable install, dependencies, trust, updates, rollback, and installer UI.

### PR 8 — MCP Connector Package Host

Per-server package records, protocol negotiation, rich content, authentication/grants, health, and tasks.

### PR 9 — Volcengine Seedream image provider

Seedream catalog and provider-specific image/editing adapter using the shared artifact runtime.

### PR 10 — Async job runtime and Seedance video

Generic job persistence first, then Seedance submit/poll/cancel/artifacts and safety authorization.

### PR 11 — Meeting intelligence

Media worker contract, VAD/alignment/diarization, canonical timeline, UI, exports, and evals.

### PR 12 — Computer Use 2.0 foundation

Observation/action schemas, Windows accessibility fusion, driver loop, telemetry, and internal task suite. macOS/Linux follow as isolated backend PRs.

### PR 13 — Graph 2.0 domain and retrieval

Canonical entities/claims/evidence/temporal model, migrations, entity resolution, communities, retrieval lanes, and scalable UI.

### PR 14 — Extensions Hub

Unified catalog/lifecycle UI after Skills and Connectors both use Package Host v2.

### PR 15 — Volcengine speech and embeddings

RealtimeVoice, TTS, voice cloning, interpretation, text embedding, and multimodal embedding as separate capability packages.

---

## 20. Release gates

### Foundation gate

- Model Descriptor v2 merged.
- Durable interactions merged.
- Async job/artifact contract approved.
- Package Host v2 contract approved.
- Provider fixture framework active.

### Provider gate

- Qwen Image 3.0 full tests pass.
- Volcengine text/agent provider passes streaming/tool/usage fixtures.
- Catalog discovery and lifecycle UI work with restricted credentials.

### Extensions gate

- Skills and MCP both use Package Host state.
- Permissions, provenance, updates, rollback, and health are visible.
- No settings page privately assembles runtime capabilities.

### Intelligence gate

- Retrieval Router benchmark passes agreed thresholds.
- Meeting WER/DER/evidence scorecard is available.
- Computer Use task suite reports success/recovery/latency.
- Graph local/global answers retain source evidence.

### Product gate

- No preview model is an implicit default.
- Pending interactions survive restart.
- Long jobs survive restart and can be cancelled.
- Provider failures are actionable and correctly classified.
- Existing user configurations migrate without data loss.

---

## 21. Immediate next actions

1. Finish PR #264 validation and correct the rolling-date label.
2. Add issue/epic records matching the ordered PR plan.
3. Implement Model Descriptor v2 before adding broad ByteDance model lists.
4. Build the Volcengine Ark Responses fixture adapter with the officially documented Seed 2.0 Lite request as the first compatibility fixture.
5. Extend Qwen image generation to full 3.0 editing semantics.
6. Add the provider catalog drift report job.
7. Start the durable interaction runtime in parallel because Computer Use, approvals, and long jobs all depend on it.

## 22. Source verification record

External model decisions in this roadmap were checked against official provider material available on 2026-08-01, including:

- Alibaba Cloud Model Studio — Qwen Image Generation and Editing 3.0 API Reference;
- Alibaba Cloud Model Studio — Text Generation, Visual Understanding, Omni-modal, Realtime, Speech Synthesis, and Model Lifecycle documentation;
- Volcengine — Doubao and Ark product/model pages;
- Volcengine Ark — Responses API getting-started documentation;
- Volcengine official release/product material for Seed 2.x, Seedream 5.x, and Seedance 2.0.

Where official pages identify a model family but do not expose a stable API snapshot ID, this roadmap intentionally requires live discovery rather than inventing an identifier.
