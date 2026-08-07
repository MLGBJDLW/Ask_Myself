# Wave 4 video Shot Board, queue, and compare primary-source research

Date: 2026-08-07

## Decision summary

PR 15 should add a project-scoped, durable video workflow aggregate whose
first renderer is a structured Shot Board. The aggregate owns an ordered shot
list, each shot owns ordered generation variants, and each shot has at most one
durable selected variant. A compact queue projects the existing PR 13 media job
and attempt lifecycle; it must not invent a second job-state machine.

The persisted authoring model is graph-shaped even though the first UI is not a
node editor. Stable IDs and explicit references preserve the future DAG seam:

```text
Brief
  -> Shot
       -> Prompt
       -> ReferenceAsset(s)
       -> GenerateVideo variant 1..n
       -> SelectVariant
```

Shot order, variant order, and `selectedVariantId` are product data, not
transient React state. A retry creates another durable PR 13 attempt while
preserving the failed attempt. Cancellation stays a request until the runtime
or provider confirms it. A cross-provider fallback is a separately disclosed
attempt and is impossible without consent scoped to the affected shot inputs
and destination provider.

PR 15 should not add a general node canvas, duplicate provider adapters, or
implement Timeline/FFmpeg export. The simple Timeline and export remain PR 16.
This is an original Nexa design. No upstream implementation source was copied.

## Acceptance boundary from `D:\Nexa.txt`

This note covers the `video-shot-board-queue-compare` PR and these explicit
requirements:

- a Brief and structured Shot Board instead of a single prompt box;
- ordered shots with prompt, reference assets, provider/model, and state;
- multiple variants per shot;
- an asynchronous generation queue with status, estimated cost, retry, and
  cancellation;
- variant comparison followed by an explicit durable selection;
- a typed graph representation under the structured UI, without exposing a
  complex node editor in the MVP; and
- visible provider, data-region, retention/deletion, watermark, provenance, and
  fallback facts before assets leave the device.

It intentionally leaves these items out of PR 15:

- Timeline tracks, trimming, concatenation, preview render, FFmpeg, and export;
- Extend, Edit, Upscale, GenerateAudio, and other Wave 5 graph nodes;
- additional provider adapters and direct ByteDance or Veo integration; and
- automatic storyboard/script generation.

## Source snapshots

All GitHub source and license links below are pinned to the 40-character commit
observed on 2026-08-07. Moving branch links are not used as evidence.

