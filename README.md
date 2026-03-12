# Ask Myself

> A local-first personal knowledge recall engine — rediscover what you already know.

<!-- TODO: Add hero screenshot here -->

## Overview

Ask Myself is a desktop application that turns your local files into a searchable, AI-augmented knowledge base. Point it at your folders — notes, PDFs, logs, spreadsheets — and it indexes everything locally. When you need to find something, describe it however you remember it: a half-recalled phrase, an approximate date, a vague topic. The app converges on the right content through hybrid search and an AI agent that cites every claim back to your documents.

Unlike cloud-based tools, everything stays on your machine. Indexing, embedding, and search run locally using SQLite and ONNX models. The AI agent connects to your choice of LLM provider (OpenAI, Anthropic, Google Gemini, or local Ollama) but never sends your documents — only search-derived context. You control what gets indexed, what gets redacted, and what gets shared.

The core loop is: **ingest → index → search → cite → save**. Findings can be saved as Playbooks — curated evidence collections with steps, commands, and source citations that get stronger with use.

## Features

### 📂 Knowledge Management

- **Multi-source ingestion** — Add local folders as knowledge sources with include/exclude glob patterns
- **Supported formats** — Markdown, plain text, log files, PDF, DOCX, Excel (.xlsx), PowerPoint (.pptx), images
- **File watching** — Automatic re-indexing when files change (powered by `notify`)
- **Incremental updates** — Content-hashed chunks avoid redundant processing
- **OCR** — ONNX-based PaddleOCR (PP-OCRv4) extracts text from images and scanned PDFs, with optional LLM Vision fallback

### 🤖 AI-Powered Chat

