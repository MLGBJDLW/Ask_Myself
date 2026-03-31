# Ask Myself

> A local-first personal knowledge recall engine.

Ask Myself is a desktop application that turns your local files into a searchable, AI-augmented knowledge base. Point it at folders containing notes, PDFs, logs, spreadsheets, and other documents; it indexes everything locally, lets you search in natural language, and uses an evidence-first agent to answer with citations back to your own data.

Unlike cloud-native note tools, the core data path stays on your machine. Indexing, parsing, embedding, OCR, search, collections, and chat persistence all run locally. External LLM providers can be used for generation, but the app sends scoped context rather than your full document store.

The project has recently evolved beyond a flat chat log:

- Conversations now persist structured collection context.
- Each user turn can persist route, status, trace, and final answer bindings.
- The chat UI is moving toward a turn-driven trace timeline rather than disconnected thinking/tool/reply fragments.
- Collections can launch scoped follow-up chat with both source scope and collection metadata attached.

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

### AI-Powered Chat

- Evidence-first answers with `[cite:CHUNK_ID]` citations
- Hybrid retrieval over your local knowledge base
- Route-aware agent that distinguishes direct response, retrieval, collection-focused, file, source-management, and web-style requests
- Persistent conversations with recoverable turn traces
- Live trace timeline for thinking, tool activity, route selection, and status
- Collection-aware chat handoff from the Collections page
- Configurable providers: OpenAI, Anthropic, Google Gemini, Ollama, and other OpenAI-compatible endpoints already supported by the codebase
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

## Current Architectural Highlights

- Structured conversation context:
  - `conversations`
  - `messages`
  - `conversation_sources`
  - `conversation_turns`
  - `conversation_checkpoints`
- Structured agent trace pipeline:
  - live `traceEvents` on the frontend
  - persisted turn traces on the backend
  - route/status/tool/reply all becoming explicit objects
- Collection-aware conversations:
  - collection metadata persisted on the conversation
  - source scope persisted separately
  - collection-driven prompt sections injected server-side

## Tech Stack

| Layer | Technology |
| --- | --- |
| Desktop shell | Tauri 2 |
| Frontend | React 18, TypeScript, Tailwind CSS 4 |
| Animation/UI | Framer Motion, Lucide, cmdk |
| Core backend | Rust |
| Database | SQLite via `rusqlite` |
| Search | SQLite FTS5 + local vector search |
| Embeddings | ONNX Runtime, tokenizers, optional API embeddings |
| OCR | PaddleOCR ONNX models |
| Routing | React Router 7 |
| Build tooling | Vite 6, Cargo |

## Built-in Agent Tools

The default registry currently exposes two dozen built-in tools, including:

- Search tools
- Collection management tools
- Evidence retrieval tools
- File read/edit/create tools
- Directory and document listing tools
- Comparison and summarization tools
- Source management tools
- Statistics and verification tools
- MCP-backed tools

See [docs/TOOLS.md](docs/TOOLS.md) for the tool reference.

## Getting Started

### Prerequisites

- Rust 1.75+
- Node.js 18+
- Tauri 2 system dependencies

### Install

```bash
git clone https://github.com/MLGBJDLW/Ask_Myself.git
cd Ask_Myself
cd apps/desktop
npm install
cd ../..
```

### Development

```bash
cd apps/desktop
npm run tauri dev
```

### Production Build

```bash
cd apps/desktop
npm run tauri build
```

## Feature Flags

The `ask-core` crate uses Cargo features to gate heavier functionality:

| Feature | Default | Notes |
| --- | --- | --- |
| `ocr` | Yes | OCR support for images and scanned PDFs |
| `video` | No | Video/audio tooling; requires LLVM / libclang |

Examples:

```bash
# Core crate only
cargo build -p ask-core

# Enable video support
cargo build -p ask-core --features video
```

## Repository Layout

```text
self-reply/
|- crates/
|  |- core/
|     |- src/
|        |- agent/            # Agent execution and routing
|        |- conversation/     # Conversations, turns, checkpoints
|        |- llm/              # Provider adapters
|        |- tools/            # Built-in agent tools
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
