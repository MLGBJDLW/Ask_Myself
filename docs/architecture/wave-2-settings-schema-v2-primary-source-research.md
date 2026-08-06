# Wave 2 Settings Schema V2: Primary-source Research

This note records the primary-source review for PR 8,
`settings-schema-v2-and-migration`, from the Wave 2 work described in
`D:\Nexa.txt` lines 718-896, 1298-1308, and 1352-1363. It was prepared on
2026-08-06 against immutable upstream commits. It is a design input for an
independent Nexa implementation, not a proposal to copy another project's
configuration format or persistence code.

## Executive decision

PR 8 should establish the versioned settings substrate and prove reversible
migration before PR 9 introduces the full connection/model/capability
registries:

1. Persist sparse, versioned settings documents for four explicit scopes:
   Application, Workspace, Agent, and Task. Resolve them in that order and
   return per-field provenance with the effective value.
2. Keep ordinary configuration merge semantics separate from permission
   composition. Settings may use a higher-scope override; permissions are
   fail-closed, with `Deny > RequireApproval > Allow`, and a task grant may
   satisfy an approval requirement but may never relax a parent deny.
3. Make `inherit` structural: an absent field means inherit, an explicit null
   means clear only where the field schema permits it, and Reset to inherited
   deletes the override. Do not overload empty strings, zeroes, or default
   values to mean inheritance.
4. Treat presets as immutable, versioned sparse patches plus user overrides,
   not copied full settings objects. Pin the selected preset version so an app
   update cannot silently change an existing Agent's effective permissions.
5. Migrate through a transaction that stores an encrypted raw V1 snapshot,
   writes V2, verifies a V1 -> V2 -> V1 round trip, and only then flips one
   active-schema pointer. Explicit rollback flips the pointer back and restores
   the exact snapshot; it does not attempt a lossy reverse transform.
6. PR 8 must not create another credential copy. V2 stores connection and
   credential references only. Existing encrypted secrets remain in their V1
   homes until the Connection Registry migration in PR 9.

The old rows and the V2 documents should coexist through the rollback window.
Removal of V1 storage is a later, separately reviewed migration after rollback
telemetry and previous-version compatibility fixtures are clean.

## Reviewed upstream revisions

