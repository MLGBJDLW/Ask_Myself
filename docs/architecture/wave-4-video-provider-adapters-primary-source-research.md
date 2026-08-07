# Wave 4 video provider adapters primary-source research

Date: 2026-08-07

## Decision summary

PR 14 should implement two provider adapters behind the PR 13
`media_generation` runtime:

1. MiniMax, with separate contracts for the legacy Hailuo `/v1` API and the
   current H3 `/v2` API; and
2. Runway, version-pinned to `X-Runway-Version: 2024-11-06` and validated by
   the selected model's exact OpenAPI branch.

Two earlier watchlist conclusions in `D:\Nexa.txt` are now stale:

- `minimax/MiniMax-H3` is present in MiniMax's official, priced Video
  Generation V2 OpenAPI contract. It is no longer `announced /
  contract_pending`.
- `runway/seedance2_5` is present in Runway's official OpenAPI contract for
  text-to-video, image-to-video, and video-to-video. It is callable through
  Runway. This does **not** make a direct `bytedance/seedance-2.5` adapter
  verified; that separate provider/model tuple remains on the watchlist until
  ByteDance publishes and exposes a direct contract usable by Nexa.

The official pages do not label either current API model as preview or GA.
Within Nexa's required `releaseStatus` enum, `ga` is the least misleading
operational mapping because each model is documented, callable, and priced and
neither contract marks it preview. That mapping is an explicit Nexa inference,
not a quotation of provider release terminology. It must be re-audited when
the live contract snapshot changes.

Neither provider documents submission idempotency or a lookup by client
idempotency key. A transport failure after a generation POST therefore becomes
PR 13's `provider_unknown`; Nexa must not automatically submit the same attempt
again. Provider task identity must be committed before polling starts, and
successful output URLs must be downloaded into Nexa's verified local CAS
before a job becomes `completed`.

This is an original Nexa integration design. No upstream implementation source
was copied.

## PR boundary and existing runtime contract

This note covers the `video-provider-adapters` PR from `D:\Nexa.txt` and the
adapter seams that attach to the current PR 13 contracts:

- capability manifests and strict model/operation validation;
- provider-scoped authentication, submission, status, cancellation, output
  download, cost, and normalized errors;
- durable observation of provider task identity and provider events;
- upload locators that remain unverified external evidence until Nexa has
  downloaded and hashed output bytes; and
- release, privacy, retention, deletion, moderation, watermark, and provenance
  facts that can be shown without overclaiming.

It does not add Shot Board, Timeline, generic workflow authoring, a public
webhook relay, direct ByteDance access, Google Veo, or a provider-independent
fallback policy. Fallback remains a new, consented attempt; it is never silent.

The adapter must populate PR 13's existing attempt identity rather than create
another job system:

- `provider_id`, `provider_source`, `model_id`, and `api_version` identify the
  exact remote contract;
- `normalized_request_json` contains the provider-independent values after
  defaults are made explicit;
- `provider_request_extras_json` contains bounded, allowlisted provider-only
  fields;
- `provider_task_id` is stored as soon as a successful submit response is
  parsed;
- `provider_unknown` is used for ambiguous submission without a task ID;
- provider observations are appended with stable source-scoped dedupe keys;
  and
- final URLs are temporary locators, not `media_assets`, until bytes have been
  downloaded, typed, sized, hashed, and committed.

## Source snapshots

The MiniMax and Runway documentation links are live first-party contracts
observed on 2026-08-07. They are not immutable, so PR 14 should retain fixture
snapshots in tests and record the observed contract/API version in the manifest.

### MiniMax official sources

