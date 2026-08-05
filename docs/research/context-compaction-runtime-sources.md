# Context compaction, operation runtime, database execution, and context projection: primary-source notes

Date: 2026-08-05

This note validates the Context Compaction, Operation Runtime, database-execution, and context-projection directions in `D:\Nexa.txt` against first-party documentation and immutable source snapshots. It is an engineering input, not an implementation record. Statements labeled **Nexa inference** are design conclusions drawn from the cited behavior; they are not claims that an upstream project implements the proposed Nexa contract.

## Executive decision

1. **Make compaction a runtime operation, not a long UI RPC.** Start must return an operation identity and snapshot boundary immediately; progress, cancellation, and the one terminal outcome must flow through the same canonical runtime event stream used by agent turns. Codex proves the value of immediate acknowledgement plus standard turn/item notifications, bounded ingress, and generated schemas, but its public `thread/compact/start` response is empty and its `contextCompaction` item contains only an `id`. Nexa therefore needs a richer operation contract rather than a literal copy. [Codex compact protocol](https://github.com/openai/codex/blob/5d89ab65dc9d4d0c55796c11df112b54157922b4/codex-rs/app-server/README.md#L786-L800), [Codex compact response type](https://github.com/openai/codex/blob/5d89ab65dc9d4d0c55796c11df112b54157922b4/codex-rs/app-server-protocol/src/protocol/v2/thread.rs#L988-L998), [Codex item type](https://github.com/openai/codex/blob/5d89ab65dc9d4d0c55796c11df112b54157922b4/codex-rs/app-server-protocol/src/protocol/v2/item.rs#L386-L397)
2. **Keep the canonical transcript immutable; compact only the model-context projection.** Pi's session storage is append-only and records compaction separately. Its current codebase contains two relevant formats: the interactive `coding-agent` keeps a `firstKeptEntryId`, while the newer agent harness materializes a self-contained `retainedTail`. Nexa should preserve the idea, but store a checkpoint plus a bounded, typed retention boundary or message references instead of blindly embedding every retained `AgentMessage`. [Pi append-only session manager](https://github.com/badlogic/pi-mono/blob/588915ec71714688cee8b7153339e8bdebb3e82e/packages/coding-agent/src/core/session-manager.ts#L844-L855), [coding-agent compaction projection](https://github.com/badlogic/pi-mono/blob/588915ec71714688cee8b7153339e8bdebb3e82e/packages/coding-agent/src/core/session-manager.ts#L410-L469), [harness retained tail](https://github.com/badlogic/pi-mono/blob/588915ec71714688cee8b7153339e8bdebb3e82e/packages/agent/src/harness/session/types.ts#L44-L51)
3. **Move synchronous rusqlite work off Tokio workers and put admission control in front of it.** A persistent writer lane on a dedicated thread is the best fit for long-lived serialized work; use a bounded Tokio channel, explicit overload/queue-deadline semantics, and per-request timing. Optional read connections can improve concurrency in WAL mode, but SQLite still permits only one writer at a time. [Tokio blocking-work guidance](https://github.com/tokio-rs/tokio/blob/108d6d3dc038332af2af83957748333091e35b3f/tokio/src/task/blocking.rs#L83-L149), [Tokio bounded channel contract](https://github.com/tokio-rs/tokio/blob/108d6d3dc038332af2af83957748333091e35b3f/tokio/src/sync/mpsc/bounded.rs#L111-L165), [SQLite WAL concurrency](https://www.sqlite.org/wal.html#concurrency)
4. **Adopt a versioned host/runtime seam without forcing a process split.** Current OpenHands separates Agent Canvas from Agent Server and can run the frontend and backend independently or connect one frontend to multiple local, remote, and cloud backends. Nexa should first make that boundary authoritative in-process; a later process or remote split should become a deployment decision, not another business-logic rewrite. [OpenHands Agent Canvas architecture](https://github.com/OpenHands/OpenHands/blob/bf2e37dcad66e0ce8e608034ba567cad5fd49ccd/README.md#L59-L80), [OpenHands Agent Server package contract](https://docs.openhands.dev/sdk/arch/agent-server#when-to-use-it)
5. **Cancellation needs a commit fence, not only an aborted future.** Codex's task wrapper cancels a token, waits a short grace interval, and aborts the task handle; rusqlite can interrupt a currently executing query from another thread. Neither mechanism alone guarantees that a late result cannot commit. Nexa must check cancellation, lease generation, and snapshot identity inside the final transaction before changing the active context pointer. [Codex task abort path](https://github.com/openai/codex/blob/5d89ab65dc9d4d0c55796c11df112b54157922b4/codex-rs/core/src/tasks/mod.rs#L850-L895), [rusqlite `InterruptHandle`](https://github.com/rusqlite/rusqlite/blob/cb6ad5c6cd7ea6e1ed21242a32f7b5bbf13fa1eb/src/lib.rs#L1294-L1311)

## Source snapshots and license boundaries

| Project | Verified snapshot | Current license at snapshot | Relevant surface |
| --- | --- | --- | --- |
| `openai/codex` | `5d89ab65dc9d4d0c55796c11df112b54157922b4` | Apache-2.0 ([license](https://github.com/openai/codex/blob/5d89ab65dc9d4d0c55796c11df112b54157922b4/LICENSE)) | app-server protocol, compaction lifecycle, bounded transport, turn interruption |
| `badlogic/pi-mono` | `588915ec71714688cee8b7153339e8bdebb3e82e` | MIT ([license](https://github.com/badlogic/pi-mono/blob/588915ec71714688cee8b7153339e8bdebb3e82e/LICENSE)) | `AgentSession`, JSONL session tree, compaction entries, `retainedTail` harness |
| `OpenHands/OpenHands` | `bf2e37dcad66e0ce8e608034ba567cad5fd49ccd` | MIT ([license](https://github.com/OpenHands/OpenHands/blob/bf2e37dcad66e0ce8e608034ba567cad5fd49ccd/LICENSE)) | Agent Canvas frontend/ingress and typed Agent Server adapters |
| `OpenHands/software-agent-sdk` | `0c8f97aab8a22d438bdea45ae3963e6050a9374c` | MIT ([license](https://github.com/OpenHands/software-agent-sdk/blob/0c8f97aab8a22d438bdea45ae3963e6050a9374c/LICENSE)) | SDK, Agent Server, workspace, remote conversation, leases |
| `rusqlite/rusqlite` | `cb6ad5c6cd7ea6e1ed21242a32f7b5bbf13fa1eb` (`rusqlite` 0.40.1) | MIT ([manifest](https://github.com/rusqlite/rusqlite/blob/cb6ad5c6cd7ea6e1ed21242a32f7b5bbf13fa1eb/Cargo.toml#L1-L13)) | synchronous `Connection`, transactions, query interruption |
| `tokio-rs/tokio` | `108d6d3dc038332af2af83957748333091e35b3f` (`tokio` 1.53.1) | MIT ([manifest](https://github.com/tokio-rs/tokio/blob/108d6d3dc038332af2af83957748333091e35b3f/tokio/Cargo.toml#L9-L15)) | blocking-work isolation and bounded MPSC |
| `programatik29/tokio-rusqlite` | `aa06eb79eafe798971aec44d7d19f3f498228495` (0.7.0) | MIT ([manifest](https://github.com/programatik29/tokio-rusqlite/blob/aa06eb79eafe798971aec44d7d19f3f498228495/Cargo.toml#L1-L11)) | dedicated-connection-thread reference implementation |
| `deadpool-rs/deadpool` / `deadpool-sqlite` | `85d34050e9f5e1b2363f96b34edac7706c86a9fe` (0.13.0) | MIT OR Apache-2.0 ([manifest](https://github.com/deadpool-rs/deadpool/blob/85d34050e9f5e1b2363f96b34edac7706c86a9fe/crates/deadpool-sqlite/Cargo.toml#L1-L11)) | bounded pool plus blocking-lane interaction reference |
| SQLite | official documentation current on 2026-08-05 | Public Domain ([official statement](https://sqlite.org/copyright.html)) | WAL concurrency, checkpointing, version safety |

`All-Hands-AI/OpenHands` is the historical owner spelling in the task; GitHub currently resolves the canonical project under `OpenHands/OpenHands`. The current architecture also puts the reusable runtime in `OpenHands/software-agent-sdk`, so both repositories are required to verify the boundary rather than relying on an older monorepo summary.

## 1. OpenAI Codex: protocol lifecycle, backpressure, and cancellation

### 1.1 What the app-server contract actually guarantees

The app-server is the rich-client boundary used by Codex interfaces. It exposes JSON-RPC over transports, explicitly documents bounded queues between ingress, request processing, and outbound writes, and assigns overload error `-32001` to saturated request ingress. It can generate TypeScript and JSON Schema matching the running Codex version. [app-server backpressure and schema](https://github.com/openai/codex/blob/5d89ab65dc9d4d0c55796c11df112b54157922b4/codex-rs/app-server/README.md#L49-L64)

`thread/compact/start` returns `{}` immediately. Compaction is represented as a turn, emits the normal `turn/*` and `item/*` lifecycle, and produces one `contextCompaction` item with `item/started` and `item/completed`. The protocol also exposes `turn/interrupt` for an in-flight turn. [compact example](https://github.com/openai/codex/blob/5d89ab65dc9d4d0c55796c11df112b54157922b4/codex-rs/app-server/README.md#L786-L800), [turn/item lifecycle and interrupt](https://github.com/openai/codex/blob/5d89ab65dc9d4d0c55796c11df112b54157922b4/codex-rs/app-server/README.md#L77-L84), [API overview](https://github.com/openai/codex/blob/5d89ab65dc9d4d0c55796c11df112b54157922b4/codex-rs/app-server/README.md#L194-L203)

The implementation maps the request directly to `Op::Compact` and only then returns the empty response. `turn/interrupt` validates the active turn and submits `Op::Interrupt`; the response is held until the abort lifecycle arrives. [compact request processor](https://github.com/openai/codex/blob/5d89ab65dc9d4d0c55796c11df112b54157922b4/codex-rs/app-server/src/request_processors/thread_processor.rs#L1876-L1888), [interrupt request processor](https://github.com/openai/codex/blob/5d89ab65dc9d4d0c55796c11df112b54157922b4/codex-rs/app-server/src/request_processors/turn_processor.rs#L1409-L1469)

Core task ownership is stronger than merely dropping a UI `await`: the session owns the running task and a cancellation token. On abort it cancels the token, waits up to 100 ms for cooperative settlement, aborts the Tokio task handle, then runs task-specific cleanup. [task cancellation contract](https://github.com/openai/codex/blob/5d89ab65dc9d4d0c55796c11df112b54157922b4/codex-rs/core/src/tasks/mod.rs#L176-L224), [running-task ownership](https://github.com/openai/codex/blob/5d89ab65dc9d4d0c55796c11df112b54157922b4/codex-rs/core/src/tasks/mod.rs#L308-L417), [abort implementation](https://github.com/openai/codex/blob/5d89ab65dc9d4d0c55796c11df112b54157922b4/codex-rs/core/src/tasks/mod.rs#L850-L895)

There is an important limit: `CompactTask::run` currently names its token `_cancellation_token` and does not inspect it. The outer session abort still terminates the task handle after the grace interval, but this is not evidence of phase-aware cancellation or a compaction-specific total deadline. [Codex `CompactTask`](https://github.com/openai/codex/blob/5d89ab65dc9d4d0c55796c11df112b54157922b4/codex-rs/core/src/tasks/compact.rs#L17-L84)

### 1.2 Bounded queues are explicit, but capacities and failure policy are transport-specific

Codex's internal transport capacity is 128. Requests that arrive when the ingress queue is full receive the overload error when possible; responses and notifications follow different waiting behavior. [transport capacity and overload](https://github.com/openai/codex/blob/5d89ab65dc9d4d0c55796c11df112b54157922b4/codex-rs/app-server-transport/src/transport/mod.rs#L22-L25), [ingress admission logic](https://github.com/openai/codex/blob/5d89ab65dc9d4d0c55796c11df112b54157922b4/codex-rs/app-server-transport/src/transport/mod.rs#L221-L257)

The current WebSocket writer uses a much larger bounded queue, 32,768 messages, and installs a disconnect token; the app-server disconnects a slow disconnectable connection once its writer queue fills. This proves that queue size and overload policy are transport/workload decisions, not reusable magic constants. [WebSocket queue](https://github.com/openai/codex/blob/5d89ab65dc9d4d0c55796c11df112b54157922b4/codex-rs/app-server-transport/src/transport/websocket.rs#L46-L49), [WebSocket connection ownership](https://github.com/openai/codex/blob/5d89ab65dc9d4d0c55796c11df112b54157922b4/codex-rs/app-server-transport/src/transport/websocket.rs#L175-L205), [slow-connection policy](https://github.com/openai/codex/blob/5d89ab65dc9d4d0c55796c11df112b54157922b4/codex-rs/app-server/src/transport.rs#L136-L168)

### 1.3 Borrowing boundary for Nexa

Borrow:

- one versioned protocol for Thread/Turn/Item or Resource/Operation/Item;
- immediate acknowledgement followed by canonical lifecycle events;
- generated Rust/TypeScript/JSON Schema contracts;
- bounded ingress with a typed overload response and retry guidance;
- cancellation owned by the runtime, not a page-local boolean;
- item start/completion as the authoritative state transition.

Do not copy:

- the empty compact response; Nexa needs `operationId`, `snapshotHighWatermark`, and initial state;
- the id-only `contextCompaction` item; Nexa needs phase, progress, fallback use, and terminal reason;
- Codex's capacities or slow-client disconnect policy without Nexa measurements;
- an assumption that the public contract promises durable restart recovery or a total compaction deadline. The cited contract does not specify either;
- the compact task's internal cancellation details as a substitute for a commit fence.

## 2. Pi: session ownership and non-destructive context projection

### 2.1 `AgentSession` owns lifecycle, persistence, compaction, and abort

Pi's `AgentSession` event surface includes `compaction_start` and `compaction_end`, with explicit reason, aborted state, retry intent, result, and error. The session stores separate abort controllers for manual and automatic compaction. [AgentSession events](https://github.com/badlogic/pi-mono/blob/588915ec71714688cee8b7153339e8bdebb3e82e/packages/coding-agent/src/core/agent-session.ts#L140-L180), [compaction state](https://github.com/badlogic/pi-mono/blob/588915ec71714688cee8b7153339e8bdebb3e82e/packages/coding-agent/src/core/agent-session.ts#L302-L335)

Manual `compact()` first aborts the current agent operation, creates an `AbortController`, emits start, prepares the boundary, and passes the signal through extension hooks and the summarizer. Before persistence it checks `signal.aborted`; only then does it append a compaction entry, rebuild the model context, and emit a terminal event. `abortCompaction()` aborts both manual and automatic controllers. [manual compact start](https://github.com/badlogic/pi-mono/blob/588915ec71714688cee8b7153339e8bdebb3e82e/packages/coding-agent/src/core/agent-session.ts#L1785-L1827), [commit and terminal event](https://github.com/badlogic/pi-mono/blob/588915ec71714688cee8b7153339e8bdebb3e82e/packages/coding-agent/src/core/agent-session.ts#L1866-L1929), [`abortCompaction`](https://github.com/badlogic/pi-mono/blob/588915ec71714688cee8b7153339e8bdebb3e82e/packages/coding-agent/src/core/agent-session.ts#L1932-L1938)

`AgentSessionRuntime` separately owns session replacement. It aborts and settles the active response, emits shutdown, invalidates the old session, then applies and rebinds the newly created runtime. This is a useful ownership seam for Nexa's runtime factory and conversation switching. [AgentSessionRuntime ownership](https://github.com/badlogic/pi-mono/blob/588915ec71714688cee8b7153339e8bdebb3e82e/packages/coding-agent/src/core/agent-session-runtime.ts#L67-L95), [replacement lifecycle](https://github.com/badlogic/pi-mono/blob/588915ec71714688cee8b7153339e8bdebb3e82e/packages/coding-agent/src/core/agent-session-runtime.ts#L167-L193)

### 2.2 Current Pi has two compaction entry shapes; both must be named accurately

The interactive coding-agent's `CompactionEntry` contains `summary`, `firstKeptEntryId`, `tokensBefore`, optional usage/details, and provenance. `SessionManager.appendCompaction()` adds that entry as a child of the current leaf; it does not rewrite older entries. Context construction finds the latest compaction, emits its summary, includes entries beginning at `firstKeptEntryId`, and then includes entries after the compaction. [coding-agent entry type](https://github.com/badlogic/pi-mono/blob/588915ec71714688cee8b7153339e8bdebb3e82e/packages/coding-agent/src/core/session-manager.ts#L69-L80), [`appendCompaction`](https://github.com/badlogic/pi-mono/blob/588915ec71714688cee8b7153339e8bdebb3e82e/packages/coding-agent/src/core/session-manager.ts#L1096-L1115), [context projection](https://github.com/badlogic/pi-mono/blob/588915ec71714688cee8b7153339e8bdebb3e82e/packages/coding-agent/src/core/session-manager.ts#L410-L469)

The newer `packages/agent` harness uses a different `CompactionEntry`: `summary`, materialized `retainedTail: AgentMessage[]`, `tokensBefore`, and optional metadata. Context projection replaces every entry through the latest compaction with the compaction entry, then expands the summary plus retained tail before applying later entries. [harness entry type](https://github.com/badlogic/pi-mono/blob/588915ec71714688cee8b7153339e8bdebb3e82e/packages/agent/src/harness/session/types.ts#L44-L51), [harness context projection](https://github.com/badlogic/pi-mono/blob/588915ec71714688cee8b7153339e8bdebb3e82e/packages/agent/src/harness/session/context.ts#L45-L99)

The harness compactor reconstructs a previous retained tail as virtual entries, selects a new cut, and stores the newly retained messages directly in the next compaction result. This makes the checkpoint self-contained with respect to the prior prefix. [harness preparation](https://github.com/badlogic/pi-mono/blob/588915ec71714688cee8b7153339e8bdebb3e82e/packages/agent/src/harness/compaction/compaction.ts#L595-L686), [harness compact result](https://github.com/badlogic/pi-mono/blob/588915ec71714688cee8b7153339e8bdebb3e82e/packages/agent/src/harness/compaction/compaction.ts#L706-L793)

Pi's own session-format documentation now describes both shapes: older/current coding-agent compatibility via `firstKeptEntryId`, and newer harness checkpoints via `retainedTail`. Therefore the statement “Pi uses retainedTail” is only correct when scoped to the harness; it is not the sole `AgentSession` session format. [Pi session-format compaction entries](https://github.com/badlogic/pi-mono/blob/588915ec71714688cee8b7153339e8bdebb3e82e/packages/coding-agent/docs/session-format.md#L229-L248), [context-building rules](https://github.com/badlogic/pi-mono/blob/588915ec71714688cee8b7153339e8bdebb3e82e/packages/coding-agent/docs/session-format.md#L320-L342)

### 2.3 Borrowing boundary for Nexa

Borrow:

- session-owned compaction lifecycle and explicit abort API;
- a separate compaction record rather than rewriting canonical messages;
- projection at context-build time: checkpoint summary + retained boundary + later messages;
- append-only history and branchable ancestry;
- a self-contained checkpoint option for restart and migration resilience;
- explicit compaction reasons and terminal outcomes.

Do not copy:

- Pi's JSONL tree as Nexa's database schema;
- the coding-agent's `firstKeptEntryId` without a snapshot hash/high-watermark check;
- unbounded materialization of `retainedTail`; large tool results, reasoning, media metadata, and secret-bearing payloads can make the checkpoint another large duplicate;
- model-only compaction without a deterministic fallback and a total deadline. The cited Pi method passes cancellation but does not establish Nexa's required fallback policy;
- Pi's feature scope. Nexa still needs desktop task state, knowledge, browser, terminal, media/office, approvals, and multi-operation scheduling.

**Nexa inference:** store `summary`, `snapshot_high_watermark`, `snapshot_hash`, token counts, and either retained message IDs plus a schema version or a strictly budgeted materialized tail. Reconstruct model context from that checkpoint plus canonical messages after the snapshot. The transcript table remains the user-visible historical truth.

## 3. OpenHands: frontend, server, runtime, and ownership boundaries

### 3.1 Current architecture, not the older monolith description

The current `OpenHands/OpenHands` repository presents Agent Canvas as a control center that connects to multiple agent backends. One frontend can switch between local, remote, and cloud Agent Servers, and the launcher can run frontend-only or backend-only. Its architecture section identifies Agent Server as a REST API for running multiple agents and Automation Server as a separate companion. [Agent Canvas backend model](https://github.com/OpenHands/OpenHands/blob/bf2e37dcad66e0ce8e608034ba567cad5fd49ccd/README.md#L33-L46), [split deployment](https://github.com/OpenHands/OpenHands/blob/bf2e37dcad66e0ce8e608034ba567cad5fd49ccd/README.md#L59-L80), [architecture section](https://github.com/OpenHands/OpenHands/blob/bf2e37dcad66e0ce8e608034ba567cad5fd49ccd/README.md#L124-L135)

The Agent Server package runs the Software Agent SDK behind HTTP and WebSocket APIs specifically so a Canvas backend or other service can start conversations, stream events, and operate a workspace without embedding the SDK in the same process. [official Agent Server architecture](https://docs.openhands.dev/sdk/arch/agent-server#when-to-use-it), [Agent Server source README](https://github.com/OpenHands/software-agent-sdk/blob/0c8f97aab8a22d438bdea45ae3963e6050a9374c/openhands-agent-server/openhands/agent_server/README.md#L1-L13), [REST and WebSocket endpoints](https://github.com/OpenHands/software-agent-sdk/blob/0c8f97aab8a22d438bdea45ae3963e6050a9374c/openhands-agent-server/openhands/agent_server/README.md#L296-L330)

Agent Canvas enforces the seam in code: an architecture test rejects direct shared Axios usage, low-level HTTP-client construction, and ad-hoc `/api/` fetches. The adapter uses the generated `@openhands/typescript-client` client types. [no-direct-agent-server-calls test](https://github.com/OpenHands/OpenHands/blob/bf2e37dcad66e0ce8e608034ba567cad5fd49ccd/src/api/no-direct-agent-server-calls.test.ts#L32-L76), [typed adapter imports](https://github.com/OpenHands/OpenHands/blob/bf2e37dcad66e0ce8e608034ba567cad5fd49ccd/src/api/agent-server-adapter.ts#L1-L28)

The server's `ConversationService` owns persisted conversation metadata and live runtimes, lazily hydrating an `EventService` when needed. It has an explicit maximum concurrent run count and lease TTL. [ConversationService](https://github.com/OpenHands/software-agent-sdk/blob/0c8f97aab8a22d438bdea45ae3963e6050a9374c/openhands-agent-server/openhands/agent_server/conversation_service.py#L563-L592)

Its conversation lease records an owner, monotonically increasing generation, and expiry. Claim can fence out a live owner or take over a stale/dead one; guarded writes re-check ownership and release only the matching generation. This is stronger than a bare per-conversation mutex because stale owners cannot safely write after takeover. [lease contract and claim](https://github.com/OpenHands/software-agent-sdk/blob/0c8f97aab8a22d438bdea45ae3963e6050a9374c/openhands-agent-server/openhands/agent_server/conversation_lease.py#L101-L164), [renew, guarded write, and release](https://github.com/OpenHands/software-agent-sdk/blob/0c8f97aab8a22d438bdea45ae3963e6050a9374c/openhands-agent-server/openhands/agent_server/conversation_lease.py#L187-L214)

The remote SDK combines WebSocket events with REST reconciliation. It treats a post-run WebSocket state snapshot as authoritative, polls REST as a health/failure fallback, and has a hard fallback so a missing final WebSocket snapshot cannot wait forever. This is a useful model for live-versus-durable frontend projection, not a reason to copy its exact timeout. [remote run contract](https://github.com/OpenHands/software-agent-sdk/blob/0c8f97aab8a22d438bdea45ae3963e6050a9374c/openhands-sdk/openhands/sdk/conversation/impl/remote_conversation.py#L1136-L1155), [authoritative WS plus REST fallback](https://github.com/OpenHands/software-agent-sdk/blob/0c8f97aab8a22d438bdea45ae3963e6050a9374c/openhands-sdk/openhands/sdk/conversation/impl/remote_conversation.py#L1191-L1215), [bounded terminal fallback](https://github.com/OpenHands/software-agent-sdk/blob/0c8f97aab8a22d438bdea45ae3963e6050a9374c/openhands-sdk/openhands/sdk/conversation/impl/remote_conversation.py#L1226-L1315)

### 3.2 Borrowing boundary for Nexa

Borrow:

- a generated, typed adapter as the only frontend route to runtime functions;
- a host/runtime boundary independent of where the runtime process lives;
- lazy per-conversation runtime hydration;
- event-stream plus durable-state reconciliation;
- per-conversation ownership with generation fencing;
- an independent automation/job backend rather than placing scheduling in the chat page.

Do not copy:

- a mandatory HTTP/WebSocket or multi-process split for the first Nexa refactor;
- file-based TTL leases inside a single-process desktop app;
- OpenHands' exact run, poll, or fallback timeouts;
- the assumption that remote and local hosts have identical security boundaries.

**Nexa inference:** define the Rust protocol and service interfaces as if the desktop were calling a server, but initially use an in-process adapter. Persist operation and lease generation in SQLite; use an in-memory per-conversation guard for fast exclusion and a transactional generation/snapshot check for correctness.

## 4. rusqlite and Tokio: the database execution layer

### 4.1 Constraints verified in source

`rusqlite::Connection` is a synchronous object backed by a `RefCell<InnerConnection>` and is explicitly `Send`; it is not an async database executor. Sharing one connection behind `std::sync::Mutex` and calling it directly from async commands therefore moves the blocking problem into Tokio's worker threads. [rusqlite `Connection`](https://github.com/rusqlite/rusqlite/blob/cb6ad5c6cd7ea6e1ed21242a32f7b5bbf13fa1eb/src/lib.rs#L360-L369)

Tokio's official guidance says blocking work must not run in ordinary futures. `spawn_blocking` is for bounded work that eventually finishes; long-lived processing loops should use dedicated threads. The blocking-pool limit is large by default, excess work queues after the limit, and already-started blocking tasks cannot be aborted. [Tokio `spawn_blocking`](https://github.com/tokio-rs/tokio/blob/108d6d3dc038332af2af83957748333091e35b3f/tokio/src/task/blocking.rs#L83-L135)

Tokio's bounded MPSC provides backpressure: once capacity is full, senders wait until capacity returns. This is the appropriate primitive for explicit DB admission; cancellation/timeout can race the `send` or reservation before work enters the writer lane. [Tokio bounded MPSC](https://github.com/tokio-rs/tokio/blob/108d6d3dc038332af2af83957748333091e35b3f/tokio/src/sync/mpsc/bounded.rs#L111-L165)

`tokio-rusqlite` is a useful small reference for thread ownership: each connection gets a dedicated thread, requests are closures sent to that thread, and results return through a one-shot channel. However, its current implementation uses `crossbeam_channel::unbounded`, so copying it directly would violate Nexa's bounded-ingress requirement. [tokio-rusqlite design](https://github.com/programatik29/tokio-rusqlite/blob/aa06eb79eafe798971aec44d7d19f3f498228495/src/lib.rs#L1-L15), [`call` request/reply](https://github.com/programatik29/tokio-rusqlite/blob/aa06eb79eafe798971aec44d7d19f3f498228495/src/lib.rs#L265-L305), [unbounded channel and event loop](https://github.com/programatik29/tokio-rusqlite/blob/aa06eb79eafe798971aec44d7d19f3f498228495/src/lib.rs#L374-L428)

`deadpool-sqlite` shows the alternative read-pool shape: pool objects contain a `SyncWrapper<rusqlite::Connection>`, and each `interact` closure runs through the runtime's blocking lane while serializing access to that connection. This is evidence for bounded additional connections, not evidence that SQLite supports parallel writers. [deadpool-sqlite manager](https://github.com/deadpool-rs/deadpool/blob/85d34050e9f5e1b2363f96b34edac7706c86a9fe/crates/deadpool-sqlite/src/lib.rs#L48-L99), [`SyncWrapper::interact`](https://github.com/deadpool-rs/deadpool/blob/85d34050e9f5e1b2363f96b34edac7706c86a9fe/crates/deadpool-sync/src/lib.rs#L66-L140)

SQLite WAL allows readers and a writer to proceed concurrently, but there is still only one writer at a time. Long readers can block checkpoint completion, and automatic checkpoints can make an occasional commit much slower. [SQLite WAL concurrency](https://www.sqlite.org/wal.html#concurrency), [checkpoint performance](https://www.sqlite.org/wal.html#performance_considerations)

The current SQLite WAL documentation also records a WAL-reset corruption bug fixed in SQLite 3.51.3 and selected backports; it affected multiple connections in separate threads/processes when a checkpoint and writer reset raced. Any Nexa move from one connection to a read pool must first verify the actually linked SQLite version. [SQLite WAL-reset bug](https://www.sqlite.org/wal.html#the_wal_reset_bug)

rusqlite exposes `InterruptHandle`, which is `Send + Sync` and can cause a query running on another thread to fail with `SQLITE_INTERRUPT`. It can help cancel long-running queries, but it does not remove the need for short transactions and a final commit fence. [rusqlite interrupt handle creation](https://github.com/rusqlite/rusqlite/blob/cb6ad5c6cd7ea6e1ed21242a32f7b5bbf13fa1eb/src/lib.rs#L1032-L1037), [interrupt implementation](https://github.com/rusqlite/rusqlite/blob/cb6ad5c6cd7ea6e1ed21242a32f7b5bbf13fa1eb/src/lib.rs#L1294-L1311)

### 4.2 Recommended Nexa execution model

This is a **Nexa inference** from the constraints above:

```text
async caller
  -> bounded admission (capacity + queue deadline + cancellation)
  -> DatabaseExecutor
       writer lane: one persistent dedicated thread + one writer connection
       read lane: small bounded pool of read connections, only after WAL/version validation
  -> per-request oneshot result
```

Required behavior:

- use a bounded writer queue; overload is a typed retryable error, never silent growth;
- record `queued_at`, `started_at`, `finished_at`, queue wait, execution time, transaction hold time, and queue depth;
- use a single writer lane and short transactions;
- never perform network/model awaits inside a database closure or transaction;
- allow a queue deadline to cancel before admission;
- keep an `InterruptHandle` for long-running SQL where interruption is safe and map `SQLITE_INTERRUPT` to operation cancellation;
- after DB work starts, cancellation prevents publication/commit of stale operation state even if the blocking closure returns later;
- add read connections only after checking the linked SQLite version and testing WAL/checkpoint behavior on all supported platforms;
- do not expose raw `Connection`, mutex guards, or database implementation types above repositories.

## 5. Proposed authoritative Nexa contracts

### 5.1 Operation runtime

Start response:

```rust
struct StartContextCompactionResponse {
    operation_id: OperationId,
    conversation_id: ConversationId,
    snapshot_high_watermark: MessageId,
    snapshot_hash: ContentHash,
    state: OperationState, // queued | running
}
```

Canonical lifecycle:

```text
operation/queued
operation/started
operation/progress(stage, elapsed, attempt, queue_wait?)
item/started(contextCompaction)
item/completed(contextCompaction)
operation/completed | operation/cancelled | operation/failed
```

Every operation must have exactly one terminal event. Cancellation changes state to `cancelling`, cancels provider work, interrupts safe DB queries, and prevents the active checkpoint pointer from changing. Deadline is one absolute monotonic budget shared by attempts; retries cannot reset it.

Suggested compact stages, based on the work described in `D:\Nexa.txt`:

```text
load_snapshot
plan_boundary
build_fallback
provider_summary
validate_summary
commit_checkpoint
publish_projection
```

Codex supports the shared-lifecycle direction; Pi supports compaction-specific terminal metadata; OpenHands supports lease generation and durable/live reconciliation. Nexa needs all three properties in one operation abstraction.

### 5.2 Context checkpoint projection

```text
messages
  canonical transcript; never deleted or rewritten by compaction

context_compactions
  id
  conversation_id
  operation_id
  snapshot_high_watermark
  snapshot_hash
  summary
  retained_boundary_version
  retained_message_ids / bounded_retained_tail
  tokens_before
  tokens_after
  provider/model/usage
  source = extractive | abstractive
  status
  created_at

conversations
  active_context_compaction_id
```

The commit transaction should insert one immutable checkpoint and compare-and-swap the active pointer only if:

- the operation still owns the conversation lease generation;
- cancellation has not been requested;
- the snapshot high-watermark and hash still match the planned snapshot;
- the checkpoint payload validates and remains within storage/context budgets.

Model input becomes:

```text
stable system/tool/source context
+ latest valid context checkpoint
+ canonical messages after snapshot_high_watermark
```

The UI transcript continues to read canonical messages. Restore/fork changes the selected projection or branch; it does not replace historical rows.

### 5.3 Deterministic fallback

None of the reviewed upstream boundaries is sufficient evidence for making successful LLM summarization a liveness dependency. Nexa should construct a bounded extractive checkpoint before the provider call, then replace it with an abstractive result only if the result arrives before the total deadline and passes validation. Timeout, cancellation, rate limit, and transient network failure must either complete with the safe fallback or produce a typed retryable terminal state according to explicit policy.

The fallback must preserve, in typed/budgeted sections:

- current objective and user constraints;
- unresolved approvals and task state;
- file paths, symbols, errors, and executed command outcomes;
- pending tool-call/result pairing;
- source and attachment references needed to reconstruct authorized context;
- recent complete turns up to a deterministic budget.

## 6. Acceptance tests and architecture gates

### Runtime liveness

- conversation A can compact while conversation B streams and settings/Task Center continue reading;
- saturated operation or DB queues return the documented overload result within a bounded time;
- a provider that never yields hits the absolute compact deadline;
- cancellation during every stage produces one terminal event and never changes the active checkpoint;
- double-start on one conversation is idempotent or returns the existing lease holder;
- restart marks orphaned operations retryable/failed or resumes them according to kind; no operation remains permanently `running`.

### Context integrity

- canonical transcript IDs and content hashes are identical before and after compaction;
- new messages arriving after the snapshot either remain delta messages or supersede the operation; they are never deleted;
- repeated compactions rebuild from checkpoint plus delta without losing objective, constraints, tool pairing, or source references;
- retained-tail storage remains within a hard byte/token cap even for multi-megabyte tool results;
- legacy checkpoint/archived-message data remains readable through a versioned migration adapter.

### Database execution

- no async command directly locks or executes a rusqlite connection;
- no network `await` can occur inside a repository transaction closure;
- writer queue capacity, wait time, execution time, and transaction hold time are observable;
- a cancelled queued write never begins; a cancelled running query uses interruption where safe and cannot publish stale state;
- read-pool tests cover checkpoint starvation, `SQLITE_BUSY`, shutdown, and the actual bundled/system SQLite version;
- CI rejects unbounded runtime ingress and raw database access outside the store implementation.

## Bottom line

The upgrade direction in `D:\Nexa.txt` is supported, with four precision corrections:

1. Codex is the protocol/backpressure reference, not a complete compact-job specification; Nexa must add operation identity, detailed progress, deadline, durability, and commit fencing.
2. Pi validates non-destructive session projection, but current Pi has both `firstKeptEntryId` and harness `retainedTail` formats. Nexa should adopt the checkpoint principle, not conflate or copy the formats.
3. OpenHands now demonstrates Agent Canvas/Agent Server/SDK separation and generation-fenced conversation ownership. Nexa can adopt the seam in-process before considering a process split.
4. A dedicated DB thread or bounded blocking lane solves async-runtime starvation only when admission is bounded. `tokio-rusqlite`'s dedicated-thread idea is useful, but its unbounded channel is specifically unsuitable as Nexa's final executor.
