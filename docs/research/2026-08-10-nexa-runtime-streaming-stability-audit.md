# Nexa runtime and streaming stability audit

Date: 2026-08-10

Status: evidence-backed diagnosis followed by implementation on `fix/runtime-streaming-stability`

## Executive conclusion

The reported failures are not one bug. Current Nexa combines at least four independent failure chains that amplify one another:

1. A background source change can start an unrestricted local embedding job inside the desktop process. A live sample reached 5.6 GiB working set, 6.37 GiB private memory, and approximately sixteen fully busy CPU threads. Once the job completed, the same process returned to about 425 MiB and idle CPU. This is a confirmed resource-starvation mechanism.
2. Live provider output is persisted and presented at a nominal 20 Hz, while the frontend adds a second 30 ms typewriter clock, full thinking-Markdown parsing, synchronous scroll layout, and motion layout. DeepSeek V4 Flash produces far more thinking blocks than the comparison models, so it exercises this path most severely.
3. The Responses adapter parses the terminal aggregate before it reconciles already streamed function arguments. It also handles `response.completed` and `response.incomplete` in the same branch and applies exact byte-prefix reconciliation to provider terminal text. This turns compatible provider variation or expected truncation into generic terminal errors.
4. Retry, fallback, terminal classification, compatibility heuristics, and error rendering are split across several modules and representations. The safety policy after visible output is correct, but the architecture makes that policy difficult to reason about and produces messages such as `LLM error: LLM error: ...`.

The immediate stability work should therefore be treated as a P0/P1 runtime program, not another isolated DeepSeek patch.

## Implemented outcome

The implementation following this audit replaced the confirmed hot paths instead of adding provider-specific patches:

- `BackgroundWorkGovernor` now owns watcher-job deduplication, foreground pause, obsolete-generation cancellation, and single-flight admission. Embedding queries are source-scoped, batches are bounded to eight by default, ONNX intra-op work is capped at two threads, and each batch commits immediately.
- `ResponsesAssembler` now treats argument deltas as provisional. Client tool arguments reach the agent only after `response.function_call_arguments.done`, `response.output_item.done`, or a compatible completed terminal snapshot proves that they are a valid JSON object. Incomplete calls remain non-executable.
- `ProviderStreamFailure` preserves terminal error type across adapter and agent seams. The exact reported message now renders once instead of becoming `LLM error: LLM error: ...`.
- OpenAI-compatible adapters now perform one wire attempt. The agent's pre-visible-output attempt controller owns same-route retries, automatic fallback owns route changes, and any visible output irreversibly disables transparent replay.
- The Run Event producer queue is bounded. Live answer/thinking/usage projection is decoupled from a journal that flushes in bounded transactions, while tool/status/terminal boundaries still persist before presentation.
- The frontend deleted the artificial typewriter clock, active-stream layout animation, the second animation-frame scheduler, and streaming-time full Markdown parsing for thinking. StreamStore is the sole presentation clock; terminal thinking is parsed as Markdown only after the hot streaming phase.

Validation on this Windows host:

- all desktop TypeScript contracts and the production Vite build passed;
- 1,649 of 1,650 executed core tests passed, with nine ignored; the one stable failure is an unchanged Windows GNU temporary-workspace path assertion outside this patch;
- 132 of 134 desktop Rust tests passed in the minimal GNU build; the two stable failures are unchanged provider-helper assertions (IPv6 loopback URL representation and legacy provider-sniffing expectation);
- every new focused contract for source scoping, foreground governance, Responses completion gates, single wire attempts, visible-output fallback suppression, bounded outbox failure, batched journal classification, and live transcript projection passed.

The full default-feature Windows/MSVC build remains a remote GitHub Actions gate because this workstation does not expose the MSVC linker and the ONNX dependency does not publish the required GNU binary.

## Scope and evidence

This audit used only read-only product/process/database inspection, existing tests, repository source, Windows Error Reporting artifacts, and primary-source upstream research. It did not stop the application, mutate the live database, change provider settings, or edit product code.

### Installed product and failure timeline

- The running executable was `C:\Program Files\Nexa\nexa-desktop.exe`, version `0.12.8.0`.
- Windows recorded an `AppHangB1` for Nexa at 2026-08-10 11:40:35 local time. The report identifier was `38e47364-5544-496c-9b4f-cbb593b0908b`.
- The WER snapshot recorded Nexa at about 813 MiB working set with a peak near 2.48 GiB. The relevant WebView2 renderer and GPU processes also had material CPU and memory histories.
- After restart, a later live source-indexing sample captured Nexa at about 5.6 GiB working set and 6.37 GiB private memory. Sixteen threads each consumed approximately a full core over a two-second sample. After embedding completed, working set returned to about 425 MiB and CPU returned to idle.
- The WER artifact proves a real application hang. It does not contain enough symbolic stack evidence to claim that one particular Rust call caused the 11:40 hang. The later live sample independently proves that the current runtime can starve the desktop in exactly the observed manner.

