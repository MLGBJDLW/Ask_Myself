# Wave 3 Vision Router and Observation Contract: Primary-source Research

This note records the primary-source review for PR 10,
`vision-router-and-observation-contract`, from the Wave 3 work described in
`D:\Nexa.txt` lines 595-714, 1309-1320, and 1356-1366. It was prepared on
2026-08-06 against immutable upstream commits. It is a design input for an
independent Nexa implementation. No upstream implementation is copied into
Nexa.

## Executive decision

PR 10 should replace the current binary attachment branch with one durable
Vision Router that is independent of the primary reasoning model:

1. `vision` is a real Capability Registry binding. A text-only reasoning model
   may consume observations produced by a separately selected vision target.
   Selecting that target is an explicit user action; migration must never add
   a new cloud image recipient silently.
2. A pure classifier produces a finite route plan before any provider call.
   Dense text, scans, receipts, and exact-transcription requests prefer local
   OCR. Photos, charts, diagrams, UI, and spatial reasoning prefer vision.
   Mixed or uncertain inputs may run local OCR and then a vision supplement.
3. The main reasoning model receives a validated `VisionObservationV1`, not an
   auxiliary model's free-form response. Invalid or oversized structured
   output is a failed vision attempt and follows the declared fallback plan.
4. Raw image bytes go to no more than one remote provider for one plan unless
   the user explicitly permits a cross-provider fallback. Derived observation
   text may still cross the primary model boundary, so the recorded privacy
   scope must distinguish local, single-provider, and multi-provider routes.
5. The observation cache key is the attachment hash plus a secret-free vision
   profile hash. The profile freezes the observation contract, intent class,
   OCR settings, route policy, selected target identities and revisions, and
   privacy choices. Cache defaults and deletion controls remain visible.
6. `Off`, `Ask every time`, `Auto`, and `Always auxiliary` are separate from
   the Capability Registry's disabled/ask/automatic *failure fallback* mode.
   The first controls how an attachment is interpreted; the second controls
   whether a failed selected target may advance to another target.
7. Rollback is reversible. The legacy direct-image/OCR branch remains callable
   behind the existing Registry read pointer until the vision runtime and its
   activation contract are validated.

## Reviewed upstream revisions

