# Agent Streaming Protocol

Status: normative runtime contract.

## Ownership

Each Agent Run has exactly one core **Run Event outbox**, keyed by `runId` and
reused for the lifetime of that run. Runtime producers may submit only durable,
unsequenced `AgentRunEvent` values. The outbox alone assigns the monotonic
`eventSeq`, validates protocol version 2, commits the ordered ledger, and
arbitrates terminal acceptance. Desktop code is a delivery adapter, not another
sequence or terminal owner.

The first accepted true terminal event closes the outbox permanently; later
submissions are rejected as already closed. `paused` and `awaiting_user_input`
are durable, resumable phases rather than terminal outcomes, so they leave the
same outbox open. Completion, failure, timeout, and cancellation close it.
Opening an existing run resumes at `MAX(eventSeq) + 1`; historical gaps are
neither filled nor renumbered. The registry stores only a weak reference while
the actor retains its open outbox through suspension. A true terminal or
fail-closed outcome ends that actor lifetime; reopening then reconstructs the
closed state from durable storage.

If durable persistence fails or the bounded producer queue saturates, the
outbox fails closed: it cancels the run cancellation domain, rejects later
events, preserves the contiguous accepted prefix that can still be committed,
and emits exactly one internally generated ephemeral failure terminal for live
presentation. Runtime producers cannot submit ephemeral events themselves. A
continuing executor cannot invoke more tools after the ledger becomes unsafe.

Provider-native replay envelopes remain backend-only durable state. They are
never compacted into the public stream payload and are not replaced by this
presentation protocol.

## Delivery channels

- `agent://run-event` carries only `{ conversationId, runEvent }` to the main
  window. The runtime schema rejects unknown envelope or RunEvent fields.
- `agent://heartbeat` carries liveness only. It has no sequence, trace entry, or
  durable row.
- `agent://task-snapshot` carries the materialized task projection at lifecycle
  boundaries, not for each output block.
- `companion://projection-changed` invalidates the Companion's low-frequency
  projection without exposing chat content.

The desktop delivery adapter is invoked only after the Run Event rows and any
semantic task projection have completed on the database writer lane. Delivery
is best effort: an unavailable main window does not roll back durable state or
hold the run's completion barrier open.

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

Replaceable `ToolRunUpdated` previews are likewise coalesced by `callId` on the
stream cadence. A semantic boundary flushes the latest preview before the next
tool lifecycle event. Provider argument fragments therefore do not create one
durable row per transport chunk.

## Completion and recovery

A terminal submission is not completion by itself. The terminal completion
barrier resolves only after the terminal Run Event and its task projection are
durable; run finalization waits for that barrier before exposing dependent
completion state. A resumable pause or user-input wait may flush its accepted
prefix, but it does not satisfy the terminal barrier. Main-window availability
is not part of this durability contract.

Launch, pause, stop, and same-run continuation decisions are serialized by the
run's lifecycle barrier. A new launch acquires that barrier after its durable
run ID exists and re-reads the task projection before spawning; checkpoint and
interaction continuations acquire it before claiming their durable response.
This closes both the database-to-session registration gap and duplicate
executor retries. A checkpoint continuation appends one idempotent transcript
message and reuses the original turn, run, and open outbox.
Creating a resumable pause is also one outbox-owned transaction: its checkpoint
row, paused Run Event, task projection, and turn projection commit together
before the checkpoint is returned or delivered.

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
bounded message. A recovery pass replays the ordered
durable ledger and confirms completion with the final assistant message joined
through the conversation turn. A completed task without that message is not
allowed to settle to a blank answer; recovery remains armed until the durable
message is available.

The chat surface hydrates an active conversation once. Live events and recovery
patch that projection instead of initiating a second completion fetch.
