# Agent Streaming Protocol

Status: normative runtime contract.

## Ownership

Each Agent Run has exactly one core **Run Event outbox**, keyed by `runId` and
reused for the lifetime of that run. Runtime producers may submit only durable,
unsequenced `AgentRunEvent` values. The outbox alone assigns the monotonic
`eventSeq`, validates protocol version 2, commits the ordered ledger, and
arbitrates terminal acceptance. Desktop code is a delivery adapter, not another
sequence or terminal owner.

One narrow exception exists for replaceable live tool-input snapshots. The
desktop bridge may call the outbox's explicit ephemeral publication interface;
the outbox still sequences and orders the event, but deliberately omits it from
SQLite. Lifecycle, approval, resumable, result, error, and terminal events must
remain durable and use the normal submission interface.

`flush()` is a processed-through barrier: when an ephemeral publication has
been delivered or intentionally dropped, its sequence advances that barrier
after every earlier durable event is committed. It must not wait for a row that
the protocol deliberately never writes.

The first accepted true terminal event closes the outbox permanently; later
submissions are rejected as already closed. `paused` and `awaiting_user_input`
are durable, resumable phases rather than terminal outcomes, so they leave the
same outbox open. Completion, failure, timeout, and cancellation close it.
Opening an existing run resumes at `MAX(eventSeq) + 1`; historical gaps are
neither filled nor renumbered. The registry stores only a weak reference while
the actor retains its open outbox through suspension. A true terminal or
fail-closed outcome ends that actor lifetime; reopening then reconstructs the
closed state from durable storage.

If durable persistence fails or a durable event saturates the bounded producer queue, the
outbox fails closed: it cancels the run cancellation domain, rejects later
events, preserves the contiguous accepted prefix that can still be committed,
and emits exactly one internally generated ephemeral failure terminal for live
presentation. A replaceable preview is instead dropped under queue pressure; a
diagnostic snapshot must never poison the authoritative run. A continuing
executor cannot invoke more tools after the durable ledger becomes unsafe.
The failure terminal uses `max(durableHead, actorLiveHighWater) + 1`; deriving
it from SQLite alone could reuse a sequence already assigned to an ephemeral
preview or to the durable batch whose commit failed.

Provider-native replay envelopes remain backend-only durable state. They are
never compacted into the public stream payload and are not replaced by this
presentation protocol.

Provider prompt history is also a projection, not the durable ledger itself.
Runtime and controller messages marked volatile exist only for the current
sampling step and are never persisted as conversation transcript. When a
legacy assistant/tool unit cannot be replayed on the selected provider route,
the prompt projection drops its calls, results, reasoning, and diagnostics. It
may retain only a separate non-empty assistant answer that closed that unit; if
none exists, the unit is omitted. The projection must not manufacture a
natural-language assistant summary or replay-boundary system message.

Final-answer samples keep only the newest current-step controller directive;
superseded volatile directives are excluded. If a visible answer nevertheless
starts a line with a reserved internal replay/controller header that the current
user did not quote, the sample is contaminated: reset its public stream, retry
once with tools suppressed, and never persist the discarded text. A second
contaminated sample fails closed instead of becoming conversation history.

## Delivery channels

- `agent://run-event` carries only `{ conversationId, runEvent }` to the main
  window. The runtime schema rejects unknown envelope or RunEvent fields.
- `agent://heartbeat` carries liveness only. It has no sequence, trace entry, or
  durable row.
- `agent://task-snapshot` carries the materialized task projection at lifecycle
  boundaries, not for each output block.
- `companion://projection-changed` invalidates the Companion's low-frequency
  projection without exposing chat content.

For durable events, the desktop delivery adapter is invoked only after the Run
Event rows and any semantic task projection have completed on the database
writer lane. An ephemeral preview first flushes any earlier durable batch, then
is delivered directly in sequence. Delivery is best effort: an unavailable
main window does not roll back durable state or hold the run's completion
barrier open.

