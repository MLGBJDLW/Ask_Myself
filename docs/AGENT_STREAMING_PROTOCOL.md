# Agent Streaming Protocol

Status: normative runtime contract.

## Ownership

Each agent task run has exactly one native outbox actor. Runtime producers may
submit only unsequenced `AgentRunEvent` values. The actor alone assigns the
monotonic `eventSeq`, validates protocol version 2, commits durable events, and
then delivers them to the main window. A terminal `done` or `error` is the last
accepted event for the run.

If validation or durable persistence fails, the outbox fails closed and cancels
the run cancellation domain before emitting its ephemeral failure terminal. A
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

Tool execution is projected through the typed `ToolRun` lifecycle. Provider
assembly fragments such as partial `ToolCall` arguments are not a public UI
protocol.

## Ordering and blocks

The renderer advances a run only when the exact next `eventSeq` is available.
Future events remain buffered and trigger durable recovery; duplicates and
post-terminal events are ignored. Answer and thinking block deltas also require
the exact UTF-8 byte offset. Tool and reset boundaries rotate block identities.

Ordering is also bound to `runId`. A different run may replace only a settled
retained projection; an event from another run cannot enter a live projection.
The launch handshake is authoritative: if a stopped run races into a freshly
reset state before the new handle arrives, binding the handle discards that
claim and recovers the new run ledger. After binding, events from any other
run are rejected before they can enter the ordering buffer.

Output is coalesced into bounded blocks before it enters the outbox, so model
tokens do not perform synchronous SQLite work. Durable blocks are written on
the dedicated database writer lane before main-window delivery.

## Completion and recovery

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
