# Runtime performance, delegation, overlays, and long-session UI: primary-source notes

Date: 2026-08-02

This note validates the upgrade directions in `D:\TODO.txt` against first-party documentation and immutable source snapshots. It is an engineering input, not an implementation record. “Recommended” statements below are design inferences for Nexa; factual statements about libraries and protocols link directly to their owning project, specification, or source.

## Executive decision

1. **Pool transports, not provider instances.** A reused `reqwest::Client` already owns a shared connection pool and is internally `Arc`-backed. Nexa should cache Clients by transport-affecting configuration and keep credentials/provider state above that layer. For normal HTTPS endpoints, allow ALPN to negotiate HTTP/2 or HTTP/1.1; retain a separate HTTP/1.1 profile for origins proven unhealthy over HTTP/2. [reqwest 0.12.28 Client contract](https://github.com/seanmonstar/reqwest/blob/d97859910c357827ad5993d37ce750ad595f4fff/src/async_impl/client.rs#L74-L94), [reqwest 0.12.28 ALPN setup](https://github.com/seanmonstar/reqwest/blob/d97859910c357827ad5993d37ce750ad595f4fff/src/async_impl/client.rs#L824-L842)
2. **Make queueing, connection, first-byte, idle, and whole-run deadlines separate.** A Tokio semaphore bounds concurrency but does not bound queue time. Cancellation and shutdown also need explicit ownership: a parent `CancellationToken`, child tokens per batch/worker, and every spawned worker/collector in a `JoinSet` that is drained within a grace window. [Tokio semaphore behavior](https://github.com/tokio-rs/tokio/blob/be689a35f5ade5a39e507f79d3ec85cdab27806f/tokio/src/sync/semaphore.rs#L14-L24), [Tokio `JoinSet::shutdown`](https://github.com/tokio-rs/tokio/blob/be689a35f5ade5a39e507f79d3ec85cdab27806f/tokio/src/task/join_set.rs#L371-L383)
3. **A batch result must be a stream of durable worker outcomes, not one future containing a final vector.** LangGraph runs independent tasks concurrently, yields after completions, commits each task, and cancels remaining work on an unhandled failure or deadline. OpenAI Agents similarly uses bounded concurrency, `FIRST_COMPLETED`, sibling cancellation, and a bounded settlement window. Nexa should copy those lifecycle properties while adding explicit `All`, `Quorum`, `FirstSuccess`, and deadline completion policies. [LangGraph runner](https://github.com/langchain-ai/langgraph/blob/b2926a0ff9589c28c7e01fe7cdbb337b86d5a4b4/libs/langgraph/langgraph/pregel/_runner.py#L258-L344), [OpenAI Agents tool execution](https://github.com/openai/openai-agents-python/blob/87425fae1c2a9a4327686f1fa36eef2aabffdc1d/src/agents/run_internal/tool_execution.py#L1492-L1564)
4. **Use the right overlay primitive for the interaction.** Radix Select is suitable for a small fixed choice set; searchable model/provider/skill pickers should be a Popover plus `cmdk`, with Floating UI/Radix collision handling. `cmdk` is accessible but explicitly does not virtualize, so large result sets require `shouldFilter={false}` plus a separate virtualizer and careful active-option mounting. [Radix Select source](https://github.com/radix-ui/primitives/blob/f7ecd5ab16f5e1e820eb5786a1419a98a2d594ae/packages/react/select/src/select.tsx#L322-L398), [`cmdk` 1.1.1 FAQ](https://github.com/pacocoursey/cmdk/blob/fb4ea04e9ec211777fbb39c6104e3c5f2ee107d2/README.md#L432-L436)
5. **Prefer TanStack Virtual for the chat transcript.** Its current first-party chat contract includes end anchoring, stable prepends, following appends only when already pinned, and dynamic measurement while the last streaming row grows. `react-window` remains a viable simpler list, but its own API warns that dynamic row heights are less efficient and it does not expose the same chat-specific end-anchor contract. [TanStack Virtual chat guide source](https://github.com/TanStack/virtual/blob/d2cf98beea1696c7187c06b57c9e724d1957963c/docs/chat.md#L5-L18), [`react-window` source documentation](https://github.com/bvaughn/react-window/blob/4d9eebbb510262b3b7e95463cf49a10de53ea77d/README.md#L67-L81)
6. **Coalesce stream events before publishing React snapshots.** React batching does not make a high-frequency external stream free, and React 19.2.8's exported `unstable_batchedUpdates` is a passthrough. Nexa should reduce event-to-snapshot frequency at the store boundary, publish at most once per animation frame for ordinary deltas, preserve immediate terminal/approval/error delivery, and keep immutable snapshots referentially stable. [React 19.2.8 source](https://github.com/facebook/react/blob/1dd4ecbdabf826f527fc9a58c05ea70375b7d170/packages/react-dom/src/shared/ReactDOM.js#L51-L74), [React `useSyncExternalStore` contract](https://react.dev/reference/react/useSyncExternalStore#im-getting-an-error-the-result-of-getsnapshot-should-be-cached)

## 1. Repository baseline relevant to these decisions

The desktop core declares `reqwest 0.12` with HTTP/2 enabled, Tokio 1 with full features, and `tokio-util 0.7`; the frontend declares `cmdk 1.1.1` and React/React DOM 19.2.8. [Nexa Rust dependencies](https://github.com/MLGBJDLW/Nexa/blob/cd70bd05c497a54c931c94475aa3774f15c7eb96/crates/core/Cargo.toml#L32-L35), [Nexa frontend dependencies](https://github.com/MLGBJDLW/Nexa/blob/cd70bd05c497a54c931c94475aa3774f15c7eb96/apps/desktop/package.json#L34-L46)

The OpenAI, Anthropic, and Google adapters each construct a Client with a 15-second idle timeout, five idle connections per host, and `http1_only()`. This preserves the documented workaround for stale HTTP/2 SSE streams, but it also prevents ALPN-negotiated HTTP/2 and makes pool lifetime depend on each provider object's lifetime. [OpenAI adapter](https://github.com/MLGBJDLW/Nexa/blob/cd70bd05c497a54c931c94475aa3774f15c7eb96/crates/core/src/llm/openai.rs#L931-L946), [Anthropic adapter](https://github.com/MLGBJDLW/Nexa/blob/cd70bd05c497a54c931c94475aa3774f15c7eb96/crates/core/src/llm/anthropic.rs#L828-L843), [Google adapter](https://github.com/MLGBJDLW/Nexa/blob/cd70bd05c497a54c931c94475aa3774f15c7eb96/crates/core/src/llm/google.rs#L1144-L1159)

The current delegation budget increments `calls_started` and reserves tokens before awaiting `Semaphore::acquire_owned`; the wait races only parent cancellation, not a queue deadline. Later, worker errors are not captured as a distinct event, the collector is awaited without a bound, and `buffer_unordered(...).collect().await` returns the batch only after every worker resolves. [Nexa admission and queue wait](https://github.com/MLGBJDLW/Nexa/blob/cd70bd05c497a54c931c94475aa3774f15c7eb96/apps/desktop/src-tauri/src/subagent_tool.rs#L519-L581), [Nexa worker event collection](https://github.com/MLGBJDLW/Nexa/blob/cd70bd05c497a54c931c94475aa3774f15c7eb96/apps/desktop/src-tauri/src/subagent_tool.rs#L1819-L1878), [Nexa batch collection](https://github.com/MLGBJDLW/Nexa/blob/cd70bd05c497a54c931c94475aa3774f15c7eb96/apps/desktop/src-tauri/src/subagent_tool.rs#L2558-L2577)

The stream store currently coalesces notifications with `queueMicrotask`, while `getStream` constructs a new wrapper snapshot; chat session caches are unbounded records keyed by conversation ID. A microtask only coalesces mutations occurring before that microtask runs, so separate backend callbacks can still publish more often than the display refresh. [Nexa stream-store publication](https://github.com/MLGBJDLW/Nexa/blob/cd70bd05c497a54c931c94475aa3774f15c7eb96/apps/desktop/src/lib/streamStore.ts#L45-L105), [Nexa conversation caches](https://github.com/MLGBJDLW/Nexa/blob/cd70bd05c497a54c931c94475aa3774f15c7eb96/apps/desktop/src/lib/useChatSession.ts#L351-L363)

## 2. HTTP transport pooling and HTTP/2 fallback

### 2.1 What reqwest actually guarantees

`reqwest::Client` owns its connection pool, recommends creating one Client and reusing it, is cheap to clone, and already stores its internals behind an `Arc`. A second “pool of borrowed Clients” is therefore unnecessary for identical transport configuration; the useful cache is a map from transport configuration to long-lived Client. [reqwest Client source](https://github.com/seanmonstar/reqwest/blob/d97859910c357827ad5993d37ce750ad595f4fff/src/async_impl/client.rs#L74-L94)

In reqwest 0.12.28, the builder defaults are a 90-second idle timeout, no finite `pool_max_idle_per_host` limit, and `HttpVersionPref::All`. Nexa currently overrides these to 15 seconds, five, and HTTP/1 only on its main cloud-provider paths. [reqwest defaults](https://github.com/seanmonstar/reqwest/blob/d97859910c357827ad5993d37ce750ad595f4fff/src/async_impl/client.rs#L297-L311), [reqwest default protocol preference](https://github.com/seanmonstar/reqwest/blob/d97859910c357827ad5993d37ce750ad595f4fff/src/async_impl/client.rs#L332-L346)

With rustls and `HttpVersionPref::All`, reqwest advertises `h2` followed by `http/1.1` through ALPN. `http1_only()` selects only HTTP/1, while `http2_prior_knowledge()` selects only HTTP/2. For ordinary HTTPS provider endpoints, “try H2 and negotiate H1” is therefore the default `All` profile; `http2_prior_knowledge()` is not the fallback mechanism. [reqwest ALPN source](https://github.com/seanmonstar/reqwest/blob/d97859910c357827ad5993d37ce750ad595f4fff/src/async_impl/client.rs#L824-L842), [reqwest forced-protocol methods](https://github.com/seanmonstar/reqwest/blob/d97859910c357827ad5993d37ce750ad595f4fff/src/async_impl/client.rs#L1545-L1562)

HTTP/2 distinguishes retry-safe failures precisely. RFC 9113 says a `REFUSED_STREAM` guarantees no application processing and can be retried even for a non-idempotent method; a generic reset does not provide that guarantee. This matters for LLM POST requests because blindly replaying after partial server processing can duplicate generation or billing. [RFC 9113 request reliability](https://www.rfc-editor.org/rfc/rfc9113.html#section-8.7), [RFC 9113 error codes](https://www.rfc-editor.org/rfc/rfc9113.html#section-7)

### 2.2 Recommended Nexa transport model

Use two layers:

```rust
struct TransportPoolKey {
    proxy_profile: ProxyProfileId,
    tls_profile: TlsProfileId,       // roots, client cert, SNI policy
    protocol_profile: ProtocolProfile, // AutoAlpn | Http1Only
    network_profile: NetworkProfileId, // bind address / resolver where applicable
}

struct EndpointHealthKey {
    origin: Origin,
    proxy_profile: ProxyProfileId,
}

struct ProviderSession {
    transport: reqwest::Client,
    endpoint: Url,
    credential: SecretHandle,
    request_defaults: ProviderRequestDefaults,
}
```

This split is an inference from the Client-level configuration above. Do **not** include a bearer credential fingerprint in `TransportPoolKey` when authorization is attached per request: doing so fragments otherwise reusable connections. Include credential identity only when it changes transport state, such as mTLS identity, a credential-bound proxy, or credential-bearing default headers. Provider sessions may remain credential-specific while sharing the transport.

Recommended protocol policy:

- Start official HTTPS origins in `AutoAlpn`; that profile can negotiate H2 or H1 on connection establishment.
- Maintain origin-scoped health counters for H2 stream reset, first-byte failure, connection failure, and successes. A threshold moves the origin to `Http1Only` for a bounded cool-down, not forever.
- Never mutate a Client's protocol mode in place; select a different pooled Client because protocol preference and its connections belong to the Client configuration.
- Retry the same POST automatically only when the transport can prove it was not processed (`REFUSED_STREAM` or the RFC-defined GOAWAY stream-number case), or when the provider offers an idempotency key with matching semantics. Otherwise fail the attempt, downgrade the **next** request, and surface the precise reason.
- Record `connection_reused`, negotiated HTTP version, connect time, first-byte time, stream reset class, and downgrade state by origin. Promote back to `AutoAlpn` only through a half-open probe or successful bounded trial.
- Tune idle lifetime from measurements. A universal 15-second idle timeout is shorter than normal human think time and defeats warm reuse; a universal unlimited pool is also not a concurrency policy. Keep the provider concurrency governor separate from idle socket capacity.

### 2.3 Transport acceptance tests

- Reusing a provider/session and cloning a Client must reuse a warm connection in an instrumented local H1 server test.
- Six concurrent requests to an H2 test origin must negotiate H2 and share a transport connection while respecting the provider governor.
- An H1-only TLS origin must succeed from `AutoAlpn` without a manual retry.
- Inject `REFUSED_STREAM`, `HTTP_1_1_REQUIRED`, a generic `CANCEL`, and a reset after response bytes. Only the spec-proven unprocessed cases may transparently replay.
- After the downgrade threshold, new work must select `Http1Only`; after cool-down, exactly one half-open trial should probe H2.
- Credential rotation that changes only an Authorization header must not discard the transport. Proxy, client-certificate, root-store, or protocol changes must select a different Client.

## 3. Tokio concurrency, deadlines, cancellation, and bounded shutdown

### 3.1 Primitive behavior that constrains the scheduler

Tokio's semaphore is fair: permits are assigned in request order. A large `acquire_many` at the front can block smaller acquisitions behind it. Cancelling `acquire_owned` loses that waiter's place in the queue, which is acceptable for an expired queue deadline but means retries must re-enter admission rather than reuse an assumed position. [Tokio semaphore fairness](https://github.com/tokio-rs/tokio/blob/be689a35f5ade5a39e507f79d3ec85cdab27806f/tokio/src/sync/semaphore.rs#L14-L24), [`acquire_owned` cancellation behavior](https://github.com/tokio-rs/tokio/blob/be689a35f5ade5a39e507f79d3ec85cdab27806f/tokio/src/sync/semaphore.rs#L755-L765)

Closing a semaphore prevents new permits and notifies all pending waiters. This should be part of scheduler shutdown so queued workers do not depend solely on every caller observing a separate token. [Tokio `Semaphore::close`](https://github.com/tokio-rs/tokio/blob/be689a35f5ade5a39e507f79d3ec85cdab27806f/tokio/src/sync/semaphore.rs#L963-L965)

`tokio::select!` returns when the first branch completes and drops the others. Cancellation correctness belongs to the branch future: channel `recv` and stream `next` are cancellation-safe, while semaphore acquisition deliberately loses queue position and several full-buffer I/O helpers are not cancellation-safe. [Tokio `select!` behavior](https://github.com/tokio-rs/tokio/blob/be689a35f5ade5a39e507f79d3ec85cdab27806f/tokio/src/macros/select.rs#L3-L16), [Tokio cancellation-safety list](https://github.com/tokio-rs/tokio/blob/be689a35f5ade5a39e507f79d3ec85cdab27806f/tokio/src/macros/select.rs#L90-L133)

`tokio::time::timeout` cancels by dropping the wrapped future, and a future that does not yield can run past the deadline. Deadlines therefore require cooperative async code and cannot by themselves stop blocking work. [Tokio timeout contract](https://github.com/tokio-rs/tokio/blob/be689a35f5ade5a39e507f79d3ec85cdab27806f/tokio/src/time/timeout.rs#L18-L42)

A `CancellationToken` parent cancels all child tokens, while cancelling one child does not cancel its parent. This maps naturally to `turn -> batch -> worker -> collector/tool` ownership. [tokio-util 0.7.18 cancellation tree](https://github.com/tokio-rs/tokio/blob/9cc02cc88d083113cd9889a74b382e39e430e180/tokio-util/src/sync/cancellation_token.rs#L145-L201)

`JoinSet` returns tasks in completion order, aborts contained tasks when dropped, and its `shutdown` aborts all tasks then joins until empty. A raw detached collector handle does not provide this structured ownership. [Tokio `JoinSet` contract](https://github.com/tokio-rs/tokio/blob/be689a35f5ade5a39e507f79d3ec85cdab27806f/tokio/src/task/join_set.rs#L18-L25), [`JoinSet::shutdown`](https://github.com/tokio-rs/tokio/blob/be689a35f5ade5a39e507f79d3ec85cdab27806f/tokio/src/task/join_set.rs#L371-L383)

### 3.2 Recommended worker lifecycle

```text
validated
  -> queued(queue_deadline)
  -> admitted(permit + initial credit)
  -> connecting(connect_deadline)
  -> waiting_first_byte(first_byte_deadline)
  -> running(stream_idle_deadline + run_deadline)
  -> draining(shutdown_grace)
  -> completed | failed | timed_out | cancelled
```

Admission should be two-phase:

1. Validate model, role, tools, source scope, and call-count policy; enqueue without consuming the full output credit.
2. Race `queue_deadline`, batch/turn cancellation, scheduler close, and `acquire_owned`. Only after the permit is acquired should Nexa increment the started counter and reserve the initial token credit. This prevents queued work from starving runnable work of the budget.

Use one monotonic absolute batch deadline and derive remaining durations at every phase. Independent phase ceilings may be smaller, but retries must not reset the batch deadline. For example, a retry gets `min(connect_ceiling, batch_deadline - now)`, not a fresh full timeout.

Every spawned worker, event bridge, and collector belongs to one `JoinSet`. On a terminal worker outcome:

1. close that worker's event sender;
2. persist its final status/result/error/usage;
3. cancel its child token;
4. wait a small grace interval for collectors to flush;
5. abort and join leftovers;
6. release permit and budget credit in a guard/finalizer path.

Do not await `event_task` without a deadline. `JoinSet::shutdown()` can itself be wrapped in a short timeout; dropping the set remains the final abort safety net. Blocking tools must run in an explicitly governed blocking lane because cooperative async cancellation cannot stop code that never yields.

## 4. Mature orchestration patterns and the Nexa scheduler contract

### 4.1 Patterns verified in source

OpenAI Agents' function-tool runner fills a configurable number of slots, creates tasks, waits for `FIRST_COMPLETED`, records completed outputs in deterministic tool-call order, then fills newly available slots. On a failure, it cancels cancellable siblings and drains cancellation/post-invoke cleanup separately. [OpenAI Agents concurrency loop](https://github.com/openai/openai-agents-python/blob/87425fae1c2a9a4327686f1fa36eef2aabffdc1d/src/agents/run_internal/tool_execution.py#L1492-L1564), [OpenAI Agents deterministic result recording](https://github.com/openai/openai-agents-python/blob/87425fae1c2a9a4327686f1fa36eef2aabffdc1d/src/agents/run_internal/tool_execution.py#L326-L378)

Its cleanup is bounded by a monotonic deadline and returns any tasks that remain after the settlement window; background callbacks consume late task exceptions so cancellation cleanup does not hide the triggering failure. This is a useful reference for Nexa's collector shutdown and root-cause preservation. [OpenAI Agents bounded settlement](https://github.com/openai/openai-agents-python/blob/87425fae1c2a9a4327686f1fa36eef2aabffdc1d/src/agents/run_internal/tool_execution.py#L437-L484), [OpenAI Agents sibling-cancellation helpers](https://github.com/openai/openai-agents-python/blob/87425fae1c2a9a4327686f1fa36eef2aabffdc1d/src/agents/run_internal/tool_execution.py#L270-L323)

OpenAI Agents also separates immediate cancellation from graceful “after turn” cancellation, and its streaming result owns an event queue plus its run/guardrail tasks. The documented contract tells consumers to continue draining events until cancellation completes. [OpenAI Agents streaming ownership](https://github.com/openai/openai-agents-python/blob/87425fae1c2a9a4327686f1fa36eef2aabffdc1d/src/agents/result.py#L526-L576), [OpenAI Agents cancellation modes](https://github.com/openai/openai-agents-python/blob/87425fae1c2a9a4327686f1fa36eef2aabffdc1d/src/agents/result.py#L730-L775)

LangGraph's Pregel runner explicitly owns concurrent execution, write commits, yielding when output is available, and interruption of sibling tasks. It submits independent tasks, waits for the first completion, yields updates/debug output per completion, and applies a single absolute end time to its loop. [LangGraph runner responsibility](https://github.com/langchain-ai/langgraph/blob/b2926a0ff9589c28c7e01fe7cdbb337b86d5a4b4/libs/langgraph/langgraph/pregel/_runner.py#L135-L169), [LangGraph concurrent tick](https://github.com/langchain-ai/langgraph/blob/b2926a0ff9589c28c7e01fe7cdbb337b86d5a4b4/libs/langgraph/langgraph/pregel/_runner.py#L258-L341)

LangGraph commits success, cancellation, and errors per task. Its fatal path cancels inflight tasks and re-raises the unhandled exception; if the absolute timeout expires, it cancels inflight tasks and raises timeout. [LangGraph task commit](https://github.com/langchain-ai/langgraph/blob/b2926a0ff9589c28c7e01fe7cdbb337b86d5a4b4/libs/langgraph/langgraph/pregel/_runner.py#L574-L613), [LangGraph fail-fast/timeout path](https://github.com/langchain-ai/langgraph/blob/b2926a0ff9589c28c7e01fe7cdbb337b86d5a4b4/libs/langgraph/langgraph/pregel/_runner.py#L616-L697)

LangGraph exposes task start/finish events containing result and error, separately from token messages, state updates, checkpoints, and custom progress. Its timeout policy also distinguishes a hard run timeout from an idle timeout refreshed by progress/heartbeats, while warning that cancellation is cooperative. [LangGraph stream modes](https://github.com/langchain-ai/langgraph/blob/b2926a0ff9589c28c7e01fe7cdbb337b86d5a4b4/libs/langgraph/langgraph/types.py#L120-L133), [LangGraph task stream payload](https://github.com/langchain-ai/langgraph/blob/b2926a0ff9589c28c7e01fe7cdbb337b86d5a4b4/libs/langgraph/langgraph/types.py#L318-L330), [LangGraph timeout policy](https://github.com/langchain-ai/langgraph/blob/b2926a0ff9589c28c7e01fe7cdbb337b86d5a4b4/libs/langgraph/langgraph/types.py#L449-L478)

### 4.2 Recommended `DelegationScheduler` contract

Separate these modules:

```rust
DelegationScheduler          // admission, lanes, deadlines, completion policy
ProviderConcurrencyGovernor  // origin/provider limits and rate-limit feedback
DelegationBatch              // durable batch identity and aggregate policy
DelegationWorker             // worker state machine and cancellation child
WorkerEventBridge            // bounded, ordered event stream to parent/UI
WorkerResultStore            // durable result/error/usage/artifact references
```

Emit an append-only event envelope as soon as state changes:

```rust
WorkerEvent {
    batch_id,
    worker_id,
    sequence,
    at,
    phase, // queued, admitted, connecting, first_token, running, terminal
    payload: Progress | PartialArtifact | Completed | Failed | TimedOut | Cancelled,
}
```

The parent/UI can then observe progress without waiting for a tool-call vector. A `Completed` result should contain a compact synthesis payload plus artifact/evidence references; large detail belongs in an artifact so the parent prompt does not grow with every partial event.

Completion policy is separate from failure policy:

| Completion policy | Parent may resume when | Remaining workers |
| --- | --- | --- |
| `All` | every worker is terminal | none |
| `Quorum(n)` | `n` usable results exist | continue as supplemental or cancel by profile |
| `FirstSuccess` | first usable result exists | cancel and bounded-drain siblings |
| `Deadline` | batch deadline expires | persist completed/failed; cancel rest |
| `ParentDecides` | parent sends continue/cancel | remain governed and visible |

Classify errors before applying fail-fast:

- **Batch-fatal:** invalid/auth-denied model, invalid tool policy, corrupted shared snapshot, parent cancellation. Cancel siblings immediately.
- **Worker-local:** one source unavailable, one research hypothesis fails, one critic rejects its input. Record it and continue if the completion policy can still succeed.
- **Transient/provider-wide:** rate limit, connection storm, endpoint outage. Feed the global provider governor/circuit state so six workers do not retry independently.

Root-cause selection must be deterministic. Preserve the first/highest-priority causal failure, then attach cancellation and late-cleanup failures as secondary diagnostics; the OpenAI Agents code above explicitly avoids letting a late failure mask the triggering one.

### 4.3 Partial-result and shutdown acceptance tests

- With six workers finishing at 1, 2, 3, 30, 60, and 90 seconds, `Quorum(3)` must release the parent immediately after the third usable result while all later states remain visible.
- An auth failure before first byte must become a worker/batch error event without requiring another model turn to explain it.
- One worker whose event sender never closes must not keep the batch tool call alive past the collector grace window.
- Cancelling a parent must cancel all batch children; cancelling one worker must not cancel the parent or unrelated workers.
- A queue timeout must release call/budget accounting and must not count as a started provider request.
- A run deadline must not be extended by retries; a stream-idle deadline may refresh only on meaningful bytes/events, not on local polling.
- Failure plus sibling-cancellation failure must report the original failure as primary.

## 5. Overlay primitives, positioning, accessibility, and virtualized choices

### 5.1 Primitive selection

Radix Select implements a combobox trigger, a listbox popup, typeahead, focus restoration, Escape/outside handling, portal support, and optional Popper positioning/collision inputs in source. It is a good primitive for small, fixed, single-value choices where the popup is not an editable search surface. [Radix Select trigger/typeahead](https://github.com/radix-ui/primitives/blob/f7ecd5ab16f5e1e820eb5786a1419a98a2d594ae/packages/react/select/src/select.tsx#L322-L398), [Radix Select portal/content/listbox](https://github.com/radix-ui/primitives/blob/f7ecd5ab16f5e1e820eb5786a1419a98a2d594ae/packages/react/select/src/select.tsx#L474-L503), [Radix Select positioned content](https://github.com/radix-ui/primitives/blob/f7ecd5ab16f5e1e820eb5786a1419a98a2d594ae/packages/react/select/src/select.tsx#L647-L892)

Radix Popover provides a configurable portal container, modal/non-modal focus behavior, close autofocus, Escape/outside interaction handling, and anchor-based content. Its Popper layer uses Floating UI `offset`, `shift`, `flip`, `size`, and optional `hide` middleware with explicit collision boundaries/padding. [Radix Popover portal source](https://github.com/radix-ui/primitives/blob/f7ecd5ab16f5e1e820eb5786a1419a98a2d594ae/packages/react/popover/src/popover.tsx#L183-L217), [Radix Popover focus/dismiss source](https://github.com/radix-ui/primitives/blob/f7ecd5ab16f5e1e820eb5786a1419a98a2d594ae/packages/react/popover/src/popover.tsx#L416-L485), [Radix Popper collision source](https://github.com/radix-ui/primitives/blob/f7ecd5ab16f5e1e820eb5786a1419a98a2d594ae/packages/react/popper/src/popper.tsx#L169-L274)

Floating UI's `autoUpdate` observes ancestor scroll/resize, element resize, and layout shift, but its official documentation says to install it only while the floating element is mounted/open and always run its cleanup; otherwise many dormant overlays can cause severe performance degradation. [Floating UI `autoUpdate`](https://floating-ui.com/docs/autoupdate)

Floating UI's `flip` keeps a preferred placement until it no longer fits, `shift` keeps the element within a clipping container, and `size` exposes available dimensions and reference dimensions so a select can match trigger width and cap height. [Floating UI `flip`](https://floating-ui.com/docs/flip), [Floating UI React positioning middleware](https://floating-ui.com/docs/usefloating#middleware), [Floating UI `size`](https://floating-ui.com/docs/size)

`cmdk` 1.1.1 describes itself as an accessible combobox, composes Radix Dialog for its dialog mode, supports a custom portal container, and recommends Radix Popover for combobox composition. It does not provide virtualization; its documented path is to disable internal filtering/sorting with `shouldFilter={false}` and bring a virtualizer. [`cmdk` overview](https://github.com/pacocoursey/cmdk/blob/fb4ea04e9ec211777fbb39c6104e3c5f2ee107d2/README.md#L1-L7), [`cmdk` dialog/portal](https://github.com/pacocoursey/cmdk/blob/fb4ea04e9ec211777fbb39c6104e3c5f2ee107d2/README.md#L153-L174), [`cmdk` Popover recommendation](https://github.com/pacocoursey/cmdk/blob/fb4ea04e9ec211777fbb39c6104e3c5f2ee107d2/README.md#L399-L425), [`cmdk` virtualization FAQ](https://github.com/pacocoursey/cmdk/blob/fb4ea04e9ec211777fbb39c6104e3c5f2ee107d2/README.md#L432-L436)

Recommended mapping:

- `NexaSelect` = Radix Select for theme, execution mode, orchestration profile, and other compact fixed lists.
- `NexaCombobox` = Radix Popover + `cmdk` for model, provider, skill, workflow, and voice search.
- `NexaMenu` = Radix DropdownMenu/ContextMenu for actions; do not express action commands as listbox options.
- One `OverlayProvider` owns the portal container, collision boundary, z-index layers, motion tokens, and focus-return policy.

### 5.2 Accessibility constraints under virtualization

The WAI-ARIA combobox pattern keeps DOM focus on the combobox and points `aria-activedescendant` at the focused option in its controlled popup. The ARIA specification requires that reference to resolve to an existing owned element, and recommends keeping the active descendant visible/in view. [WAI-ARIA combobox pattern](https://www.w3.org/WAI/ARIA/apg/patterns/combobox/), [WAI-ARIA 1.2 `aria-activedescendant`](https://www.w3.org/TR/wai-aria/#aria-activedescendant)

The WAI-ARIA listbox pattern requires options to be contained/owned by the listbox. When the full set is not in the DOM because items load/appear dynamically, options need correct `aria-setsize` and `aria-posinset`. It also warns that a listbox option is not an accessible container for nested interactive elements such as buttons or checkboxes. [WAI-ARIA listbox pattern](https://www.w3.org/WAI/ARIA/apg/patterns/listbox/)

Therefore a virtualized `NexaCombobox` must:

- keep the current active option mounted even if it lies just outside the normal visible range;
- scroll the active option into view before updating `aria-activedescendant`;
- expose set size and position for the filtered logical result set;
- retain stable option IDs/keys across filtering and reordering;
- render secondary badges/descriptions as non-interactive option content; put actions in a menu/dialog instead;
- test Arrow Up/Down, Home/End, Enter, Escape, printable search, IME composition, pointer hover, and focus return with screen readers.

TanStack Virtual's `rangeExtractor` can force specific indices to render outside the visible range, and `getItemKey` can supply stable logical keys. Those APIs fit the active-option pinning requirement above. [TanStack Virtual range/key APIs](https://github.com/TanStack/virtual/blob/d2cf98beea1696c7187c06b57c9e724d1957963c/docs/api/virtualizer.md#L77-L141)

### 5.3 Overlay positioning contract for a desktop WebView

Use `position="popper"`, pass the actual app viewport/overlay root as the collision boundary, reserve padding for the window chrome/safe area, and cap content using the available-height CSS variable. Install position observers only while open. Match trigger width for selects/comboboxes, but allow a documented minimum/maximum for long model names.

Animation must use the resolved `data-side`/transform origin so a collision flip does not animate from the old side. Reduced-motion mode should remove transform movement while preserving state visibility. Radix exposes resolved side/alignment and a collision-aware transform origin for this purpose. [Radix Popover official API and animation variables](https://www.radix-ui.com/primitives/docs/components/popover)

Do not mix independent document-level Escape/outside-click handlers with Radix dismissable layers. Nested overlays need one stack/branch policy so Escape closes only the topmost eligible layer and focus returns to the correct trigger.

## 6. Chat/message virtualization

TanStack Virtual is headless and supports overscan, stable keys, dynamic `measureElement`, custom ranges, and scroll-to-index APIs. Its current chat guide adds `anchorTo: 'end'`, prepend-stable lookup by persistent item key, `followOnAppend`, and adjustment when the last streaming item's measured height grows. [TanStack Virtualizer API source](https://github.com/TanStack/virtual/blob/d2cf98beea1696c7187c06b57c9e724d1957963c/docs/api/virtualizer.md#L31-L141), [TanStack Virtual chat behavior](https://github.com/TanStack/virtual/blob/d2cf98beea1696c7187c06b57c9e724d1957963c/docs/chat.md#L38-L99)

`react-window` is intentionally a list component for rendering large data sets; v2 accepts fixed, percentage, function, or dynamic-cache row heights, but documents dynamic heights as less efficient. It offers overscan and custom row keys, but no first-party end-anchored streaming contract comparable to TanStack Virtual's current chat guide. [`react-window` purpose](https://github.com/bvaughn/react-window/blob/4d9eebbb510262b3b7e95463cf49a10de53ea77d/README.md#L1-L3), [`react-window` row height](https://github.com/bvaughn/react-window/blob/4d9eebbb510262b3b7e95463cf49a10de53ea77d/README.md#L67-L81), [`react-window` overscan/keys](https://github.com/bvaughn/react-window/blob/4d9eebbb510262b3b7e95463cf49a10de53ea77d/README.md#L139-L160)

Recommended Nexa transcript design:

- Use TanStack Virtual with a persistent message/turn ID key, normal chronological array order, `anchorTo: 'end'`, measured dynamic rows, and modest overscan.
- Treat the live assistant turn as one virtual row whose internal stream blocks update; do not create a DOM row per token/event.
- Follow appended output only if the user was already at the end. If the user scrolls upward, show “Jump to latest” and preserve their viewport.
- Prepend older history normally; stable keys plus end anchoring should preserve the visible item without manual inverted transforms.
- Memoize completed message rows and cache Markdown output by `(message_id, content_hash, renderer_version)`; invalidate only the changed live row.
- Keep focus/search hits accessible even when their row is outside the current virtual range by scrolling and then focusing, rather than pointing accessibility state at an unmounted node.

Acceptance should include 10,000 messages, mixed code/Markdown/images, older-history prepend, streaming growth at the bottom, user-scrolled-away behavior, resize/DPI changes, search-to-offscreen result, and keyboard/screen-reader traversal. DOM message rows should remain bounded by viewport plus overscan, not total history.

## 7. React stream publication and bounded caches

### 7.1 Batching at the store boundary

React batches multiple state updates made within its event handling, but this does not reduce the number of independent external events or the cost of repeatedly rebuilding large arrays before React sees them. React's own guidance describes batching as processing queued updates after the event handler; it is not a stream backpressure mechanism. [React state batching](https://react.dev/learn/queueing-a-series-of-state-updates#react-batches-state-updates)

For Nexa's pinned React 19.2.8, `unstable_batchedUpdates` is implemented as a passthrough, so wrapping every backend callback with it would not supply the desired frame-level coalescing. [React 19.2.8 implementation](https://github.com/facebook/react/blob/1dd4ecbdabf826f527fc9a58c05ea70375b7d170/packages/react-dom/src/shared/ReactDOM.js#L51-L74)

`useSyncExternalStore` requires a stable subscription function and a cached immutable snapshot: React re-renders when the snapshot identity changes, and returning a fresh object for every `getSnapshot` call can cause an infinite render loop. [React `useSyncExternalStore`](https://react.dev/reference/react/useSyncExternalStore)

Recommended publication pipeline:

```text
backend events
  -> ordered mutable accumulator per conversation/run
  -> mark dirty fields + schedule one animation-frame flush
  -> freeze/publish one immutable snapshot and version
  -> useSyncExternalStore subscribers read the same snapshot object
  -> selectors/memoized rows re-render only affected surfaces
```

Rules:

- Ordinary text/thinking/tool-progress deltas publish at most once per animation frame. Accumulation itself may remain immediate so ordering, watchdogs, and persistence are correct.
- The first visible token can request the next frame immediately and should retain separate arrival vs paint timestamps.
- Error, approval-required, cancellation, and terminal events flush immediately, after first applying any pending deltas in sequence order.
- One frame should concatenate text chunks once and batch trace/tool mutations; avoid repeated array spreads per raw token.
- Cache exactly one immutable snapshot per stream version. `getSnapshot()` must return it unchanged until the next publish.
- `startTransition`/`useDeferredValue` can deprioritize expensive derived panels, but they do not reduce network/store updates; React explicitly says `useDeferredValue` does not prevent extra requests and does not make the underlying render cheaper. [React `useDeferredValue`](https://react.dev/reference/react/useDeferredValue)

### 7.2 LRU and resource disposal

The reference `lru-cache` implementation requires at least one of item `max`, `maxSize`, or TTL to prevent unsafe unbounded storage. With `maxSize`, each item supplies a size calculation; TTL alone does not preemptively remove stale entries unless autopurge is enabled. It also exposes `dispose` for resource cleanup on eviction. [`lru-cache` 11.5.2 bounds](https://github.com/isaacs/node-lru-cache/blob/16b3a916662ab449d496b7b4b4f04132565d1d28/README.md#L40-L72), [`lru-cache` storage-safety details](https://github.com/isaacs/node-lru-cache/blob/16b3a916662ab449d496b7b4b4f04132565d1d28/README.md#L112-L139)

Nexa does not have to adopt that package, but its cache contract should copy the safety properties:

```ts
ConversationCacheEntry {
  messages;
  turns;
  taskRuns;
  renderedMarkdown;
  measuredRowHeights;
  approximateBytes;
  lastAccess;
  persistenceState;
}
```

- Bound both entry count and approximate byte weight; a count-only bound fails when one conversation contains huge messages/images.
- Pin the active conversation and any live run. Evict least-recently-used inactive conversations only after canonical data is persisted.
- Store artifact IDs/blob URLs instead of base64 image bodies in React state. Eviction/dispose must revoke object URLs and clear timers/subscriptions owned by the entry.
- Keep full trace/run events in the durable store; retain only a bounded recent live projection in memory and rebuild older views on demand.
- Separate transcript data LRU from rendered-Markdown and measurement caches so derived artifacts can be evicted more aggressively.
- Add counters for entries, calculated bytes, evictions by reason, restore latency, and hit rate. Memory limits without observability will be tuned by guesswork.

## 8. Suggested implementation order and gates

### Model-limit discovery must keep input and output separate

Google's model resource exposes `inputTokenLimit` and `outputTokenLimit` as independent fields, so provider discovery should populate both instead of deriving one from the other or applying a shared 32k ceiling. The current official specifications list Gemini 2.5 Pro at 1,048,576 input tokens and 65,536 output tokens; the Gemini 3.5 Flash guide likewise documents a 1M input window and up to 65k output. Nexa should prefer discovered catalog values, retain conservative fallbacks only when discovery is unavailable, and clamp each worker's requested output credit independently of its selected parent context. [Google Models API](https://ai.google.dev/api/models), [Gemini 2.5 Pro limits](https://ai.google.dev/gemini-api/docs/models/gemini-2.5-pro), [Gemini 3.5 Flash limits](https://ai.google.dev/gemini-api/docs/whats-new-gemini-3.5)

1. **Telemetry contract:** measure launch acknowledgement, context/tool initialization, transport connect/reuse/version, first byte, first visible paint, queue wait, worker lifecycle, and cache size.
2. **Shared transport pool:** remove per-provider Client construction from the turn path; add `AutoAlpn` and `Http1Only` profiles plus origin health and retry safety tests.
3. **Scheduler lifecycle:** two-phase admission, deadline taxonomy, cancellation tree, `JoinSet` ownership, bounded collector drain, and direct error events.
4. **Partial batch results:** durable worker event envelopes and explicit completion policies; allow parent synthesis at quorum without hiding late work.
5. **Frame-coalesced stream snapshots and LRU:** publish immutable per-version snapshots, cap trace/cache memory, and add disposal.
6. **Transcript virtualization:** implement TanStack Virtual chat contract and long-session/accessibility tests.
7. **Overlay system:** add Radix Select/Popover/Menu primitives, retain `cmdk` for search, add active-option-safe virtualization for large pickers, then migrate handwritten/native dropdowns.

A phase is not complete until fault-injection/soak tests prove its bound. In particular, “timeout configured” is insufficient unless queued work, worker bodies, event collectors, retries, and shutdown each terminate within a measured limit; “virtualized” is insufficient unless focus and `aria-activedescendant` never target an unmounted option; and “HTTP/2 enabled” is insufficient unless connection reuse, safe downgrade, and non-duplicating retry behavior are observable.

## Source snapshot

The immutable source links in this note use these revisions:

| Project | Revision used |
| --- | --- |
| Nexa baseline | `cd70bd05c497a54c931c94475aa3774f15c7eb96` |
| reqwest 0.12.28 | `d97859910c357827ad5993d37ce750ad595f4fff` |
| Tokio 1.53.0 | `be689a35f5ade5a39e507f79d3ec85cdab27806f` |
| tokio-util 0.7.18 | `9cc02cc88d083113cd9889a74b382e39e430e180` |
| OpenAI Agents Python | `87425fae1c2a9a4327686f1fa36eef2aabffdc1d` |
| LangGraph | `b2926a0ff9589c28c7e01fe7cdbb337b86d5a4b4` |
| Radix Primitives | `f7ecd5ab16f5e1e820eb5786a1419a98a2d594ae` |
| cmdk 1.1.1 | `fb4ea04e9ec211777fbb39c6104e3c5f2ee107d2` |
| TanStack Virtual | `d2cf98beea1696c7187c06b57c9e724d1957963c` |
| react-window | `4d9eebbb510262b3b7e95463cf49a10de53ea77d` |
| React 19.2.8 | `1dd4ecbdabf826f527fc9a58c05ea70375b7d170` |
| lru-cache 11.5.2 | `16b3a916662ab449d496b7b4b4f04132565d1d28` |