Tool execution is projected through the typed `ToolRun` lifecycle. Provider
assembly fragments such as partial `ToolCall` arguments are not a public UI
protocol.

## Ordering and blocks

Live delivery advances a run only when the exact next `eventSeq` is available.
Future events remain buffered and trigger durable recovery; duplicates and
post-terminal events are ignored. A complete authoritative database replay may
advance across a missing sequence because historical ledgers can contain gaps
for events that were intentionally not retained. It never renumbers committed
events. Answer and thinking block deltas also require the exact UTF-8 byte
offset. Tool and reset boundaries rotate block identities.

Gap recovery uses the same authority for a bounded durable suffix. It merges
that suffix with live events buffered while the query was in flight only after
every page is consumed, then may advance across sequence numbers absent from
SQLite. The first page freezes a durable high-water mark and each continuation
is bounded to 2,048 rows, so a busy producer cannot move the recovery target or
hide a later durable page behind an already-buffered live event. Live events
that arrive after the query's captured high-water remain buffered for another
authoritative pass. Strict live dispatch alone never guesses that a gap was
ephemeral.

Ordering is also bound to `runId`. A different run may replace only a settled,
unbound retained projection; an event from another run cannot enter a bound
projection. The launch handshake is authoritative: if a stopped run races into
a freshly reset state before the new handle arrives, binding the handle
discards that claim and recovers the new run ledger. After binding, events from
any other run are rejected before they can enter the ordering buffer, including
after the bound run settles.

Only `outputDelta`, `thinking`, and `usageUpdated` are eligible for deferred
outbox batching. The outbox commits those events after at most 100 ms or when
the pending batch reaches 32 events. Any other semantic event immediately
flushes the pending deferred events and itself, preserving their assigned
order. This keeps token-volume traffic off the synchronous SQLite path without
weakening lifecycle ordering. Every durable batch is written on the dedicated
database writer lane before main-window delivery.

Provider tool arguments are assembled losslessly, but the Tool input session
builds semantic previews only on the first observation and each 2 KiB
cumulative-input bucket. Preparing parsing and raw display are bounded to a
32 KiB prefix. The desktop Tool preview journal then keeps only the latest
snapshot per `callId`, publishing after another 2 KiB of growth or a two-second
slow-stream heartbeat. Only `ToolRunUpdated(status=preparing)` may enter this
journal. Approval-pending, running, and execution-progress updates bypass it and
remain durable lifecycle events. The completed `ToolRun` remains the final
durable authority. Provider argument fragments therefore cannot create one
parse, full diff, frontend event, or SQLite row per transport chunk.

Argument assembly also preserves the provider wire shape: string values are
opaque fragments, while object-valued OpenAI-compatible arguments are typed
complete snapshots and replace the prior snapshot. The runtime never infers
that distinction by reparsing every growing string.

The exposed JSON schema plus host validation is the executable tool contract.
Provider deltas and repaired historical envelopes never authorize execution,
and the system prompt does not duplicate a global "return valid tool JSON"
contract. A malformed or truncated draft is discarded and receives one short,
call-specific retry instruction; only the newly completed, schema-valid call
may cross the dispatch boundary.

## Completion and recovery

Provider terminal rules are dialect-specific. A Chat Completions stream with a
parsed terminal `finish_reason` is complete even when a compatible server closes
immediately without a trailing `[DONE]`; EOF without either terminal evidence
remains an interruption. DeepSeek Responses instead requires
`response.completed`, `response.incomplete`, or `response.failed` and does not
use `[DONE]`. OpenAI Responses and DeepSeek Responses retain separate request,
replay, usage, and terminal capability profiles.

Context planning always keeps a concrete response reserve, but that estimate
is not automatically a provider limit. `max_tokens` / `max_output_tokens` is
sent only for a saved explicit override, a verified endpoint/model catalog
capability, or a caller-authorized cumulative worker budget. Unknown and custom
routes remain provider-managed instead of inheriting Nexa's fallback reserve.