| Project | Immutable revision | Evidence used |
| --- | --- | --- |
| OpenAI Codex | Stable [`rust-v0.146.1`](https://github.com/openai/codex/releases/tag/rust-v0.146.1) at `79b4f03d35962b005b007a015113b38930711665`; current configuration source at `82b17bc724aa789c482d29c02a399faf3e2eafcf` | Ordered config layers, effective-value merge, per-key origin metadata, layer fingerprints, named sparse profiles, and domain-specific policy composition |
| Home Assistant Core | [`2026.8.0`](https://github.com/home-assistant/core/releases/tag/2026.8.0) at `4a9dce13f61d03960ad5d2710e2af9fd2a78af54` | Major/minor versions on persisted config entries, migration gating, explicit migration failure, and ordered storage upgrades |
| Kubernetes | [`v1.35.0`](https://github.com/kubernetes/kubernetes/releases/tag/v1.35.0) at `66452049f3d692768c39c797b21b793dce80314e` | Separation of endpoints, authentication material, and named binding contexts; extension preservation |
| Kubernetes documentation | Current source at `437c1d235d3a2233e39fbb7d1bed0b72f136633c` | Normative lossless version round trips and release rollback compatibility |

No secondary summaries were used.

## Requirements traced from `D:\Nexa.txt`

| Wave 2 requirement | PR 8 exit evidence |
| --- | --- |
| App -> Workspace -> Agent -> Task inheritance | One deterministic resolver accepts four sparse documents and returns effective values plus the winning scope/revision for every leaf |
| Settings information architecture | The persisted schema separates connection refs, capability/model bindings, permissions, and advanced model parameters even if PR 8 retains the old UI temporarily |
| Presets | Built-ins are versioned sparse patches; existing selections are pinned; Custom is user-owned data; Reset removes an override |
| Credentials configured once | V2 contains `connectionId`/`credentialRef`, never an API key; migration does not duplicate decrypted secrets |
| Lossless automatic migration | V1 fixtures round-trip semantically through V2; unknown data survives in a bounded passthrough; failure cannot activate partial V2 data |
| Explicit rollback | A durable migration record and snapshot can restore the exact V1 bytes/columns and active schema repeatedly without deleting V2 |
| Future capability registry | Bindings identify a capability and primary/fallback model references without embedding connection secrets or duplicating model descriptors |

## Reviewed Nexa seams

The repository has strong pieces to reuse, but the current storage boundaries
cannot express Settings V2 end to end:

- [`AppConfig`](../../crates/core/src/app_settings.rs) is one JSON value under
  the `app_config` key. Serde defaults provide forward tolerance, but the only
  persisted settings-version marker is currently
  `tool_visibility_defaults_version`; there is no document schema version,
  revision, scope identity, or field-origin metadata.
- `AppConfig` embeds image, TTS, STT, and web-search API keys. Save/load encrypts
  and decrypts those fields, so a migration that snapshots the in-memory value
  would accidentally create a second plaintext secret boundary. Migration must
  snapshot raw stored bytes or explicitly re-encrypt before persistence.
- [`AgentConfig` and `SaveAgentConfigInput`](../../crates/core/src/conversation/mod.rs)
  combine provider, API key, endpoint, model, tuning, summarization, image
  generation, delegated tools/skills, budgets, and deadlines in one row and API.
  This is the exact connection/model/capability/policy coupling PR 8 must stop
  reproducing in its new schema.
- [`provider-presets.json`](../../shared/provider-presets.json) is already a
  shared frontend/backend catalog, and the provider catalog resolves exact
  endpoints. Preserve this source-of-truth boundary; a settings migration must
  not infer a trusted provider or credential scope from a label or similar
  hostname.
- [`policy_engine.rs`](../../crates/core/src/policy_engine.rs) already uses
  `Allow`, `RequireApproval`, and `Deny`, with deny terminating evaluation.
  Settings V2 should retain that conservative lattice rather than make policy a
  generic last-write-wins boolean.
- [`tool_permission_policies`](../../crates/core/src/approval.rs) currently
  resolves exact-to-wildcard resource keys, while global approval mode lives in
  `AppConfig`. There is no workspace/agent/task scope or origin explanation;
  PR 8 should introduce a scope-aware representation without silently treating
  old wildcard grants as application-wide grants.
- [`run_migrations`](../../crates/core/src/migrations/mod.rs) records named SQL
  migrations and has useful idempotent partial-migration tests. However, the
  current future-migration SQL and `_migrations` marker are not wrapped together
  by an explicit Rust transaction. The V1-to-V2 data transform and its active
  pointer require a dedicated transaction and fault-injection coverage.

## 1. Version the persisted document, not individual defaults

### Primary-source evidence

Home Assistant stores both a major `version` and `minor_version` on every
configuration entry
([entry fields](https://github.com/home-assistant/core/blob/4a9dce13f61d03960ad5d2710e2af9fd2a78af54/homeassistant/config_entries.py#L391-L459)).
Before setup it rejects data newer than the running handler, requires a major
migration handler when versions differ, and reports migration failure instead
of continuing with a guessed shape
([migration gate](https://github.com/home-assistant/core/blob/4a9dce13f61d03960ad5d2710e2af9fd2a78af54/homeassistant/config_entries.py#L1145-L1195)).
Its store migration receives the old major/minor version and applies ordered,
conditional transformations
([storage migration](https://github.com/home-assistant/core/blob/4a9dce13f61d03960ad5d2710e2af9fd2a78af54/homeassistant/config_entries.py#L2057-L2110)).

This is a useful versioning and failure-state precedent. It is not by itself a
rollback design: the shown migration mutates the old object and does not retain
the original representation. Nexa needs the additional snapshot and active
pointer described below.

### Nexa V2 envelope

Persist one sparse document per scope, with schema metadata outside the patch:

```rust
enum SettingsScopeKind {
    Application,
    Workspace,
    Agent,
    Task,
}

struct SettingsDocumentV2 {
    schema_version: u32,       // exactly 2 for this contract
    revision: u64,             // optimistic-concurrency token
    scope_kind: SettingsScopeKind,
    scope_id: String,          // "app" or stable workspace/agent/task id
    preset: Option<PresetSelection>,
    values: SettingsPatchV2,   // sparse overrides only
    extensions: JsonObject,    // bounded legacy/unknown passthrough
}
```

Recommended persistence:

```text
settings_documents
  scope_kind       TEXT NOT NULL
  scope_id         TEXT NOT NULL
  schema_version   INTEGER NOT NULL
  revision         INTEGER NOT NULL
  preset_id        TEXT
  preset_version   INTEGER
  preset_hash      TEXT
  values_json      TEXT NOT NULL
  extensions_json  TEXT NOT NULL DEFAULT '{}'
  created_at / updated_at
  PRIMARY KEY (scope_kind, scope_id)

settings_schema_state
  singleton_id     INTEGER PRIMARY KEY CHECK (singleton_id = 1)
  active_version   INTEGER NOT NULL
  migration_id     TEXT
  activated_at     TEXT

settings_migration_snapshots
  migration_id / source_kind / source_id
  from_version / to_version
  source_revision
  source_ciphertext_or_raw_json
  source_hash / target_hash
  status / created_at / rolled_back_at
```

Use a compare-and-set write (`WHERE revision = expected_revision`) and return a
typed conflict containing the current revision. Codex's configuration loader
explicitly exposes per-layer stable versions for optimistic concurrency and
per-key origins for UI explanation
([loader contract](https://github.com/openai/codex/blob/82b17bc724aa789c482d29c02a399faf3e2eafcf/codex-rs/config/src/loader/README.md#L1-L22)).

`schema_version` governs decode/transform compatibility. `revision` governs
concurrent edits. A preset version and catalog version are references, not
substitutes for either field.

Unknown V1 fields must not disappear merely because the V2 Rust struct does not
yet understand them. Kubernetes kubeconfig keeps extension maps specifically
so reads and writes do not clobber unknown data
([config and extensions](https://github.com/kubernetes/kubernetes/blob/66452049f3d692768c39c797b21b793dce80314e/staging/src/k8s.io/client-go/tools/clientcmd/api/types.go#L31-L56)).
Nexa should keep a size-limited `extensions.legacyV1` passthrough during the
compatibility window, reject duplicate/oversized keys, and never execute or
trust extension content.

## 2. Layered values need provenance and field-specific merge rules

### Primary-source evidence

Codex defines an explicit precedence stack, folds layers from lowest to highest,
ignores disabled layers when calculating the effective result, and surfaces the
layer list for UI
([layering model](https://github.com/openai/codex/blob/82b17bc724aa789c482d29c02a399faf3e2eafcf/codex-rs/config/src/loader/README.md#L24-L44)).
Its `ConfigLayerStack` separately exposes the effective merged config, per-field
origins, and iterators in both precedence directions
([effective config and origins](https://github.com/openai/codex/blob/82b17bc724aa789c482d29c02a399faf3e2eafcf/codex-rs/config/src/state.rs#L444-L499)).
The generic merge recursively merges tables but replaces non-table values,
including arrays, while named exceptional paths receive explicit semantics
([merge implementation](https://github.com/openai/codex/blob/82b17bc724aa789c482d29c02a399faf3e2eafcf/codex-rs/config/src/merge.rs#L56-L120),
[array boundary](https://github.com/openai/codex/blob/82b17bc724aa789c482d29c02a399faf3e2eafcf/codex-rs/config/src/merge.rs#L131-L134)).

### Nexa resolver contract

Resolve in this fixed order:

```text
Application defaults
  -> selected Workspace profile
  -> Agent profile
  -> Task temporary overrides/grants
```

For every leaf return:

```rust
struct ResolvedSetting<T> {
    value: T,
    origin_scope: SettingsScopeKind,
    origin_id: String,
    origin_revision: u64,
    preset_origin: Option<PresetSelection>,
}
```

Merge rules must live in the schema/typed resolver, not in React:

- scalar/object leaf: highest present override wins;
- object container: recursively merge declared fields;
- ordered fallback models: replace as a complete ordered list;
- capability set: explicit replace or keyed patch, never accidental array
  concatenation;
- deny lists and immutable safety ceilings: union/intersection according to the
  policy rule, never ordinary last-write-wins;
- nullable fields: null clears only when declared `clearable`; absence inherits;
- unknown fields: preserve in extensions but exclude from effective runtime
  configuration.

The Settings UI edits only the selected layer. It shows the effective value and
`Inherited from <scope/preset>` from resolver provenance. Reset to inheritance
deletes the leaf from that document and increments its revision. The UI must
not write the currently inherited value into the child layer, because that
would create a hidden override and prevent future parent updates.

At turn creation, persist the effective settings/policy revision set on the
task run. A mid-run settings edit applies to the next run unless a narrowly
defined live field explicitly opts in; provider/model/credential/policy changes
must not mutate an in-flight execution.

## 3. Permission inheritance is not generic configuration merge

Codex's requirements stack documents the same distinction: ordinary values use
layer precedence, while security-sensitive fields have domain-specific rules,
including fail-closed conflicts and union of filesystem deny paths
([requirements composition](https://github.com/openai/codex/blob/82b17bc724aa789c482d29c02a399faf3e2eafcf/codex-rs/config/src/requirements_layers/stack.rs#L1-L14)).

Nexa should compose each matching policy across all four scopes using this
lattice:

```text
Deny  >  RequireApproval  >  Allow
```

Rules:

- an App or Workspace deny is a ceiling and cannot be changed by Agent or Task;
- a child `RequireApproval` can narrow a parent Allow;
- a child Allow cannot relax a parent `RequireApproval` without a durable,
  user-approved task grant;
- a task grant is scoped to task id, capability/resource selector, issuer,
  creation time, expiry, and one-shot/session semantics;
- a task grant satisfies only `RequireApproval`; it never overrides Deny;
- unrecognized policy values, missing parents, expired grants, and resolver
  conflicts fail closed;
- the effective decision includes every matched rule id and origin for the
  approval UI and audit log, but never includes a secret value.

Do not automatically reinterpret existing `allow_forever` wildcard policies as
Application-level V2 Allow rules. Import them as legacy-scoped rules with a
visible review flag, or leave them on the compatibility path until the user
confirms their new scope.

## 4. Separate connection identity, credentials, and capability bindings

Kubernetes kubeconfig is a useful structural precedent. Its top-level config
has separately named `Clusters`, `AuthInfos`, and `Contexts`; a Context is only
a tuple of references linking communication target, identity, and namespace
([top-level maps](https://github.com/kubernetes/kubernetes/blob/66452049f3d692768c39c797b21b793dce80314e/staging/src/k8s.io/client-go/tools/clientcmd/api/types.go#L31-L55),
[context references](https://github.com/kubernetes/kubernetes/blob/66452049f3d692768c39c797b21b793dce80314e/staging/src/k8s.io/client-go/tools/clientcmd/api/types.go#L161-L176)).
Endpoint/TLS fields live on Cluster, while tokens, keys, and passwords live on
AuthInfo and are marked as sensitive data
([connection fields](https://github.com/kubernetes/kubernetes/blob/66452049f3d692768c39c797b21b793dce80314e/staging/src/k8s.io/client-go/tools/clientcmd/api/types.go#L68-L106),
[authentication fields](https://github.com/kubernetes/kubernetes/blob/66452049f3d692768c39c797b21b793dce80314e/staging/src/k8s.io/client-go/tools/clientcmd/api/types.go#L108-L159)).

The corresponding Nexa boundary is:

```text
Connection
  provider account + endpoint + region + organization/project
  -> credentialRef (secret vault row; never a model id)

ModelDescriptor
  immutable catalog identity + modalities/capabilities/limits/status
  -> compatible connection kinds (never an API key)

CapabilityBinding
  capability id
  -> primary model ref + ordered fallback model refs
  -> optional connection selection per model ref

Agent/Task settings
  -> capability binding refs + policy overrides + advanced parameters
```

PR 8 may define these reference types and migrate V1 provider/model values into
a legacy binding projection. PR 9 owns the normalized registries and secret
move. Until then, V1 remains the only owner of each encrypted API key. Never
copy a key into `settings_documents`, preset JSON, model descriptors, migration
telemetry, or rollback diagnostics.

Unknown, HTTP, non-standard-port, or user-edited endpoints remain untrusted and
must not inherit a catalog credential merely because provider/model labels
match. Preserve Nexa's current exact endpoint boundary during migration.

## 5. Presets are pinned sparse patches, not mutable full copies

Codex named profiles model configuration options as optional fields and refer
to a model provider by key rather than embedding provider data
([profile shape](https://github.com/openai/codex/blob/82b17bc724aa789c482d29c02a399faf3e2eafcf/codex-rs/config/src/profile_toml.rs#L20-L71)).
Its user profile is layered over base user config rather than replacing the
base document
([profile merge](https://github.com/openai/codex/blob/82b17bc724aa789c482d29c02a399faf3e2eafcf/codex-rs/config/src/state.rs#L323-L338)).

Implement each built-in Nexa preset as:

```rust
struct PresetDefinitionV1 {
    id: String,              // chat_only, research, coding, full_agent_safe...
    version: u32,
    patch: SettingsPatchV2,
    content_hash: String,
}
```

Selection persists `(preset_id, preset_version, content_hash)` plus the user's
sparse overrides. Existing selections resolve against the pinned definition.
When a shipped preset changes, publish a new version and offer an explicit
upgrade diff; do not silently broaden tools or permissions. A missing pinned
version fails closed and shows repair UI. `Custom` is materialized as a
user-owned sparse patch without a built-in reference.

Preset patches cannot contain credentials, raw endpoints with secrets, task
grants, or workspace ids. Applying a preset does not erase unrelated custom
overrides unless the user confirms a replace operation with a preview.

## 6. Automatic migration and explicit rollback

Kubernetes' compatibility policy supplies the critical standard: objects must
round-trip between versions without information loss
([round-trip rule](https://github.com/kubernetes/website/blob/437c1d235d3a2233e39fbb7d1bed0b72f136633c/content/en/docs/reference/using-api/deprecation-policy.md#L64-L73)).
It also requires the new and previous storage versions to coexist before the
storage version advances so an upgrade can roll back without breakage
([storage rollback rule](https://github.com/kubernetes/website/blob/437c1d235d3a2233e39fbb7d1bed0b72f136633c/content/en/docs/reference/using-api/deprecation-policy.md#L99-L112)).

Apply the same release discipline locally:

### Phase A: preflight without mutation

1. Read raw encrypted V1 `app_config` JSON and raw `agent_configs` columns.
2. Canonicalize only documented aliases/defaults; retain the exact source bytes
   and a cryptographic hash.
3. Decode into a `LegacySettingsV1` type that preserves unknown fields.
4. Transform into candidate V2 documents and a V1 compatibility projection.
5. Verify schema validation, referential integrity, credential boundaries, and
   semantic V1 -> V2 -> V1 equality before opening the write transaction.

### Phase B: one transactional activation

Inside one `BEGIN IMMEDIATE` transaction:

1. insert an idempotent migration record keyed by a stable migration id;
2. store the raw encrypted V1 snapshots and hashes;
3. insert/upsert every V2 document with revision 1;
4. store target hashes and round-trip verification status;
5. set `settings_schema_state.active_version = 2` last;
6. commit the migration record and active pointer together.

On any error, roll back the transaction and continue on V1 with a typed,
privacy-safe migration failure. Never partially activate V2 or silently fall
back to defaults.

### Compatibility window

- Reads use the active pointer; there is no per-call heuristic based on which
  rows happen to exist.
- Keep V1 rows and V2 documents. V2 writes may update a compatibility
  projection where representable, but must never overwrite the original
  migration snapshot.
- Preserve V2-only edits in the V2 sidecar even after rollback; an old binary
  may ignore them, but rollback must not delete them.
- Do not delete snapshots automatically. Provide a later explicit cleanup only
  after the supported rollback window and telemetry review.

### Explicit rollback

`rollback_settings_schema_v2` must:

1. validate snapshot hashes and confirm the target migration is active;
2. restore exact V1 raw values/columns in a transaction;
3. flip `active_version` to 1 last;
4. mark the migration `rolled_back` without deleting V2;
5. be idempotent and safe after restart or repeated invocation.

Rollback is a pointer flip plus snapshot restoration, not `convert(v2) -> v1`.
That distinction protects V1 fields unknown to the V2 runtime and avoids
pretending that V2-only concepts have a lossless V1 representation.

## Required tests and observability

### Migration fixtures

- Golden V1 fixtures: default/empty, every provider family, custom endpoints,
  all speech/image/web-search configs, every Agent advanced field, local models,
  null/empty legacy values, unknown fields, and malformed-but-previously
  tolerated values.
- For every valid fixture, assert semantic V1 -> V2 -> V1 equality, exact
  preservation of unknown fields, no credential duplication, and stable hashes.
- Load a migrated database with the V1 compatibility reader after rollback and
  prove it yields the original effective configuration.
- Inject failure after snapshot, each document write, verification, pointer
  flip, and migration marker; every failure leaves one coherent active version.
- Run automatic migration twice, restart between phases, and invoke rollback
  twice. Results and revisions remain stable.
- Corrupt or remove a snapshot and prove rollback fails closed without changing
  the active version.

### Resolver and policy matrix

- Table-test every scalar/null/absent combination across all four scopes and
  assert value, origin scope/id/revision, and Reset behavior.
- Test ordered-list replacement, keyed capability patches, unknown extensions,
  preset + user override, missing pinned preset, and preset-version upgrades.
- Exhaustively test policy combinations. A parent Deny is never relaxed; a
  child RequireApproval narrows Allow; an approved, unexpired task grant can
  satisfy RequireApproval only for its exact task/resource.
- Resolve two simultaneous edits from the same revision: one succeeds and one
  receives a typed conflict without last-writer data loss.
- Pin a task's effective revision set and prove later settings changes do not
  alter the in-flight provider, model, credentials, or permission decision.

### Privacy-safe telemetry

Record migration id, source/target schema versions, duration, document counts,
result, rollback result, failing phase/code, resolver conflicts, and preset
version mismatches. Never record raw settings JSON, API keys, encrypted secret
payloads, endpoints containing credentials, extension contents, task grants,
or migration snapshots.

## PR 8 implementation boundary and rollout order

1. Add typed V1/V2 documents, scope ids, revisions, preset references, and
   effective resolver with provenance.
2. Add transactional settings migration support plus schema-state and snapshot
   tables; do not squeeze the data transform into an untracked load-time Serde
   default migration.
3. Implement V1 -> V2 mapping, compatibility projection, automatic activation,
   explicit rollback, and fault-injection/golden-fixture tests.
4. Add policy composition and task-grant types, reusing the current conservative
   policy effect lattice. Keep existing runtime policy reads behind an adapter
   until parity is proven.
5. Add versioned built-in preset definitions and resolver tests. UI may expose
   effective origin/Reset minimally, but a full Settings information-architecture
   redesign is not required to prove the storage contract.
6. Keep existing credentials in V1 encrypted storage and store only references
   in V2. PR 9 creates the Connection, Model, and Capability registries and then
   performs a separately reversible secret migration.
7. Do not remove V1 storage, old read paths, or snapshots in PR 8.

PR 8 exits only when migration is atomic and idempotent, rollback restores the
exact V1 representation, all four scopes resolve deterministically with field
origins, permission inheritance cannot broaden a parent deny, presets are
pinned sparse patches, and no new credential copy is created.

## Implemented PR 8 contract

The Nexa implementation keeps the public seam in
`crates/core/src/settings_schema_v2.rs` and the storage change in migration
`v095_settings_schema_v2`:

- V2 profiles carry schema version, compare-and-set revision, explicit scope,
  a pinned preset selection, sparse connection/model/capability/policy/advanced
  namespaces, bounded compatibility extensions, and legacy-source metadata.
- Ordinary values resolve Application -> Workspace -> Agent -> Task with the
  winning scope, revision, and preset origin. Policies compose independently
  through `Deny > RequireApproval > Allow`; resolution fails closed without an
  Application policy ceiling. An exact, unexpired task/resource grant can
  satisfy approval but cannot relax Deny, and consumed one-shot grants are
  rejected.
- Built-in presets are credential-free versioned patches. Their SHA-256 hash is
  part of the saved selection, and a missing or changed pinned definition
  fails closed.
- Startup performs preflight plus explicit field-by-field verification of every
  V1 -> V2 structured connection, model, capability, advanced-setting, and
  compatibility mapping. It then writes agent and application profiles,
  outer-encrypted exact V1 snapshots, hashes, journal rows, and the
  active-version pointer in one immediate SQLite transaction. A failed
  verification leaves V1 active.
- V2 documents contain credential references only. Known and unknown secret
  fields and credential-bearing endpoints are removed from compatibility
  projections, and native V2 writes recursively reject inline secrets; exact
  raw values exist only inside the outer-encrypted rollback snapshot and the
  unchanged V1 store.
- Explicit global rollback verifies every snapshot hash, restores exact V1
  agent and application rows, flips the pointer last, and retains V2 sidecars.
  Repeated rollback is idempotent, and startup does not silently reactivate a
  deliberately rolled back schema.
- The desktop bridge exposes migration state, profile listing/CAS saves,
  explicit migration, and rollback. Provider execution continues through the
  V1 compatibility reader until PR 9 installs the normalized registries; V1
  writes and their migration-managed V2 projection commit atomically while V2
  is active. Projection fingerprints ignore encryption/timestamp churn without
  mutating the original encrypted rollback snapshot or its independent hash.
  Deletions supersede their journal entry, and full migration removes orphaned
  managed profiles so rollback cannot resurrect an intentionally deleted row.

## License and integration boundaries

| Upstream | License at reviewed commit | Permitted design use | Integration boundary |
| --- | --- | --- | --- |
| OpenAI Codex | [Apache-2.0](https://github.com/openai/codex/blob/82b17bc724aa789c482d29c02a399faf3e2eafcf/LICENSE) | Layer precedence, origins, revisions/fingerprints, sparse profile, and domain-specific policy composition concepts | Implement in Nexa Rust/SQLite types; do not copy Codex config loader or TOML-specific exceptions. Preserve notices if code is ever copied |
| Home Assistant Core | [Apache-2.0](https://github.com/home-assistant/core/blob/4a9dce13f61d03960ad5d2710e2af9fd2a78af54/LICENSE.md) | Major/minor config versions, migration gate, explicit migration failure, and ordered upgrade concepts | Do not import Python storage code or treat its in-place migration as Nexa rollback support |
| Kubernetes v1.35.0 | [Apache-2.0](https://github.com/kubernetes/kubernetes/blob/66452049f3d692768c39c797b21b793dce80314e/LICENSE) | Separate connection/auth/binding identity and extension preservation concepts | Nexa does not adopt kubeconfig or Kubernetes APIs; use independent connection/model/capability schemas |
| Kubernetes documentation | [CC-BY-4.0](https://github.com/kubernetes/website/blob/437c1d235d3a2233e39fbb7d1bed0b72f136633c/LICENSE) | Compatibility requirements for lossless round trips and rollback-safe storage-version transitions | Treat as design policy with attribution; do not copy documentation text into product UI |

All external claims in this note point to upstream source or first-party project
policy pinned to immutable commits. The resulting Nexa implementation should be
written independently and preserve the repository's existing exact-endpoint,
credential-encryption, and conservative-policy boundaries.