- **Evidence-first agent** — Every answer is grounded in your documents with `[cite:CHUNK_ID]` citations
- **Multi-angle recall** — Vague queries trigger synonym expansion, cross-language search, and date-range inference
- **Tool-using agent** — 20 built-in tools the AI can call autonomously (see [Tools](#-tools-ai-agent) below)
- **Conversation history** — Persistent chat sessions with context carry-over
- **Configurable LLM providers** — OpenAI, Anthropic, Google Gemini, Ollama (local)
- **Custom system prompts** — Override agent behavior per conversation
- **Personalization** — Learns from feedback to surface preferred sources and adapt responses

### 🔍 Search

- **Hybrid search** — Combines SQLite FTS5 (BM25) with vector similarity for best-of-both retrieval
- **Local embeddings** — ONNX Runtime with multilingual models (MiniLM-L12 384d or E5-Base 768d), plus TF-IDF fallback
- **API embeddings** — Optional OpenAI-compatible embedding endpoints
- **Filters** — Source, file type, date range
- **Feedback loop** — Thumbs up/down/pin on results to improve future rankings

### 📋 Playbooks

- **Curated evidence collections** — Save search findings as reusable documents
- **Structured format** — Goal, prerequisites, steps, notes, and linked citations
- **Searchable** — Playbooks are first-class searchable entities
- **AI-generated** — Ask the agent to create playbooks from conversation findings

### 🔒 Privacy & Security

- **Local-first** — All data stored in a local SQLite database; no cloud requirement
- **Content redaction** — Regex-based rules to strip sensitive content before storage
- **Exclude patterns** — Glob rules to skip files/directories during ingestion
- **No telemetry** — Zero data collection

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Desktop shell | [Tauri 2](https://tauri.app/) |
| Frontend | React 18, TypeScript, Tailwind CSS 4 |
| UI components | Custom component library (Framer Motion, Lucide icons, cmdk) |
| Backend (core) | Rust |
| Database | SQLite via [rusqlite](https://github.com/rusqlite/rusqlite) (FTS5 + blob vectors) |
| Embeddings (local) | [ONNX Runtime](https://onnxruntime.ai/) via `ort`, [tokenizers](https://github.com/huggingface/tokenizers) |
| Embeddings (API) | OpenAI-compatible endpoints via `reqwest` |
| OCR | PaddleOCR PP-OCRv4 ONNX models |
| File parsing | `lopdf` (PDF), `dotext` (DOCX), `calamine` (Excel), `image`/`imageproc` |
| File watching | `notify` 7 |
| LLM providers | OpenAI, Anthropic, Google Gemini, Ollama |
| Markdown rendering | `react-markdown` + `remark-gfm` |
| Routing | React Router 7 |
| Build tooling | Vite 6, Cargo |

## Getting Started

### Prerequisites

- [Rust](https://rustup.rs/) 1.75+
- [Node.js](https://nodejs.org/) 18+
- Tauri 2 system dependencies ([platform-specific guide](https://tauri.app/start/prerequisites/))

### Installation

```bash
# Clone the repository
git clone https://github.com/MLGBJDLW/Ask_Myself.git
cd Ask_Myself

# Install frontend dependencies
cd apps/desktop
npm install
cd ../..
```

### Development

```bash
cd apps/desktop
npm run tauri dev
```

This starts the Vite dev server and launches the Tauri window with hot reload.

### Building

```bash
cd apps/desktop
npm run tauri build
```

Produces a platform-specific installer in `target/release/bundle/`.

### Feature Flags

The `ask-core` crate uses Cargo features to gate heavy dependencies:

| Feature | Default | Requires |
|---------|---------|----------|
| `ocr` | Yes | — |
| `video` | No | LLVM / libclang |

```bash
# Default build (OCR only, no external deps)
cargo build -p ask-core

# With video transcription support
cargo build -p ask-core --features video
```

To enable the `video` feature you need libclang installed:

- **macOS** — `brew install llvm`
- **Ubuntu** — `apt install libclang-dev`
- **Windows** — Install LLVM from [llvm-project releases](https://github.com/llvm/llvm-project/releases) and set `LIBCLANG_PATH=C:\Program Files\LLVM\bin`

The desktop app builds and runs without LLVM by default (`npm run tauri dev` just works).
To enable video transcription during development:

```bash
cd apps/desktop
# macOS/Linux
LIBCLANG_PATH="..." npm run tauri dev -- -- --features video
# Windows (PowerShell)
$env:LIBCLANG_PATH="C:\Program Files\LLVM\bin"; npm run tauri dev -- -- --features video
```

Prebuilt releases include all features.

## Architecture

```
self-reply/
├── crates/
│   └── core/               # Rust core library (ask-core)
│       ├── src/
│       │   ├── agent/       # AI agent framework + context management
│       │   ├── llm/         # LLM providers (OpenAI, Anthropic, Gemini, Ollama)
│       │   ├── tools/       # Agent tool implementations
│       │   ├── conversation/ # Chat session persistence
│       │   ├── search.rs    # Hybrid FTS + vector search
│       │   ├── embed.rs     # ONNX & TF-IDF embedding engines
│       │   ├── parse.rs     # Document parsing & chunking
│       │   ├── ingest.rs    # File scanning & indexing pipeline
│       │   ├── ocr.rs       # PaddleOCR ONNX integration
│       │   ├── watcher.rs   # File system change monitoring
│       │   ├── feedback.rs  # Thumbs up/down/pin on results
│       │   ├── playbook.rs  # Playbook CRUD
│       │   ├── privacy.rs   # Redaction rules & exclude patterns
│       │   ├── personalization.rs # User preference learning
│       │   └── db.rs        # SQLite database & migrations
│       └── prompts/         # System prompt & tool JSON schemas
├── apps/
│   └── desktop/             # Tauri desktop app
│       ├── src/             # React frontend
│       │   ├── pages/       # Search, Chat, Sources, Playbooks, Settings
│       │   ├── components/  # Shared UI components
│       │   ├── i18n/        # Internationalization (10 languages)
│       │   └── lib/         # API client, hooks, utilities
│       └── src-tauri/       # Tauri Rust backend (bridges core ↔ frontend)
├── docs/                    # Design documents
└── testdata/                # Sample vault for testing
```

## 🛠 Tools (AI Agent)

The agent has access to 20 built-in tools for search, analysis, writing, and knowledge management that it calls autonomously during conversations. See [docs/TOOLS.md](docs/TOOLS.md) for the complete tool reference.

## 🌐 Supported Languages

The UI is available in 10 languages:

| Language | Code |
|----------|------|
| English | `en` |
| 简体中文 (Simplified Chinese) | `zh-CN` |
| 繁體中文 (Traditional Chinese) | `zh-TW` |
| 日本語 (Japanese) | `ja` |
| 한국어 (Korean) | `ko` |
| Español (Spanish) | `es` |
| Français (French) | `fr` |
| Deutsch (German) | `de` |
| Português (Portuguese) | `pt` |
| Русский (Russian) | `ru` |

The AI agent also performs cross-language search — queries in one language will find content written in another.

## License

MIT