### Provider runs

The live database contained two adjacent failed DeepSeek V4 Flash runs:

| Failure | Duration | Events | Event rate | Output composition | Terminal error |
|---|---:|---:|---:|---|---|
| Run A | 58 s | 756 | 13.0/s average, 71/s peak | 681 thinking blocks, 20 answer blocks | `Responses function_call contained incomplete arguments` |
| Run B | 22 s | 261 | 11.9/s average, 68/s peak | 218 thinking blocks, 20 answer blocks | `Responses thinking deltas did not match the completed response` |

The incomplete function call was a `spawn_subagent` call whose streamed argument text ended mid-string. Nexa did not execute that call. Keeping the visible partial response and suppressing an automatic resend after visible output was the correct safety decision.

Successful comparison runs showed the provider-dependent load:

| Route | Duration | Events | Average event rate | Thinking blocks | Answer blocks |
|---|---:|---:|---:|---:|---:|
| DeepSeek V4 Flash | 212 s | 2,815 | 13.3/s | 1,964 | 803 |
| DeepSeek V4 Pro | 52 s | 361 | 6.9/s | 131 | 180 |
| Alibaba/Kimi route | 372 s | 891 | 2.4/s | 344 | 398 |

This explains why Flash is the clearest failure even though other providers can also feel jerky: the shared runtime path is inefficient, and Flash produces enough thinking deltas to saturate it.

### Existing checks

- `npm run test:streaming` in `apps/desktop` passed the full frontend streaming contract suite. The previously fixed ordering-gap, UTF-8 offset, run-identity, terminal, and frame-notification contracts remain green.
- The merged remote PR checks for the current DeepSeek Responses lifecycle branch were green.
- A focused local Rust test run could not start because the machine does not expose the MSVC `link.exe`. This is an environment limitation, not evidence that the Rust tests fail.

## Finding 1: background embedding can starve the desktop process

Severity: P0 for responsiveness; P1 for data/index correctness and operability

### Confirmed mechanism

All ten registered sources currently have watching enabled. Some roots overlap, and broad include globs allow internal artifacts such as `.git` checkpoint paths to enter the index.

The watcher runs a synchronous loop in the desktop process. For each debounced group it ingests changed files and then calls `embed_source` directly (`apps/desktop/src-tauri/src/commands.rs:399-485`). There is no foreground-aware scheduler, cancellation token, resource budget, or global single-flight guard at this seam.

The local E5 embedder chooses half of all available CPUs for ONNX intra-op parallelism (`crates/core/src/embed.rs:857-871`). On this machine that became sixteen fully occupied threads.

`embed_source` uses batches of 64 and retains every generated vector until the entire job finishes (`crates/core/src/ingest.rs:582-608`). Each E5 inference materializes a last-hidden-state tensor shaped approximately `[batch, sequence, hidden]` before pooling (`crates/core/src/embed.rs:924-1040`). The live process sample demonstrated the resulting multi-gigabyte transient.

The method is nominally source-scoped but calls `get_chunks_without_embeddings(model)`, whose SQL has no source predicate (`crates/core/src/db.rs:272-292`). Today that often converges to the desired global state, but its interface promise and implementation do not match. Any source event may absorb missing work from every source.

### Required design

Create a deep `BackgroundWorkGovernor` module. Its interface should accept source-scoped jobs with priority, cancellation, memory/CPU budgets, and progress checkpoints. The internal implementation should own:

- deduplication and single-flight behavior across overlapping watchers;
- foreground preemption and pause/resume;
- a small configurable ONNX thread budget;
- adaptive micro-batches based on model and available memory;
- source-scoped missing-chunk queries;
- incremental embedding commits after each bounded batch;
- durable resumability without retaining the whole job in memory.

The file watcher should report intent to this module. It must not execute inference itself.

### Deletion candidates

- Delete direct `embed_source` execution from the watcher thread.
- Delete the default assumption that every registered source should be watched recursively.
- Delete duplicate native watches for nested roots; retain logical source scope in the database if the user needs it.
- Delete broad indexing of implementation artifacts by adding formal default exclusions for `.git`, dependency caches, build outputs, temporary files, and Nexa/Codex checkpoint internals.
- Delete global missing-chunk selection from the source-scoped embedding interface.
- Delete whole-job vector accumulation.

