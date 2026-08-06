# Wave 1 Interaction Runtime: Primary-source Research

This note records the primary-source review for the Wave 1 work described in
`D:\Nexa.txt` lines 21-205 and 1287-1296. It was prepared on 2026-08-06
against immutable upstream commits. It is a design input for an independent
Nexa implementation; it is not a claim that Nexa implements the wire protocol
or persistence model of any reviewed project.

## Executive decision

Wave 1 should replace the current question-card convention with a durable
interaction control plane:

1. `request_user_input` produces a versioned `InteractionRequest` and suspends
   the turn at a durable continuation boundary. It does not merely return a
   tool artifact and rely on prompt wording to make the model stop.
2. The request, draft, response, transition history, and turn checkpoint are
   persisted before the renderer is notified. A process restart reconstructs
   the pending interaction without reconstructing an in-memory future.
3. Submission is a compare-and-set transaction keyed by `interactionId`, a
   one-time resume-token generation, and a client submission id. A duplicated
   click, reconnect replay, or retried IPC call cannot resume a turn twice.
4. A conversation-scoped Interaction Store projects persisted state into one
   `ConversationGateHost` immediately above the composer. The active Decision
   Tray is independent of the message list, thinking trace, and tool-card
   mount lifecycle.
5. The question wizard persists every draft revision, supports sequential
   navigation and review, and turns into a compact timeline record after the
   runtime acknowledges the response.
6. MCP elicitation is an adapter into this runtime, not its data model. Nexa
   must negotiate the relevant MCP capability and preserve server identity,
   protocol version, request id, and opaque resume state without allowing an
   untrusted server to collect secrets through an ordinary form.

The connection banner introduced in Wave 0 can share the same composer-adjacent
surface host, visual primitives, and accessibility rules. It must not share the
interaction persistence state machine: a transport reconnect and a turn blocked
on a user decision have different ownership, completion, and recovery rules.

## Reviewed upstream revisions

