# Nexa Agent Runtime

Nexa's local-first execution context for turning a user request into a durable, recoverable Agent Run and its observable Run Events.

## Language

**Run Event outbox**:
The single ordered publication authority owned by one Agent Run. It accepts unsequenced durable events and explicitly ephemeral live-preview events through separate interfaces, and is permanently closed by that run's true terminal outcome.
_Avoid_: publication manager, event service, stream sequencer

**Tool input session**:
The per-model-sample boundary that keeps provider argument assembly lossless while admitting only bounded, byte-bucketed semantic previews. It resets with a retried sample and never authorizes execution.
_Avoid_: tool argument debounce, partial tool call executor, diff stream

**Live preview event**:
A replaceable, sequenced Run Event projection delivered through the outbox but intentionally omitted from the durable ledger. Queue pressure may drop it without failing the Agent Run; lifecycle and terminal events are never live previews.
_Avoid_: best-effort durable event, preview database row, tool progress history

**Resumable pause**:
A durable, nonterminal Agent Run phase that records a restartable checkpoint while leaving the Run Event outbox open for continuation.
_Avoid_: paused terminal, `done(status=paused)`

**Run lifecycle barrier**:
The per-run serialization boundary spanning continuation claim, executor spawn, session registration, pause, and stop decisions.
_Avoid_: session registration flag, launch sleep, best-effort pause race

**Model attempt**:
One provider sample together with its transport-recovery budget, immutable accepted route, and replay projection. It starts from unprojected history and owns every physical retry until it either accepts output or returns a typed failure to the turn loop.
_Avoid_: sampling retry loop, stream recovery decision plumbing, current provider route

**Turn budget**:
The per-user-turn authority that counts complete validated tool batches entering execution, whether controller-directed prefetch/reconnaissance or model-directed client tools, while assigning independent sequence numbers to physical provider samples and reserving one answer-only sample after a finite tool-round limit. Zero blocks every tool-dispatch path. Transport retry, output continuation, context rollover, steering restart, rejected drafts, and loop-guard-blocked synthetic results never spend this budget.
_Avoid_: max model iterations, shared retry counter, final iteration

**Provider terminal**:
The provider-adapter fact describing why one physical sample ended, including output limit, context limit, provider pause, client-tool boundary, safety refusal, malformed/protocol-incomplete output, and retained unknown raw reasons. It authorizes no recovery or side effect by itself.
_Avoid_: generic finish reason, successful EOF, provider error string

**Provider pause replay state**:
The exact ordered provider-native assistant blocks required to resume a provider-owned hosted-tool turn, such as Anthropic `pause_turn`. It is captured as a typed replay sidecar, replayed verbatim only on the compatible route, and must be present and structurally valid before a pause can continue.
_Avoid_: visible pause text, reconstructed server tool call, blind retry

**Accepted route**:
The immutable provider route bound to a model attempt only after that route produces the first accepted stream event, or after a non-streaming completion succeeds. It is the provenance used for replay validation and the durable provider-turn envelope.
_Avoid_: active route snapshot, latest provider, pre-stream route

**Durable Run reconciliation**:
The frontend authority that selects the expected Agent Run, joins its ordered Run Events, task timeline, and final assistant message, and returns a typed active, suspended, terminal, pending, missing, or stale outcome. It owns durable-query timeouts, missing-run confirmation, and event-gap retry policy; UI stores only apply the returned projection and schedule their own timers.
_Avoid_: watchdog recovery API, hydration-only run selection, final-message polling branch

**Provider event stream**:
The sole incremental LLM provider interface. It preserves text and metadata chunks, provider-hosted tools, cancellation, and recoverable or terminal failures; chunk-only wire protocols are normalized into it in one direction.
_Avoid_: provider chunk stream, reverse stream adapter, hosted-tool filtering