These are code-path deletions. Do not delete the user's source records, source files, live database, SQLite journal, or validated index data as an emergency workaround.

## Finding 2: the presentation pipeline has multiple competing clocks

Severity: P1

### Confirmed mechanism

The backend coalesces text and thinking output on a 50 ms interval (`apps/desktop/src-tauri/src/agent_stream_bridge.rs:21-145`). That is a reasonable presentation cadence by itself.

The frontend then adds several more expensive operations:

- the store schedules notifications on animation frames (`apps/desktop/src/lib/streamStore.ts`);
- live answer text uses a typewriter interval of 30 ms (`apps/desktop/src/features/chat/ChatMessages.tsx:804` and `apps/desktop/src/lib/useTypewriter.ts`);
- live timeline projection is recomputed as displayed text, raw stream text, thinking text, tool state, and trace events change (`ChatMessages.tsx:1592-1614`);
- live items use Framer Motion layout positioning (`ChatMessages.tsx:2422-2438`);
- the thinking panel reparses the complete accumulated Markdown and performs a synchronous `scrollHeight` read plus `scrollTop` write on content change (`apps/desktop/src/components/chat/ThinkingBlock.tsx:177-183,254-279`).

The answer path has a 150 ms Markdown throttle for longer content. The thinking path does not. In the failed Flash run, 681 of 701 output blocks were thinking blocks, so the most frequent channel uses the least protected rendering path.

A pure reducer/projection microbenchmark did not reproduce a hang at the observed three-thousand-event scale. The cost appears when React reconciliation, Markdown parsing, layout reads/writes, animation, and GPU work are added. This is consistent with the live WebView2/GPU samples and provider severity gradient.

### Required design

Create one deep `LiveTranscriptProjection` module and one presentation clock. Provider events should update an in-memory semantic projection; the UI should consume at most one snapshot per animation frame, with adaptive backpressure when a frame misses its budget.

During active streaming:

- render thinking as escaped/plain incremental text or an incremental parser output;
- defer full Markdown parsing until a bounded throttle or semantic boundary;
- use an anchored tail without synchronous layout reads on every chunk;
- disable layout animation for the actively changing item;
- do not typewrite content that is already arriving incrementally.

### Deletion candidates

- Delete the typewriter clock from already-streamed answer content.
- Delete active-stream layout animation.
- Delete full accumulated-thinking Markdown parsing on each chunk.
- Delete per-chunk synchronous auto-scroll layout.
- Delete duplicate projections of raw stream text and displayed typewriter text.

## Finding 3: durable event storage sits in the presentation critical path

Severity: P1

### Confirmed mechanism

Each coalesced stream block enters the run outbox. The outbox increments sequence, waits for `save_agent_run_event`, and only then emits the event to the frontend (`apps/desktop/src-tauri/src/agent_run_outbox.rs`). `save_agent_run_event` performs one `INSERT OR REPLACE` call (`crates/core/src/db.rs`). A batch method already exists, but the live outbox does not use it.

The 50 ms coalescing limit therefore bounds token amplification but still allows approximately twenty awaited SQLite transactions per second per active stream. The live database contained 524,823 run-event rows and roughly 261 MB of run-event JSON inside a database near 1.97 GB.

The installed runtime reported `journal_mode=delete` and `synchronous=FULL` during read-only inspection, while the current source configures WAL/NORMAL for newly configured connections. That runtime/source mismatch must be explained before changing database pragmas. It is not safe to toggle the live database mode or delete journals casually.

### Required design

Split the current seam into:

- `LiveEventBus`: ephemeral ordered presentation updates, bounded and frame-coalesced;
- `RunEventJournal`: durable semantic checkpoints, batched in one transaction by time/byte/semantic thresholds;
- a mandatory terminal/tool-boundary flush that preserves replay correctness.

The journal remains authoritative for recovery. A display frame does not need its own durable row.

### Deletion candidates

- Delete one durable SQLite row/transaction per presentation block.
- Delete disk-commit latency from the frontend emission path.
- Delete unbounded retention of low-value fine-grained projection deltas after a run has a validated compact snapshot.

Do not delete ordered sequence numbers, durable terminal events, recovery prefixes, or run identity validation.

## Finding 4: Responses terminal assembly rejects recoverable provider variation

Severity: P1

### Confirmed mechanism

