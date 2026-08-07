# Wave 4 media job runtime and asset lineage primary-source research

Date: 2026-08-07

## Decision summary

PR 13 should add a provider-neutral, durable media-generation runtime under a
new `media_generation` bounded context. It should not add a video provider,
extend the current media-analysis settings, or expose the later Shot Board and
Timeline surfaces.

The durable authority is a SQLite job row plus attempt and provider-event
history. Every state transition is transactional and revision-checked. On app
restart, Nexa scans recoverable jobs and resumes observation from the persisted
provider task identity. A job whose submission outcome is ambiguous becomes
`provider_unknown`; Nexa must not blindly submit it again.

Assets become content-addressed only after Nexa has read and verified their
bytes. Lineage is stored independently of the deduplicated blob so that every
input, output, retry, variant, edit, and export occurrence remains attributable
to the attempt that produced it. Cancellation intent, provider confirmation,
local binary deletion, remote deletion, and metadata retention are distinct
facts.

This is an original Nexa design. No upstream implementation source was copied.

## PR boundary from `D:\Nexa.txt`

This note covers the `media-job-runtime-and-asset-lineage` PR only:

- the formal media job lifecycle;
- durable restart recovery and idempotent submission boundaries;
- per-attempt history and provider event ingestion;
- content-addressed assets and asset relations;
- cancellation, expiration, retention, and deletion bookkeeping; and
- the storage/runtime seams required by later provider adapters.

It intentionally leaves these items to later PRs:

- capability manifests and MiniMax, Runway, Veo, or Seedance adapters;
- provider-specific polling, webhook authentication, upload, and cancellation;
- Shot Board, queue, compare, DAG authoring, Timeline, and FFmpeg export UI; and
- any change that treats the existing video-ingestion settings as a generation
  platform.

## Source snapshots

All GitHub source links below are pinned to the 40-character commit observed on
2026-08-07. Moving branch links are intentionally not used as evidence.