| Source | Pinned revision | License | Evidence used |
| --- | --- | --- | --- |
| ComfyUI [dynamic graph and topological execution](https://github.com/Comfy-Org/ComfyUI/blob/0ab8332bfa41c695b1c104a6535ff1fde81c7939/comfy_execution/graph.py), [prompt executor and queue](https://github.com/Comfy-Org/ComfyUI/blob/0ab8332bfa41c695b1c104a6535ff1fde81c7939/execution.py), and [job cancellation API](https://github.com/Comfy-Org/ComfyUI/blob/0ab8332bfa41c695b1c104a6535ff1fde81c7939/server.py) | `0ab8332bfa41c695b1c104a6535ff1fde81c7939` | [GPL-3.0](https://github.com/Comfy-Org/ComfyUI/blob/0ab8332bfa41c695b1c104a6535ff1fde81c7939/LICENSE) | Stable node/link graph inputs, dependency execution, queued/running/history separation, priority ordering, and cancellation that distinguishes queued removal from running interruption. |
| InvokeAI [validated graph and execution state](https://github.com/invoke-ai/InvokeAI/blob/7b42cd5104f6d719a78161864d073e74061af37f/invokeai/app/services/shared/graph.py), [versioned workflow records](https://github.com/invoke-ai/InvokeAI/blob/7b42cd5104f6d719a78161864d073e74061af37f/invokeai/app/services/workflow_records/workflow_records_common.py), [queue contract](https://github.com/invoke-ai/InvokeAI/blob/7b42cd5104f6d719a78161864d073e74061af37f/invokeai/app/services/session_queue/session_queue_common.py), [SQLite queue and retries](https://github.com/invoke-ai/InvokeAI/blob/7b42cd5104f6d719a78161864d073e74061af37f/invokeai/app/services/session_queue/session_queue_sqlite.py), and [queue API](https://github.com/invoke-ai/InvokeAI/blob/7b42cd5104f6d719a78161864d073e74061af37f/invokeai/app/api/routers/session_queue.py) | `7b42cd5104f6d719a78161864d073e74061af37f` | [Apache-2.0](https://github.com/invoke-ai/InvokeAI/blob/7b42cd5104f6d719a78161864d073e74061af37f/LICENSE) | Durable queue rows, explicit statuses, per-item ownership, bounded enqueue, transactional claim, status sequence, retry-as-new-record with `retried_from_item_id`, and cancel endpoints. |
| InvokeAI [gallery compare state](https://github.com/invoke-ai/InvokeAI/blob/7b42cd5104f6d719a78161864d073e74061af37f/invokeai/frontend/web/src/features/gallery/store/gallerySlice.ts), [compare candidate action](https://github.com/invoke-ai/InvokeAI/blob/7b42cd5104f6d719a78161864d073e74061af37f/invokeai/frontend/web/src/features/gallery/components/ContextMenu/MenuItems/ContextMenuItemSelectForCompare.tsx), [comparison renderer](https://github.com/invoke-ai/InvokeAI/blob/7b42cd5104f6d719a78161864d073e74061af37f/invokeai/frontend/web/src/features/gallery/components/ImageViewer/ImageComparison.tsx), and [comparison toolbar](https://github.com/invoke-ai/InvokeAI/blob/7b42cd5104f6d719a78161864d073e74061af37f/invokeai/frontend/web/src/features/gallery/components/ImageViewer/CompareToolbar.tsx) | `7b42cd5104f6d719a78161864d073e74061af37f` | Apache-2.0 | Slider, side-by-side, hover, swap, fit, and explicit compare-candidate controls; also direct evidence that compare/selection UI state is intentionally transient and image-only in this implementation. |
| OpenTimelineIO [timeline structure](https://github.com/AcademySoftwareFoundation/OpenTimelineIO/blob/0eebd211b2055f111e2c53d04b5581adc594c1fc/docs/tutorials/otio-timeline-structure.md), [ordered composition children](https://github.com/AcademySoftwareFoundation/OpenTimelineIO/blob/0eebd211b2055f111e2c53d04b5581adc594c1fc/src/opentimelineio/composition.h), and [clip media-reference selection](https://github.com/AcademySoftwareFoundation/OpenTimelineIO/blob/0eebd211b2055f111e2c53d04b5581adc594c1fc/src/opentimelineio/clip.h) | `0eebd211b2055f111e2c53d04b5581adc594c1fc` | [Apache-2.0](https://github.com/AcademySoftwareFoundation/OpenTimelineIO/blob/0eebd211b2055f111e2c53d04b5581adc594c1fc/LICENSE.txt) | Ordered children, explicit clip order, multiple named media references, and a separate active media-reference key. |

## Evidence-to-design mapping

### 1. Persist a structured authoring model; compile it to a DAG

ComfyUI's `DynamicPrompt` retains an original prompt graph and supports
ephemeral expansion, while `ExecutionList` performs a topological dissolve of
the dependency graph. InvokeAI's `Graph` separately persists stable node IDs and
typed edges, validates graph integrity, and keeps execution progress in a
`GraphExecutionState`. These projects demonstrate that authoring structure and
runtime execution state are different concepts.

Nexa should preserve the same boundary without copying either node-editor
model. The durable authoring aggregate is:

```text
VideoWorkflow
  id, projectId, title, brief, aspectRatio, targetDuration, revision
  shots[] ordered by ordinal

VideoShot
  id, workflowId, ordinal, title, prompt, operation
  referenceAssetIds[] ordered
  provider/model/API snapshot
  requested duration/resolution/aspect/seed/audio
  privacy disclosure snapshot
  variants[] ordered
  selectedVariantId nullable

VideoVariant
  id, workflowId, shotId, ordinal, label
  mediaJobId
```

The aggregate's stable IDs form the graph identity. A deterministic compiler
can project each shot to `Prompt`, zero or more `ReferenceAsset`, one
`GenerateVideo` node per variant, and one `SelectVariant` node. PR 15 does not
need public arbitrary node creation, graph coordinates, ports, or canvas
layout. Future node-editor metadata can be added without changing shot,
variant, media-job, or asset identities.

Every aggregate edit should be revision-checked. Reordering updates ordinals in
one transaction and preserves IDs. A queued variant captures the normalized
shot/provider snapshot used to create its PR 13 media job; later editing of the
shot never retroactively changes an already-submitted request.

### 2. Treat order and selection as durable domain facts

OpenTimelineIO represents a simple cut list as one `Track` containing `Clip`
children in order. Its `Composition` owns an ordered child vector and exposes
index-aware insertion. More importantly for generated variants, OTIO `Clip`
schema 2 can hold multiple named media references and stores a separate
`active_media_reference_key`.

Nexa should adapt those two ideas:

- `ordinal` is explicit for both shots and variants;
- variant identity is independent of its current order;
- every generated variant remains linked to its media job and verified output
  assets, including non-selected variants;
- `selectedVariantId` is a nullable foreign identity owned by the shot; and
- selection is rejected unless the variant belongs to that shot and has a
  completed, locally verified output.

The Compare panel may have transient left/right candidates, playback position,
zoom, and layout mode. Those UI facts are not the selection. The only action
that changes downstream composition is an explicit `Select` command that
revision-checks the shot and persists `selectedVariantId`, `selectedAt`, and
optionally the selecting actor.

This intentionally improves on InvokeAI's inspected compare implementation.
InvokeAI offers useful slider, side-by-side, hover, swap, and fit interactions,
but `gallerySlice` places both `selection` and `imageToCompare` on its persistence
denylist. It also clears `imageToCompare` when the active item is a video. Nexa
can adapt the compact comparison modes, but cannot use ephemeral gallery state
as workflow authority and must implement video-aware synchronized comparison.

### 3. Make the queue a projection of PR 13, not another job engine

ComfyUI separates queued, currently running, and history collections and orders
pending prompts with a heap. That is a useful UI projection, but the inspected
`PromptQueue` collections are process memory. Nexa must not adapt that storage
boundary because `D:\Nexa.txt` requires work to survive app restart.

InvokeAI provides the stronger persistence precedent. Its queue item has a
stable item ID, status, monotonically increasing `status_sequence`, priority,
batch, error details, and retry lineage. Its SQLite dequeue holds a claim lock
across select-and-transition so concurrent workers cannot claim the same row.
Enqueue is bounded by a configured maximum, and terminal history is pruned
separately.

Nexa's compact Generation Queue should expose:

| Queue field | Authority |
| --- | --- |
| workflow, shot, and variant identity | PR 15 workflow aggregate |
| pending order / local concurrency intent | PR 15 coordinator row or lease |
| state, current attempt, retry count, and provider task ID | PR 13 media job snapshot |
| estimated/final cost | PR 13 media job and provider event evidence |
| progress and latest error | PR 13 provider events |
| cancellation requested/confirmed | PR 13 cancellation fields and adapter result |
| output and lineage | PR 13 asset relations |

If PR 15 needs a scheduler table, it should contain only scheduling intent,
lease ownership, and references to `mediaJobId`; it must never copy provider
task IDs or claim that a provider state changed. A restart rebuilds visible
queue rows from durable workflow/variant/job associations and reconciles them
through the PR 13 recovery plan.

### 4. Retry creates a new attempt and preserves the failed one

InvokeAI retries only failed or canceled root items. It clones a new execution
session, inserts a new queue row, and records `retried_from_item_id`; it does not
rewrite the failed row back to pending. Nexa already has the more specific PR 13
attempt model and should adapt the history-preserving behavior at that seam.

An explicit same-provider retry should:

1. reload the media job and expected revision;
2. verify that the last attempt has a persisted retry classification and that
   `nextEligibleAt` has elapsed;
3. reject `provider_unknown` unless the provider lookup evidence required by PR
   13 has resolved the ambiguous submission;
4. create a new attempt with a new idempotency key while keeping the variant ID
   and normalized request snapshot stable; and
5. append the attempt to the same visible queue card rather than erasing the
   earlier failure.

Changing prompt, inputs, provider, model, resolution, duration, or another
request-identity field is not a retry. It creates a new variant and media job so
that compare, cost, lineage, and consent remain truthful.

Retry budgets are bounded by the job's `maxAttempts`. The UI must distinguish
`retry available`, `retry after <time>`, `not retryable`, and
`submission outcome unknown`; a generic Retry button must not bypass these
states.

### 5. Cancellation is targeted, idempotent, and two-phase

ComfyUI's job API distinguishes a queued job, which can be removed, from a
running job, which must be interrupted. Its current implementation checks and
signals the running prompt under the queue mutex to avoid interrupting the next
prompt after a race. InvokeAI likewise provides item-, batch-, destination-, and
queue-scoped cancellation and updates queued rows separately from in-progress
workers.

Nexa's first UI needs only targeted item cancellation. It should:

- revision-check the exact variant/media job;
- disable duplicate requests after `cancellationRequestedAt` is persisted;
- remove a not-yet-submitted local scheduling intent transactionally;
- otherwise delegate to PR 13 and the bound PR 14 adapter;
- render `cancelling` until provider observation or a conclusive adapter result;
  and
- keep `cancel unsupported`, `cancel failed`, and terminal-record deletion
  disclosures visible instead of projecting success.

Bulk cancel may be added later, but it should be defined as repeated targeted
cancellation with per-item results. A single optimistic "all canceled" result
would hide partial provider failures.

### 6. Cross-provider fallback is a disclosure and consent boundary

The inspected ComfyUI and InvokeAI graph/queue/compare sources do not model a
portable cloud-provider privacy manifest or authorization to forward the same
asset to another provider. They therefore provide no safe precedent for
cross-provider fallback. Nexa's acceptance text and the PR 13/14 provider
contracts remain authoritative here.

Before enqueue, every shot must show the exact effective facts from its trusted
connection plus selected capability manifest:

- provider, model, API version, endpoint/account scope, and data region;
- the ordered local assets that will be uploaded;
- retention and deletion availability/deadline;
- watermark and provenance behavior, including `unknown` rather than an
  optimistic default;
- estimated cost and currency; and
- whether fallback is disabled or which alternative provider would receive the
  same inputs.

Consent must be fail-closed. A useful durable authorization shape is:

```text
CrossProviderFallbackConsent
  workflowId, shotId
  sourceProviderId, allowedDestinationProviderId
  orderedInputAssetIds or request fingerprint
  grantedAt, revokedAt
```

A blanket application preference or a stale shot boolean is insufficient. The
authorization is invalidated when the input set, source/destination provider,
provider endpoint scope, or privacy facts change. The new attempt records
`crossProviderFallbackAuthorized=true` only after checking that exact consent.
The queue shows a confirmation step before the first upload to the fallback
provider. It never silently forwards an image or video after a failure.

Same-provider retry is not cross-provider fallback, but it still obeys provider
retention and retry eligibility. Switching connections within the same branded
provider is also a new disclosure if endpoint/account or data-region scope
changes.

### 7. Keep the first UI compact and task-oriented

The source evidence supports a four-surface layout without requiring a node
canvas:

1. **Brief** — title, intent, style, target duration, and aspect ratio.
2. **Shot Board** — ordered compact shot cards; prompt and references remain
   editable; provider/model/privacy are visible; each card shows variant count,
   selected variant, and aggregate state.
3. **Generation Queue** — pending/running/attention/finished sections with
   progress, cost, cancel, and reason-aware retry.
4. **Compare** — two chosen variants with synchronized play/pause, seek, mute,
   and fit; explicit Select commits the winner.

InvokeAI's compare modes are appropriate inspiration for still frames. For
video, slider/hover should be optional poster-frame aids; side-by-side video
with one shared clock is the dependable default. Selection remains available
without animation, and all state changes must work with reduced motion.

## Patterns adapted versus avoided

| Pattern | Decision for Nexa |
| --- | --- |
| ComfyUI dependency graph and topological execution | Adapt the typed DAG seam and deterministic projection; do not expose a free-form node editor in PR 15. |
| ComfyUI heap queue and queued/running/history views | Adapt the projection language and targeted cancellation; avoid process-memory queue authority. |
| InvokeAI validated graph versus execution state | Adapt the authoring/runtime separation and stable IDs. |
| InvokeAI SQLite queue, status sequence, bounded enqueue, and retry lineage | Adapt durable claims, revision-aware events, bounded concurrency, and history-preserving retries through PR 13 attempts. |
| InvokeAI gallery compare modes | Adapt compact comparison interactions; avoid image-only and persistence-denylisted selection state. |
| OpenTimelineIO ordered children | Adapt explicit shot/variant order; defer actual Timeline schemas and export to PR 16. |
| OpenTimelineIO multiple media references plus active key | Adapt independent variant records plus one explicit `selectedVariantId`; keep every non-selected variant and its lineage. |
| Automatic provider fallback | Avoid. None of the inspected OSS queue/compare contracts supplies the required disclosure/consent boundary. |
| Copying upstream source | Avoid. ComfyUI is GPL-3.0 and is used only as behavioral evidence; the Apache-2.0 projects are also used as design evidence, not copied implementations. |

## Contract and regression checklist

### Storage and graph

- Workflow and shot edits use expected revisions; stale edits fail with a
  reloadable conflict.
- Shot and variant ordinals are unique within their parent and reorder
  atomically.
- Deleting/reordering a shot never renumbers identities or corrupts variant
  lineage.
- A selected variant must belong to the same shot and have a verified completed
  output; deleting it clears or rejects selection explicitly.
- Editing a shot after enqueue does not mutate the submitted job snapshot.
- The structured aggregate deterministically projects to an acyclic typed graph
  with stable node IDs.

### Queue, retry, and cancellation

- Restart reconstructs pending/running/attention cards without resubmission.
- Concurrent workers cannot claim the same local queue intent.
- Queue concurrency and retained terminal history are bounded.
- A retry creates a new attempt and preserves earlier events, errors, cost, and
  provider task IDs.
- `provider_unknown` never presents an ordinary Retry action.
- Cancellation of a pending local item and a running provider item follow
  distinct paths and have per-item results.
- Cancellation remains `requested` until conclusive runtime/provider evidence.

### Compare and selection

- Compare candidates may be transient; the selected variant survives reload
  and is independent of which candidates are currently open.
- Two videos share one playback clock and do not create competing audio output.
- Compare works with keyboard controls and reduced motion.
- Selection uses expected shot revision and rejects a variant from another
  shot, another project, or an incomplete job.

### Privacy and fallback

- Provider/model/API scope, data region, retention/deletion, watermark,
  provenance, ordered inputs, and estimated cost are visible before enqueue.
- Unknown provider facts display as unknown and cannot inherit trusted defaults
  from another endpoint.
- Cross-provider fallback is disabled by default.
- Consent is scoped to exact input/provider facts, can be revoked, and is
  re-requested after a material change.
- No error path silently uploads the same asset to another provider.

## Licensing and implementation boundary

The source review establishes behavioral precedents only. No code, schema, UI
text, CSS, tests, or assets should be copied from ComfyUI. Its GPL-3.0 source is
particularly limited to architectural observation. InvokeAI and
OpenTimelineIO are Apache-2.0, but PR 15 should still implement original Rust,
SQLite, TypeScript, and React contracts that fit Nexa's existing PR 13/14
runtime and design system.
