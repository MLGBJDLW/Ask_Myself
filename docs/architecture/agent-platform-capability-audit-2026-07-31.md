# Nexa Agent Platform Capability Audit

**Audit date:** 2026-07-31  
**Baseline:** `master` at `8d3bd46f3304f00dd8af6086854e04f5b56e112b`  
**Implementation branch:** `agent/platform-capability-audit`

## Executive decision

Nexa already contains real implementations for every area in this audit. The main problem is not that the features are missing; it is that several of them stop one architectural layer before becoming a dependable product surface:

- Computer Use has a guarded Windows window-control backend, but not a cross-platform desktop-agent runtime.
- Video analysis has local Whisper, frame extraction, scene sampling, and OCR, but not a meeting-grade speech pipeline.
- Skills already accept safe local directories, `SKILL.md`, `.skill`, and `.zip` packages, but lack universal intake, provenance, dependency resolution, updates, rollback, and trust lifecycle.
- MCP has multiple transports and tool calls, but is still represented as manually configured endpoints rather than installable connector packages, and the client is centered on the 2025 protocol era.
- Graph has useful entities, aliases, relations, evidence, traversal, and visualization, but lacks entity resolution, temporal/conflict modeling, communities, graph-native retrieval modes, and quality evaluation.
- Code-vs-document retrieval is already mostly correct. The remaining work is to make the routing deterministic, observable, and configurable rather than relying mainly on prompt instructions.
- Terminal selection is split between the interactive PTY and the stateless Agent shell tool, so a UI-only preference would be misleading.
- Turn folding currently treats reply-channel content as foldable history. That is the direct cause of full answers disappearing behind Thinking when a model emits a small closing reply.

The recommended product model is an **Extensions Hub** with separate runtime types:

1. **Skills** — instructions, scripts, references, and assets loaded through progressive disclosure.
2. **Connectors** — MCP-backed external capabilities with authentication, permissions, health, updates, and lifecycle.
3. **Apps** — interactive connector UI surfaces such as MCP Apps.

These can share catalog, installation, permissions, provenance, and update UX, but they must not be collapsed into one execution format.

## Maturity snapshot

| Area | Current maturity | Main blocker | Target |
|---|---:|---|---:|
| Usage analytics | 3.5 / 5 | Timeline is chart-first and hard to scan longitudinally | 4.2 / 5 |
| Computer Use | 2.2 / 5 | Window-level Windows backend; no accessibility fusion or desktop runtime | 4.0 / 5 |
| Video / meetings | 2.3 / 5 | ASR-only transcript schema; no VAD/alignment/diarization/reconciliation | 4.0 / 5 |
| Skills | 3.0 / 5 | Local package import without source/lifecycle/dependency model | 4.3 / 5 |
| MCP / connectors | 2.5 / 5 | Endpoint configuration, tools-only projection, protocol-era gap | 4.3 / 5 |
| Knowledge graph | 2.8 / 5 | Extraction/display graph, not yet a graph retrieval and evidence system | 4.2 / 5 |
| RAG / grep routing | 3.8 / 5 | Correct policy exists but is prompt-led and lightly measured | 4.5 / 5 |
| Shell / terminal | 2.7 / 5 | PTY and Agent execution resolve shells independently | 4.0 / 5 |
| Turn lifecycle / cards | 3.0 / 5 | Presentation heuristics stand in for explicit final/input-required states | 4.5 / 5 |

---

## 1. Usage analytics: contribution activity first

### Current state

`apps/desktop/src/components/settings/UsageAnalyticsSettingsTab.tsx` already provides:

- time presets and custom ranges;
- provider, model, and operation filters;
- token, request, cache, cost, and coverage metrics;
- stacked trend bars;
- provider/model and operation tables;
- CSV/JSON export and deletion.

The weakness is visual hierarchy. A long list of horizontal bars is precise for a selected range, but poor for recognizing streaks, inactive periods, bursts, and year-scale habits.

### Implemented in this branch

- Added `UsageContributionHeatmap.tsx`.
- The first activity visualization is now a GitHub-style seven-row contribution grid.
- The heatmap always requests day-level data for the current year, independent of the detail chart bucket.
- Provider/model/operation filters apply to both the heatmap and the detail chart.
- Token and request modes are shared by both views.
- The existing bars remain below the heatmap as the precise second-level view.
- Values use logarithmic intensity so one extreme day does not make the rest of the year unreadable.
- Keyboard focus, accessible labels, localized dates, and horizontal overflow are included.

### Follow-up