| Source | Pinned revision | License / terms | Evidence used |
| --- | --- | --- | --- |
| [Temporal History service architecture](https://github.com/temporalio/temporal/blob/dfe9eb837a4f9604fe20b1a2d2edc963c6059b73/docs/architecture/history-service.md) and [Workflow lifecycle](https://github.com/temporalio/temporal/blob/dfe9eb837a4f9604fe20b1a2d2edc963c6059b73/docs/architecture/workflow-lifecycle.md) | `dfe9eb837a4f9604fe20b1a2d2edc963c6059b73` | [MIT](https://github.com/temporalio/temporal/blob/dfe9eb837a4f9604fe20b1a2d2edc963c6059b73/LICENSE) | Durable history, atomic state/task updates, recovery from persistence, and eventual task delivery. |
| [Temporal Workflow Service request contract](https://github.com/temporalio/api/blob/3ebdff42a9f07ac484b415fe8ff0b483b4ce3340/temporal/api/workflowservice/v1/request_response.proto) and [event types](https://github.com/temporalio/api/blob/3ebdff42a9f07ac484b415fe8ff0b483b4ce3340/temporal/api/enums/v1/event_type.proto) | `3ebdff42a9f07ac484b415fe8ff0b483b4ce3340` | [MIT](https://github.com/temporalio/api/blob/3ebdff42a9f07ac484b415fe8ff0b483b4ce3340/LICENSE) | Stable start request IDs, retry/timeout bounds, and the distinction between cancellation requested and cancellation confirmed. |
| [OpenLineage core schema](https://github.com/OpenLineage/OpenLineage/blob/9c9144caef115053a23df55a939760ba2a4e5922/spec/OpenLineage.json), [dataset-version facet](https://github.com/OpenLineage/OpenLineage/blob/9c9144caef115053a23df55a939760ba2a4e5922/spec/facets/DatasetVersionDatasetFacet.json), and [lifecycle facet](https://github.com/OpenLineage/OpenLineage/blob/9c9144caef115053a23df55a939760ba2a4e5922/spec/facets/LifecycleStateChangeDatasetFacet.json) | `9c9144caef115053a23df55a939760ba2a4e5922` | [Apache-2.0](https://github.com/OpenLineage/OpenLineage/blob/9c9144caef115053a23df55a939760ba2a4e5922/LICENSE) | Run identity, time-stamped state observations, explicit inputs/outputs, dataset versions, and lifecycle changes. |
| [Marquez initial schema](https://github.com/MarquezProject/marquez/blob/180f37b22387146187af1ef0279e3ee1d1ccd789/api/src/main/resources/marquez/db/migration/V1__initial_schema.sql), [lineage-event schema](https://github.com/MarquezProject/marquez/blob/180f37b22387146187af1ef0279e3ee1d1ccd789/api/src/main/resources/marquez/db/migration/V17.2__open_lineage.sql), and [cascade policy](https://github.com/MarquezProject/marquez/blob/180f37b22387146187af1ef0279e3ee1d1ccd789/api/src/main/resources/marquez/db/migration/V63__alter_tables_add_on_cascade_delete.sql) | `180f37b22387146187af1ef0279e3ee1d1ccd789` | [Apache-2.0](https://github.com/MarquezProject/marquez/blob/180f37b22387146187af1ef0279e3ee1d1ccd789/LICENSE) | Separate current run state, run-state history, dataset versions, run/input mappings, raw lineage events, and explicit delete relationships. |
| [Dagster event-log storage contract](https://github.com/dagster-io/dagster/blob/6542eff83164cbb1b544225d0890a38c6aeb75c9/python_modules/dagster/dagster/_core/storage/event_log/base.py) and [data-version provenance](https://github.com/dagster-io/dagster/blob/6542eff83164cbb1b544225d0890a38c6aeb75c9/python_modules/dagster/dagster/_core/definitions/data_version.py) | `6542eff83164cbb1b544225d0890a38c6aeb75c9` | [Apache-2.0](https://github.com/dagster-io/dagster/blob/6542eff83164cbb1b544225d0890a38c6aeb75c9/LICENSE) | Cursor-based event reads, explicit event/asset deletion contracts, and provenance from code version plus ordered input versions. |
| [CloudEvents core specification](https://github.com/cloudevents/spec/blob/c2845a49bc9831be02f305a4a792401b932d77d4/cloudevents/spec.md) | `c2845a49bc9831be02f305a4a792401b932d77d4` | [Apache-2.0](https://github.com/cloudevents/spec/blob/c2845a49bc9831be02f305a4a792401b932d77d4/LICENSE) | `(source, id)` event deduplication, type/schema/time metadata, compact payloads, and sensitive-context restrictions. |
| [OCI content-descriptor specification](https://github.com/opencontainers/image-spec/blob/af26a05fba5ee648512f4ea3c9fda1fcc1b6d6dc/descriptor.md) and [content-addressability considerations](https://github.com/opencontainers/image-spec/blob/af26a05fba5ee648512f4ea3c9fda1fcc1b6d6dc/considerations.md) | `af26a05fba5ee648512f4ea3c9fda1fcc1b6d6dc` | [Apache-2.0](https://github.com/opencontainers/image-spec/blob/af26a05fba5ee648512f4ea3c9fda1fcc1b6d6dc/LICENSE) | A descriptor binds media type, digest, and byte size; content is verified before use and may be safely deduplicated. |
| [GDPR Articles 5 and 17](https://eur-lex.europa.eu/eli/reg/2016/679/2016-05-04) | Official consolidated EUR-Lex text | EU legal text | Data minimisation, storage limitation, and erasure inform a conservative privacy-engineering baseline; this note does not make a jurisdiction-specific legal conclusion. |

## Evidence-to-design mapping

### 1. Persist authority before doing external work

Temporal treats workflow history, mutable state, and follow-up tasks as durable
persistence concerns. Its architecture reloads from persistence when a dirty
state update cannot be committed, and its lifecycle document explicitly calls
history/state writes durable. Nexa should adopt the boundary, not Temporal's
distributed implementation.

For every transition, one SQLite transaction should:

1. compare the expected `revision` and current state;
2. update the canonical `media_jobs` row;
3. create or update the current `media_job_attempts` row;
4. append any accepted provider event or transition evidence; and
5. increment the job revision.

File downloads, hashing, uploads, provider calls, and polling occur outside the
database mutex. Their result is committed in a short transaction. A stale poll
or delayed webhook that loses the revision compare must be re-evaluated against
the newly loaded job rather than overwriting it.

PR 13 does not need a general workflow engine or full event sourcing. The job
row is the current authority; attempt and provider-event history are the durable
evidence used for recovery, diagnosis, and later UI projection.

### 2. Define recoverable, indeterminate, and terminal states

The formal lifecycle remains the one required by `D:\Nexa.txt`:

```text
draft
  -> validating
  -> uploading_assets
  -> submitting
  -> queued
  -> running
  -> post_processing
  -> completed

failed / cancelled / expired / provider_unknown
```

Those labels need sharper semantics:

- `draft`, `validating`, and `uploading_assets` are local and recoverable;
- `submitting` means Nexa may be crossing the external side-effect boundary;
- `queued` and `running` require a persisted provider task identity;
- `post_processing` means provider success is known but outputs are not yet
  downloaded, verified, related, and committed;
- `completed`, `failed`, `cancelled`, and `expired` are terminal;
- `provider_unknown` is an indeterminate attention state, not proof of failure
  and not permission to resubmit.

Temporal's API separately represents cancel request and cancellation completion.
Nexa should likewise persist `cancellation_requested_at` while retaining the
last confirmed active state, or add a visible `cancelling` projection. It may
set `cancelled` only after local/provider confirmation. An unsupported or failed
remote cancel remains observable.

Job expiry is also separate from retention expiry. `expired` describes the
provider task or product job contract. Local asset deletion and remote provider
retention use their own deadlines and acknowledgements.

### 3. Restart recovery is a deterministic reconciliation pass

At startup, after database migration and before the renderer treats job state
as live, a single runtime owner should scan:

- all non-terminal states;
- `provider_unknown` jobs that have a provider task identity; and
- due cancellation or deletion requests.

Recovery rules are state-specific:

| Persisted state | Recovery action |
| --- | --- |
| `draft` / `validating` | Resume local validation or leave the draft editable. |
| `uploading_assets` | Reconcile verified local input assets; upload is a later adapter concern. |
| `submitting` with provider task ID | Observe that exact task; never create another attempt. |
| `submitting` without provider task ID | Query by the persisted provider idempotency key only if the adapter contract guarantees it; otherwise move to `provider_unknown`. |
| `queued` / `running` | Reattach polling/webhook observation to the persisted task identity. |
| `post_processing` | Resume output download and verification from durable attempt/result metadata. |
| terminal state | Do not resume execution; only process explicit retention/deletion work. |

OpenLineage identifies each run independently and Marquez retains current state
plus run-state/event history. The corresponding Nexa invariant is that app
restart never manufactures a new attempt just because memory was lost.

### 4. Idempotency is a stored contract, not a retry header

Temporal's start request includes a stable request ID and separately defines
workflow ID conflict/reuse and retry policy. Nexa should persist both a caller
idempotency key and a canonical request fingerprint:

- same key plus same fingerprint returns the existing job;
- same key plus a different fingerprint is a conflict;
- a new explicit retry creates the next immutable attempt number;
- each attempt gets its own provider submission key; and
- cross-provider fallback creates a new attempt only after the user's recorded
  consent permits that provider.

The fingerprint should cover canonical normalized parameters, provider/model/API
version and endpoint identity, operation, and ordered input asset IDs. It must
not depend on JSON object insertion order, timestamps, filesystem paths, or
ephemeral signed URLs.

Persist the provider endpoint/account/region scope used by an attempt. A
provider task ID is unique only inside that source scope; `provider_id` alone is
too broad, while a moving base URL string without credential/region identity is
too weak.

### 5. Attempts are immutable history, not a retry counter

Temporal exposes attempt, last failure, last completion time, retry interval,
next schedule time, and expiration as distinct fields. Marquez similarly keeps
run-state history instead of replacing prior runs. Nexa's
`media_job_attempts` should therefore preserve, per attempt:

- attempt number and provider submission idempotency key;
- provider, model, API version, endpoint/account/region identity;
- provider task ID when known;
- created, submitted, first/last observed, and completed timestamps;
- normalized state and raw/redacted failure evidence;
- retry classification and next eligible time; and
- cancellation request/result.

Prior attempts must never be overwritten when a retry or fallback begins. The
job row may cache `current_attempt_id` and retry count for fast reads, but those
are projections of attempt history. A bounded `max_attempts` is required before
any adapter starts automatic retry.

### 6. Provider events use source-scoped deduplication

CloudEvents requires producers to make `(source, id)` unique and permits a
duplicate resend to carry the same ID. Nexa should model that pair directly:

- `source` identifies provider plus endpoint/account/region or webhook source;
- `event_id` is the provider event ID or a documented stable polling identity;
- `event_type`, schema/version, provider occurrence time, and Nexa observation
  time are separate fields;
- uniqueness is `(source, event_id)`, not `(provider_id, event_id)`; and
- every event points to both `job_id` and `attempt_id`.

Polling providers without event IDs need an adapter-defined stable key derived
from provider task identity plus a provider version/update marker. Hashing the
entire volatile response body is not a reliable event identity.

Provider payloads should be bounded and redacted before persistence. CloudEvents
warns that context attributes are commonly logged and should not carry sensitive
information; it also recommends linking to large data instead of embedding it.
Nexa must therefore exclude authorization headers, signed download query
parameters, credentials, and raw private media. Store normalized status plus a
small redacted diagnostic payload, not the output binary.

### 7. A content address is earned by verifying bytes

The OCI descriptor contract combines media type, digest, and raw byte size and
requires consumers to verify untrusted content before using it. Nexa should use
the same integrity seam for media assets:

```text
sha256:<64 lowercase hex> + byte_length + detected media_type
```

The content hash is calculated from the actual raw bytes, not the request,
provider task ID, filename, URL, or metadata. Download/import uses a temporary
file, enforces a size bound while streaming, detects the media signature, hashes
the bytes, verifies any provider-declared size/digest, and only then atomically
moves the file into the durable asset store and commits the database row.

A `provider_remote` or `external` locator for bytes Nexa has never read is not a
verified content-addressed asset. Keep it as attempt/result metadata until
materialized, or represent an explicit `integrity_state = unverified`; do not
place an unverified provider-declared value into the authoritative asset-ID
column.

Use application data, not the cache directory, for retained generation assets.
The existing `ManagedLocalAssetStore` establishes useful signature validation,
SHA-256, write-once, and scoped garbage-collection patterns, but its generated
audio is an evictable cache and its theme assets have different lifetime and
size rules. PR 13 should add a separate generation-asset store rather than
silently broadening those policies.

### 8. Deduplicate blobs, never lineage occurrences

OCI content addressing permits one stored blob to satisfy many references.
OpenLineage explicitly records run inputs and outputs, and Dagster provenance
binds a materialization to code version and ordered input data versions. Nexa
should therefore separate:

- `media_assets`: one verified byte object and intrinsic metadata; and
- `media_asset_relations`: each use/production edge and its context.

Every relation needs `job_id` and `attempt_id`, because different retries or
fallback providers under one job can produce different outputs. A useful
minimal vocabulary is:

```text
input / output / derived_from / variant_of / extends / edits /
audio_track / export
```

The relation stores child asset, optional parent asset, stable ordinal, and
small operation metadata. Content deduplication must not collapse two relation
rows. For example, identical output bytes produced by two attempts share a blob
but retain two provenance occurrences.

Assets referenced by any retained relation or export use `ON DELETE RESTRICT`.
Owned history rows such as attempts and their events may use cascade when the
parent job is explicitly purged. Marquez demonstrates that cascade behavior is
schema policy, not an incidental cleanup detail; Nexa must choose it table by
table.

### 9. Privacy retention and deletion are multi-party workflows

The GDPR source establishes data minimisation, storage limitation, and erasure
as conservative privacy-engineering inputs. It does not mean every generated
asset is personal data or that one global retention duration is legally correct.
Nexa should persist enough policy to make the actual behavior visible:

- provider and data region used by each attempt;
- provider retention deadline or `unknown`;
- local retention policy and expiry;
- whether fallback to a different provider was authorized;
- remote deletion requested, confirmed, unsupported, or failed;
- local deletion requested and completed; and
- watermark/provenance facts known for each output.

Cancel is not delete. Expiry is not confirmed remote deletion. Removing a DB
row is not proof that the local blob disappeared, and deleting a shared blob is
unsafe while another retained relation references it.

An explicit user deletion flow should:

1. persist deletion intent and make the asset unavailable to new work;
2. request cancellation for an active job;
3. request remote deletion only through a provider contract that supports it;
4. remove relations/exports selected by the user;
5. delete a local blob only when no retained relation references its digest;
6. record completion or a precise remote-deletion limitation; and
7. redact or purge sensitive raw event payloads while retaining only the minimal
   non-sensitive tombstone needed to explain the outcome.

No failure path may silently send the same private input to another provider.
The existing `allow_cross_provider_fallback = false` default should remain a
hard runtime gate, not UI-only text.

## Concrete Nexa integration constraints

### Deep-module design check

- **Module**: `media_generation` owns one provider-neutral job and asset domain.
- **Interface**: callers create/read jobs, append attempt-scoped observations,
  import verified assets, and relate them without receiving a database handle.
- **Implementation**: SQLite transactions, revision checks, payload redaction,
  restart reconciliation, hashing, signature checks, and write-once paths remain
  private.
- **Depth**: the small public surface hides the state matrix, idempotency,
  concurrency, integrity, privacy, and recovery rules.
- **Seam**: the durable runtime is the boundary between product workflows and
  external media providers.
- **Adapter**: PR 14 provider integrations translate vendor contracts into the
  runtime Interface; they do not own job state.
- **Leverage**: one invariant set serves polling, webhooks, retries, fallback,
  Shot Board, compare, and export work.
- **Locality**: media-generation reasoning stays under one bounded context
  instead of being scattered through React, Tauri commands, and `video.rs`.

### Database and migration

- Add the schema as the next incremental migration in
  `crates/core/src/migrations/mod.rs`; include migration idempotency tests and
  foreign-key/index assertions.
- Use SQLite `CHECK` constraints for states, operations, booleans, non-negative
  sizes/costs, and valid JSON; use partial unique indexes for optional provider
  task identities.
- Scope provider task and event uniqueness by persisted source identity, not
  provider display ID alone.
- Use `TransactionBehavior::Immediate` for job creation and revision-checked
  transitions so two poll/webhook writers cannot both advance the same revision.
- Keep timestamps in one representation. If text RFC 3339 is retained, do not
  mix it with SQLite local-time strings or compare timestamps lexically unless
  the format is canonical UTC.

### Core module and runtime ownership

- Create a new `crates/core/src/media_generation/` module. The current
  `crates/core/src/media.rs` and video configuration remain media-analysis and
  ingestion concerns.
- Put state-machine validation, persistence, recovery planning, and asset-store
  logic in `nexa-core`; Tauri commands should be thin adapters.
- Start one recovery/observation owner after `Database::new` and migrations.
  Renderer mounts and window reloads must not create additional poll loops.
- Use the bounded database executor for async command access. Never hold the
  SQLite mutex across network or filesystem I/O.
- Do not add real provider networking in PR 13. Define ports/fakes that make
  restart, ambiguous submission, stale observation, cancellation, and deletion
  behavior testable before adapter code exists.

### Schema details that must survive later PRs

- `media_jobs`: stable Nexa ID, caller key, canonical fingerprint, state,
  revision, current attempt, normalized request, cost projection, timestamps,
  region/retention/fallback policy, and no secret material.
- `media_job_attempts`: immutable attempt identity and provider/source snapshot,
  provider task ID, attempt lifecycle, errors, retry/cancel timing.
- `media_provider_events`: source-scoped event identity, job and attempt,
  schema/type, provider occurrence time, observation time, bounded redacted
  payload.
- `media_assets`: verified digest, byte size, detected media type, durable local
  storage key, intrinsic dimensions/duration, and integrity state.
- `media_asset_relations`: job and producing/consuming attempt, child/parent
  assets, relation type, ordinal, and operation metadata.
- `media_exports`: may be created as a forward-compatible table, but rendering
  behavior belongs to PR 16.

## Rejected approaches

| Approach | Reason rejected |
| --- | --- |
| React/in-memory generation queue | App restart loses authority and leaves `running` cards permanently stale. |
| Reusing `VideoSettingsSection` or the ingestion `media.rs` module | Collapses media analysis and media generation into one bounded context. |
| Copying Temporal or embedding a general workflow engine | Nexa needs a small provider-neutral state machine, not a distributed workflow service. |
| Blindly retrying `submitting` after timeout/restart | May create duplicate billable provider jobs. |
| Treating provider task ID as Nexa job ID | Provider IDs are source-scoped, may be absent during ambiguity, and do not express local retries. |
| One mutable attempt row plus `retry_count` | Erases which provider/model/version failed and destroys diagnosis and provenance. |
| Deduplicating provider events by `provider_id` plus payload hash | Provider ID omits source scope; volatile payload hashes are not stable event identities. |
| Using URL, path, filename, or provider-declared hash as asset ID | Does not prove which bytes Nexa retained. |
| Deduplicating relation rows because asset hashes match | Loses attempt-specific input/output provenance. |
| Cascading asset deletion from job deletion | A content-addressed blob may be shared by another job/export. |
| Treating cancel, expiry, and delete as synonyms | Misstates provider confirmation and can leave remote or local data retained. |
| Persisting raw webhook/download payloads indefinitely | Can retain signed URLs, credentials, and private metadata beyond their purpose. |

## Required verification for PR 13

- Fresh and upgraded databases contain every table, index, foreign key, JSON
  check, state check, and uniqueness scope.
- The full transition matrix rejects skipped, stale-revision, and terminal-state
  transitions.
- Restart tests cover every active state, `provider_unknown`, post-processing,
  pending cancellation, and pending deletion.
- Same idempotency key/fingerprint returns the same job; the same key with a
  different fingerprint fails without writing an attempt.
- A crash after request transmission but before provider-task persistence never
  auto-submits a second job.
- New retries append attempt history and retain the prior provider/model/error.
- Duplicate `(source, event_id)` observations are harmless; the same event ID
  from a different source is accepted.
- Late poll/webhook updates cannot overwrite a newer revision or terminal state.
- Asset ingestion streams to a bound, verifies detected media type, byte size,
  and SHA-256, writes once, and rejects tampered/mismatched content.
- Identical bytes deduplicate storage while separate job/attempt relations remain
  queryable.
- A retry/fallback output points to the exact producing attempt.
- Deleting one relation does not remove a blob still referenced elsewhere;
  deleting the last relation removes only the generation-store blob in scope.
- Cancellation-request persistence and cancellation confirmation are tested
  separately, including unsupported/failed remote cancellation.
- Event payload redaction removes credentials and signed URL query data.
- No test or production path contacts a real media-generation provider in this
  PR.

## Reproducible revision audit

The GitHub snapshots were obtained with `git ls-remote <repository> HEAD`. A
reviewer can re-run that command to detect upstream movement; the evidence in
this note remains reproducible because every GitHub source and license link
contains the recorded 40-character revision.