| Project | Released baseline | Additional immutable source |
| --- | --- | --- |
| OpenAI Codex | [`rust-v0.146.1`](https://github.com/openai/codex/releases/tag/rust-v0.146.1) at `79b4f03d35962b005b007a015113b38930711665` | Current source at `7a0e974e08c798d1e8d59d407aeb6e24db1313af`; used where the current interactive protocol differs from the release |
| LangGraph | [`1.2.10`](https://github.com/langchain-ai/langgraph/releases/tag/1.2.10) at `41341457342327166d72fc11952ab28fb61ec0bf` | None required |
| VS Code | [`1.132.0`](https://github.com/microsoft/vscode/releases/tag/1.132.0) at `df53daabb18cd157bdb08c7f01c34df936cf12f4` | The released tree contains both Agent Host state actions and the workbench carousel behavior reviewed below |
| MCP specification | [`2026-07-28`](https://github.com/modelcontextprotocol/modelcontextprotocol/releases/tag/2026-07-28) at `5f5440bb26a62e2cf3440b92da5a667efa03b267` | The same release tree retains the `2025-11-25` specification used by Nexa's current negotiated-version analysis |

The latest released Codex protocol has stable question/call/turn identities but
does not yet contain the current main branch's required `isBlocking` field
([released shape](https://github.com/openai/codex/blob/79b4f03d35962b005b007a015113b38930711665/codex-rs/protocol/src/request_user_input.rs#L8-L60)).
Consequently, `isBlocking` is treated below as useful current upstream evidence,
not a released compatibility promise that Nexa should adopt verbatim.

## Requirements traced from `D:\Nexa.txt`

| Wave 1 requirement | Runtime evidence required for exit |
| --- | --- |
| Stable `InteractionRequest` id, status, and resume token | A persisted row and state-transition log exist before the UI event; token validation is scoped to that row and generation |
| Survive scrolling, conversation switch, reconnect, and app restart | The tray is rebuilt from the database, not a component instance or stream-only store |
| Decision Tray above the composer | The active request is rendered by the chat-page shell, outside `ChatMessages` and `ToolCallCard` |
| Sequential wizard, review, drafts, keyboard operation | Draft answers and current index survive unmount/restart; final submission is explicit and atomic |
| `request_user_input` no longer appears in thinking/tool cards | The tool dispatch returns a suspension control effect; the timeline receives only a compact durable summary after resolution |
| FIFO plus risk priority, `1 of N` | Ordering is deterministic (`risk_priority DESC`, then durable queue sequence ASC), preserving FIFO within a priority |
| Agent resumes only after a valid response | A valid response transaction advances the turn from `awaiting_user_input` to `resuming`; ordinary chat text does not satisfy the gate |
| Pending is not thinking | Turn/run status and event projection explicitly use `awaiting_user_input`; no reasoning or assistant-content placeholder is emitted |
| Reconnect uses the same persistent-state component family | One composer-adjacent host renders typed interaction and connection surfaces without merging their state machines |

## Reviewed Nexa seams

The repository already contains most UI ingredients, but not the required
runtime ownership:

- `crates/core/src/tools/request_user_input_tool.rs` validates one to three
  questions and returns a versioned `questionRequest` artifact plus prose that
  tells the model to stop. It does not suspend the agent loop or persist a
  resumable continuation.
- `apps/desktop/src/components/chat/QuestionRequestPanel.tsx` keeps answers,
  free-text drafts, and submitted state in React `useState`/`useRef`. Component
  unmount therefore remains a data-loss boundary.
- `apps/desktop/src/components/chat/ToolCallCard.tsx` extracts and renders the
  request at the tool-card boundary. `ChatMessages.tsx` also detects the pending
  tool inside a thinking timeline, so request visibility remains coupled to
  historical rendering.
- `apps/desktop/src/pages/ChatPage.tsx` already renders
  `ConnectionStatusBanner` directly above the composer. This is the correct
  insertion seam for a conversation gate host.
- `conversation_turns.status`, `agent_task_runs.status/phase`, and the ordered
  `agent_run_events` table already provide durable turn/run/event identities.
  Wave 1 should extend those contracts and add interaction-specific tables,
  rather than introducing a second conversation identity system.
- `crates/core/src/mcp/client.rs` negotiates at most MCP `2025-11-25`, sends an
  empty client capability object, and answers every incoming server request
  with JSON-RPC `Method not found`. MCP elicitation therefore is not currently
  callable and must not be advertised until a real server-request dispatcher
  and Interaction Runtime adapter exist.

## 1. A user-input request is a control-plane operation

### OpenAI Codex evidence

Codex gives question and answer payloads typed identities rather than treating
the UI as arbitrary tool JSON. Its protocol defines stable question ids,
answer maps, the associated call id and turn id, and an explicit `isBlocking`
field
([protocol types](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/protocol/src/request_user_input.rs#L8-L70)).
The app-server request adds `threadId`, `turnId`, and `itemId`, then maps answers
back by question id
([app-server types](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/app-server-protocol/src/protocol/v2/item.rs#L1634-L1692)).

The tool handler awaits the session response before returning the tool output,
so the model cannot continue past the call with a fabricated answer
([handler](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/core/src/tools/handlers/request_user_input.rs#L71-L97)).
That handler also rejects non-root agent sessions
([root ownership](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/core/src/tools/handlers/request_user_input.rs#L38-L68)).
Nexa should likewise route a delegated worker's request through the root
conversation's interaction broker, retaining subtask provenance without
creating an invisible child-owned Decision Tray.
At the app-server boundary, a separate `serverRequest/resolved` notification is
emitted both for a client answer and for lifecycle cleanup on turn
start/completion/interruption
([protocol guidance](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/app-server/README.md#L1642-L1646),
[resolved type](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/app-server-protocol/src/protocol/v2/notification.rs#L50-L56)).
This gives a client an authoritative way to remove a pending prompt even when
it did not submit the final response.

Codex's TUI separately tracks unresolved approvals, user-input requests, and
MCP elicitations for replay. It keys user input by turn and call id, removes
answers FIFO within a turn, clears prompts on terminal turn events, and only
replays requests still present in the pending set
([state and FIFO rationale](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/tui/src/app/pending_interactive_replay.rs#L24-L48),
[answer removal](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/tui/src/app/pending_interactive_replay.rs#L143-L165),
[request registration](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/tui/src/app/pending_interactive_replay.rs#L216-L229),
[terminal cleanup and replay filter](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/tui/src/app/pending_interactive_replay.rs#L269-L278),
[pending projection](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/tui/src/app/pending_interactive_replay.rs#L352-L398)).

### Boundary: Codex is not proof of restart durability

The Codex session shown at the reviewed commit still puts pending input senders
in a turn-local map and awaits a Tokio `oneshot`
([turn state](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/core/src/state/turn.rs#L88-L99),
[request/response wait](https://github.com/openai/codex/blob/7a0e974e08c798d1e8d59d407aeb6e24db1313af/codex-rs/core/src/session/mod.rs#L2643-L2706)).
Its replay state is excellent evidence for correlation, FIFO removal, resolved
notifications, and stale-prompt cleanup, but Nexa's stronger app-restart exit
criterion requires a database-backed continuation rather than copying the
in-memory waiter.

### Nexa contract

Treat tool suspension as an explicit executor result:

```rust
enum ToolControlEffect {
    Complete(ToolResult),
    Suspend(InteractionRequest),
}
```

`request_user_input` validates its model-facing arguments, creates the durable
request and continuation checkpoint in one database transaction, transitions
the turn/run to `awaiting_user_input`, emits an ordered interaction event, and
returns `Suspend`. The agent task then exits as suspended. It is not left alive
on a channel and is not subject to model-stream idle or retry timers.

After a valid answer, a fresh runtime task loads the checkpoint, materializes
one canonical tool result containing the typed answers, and continues the same
turn identity. No reasoning text or placeholder assistant message is needed.

## 2. Durable suspension requires a checkpoint and replay policy

### LangGraph evidence

LangGraph's `interrupt()` explicitly pauses execution, surfaces a value to the
client, and requires `Command(resume=...)` to continue. It also states that a
checkpointer is mandatory because the graph state must be persisted
([interrupt contract](https://github.com/langchain-ai/langgraph/blob/41341457342327166d72fc11952ab28fb61ec0bf/libs/langgraph/langgraph/types.py#L811-L831)).
Interrupts have stable ids that can be resumed directly, and `Command.resume`
accepts either one next value or an id-to-value map
([interrupt identity](https://github.com/langchain-ai/langgraph/blob/41341457342327166d72fc11952ab28fb61ec0bf/libs/langgraph/langgraph/types.py#L533-L578),
[resume command](https://github.com/langchain-ai/langgraph/blob/41341457342327166d72fc11952ab28fb61ec0bf/libs/langgraph/langgraph/types.py#L758-L784)).
Its checkpoint interface calls `thread_id` the primary key for saving state,
resuming interrupts, and time-travel, and exposes explicit get/list/put
operations
([checkpoint contract](https://github.com/langchain-ai/langgraph/blob/41341457342327166d72fc11952ab28fb61ec0bf/libs/checkpoint/langgraph/checkpoint/base/__init__.py#L176-L207),
[checkpoint API](https://github.com/langchain-ai/langgraph/blob/41341457342327166d72fc11952ab28fb61ec0bf/libs/checkpoint/langgraph/checkpoint/base/__init__.py#L227-L295)).

The important replay warning is equally explicit: resume starts the interrupted
node again and re-executes its logic; multiple interrupt values are matched by
call order within the task
([re-execution semantics](https://github.com/langchain-ai/langgraph/blob/41341457342327166d72fc11952ab28fb61ec0bf/libs/langgraph/langgraph/types.py#L818-L831)).

### Nexa contract

Nexa should checkpoint at the completed tool-call boundary, not at an arbitrary
Rust future. The checkpoint must include:

- conversation, turn, run, tool-call, provider, model, and protocol versions;
- validated canonical history up to and including the assistant tool call;
- the interaction id and resume-token generation;
- current iteration/budget counters and the post-response continuation point;
- the effective tool/policy snapshot needed to resume safely;
- a checksum/version so incompatible checkpoints fail closed with a typed
  migration error.

Do not rerun earlier tools or non-idempotent side effects merely because a node
is reconstructed. If implementation convenience requires replay, every
pre-suspension effect must have a persisted idempotency key and the dispatcher
must return its recorded result instead of executing it again.

## 3. Protocol, storage, and exactly-once submission

Use a versioned internal protocol independent from any provider or MCP wire
shape:

```ts
interface InteractionRequestV1 {
  schemaVersion: 1;
  interactionId: string;
  conversationId: string;
  turnId: string;
  runId: string;
  toolCallId?: string;
  source: "agent" | "approval" | "mcp" | "system";
  sourceIdentity?: string;
  kind:
    | "user_input"
    | "approval"
    | "conflict_resolution"
    | "credential_request"
    | "high_risk_confirmation";
  title: string;
  description?: string;
  questions: InteractionQuestion[];
  required: boolean;
  riskPriority: number;
  queueSequence: number;
  status: InteractionStatus;
  resumeGeneration: number;
  resumeToken: string;
  createdAt: string;
  expiresAt?: string;
}

interface InteractionResponseV1 {
  schemaVersion: 1;
  interactionId: string;
  submissionId: string;
  resumeGeneration: number;
  resumeToken: string;
  action: "submit" | "decline" | "cancel";
  answers: Record<string, InteractionAnswer>;
  submittedAt: string;
}
```

The public resume token is a high-entropy opaque bearer value scoped to one
interaction and generation. Persist only a keyed digest, never log it, never
place it in telemetry or the compact timeline artifact, and rotate it if a
request is superseded. `interactionId` is an identity, not an authorization
token.

Recommended SQLite ownership:

```text
interaction_requests
  interaction_id PK
  schema_version, conversation_id, turn_id, run_id, tool_call_id
  source_kind, source_identity, kind, required, risk_priority, queue_sequence
  status, request_json, resume_generation, resume_token_digest
  created_at, updated_at, expires_at, terminal_at

interaction_drafts
  interaction_id PK/FK, revision, answers_json, current_question_index
  review_open, updated_at

interaction_responses
  interaction_id UNIQUE/FK, submission_id UNIQUE
  resume_generation, action, answers_json, submitted_at

interaction_events
  interaction_id, event_seq, from_status, to_status, reason, metadata_json
  PRIMARY KEY (interaction_id, event_seq)
```

The response transaction must:

1. load the request by interaction id;
2. verify conversation/turn scope, nonterminal state, expiry, generation, and
   token digest using constant-time comparison;
3. validate every answer against the persisted question schema;
4. insert the response under both uniqueness constraints;
5. compare-and-set request status to `submitted`;
6. transition the turn/run to `resuming` and append ordered durable events;
7. commit before scheduling the continuation.

If the same `submissionId` is retried with the same digest, return the existing
result. Any different second response for the same interaction is a typed
conflict, not another resume. After the resumed executor has accepted and
persisted the tool result, transition `submitted -> acknowledged`. Terminal
states are `acknowledged`, `cancelled`, `expired`, `superseded`, and `failed`.

The requested lifecycle remains visible in events:

```text
pending -> presented -> partially_answered -> submitted -> acknowledged
    |           |               |                 |
    +-----------+---------------+-----------------+
          cancelled / expired / superseded / failed
```

Presentation and draft transitions may be coalesced to avoid write amplification,
but the final answer and every terminal transition are never coalesced.

## 4. Decision Tray and question wizard

### VS Code evidence

VS Code 1.132's released Agent Host state protocol is especially close to the
required separation. It defines accept/decline/cancel, typed text/number/
boolean/single-select/multi-select questions, stable question and option ids,
ordered questions, and answers keyed by question id
([released state types](https://github.com/microsoft/vscode/blob/df53daabb18cd157bdb08c7f01c34df936cf12f4/src/vs/platform/agentHost/common/state/protocol/channels-chat/state.ts#L275-L415)).
An unresolved input remains a typed transcript response part with `response`
absent until completion
([transcript state](https://github.com/microsoft/vscode/blob/df53daabb18cd157bdb08c7f01c34df936cf12f4/src/vs/platform/agentHost/common/state/protocol/channels-chat/state.ts#L930-L949)).
Its actions independently upsert a request while preserving drafts, change one
draft answer, and complete with a final response and optional answer replacement
([released actions](https://github.com/microsoft/vscode/blob/df53daabb18cd157bdb08c7f01c34df936cf12f4/src/vs/platform/agentHost/common/state/protocol/channels-chat/actions.ts#L731-L783)).
That request/upsert, answer-change, and complete split is a sound reducer/event
precedent for Nexa, but not a wire-compatibility target.

VS Code's built-in question tool preserves question order, validates the input
shape, assigns a stable internal id instead of trusting a display header as an
identity, appends a dedicated question-carousel progress object, and awaits its
completion
([tool schema](https://github.com/microsoft/vscode/blob/df53daabb18cd157bdb08c7f01c34df936cf12f4/src/vs/workbench/contrib/chat/common/tools/builtinTools/askQuestionsTool.ts#L93-L169),
[awaited interaction](https://github.com/microsoft/vscode/blob/df53daabb18cd157bdb08c7f01c34df936cf12f4/src/vs/workbench/contrib/chat/common/tools/builtinTools/askQuestionsTool.ts#L185-L273),
[stable question ids](https://github.com/microsoft/vscode/blob/df53daabb18cd157bdb08c7f01c34df936cf12f4/src/vs/workbench/contrib/chat/common/tools/builtinTools/askQuestionsTool.ts#L378-L427)).
Its carousel supports an idempotent `dismiss`, separates final data from a
deferred completion, and keeps draft answers/current index/collapsed state in
the runtime object
([carousel state](https://github.com/microsoft/vscode/blob/df53daabb18cd157bdb08c7f01c34df936cf12f4/src/vs/workbench/contrib/chat/common/model/chatProgressTypes/chatQuestionCarouselData.ts#L11-L61)).
The widget restores those draft fields after remount and supports
`Ctrl/Cmd+Enter`, Enter-based progression, arrow keys, Space, and numeric
selection
([draft restore](https://github.com/microsoft/vscode/blob/df53daabb18cd157bdb08c7f01c34df936cf12f4/src/vs/workbench/contrib/chat/browser/widget/chatContentParts/chatQuestionCarouselPart.ts#L190-L218),
[submit keys](https://github.com/microsoft/vscode/blob/df53daabb18cd157bdb08c7f01c34df936cf12f4/src/vs/workbench/contrib/chat/browser/widget/chatContentParts/chatQuestionCarouselPart.ts#L290-L312),
[draft updates](https://github.com/microsoft/vscode/blob/df53daabb18cd157bdb08c7f01c34df936cf12f4/src/vs/workbench/contrib/chat/browser/widget/chatContentParts/chatQuestionCarouselPart.ts#L319-L354),
[choice keyboard handling](https://github.com/microsoft/vscode/blob/df53daabb18cd157bdb08c7f01c34df936cf12f4/src/vs/workbench/contrib/chat/browser/widget/chatContentParts/chatQuestionCarouselPart.ts#L1295-L1318)).

### Boundary: the VS Code draft is deliberately transient

`ChatQuestionCarouselData.toJSON()` excludes draft answers, current index, and
collapsed state, retaining only serializable final data and presentation
metadata
([serialization](https://github.com/microsoft/vscode/blob/df53daabb18cd157bdb08c7f01c34df936cf12f4/src/vs/workbench/contrib/chat/common/model/chatProgressTypes/chatQuestionCarouselData.ts#L63-L78)).
This is a useful remount pattern but does not satisfy Nexa's app-restart
criterion. Nexa should reuse the interaction concepts, not copy that transient
storage boundary.

### Nexa UI contract

Render this ownership hierarchy:

```text
ChatPage
  Message timeline
  ConversationGateHost
    active DecisionTray / high-risk Modal trigger
    connection/retry surface
  Composer
```

The host selects requests from the conversation-scoped Interaction Store, which
hydrates from the backend and then applies ordered events. It does not scan
message artifacts to discover pending work. Switching conversations changes the
selector; returning reuses the persisted request and draft. Reconnect performs
a snapshot-plus-cursor reconciliation before applying new events.

Wizard behavior:

- default to one question at a time and show `current / total` plus `1 of N`
  for queued interaction requests;
- single choice and confirmation may advance after selection, while multiple
  choice requires Continue;
- text uses `Ctrl/Cmd+Enter` to advance; ordinary Enter remains available for
  multiline content where appropriate;
- Back never discards the current draft; the final step is Review, where every
  answer can be edited before one atomic submission;
- save a monotonically versioned draft after each semantic change, with a
  short debounce for typing and a flush on blur/unmount/window close;
- use local renderer storage only as a write-through crash buffer keyed by
  interaction id and draft revision; the SQLite row is authoritative;
- expose separate actions for answering the gate, adding ordinary context to
  the agent, and cancelling the task;
- after acknowledgement, remove the tray and render one compact, expandable
  timeline record. Redact secret-classified fields and do not retain a resume
  token in the artifact.

Only high-risk confirmation becomes a blocking modal. Narrow windows use a
bottom sheet. A normal user-input request remains visible above the composer
without taking over the entire app.

## 5. MCP elicitation is an adapter, not the internal protocol

### Current negotiated MCP `2025-11-25`

The official MCP `2025-11-25` specification defines `elicitation/create` as a
server-to-client request. Clients advertise form and/or URL support during
initialization, and servers must not send an unsupported mode
([capability negotiation](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/5f5440bb26a62e2cf3440b92da5a667efa03b267/docs/specification/2025-11-25/client/elicitation.mdx#L49-L77),
[request shape](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/5f5440bb26a62e2cf3440b92da5a667efa03b267/docs/specification/2025-11-25/client/elicitation.mdx#L79-L115)).
The JSON-RPC request id correlates the server request and client response
([form exchange](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/5f5440bb26a62e2cf3440b92da5a667efa03b267/docs/specification/2025-11-25/client/elicitation.mdx#L237-L275)).

The trust boundary is normative. Form mode must not request passwords, API
keys, access tokens, or payment credentials; URL mode is required for those
interactions. The client must identify the requesting server, offer clear
decline/cancel paths, allow review/edit before sending form data, and show the
target host before opening a URL
([security requirements](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/5f5440bb26a62e2cf3440b92da5a667efa03b267/docs/specification/2025-11-25/client/elicitation.mdx#L26-L47),
[URL boundary](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/5f5440bb26a62e2cf3440b92da5a667efa03b267/docs/specification/2025-11-25/client/elicitation.mdx#L330-L356)).

Immediate Nexa implication: do not add `elicitation` to the initialization
capabilities until `McpClient` can process nested server requests while a tool
call is outstanding. When enabled, namespace the durable source identity by
connector installation/server identity plus the JSON-RPC request id. Validate
the restricted schema into Nexa-owned question types, retain the original
schema for response validation, and answer the same server connection only
after the internal interaction reaches `submitted`.

### MCP `2026-07-28` multi-round request/response

The released MCP `2026-07-28` specification makes multi-round trip requests a
breaking replacement for the previous nested server-initiated request pattern.
The server terminates the initial request with an input-required result, and the
client later retries the original method under a new JSON-RPC id
([MRTR change and flow](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/5f5440bb26a62e2cf3440b92da5a667efa03b267/docs/specification/2026-07-28/basic/patterns/mrtr.mdx#L7-L52)).
It introduces `resultType: "input_required"`,
maps server-generated input request keys to client response keys, and carries
an opaque `requestState` that the client returns when retrying the original
operation
([result type](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/5f5440bb26a62e2cf3440b92da5a667efa03b267/schema/2026-07-28/schema.ts#L209-L235),
[input request/response maps](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/5f5440bb26a62e2cf3440b92da5a667efa03b267/schema/2026-07-28/schema.ts#L537-L568),
[opaque request state](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/5f5440bb26a62e2cf3440b92da5a667efa03b267/schema/2026-07-28/schema.ts#L571-L608)).
Its `ElicitResult` distinguishes accept, decline, and cancel and only includes
typed form content on accepted form responses
([result schema](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/5f5440bb26a62e2cf3440b92da5a667efa03b267/schema/2026-07-28/schema.ts#L3126-L3149)).

The released normative workflow further requires clients to echo
`requestState` exactly without inspecting or modifying it, to use a different
JSON-RPC id on retry, and to scope it only to that retried operation
([client requirements](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/5f5440bb26a62e2cf3440b92da5a667efa03b267/docs/specification/2026-07-28/basic/patterns/mrtr.mdx#L249-L257)).
Servers must treat the state as attacker-controlled, protect its integrity when
it affects authorization or business logic, bind it to principal/TTL/original
request, and add server-side single-use enforcement where needed
([server requirements](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/5f5440bb26a62e2cf3440b92da5a667efa03b267/docs/specification/2026-07-28/basic/patterns/mrtr.mdx#L221-L247)).

This newer shape is conceptually close to a durable resume flow, but Nexa does
not currently negotiate `2026-07-28`. Add it only in a separately tested MCP
protocol upgrade. Preserve `requestState` byte-for-byte as opaque, size-limited,
sensitive adapter state; never reinterpret it as Nexa's resume token. The
internal token authorizes a Nexa state transition, while MCP `requestState`
belongs to an untrusted remote server and is merely echoed on retry.

For either protocol generation:

- persist connector/server attribution and display it prominently;
- reject unsupported schemas with `decline` or `cancel`, never by guessing a
  form rendering;
- use URL mode for secrets and payments; internal `credential_request` should
  navigate to Nexa's credential vault/settings and persist only completion or
  cancellation, never the secret value;
- validate and sanitize all server text/Markdown, enforce string/option/schema
  size limits, and treat external URLs as untrusted navigation;
- cancel or fail the MCP request deterministically when the connector stops,
  the turn is cancelled, the request expires, or the app cannot restore the
  originating transport.

MCP Sampling is not a Wave 1 expansion target. The `2026-07-28` specification
marks Sampling deprecated and says new implementations should integrate with
LLM provider APIs instead
([deprecation](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/5f5440bb26a62e2cf3440b92da5a667efa03b267/docs/specification/2026-07-28/client/sampling.mdx#L7-L22)).
Nexa should preserve only negotiated legacy compatibility when needed; model
selection, credentials, policy, and execution remain in Nexa's provider and
capability layers.

## 6. Queueing, reconnect, and cross-conversation visibility

Persist a monotonically increasing `queueSequence` inside the same transaction
that creates a request. Select the visible request by:

```text
required/high-risk first
then risk_priority descending
then queue_sequence ascending
then interaction_id ascending as a deterministic tie-breaker
```

FIFO is preserved within each risk class. A superseding request creates a new
interaction id/generation and atomically marks the older request
`superseded`; it never mutates the old question set under the same id.

Conversation-list and task-center badges derive from a database query for
nonterminal interactions, not from whether a chat component is mounted. System
notifications carry only conversation/title/count metadata and never answers,
questions marked sensitive, or resume tokens.

`ConversationGateHost` can render both an interaction and connection state,
but ordering and completion remain typed:

- an active required interaction owns the primary tray;
- high-risk confirmation escalates to a modal;
- reconnect/degraded state stays visible as a compact status row and never
  changes the interaction status;
- reconnect success reconciles pending interactions from snapshot/cursor and
  removes only stale transport state;
- a submitted interaction remains submitted across reconnect and is not sent
  again unless the idempotent response API confirms the existing submission.

## Required tests and observability

### Core state-machine and database tests

- Every allowed transition succeeds; every skipped, backwards, or terminal
  transition fails with a stable code.
- Request creation, turn suspension, checkpoint persistence, and first event
  commit atomically. Inject a failure at each write and prove no half-created
  gate is visible.
- Duplicate `submissionId`, duplicate button click, repeated IPC delivery, and
  reconnect replay yield one response row, one resume event, and one tool
  result.
- A stale, wrong-conversation, expired, superseded, malformed, or wrong-
  generation resume token never advances the turn.
- App restart after `pending`, `partially_answered`, `submitted`, and immediately
  before `acknowledged` reconstructs the correct state without rerunning a
  completed tool.
- Cancellation, expiry, supersession, connector shutdown, turn interruption,
  and conversation deletion clean up the request and continuation according to
  foreign-key policy.
- Risk ordering and FIFO ordering remain stable under concurrent creation and
  clock skew.

### Agent/runtime tests

- `request_user_input` transitions the turn to `awaiting_user_input`, ends the
  active execution task, and emits no thinking, retry, assistant placeholder,
  or next provider call.
- Ordinary composer text adds context but cannot satisfy the interaction.
- A valid response resumes the original turn once with a canonical tool
  result, preserved provider/model/policy snapshot, and the same turn/run ids.
- Resumption after restart and resumption after network reconnect have the same
  provider-facing history.
- Parallel conversations and multiple queued interactions cannot cross-deliver
  answers.

### Frontend and accessibility tests

- Scrolling, timeline folding, tool-card unmount, conversation switching,
  route switching, reconnect, and full app restart preserve the tray and draft.
- The tray is structurally between the timeline and composer at desktop width;
  narrow width uses a bottom sheet and high risk uses the modal path.
- Single choice, multi-choice, confirm, short text, long text, Back, Continue,
  Review/edit, `Ctrl/Cmd+Enter`, Escape/cancel policy, arrow keys, Space, and
  focus restoration are covered with keyboard-only tests.
- `1 of N`, conversation badges, task-center `Waiting for you`, compact answered
  summaries, and reduced-motion behavior have focused Playwright assertions.
- Screen readers receive an announced state change and question position, while
  focus is never stolen repeatedly by reconnect or hydration replays.

### MCP conformance tests

- Do not advertise elicitation before the handler is enabled.
- Negotiate `2025-11-25` form-only, form+URL, and unsupported-mode fixtures;
  preserve request id and server identity through submit/decline/cancel.
- Reject nested/oversized/unsupported schemas and secret collection in form
  mode; show and validate the URL host before navigation.
- Add `2026-07-28` fixtures only with the protocol upgrade: input-request key
  matching, opaque `requestState` echo, retry, repeated `input_required`, and
  cancellation.

### Privacy-safe telemetry

Record state, duration, counts, queue depth, restart/reconnect recovery,
submission retries, conflicts, expiry, source kind, and sanitized failure code.
Do not record question/answer text, draft content, resume tokens or digests,
MCP request state, credential values, or external form payloads.

## Rollout order

1. Add the versioned Rust protocol, SQLite migrations/repository, transition
   validator, idempotent response transaction, and restart tests behind an
   `interaction_runtime_v1` feature flag.
2. Add the suspension control effect and checkpoint/resume path. Keep the old
   question artifact renderer available only as a rollback projection.
3. Add the frontend Interaction Store and `ConversationGateHost`, hydrate from
   a snapshot plus event cursor, and move `ConnectionStatusBanner` into the
   shared host without changing its state model.
4. Build the sequential Decision Tray wizard, durable drafts, Review, compact
   timeline summary, badges, task-center state, and accessibility/E2E coverage.
5. Migrate `request_user_input`; stop special-casing it in `ToolCallCard` and
   thinking rendering after parity tests pass.
6. Reuse the runtime for approvals/conflict resolution and add high-risk modal
   policy. Credential interactions route through secure settings/vault flows.
7. Add the MCP `2025-11-25` server-request dispatcher and elicitation adapter.
   Upgrade to MCP `2026-07-28` MRTR separately after negotiation and conformance
   fixtures are ready.
8. Remove the compatibility artifact path only after restart, duplicate-submit,
   and rollback telemetry show no lost or double-resumed interactions.

Wave 1 exits only when a pending request and its draft survive process restart,
one interaction cannot be submitted or resumed twice, the agent is genuinely
suspended rather than still thinking, the Decision Tray is independent from
the timeline/tool card, and the full local/remote validation matrix passes.

## License and integration boundaries

| Upstream | License at reviewed commit | Permitted design use | Integration boundary |
| --- | --- | --- | --- |
| OpenAI Codex release `rust-v0.146.1` plus current source `7a0e974e08c798d1e8d59d407aeb6e24db1313af` | [Apache-2.0](https://github.com/openai/codex/blob/79b4f03d35962b005b007a015113b38930711665/LICENSE) | Request correlation, blocking tool, resolved notification, FIFO replay, and stale-cleanup concepts | Implement independently in Nexa; do not copy the TUI/runtime implementation. If code is copied later, preserve license/notice obligations and mark modifications |
| LangGraph `1.2.10` at `41341457342327166d72fc11952ab28fb61ec0bf` | [MIT](https://github.com/langchain-ai/langgraph/blob/41341457342327166d72fc11952ab28fb61ec0bf/LICENSE) | Checkpoint-required interrupt/resume and replay-warning concepts | No Python dependency and no copied checkpointer; use Nexa SQLite/runtime contracts |
| VS Code `1.132.0` plus current carousel source | [MIT](https://github.com/microsoft/vscode/blob/df53daabb18cd157bdb08c7f01c34df936cf12f4/LICENSE.txt) | Stable question ids, typed state actions, wizard navigation, draft/remount, keyboard, and compact completion concepts | Do not copy workbench code, CSS, icons, or product-specific strings; implement with Nexa components and translations |
| MCP specification `2026-07-28` at `5f5440bb26a62e2cf3440b92da5a667efa03b267` | [Mixed MIT/Apache-2.0 transition; non-spec documentation CC-BY-4.0](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/5f5440bb26a62e2cf3440b92da5a667efa03b267/LICENSE) | Protocol interoperability for elicitation, capability negotiation, and multi-round input-required results | Implement only the negotiated version; do not vendor schemas/docs without applicable notices; treat server payloads and opaque state as untrusted |

All external claims in this note point to upstream source code or official
specifications pinned to immutable commits. No secondary summaries were used,
and no substantial upstream implementation should be copied merely because its
behavior informed this design.