| Project | Immutable revision | Evidence used |
| --- | --- | --- |
| Unstructured | [`114a1d511df49e8680e9608b14ee85dbd2c480dd`](https://github.com/Unstructured-IO/unstructured/commit/114a1d511df49e8680e9608b14ee85dbd2c480dd) | A finite PDF/image strategy selector; deterministic auto routing; explicit dependency-aware fallback and terminal failure |
| PaddleOCR | [`2661c7c0ef5c613e8f93c6e93b2e052399f0f854`](https://github.com/PaddlePaddle/PaddleOCR/commit/2661c7c0ef5c613e8f93c6e93b2e052399f0f854) | Structured OCR JSON containing detection/recognition polygons, thresholds, text, and per-region scores |
| Docling Core | [`2cae21e3f8f4ba0198bf4605b43bf105efe60a04`](https://github.com/docling-project/docling-core/commit/2cae21e3f8f4ba0198bf4605b43bf105efe60a04) | A versioned document object with text, pictures, tables, charts, metadata, bounding boxes, and source provenance |
| OpenAI Agents SDK for Python | [`f3b6c617853880b6dbad16b58ff9d071d5756afb`](https://github.com/openai/openai-agents-python/commit/f3b6c617853880b6dbad16b58ff9d071d5756afb) | Output schemas are separate from plain text and validate model JSON strictly into a typed result |
| LangGraph | [`658541c4960f329864a2523fc7d52427e8190bed`](https://github.com/langchain-ai/langgraph/commit/658541c4960f329864a2523fc7d52427e8190bed) | Conditional routes declare a closed path map; graph construction validates nodes and terminal edges |

The reviewed code is Apache-2.0 (Unstructured and PaddleOCR) or MIT (Docling
Core, OpenAI Agents SDK, and LangGraph). Nexa adopts the architectural ideas
below and keeps its own Rust/TypeScript implementation. No secondary summary
is used as design evidence.

## Requirements traced from `D:\Nexa.txt`

| Wave 3 requirement | PR 10 exit evidence |
| --- | --- |
| Text-only primary model can use auxiliary vision | A runtime `vision` binding resolves and pins independently of `text_generation`; its structured observation becomes a text part for the primary turn |
| OCR/Vision heuristic is not one fixed order | A deterministic classifier and route planner select OCR-first, vision-first, mixed, native-direct, local-only, or metadata-only from policy and attachment evidence |
| `VisionObservation` is structured | One versioned Rust/TypeScript contract validates summaries, OCR regions, tables, entities, chart data, source provenance, confidence semantics, fallback reason, and privacy scope |
| User modes and preferred model | Settings expose Off/Ask/Auto/Always auxiliary, an eligible Registry target, ordered fallback contract, local preference, cross-provider consent, and cache controls |
| Conversation status chip and actions | Each persisted attachment carries a compact observation reference/status; the detail surface can inspect results, retry OCR or vision, open model selection, and delete cached data |
| Cache by attachment and profile | SHA-256 attachment identity plus BLAKE3 profile identity is unique, secret-free, revision-aware, bounded, and explicitly deletable |
| No silent privacy regression | Migration preserves native vision and local OCR behavior; no auxiliary cloud target activates without an explicit binding or a same-target native parity match |

## Reviewed Nexa seams

Nexa has all transport primitives needed by PR 10, but the ownership and
observation contracts are missing:

- [`desktop_agent_session.rs`](../../apps/desktop/src-tauri/src/desktop_agent_session.rs)
  currently checks only whether the primary model supports vision. It sends
  the raw image directly when true and otherwise calls OCR. There is no
  attachment classifier, independent vision binding, per-turn route choice,
  structured observation, or reusable result cache.
- The non-vision branch calls `extract_text_from_image(..., None)`. The OCR
  module therefore cannot use its optional LLM fallback in this path even
  though the setting says that fallback is enabled.
- [`ocr.rs`](../../crates/core/src/ocr.rs) already produces text regions,
  bounding boxes, and recognition confidence. Its LLM fallback instead turns
  arbitrary model text into one page-sized region and assigns confidence
  `1.0`; PR 10 must not represent that synthetic value as calibrated evidence.
- [`capability_registry/resolver.rs`](../../crates/core/src/capability_registry/resolver.rs)
  already maps `vision` to an image-input requirement and can identify
  eligible model targets. Runtime activation is still limited to
  `text_generation`, so the route cannot currently be selected or pinned.
- [`settings_schema_v2.rs`](../../crates/core/src/settings_schema_v2.rs)
  already owns primary/fallback targets, constraints, and an extensible options
  map. Vision policy belongs in that binding, not in another Provider form.
- OpenAI-compatible, Anthropic, Gemini, and Ollama adapters already serialize
  `ContentPart::Image`. PR 10 should reuse those adapters and must not add a
  parallel set of provider-specific image clients.
- [`ImageAttachment`](../../crates/core/src/conversation/mod.rs) persists only
  base64 data, media type, and name. It has no stable attachment identity or
  observation status, so it cannot address a cache entry or render a durable
  status chip.

## 1. Use a closed route plan, not nested fallback conditionals

### Primary-source evidence

Unstructured represents its supported document strategies as a closed set and
rejects invalid combinations before work begins
([strategy validation](https://github.com/Unstructured-IO/unstructured/blob/114a1d511df49e8680e9608b14ee85dbd2c480dd/unstructured/partition/strategies.py#L7-L21)).
Its auto policy chooses a concrete strategy from input properties, then applies
dependency-aware fallback and raises when no valid processor exists
([strategy resolution](https://github.com/Unstructured-IO/unstructured/blob/114a1d511df49e8680e9608b14ee85dbd2c480dd/unstructured/partition/strategies.py#L24-L95)).

LangGraph likewise records conditional destinations as an explicit map or a
typed closed return set
([conditional-edge contract](https://github.com/langchain-ai/langgraph/blob/658541c4960f329864a2523fc7d52427e8190bed/libs/langgraph/langgraph/graph/state.py#L962-L1018)).
Its graph builder rejects invalid start/end nodes and missing destinations
before execution
([edge validation](https://github.com/langchain-ai/langgraph/blob/658541c4960f329864a2523fc7d52427e8190bed/libs/langgraph/langgraph/graph/state.py#L913-L960)).

### Nexa decision

The classifier returns evidence; it does not call OCR or a provider. The
planner consumes evidence plus one frozen policy and returns one of these
closed plans:

```rust
enum VisionRoutePlan {
    MetadataOnly,
    NativeDirect,
    OcrOnly,
    VisionOnly,
    OcrThenVision,
    VisionThenOcr,
}
```

Every plan contains its terminal/fallback edges. Execution must not invent a
new provider or reverse the plan after a failure. A route trace records the
classifier evidence, selected plan, attempted processors, cache result, and
stable reason codes.

The decision matrix is:

| Policy/input | Initial plan | Allowed continuation |
| --- | --- | --- |
| Off | Metadata only | Explicit per-turn OCR or vision override only |
| Ask without answer | No execution; `decision_required` | Resume only with the same attachment hash and a user route choice |
| Native-capable primary, Auto, ordinary visual request | Native direct | No auxiliary provider; OCR supplement only when exact text is requested and locally enabled |
| Dense text/scan/receipt/screenshot/exact transcription | OCR only | Vision supplement only on low confidence, missing text, or complex-layout evidence |
| Photo/chart/diagram/UI/spatial reasoning | Vision only | OCR supplement for requested exact text or a retryable vision failure |
| Mixed or uncertain | OCR then vision | Merge typed evidence; skip the remote call if local OCR conclusively satisfies a text-only intent |
| Local-only privacy | Local OCR or selected local vision | Never cross to a cloud target |
| Always auxiliary | Selected vision target | Declared OCR/fallback edges only; primary never receives the raw image |

`Ask every time` is resolved before provider execution. The send surface must
show OCR, Vision, and Auto choices and must not persist a failed turn merely to
ask the question. The answer is stored with the launch artifacts and bound to
the attachment hashes so a forged or stale answer cannot authorize a new
image.

## 2. Structured observations are the boundary

### Primary-source evidence

PaddleOCR's result writer keeps detection polygons, recognition polygons,
recognized text, per-region scores, and the score threshold as separate JSON
fields
([structured OCR result](https://github.com/PaddlePaddle/PaddleOCR/blob/2661c7c0ef5c613e8f93c6e93b2e052399f0f854/deploy/cpp_infer/src/pipelines/ocr/result.cc#L310-L384)).
This is materially safer than flattening all evidence into one string because
the consumer can preserve location and confidence semantics.

Docling Core separates text, pictures, tables, and other document items in one
versioned document object
([document collections](https://github.com/docling-project/docling-core/blob/2cae21e3f8f4ba0198bf4605b43bf105efe60a04/docling_core/types/doc/document.py#L141-L176)).
Extracted items point back to source page, bounding box, and character span
([provenance model](https://github.com/docling-project/docling-core/blob/2cae21e3f8f4ba0198bf4605b43bf105efe60a04/docling_core/types/doc/common/reference.py#L165-L177)).
Its picture data is a discriminated union that can represent descriptions,
classification, tabular charts, several chart types, and other annotations
([picture data union](https://github.com/docling-project/docling-core/blob/2cae21e3f8f4ba0198bf4605b43bf105efe60a04/docling_core/types/doc/items/picture/picture.py#L37-L69)).

The OpenAI Agents SDK makes the output schema an explicit object and validates
JSON into the declared type
([schema interface](https://github.com/openai/openai-agents-python/blob/f3b6c617853880b6dbad16b58ff9d071d5756afb/src/agents/agent_output.py#L18-L54),
[strict validation](https://github.com/openai/openai-agents-python/blob/f3b6c617853880b6dbad16b58ff9d071d5756afb/src/agents/agent_output.py#L120-L172)).
The relevant principle is local validation: a prompt that asks for JSON is not
itself an output contract.

### Nexa decision

Use one versioned cross-language contract:

```rust
struct VisionObservationV1 {
    schema_version: u16,
    attachment_id: String,
    attachment_hash: String,
    profile_hash: String,
    intent: VisionIntent,
    summary: Option<String>,
    ocr_text: Option<String>,
    regions: Vec<VisionRegion>,
    tables: Vec<ExtractedTable>,
    entities: Vec<ExtractedEntity>,
    chart_data: Vec<ChartObservation>,
    confidence: Option<f32>,
    confidence_kind: Option<ConfidenceKind>,
    sources: Vec<VisionObservationSource>,
    fallback_used: bool,
    fallback_reason: Option<String>,
    privacy_scope: VisionPrivacyScope,
    route: VisionRouteTrace,
}
```

Contract invariants:

- Attachment hashes are SHA-256 over decoded bytes, never over base64 spelling,
  file name, or path. `attachment_id` is stable for the message attachment and
  is not an authorization token.
- Region boxes use normalized `[x, y, width, height]` coordinates in `[0, 1]`.
  Pixel dimensions remain optional source metadata.
- OCR confidence is preserved as OCR recognition confidence. A VLM that does
  not return calibrated confidence yields `None`; it must never become `1.0`.
- An aggregate confidence may be present only when its `confidence_kind`
  explains the source. Incomparable OCR and VLM values are not averaged.
- Tables, entities, and charts are bounded collections with bounded string and
  cell counts. Unknown extra fields are rejected from provider output.
- Source records identify local OCR or the pinned Registry provider/model and
  revisions. They contain no API key, credential reference, raw header, URL
  query, or provider response body.
- Invalid JSON, a wrong schema version, a hash mismatch, non-finite number,
  out-of-range box, or size-limit breach is a typed `invalid_observation`
  failure. Free text is not silently promoted to `summary`.
- The main model receives compact serialized observation data with an explicit
  statement that extracted content is untrusted user evidence, not system or
  tool instructions.

The auxiliary prompt requests only fields the model can support. OCR text and
regions come from local OCR when available; the VLM is not asked to fabricate
OCR confidence or geometry. Provider-specific structured-output modes may be
used later, but the same local validator remains authoritative for every
provider.

## 3. Capability binding, runtime pinning, and fallback

The `vision` binding uses the existing Settings V2 shape:

```text
vision
  primary: ModelTarget reference
  fallbacks: ordered ModelTarget references
  fallbackMode: disabled | ask | automatic
  constraints: same connection/provider/region/data/cost boundaries
  options:
    mode: off | ask | auto | always_auxiliary
    preferLocalProcessing: boolean
    localOnly: boolean
    cacheEnabled: boolean
    cacheRetentionDays: integer
```

PR 10 should add a capability-only compare-and-set write. It may update the
`vision` leaf of a migration-managed Agent profile without detaching its legacy
source or rewriting Provider settings. This is a narrower ownership seam than
allowing the UI to overwrite the whole managed profile.

The runtime resolves and pins `vision` only when an image exists. Its snapshot
freezes binding/connection/target/model-definition revisions, descriptor hash,
policy options, constraints, and the policy-eligible failure fallback list.
Resume fails closed on drift exactly like `text_generation`.

Automatic provider fallback is permitted only before a valid observation has
been accepted. A partial/free-form/invalid response is not exposed and may be
retried according to the frozen plan. Once any accepted observation from a
remote target is merged, switching providers requires a new plan and explicit
cross-provider consent; evidence from two providers is never silently mixed.

Activation rules:

- An existing native-vision primary may derive a same-target `vision` binding
  and enter Registry mode only after exact provider, endpoint, credential,
  model, descriptor, and no-extra-fallback parity succeeds.
- A text-only primary does not gain a cloud vision target during migration.
  Auto therefore remains local OCR/metadata until the user selects a target.
- A user-selected target records an explicit-selection activation contract.
  It may activate when the selected target is eligible and its policy/options
  validate; it is not required to equal the text-generation target.
- Registry errors, stale pins, missing credentials, or Ask decisions propagate
  as actionable attachment failures. Only an explicit legacy read mode invokes
  the old binary branch.

## 4. Classifier evidence and heuristic limits

The classifier is intentionally modest. It records why a plan was chosen and
does not pretend that a filename or aspect ratio proves document type.

Inputs:

- explicit per-turn user choice;
- policy mode and privacy constraints;
- primary and auxiliary model capabilities/locality;
- MIME type, byte length, decoded dimensions, animation status, and file-name
  hints;
- normalized user-intent terms for exact transcription, receipts/documents,
  charts/diagrams/UI, comparison, description, and spatial reasoning;
- optional lightweight local OCR evidence: text count, region count, mean/min
  confidence, and normalized text-region coverage.

Outputs include an intent class (`dense_text`, `visual_reasoning`, `mixed`, or
`unknown`), confidence in the *route classification*, reason codes, and a
closed plan. Route confidence is not observation confidence.

The heuristic must be conservative:

- filename/prompt hints can increase evidence but cannot authorize remote
  egress or override local-only policy;
- low OCR confidence does not prove that a VLM will understand the image;
- high OCR confidence can satisfy exact-text intent but does not prove chart,
  layout, or spatial understanding;
- animated images and unsupported/oversized data fail before any provider
  call;
- cancellation is checked before OCR, before every provider call, and before
  cache persistence.

## 5. Cache identity, lifecycle, and privacy

The cache identity is:

```text
attachment_hash = sha256(decoded_attachment_bytes)
profile_hash = blake3(canonical_json({
  observation_schema_version,
  classifier_version,
  intent_class,
  route_mode,
  ocr_config_subset,
  selected_target_ids_and_revisions,
  binding_revision,
  privacy_options,
}))
cache_key = attachment_hash + ":" + profile_hash
```

Secret material, credential references, base64 data, display names, timestamps,
and provider response IDs are excluded from the profile. Canonical JSON uses
sorted keys and stable enum spellings.

The database stores the validated observation, compact route trace, timestamps,
and expiry. It does not duplicate image bytes. A cache hit revalidates the
contract and updates `last_used_at`; corrupt rows are removed and recomputed.
The default is local persistent cache with a visible 30-day retention. Users
can disable new cache writes, delete one attachment/profile result, delete all
results for one image, or clear the full observation cache. Expired rows are
deleted opportunistically on lookup/insert and through a startup maintenance
hook.

Conversation attachment JSON stores stable identity plus a compact observation
status/reference so history remains renderable. The detail view loads the
validated cache row. Deleting a result clears the cache and the message
reference; it does not delete the original user attachment unless the user
separately deletes the message/conversation.

Privacy scope is computed, not accepted from provider output:

- `local`: raw image and derived content stay in local OCR/local-model paths;
- `single_provider`: one remote provider receives the raw image and the primary
  reasoning provider is the same provider identity;
- `multi_provider`: the raw image provider and primary reasoning provider
  differ, or an explicitly consented provider fallback contributes evidence.

The UI shows this scope before retrying a route that increases egress.

## 6. Settings and conversation UX

The Capability Registry panel owns Image Understanding model selection and
failure fallback. OCR model installation remains in Models/Media because it is
local runtime readiness, not a cloud Connection.

The compact settings surface shows:

```text
Image understanding   Auto (recommended)
Preferred model       <eligible vision target>
Fallback              Disabled / Ask / Automatic
Privacy               Prefer local; Local only; Allow cross-provider fallback
Cache                 On; 30 days; Clear cached observations
```

The target picker lists only image-input-eligible targets and displays
Connection, locality, availability, and lifecycle. It never exposes a secret.
Saving uses expected profile/target revisions and reports a conflict instead
of overwriting concurrent changes.

When an attachment is present, the composer shows the effective route policy.
Auto can be overridden to OCR only or Vision only. Ask has no default choice
and blocks send until the user selects one. The choice is bound to the current
attachment hashes.

After processing, the image displays a small status chip such as:

```text
Vision: Gemini - complete
OCR - 94% confidence
Local OCR only
```

The detail surface shows the observation, provenance, route reason, cache age,
and privacy scope. Actions are: rerun Auto, OCR only, or Vision only; open the
vision target selector; and delete cached observation. Reduced-motion behavior
and keyboard focus follow the existing Settings and Modal primitives.

## 7. Failure model and rollback

Stable failure reason codes include:

- `decision_required`
- `vision_disabled`
- `attachment_decode_failed`
- `attachment_too_large`
- `unsupported_media_type`
- `ocr_unavailable`
- `ocr_low_confidence`
- `vision_binding_missing`
- `vision_target_ineligible`
- `vision_invocation_failed`
- `invalid_observation`
- `cross_provider_consent_required`
- `local_only_route_unavailable`
- `stale_runtime_snapshot`
- `cancelled`

User-facing text is derived from these codes. Raw provider error bodies,
credentials, image bytes, and OCR contents are never logged. Diagnostics may
record sizes, hashes truncated for correlation, model/target IDs, latency, and
route reason codes.

Rollback changes the durable `vision` read pointer to legacy. It preserves
Settings V2 bindings and cache rows for diagnosis. The user can separately
clear cached observations. A legacy rollback does not authorize a new
auxiliary provider and retains the pre-PR direct-native/OCR behavior.

## 8. Test and rollout contract

Core tests:

- each explicit mode and per-turn override;
- exact-text, dense-text, visual, mixed, unknown, and local-only decisions;
- OCR confidence/coverage thresholds and no VLM confidence fabrication;
- strict structured-output parsing, bounds, collection/size limits, and wrong
  attachment/profile hash rejection;
- observation merge with source and privacy provenance;
- canonical profile hash stability and invalidation on every relevant revision
  or policy change;
- cache hit, corrupt-row eviction, expiry, per-image delete, and clear-all;
- no automatic provider advance after accepting evidence;
- cross-provider/local-cloud/data constraints and pinned-revision drift;
- migration parity, explicit-selection activation, rollback, and resume.

Desktop/runtime tests:

- a text-only primary receives a compact observation and no raw image;
- a native-vision primary in Auto receives one raw image and makes no auxiliary
  call;
- OCR-first can skip the cloud call when it satisfies exact-text intent;
- mixed routing invokes local OCR and one selected vision target;
- Ask blocks before launch until the choice covers the current hashes;
- cancellation stops before cache persistence;
- attachment status survives message reload and cache deletion removes it;
- no event/log/IPC projection contains credentials or raw provider errors.

Frontend tests:

- eligible target filtering and exact revision save;
- mode, privacy, fallback, cache defaults, and clear controls;
- composer Ask gating and per-turn route override;
- compact status chip, keyboard-accessible detail view, retry actions, and
  reduced-motion behavior;
- legacy attachment JSON without identity/status remains renderable.

Rollout order:

1. Add the versioned observation types, classifier, validator, and cache table
   behind tests; runtime remains legacy.
2. Add the capability-only CAS write and secret-free Settings UI; keep the
   `vision` activation pointer legacy until its contract matches.
3. Enable pinned `vision` resolution and run the router for attachments.
4. Add status/detail/retry/delete UX and compatibility hydration for legacy
   attachments.
5. Run focused and full Rust/frontend/Playwright checks, then perform the
   capped Standards and Spec P1 review before opening the stacked PR.

## License and integration boundary

| Project | License evidence | Nexa boundary |
| --- | --- | --- |
| Unstructured | [Apache-2.0](https://github.com/Unstructured-IO/unstructured/blob/114a1d511df49e8680e9608b14ee85dbd2c480dd/LICENSE.md) | Reimplement a finite strategy/planner vocabulary; do not copy Python routing or dependency probes |
| PaddleOCR | [Apache-2.0](https://github.com/PaddlePaddle/PaddleOCR/blob/2661c7c0ef5c613e8f93c6e93b2e052399f0f854/LICENSE) | Keep Nexa's existing ONNX engine; align the local result contract with text/score/region separation |
| Docling Core | [MIT](https://github.com/docling-project/docling-core/blob/2cae21e3f8f4ba0198bf4605b43bf105efe60a04/LICENSE) | Reimplement a compact single-image observation, not Docling's document model or serializers |
| OpenAI Agents SDK | [MIT](https://github.com/openai/openai-agents-python/blob/f3b6c617853880b6dbad16b58ff9d071d5756afb/LICENSE) | Apply strict local typed validation across all Nexa providers; do not copy Pydantic helpers |
| LangGraph | [MIT](https://github.com/langchain-ai/langgraph/blob/658541c4960f329864a2523fc7d52427e8190bed/LICENSE) | Reimplement a small closed Rust plan enum; do not add LangGraph or copy its graph runtime |

No new upstream runtime dependency is required by this design. If any source is
later copied rather than independently implemented, its notice and license
obligations must be reviewed in that change.
