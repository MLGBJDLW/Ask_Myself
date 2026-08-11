# Nexa Agent Runtime

Nexa's local-first execution context for turning a user request into a durable, recoverable Agent Run and its observable Run Events.

## Language

**Run Event outbox**:
The single ordered publication authority owned by one Agent Run. It accepts unsequenced Run Events and is permanently closed by that run's true terminal outcome.
_Avoid_: publication manager, event service, stream sequencer

**Resumable pause**:
A durable, nonterminal Agent Run phase that records a restartable checkpoint while leaving the Run Event outbox open for continuation.
_Avoid_: paused terminal, `done(status=paused)`

**Run lifecycle barrier**:
The per-run serialization boundary spanning continuation claim, executor spawn, session registration, pause, and stop decisions.
_Avoid_: session registration flag, launch sleep, best-effort pause race

**Model attempt**:
One provider sample together with its transport-recovery budget, immutable accepted route, and replay projection. It starts from unprojected history and owns every physical retry until it either accepts output or returns a typed failure to the turn loop.
_Avoid_: sampling retry loop, stream recovery decision plumbing, current provider route

**Accepted route**:
The immutable provider route bound to a model attempt only after that route produces the first accepted stream event, or after a non-streaming completion succeeds. It is the provenance used for replay validation and the durable provider-turn envelope.
_Avoid_: active route snapshot, latest provider, pre-stream route

**Durable Run reconciliation**:
The frontend authority that selects the expected Agent Run, joins its ordered Run Events, task timeline, and final assistant message, and returns a typed active, suspended, terminal, pending, missing, or stale outcome. It owns durable-query timeouts, missing-run confirmation, and event-gap retry policy; UI stores only apply the returned projection and schedule their own timers.
_Avoid_: watchdog recovery API, hydration-only run selection, final-message polling branch

**Provider event stream**:
The sole incremental LLM provider interface. It preserves text and metadata chunks, provider-hosted tools, cancellation, and recoverable or terminal failures; chunk-only wire protocols are normalized into it in one direction.
_Avoid_: provider chunk stream, reverse stream adapter, hosted-tool filtering