The backend should eventually expose a dedicated daily-activity endpoint or a multi-series request so this does not require two analytics queries. The heatmap should later support:

- rolling 365 days versus calendar year;
- model/provider comparison rows;
- click-to-drill into a day;
- cost, cache hit, successful runs, and tool-time modes;
- annotations for releases or configuration changes.

---

## 2. Computer Use

### What is actually implemented

The native path in `crates/core/src/tools/computer_use_tool.rs` and `docs/computer-use-integration.md` is a real Windows implementation, not a placeholder. It includes:

- top-level window enumeration;
- capture of a selected window;
- cursor coordinates and geometry metadata;
- mouse movement, click, double click, drag, and scroll;
- keyboard text and key actions;
- short-lived observation handles scoped to the conversation;
- stale-screen detection before control;
- mandatory approval for control calls;
- a fresh observation after an action.

This is sufficient for a cautious Agent to inspect and operate many ordinary Windows application windows when the tool is registered and approved.

### Current stage

The correct description is **guarded window-level Computer Use**, not a full desktop agent.

It can perform a screenshot/action/screenshot loop. It cannot yet reliably provide the experience implied by “look at my current desktop and operate it like a person” because the following are missing or incomplete:

1. **Whole virtual desktop and multi-monitor capture.** The current abstraction is centered on a selected top-level window.
2. **Cross-platform native backends.** Native control is Windows-only; other platforms depend on external MCP paths.
3. **Accessibility-tree fusion.** There is no Windows UI Automation, macOS AX, or Linux AT-SPI element tree combined with pixels.
4. **Stable element references.** Coordinates are tied to an observation, but buttons, inputs, menus, and semantic targets do not have durable element handles.
5. **Low-latency action batches.** Primitive actions are round-tripped individually; the control loop is safe but not fast or fluid.
6. **Continuous visual diffs.** There is no event-driven observation stream or dirty-region pipeline.
7. **Recovery policy.** There is no first-class model for occlusion, focus loss, unexpected dialogs, DPI changes, application hangs, or action verification.
8. **Evaluation and telemetry.** Action latency, target accuracy, retries, recovery rate, and task success are not measured as a Computer Use product surface.

### Computer Use 2.0 architecture

#### A. Perception runtime

Create `DesktopObservationService` with platform adapters:

- `WindowsObservationBackend`: Windows Graphics Capture + UI Automation.
- `MacObservationBackend`: ScreenCaptureKit + Accessibility API.
- `LinuxObservationBackend`: PipeWire/portal + AT-SPI.

Each observation should contain:

```text
observation_id
virtual_desktop_geometry
monitor_geometry[]
window_geometry[]
screenshot or tile references
changed_regions[]
cursor
focused_window
accessibility_snapshot
redaction_regions[]
captured_at
```

Accessibility nodes should expose stable IDs, role, name, value, enabled/focused state, bounds, available actions, and parent/child relationships. Vision remains the fallback and verification source; accessibility is not a replacement for screenshots.

#### B. Action runtime

Use one canonical action schema for native Computer Use and MCP-backed Computer Use:

```text
move, click, double_click, drag, scroll,
type_text, key_down, key_up, key_chord,
focus_window, invoke_element, set_value, wait
```

Add bounded action batches with interruption points:

- maximum action count;
- maximum elapsed time;
- abort on screen divergence;
- automatic observation after meaningful actions;
- user takeover at any time.

#### C. Driver loop

Introduce a dedicated desktop driver rather than leaving all control semantics in the generic tool loop:

```text
observe -> choose target -> act -> verify -> recover/continue
```

The driver owns action budget, latency budget, observation compaction, retry policy, and visual/accessibility reconciliation. It emits normal Agent events so the UI still shows progress and approvals.

#### D. Security

Replace “approve every primitive forever” with risk tiers:

- read-only observation;
- reversible navigation;
- data entry;
- external communication;
- authentication/secrets;
- purchase, deletion, installation, and other high-impact actions.

Always retain confirmation for high-impact actions. Add application allowlists, protected-window detection, secret-field redaction, clipboard restrictions, and an immutable action replay log.

### Acceptance criteria

- Capture the full desktop and any selected monitor or window.
- Operate Windows, macOS, and Linux through the same model-facing schema.
- Prefer semantic element actions when available and verify with pixels.
- Complete a representative OSWorld-style internal suite with recorded success, latency, and retry metrics.
- Preserve user takeover and confirmation for high-impact actions.

---

## 3. Video analysis and meeting intelligence

### Current state

`crates/core/src/video.rs` already performs:

- FFmpeg media probing and audio extraction;
- conversion to 16 kHz mono WAV;
- local Candle Whisper transcription;
- fixed-interval and scene-based frame sampling;
- frame OCR;
- transcript and frame result aggregation.

The current transcript contract is essentially:

```text
start_time, end_time, text
```

That is adequate for rough video search but not for meetings, interviews, calls, or podcasts.

### Root causes of current quality limits

- Audio is collapsed to mono before channel-aware analysis.
- There is no dedicated voice activity detection stage.
- There are no word-level timestamps or forced alignment.
- There is no speaker diarization or speaker identity model.
- Overlapping speech is not represented.
- ASR and visual timelines are not reconciled into one canonical media timeline.
- `use_gpu` and `beam_size` configuration do not currently describe the real local decode path accurately enough; the local pipeline needs capability reporting rather than optimistic settings.
- Language handling is configured as a transcription option rather than a robust detection, confidence, and fallback pipeline.
- There is no benchmark harness for WER, timestamp error, diarization error rate, or speaker-attributed WER.

### Recommended meeting pipeline

```text
Media probe
  -> channel-preserving decode
  -> audio normalization
  -> VAD
  -> ASR
  -> word-level forced alignment
  -> speaker diarization
  -> overlap detection
  -> ASR/diarization reconciliation
  -> sentence/turn segmentation
  -> meeting intelligence
  -> visual timeline fusion
```

Use provider interfaces instead of coupling the Rust core to one model:

```rust
trait AsrProvider
trait AlignmentProvider
trait DiarizationProvider
trait VisualUnderstandingProvider
trait MeetingSummarizer
```

Recommended backends:

- local lightweight mode: current Candle Whisper plus VAD;
- local meeting mode: faster-whisper/WhisperX-style worker plus pyannote Community-1 or a NeMo diarization worker;
- managed mode: provider APIs that return word and speaker information;
- enterprise/offline mode: pinned local model bundles with no network dependency.

A sidecar is acceptable here if it is treated as a versioned media worker with a strict JSON protocol, health checks, model inventory, progress events, cancellation, and reproducible dependency locking. Python dependencies should not leak into the Rust domain model.

### Canonical schema

```text
MediaAnalysis
  duration_ms
  language_candidates[]
  speakers[]
  words[]
  turns[]
  scenes[]
  visual_events[]
  chapters[]
  summary
  decisions[]
  action_items[]
  open_questions[]
  evidence_links[]

Word
  start_ms, end_ms, text
  confidence
  speaker_id
  overlap
  language

SpeakerTurn
  start_ms, end_ms
  speaker_id
  text
  confidence
  word_ids[]
  overlap_with[]
```

Speaker labels should begin as stable anonymous IDs (`SPEAKER_00`). Optional naming should be a separate consented step using meeting metadata, channel identity, or enrolled voiceprints.

### Meeting features

- speaker-attributed transcript;
- editable speaker names with propagation;
- chapters and topic changes;
- decisions, action items, owners, dates, and unresolved questions;
- “jump to evidence” from every summary claim;
- active-speaker timeline aligned with frames/slides;
- export to Markdown, JSON, SRT/VTT, and structured minutes;
- redaction and local-only processing modes.

### Evaluation

Track at least:

- word error rate;
- timestamp mean absolute error;
- diarization error rate and Jaccard error rate;
- speaker-attributed WER;
- overlap recall;
- hallucinated speech rate in silent regions;
- real-time factor and peak memory;
- action-item/decision precision and evidence faithfulness.

---

## 4. Skills

### What already works

The current Skills importer is stronger than the UI suggests. `crates/core/src/skills/importer.rs`, `crates/core/src/tools/manage_skill_tool.rs`, `apps/desktop/src/components/settings/SkillInstaller.tsx`, and `docs/SKILL_PACKAGES.md` already support:

- a local `SKILL.md` file;
- a skill directory;
- `.skill` archives;
- `.zip` archives;
- UTF-8/frontmatter validation;
- archive path traversal protection;
- archive file-count, per-file, total-size, and depth limits;
- conflict and scan reporting.

Therefore the missing feature is not “ZIP support.” The missing feature is a complete acquisition and lifecycle system.

### Gaps

- only local filesystem intake;
- no URL, Git repository, GitHub/Gitee release, registry, or clipboard intake;
- no format adapters for common Agent Skills layouts and prompt-pack conventions;
- no package identity, immutable version, source commit, checksum, or lockfile;
- no declared runtime dependencies or capability requirements;
- no update, rollback, or channel policy;
- no global/workspace/project installation scopes;
- no quarantine or trust promotion;
- no publisher/signature model;
- no activation-quality evaluation;
- no safe preflight for scripts beyond archive validation.

