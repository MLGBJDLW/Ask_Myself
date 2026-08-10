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
