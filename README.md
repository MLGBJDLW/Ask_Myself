# Nexa

> A local-first desktop assistant and personal knowledge workspace.

[![CI](https://github.com/MLGBJDLW/Nexa/actions/workflows/ci.yml/badge.svg?branch=master)](https://github.com/MLGBJDLW/Nexa/actions/workflows/ci.yml?query=branch%3Amaster)
[![Release](https://github.com/MLGBJDLW/Nexa/actions/workflows/release.yml/badge.svg)](https://github.com/MLGBJDLW/Nexa/actions/workflows/release.yml)
[![Latest release](https://img.shields.io/github/v/release/MLGBJDLW/Nexa)](https://github.com/MLGBJDLW/Nexa/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey)

[中文说明](README.zh-CN.md)

Nexa is a local desktop assistant built around your own files and knowledge base. Point it at folders containing notes, PDFs, logs, spreadsheets, presentations, and other documents; it indexes everything locally, lets you search in natural language, grounds answers in evidence from your own data, and helps with everyday desktop work such as investigation, summarization, document creation, comparison, and office-style assistance.

Unlike cloud-native note tools, the core data path stays on your machine. Indexing, parsing, embedding, OCR, search, collections, and chat persistence all run locally. External LLM providers can be used for generation, but the app sends scoped context rather than your full document store.

The current chat runtime goes beyond a flat message log:

- Conversations now persist structured collection context.
- Each user turn can persist route, status, trace, and final answer bindings.
- The chat UI renders a turn-driven trace timeline rather than disconnected thinking/tool/reply fragments.
- Collections can launch scoped follow-up chat with both source scope and collection metadata attached.
- Conversations can be archived, opened read-only, restored, or permanently deleted from a visible archive view.
- The docked terminal is linked to the active conversation: users can send a selection into the prompt, and the agent can inspect the linked session or request approval to write/interrupt it.

## Product Direction

Nexa is no longer only a “local knowledge base chat” product. The product direction is a local-first assistant with a strong knowledge base core.

The active direction is:

- local-first personal knowledge recall
- evidence-first investigation over the user's own files
- practical desktop assistance for normal users, not just programmers
- office and document help that stays grounded in the user's local context

The product should feel like a trustworthy desktop assistant with strong local memory, not a developer-only agent console.

See the living docs:

- [docs/PRODUCT_DIRECTION.md](docs/PRODUCT_DIRECTION.md)
- [docs/ROADMAP.md](docs/ROADMAP.md)
- [docs/UX_QUALITY_BAR.md](docs/UX_QUALITY_BAR.md)
- [docs/I18N_GUIDELINES.md](docs/I18N_GUIDELINES.md)
- [docs/ECOSYSTEM_ARCHITECTURE.md](docs/ECOSYSTEM_ARCHITECTURE.md)
- [docs/README.md](docs/README.md)

## Core Workflow

`ingest -> index -> search -> cite -> collect -> ask`

1. Ingest files from local sources.
2. Parse, chunk, embed, and index them.
3. Retrieve evidence with hybrid FTS + vector search.
4. Ground answers with citations.
5. Save important evidence into collections.
6. Continue asking from a collection-aware or source-scoped chat context.

## Features

### Knowledge Management

- Multi-source ingestion with include/exclude glob patterns
- Incremental re-indexing using content hashes
- File watching via `notify`
- OCR for images and scanned PDFs
- Optional video/audio processing behind feature flags

Supported formats include:

- Markdown
- Plain text
- Log files
- PDF
- DOCX
- XLSX
- PPTX
- Images

### AI-Powered Chat and Assistance

- Evidence-first answers with `[cite:CHUNK_ID]` citations
- Hybrid retrieval over your local knowledge base
- Route-aware agent that distinguishes direct response, retrieval, collection-focused, file, source-management, and web-style requests
- Persistent conversations with recoverable turn traces
- Live trace timeline for thinking, tool activity, route selection, and status
- Collection-aware chat handoff from the Collections page
- Consumer-friendly investigation workspace in the Chat UI
- Recall Mode entry in Search for vague memory lookup
- Office-style document assistance through document and file tools
- Configurable model providers across four built-in adapters — OpenAI-compatible, Anthropic, Google Gemini, and Ollama — with bundled presets for OpenAI, OpenRouter, Anthropic, Gemini (including Gemini 3.6 Flash and Gemini 3.5 Flash-Lite), DeepSeek, Qwen (including the dedicated Token Plan route for Qwen3.8 Max Preview), Zhipu, Moonshot, Doubao, Baichuan, Yi, LM Studio, Azure OpenAI, and other OpenAI-compatible endpoints
- A compact split control below the prompt: model selection on the left and model-aware reasoning effort or thinking budget on the right
- Archived conversation browsing with read-only replay, restore, delete, search, and direct-link handling
- A conversation-linked terminal with selection-to-prompt, normal terminal copy/paste shortcuts, bounded output capture, and approval-gated agent interaction
- Switchable agent personas and a slash-command palette for quick actions
- Durable agent, user, and project memory tools
- Markdown answers with LaTeX math and Mermaid diagram rendering
- Read-only plan mode and dynamic tool discovery via `tool_search`
- Custom per-conversation system prompts
- Answer caching and personalization signals from feedback

### Search

- SQLite FTS5 for lexical search
- Vector similarity for semantic search
- Hybrid ranking with reranking layers such as feedback and source preferences
- Filters for source, file type, and date range
- Save evidence directly into collections from search results

### Collections

Collections (historically called Playbooks in the code) are curated evidence workspaces:

- Save and organize cited chunks
- Edit notes and reorder citations
- Load real evidence details, not just chunk IDs
- Launch collection-scoped chat with persisted collection context
- Reuse collections as a higher-signal working set for future answers

### Privacy and Security

- Local-first storage with SQLite
- Regex-based redaction rules
- Source exclusion rules
- No telemetry pipeline in the product itself

### Ecosystem and Extensions

Nexa separates extension surfaces by risk and purpose instead of treating every
external touchpoint as a plugin:

- Capability packages describe coherent built-in or installable abilities with
  `capability.yaml`.
- MCP connectors are the first external lane for service and tool integration.
- Skill packages share reusable agent methods, references, scripts, and assets.
- Workflow packages define user-facing task templates.
- Adapters sit behind stable provider interfaces for models, search, image, and
  document runtimes.
- Native plugins are reserved for future isolated code, hook, or UI extensions
  when safer surfaces are not enough.

See:

- [docs/ECOSYSTEM_ARCHITECTURE.md](docs/ECOSYSTEM_ARCHITECTURE.md)
- [docs/CAPABILITY_PACKAGES.md](docs/CAPABILITY_PACKAGES.md)
- [docs/MCP_CONNECTORS.md](docs/MCP_CONNECTORS.md)
- [docs/SKILL_PACKAGES.md](docs/SKILL_PACKAGES.md)
- [docs/WORKFLOW_PACKAGES.md](docs/WORKFLOW_PACKAGES.md)
- [docs/PROTOCOL_EXITS.md](docs/PROTOCOL_EXITS.md)
- [docs/NATIVE_PLUGIN_RUNTIME.md](docs/NATIVE_PLUGIN_RUNTIME.md)

## Current Architectural Highlights

- Structured conversation context:
  - `conversations`
  - `messages`
  - `conversation_sources`
  - `conversation_turns`
  - `conversation_checkpoints`
  - archived-state lifecycle and read-only replay
- Structured agent trace pipeline:
  - live `traceEvents` on the frontend
  - persisted turn traces on the backend
  - route/status/tool/reply all becoming explicit objects
- Collection-aware conversations:
  - collection metadata persisted on the conversation
  - source scope persisted separately
  - collection-driven prompt sections injected server-side
- Conversation-linked terminal sessions:
  - the visible PTY stays user-controlled
  - recent output is available to the active agent as untrusted local evidence
  - writes and interrupts require explicit approval

## Tech Stack

| Layer | Technology |
| --- | --- |
| Desktop shell | Tauri 2 |
| Frontend | React 19.2, TypeScript, Tailwind CSS 4 |
| Animation/UI | Framer Motion, Lucide, cmdk |
| Core backend | Rust |
| Database | SQLite via `rusqlite` |
| Search | SQLite FTS5 + local vector search |
| Embeddings | ONNX Runtime, tokenizers, optional API embeddings |
| OCR | PaddleOCR ONNX models |
| Routing | React Router 8.3 |
| Markdown/Math/Diagrams | react-markdown, KaTeX, Mermaid |
| Build tooling | Vite 6, Cargo |

## Built-In Agent Tools

The runtime currently exposes 50+ built-in tools, with additional tools available from enabled MCP connectors, including:

- Search tools
- Collection management tools
- Evidence retrieval tools
- File read/edit/create tools
- Directory and document listing tools
- Comparison and summarization tools
- Source management tools
- Statistics and verification tools
- Conversation-linked terminal inspection and approval-gated interaction
- MCP connector tools exposed by enabled connectors

See [docs/TOOLS.md](docs/TOOLS.md) for the tool reference.

## Getting Started

### Prerequisites

- Rust stable (selected by `rust-toolchain.toml`)
- Node.js 20+ (CI builds on Node 24)
- Tauri 2 system dependencies

### Install

```bash
git clone https://github.com/MLGBJDLW/Nexa.git
cd Nexa
npm ci
npm ci --prefix apps/desktop
```

### Development

```bash
cd apps/desktop
npm run tauri -- dev
```

For frontend-only work, run `npm run dev` from `apps/desktop`.

### Production Build

```bash
cd apps/desktop
npm run tauri -- build
```

### Verification

Run frontend commands from `apps/desktop`:

```bash
npm test
npm run build
npm run e2e
```

Run Rust checks from the repository root:

```bash
cargo fmt --all -- --check
cargo clippy -p nexa-core -- -D warnings
cargo test -p nexa-core
cargo check -p nexa-desktop
```

## Feature Flags

The `nexa-core` crate uses Cargo features to gate heavier functionality:

| Feature | Default | Notes |
| --- | --- | --- |
| `ocr` | Yes | OCR for images and scanned PDFs (PaddleOCR ONNX models) |
| `video` | No | Video/audio analysis: FFmpeg audio extraction + Whisper speech-to-text (Candle). Requires `ffmpeg`/`ffprobe` on PATH and pulls in extra model dependencies |

Examples:

```bash
# Core crate only
cargo build -p nexa-core

# Enable video support
cargo build -p nexa-core --features video
```

## Repository Layout

```text
Nexa/
|- crates/
|  |- core/
|     |- src/
|        |- agent/            # Agent execution and routing
|        |- conversation/     # Conversations, turns, checkpoints
|        |- llm/              # Provider adapters
|        |- mcp/              # MCP connector client/runtime
|        |- skills/           # Skill packages, scanner, importer, selector
|        |- tools/            # Built-in agent tools
|        |- capability_package.rs
|        |- ecosystem.rs
|        |- protocol_exports.rs
|        |- workflow_catalog.rs
|        |- search.rs         # Hybrid retrieval
|        |- embed.rs          # Embeddings
|        |- parse.rs          # Parsing and chunking
|        |- ingest.rs         # Ingestion pipeline
|        |- playbook.rs       # Collection CRUD
|        |- personalization.rs
|        |- privacy.rs
|        |- db.rs
|        |- migrations/
|- apps/
|  |- desktop/
|     |- src/
|        |- pages/            # Search, Chat, Sources, Collections, Settings
|        |- components/       # Shared UI and trace components
|        |- lib/              # API client, hooks, streaming store, helpers
|     |- src-tauri/           # Tauri backend bridge
|- docs/
|- testdata/
```

## What Is Still Being Strengthened

The current direction of the project is clear, but a few major upgrades are still in progress:

- Moving the chat UI fully to a turn-driven model
- Expanding the route layer from heuristics into a richer query router
- Deepening collection-aware retrieval and answer planning
- Making persisted traces the primary replay source across the app
- Reducing large-file complexity in both Rust and React modules
- Evolving from a recall-only experience toward a broader consumer desktop assistant
- Raising the i18n discipline bar so all shipped locales remain coherent
- Defining and enforcing a stronger UX quality bar across Search, Collections, and Chat

## Supported UI Languages

The desktop UI ships with 10 languages:

- English
- Simplified Chinese
- Traditional Chinese
- Japanese
- Korean
- Spanish
- French
- German
- Portuguese
- Russian

## License

MIT