### Universal intake pipeline

```text
Acquire
  -> identify by magic bytes and repository layout
  -> unpack into quarantine
  -> discover candidate skill roots
  -> normalize manifest and paths
  -> validate specification
  -> scan scripts/assets/references
  -> resolve dependencies and capabilities
  -> present install plan and warnings
  -> user approval
  -> immutable install
  -> activate catalog metadata
```

Supported sources should include:

- local file/directory;
- ZIP, TAR, TAR.GZ, and `.skill` by content signature rather than extension alone;
- HTTPS download;
- Git URL and a subdirectory within a repository;
- GitHub/Gitee repository or release asset;
- curated registry package;
- pasted `SKILL.md` content.

“Install almost anything” must mean **recognize and normalize many package shapes**, not silently execute arbitrary code from the internet.

### Package model

```text
SkillPackage
  id
  name
  version
  source_uri
  source_revision
  checksum
  publisher
  specification
  entrypoint
  files[]
  dependencies[]
  required_capabilities[]
  declared_tools[]
  compatibility
  license
  trust_state

SkillInstallation
  package_id
  scope: global | workspace | project
  enabled
  installed_at
  update_channel
  granted_capabilities[]
  previous_version
```

Keep progressive disclosure:

1. catalog metadata at session start;
2. full instructions only when selected;
3. scripts/references/assets only when needed.

### Security and lifecycle

- immutable version directories and atomic pointer swap;
- checksums and provenance visible before installation;
- script-language and executable inventory;
- capability diff on update;
- dry-run validation in a restricted environment;
- rollback to the previous version;
- quarantine for unknown publishers;
- project-level trust boundaries;
- no implicit network or shell permission merely because a skill contains a script.

### UI

The Skills settings page should become an install center:

- search catalog;
- paste URL;
- choose archive/directory;
- install from Git repository;
- preview discovered skills in a bundle;
- view permissions, files, source, checksum, and warnings;
- update/rollback/uninstall;
- choose global/workspace/project scope;
- test whether a skill triggers on representative prompts.

---

## 5. MCP and connector packages

### Current state

Nexa currently stores MCP server definitions and connects over:

- stdio;
- Streamable HTTP;
- legacy SSE.

The client initializes a server, lists tools, calls tools, and projects MCP tools into the Agent tool registry. This is useful, but the product abstraction is still an endpoint form rather than a connector/plugin lifecycle.

### Important protocol-era gap

The current client is designed around legacy initialization and session-oriented MCP revisions through the 2025 era. The 2026-07-28 protocol revision changes the wire model substantially, including stateless requests, `server/discover`, per-request metadata, an extension framework, and redesigned Tasks. At the audit date, official release surfaces were still in the RC-to-final transition while current SDKs had already begun shipping dual-era support. Nexa must therefore implement negotiation and compatibility, not simply replace the old client.

### Missing MCP capabilities

- dual-era `server/discover` / legacy `initialize` negotiation;
- protocol conformance fixtures;
- resources and resource templates;
- prompts and completion support where available;
- preservation of rich content instead of flattening everything to text;
- structured images/audio/resources/embedded objects;
- MCP Apps host surface;
- Tasks mapped to Nexa’s durable task lifecycle;
- extension negotiation;
- schema revision tracking and tool-definition invalidation;
- package identity/version/source metadata;
- connector-specific authentication flows and secret references;
- per-tool grants and scope escalation;
- health, logs, restart policy, update, uninstall, and rollback;
- deprecation path for legacy SSE.

### Recommended layered model

MCP should be the runtime protocol, while “plugin” or “connector” is the product package:

```text
ConnectorPackage
  identity, version, publisher, icon, description
  install recipe
  runtime definitions[]
  auth requirements
  requested capabilities
  compatibility

ConnectorInstallation
  package + version
  scope
  enabled runtimes
  secret references
  grants
  update policy

McpRuntimeEndpoint
  transport
  command/url/headers/environment
  negotiated protocol era
  discovered capabilities
  health and logs

CapabilityGrant
  connector, server, tool/resource/prompt
  scope and risk
  approval policy
```

This allows one connector package to expose one or more MCP servers while preserving MCP interoperability.

### MCP client upgrade plan