- [documentation index](https://platform.minimax.io/docs/llms.txt)
- [Video Generation V2 OpenAPI](https://platform.minimax.io/docs/api-reference/video/generation/api/v2-video-generation.json)
- [V2 create](https://platform.minimax.io/docs/api-reference/video-generation-v2-create.md),
  [query](https://platform.minimax.io/docs/api-reference/video-generation-v2-query.md),
  [list](https://platform.minimax.io/docs/api-reference/video-generation-v2-list.md),
  and [cancel/delete](https://platform.minimax.io/docs/api-reference/video-generation-v2-delete.md)
- legacy [text-to-video OpenAPI](https://platform.minimax.io/docs/api-reference/video/generation/api/text-to-video.json),
  [image-to-video OpenAPI](https://platform.minimax.io/docs/api-reference/video/generation/api/image-to-video.json),
  [first/last-frame OpenAPI](https://platform.minimax.io/docs/api-reference/video/generation/api/start-end-to-video.json),
  [query](https://platform.minimax.io/docs/api-reference/video-generation-query.md),
  and [download](https://platform.minimax.io/docs/api-reference/video-generation-download.md)
- [file upload](https://platform.minimax.io/docs/api-reference/file-management-upload.md)
  and [file deletion](https://platform.minimax.io/docs/api-reference/file-management-delete.md)
- [rate limits](https://platform.minimax.io/docs/guides/rate-limits.md),
  [pay-as-you-go pricing](https://platform.minimax.io/docs/guides/pricing-paygo.md),
  and [MiniMax API privacy policy](https://platform.minimax.io/protocol/privacy-policy)

### Runway official sources

- [machine-readable API context](https://docs.dev.runwayml.com/ai-context.md)
  and [live OpenAPI](https://docs.dev.runwayml.com/openapi.json)
- [API reference](https://docs.dev.runwayml.com/api.md),
  [SDK and polling guidance](https://docs.dev.runwayml.com/api-details/sdks.md),
  and [versioning policy](https://docs.dev.runwayml.com/api-details/versioning.md)
- [input contract](https://docs.dev.runwayml.com/assets/inputs.md),
  [ephemeral uploads](https://docs.dev.runwayml.com/assets/uploads.md), and
  [output contract](https://docs.dev.runwayml.com/assets/outputs.md)
- [HTTP errors](https://docs.dev.runwayml.com/errors/errors.md),
  [task failures](https://docs.dev.runwayml.com/errors/task-failures.md),
  [moderation](https://docs.dev.runwayml.com/api-details/moderation.md), and
  [usage tiers](https://docs.dev.runwayml.com/usage/tiers.md)
- [pricing](https://docs.dev.runwayml.com/guides/pricing.md),
  [API product assertions](https://runwayml.com/api),
  [data security](https://runwayml.com/data-security), and
  [terms of use](https://runwayml.com/terms-of-use)

### Pinned OSS implementation evidence

All GitHub links are pinned to an exact 40-character commit. They are design
evidence only, not substitute API contracts.

| Source | Pinned revision | License | Evidence and boundary |
| --- | --- | --- | --- |
| Runway's official Node SDK [task resource](https://github.com/runwayml/sdk-node/blob/94b6498783283df01ea15bf7a96dcfcba56fe0d8/src/resources/tasks.ts), [polling helper](https://github.com/runwayml/sdk-node/blob/94b6498783283df01ea15bf7a96dcfcba56fe0d8/src/lib/polling.ts), [upload helper](https://github.com/runwayml/sdk-node/blob/94b6498783283df01ea15bf7a96dcfcba56fe0d8/src/resources/uploads.ts), and [client retry logic](https://github.com/runwayml/sdk-node/blob/94b6498783283df01ea15bf7a96dcfcba56fe0d8/src/client.ts) | `94b6498783283df01ea15bf7a96dcfcba56fe0d8` | [Apache-2.0](https://github.com/runwayml/sdk-node/blob/94b6498783283df01ea15bf7a96dcfcba56fe0d8/LICENSE) | Confirms the task union, 6-second jittered polling, terminal handling, two-stage upload, default generic retries, required version header, and that the generic `idempotencyKey` option emits no header because this client defines no `idempotencyHeader`. Nexa must not copy generic POST retries across an ambiguous side-effect boundary. |
| ComfyUI's MiniMax partner [H3 and legacy video nodes](https://github.com/Comfy-Org/ComfyUI/blob/0ab8332bfa41c695b1c104a6535ff1fde81c7939/comfy_api_nodes/nodes_minimax.py) | `0ab8332bfa41c695b1c104a6535ff1fde81c7939` | [GPL-3.0](https://github.com/Comfy-Org/ComfyUI/blob/0ab8332bfa41c695b1c104a6535ff1fde81c7939/LICENSE) | Demonstrates a real adapter that separates submit, poll, and download and validates multimodal counts/durations. It also proves why live first-party OpenAPI must win: this proxy sends undocumented H3 `seed` and `aigc_watermark` fields and enforces a 5-second minimum while MiniMax's live OpenAPI documents 4–15 seconds and neither extra field. No GPL code or undocumented extension should be copied. |

## Provider source and credential identity

Provider identity is not just `minimax` or `runway`. Each attempt should bind an
immutable source key composed from:

```text
provider_id + normalized_base_url + api_contract_version + credential/account fingerprint
```

The fingerprint is a local opaque identifier, never the secret. It prevents a
task ID created under one account or endpoint from being queried with another.
Both secrets stay in the Rust/Tauri process; they must not be sent to the
renderer or logged. The official Runway SDK likewise disables browser use to
protect its secret.

Neither live OpenAPI exposes a request-selectable region or regional base URL:

| Provider | Base URL | Auth | Region conclusion |
| --- | --- | --- | --- |
| MiniMax | `https://api.minimax.io` | `Authorization: Bearer <API key>` | No region parameter or alternate server in the video specs. The current privacy policy says personal data is stored in a US data center, but that is not a per-request routing guarantee for every media byte. Show provider-managed routing and no selectable region. |
| Runway | `https://api.dev.runwayml.com` | `Authorization: Bearer <secret>` plus `X-Runway-Version: 2024-11-06` | No region parameter or alternate server in the OpenAPI. Enterprise statements about third-party processing must not be shown as guarantees for an ordinary developer account. Show region as unknown/provider-managed. |

An edited or non-official endpoint must become a distinct untrusted
`provider_source`; it must not inherit credentials or the official manifest's
privacy assertions by provider name alone.

## MiniMax adapter contract

### Contract families and model availability

MiniMax currently exposes two materially different video contracts on the same
origin. They should share authentication/HTTP plumbing but not a request or
state parser.

| Contract | Endpoint and official model IDs | Operations | Nexa release mapping |
| --- | --- | --- | --- |
| H3 V2 | `POST /v2/video_generation`; model enum is exactly `MiniMax-H3` | text-to-video, keyframe image-to-video, multimodal reference-to-video | `ga` by the documented/priced inference above; observed API version `v2` |
| Legacy Hailuo text-to-video | `POST /v1/video_generation`; `MiniMax-Hailuo-2.3`, `MiniMax-Hailuo-02`, `T2V-01-Director`, `T2V-01` | text-to-video | `ga`; legacy contract family `v1` |
| Legacy Hailuo image-to-video | same path; `MiniMax-Hailuo-2.3`, `MiniMax-Hailuo-2.3-Fast`, `MiniMax-Hailuo-02`, `I2V-01-Director`, `I2V-01-live`, `I2V-01` | first-frame image-to-video | `ga`; legacy contract family `v1` |
| Legacy first/last frame | same path; model is exactly `MiniMax-Hailuo-02` | first/last-frame image-to-video | `ga`; legacy contract family `v1` |

Do not merge Runway's separate `hailuo3` model ID into the direct MiniMax model
namespace. The provider and billing/retention boundary is different even when
the underlying model family is related.

### H3 V2 create validation

The `VideoGenerationV2Req` schema requires `model`, `content`, `resolution`,
and `duration`:

- `model` is exactly `MiniMax-H3`;
- every request includes one non-empty `content` item with `type=text`; each
  text value is at most 7,000 characters;
- `resolution` is `768P` or `2K`;
- `duration` is an integer from 4 through 15 inclusive; and
- `ratio` is one of `adaptive`, `21:9`, `16:9`, `4:3`, `1:1`, `3:4`, or
  `9:16`.

The content modes are mutually exclusive:

- text-to-video contains text only, requires a concrete ratio, and rejects
  `adaptive`;
- keyframe image-to-video uses at most one `first_frame` and at most one
  `last_frame`; a lone unlabelled image defaults to first frame, and the ratio
  is forced to `adaptive` even if another valid value is supplied; and
- reference-to-video uses `reference_image`, `reference_video`, and/or
  `reference_audio`; it cannot be mixed with first/last frames, and ratio is
  optional with `adaptive` as the provider default.

Input locations may be public URLs, `mm_file://{file_id}`, or data URIs. The
total JSON request body is at most 64 MB, so base64 must not be the default.
H3 constraints are:

| Input | Format and size | Dimensions/duration/count |
| --- | --- | --- |
| Image | JPG/JPEG/PNG/WebP/HEIC/HEIF, at most 30 MB each | width and height 256–5760 px, aspect 0.4–2.5; first <=1, last <=1, references <=9 |
| Reference video | MP4/MOV; H.264/H.265 video and AAC/MP3 audio; at most 50 MB each | 2–15 s each, at most 3 and 15 s total; 256–5760 px, aspect 0.4–2.5, 23.976–60 fps |
| Reference audio | WAV/MP3, at most 15 MB each | 2–15 s each, at most 3 and 15 s total |

The upload API's `video_generation_input` purpose accepts the same per-file
sizes, returns a file usable as `mm_file://{file_id}`, and documents seven-day
validity. The upload must be a separate resumable local step; an expired file
requires a new upload before submit.

The H3 OpenAPI does **not** document `seed`, negative prompt, an audio-output
toggle, `aigc_watermark`, or other watermark/provenance controls. Those
manifest flags stay false/unknown. The ComfyUI proxy fields are not evidence of
a public direct MiniMax contract.

### Legacy Hailuo validation

The `/v1` branches use distinct schemas even though they share a path:

- text-to-video requires `model` and `prompt`; prompt is at most 2,000
  characters;
- image-to-video requires `model` and `first_frame_image`; prompt is optional;
- first/last-frame requires `MiniMax-Hailuo-02` and `last_frame_image`; the
  official operation permits optional first frame and prompt; and
- images are public URLs or data URIs, JPG/JPEG/PNG/WebP, under 20 MB, with
  short edge over 300 px and aspect 0.4–2.5.

The adapter must encode the documented model-specific duration/resolution
matrix instead of exposing the union of every schema value:

| Model family | Valid combinations relevant to the first adapter |
| --- | --- |
| `MiniMax-Hailuo-2.3`, `MiniMax-Hailuo-2.3-Fast` where the operation supports it | 768P at 6 or 10 s; 1080P at 6 s |
| `MiniMax-Hailuo-02` | 768P at 6 or 10 s; 1080P at 6 s; image-to-video also documents 512P at 6 or 10 s |
| first/last-frame `MiniMax-Hailuo-02` | 768P at 6 or 10 s; 1080P at 6 s |

Older T2V/I2V model IDs in the official enums should be represented only if
their own parameter matrix is captured. Do not project the Hailuo 2.3 matrix
onto them. The legacy schema documents no seed, cancel endpoint, negative
prompt, or provenance response.

### Submit, status, callback, cancel, and download

H3 V2 create returns only `{ "task_id": "..." }`. Query is
`GET /v2/query/video_generation/{task_id}` and list is
`GET /v2/query/video_generation`. The task union contains:

```text
queued -> running -> succeeded
                   -> failed
queued -> cancelled
```

It also carries `created_at`, `updated_at`, `error.code/message`, output
`content.url`, resolution, duration, ratio, task type, modality, and billed
usage. Query and list cover only tasks from the last seven days. After that,
`invalid task_id` does not prove local failure; Nexa should retain its own
history and classify an unresolved nonterminal task as expired/attention-needed
according to the local retention policy.

`DELETE /v2/video_generation/{task_id}` is status-dependent:

- `queued` cancels without charge;
- `succeeded` or `failed` deletes the task record;
- `running` cannot be cancelled; and
- `cancelled` cannot be deleted through this operation.

Deleting a task record is not documented as erasing the output bytes. Nexa may
record provider acknowledgement of record deletion, but must not mark remote
asset erasure verified.

The legacy query is `GET /v1/query/video_generation?task_id=...` with states
`Preparing`, `Queueing`, `Processing`, `Success`, and `Fail`. Success returns a
`file_id`; `GET /v1/files/retrieve?file_id=...` returns a download URL valid for
one hour. The legacy API has no documented cancellation endpoint.

Both families accept `callback_url`. MiniMax first sends a `challenge` and
requires the same value within three seconds; subsequent bodies mirror query
state. The public docs do not specify a callback signature, shared secret, or
event ID. The desktop adapter should therefore advertise polling as its
supported end-to-end observation mode. A future authenticated HTTPS relay can
add webhook support as a separate trust boundary; merely accepting a callback
URL is not enough.

Polling observations without provider event IDs should deduplicate H3 events
by provider source, task ID, normalized status, and `updated_at`. Legacy polling
lacks an update timestamp, so only state changes and terminal evidence should
be appended. Signed output URL query parameters must be redacted from stored
event payloads.

### Errors, retry, rate limits, and cost

H3 uses real HTTP error statuses and an OpenAI-style error body with
`error.type`, `error.message`, `error.http_code`, and `request_id`. Documented
types include authentication 401, bad request 400, insufficient balance 402,
unprocessable/safety 422, rate limit 429, overload 529, and server 500. Retain
`request_id` and bounded redacted error detail.

Legacy endpoints can return HTTP 200 with a non-zero
`base_resp.status_code`. The adapter must always inspect it. Important codes
include 1002 rate limit, 1004 authentication, 1026 sensitive/invalid input,
1027 sensitive output, and 1039 token/rate limit. Safety errors are permanent
for the same request; rate/overload errors are retryable only before a remote
task might have been created.

The current account limits are:

- legacy Hailuo video generation: 5 RPM free and 20 RPM paid; and
- H3 V2: maximum concurrent tasks 2 free and 15 paid.

These are a dated capability snapshot, not constants to bake into scheduling.
The adapter should honor a manifest/account override and use bounded backoff
with jitter.

Current H3 pay-as-you-go estimation is deterministic when all input durations
and counts are known:

- output: USD 0.08/s at 768P or USD 0.13/s at 2K;
- audio references: free;
- images: first five free, then USD 0.04 each; and
- reference video: input seconds billed at the selected output resolution's
  per-second rate.

Store the pricing observation date and an itemized estimate. H3 query returns
usage but no final USD total, so do not claim an actual charge from the local
formula. Legacy pricing is a fixed table by model/resolution/duration in the
official pricing page and should likewise be a dated manifest snapshot.

### MiniMax privacy, retention, moderation, and provenance

The H3 query window is seven days, uploaded H3 input files are valid for seven
days, and legacy download URLs are valid for one hour. None of those facts is a
general input/output deletion SLA. The privacy policy gives purpose-based
personal-data retention and describes US storage, but does not give a
video-task-specific erasure deadline. Nexa must show provider retention as
unknown beyond the documented locator/query windows.

Legacy 1026/1027 and H3 422/task errors expose moderation outcomes. The public
contract does not expose a moderation policy selector. The adapter should map
input/output safety failures separately, never retry them unchanged, and avoid
showing raw provider text without Nexa context.

No current direct H3 or legacy OpenAPI response provides a watermark or C2PA /
content-credential assertion. Record provider/model/task lineage in Nexa, but
set provider watermark and provenance facts to unknown unless the response
actually supplies evidence.

## Runway adapter contract

### Version, endpoints, and model availability

Every request uses:

```text
base URL: https://api.dev.runwayml.com
Authorization: Bearer <secret>
X-Runway-Version: 2024-11-06
```

The `/v1` URL segment is not the API version. Runway's version policy says old
header versions are supported for four months after a new version is created,
so Nexa must persist the header version per attempt and treat a future header
upgrade as a manifest/fixture change.

The live video operation matrix is:

| Operation | Endpoint | Exact current model IDs |
| --- | --- | --- |
| Text-to-video | `POST /v1/text_to_video` | `gen4.5`, `veo3.1`, `veo3.1_fast`, `hailuo3`, `happyhorse_1_0`, `seedance2`, `seedance2_fast`, `seedance2_mini`, `gemini_omni_flash`, `seedance2_5` |
| Image-to-video | `POST /v1/image_to_video` | `gen4.5`, `gen4_turbo`, `veo3.1`, `veo3.1_fast`, `hailuo3`, `happyhorse_1_0`, `seedance2`, `seedance2_fast`, `seedance2_mini`, `gemini_omni_flash`, `seedance2_5` |
| Video-to-video | `POST /v1/video_to_video` | `aleph2`, `hailuo3`, `seedance2`, `seedance2_fast`, `seedance2_mini`, `gemini_omni_flash`, `seedance2_5` |

Each request is a discriminated union keyed by `model` and rejects additional
properties. A provider-level union of fields is unsafe: validate against the
specific `(operation, model, API header version)` branch. PR 14 may initially
ship a tested subset, but it must not advertise the remaining IDs until their
individual branches and costs are captured.

### Runway `seedance2_5` exact contract

`seedance2_5` is the literal Runway model ID, with an underscore. Its three
branches share:

- optional `promptText`, 1–15,000 UTF-16 code units when present;
- `audio` boolean, provider default `true`;
- integer `duration` 4–30 seconds;
- no seed, negative prompt, moderation override, webhook, or provenance field;
  and
- exactly twelve ratios:
  `992:432`, `864:496`, `752:560`, `640:640`, `560:752`, `496:864`,
  `1470:630`, `1280:720`, `1112:834`, `960:960`, `834:1112`, and
  `720:1280`.

The first six ratios are the 480p tier and the last six are the 720p tier. The
input guide notes that `864:496` and `496:864` produce standard 854x480 and
480x854 pixels despite their parameter spelling. `1280:720` is the documented
default ratio. Nexa should make ratio, duration, and audio explicit in its
normalized request so output and cost do not drift with server defaults.

Operation-specific rules are:

| Operation | Required by OpenAPI | References and conditional rules |
| --- | --- | --- |
| Text-to-video | `model` | up to 30 image references, up to 10 video references, and up to 10 audio references; combined reference video duration <=30 s and combined audio duration <=30 s |
| Image-to-video | `promptImage`, `model` | prompt image can be one URI or an array. `position=first/last` is keyframe mode; omitted position is reference-image mode; do not mix the modes. Reference audio count <=10 and total duration <=30 s. |
| Video-to-video | `promptVideo`, `model` | `mode=reference` by default and permits duration/ratio; `mode=extend` requires `promptText`, follows the input aspect ratio, and forbids `ratio`. Additional images <=30, videos <=9, audio <=10; all input/reference video duration combined <=30 s and reference audio total <=30 s. |

The text-to-video schema genuinely requires only `model`; prompt and references
are optional at schema level. Nexa may choose a stricter product rule such as
requiring at least one conditioning input, but that must be labelled as a Nexa
policy rather than attributed to Runway.

Input/reference videos for Seedance 2.5 must be at least 480p. Reference image
aspect ratio is 0.4–4. The adapter must preflight durations, dimensions, and
media types from verified local asset metadata before upload.

### Input URLs and ephemeral uploads

Runway accepts HTTPS URLs, base64 data URIs, or `runway://` ephemeral upload
URIs. Generic limits are:

| Input | HTTPS URL bytes | Encoded data URI | Ephemeral upload |
| --- | ---: | ---: | ---: |
| Image | 16 MB | 5 MB | 200 MB |
| Video | 32 MB | 16 MB | 200 MB |
| Audio | 32 MB | 16 MB | 200 MB |

HTTPS inputs must use a domain rather than an IP address, support `HEAD`, return
valid matching `Content-Type` and `Content-Length`, avoid redirects, and be no
longer than 2,048 characters. Generic `application/octet-stream` is rejected.
Runway fetches with a user agent beginning `RunwayML API/`.

`POST /v1/uploads` with `filename` and `type: "ephemeral"` returns an upload
URL, multipart fields, and `runwayUri`. The client then performs a second
multipart POST containing those fields and the file. Files must be 512 bytes to
200 MB, require purchased credits, and expire after 24 hours. If the storage
POST fails, official guidance says not to replay it; create a new upload
placeholder. The local job must retain which asset produced each external
locator, but the `runway://` token itself should be redacted from durable UI
events.

### Submit, state, cancellation, and output download

Every generation POST returns a UUID `id` and `estimatedCost.credits`. Store the
task UUID before scheduling the first status check. Query is
`GET /v1/tasks/{id}`; Runway says consumers should not expect updates more often
than every five seconds.

The normalized mapping is:

| Runway state | Nexa state / action |
| --- | --- |
| `PENDING` | `queued` |
| `THROTTLED` | `queued`, with a provider-throttled reason; it is stored server-side and is not failure |
| `RUNNING` | `running`, preserving progress 0–1 as observation metadata |
| `SUCCEEDED` | `post_processing` until every desired output is downloaded and verified |
| `FAILED` | `failed`, preserving bounded `failureCode`, contextualized failure text, and final `cost.credits` |
| `CANCELLED` | `cancelled`, preserving final `cost.credits` when observed |

Runway's official SDK polls at six seconds with jitter and treats
`THROTTLED/PENDING/RUNNING` as nonterminal. Nexa can adopt the principle with
its own persisted scheduler; it should not hold an in-memory ten-minute waiter
as runtime authority.

`DELETE /v1/tasks/{id}` cancels `RUNNING`, `PENDING`, or `THROTTLED` tasks and
deletes finished tasks. It returns 204. A repeated delete or cancel may return
404, which the official contract says is safe to ignore for idempotency.
Persist cancellation/deletion intent before the call; only then can 204 or the
documented repeat-404 confirm that intent. An unrelated 404 does not prove
cancellation.

Cancelled and deleted tasks cannot subsequently be fetched. Deleting a
finished task says its output is removed from persistent storage according to
Runway's retention policy, but no completion time or backup-erasure SLA is
specified. Record an acknowledged remote deletion, not cryptographically
verified erasure.

Succeeded tasks return an output URL array. URLs expire in 24–48 hours, and a
fresh task GET returns refreshed URLs while the task remains available. Do not
expose these signed URLs as durable product assets. Download them immediately,
verify content type/length/hash, commit local CAS assets and output relations,
then mark the job completed.

### Idempotency, errors, and retry safety

The Runway OpenAPI exposes no webhook, client idempotency header, client request
ID, list-by-request endpoint, or submit reconciliation endpoint. The official
SDK contains a generic `idempotencyKey` option inherited from its generator,
but its Runway client never defines `idempotencyHeader`, so that option sends no
idempotency header. This is a useful source-level negative check, not permission
to invent a header.

The same SDK defaults to two retries for connection errors, 408, 409, 429, and
5xx responses. Nexa must narrow that policy:

- GET/status and a known repeat DELETE are safe to retry with backoff/jitter;
- a conclusive validation/auth failure is permanent;
- a conclusive 429 task-creation rejection can be retried as a new dispatch of
  the same still-unsubmitted attempt after its delay;
- an upload-storage failure starts a new upload placeholder as documented; and
- connection loss, timeout, 502, or 503 after a generation POST may have
  created a task. Without a returned task ID or provider idempotency lookup,
  persist `provider_unknown` and require reconciliation/user action rather
  than allowing the SDK or runtime to replay the POST.

Official HTTP retry guidance is 400/401/404 no, 429/502/503 yes, with
exponential backoff and jitter. The last rule above is intentionally more
conservative at Nexa's side-effect boundary because the public contract cannot
deduplicate an ambiguous accepted POST.

Task failures need a separate classifier:

- `SAFETY.INPUT.*`, `SAFETY.OUTPUT.*`, and
  `INPUT_PREPROCESSING.SAFETY.TEXT` are safety outcomes and should not be
  retried unchanged;
- `ASSET.INVALID` is a permanent local-validation miss;
- `INPUT_PREPROCESSING.INTERNAL` and `INTERNAL`/null may retry after delay as a
  new explicit attempt;
- `THIRD_PARTY.UNAVAILABLE` may retry only after delay as a new explicit
  attempt; and
- `INTERNAL.BAD_OUTPUT.*` may succeed after correcting the prompt/input, so it
  is not a blind identical retry.

### Rate limits and cost

Runway has no maximum requests-per-minute limit within the account's rolling
daily-generation quota. Limits are per organization; concurrency is shared by
all video models in the modality and depends on usage tier. Excess concurrent
tasks enter `THROTTLED` and remain queued in approximate submission order.
Exceeding the rolling 24-hour generation count yields 429 at task creation.
Ephemeral uploads have a separately documented rate limit without a public
fixed number.

The authoritative estimate is the submit response's
`estimatedCost.credits`; terminal tasks return final `cost.credits`. Record both
as distinct provider events. Current public pricing says one credit costs USD
0.01, but that conversion is a dated display snapshot, not a permanent billing
constant.

Current `seedance2_5` pricing is:

- 720p: 30 credits per output second plus 15 per input/reference video second;
- 480p: 20 credits per output second plus 10 per input/reference video second;
- combined billed input/reference video is capped at 30 seconds;
- reference images and audio are free; and
- minimum charge is 80 credits per generation.

Local `estimateCost` should itemize that formula and then replace/display the
provider-returned estimate when available. Final task cost wins over both.

### Runway moderation, retention, privacy, and provenance

Runway moderates every request element and reports moderation as a failed task,
not necessarily an HTTP error. The moderation page says moderated generations
cost the same as successful generations and repeated moderated requests can
suspend the account. The task-failure page specifically says
`SAFETY.INPUT.*` is not refunded. `seedance2_5` does not include the otherwise
documented `contentModeration.publicFigureThreshold` field in its discriminated
schema, so the adapter must reject that extra field for this model.

The official API product page says API customer data is not used for training
and customers own outputs. Those product assertions do not specify a
task-by-task retention interval or regional routing. The data-security page
describes encryption and request-based removal, while the task delete contract
is the only adapter-callable deletion surface. Nexa should show the exact
callable fact and leave unquantified retention/region fields unknown.

No current video task response includes a watermark, C2PA/content credential,
or provider provenance object. Store Runway, the model ID, task UUID, API
version, normalized request digest, and local asset lineage; do not assert
watermark-free output. The current terms also require applicable API
interfaces to display “Powered by Runway” with a link, so product release needs
a branding/terms check independent of the transport adapter.

## Cross-provider adapter constraints

| Concern | MiniMax | Runway | Required Nexa behavior |
| --- | --- | --- | --- |
| Submit idempotency | None documented | None documented; generic SDK option does not emit a header | Ambiguous POST becomes `provider_unknown`; no blind replay |
| Observation | H3/legacy polling; callbacks lack documented signature/event ID | Polling only; no webhook contract | Poll with jitter from durable attempt identity; do not advertise desktop webhook support |
| Cancellation | H3 queued only; legacy none | Active tasks cancellable; finished tasks deleted | Manifest is model/contract-specific; cancellation requested is distinct from confirmed |
| Output lifetime | H3 URL time-limited; legacy URL one hour | URL 24–48 hours | Download promptly, verify bytes, commit CAS before completion |
| Remote deletion | H3 deletes succeeded/failed record, not proven output erasure; legacy file delete is a separate v1 surface | finished-task delete acknowledges persistent-output deletion under policy | Track acknowledged versus verified deletion separately |
| Region | no request selector | no request selector | Show provider-managed/unknown; no silent region claim |
| Watermark/provenance | no official response fact | no official response fact | Preserve Nexa lineage; provider flags remain unknown |
| Fallback | no implicit cross-provider identity | no implicit cross-provider identity | New explicit attempt with visible provider/cost/privacy change |

Each adapter should expose strict, pure `capabilities`, `validate`, and
`estimateCost` functions before any network operation. `submit`, `status`,
`cancel`, and `download` use an injected HTTP client, bounded timeouts, response
size limits, redacted tracing, and model-specific parsers. Raw provider JSON is
never a substitute for the normalized state machine.

Because both published DELETE contracts can delete a terminal task record, the
manifest exposes `cancellationMayDeleteTerminalRecord` separately from
`supportsCancellation`. Nexa persists the user's cancellation intent before
the call, preflights the latest state, and still treats the provider response
as an observation rather than a local proof that no completion race occurred.

## Release-status and watchlist disposition

| Provider/model tuple | Disposition on 2026-08-07 | Reason |
| --- | --- | --- |
| `minimax/MiniMax-H3` on `/v2/video_generation` | enable; Nexa `ga` inference | Exact official model enum, validation schema, status/query/delete contract, rate limit, and pricing exist |
| `minimax/MiniMax-Hailuo-2.3` and operation-valid `MiniMax-Hailuo-2.3-Fast` / `MiniMax-Hailuo-02` | enable | Exact official legacy schemas and pricing exist |
| `runway/seedance2_5` | enable only for the three exact Runway operations and schemas | Exact official discriminated branches and pricing exist |
| `runway/hailuo3` | may be enabled after its Runway-specific branches receive the same fixture coverage | It is official in Runway OpenAPI, but is not interchangeable with direct MiniMax H3 |
| `bytedance/seedance-2.5` direct | `contract_pending` / watchlist | Runway's aggregator contract supplies no ByteDance endpoint, auth, region, task, cancellation, or retention contract for a direct adapter |
| MiniMax H3 `seed`, `aigc_watermark`, explicit audio-output toggle, negative prompt | `unverified` | Absent from live official H3 OpenAPI despite OSS proxy extensions |
| Provider webhook ingestion in Nexa desktop | `contract_pending` | MiniMax lacks documented callback authentication; Runway exposes no webhook |

Availability should be refreshed by an auditable catalog update, never by
quietly accepting an unknown model string. Unknown provider/model/operation
tuples fail closed with an actionable “manifest refresh required” error.

## PR 14 test requirements

### Contract and manifest fixtures

1. Snapshot the observed MiniMax H3 V2 schemas and Runway
   `2024-11-06` branches used by the adapter. A fixture update must show an
   intentional manifest diff.
2. Assert every advertised model exists for the advertised operation and no
   tuple inherits capabilities from a similarly named provider/model.
3. Assert H3 and Runway `seedance2_5` are enabled while direct ByteDance 2.5
   remains unavailable.
4. Assert provider source includes normalized official base URL, API version,
   and opaque credential/account identity.

### MiniMax validation

1. Accept H3 duration 4 and 15; reject 3 and 16. This catches the stale OSS
   5-second minimum.
2. Require H3 prompt text; reject T2V `adaptive`; force/normalize keyframe I2V
   to `adaptive`; reject keyframe/reference mixing.
3. Boundary-test every media count, file size, dimension/aspect, frame-rate,
   clip duration, total duration, and 64 MB request-body rule.
4. Reject undocumented H3 `seed`, `aigc_watermark`, negative prompt, and audio
   toggle.
5. Table-test every enabled legacy model/operation/duration/resolution tuple;
   reject union leakage such as 1080P/10 s.
6. Parse H3 real HTTP errors and legacy HTTP-200/non-zero `base_resp` errors
   into the same normalized error taxonomy without losing request ID/code.

### Runway and Seedance validation

1. Assert the required auth and exact `X-Runway-Version` header and reject
   renderer-side credential access.
2. Verify `seedance2_5` uses the underscore ID on all three endpoints and test
   its exact required fields, 15,000-unit prompt boundary, 4/30-second duration
   boundaries, twelve ratios, counts, combined-duration limits, and 480p input
   minimum.
3. Reject model-union leakage: `seed`, negative prompt, moderation override,
   webhook, or a field allowed by another Runway model.
4. Test image keyframe/reference exclusivity and video `extend` requiring
   prompt while forbidding ratio.
5. Test HTTPS URL `HEAD`, media type/length, redirect, IP-host, generic content
   type, and size failures before generation submit.
6. Test the two-stage upload, 512-byte/200-MB boundaries, 24-hour expiry, and
   “new placeholder rather than replay failed storage POST” rule.

### Durable submission and recovery

1. A successful submit persists provider task ID and attempt source before the
   first poll.
2. Connection loss after a generation POST with no task ID transitions to
   `provider_unknown` and restart does not resubmit it.
3. GET/status retries use bounded exponential backoff and jitter; Runway polls
   no faster than the documented five-second observation cadence.
4. Runway `THROTTLED` remains queued, not failed. H3 `queued/running` and legacy
   `Preparing/Queueing/Processing` map without inventing progress.
5. Stale/out-of-order observations cannot regress a terminal state and repeated
   polls deduplicate without storing signed URLs.

### Cancellation, deletion, and output assets

1. H3 queued cancel confirms `cancelled`; running cancel refusal leaves the
   job running with visible cancellation failure; legacy manifests do not offer
   cancel.
2. Runway DELETE 204 confirms the persisted intent; a repeat 404 is accepted
   only for that same intent/source/task tuple.
3. Provider success enters `post_processing`; URL expiry/refresh and interrupted
   download resume; only verified bytes create a CAS asset and permit
   `completed`.
4. Deleting local output, requesting remote deletion, provider acknowledgement,
   and metadata retention remain four independently testable facts.

### Cost, safety, privacy, and provenance

1. Golden-test H3 output/reference formulas and Seedance 2.5 480p/720p,
   minimum-credit, and input-video formulas at all boundaries.
2. Preserve submit estimate separately from Runway terminal actual cost and
   MiniMax billed usage; never overwrite history when pricing changes.
3. Safety failures are permanent for the identical request and expose a
   contextualized message without leaking raw private input or signed URL data.
4. Manifests/UI show no selectable region, no verified remote-erasure claim,
   and unknown watermark/provenance where the response supplies no evidence.
5. A fallback confirmation names the new provider, model, estimate, region
   status, retention uncertainty, and moderation boundary before creating a
   new attempt.

## Unresolved facts that must stay explicit

The reviewed primary sources do not establish:

- provider-side deduplication or reconciliation of an ambiguous submit;
- task-specific input/output retention duration beyond the documented locator,
  upload, and query windows;
- a user-selectable processing region;
- authenticated MiniMax webhook signatures or any Runway webhook;
- complete provider-side erasure including backups;
- a response-backed watermark/C2PA/content-credential fact; or
- a direct ByteDance Seedance 2.5 endpoint usable by Nexa.

Those fields remain unknown or `contract_pending`. They must not be filled from
marketing inference, model-name similarity, third-party SDK convenience, or an
OSS proxy extension.
