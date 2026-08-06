# Wave 2 Connection, Model, and Capability Registry: Primary-source Research

This note records the primary-source review for PR 9,
`connection-model-capability-registry`, from the Wave 2 work described in
`D:\Nexa.txt` lines 718-896, 1298-1308, and 1350-1363. It was prepared on
2026-08-06 against immutable upstream commits. It is a design input for an
independent Nexa implementation; no upstream source or catalog data should be
copied into Nexa.

## Executive decision

PR 9 should make one runtime registry service authoritative for connection
identity, model availability, and capability-to-model resolution while keeping
Settings V2 as the owner of scoped user choices:

1. A **Connection** is a stable account/endpoint instance. It references a
   credential; it never owns secret material. `provider_id`, display name, or a
   similar hostname must never be enough to reuse credentials.
2. A **Model Definition** describes a provider model independently of any user
   account. A **Model Target** binds that definition to a concrete Connection
   and upstream model ID. Capability bindings point to Model Targets, because
   availability, region, billing, and credentials are connection-specific.
3. A **Capability Binding** is a Settings V2 value with one primary target and
   ordered fallbacks. It is resolved only after compatibility, runtime adapter,
   policy, data-egress, and live availability checks.
4. The existing text, image, embedding, STT, and TTS JSON catalogs remain build
   inputs for this PR, but they are projected into one validated descriptor
   schema. They stop being independent runtime sources of truth. Video remains
   representable in the schema but its provider adapters belong to later PRs.
5. Discovery and probes may establish `Discoverable` and `Callable`; neither
   may claim `ProductReady`. Product readiness is a release assertion backed by
   a registered runtime adapter, UI support, validation, and tests.
6. Activation is reversible: import existing Settings V2 references, shadow
   resolve legacy and registry targets, compare decisions, then switch runtime
   reads per capability behind one durable activation record. Rollback changes
   the read pointer and preserves registry rows for diagnosis.

This creates the requested Settings information architecture without putting
permissions, tools, or advanced generation parameters back into a Provider
form. Connections, Models, Agent Capabilities, Permissions, and Advanced remain
separate domains.

## Reviewed upstream revisions