1. Add a protocol codec boundary rather than spreading revision checks through transports.
2. Attempt modern discovery where configured/supported and downgrade safely to legacy initialization.
3. Preserve per-request metadata and extension declarations.
4. Add conformance tests against pinned official SDK test servers for both eras.
5. Store discovered schema hashes and invalidate cached tool definitions on change.
6. Represent MCP content as typed Nexa tool output channels.
7. Map long-running MCP Tasks to `running`, `waiting_for_user`, `waiting_for_approval`, terminal states, cancellation, and reconnection.
8. Host MCP Apps in a sandboxed surface with explicit connector origin and capability boundaries.
9. Keep legacy SSE only as a compatibility transport with a visible deprecation warning.

### Connector UI

Create a dedicated **Extensions > Connectors** surface with:

- catalog and manual advanced setup;
- install/update/uninstall;
- authentication and secret repair;
- health and restart;
- discovered tools/resources/prompts/apps;
- per-capability enablement;
- activity/log view;
- protocol/transport/version diagnostics;
- trust and publisher information.

---

## 6. Knowledge graph

### What already works

The graph implementation is not merely decorative. `crates/core/src/knowledge_graph.rs`, `crates/core/src/compile.rs`, `apps/desktop/src/components/knowledge/KnowledgeGraphView.tsx`, and `apps/desktop/src/lib/knowledgeGraphAgent.ts` provide:

- entities and aliases;
- typed relations and relation strength;
- source/document evidence links;
- co-occurrence edges;
- filters by source, folder, entity type, relation type, and strength;
- traversal/path operations;
- graph summaries;
- focus, overview, and atlas-style visualization modes.

### Why it still feels weak

The pipeline currently behaves like **LLM extraction plus visualization**, while a dependable knowledge graph needs identity, provenance, temporal semantics, retrieval planning, and evaluation.

The largest quality risks are:

- entity matching based primarily on normalized names, aliases, and types;
- accidental merging of homonyms;
- missed cross-document coreference;
- free-form relation labels that fragment the ontology;
- no explicit claim/status/time model;
- no representation of contradictory evidence;
- no model/prompt/version lineage per extraction;
- incomplete incremental invalidation and delete/rebuild semantics;
- no communities or community reports;
- graph traversal is available, but query routing is not yet GraphRAG-style local/global/DRIFT reasoning;
- the SVG renderer will become a bottleneck for large graphs.

### Graph 2.0 data model

Add these concepts:

```text
CanonicalEntity
EntityMention
EntityAlias
EntityMergeDecision
RelationAssertion
Claim
TemporalInterval
EvidenceSpan
ExtractionRun
Community
CommunityReport
GraphSnapshot
```

A relation is not just `(source, type, target)`. It should include:

- normalized relation type;
- natural-language description;
- confidence and calibration source;
- evidence spans;
- valid time and observed time;
- extraction model, prompt, and run;
- status: asserted, disputed, superseded, rejected;
- optional inverse relation;
- scope/project/source visibility.

### Entity resolution

Use a staged resolver:

1. deterministic normalization and exact aliases;
2. type-compatible candidate generation;
3. embedding and context similarity;
4. graph-neighborhood consistency;
5. calibrated merge decision;
6. human review for ambiguous high-impact merges.

Expose merge/split history and allow users to correct identity. Corrections should become durable constraints for future indexing.

### Ontology

Maintain a relation registry with canonical names, aliases, inverses, domain/range constraints, display labels, and temporal behavior. Unknown relations may be retained, but should be mapped or flagged rather than silently creating endless near-duplicates.

### Retrieval modes

Create a graph-aware retrieval planner:

- **Local graph search:** entity-centered neighborhood plus source text evidence.
- **Global graph search:** hierarchical community reports with map/reduce aggregation.
- **DRIFT-style search:** community primer followed by focused local traversals.
- **Basic document search:** normal hybrid RAG when graph reasoning adds no value.

Every graph answer must retain text evidence. Graph structure is a routing and aggregation aid, not a replacement for citations.

### Incremental indexing

- content hash per text unit;
- extraction-run lineage;
- invalidate mentions/relations/claims derived from changed or deleted text units;
- re-resolve affected identity neighborhoods;
- incrementally update communities when practical and schedule full rebuilds when drift thresholds are exceeded;
- expose stale graph status in the UI.

### Visualization

Move large graph layout to a worker and use Canvas/WebGL beyond a node threshold. Add:

- community collapsing;
- timeline and validity filters;
- contradictory-edge display;
- evidence inspector;
- merge/split review;
- path explanation;
- saved graph lenses;
- graph quality and freshness indicators.

### Evaluation