The terminal handler treats `response.completed` and `response.incomplete` together (`crates/core/src/llm/openai.rs:2057-2097`). It first calls strict `parse_responses_completion`, which rejects a `function_call` unless terminal `arguments` are already a valid JSON object (`openai.rs:1412-1453`). Only after that parse succeeds does it attempt to reconcile the arguments accumulated from streamed events.

This order prevents valid streamed state from completing or diagnosing an incomplete terminal aggregate.

Thinking and answer reconciliation also require the terminal string to have the streamed string as an exact byte prefix (`openai.rs:1929-1945`). Providers and gateways may normalize whitespace, Unicode, or aggregate fields without violating the semantic response lifecycle. The previous failed run hit this exact-prefix rule.

### Required design

Create a deep `ResponsesAssembler` module. Its interface should consume typed Responses events keyed by response, item, content, and call identifiers and produce exactly one typed semantic terminal outcome:

- `Completed(validated_response)`;
- `Incomplete(partial, reason)`;
- `Cancelled(reason)`;
- `ProtocolViolation(diagnostic)`;
- `TransportInterrupted(partial, diagnostic)`.

The assembler should own incremental item state and reconcile streamed function arguments before final JSON/schema validation. Tool execution remains forbidden until the item is semantically complete and the final argument object validates.

`response.incomplete` must become a typed truncation outcome, not share the same generic error path as an invalid `response.completed`. Semantic equality rules should be dialect-aware and ID/event-based; exact byte-prefix matching should be a diagnostic, not the universal authorization condition.

### Deletion candidates

- Delete the combined `response.completed | response.incomplete` terminal branch.
- Delete strict terminal-first parsing of function arguments.
- Delete universal exact byte-prefix reconciliation.
- Delete generic string-only terminal classification.

Keep fail-closed tool dispatch and the no-transparent-retry rule after visible output.

## Finding 5: provider compatibility is concentrated but not deep

Severity: P1 architectural risk

The public `LlmProvider` interface exposes both `stream` and `stream_events`; the latter defaults to converting the former (`crates/core/src/llm/mod.rs:634-703`). Wrappers must forward both methods. This is a migration seam that has become a permanent dual protocol.

There are seventeen public provider types but four concrete wire adapters. Reuse is desirable, but the 5,546-line `openai.rs` currently owns Chat Completions, Responses, DeepSeek/Kimi/Qwen/Alibaba variations, native search, SSE decoding, terminal assembly, replay, retry helpers, non-streaming fallback, request shaping, and model/base-URL heuristics.

This is a shallow module: its interface looks simple, but the complexity leaks into callers, provider catalogs, route selection, fallback wrappers, and tests.

### Required design

Use explicit immutable dialect profiles selected before request construction:

```text
Provider catalog + endpoint
          |
          v
   DialectProfile
          |
          v
 ProviderAdapter.start(request, route)
          |
          v
 typed ProviderEvent stream
```

The adapter should be a deep module: a small stable interface hiding request shape, wire event parsing, semantic assembly, and replay requirements for one dialect. Model-name and base-URL guesses must not silently change protocol authorization.

After every concrete adapter produces typed events, delete the agent-facing legacy `stream()` path. If chunk streams remain useful internally, keep them behind the adapter boundary.

## Finding 6: retry and fallback ownership is fragmented

Severity: P1 architectural risk

Retry-like behavior currently exists in several places:

- OpenAI-compatible completion retries;
- automatic cross-route fallback (`llm/fallback.rs`);
- model-step connect and transient retries;
- stream reconnect and non-streaming fallback (`agent/model_step.rs`);
- reasoning-disabled safe restart.

The individual safety guards are often good. Automatic fallback correctly forbids mixing a fallback after output has been exposed. The problem is ownership: no one module can state the complete attempt lifecycle and prove that budgets, visible-output state, tool execution, replay, and route provenance agree.

Create one deep `TurnAttemptController`. It should own attempt number, route, retry budget, fallback eligibility, visible-output latch, semantic terminal state, tool-executed latch, and durable route provenance. Adapters report typed outcomes; they do not independently decide agent-level retries.

Delete duplicated retry decisions from lower layers after the controller owns them. Keep narrowly scoped transport retries only when they are provably pre-request/pre-output and surfaced to the controller as one attempt state.

## Finding 7: errors lose type information and duplicate prefixes

Severity: P2 presentation; P1 diagnostics

Legacy chunk errors are converted to `ProviderStreamEvent::TerminalError { message: error.to_string() }` (`crates/core/src/llm/mod.rs:471-485`). The model step then constructs a new `CoreError::Llm(message)` (`crates/core/src/agent/model_step.rs:756-774`). Since `CoreError::Llm` formats itself with `LLM error:`, an already formatted LLM error becomes `LLM error: LLM error: ...`.