| Project | Immutable revision | Evidence used |
| --- | --- | --- |
| OpenAI Codex | Current source at [`1151b23f01accb19e55c090a3349a32fdf2b4685`](https://github.com/openai/codex/commit/1151b23f01accb19e55c090a3349a32fdf2b4685) | Model selection references a provider registry entry; provider transport/auth metadata is separate; credentials have a selectable store; built-in IDs cannot be overridden |
| Continue | Current source at [`5522c6f44ca0ac3528b37244818fbfa39b5af470`](https://github.com/continuedev/continue/commit/5522c6f44ca0ac3528b37244818fbfa39b5af470) | Explicit model roles and capabilities; typed, namespaced secret locations; late client resolution and post-resolution validation |
| LiteLLM | Current source at [`b66d4e6965c797163e03e95de59bc23d9d62d4e7`](https://github.com/BerriAI/litellm/commit/b66d4e6965c797163e03e95de59bc23d9d62d4e7) | Cross-modality model metadata; runtime/provider-specific parameter support; remote-catalog integrity checks and bundled fallback |
| Kubernetes | Current source at [`b882c60b4023bdf09264c2d5d30a2cadebc240fb`](https://github.com/kubernetes/kubernetes/commit/b882c60b4023bdf09264c2d5d30a2cadebc240fb) | Separate endpoint, identity, and named binding-context objects; reference-based reuse |

No secondary summaries were used.

## Requirements traced from `D:\Nexa.txt`

| Wave 2 requirement | PR 9 exit evidence |
| --- | --- |
| Connection Registry: credentials configured once | One account/endpoint Connection stores a validated `credentialRef`; multiple models and capabilities reuse its ID; no registry payload, event, log, or cache contains a key |
| Model Registry: text, vision, image, video, speech, embedding, reranking | One descriptor/query surface represents all modalities and operations; existing catalogs are projections, not separate selection authorities |
| Capability Binding: primary/fallback | Every model-backed capability resolves one primary and an ordered fallback chain to Model Targets, with a machine-readable explanation for rejection or fallback |
| Settings information architecture | Connections manage accounts/endpoints/status; Models show definitions and per-connection availability; Agent Capabilities select targets; Permissions and Advanced remain separate |
| Lossless migration and rollback | Existing Settings V2 connection/model triples import deterministically; shadow parity is recorded; activation and rollback do not rewrite or delete legacy data |
| Runtime truth | An eligible target requires descriptor compatibility, a registered adapter operation, an active connection, resolvable credentials where required, policy approval, and current availability |

## Reviewed Nexa seams

Nexa already contains most of the vocabulary PR 9 needs. The missing piece is
an authoritative persisted relationship among those parts:

- [`settings_schema_v2.rs`](../../crates/core/src/settings_schema_v2.rs)
  already defines versioned connection references, model references, primary
  and fallback capability bindings, inheritance, revisions, and secret-free
  validation. It deliberately remains a shadow of legacy runtime settings.
  PR 9 should activate these references, not create a second settings format.
- [`model_catalog/descriptor.rs`](../../crates/core/src/model_catalog/descriptor.rs)
  already defines schema-versioned descriptors, modalities, lifecycle, access,
  limits, source, credential availability, and the `Known -> Discoverable ->
  Callable -> ProductReady` ladder.
- [`model_catalog/projection.rs`](../../crates/core/src/model_catalog/projection.rs)
  already projects five shared catalogs into providers, endpoints, and models,
  resolves built-ins by exact endpoint, and derives opaque IDs for custom
  endpoints. This is the migration adapter, not a reason to retain five
  runtime registries.
- [`model_catalog/merge.rs`](../../crates/core/src/model_catalog/merge.rs)
  already scopes discovery and probes to an endpoint, respects tombstones, and
  prevents discovery alone from claiming product readiness.
- [`providerCredentials.ts`](../../apps/desktop/src/lib/providerCredentials.ts)
  already implements the critical credential boundary: only exact trusted
  HTTPS endpoints share provider credentials; Token/Coding Plan, unknown, and
  user-edited endpoints remain endpoint-scoped.
- [`provider_catalog.rs`](../../crates/core/src/provider_catalog.rs) and the
  media catalog loaders still provide separate runtime entry points. PR 9 must
  route selection through the unified registry before deleting any legacy
  loaders.
- The database has Settings V2 profiles and legacy `agent_configs` model and
  endpoint identity columns, but no durable Connection, Model Target,
  availability snapshot, or activation-parity store.

Two current types are intentionally transitional. `ConnectionReferenceV2`
duplicates provider/endpoint/base URL alongside an ID, and `ModelReferenceV2`
contains `providerId + endpointId + modelId` without a connection. PR 9 should
accept those shapes during migration, canonicalize them, and emit stable
registry references for new writes.

## 1. Separate definitions, instances, and selections

### Primary-source evidence

OpenAI Codex stores the selected model separately from the selected
`model_provider` registry key
([configuration fields](https://github.com/openai/codex/blob/1151b23f01accb19e55c090a3349a32fdf2b4685/codex-rs/config/src/config_toml.rs#L147-L160)).
Provider definitions carry endpoint, environment-key, wire protocol, headers,
and retry information
([provider descriptor](https://github.com/openai/codex/blob/1151b23f01accb19e55c090a3349a32fdf2b4685/codex-rs/model-provider-info/src/lib.rs#L84-L130)),
while the credential store is configured separately
([credential store choice](https://github.com/openai/codex/blob/1151b23f01accb19e55c090a3349a32fdf2b4685/codex-rs/config/src/config_toml.rs#L245-L268)).

Kubernetes makes the same relationship explicit at a more general level:
clusters, authentication identities, and contexts are separate named maps
([top-level maps](https://github.com/kubernetes/kubernetes/blob/b882c60b4023bdf09264c2d5d30a2cadebc240fb/staging/src/k8s.io/client-go/tools/clientcmd/api/types.go#L28-L55));
a context refers to a cluster and an identity instead of copying either
([context references](https://github.com/kubernetes/kubernetes/blob/b882c60b4023bdf09264c2d5d30a2cadebc240fb/staging/src/k8s.io/client-go/tools/clientcmd/api/types.go#L161-L175)).

### Nexa decision

Use five concepts with one owner each:

| Concept | Owns | Must not own |
| --- | --- | --- |
| `ProviderDefinition` | Stable built-in/custom namespace, display metadata, supported endpoint contracts | User credentials, selected model, live availability |
| `ConnectionRecord` | Account/endpoint instance, region/org/project, credential reference, status, revision | Secret value, model capabilities, Agent permissions |
| `ModelDefinition` | Canonical model identity, modalities, capabilities, limits, lifecycle, provenance | Credential state, selection, per-account availability |
| `ModelTarget` | Connection plus upstream model/deployment ID and per-connection availability | Copied secret, inherited settings, global capability claims |
| `CapabilityBinding` | Primary target, ordered fallbacks, fallback constraints, Settings V2 provenance | Provider transport details, permission grants, duplicated descriptor |

The distinction between Model Definition and Model Target is required for
Azure deployment names, gateways/aggregators, regions, private previews, and
accounts with different allowlists. The same model can have many Targets; the
same Connection can serve many models and capabilities.

### Stable identity rules

- Built-in provider IDs are code-owned and reserved. Custom definitions use a
  separate namespace such as `custom:<uuid>` and cannot shadow built-ins.
  Codex validates reserved provider IDs and rejects collisions
  ([validation](https://github.com/openai/codex/blob/1151b23f01accb19e55c090a3349a32fdf2b4685/codex-rs/config/src/config_toml.rs#L901-L945)).
- A Connection ID is an opaque UUID. Equality is never inferred from its label.
- The endpoint fingerprint is computed from a sanitized canonical tuple:
  provider contract, scheme, lowercase IDNA host, effective port, normalized
  path, region, organization/project discriminator, and API style. Userinfo,
  query, fragment, and credentials are rejected before hashing.
- A canonical Model Definition key is `(provider_definition_id,
  canonical_model_id)`. A Model Target key is `(connection_id,
  upstream_model_id)`. Store optional `upstream_provider_id` for gateways and
  usage attribution; do not rewrite the invocation-provider identity.
- Aliases are scoped to one Provider Definition and descriptor revision. An
  alias cannot cross providers or endpoints implicitly.
- Bindings persist target IDs and the expected target revision. Human-readable
  provider/model strings are display and migration data, not authorization.

## 2. Connection Registry and credential boundary

### Primary-source evidence

Kubernetes keeps network configuration in `Cluster`
([endpoint fields](https://github.com/kubernetes/kubernetes/blob/b882c60b4023bdf09264c2d5d30a2cadebc240fb/staging/src/k8s.io/client-go/tools/clientcmd/api/types.go#L68-L105))
and identity material in `AuthInfo`
([authentication fields](https://github.com/kubernetes/kubernetes/blob/b882c60b4023bdf09264c2d5d30a2cadebc240fb/staging/src/k8s.io/client-go/tools/clientcmd/api/types.go#L108-L159)).
Continue represents secret locations as a typed union with user, organization,
package, local-environment, process-environment, and not-found variants
([secret location types](https://github.com/continuedev/continue/blob/5522c6f44ca0ac3528b37244818fbfa39b5af470/packages/config-yaml/src/interfaces/SecretResult.ts#L3-L64)).
Its client leaves non-user secrets as location references and validates the
rendered configuration again
([late resolution](https://github.com/continuedev/continue/blob/5522c6f44ca0ac3528b37244818fbfa39b5af470/packages/config-yaml/src/load/clientRender.ts#L20-L69)).

Continue's model schema also shows the coupling Nexa should avoid: `apiKey` and
`apiBase` live beside model identity and roles
([model fields](https://github.com/continuedev/continue/blob/5522c6f44ca0ac3528b37244818fbfa39b5af470/packages/config-yaml/src/schemas/models.ts#L178-L205)).
Nexa should adopt typed references and late resolution, not colocated secrets.

### Connection record

The persisted record should be equivalent to:

```rust
struct ConnectionRecord {
    schema_version: u16,
    id: ConnectionId,
    revision: u64,
    provider_definition_id: ProviderDefinitionId,
    endpoint_contract_id: Option<EndpointId>,
    endpoint_url: SanitizedUrl,
    endpoint_fingerprint: String,
    region: Option<String>,
    organization: Option<String>,
    project: Option<String>,
    credential_ref: Option<CredentialRef>,
    enabled: bool,
    health: ConnectionHealth,
    last_tested_at: Option<Timestamp>,
    created_at: Timestamp,
    updated_at: Timestamp,
}
```

`CredentialRef` is a closed, namespaced type resolved only by the backend
credential broker. PR 9 should add the durable registry namespace while
retaining the current `legacy-agent-config:` and `legacy-app-config:` readers
for rollback. The UI receives only `configured/missing/invalid/expired`, never
the reference's resolved value.

“Configure once” means one Connection can be reused across compatible Models
and Agent Capabilities. It does not mean one Connection per provider forever:
separate regions, organizations, projects, endpoints, accounts, or auth plans
must remain separate records.

### Endpoint and credential invariants

- No implicit credential reuse for HTTP, non-default ports, URL userinfo,
  query/fragment, unknown endpoints, or user-edited endpoints.
- Known endpoint reuse requires exact sanitized endpoint identity plus the same
  account scope. A provider label alone is insufficient.
- Local endpoints are explicit local-trust Connections and default to no
  credential. They are not silently classified as a cloud provider.
- Token Plan/Coding Plan and other endpoint-scoped credentials remain separate
  even when the vendor is the same.
- Changing endpoint, region, org/project, auth kind, or credential reference is
  a new Connection revision and invalidates discovery, probe, and availability
  evidence.
- Test Connection is a bounded backend operation. It redacts headers and bodies,
  follows no cross-origin redirect with credentials, and writes only classified
  status plus timing/error category.

## 3. One model descriptor, separate availability evidence

### Primary-source evidence

LiteLLM's model map spans chat, embeddings, image generation, transcription,
speech, reranking, and search in one `mode` field and records limits, regions,
reasoning, vision, audio, tool calling, structured output, and lifecycle data
([sample specification](https://github.com/BerriAI/litellm/blob/b66d4e6965c797163e03e95de59bc23d9d62d4e7/model_prices_and_context_window.json#L1-L40)).
That supports a unified descriptor, but its runtime still determines supported
parameters from the selected provider adapter and request type
([runtime parameter dispatch](https://github.com/BerriAI/litellm/blob/b66d4e6965c797163e03e95de59bc23d9d62d4e7/litellm/litellm_core_utils/get_supported_openai_params.py#L8-L59)).

Therefore catalog claims and executable runtime support are separate evidence.
Nexa's existing descriptor and readiness ladder already express this correctly.

### Descriptor and target state

Keep `ModelDescriptor` as the cross-modality contract and add provenance per
field or per source revision. Required query dimensions are:

- input/output modalities: text, image, audio, video, file, embedding;
- operations: chat/reasoning, image generation/editing, video generation,
  transcription, speech synthesis, embedding, reranking;
- tool/parallel-tool/structured-output/streaming/realtime support;
- context/output/media limits and provider-specific option schema;
- price reference and latency class, both timestamped rather than timeless;
- lifecycle, access gate, regions, source, last verification, and replacement;
- runtime adapter operation and minimum adapter version.

Persist account-specific state on `ModelTarget`, not `ModelDescriptor`:

```text
unknown -> unavailable | discoverable -> callable -> product_ready
```

`product_ready` is not learned from a provider `/models` response. It requires:

1. a curated/official descriptor with a supported schema version;
2. a registered adapter for the exact operation and endpoint contract;
3. a successful contract probe or deterministic local capability check;
4. the required product UI and input/output handling;
5. green contract, integration, and end-to-end fixtures.

Discovery may add an unknown upstream ID as `Discoverable` with conservative
capabilities. A probe may promote it to `Callable`. Only checked-in release
metadata may mark it `ProductReady`.

### Catalog ingestion and precedence

Use deterministic precedence by field, not whole-object replacement:

1. checked-in tombstones and security blocks always win;
2. checked-in Nexa product-readiness assertions win for readiness;
3. official metadata wins for official limits/lifecycle when fresh and valid;
4. curated metadata fills missing fields;
5. discovery changes only connection-scoped availability and unknown identity;
6. probes change only the facts they tested.

Each snapshot records source URI, immutable/source revision when available,
fetched time, schema version, content hash, connection revision, and validation
result. Unknown fields are bounded and preserved for forward compatibility;
unknown enum values do not automatically become supported capabilities.

LiteLLM validates that a fetched catalog is a non-empty dictionary and rejects
large count reductions as possible corruption
([integrity checks](https://github.com/BerriAI/litellm/blob/b66d4e6965c797163e03e95de59bc23d9d62d4e7/litellm/litellm_core_utils/get_model_cost_map.py#L71-L155));
it retains a bundled local backup
([bundled fallback](https://github.com/BerriAI/litellm/blob/b66d4e6965c797163e03e95de59bc23d9d62d4e7/litellm/litellm_core_utils/get_model_cost_map.py#L44-L69)).
Nexa should additionally impose byte/model/alias limits, schema validation,
per-provider shrink thresholds, signature or pinned-origin policy for remote
curation, and last-known-good rollback. A failed refresh must not empty the UI
or delete a selected target.

## 4. Capability bindings are typed routing policy

### Primary-source evidence

Continue separates model roles such as chat, autocomplete, embedding, rerank,
edit, summarize, and subagent
([roles](https://github.com/continuedev/continue/blob/5522c6f44ca0ac3528b37244818fbfa39b5af470/packages/config-yaml/src/schemas/models.ts#L23-L33))
from lower-level capabilities such as tool use and image input
([capabilities](https://github.com/continuedev/continue/blob/5522c6f44ca0ac3528b37244818fbfa39b5af470/packages/config-yaml/src/schemas/models.ts#L35-L45)).
This is a useful distinction: an operation requested by the product is not the
same thing as a model feature advertised by a catalog.

### Nexa capability taxonomy

Use stable capability IDs and a typed requirement predicate:

| Capability ID | Required model operation/evidence |
| --- | --- |
| `reasoning` | text input/output, reasoning contract, chat runtime |
| `vision` | image/file input plus text output, vision-capable chat runtime |
| `image_generation` | image output and image-generation adapter |
| `image_editing` | image input/output and edit adapter; multi-reference is a separate option gate |
| `video_generation` | video output, async-job runtime, asset handling |
| `speech_to_text` | audio input/text output and transcription adapter |
| `text_to_speech` | text input/audio output and speech adapter |
| `embedding` | embedding output and dimension contract |
| `reranking` | rerank operation and scored-list contract |

Web research, file reading/editing, Subagents, and Computer use remain tool or
permission capabilities. They may require a compatible chat model (for example
tool calling or vision), but they must not be disguised as provider models.

### Binding contract

Evolve `CapabilityBindingV2` compatibly toward:

```rust
struct CapabilityBindingV2 {
    primary: Option<ModelTargetRef>,
    fallbacks: Vec<FallbackTarget>,
    fallback_mode: FallbackMode, // disabled, ask, automatic
    constraints: BindingConstraints,
}

struct BindingConstraints {
    allowed_regions: Vec<String>,
    allow_cross_provider: bool,
    allow_cross_region: bool,
    max_cost_class: Option<String>,
    requires_streaming: bool,
    data_classes: Vec<DataClass>,
}
```

Fallback order is user-authored and stable. Deduplicate target IDs and reject a
primary repeated as fallback. Automatic fallback is allowed only when the
target satisfies the same operation contract and all data, region, cost, and
permission constraints. Crossing a provider, account, region, or local/cloud
boundary defaults to `ask`, because it changes data egress and billing.

The resolver returns both a decision and evidence:

```text
binding revision
  -> target and connection revisions
  -> descriptor compatibility
  -> adapter operation readiness
  -> credential/connection status
  -> policy and data-egress decision
  -> availability freshness
  -> selected target or ordered rejection reasons
```

Persist the selected target ID, connection revision, descriptor revision/hash,
binding revision, and fallback reason in run provenance. A mid-run settings or
Connection edit applies to the next turn unless the current runtime explicitly
supports safe rebinding.

## 5. Persistence and service boundary

PR 9 should introduce normalized, secret-free registry storage. Exact names may
follow repository migration conventions, but ownership should be equivalent to:

| Store | Key | Important constraints |
| --- | --- | --- |
| `provider_connections` | `connection_id` | unique revision; sanitized endpoint; credential ref only; soft-disable rather than cascading delete |
| `model_definitions` | provider plus canonical model ID | descriptor schema/hash/provenance; reserved provider namespace; bounded payload |
| `model_targets` | connection plus upstream model ID | FK to Connection and Model Definition; per-connection lifecycle/availability |
| `model_catalog_snapshots` | source/connection/revision | validated immutable payload metadata; last-known-good pointer; no response headers or secrets |
| `registry_activation_state` | capability/scope | legacy/registry mode, parity counters, activated revision, rollback metadata |

Capability bindings remain inside versioned Settings profiles so inheritance,
presets, revision checks, and Reset-to-inherit have one owner. The registry
service resolves and validates those references; it must not duplicate binding
configuration in another writable table.

All writes use transactions and optimistic revision checks. Deleting a
Connection is initially a reversible disable if a binding, run, usage record,
or snapshot references it. A separate confirmed cleanup may remove unreachable
history after the rollback window.

Expose one backend API surface:

```text
list/test/create/update/disable connections
list/refresh model definitions and targets
validate/resolve capability binding
explain target eligibility
get registry activation/parity status
activate/rollback registry reads
```

Frontend code must not independently merge catalogs, infer provider identity,
or resolve secrets. It renders backend projections and submits stable IDs plus
expected revisions.

## 6. Runtime activation and rollback

Settings V2 currently preserves legacy rows as the runtime source. PR 9 should
change that deliberately in six phases:

1. **Schema only.** Add registry tables and integrity constraints. Leave the
   runtime pointer on legacy.
2. **Deterministic import.** Convert every legacy/Settings V2 provider,
   endpoint, credential reference, and model triple into Connections and Model
   Targets. Exact known endpoints map to built-ins; all others receive opaque
   custom identities. Record source fingerprints and import errors.
3. **Shadow resolution.** For every eligible turn, resolve both legacy and
   registry paths without issuing a second provider request. Compare provider,
   sanitized endpoint identity, credential reference, model, and operation.
4. **Read-only UI.** Show Connections, unified Models, and binding eligibility
   from the registry while edits still dual-write through one backend
   transaction.
5. **Capability-scoped activation.** Flip a durable pointer only after parity
   fixtures and runtime matrices pass for that capability. Newly created
   settings write registry IDs; the legacy representation remains rollback
   material.
6. **Rollback window.** A rollback flips reads to legacy without deleting
   registry data. Removal of legacy writes/storage requires a later PR and
   evidence that the supported previous app version can still open the data.

Do not use a process-only feature flag as the sole activation record. A crash or
restart must reopen the same mode. Import and activation are idempotent; a
partial import cannot advance the pointer.

### Parity definition

A shadow comparison is equal only when it resolves the same:

- invocation provider and adapter operation;
- exact endpoint identity, region, and account discriminator;
- credential reference namespace and identifier, never secret value;
- upstream model/deployment ID;
- advanced parameters after defaults;
- availability/readiness decision and fallback behavior.

Differences are classified and redacted. Credential or endpoint-boundary
differences are release-blocking, not accepted noise.

## 7. Threat and credential boundaries

| Threat | Required control |
| --- | --- |
| Credential confused deputy | Exact endpoint/account-scoped credential lookup; no provider-label, alias, suffix, or model-based inheritance |
| SSRF through custom endpoint or discovery | Parse and canonicalize once; reject userinfo/query/fragment credentials; block disallowed schemes; require explicit local/private-network trust; revalidate redirects and resolved destinations |
| Secret leakage in test/discovery | Resolve secrets immediately before backend request; mark headers sensitive; redact URL/query/body/error/event data; persist only classified result |
| Catalog poisoning or truncation | Schema/size/count validation, reserved-ID protection, source allowlist/signature policy, last-known-good snapshot, tombstones, no automatic ProductReady promotion |
| Alias/provider collision | Namespaced immutable built-ins; provider-scoped aliases; deterministic ambiguity failure; custom IDs cannot shadow built-ins |
| Cross-provider fallback exfiltration | Default to ask/deny across provider, account, region, or local/cloud boundary; re-evaluate policy and data class for every fallback |
| TOCTOU during connection edits | Optimistic revisions; pin connection/descriptor/binding revisions per run/turn; invalidate probes on security-relevant edits |
| Malicious display metadata | Treat names/descriptions/pricing refs as untrusted text; no HTML execution, shell interpolation, or automatic URL opening |
| Availability spoofing | Discovery proves listing only; capability probes prove only the tested operation; runtime adapter registration remains mandatory |
| Destructive deletion | Disable first; show inbound bindings; require explicit confirmation; preserve rollback and run provenance |

The Connection Registry is not the credential vault. Its backups, diagnostics,
analytics, and IPC types must remain safe to export without decrypting secrets.

## 8. Settings information architecture

The UI should reveal the new ownership model directly:

- **Connections:** account name, endpoint/region/org/project, credential status,
  Test Connection, availability/rate/billing summary, edit/disable/delete. No
  model tuning, tools, or Agent permissions.
- **Models:** one filterable directory across all modalities. Cards show
  capabilities, limits, price/latency provenance and age, lifecycle, readiness,
  and available Connections. Editing a Connection is a linked action, not an
  inline credential field.
- **Agent Capabilities:** primary and ordered fallback target for each capability,
  compatibility explanations, inheritance origin, Reset to inherited, and
  cross-boundary fallback warnings.
- **Permissions:** unchanged policy domain for file, shell, web, connectors,
  desktop automation, writing, destructive actions, and delegation.
- **Advanced:** model-generation/runtime options, collapsed by default and
  validated against the selected target's adapter contract.

Keep unsupported models visible with a precise reason (`missing credential`,
`not available to account`, `adapter missing`, `preview`, `deprecated`,
`operation unsupported`) instead of silently dropping them. Never present
`Known` or `Discoverable` as callable.

## 9. Required tests and release gates

### Schema and identity

- Reject duplicate/reserved provider IDs and cross-provider alias collisions.
- Canonical endpoint tests cover case, trailing slash, default/non-default port,
  Unicode host, path, region, org/project, invalid URL, userinfo, query, and
  fragment.
- The same account/endpoint imports idempotently; different account, region,
  plan, or endpoint never collapses.
- Model Definition and Target identities remain stable across restart and
  catalog refresh; gateway upstream attribution does not alter invocation
  identity.
- Foreign keys, revision compare-and-swap, disable/delete references, and
  transaction rollback are exercised.

### Credential security

- Registry rows, IPC responses, logs, events, snapshots, exports, and error
  messages are scanned to prove secret values never appear.
- Unknown, HTTP, non-standard-port, user-edited, and lookalike endpoints do not
  inherit trusted credentials.
- Redirects cannot forward credentials across origin; private-network access
  requires an explicit local/custom trust decision.
- Missing/expired/wrong credentials yield classified state and cannot be
  mistaken for model incompatibility.

### Catalog and readiness

- Every entry from text, vision, image, video, STT, TTS, embedding, and
  reranking fixtures validates against the unified schema; missing/malformed
  descriptors fail closed.
- Merge tests cover source precedence, stale metadata, tombstones, aliases,
  deprecation/replacement, connection-specific discovery, probe failure, and
  offline last-known-good behavior.
- Empty, oversized, truncated, drastically smaller, wrong-schema, duplicate,
  and reserved-ID remote catalogs cannot replace a good snapshot.
- Discovery alone yields at most `Discoverable`; successful operation probe
  yields at most `Callable`; only checked-in readiness evidence yields
  `ProductReady`.
- Runtime adapter matrices verify each claimed operation in default and
  feature-gated builds. Catalog presence alone never passes the gate.

### Binding and runtime

- Compatibility predicates reject wrong modality, operation, adapter, region,
  lifecycle, access, and stale availability with stable reason codes.
- Primary/fallback order, duplicate rejection, disabled fallback, ask mode,
  automatic same-boundary fallback, and cross-provider/region/account consent
  are covered.
- App -> Workspace -> Agent -> Task inheritance preserves per-field provenance;
  Reset deletes the override and reveals the inherited binding.
- One Connection is reused by multiple model targets and Agent capabilities
  without duplicating credentials.
- A run pins target/connection/descriptor/binding revisions; concurrent edits
  fail with revision conflict or apply only to the next safe boundary.
- Usage/billing provenance records invocation provider, upstream provider when
  known, connection ID, target ID, fallback reason, and operation without a
  credential reference.

### Migration, rollback, and UI

- Golden fixtures cover every current provider and all five existing shared
  catalog surfaces, plus custom endpoints, Qwen plan separation, disabled media
  settings, missing credentials, deprecated selections, and malformed legacy
  rows.
- Legacy and registry shadow resolution is identical on all security-sensitive
  fields before activation. Deliberate mismatches are classified and block the
  switch.
- Fault injection at every import/activation transaction boundary proves a
  partial migration cannot activate.
- Repeated migrate/activate/rollback/restart cycles preserve legacy rows,
  Settings V2 revisions, selections, and secret references.
- Focused desktop tests cover Connections, unified Models, Agent Capability
  primary/fallback selection, inherited provenance, Test Connection states,
  unavailable/deprecated explanations, and reduced motion.

## 10. PR 9 exit boundary

PR 9 is complete only when:

1. existing credentials are represented once by stable references and reused
   through Connections without changing their secret-storage owner;
2. one backend registry projection serves text, vision, image, video, STT,
   TTS, embedding, and reranking metadata, even where later runtime work keeps a
   modality non-callable;
3. every model-backed Agent capability has a validated primary and ordered
   fallback contract, including explicit cross-boundary behavior;
4. registry resolution is the active runtime source for the capabilities
   declared in the PR, with shadow parity evidence and durable rollback;
5. the settings UI separates Connections, Models, Agent Capabilities,
   Permissions, and Advanced responsibilities;
6. no unknown/custom endpoint inherits a trusted credential, no catalog or
   discovery result self-promotes to product readiness, and no P1 issue remains
   after focused review and CI.

## Non-goals

- Implementing the Vision Router or `VisionObservation` contract (PR 10).
- Rewriting STT transport/spooling or adding AudioWorklet recording (PRs 11-12).
- Implementing media jobs, asset lineage, video adapters, or video editing UI
  (PRs 13-16). Video is schema-visible but not falsely product-ready.
- Migrating secret bytes into a new vault, inventing cloud secret sync, or
  exposing credential values to the frontend.
- Automatically downloading and trusting an unpinned third-party catalog.
- Replacing provider adapters with a universal OpenAI-compatible assumption.
- Removing legacy settings/catalog readers before the rollback window closes.
- Moving tool permissions, approvals, or Agent budgets into model descriptors.
- Copying upstream catalogs, provider code, or configuration formats.

## License and reuse boundary

| Project | License at reviewed revision | Nexa use in this PR |
| --- | --- | --- |
| OpenAI Codex | [Apache-2.0](https://github.com/openai/codex/blob/1151b23f01accb19e55c090a3349a32fdf2b4685/LICENSE) | Architectural comparison only: registry references, separate credential store, reserved IDs |
| Continue | [Apache-2.0](https://github.com/continuedev/continue/blob/5522c6f44ca0ac3528b37244818fbfa39b5af470/LICENSE) | Architectural comparison only: roles/capabilities and typed secret locations |
| LiteLLM | [MIT outside separately licensed restricted directories](https://github.com/BerriAI/litellm/blob/b66d4e6965c797163e03e95de59bc23d9d62d4e7/LICENSE) | Architectural comparison only: unified metadata, adapter checks, catalog integrity fallback |
| Kubernetes | [Apache-2.0](https://github.com/kubernetes/kubernetes/blob/b882c60b4023bdf09264c2d5d30a2cadebc240fb/LICENSE) | Architectural comparison only: endpoint/auth/context separation |

This note does not authorize copying source, schema text, test fixtures, or
catalog records. If implementation later copies or closely adapts upstream
material, it must preserve the applicable notices and receive a separate
license review. An independent implementation of the decisions above does not
require adding these projects as runtime dependencies.