Measure:

- entity precision/recall and merge accuracy;
- relation precision/recall;
- evidence-span correctness;
- temporal correctness;
- contradiction detection;
- local/global retrieval answer faithfulness;
- path usefulness;
- stale-edge rate after incremental updates;
- user correction rate.

---

## 7. RAG versus grep/code intelligence

### Audit result

The requested separation is already substantially implemented:

- `docs/TOOLS.md` distinguishes knowledge-base hybrid search from exact local-file search.
- `crates/core/prompts/tools/search_files.json` positions file search for source-local codebase browsing.
- `crates/core/prompts/system.md` tells coding work to begin with grep/code intelligence rather than document RAG.
- `crates/core/src/ingest.rs` identifies a broad range of source-code extensions, skips them from document embedding, and removes unsupported indexed code documents.

Therefore the frontend should **not re-enable ordinary document embeddings for source files by default**. Code deserves a separate semantic index built from symbols, ASTs, references, imports, call relationships, tests, and change history—not chunks sent through the document RAG path.

### Remaining problem

The policy is mostly expressed in prompts and tool descriptions. A model can still choose the wrong retriever, and there is not enough route telemetry to prove when this happens.

### Deterministic retrieval router

Introduce a retrieval intent classifier before tool exposure or execution:

```text
CodeExact
CodeSymbol
CodeHistory
DocumentEvidence
DocumentMetadata
GraphLocal
GraphGlobal
WebEvidence
Mixed
```

Recommended rules:

- code path, symbol, error string, import, reference, definition, or repository task -> grep/glob/code intelligence/git first;
- natural-language question over PDFs, office files, notes, or indexed sources -> hybrid RAG;
- exact phrase in ordinary files -> grep first, then RAG if semantic context is needed;
- entity relationship question -> graph local plus source evidence;
- corpus-wide themes/comparison -> graph global or document aggregation;
- mixed code-and-spec task -> run separate code and document lanes, then merge evidence.

The router should produce a structured decision containing intent, confidence, selected tools, excluded tools, and fallback order. The Agent prompt then explains the decision; it does not create the decision.

### Code indexing policy

Default:

- exclude source code from document embeddings;
- index symbols, signatures, docstrings, references, imports, tests, and file summaries in a separate code index;
- use ripgrep for exact text;
- use tree-sitter/LSP for structure;
- use git diff/blame/log for change questions;
- optionally use embeddings over symbol-level representations, never silently mix them with document vectors.

Add a per-source advanced override for unusual cases such as notebooks, literate programming, or repositories used mainly as documentation.

### Observability

Log:

- retrieval intent and confidence;
- chosen lane and fallback lane;
- query rewrite;
- result counts and latency;
- evidence used in the final answer;
- whether the Agent switched lanes;
- retrieval success judged by downstream evidence use.

---

## 8. Default terminal and shell selection

### Current state

There are two independent execution paths:

1. `TerminalDock.tsx` and Tauri PTY commands create an interactive terminal with a local dropdown.
2. `run_shell` parses an optional `shell` selector and otherwise resolves its own default.

On Windows, the two paths also use different fallback behavior. The interactive PTY probes PowerShell variants and `cmd`; `run_shell` defaults to Windows PowerShell unless the model explicitly requests another shell.

A Settings dropdown wired only to `TerminalDock` would therefore be a false fix: the Agent would continue using a different resolver.

### Target configuration

```text
ShellPreference =
  auto | pwsh | windows_powershell | bash | zsh | cmd | sh

terminal.shellPreference
agent.shellPreference
terminal.linkAgentShell = true by default
```

A single backend `ShellCapabilityService` should:

- probe executables and versions;
- identify Git Bash versus WSL versus native shells on Windows;
- report the resolved executable and dialect;
- validate the selected shell before saving;
- return a deterministic fallback chain;
- provide the same result to PTY creation, `run_shell`, and Agent prompt construction.

### Recommended fallback policy

Windows `auto`:

```text
pwsh -> configured Git Bash/native bash -> Windows PowerShell -> cmd
```

macOS `auto`:

```text
$SHELL -> zsh -> bash -> sh
```

Linux `auto`:

```text
$SHELL -> bash -> zsh -> sh
```

Do not silently route into WSL unless the user explicitly selects a WSL distribution, because path, environment, permissions, and quoting semantics change.

If a selected shell is missing:

1. show the failure and resolved fallback in Settings;
2. use the platform fallback for that run;
3. emit the actual shell/dialect into the Agent context;
4. never let the model believe Bash syntax is being used when PowerShell executed it.