Delete stringification at internal seams. Carry a typed error category, stable code, safe user message, diagnostic context, retryability, provider/dialect identity, and partial-output state until the final UI boundary.

On parser failures, preserve a bounded, redacted diagnostic fixture or digest containing event types, IDs, lengths, sequence, and terminal reason. The current live database could show the truncated argument but could not prove whether the provider sent `response.incomplete` or an invalid `response.completed` because that distinction was discarded.

## What is formal and should remain

The deletion test asks: if this module is removed, does its complexity disappear, or merely leak into every caller? These components pass the deletion test and should remain as formal contracts:

- provider-native turn envelopes and immutable route snapshots;
- final wire-history validation and fail-closed replay;
- fail-closed tool authorization and durable tool-result persistence;
- ordered run-event sequence numbers, run identity, gap recovery, and authoritative terminal events;
- no transparent resend/fallback after visible output;
- message validation at the provider boundary;
- reasoning replay policy selected by the concrete route;
- frontend reordering/UTF-8 offset contracts and cross-run rejection;
- bounded recovery and repetition guards.

Their implementations may move behind deeper module interfaces, but their invariants should not be removed.

## Proposed implementation order and acceptance gates

### Phase 0: containment

1. Add the background work governor; cap local embedding threads and batches, support cancellation, source scope, incremental commits, overlap deduplication, and foreground preemption.
2. Remove the second streaming render clock; throttle thinking rendering and disable active-stream layout work.
3. Preserve typed provider errors so diagnostics stop losing terminal kind and producing duplicate prefixes.

Proposed acceptance gates:

- source changes cannot drive the desktop above an agreed memory/CPU budget;
- the window remains interactive throughout a large spreadsheet reindex;
- foreground agent turns preempt or pause background inference;
- no internal/checkpoint paths enter an ordinary source index by default.

### Phase 1: protocol correctness

1. Extract `ResponsesAssembler` with fixtures for completed, incomplete, cancelled, truncated function arguments, mismatched terminal aggregates, multiple calls, hosted tools, and disconnect-before/after visible output.
2. Preserve terminal type and incomplete reason end to end.
3. Execute a tool only after semantic completion and final schema validation.

Acceptance gates:

- exact raw/dialect fixtures for DeepSeek Flash, DeepSeek Pro, Alibaba/Kimi, OpenAI Responses, and gateways;
- no automatic resend after visible output;
- no tool execution from incomplete arguments;
- partial visible output remains available with a precise error category.

### Phase 2: event and presentation architecture

1. Separate `LiveEventBus` from `RunEventJournal`.
2. Batch durable semantic checkpoints and force terminal/tool-boundary flushes.
3. Introduce the single-clock `LiveTranscriptProjection`.

Acceptance gates:

- one presentation commit per frame at most;
- no full-Markdown parse or forced layout per provider delta;
- crash/restart replay preserves the durable prefix, tools, and terminal state;
- storage growth is bounded by a documented retention/compaction policy.

### Phase 3: provider and attempt architecture

1. Split explicit dialect adapters out of `openai.rs`.
2. Make `TurnAttemptController` the sole agent-level retry/fallback owner.
3. Retire the legacy agent-facing chunk stream and scattered model/base-URL protocol guesses.

## Final classification

| Item | Classification | Action |
|---|---|---|
| Unrestricted synchronous background embeddings | P0 | Contain first |
| Thinking Markdown/layout/typewriter amplification | P1 | Remove duplicate work |
| Responses terminal assembly and incomplete classification | P1 | Extract typed assembler |
| Per-presentation-block durable transaction | P1 | Split live bus and journal |
| Multiple retry/fallback owners | P1 architecture | Centralize |
| Dual stream interfaces and scattered dialect heuristics | P1 architecture | Retire after migration |
| Duplicate `LLM error` prefix | P2 UI, P1 diagnostic signal | Fix with typed errors |
| Existing fail-closed replay/tool/visible-output invariants | Formal contract | Keep |

## Related repository research

- `docs/research/2026-08-10-stream-terminal-retry-tool-lifecycle-primary-source-research.md`
- `docs/research/2026-08-10-provider-streaming-runtime-upstream-architecture-research.md`

The companion Codex/Pi/Hermes source audit should be read with this report. It focuses on primary-source architectural precedents rather than Nexa-specific live evidence. It reviews the relevant streaming, tool, retry, persistence, and rendering paths at pinned commits; it deliberately does not make the unprovable claim that every line of each upstream repository was reviewed.