Attempt resets are control-plane events. They may clear an abandoned answer,
thinking block, or preparing tool card, but their retry reason is developer
telemetry and never becomes a chat status row. Reconnecting, degraded, and
recovered states remain silent when recovery succeeds; only a final
failed/offline connection state is user-facing.

A terminal submission is not completion by itself. The terminal completion
barrier resolves only after the terminal Run Event and its task projection are
durable; run finalization waits for that barrier before exposing dependent
completion state. A resumable pause or user-input wait may flush its accepted
prefix, but it does not satisfy the terminal barrier. Main-window availability
is not part of this durability contract.

Requesting a checkpoint pause first cancels and awaits the run's current
executor, establishing an execution fence before any checkpoint persistence can
block. Pending approvals owned by that run are then denied and recorded as
`approvalResolved`; no dead receiver may survive into replay. Finally, the
outbox drains the accepted prefix and commits the checkpoint under the same gate
used for terminal acceptance. Later submissions receive the explicit
`Suspended` result and cannot overtake the checkpoint. The outbox remains alive
but producer-gated while paused. A checkpoint continuation may reopen producer
submission only after the task and original turn are atomically re-queued and
before the replacement executor is spawned.

Launch, pause, stop, and same-run continuation decisions are serialized by the
run's lifecycle barrier. A new launch acquires that barrier after its durable
run ID exists and re-reads the task projection before spawning; checkpoint and
interaction continuations acquire it before claiming their durable response.
This closes both the database-to-session registration gap and duplicate
executor retries. A checkpoint continuation appends one idempotent,
presentation-hidden control message and reuses the original turn, run, and open
outbox. The checkpoint state carries bounded durable assistant output already
shown before the pause so the replacement executor continues after it instead
of repeating it.
If a committed launch cannot open its Run Event outbox before executor
registration, it fails closed through the same terminal-arbitration transaction;
it must not remain queued without an executor or block a later reply retry.
Reply retries and edits of persisted user messages use the same durable suffix
replacement transaction. Both preserve the original user-message ID, replace
the selected message content in place, and remove later turns before launching
the replacement executor.
Creating a resumable pause is also one outbox-owned transaction: its checkpoint
row, paused Run Event, task projection, and turn projection commit together
before the checkpoint is returned or delivered.

Stopping a running turn creates a resumable checkpoint. Stopping while the run
is awaiting user input instead cancels the pending interaction and terminalizes
the run; an already paused run remains resumable and is not silently cancelled.

Before accepting launches after process restart, recovery compares active task
projections with their durable Run Event boundaries. A paused or
awaiting-user-input boundary is restored only when no later `Agent started` or
`cancelling` marker invalidates it. Other interrupted active runs receive one
durable cancelled terminal through the outbox and cross its completion barrier;
an existing true terminal repairs a stale projection without appending another
terminal. The historical `done(status=paused)` encoding remains readable as a
non-closing suspension, but new producers must use a `status` event.

`done.payload.message` is the immediate authoritative assistant answer and may
replace an incomplete streamed preview. When the native payload must be bounded,
`done.payload.messageTruncated` is `true`; only then may the frontend retain a
non-empty, fully ordered streamed preview instead of replacing it with the
bounded message. A recovery pass requests the unseen durable suffix after the
frontend's `eventSeq` high-water mark in frozen, 2,048-row pages, exhausts that
snapshot, replays the ordered ledger, and confirms completion with the final
assistant message joined through the conversation turn. A completed task
without that message is not allowed to settle to a blank answer; recovery
remains armed until the durable message is available.

The chat surface hydrates an active conversation once. Live events and recovery
patch that projection instead of initiating a second completion fetch. React
stream projection is scheduled as interruptible transition work; stable sidebar
state is memoized, and incomplete Mermaid programs are rendered only after the
stream completes so navigation and stop controls retain the urgent UI lane.
When a suspended run is replayed after restart, elapsed-time presentation freezes
at the latest durable suspension event timestamp (falling back to the task
projection update time for legacy rows), not at the time the UI happens to load.