### Runtime behavior

If a `run_shell` call explicitly supplies `shell`, that call-level selection wins. Otherwise the backend applies `agent.shellPreference`. The resolved shell should be included in tool artifacts and command cards.

The Settings UI should show:

- Agent command shell;
- interactive terminal shell;
- link/unlink toggle;
- detected executable and version;
- fallback warning;
- optional working directory and environment profile later.

### Implementation boundary

This must be delivered as one cross-path change touching AppConfig, the frontend type and settings surface, Tauri PTY creation, `run_shell` resolution, prompt/runtime metadata, and tests. It should not be split into a UI-only preference.

---

## 9. Turn folding, final replies, Question Cards, and live waiting

### Root cause

`buildCollapsedLiveTrace` previously kept only the last reply outside the fold and converted earlier replies into `historySections`. `ThinkingBlock` then rendered those sections inside the collapsed Thinking disclosure.

When a provider emitted:

```text
[full answer]
[more reasoning/tool activity]
[short closing summary or question]
```

Nexa treated the short closing message as the only visible final reply and hid the full answer inside Thinking. This also allowed interactive content near earlier reply/tool sections to become visually subordinate to the fold.

### Implemented in this branch

- If a completed live timeline contains more than one reply item, it is not auto-collapsed.
- Reply-channel content is never reclassified as reasoning.
- A regression test covers a full answer followed by a small closing reply.
- Single-final-reply turns still collapse surrounding reasoning normally.
- Single-choice and confirm Question Cards submit immediately once all required choices are complete.
- A submission lock prevents duplicate sends.
- Multi-choice, free-text, and custom “Other” answers retain explicit confirmation.
- Structured `questionResponse` messages remain persisted for audit/recovery and card resolution, but are projected as control-plane rows so they do not render as a duplicate user bubble or extra turn.

### Remaining architectural problem

`request_user_input` is still represented through a tool call plus an ordinary continuation message. A durable Agent runtime should model waiting explicitly.

### Required state machine

```text
queued
running
waiting_for_user
waiting_for_approval
paused
cancelling
completed
failed
cancelled
timed_out
```

A tool or MCP Task that requests input should transition the current run to `waiting_for_user`. The run remains durable and resumable; it is not falsely shown as completed. The interaction card is pinned outside any Thinking fold until resolved.

Add canonical events:

```text
interactionRequested
interactionUpdated
interactionResolved
interactionCancelled
```

The response path should be a structured continuation API:

```text
continue_run(run_id, interaction_id, response_artifact)
```

It must not create a new ordinary conversation turn. The response may be stored as an audit event and artifact, while the visible chat timeline shows the card changing from pending to answered.

### Interaction surface rules

Never fold:

- final answer content;
- pending questions;
- pending approvals;
- forms, choices, confirmations, or handoff cards;
- errors requiring user action;
- live task controls.

Foldable by default:

- reasoning text;
- completed progress/status events;
- completed non-interactive tools;
- developer-only diagnostics.

Keep expanded or separately summarized:

- intermediate answer drafts;
- verification reports;
- failed tools with actionable errors;
- long-running task status.

### Finality protocol

Do not infer finality from “last reply in an array.” Add explicit reply semantics:

```text
replyRole = progress | intermediate | final | handoff
blockId
sequence/offset
finalized
```

Providers without native block metadata can use the adapter’s best-effort projection, but the UI should remain conservative: uncertainty must result in more visible answer content, never hidden content.

### Acceptance criteria

- A full answer can never disappear because a later short reply exists.
- Pending interaction cards stay visible across folding, navigation, reload, and reconnect.
- A single-choice selection continues the same run immediately and creates no ordinary user bubble.
- Multi-step forms submit only after completion.
- The run remains `waiting_for_user` until resolved.
- Duplicate clicks or reconnect replay cannot submit twice.
- Interaction resolution and final answer ordering are deterministic under streaming races.

---

## Cross-cutting implementation plan

### Phase 0 — included in this branch

- contribution heatmap before detailed trend bars;
- conservative answer folding;
- regression coverage for multi-reply completion;
- auto-submit completed single/confirm cards;
- hide structured Question Card continuations from ordinary bubbles;
- this architecture audit.

### Phase 1 — correctness and shared platform contracts

1. Durable `waiting_for_user` / `waiting_for_approval` runtime and structured continuation API.
2. Shared ShellCapabilityService and persisted Agent/terminal preferences.
3. Deterministic retrieval intent router and route telemetry.
4. Skills package identity, source, provenance, immutable install, update, and rollback.
5. MCP dual-era codec/negotiation and conformance tests.
6. Typed MCP content projection and connector package domain model.

