# Subscription agents

Add **GitHub Copilot** or **ChatGPT / Codex** from Settings → AI Providers →
Add Provider. Sign in through the official runtime and save the provider.
Choose the model and reasoning level in Chat. The saved provider and its currently available models
then appear in the Chat model picker. Reasoning levels come from that account's
model catalog. Signing in does not automatically create a provider.

## Execution ownership

`DesktopAgentBackend` selects either Nexa's direct API executor or an official
subscription runtime. A subscription is not an OpenAI-compatible API endpoint.
Copilot's SDK and Codex's app-server own their model loops. They use the same
Nexa tool dispatcher, approval callback, activity database, cancellation token,
browser observation fence, message persistence and ordered Run Event outbox as
direct chat. Tools retain their actual names and schemas.

Each Nexa turn creates one upstream session. The driver remains in the backend
when the renderer reloads; reconciliation reads the existing outbox instead of
submitting another upstream turn. Nexa's bounded reference history is supplied
as reference context, without replaying another provider's native tool records.
Historical skill mentions are outside new user input. The upstream runtime owns
compression within its session; Nexa remains the owner of cross-turn history.

Tool callback IDs are idempotent within the live session. Reusing an ID with
different arguments fails the run. Tool effects are serialized through the
shared dispatcher; invalid schema and denied actions produce structured errors
without an effect. The configured tool budget also applies to external callbacks
(256 calls when no explicit limit is configured). Renderer reload never replays
callbacks. Process loss terminates the turn; it does not transparently resend
an uncertain action.

Malformed historical activity records or event journals are isolated by their
database key. Their original rows remain available for repair, and their IDs
cannot be observed, mutated or reused as valid activities. Unrelated new chats
retain durable activity storage. SQL/storage failures still fail initialization;
the runtime does not silently switch to an in-memory journal.

## Official runtime boundaries

- Copilot uses the pinned SDK/verified CLI in Empty mode, explicitly selects
  custom Nexa tools and denies native permissions. Its official home remains the
  credential owner; Nexa does not copy tokens. User steering is queued until the
  current Copilot operation becomes idle, then sent in the same upstream session.
  Empty mode's forced keychain-disable environment setting is overridden with
  the caller's original setting, so enrollment and execution use the same
  system-keychain or explicitly selected file credential backend.
- Codex uses a fresh ephemeral app-server thread, no executor environments,
  read-only policy and disabled shell, web, plugins, hooks, agents and automation.
  Effective MCP names and skill paths are inventoried and disabled for that
  thread without changing global config. An inventory error prevents submission.
  This enumeration is not an OS sandbox or an atomic ban on skills created after
  the inventory. The supported CLI must accept the complete execution contract.
- Codex native clock requests receive the actual host time. Native asynchronous
  questions/status are persisted visibly, and are never interpreted as terminal
  answers. Real user replies use `turn/steer`; no suggested option is auto-sent.
  Unknown native requests receive a correlated error and terminate visibly.
- Copilot assembles response chunks by `apiCallId` and `chunkIndex`; empty
  reasoning-boundary chunks count toward completeness but reasoning is not
  included in the answer. Missing or conflicting chunks cannot emit success.
  Completed responses are saved before queued steering is submitted.
  Retry events must match the active native turn. They discard its abandoned
  completed and delta-only drafts before failure recovery and clear the same
  live blocks with canonical snapshots, preserving already-saved responses.
- The final response carries its exact persisted assistant message ID. The
  event outbox commits the closing event, task status and open conversation
  turn together; the ID must belong to that conversation and turn. Existing
  tool traces and already-finalized API turns are preserved.
- Subscription drivers validate the selected model and reasoning level before
  inference. There is no automatic switch to another model or API credential.
- Copilot and Codex parents can delegate to configured API workers through Nexa.
  Use `list_subagent_models` and select `agent_config_id` on each worker; a
  subscription cannot be silently inherited as API credentials. Subscription
  child runtimes, Mixture of Agents and scheduled isolated patch runs remain
  unavailable. Unsupported routes fail before inference.
- Manual `/compact` is unavailable for subscription conversations. Its API
  summarizer cannot consume an official subscription login; the action is hidden
  and direct requests fail before constructing an HTTP provider. The native
  runtime continues to manage context inside each active turn.

## Reconciliation and microphone behavior

Dictation and next-turn orchestration controls remain available while the
current response streams. Changing Nexus, quality or MoA updates the next-turn
selection; it does not mutate the already-running executor. The recording dock
expands above the controls and remains mounted through its exit animation,
respecting reduced-motion preferences.

The heartbeat carries the committed event high-water mark. Receiving a heartbeat
does not postpone the recovery watchdog, so an alive backend cannot indefinitely
hide missing messages. Expanded browser controls are positioned inside the app's
content area. Native webview getters run outside the shared browser mutex, and
trusted native input checks execute on the UI thread to avoid cross-thread waits.

Official legacy Qwen microphone presets migrate once to
`dashscope_realtime_asr` / `qwen3-asr-flash-realtime`. The live WebSocket uses
server VAD, publishes interim text while recording, and finishes with
`session.finish`. Ordered utterance finalization retains earlier sentences and
flushes the last sentence. User composer corrections retain ownership. Custom
HTTP endpoints stay final-only and remain labelled as such; they are not silently
converted into a guessed WebSocket service. After migration, explicitly choosing
a batch preset is preserved across settings saves and reloads.

## Verification

Automated contracts cover native event projection, async question persistence,
tool idempotency/schema failures/budget/cancellation, provider enrollment and
model/reasoning selection, recording composer behavior, browser control bounds
and event reconciliation. Ignored desktop tests named `native_*` explicitly opt
into the user's official login and one read-only tool inference. They assert a
fresh tool nonce reaches the streamed answer, executes once, persists once, and
emits one terminal event through the real forwarder/outbox, and closes the turn
with the exact final assistant ID before delivery. They are not run by ordinary CI.