### Phase 2 — product-grade capability upgrades

1. Full-desktop cross-platform Computer Use perception/action runtime.
2. Meeting-grade media worker with VAD, alignment, diarization, and reconciliation.
3. Graph identity/claims/temporal model and incremental invalidation.
4. Communities, reports, and local/global/DRIFT retrieval modes.
5. Extensions Hub UI for Skills, Connectors, Apps, permissions, health, and updates.

### Phase 3 — quality, evaluation, and scale

- Computer Use task benchmark and action telemetry;
- media WER/DER/SA-WER benchmark suite;
- skill activation and safety evaluation;
- MCP official-SDK compatibility matrix;
- graph extraction/retrieval evaluation datasets;
- retrieval-router offline and online evaluation;
- large-graph Canvas/WebGL rendering;
- enterprise policy, publisher trust, and managed extension catalogs.

## File-level work map

| Workstream | Primary files/modules |
|---|---|
| Usage | `apps/desktop/src/components/settings/UsageAnalyticsSettingsTab.tsx`, usage analytics backend |
| Computer Use | `crates/core/src/tools/computer_use_tool.rs`, new platform observation/action modules, desktop UI |
| Video | `crates/core/src/video.rs`, new media provider/worker protocol, video settings and result UI |
| Skills | `crates/core/src/skills/*`, `manage_skill_tool.rs`, `SkillInstaller.tsx`, new package/provenance tables |
| MCP | `crates/core/src/mcp/*`, MCP tools, server form/settings, new connector package and extension host |
| Graph | `knowledge_graph.rs`, `compile.rs`, graph tool/router, graph view and workers |
| Retrieval | system/tool prompts, ingest policy, search/code-intelligence tools, new retrieval router/telemetry |
| Shell | AppConfig, `shell_adapter.rs`, Tauri terminal commands, `TerminalDock.tsx`, Agent prompt metadata |
| Turn lifecycle | streaming protocol/store/view model, task runtime, Question Card, durable run events, continuation command |

## Required test matrix

### Unit and contract tests

- heatmap day bucketing, intensity, year boundaries, and filters;
- no reply content enters collapsed reasoning;
- interaction response remains available for card resolution but invisible as a turn;
- shell probe and fallback matrix per OS;
- retrieval routing fixtures;
- archive/source/provenance and rollback tests;
- MCP codecs for legacy and 2026-era fixtures;
- graph merge/split, temporal, contradiction, and invalidation tests;
- media timeline reconciliation fixtures.

### Integration tests

- Question Card -> waiting run -> structured continuation -> final reply;
- PTY and `run_shell` use the same configured shell;
- install Skill from URL/Git/archive, inspect, update, and rollback;
- install Connector package, authenticate, grant one tool, restart, and upgrade;
- MCP Task survives reconnect and cancellation;
- code query never falls into document embeddings by default;
- document query can fall back from exact grep to RAG;
- graph answer retains source evidence;
- meeting transcript assigns stable speakers and evidence timestamps;
- Computer Use observes, acts, verifies, and yields to takeover.

### Product telemetry

Every upgraded capability should report capability version, selected backend, latency, fallback/retry, terminal status, and a privacy-safe success signal. A setting that cannot prove which backend actually ran is not complete.

## Explicit non-goals

- Do not auto-execute arbitrary scripts merely because an online archive resembles a Skill.
- Do not describe MCP endpoints and Skills as the same runtime type.
- Do not re-enable ordinary document embeddings for source code by default.
- Do not silently enter WSL when a user selected Bash.
- Do not call the current window-level Windows backend a production-ready cross-platform desktop agent.
- Do not hide uncertain reply content to make the timeline look cleaner.
- Do not ship protocol-era claims without conformance fixtures and negotiated fallback.

## Recommended merge/decomposition strategy

Keep the fixes in this branch narrowly reviewable. The larger upgrades should land as separate epics and pull requests with contracts first:

1. `turn-runtime/input-required`
2. `shell/shared-preference`
3. `retrieval/intent-router`
4. `skills/package-lifecycle`
5. `mcp/dual-era-client`
6. `extensions/connector-packages`
7. `media/meeting-pipeline`
8. `computer-use/desktop-runtime`
9. `graph/evidence-and-retrieval-2`

This order removes UI correctness failures first, then establishes shared runtime contracts, then adds expensive perception/indexing capabilities on top of stable foundations.
