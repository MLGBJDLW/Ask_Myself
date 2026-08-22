# Tool Reference

Nexa ships with built-in tools that the AI agent can call during conversations,
plus tools from enabled MCP connectors. Knowledge and file tools are scoped to
configured local sources. Network, shell, desktop, connector, and live-terminal
tools declare separate trust and approval boundaries; they are not described as
knowledge-base reads.

---

## 🔍 Search & Retrieval

### `tool_search`

Search the enabled built-in and MCP tool catalog by name and description. `tool_search` is the resident discovery lane for dynamic tool visibility: when a needed enabled tool is hidden from the current model step, matching results activate that tool for the next step. Disabled MCP connectors are not discoverable until connected.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `query` | string | yes | Natural language query or tool-name fragment |
| `limit` | integer | no | Max matches, 1-20 (default 8) |

> **Example:** Ask which enabled tool should handle source-scoped text search, document comparison, or an enabled connector capability.

---

### `search_knowledge_base`

Hybrid full-text (BM25) and vector search across all indexed content. Returns evidence cards with content, source paths, relevance scores, chunk IDs for citation, and trust metadata. Supports batch queries via the `queries` parameter for synonym/variant expansion in a single call.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `query` | string | no* | Concise noun-phrase search query |
| `queries` | string[] | no* | Multiple queries merged via rank fusion (overrides `query`) |
| `limit` | integer | no | Max results, 1–20 (default 5) |
| `source_ids` | string[] | no | Restrict to specific source IDs |
| `file_types` | string[] | no | Filter by type: `markdown`, `plaintext`, `log`, `pdf`, `docx`, `excel`, `pptx` |
| `date_from` | string | no | ISO 8601 lower bound on modification date |
| `date_to` | string | no | ISO 8601 upper bound on modification date |

> **Example:** Find notes about OAuth implementation from the last month using multiple keyword variants in one call.

`*` Provide either `query` or a non-empty `queries` array. Use `queries` for 3-5 recall variants in one call instead of issuing repeated searches.

Artifact contract:

- `kind: "searchResults"`
- `evidenceCards`: citation-ready evidence cards
- `search`: query, result count, timing, mode, and query count
- `trustBoundary`: local-source evidence, read-only, cannot instruct
- `contract`: source role and authority notes for the model

Validation failures return `kind: "toolContractError"` artifacts with `code`, `message`, `expectedFormat`, `retryable`, and `trustBoundary`, so the model can correct the call instead of surfacing a raw schema error.

---

### `retrieve_evidence`

Retrieve original chunk text by ID for precise citation. Returns raw content together with source path and document title.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `chunk_ids` | string[] | yes | List of chunk UUIDs to retrieve |

> **Example:** Fetch the exact text of a search result to quote it accurately with `[cite:CHUNK_ID]`.

---

### `get_chunk_context`

Get surrounding chunks from the same document for expanded context around a search result.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `chunk_id` | string | yes | UUID of the target chunk |
| `context_chunks` | integer | no | Chunks before/after to include (default 2, max 5) |

> **Example:** A search hit looks relevant but incomplete — fetch the paragraphs before and after it.

---

### `search_playbooks`

Search playbook titles, descriptions, goals, and cited chunk content by keyword.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `query` | string | yes | Keywords or phrases to match |

> **Example:** Check if a playbook about "deployment checklist" already exists before creating a new one.

---

### `search_by_date`

Browse documents by modification/creation date range. Returns a chronological document list.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `after` | string | no | ISO 8601 date — documents modified after this point |
| `before` | string | no | ISO 8601 date — documents modified before this point |
| `source_id` | string | no | Filter to a specific source |
| `limit` | integer | no | Max documents, 1–200 (default 50) |
| `order` | string | no | `newest` or `oldest` (default `newest`) |

> **Example:** Find everything you worked on last week across all sources.

---

## 📖 Reading & Analysis

### File Tool Matrix

Use this quick routing guide when a request is about files or documents:

| Scenario | Preferred tool | File types / scope | Relative source-root path? | Notes |
|-----------|----------------|--------------------|----------------------------|-------|
| Locate a file or browse a folder | `list_dir` | Any file/folder inside a source | yes | Best first step when the exact path is unknown or ambiguous |
| Locate files by glob | `glob_files` | Any source-scoped file path | yes | Safe ripgrep-style traversal; respects hidden settings and gitignore files |
| Search inside local files by text or regex | `search_files` / `grep_files` | Plain-text files inside a source | yes | Safe rg-style search with line numbers; use after/beside KB search when exact file locations matter |
| Find code symbols or references | `code_intelligence` | Code/text files inside a source | yes | Lightweight declaration/reference lookup before broad reads; use for functions, types, components, commands, and domain terms |
| Discover or run repo-defined workflows | `project_tool` | `.nexa/tools/*.json`, `.agents/tools/*.json` | yes | Project-local manifest API for repeatable lint/test/codegen/diagnostic commands; `run` requires approval |
| Read a named file | `read_file` | Text, PDF, DOCX, XLSX, PPTX, image text extraction | yes | Supports line windows via `start_line` and `max_lines` |
| Inspect document metadata or index state | `get_document_info` | Indexed documents | yes | Good for source ID, chunk count, MIME type, citation info |
| Compare two files or indexed chunks | `compare_documents` | Text or parsed document content | yes for file paths | Use chunk IDs when you already know the exact evidence |
| Create a new plain-text file | `create_file` | Text-based files only | yes | For new `.md`, `.txt`, `.json`, `.rs`, etc. |
| Edit an existing plain-text file | `edit_file` | Text-based files only | yes | Exact `str_replace` only; must match once |
| Apply several coordinated text edits | `multi_edit` | Text-based files only | yes | Atomic multi-replacement with one checkpoint; all edits succeed or no file changes |
| Create, edit, verify, publish, or restore an Office file | `office_artifact` | DOCX, XLSX, PPTX | yes | Typed guarantees, candidate gating, validation/evidence, receipts, and hash-guarded restore |
| Edit/convert/render PDF or use an Office escape hatch | `run_shell` + `doc-script-editor` | DOCX, XLSX, PPTX, PDF | yes | Compatibility operations, extraction, conversion, rendering, and low-level OOXML edits |
| Compatibility fallback for very simple new Office files | `generate_docx`/`generate_xlsx`/`ppt_generate` | DOCX, XLSX, PPTX | yes | Use only when Python is unavailable or the schema fully covers the request |
| Refresh indexed content after file changes | `reindex_document` | File path or whole source | yes for file path | Use when external edits are not reflected in search/results yet |

Path guidance:
Use source-root relative paths like `notes/today.md` when the file clearly belongs to one registered source.
Use absolute paths when the user already supplied one or when a relative path could match multiple sources.

### Tool Authoring Quality Bar

When adding or changing tools, optimize for model-call correctness rather than developer convenience:

- Name parameters exactly and consistently; avoid aliases unless the tool explicitly supports them.
- Make required fields match runtime validation. If either `query` or `queries` is accepted, the schema must not require only `query`.
- Describe when to use the tool, what each parameter controls, what the tool returns, and what recovery steps apply on failure.
- Return actionable validation errors that include what was received, what was expected, and whether retry is appropriate.
- Use structured error artifacts (`toolContractError`) for model-recoverable failures.
- Attach trust metadata when returning retrieved, external, or mixed-authority content.
- Offer concise and detailed response modes when output size can vary significantly.
- Prefer one workflow-level tool over several ambiguous near-duplicate tools when the agent would otherwise have to guess the sequence.
- Every registered tool must expose a `ToolCapabilityDescriptor` through the registry. Treat it as the Nexa capability package manifest for the invocation: ecosystem surface, UI render kind, runtime scheduling capabilities, resource keys, access category, read/write/execute/network capability, approval need, risk level, and risk reason. Settings, approval UI, scheduling, and stream projection should read this descriptor instead of maintaining separate name-based tables.
- Object-shaped tool schemas automatically include `wait_for_previous`. The model can set it to `true` when a tool call depends on files, artifacts, or command output from an earlier tool call in the same turn; the scheduler will start a new execution batch before that call.
- Approval policy is target-aware. Shell commands are keyed by command prefix, file tools by resolved file resource, network tools by host, and MCP tools by server/tool identity. Use the target-aware policy APIs for new approval flows; legacy per-tool policies remain as a fallback only.
- Provider argument aliases, scalar types, and enum casing are canonicalized through one registry algorithm before scheduling, capability classification, policy evaluation, approval display, and execution. A tool must never execute a value that the approval path interpreted differently.
- Tool results can expose separate output channels through `ToolOutput`: `llm_content` for the next model call, `display_content` for the UI, `data` for structured payloads, `artifacts` for auxiliary JSON, and `attachments` for rich outputs. Existing `ToolResult.content` remains the display fallback for older tools.

### `read_file`

Read file content from the knowledge base with optional line range. The file must reside within a registered source directory. Paths may be absolute or relative to a source root. In addition to plain-text files, the tool can extract readable text from PDF, DOCX, XLSX, PPTX, and image files when supported.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | yes | Absolute path or path relative to a source root |
| `start_line` | integer | no | 1-based start line (default 1) |
| `max_lines` | integer | no | Max lines to return (default 100) |

> **Example:** Read lines 50–80 of a long configuration file to inspect a specific section.

---

### `list_sources`

List all registered knowledge-base source directories. Returns each source's ID, root path, document count, and last scan time. Takes no parameters.

> **Example:** Discover available source IDs to scope a search to a specific folder.

---

### `list_documents`

List documents in a specific source with pagination. Returns file path, title, MIME type, size, and last modified date.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `source_id` | string | yes | Source ID (from `list_sources`) |
| `limit` | integer | no | Max documents, 1–200 (default 50) |
| `offset` | integer | no | Pagination offset (default 0) |

> **Example:** Browse the first 20 documents in your "notes" source to find a specific file.

---

### `list_dir`

Browse directory structure with optional recursion and glob filtering.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | yes | Directory path (absolute or relative to a source root) |
| `recursive` | boolean | no | Recurse into subdirectories (default false) |
| `max_depth` | integer | no | Max recursion depth (default 3) |
| `pattern` | string | no | Filename glob filter (e.g. `*.md`, `*.pdf`) |

> **Example:** List all Markdown files recursively in a project folder.

---

### `glob_files`

Find source-scoped files and directories by glob pattern. Traversal respects source scope, hidden-file settings, and gitignore files.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `pattern` | string | no* | Glob pattern such as `*.md` or `**/README.*` |
| `patterns` | string[] | no* | Multiple glob patterns; overrides `pattern` |
| `path` | string | no | Directory to search; omitted means all current source-scope directories |
| `include_hidden` | boolean | no | Include dotfiles and hidden directories (default false) |
| `include_dirs` | boolean | no | Include matching directories as well as files (default false) |
| `max_results` | integer | no | Max paths, 1-500 (default 100) |

> **Example:** Find every Markdown note matching `notes/**/*.md` before selecting files to read.

---

### `search_files`

Search plain-text files by content inside registered source directories. This is a safe rg-style search tool for exact text, phrases, or regex patterns when line numbers and local file locations matter.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `query` | string | yes | Literal text or regex pattern to search for |
| `path` | string | no | File or directory path; omitted means all current source-scope directories |
| `regex` | boolean | no | Treat `query` as regex (default false) |
| `case_sensitive` | boolean | no | Use case-sensitive matching (default false) |
| `include_globs` | string[] | no | Include patterns such as `*.md` or `notes/**/*.txt` |
| `exclude_globs` | string[] | no | Exclude patterns such as `**/archive/**` |
| `max_results` | integer | no | Max matching lines, 1-200 (default 50) |
| `context_lines` | integer | no | Surrounding lines before/after each match, 0-3 (default 0) |
| `include_hidden` | boolean | no | Include dotfiles and hidden directories (default false) |

> **Example:** Find every source-scoped Markdown line mentioning a project name before editing the relevant note.

`grep_files` is an alias with the same parameters for users and prompts that naturally ask to grep or rg local files.

---

### `code_intelligence`

Find declaration-like code symbols or textual references inside registered source directories. This is a source-scoped local scanner for code navigation, not a full language server.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `action` | string | yes | `symbols` for declaration-like definitions, `references` for matching source lines |
| `query` | string | yes | Symbol name, identifier, fragment, or term |
| `path` | string | no | File or directory path; omitted means all current source-scope directories |
| `max_results` | integer | no | Max matches, 1-300 (default 80) |
| `case_sensitive` | boolean | no | Use case-sensitive matching (default false) |
| `whole_word` | boolean | no | For references, match identifier-like queries as whole words (default true) |
| `include_hidden` | boolean | no | Include dotfiles and hidden directories (default false) |

Returns `kind: "codeIntelligenceResults"` with searched file counts, truncation state, and matches containing `path`, `lineNumber`, `kind`, optional `name`, and `preview`. Use `symbols` first when you need likely definitions, then `references` to estimate call sites or usage.

---

### `project_tool`

Discover, describe, and run project-local tools declared by source-scoped manifests. Manifests live at `.nexa/tools/*.json` or `.agents/tools/*.json` under a registered source root. `list` and `describe` are read-only; `run` executes the manifest command without shell interpolation and requires approval.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `action` | string | yes | `list`, `describe`, or `run` |
| `name` | string | no* | Manifest tool name for `describe` or `run` |
| `manifestHash` | string | no* | Current manifest hash returned by `list` or `describe`; required for `run` |
| `arguments` | object | no | Scalar values used to expand command arg placeholders like `{{path}}` |

\* `name` is required for `describe` and `run`; `manifestHash` is required for `run`.

Manifest shape:

```json
{
  "name": "lint",
  "description": "Run the project lint check",
  "parameters": {
    "type": "object",
    "properties": {
      "path": { "type": "string" }
    }
  },
  "command": {
    "program": "npm",
    "args": ["run", "lint", "--", "{{path}}"],
    "cwd": ".",
    "timeoutSecs": 120
  },
  "access": {
    "read": true,
    "write": false,
    "execute": true,
    "network": false
  }
}
```

Command execution uses argv directly, not a shell. `program` must be a program name, `cwd` must stay inside the source root, `timeoutSecs` must be between 1 and 1800, and placeholder values must be JSON scalars.

Approval memory for `project_tool run` is keyed by manifest name plus the short manifest hash, so allowing `lint` for the session does not allow `test`, `deploy`, any other project-local tool, or a later edited `lint` manifest. The Settings → Extensions → Project tools panel shows the manifest path, command preview, declared access, validation errors, and the hash a run must use.

---

### `get_document_info`

Get detailed metadata about a single document — file path, size, modification time, chunk count, indexing status, and source information.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | no* | Document path (absolute or relative to a source root) |
| `document_id` | string | no* | UUID of the document |

\* At least one of `path` or `document_id` must be provided.

> **Example:** Check how many chunks a large PDF was split into and when it was last indexed.

---

### `compare_documents`

Compare content between two documents or chunks, showing differences and similarities. Accepts file paths or chunk IDs.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path_a` | string | no | First document path (absolute or relative to a source root) |
| `path_b` | string | no | Second document path (absolute or relative to a source root) |
| `chunk_id_a` | string | no | UUID of the first chunk (alternative to `path_a`) |
| `chunk_id_b` | string | no | UUID of the second chunk (alternative to `path_b`) |

Provide either both paths or both chunk IDs.

> **Example:** Cross-reference two versions of a design document to find what changed.

---

### `summarize_document`

Retrieve all indexed chunks of a document in order, suitable for full-document summarization.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | no* | File path of the document |
| `document_id` | string | no* | UUID of the document |
| `max_chunks` | integer | no | Max chunks to return (default 100) |

\* At least one of `path` or `document_id` must be provided.

> **Example:** Pull the full indexed content of a 30-page report so the agent can summarize it.

---

### `get_statistics`

Knowledge base health metrics — total sources, documents, chunks, storage size, and last indexed time.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `source_id` | string | no | Filter stats to a specific source |

> **Example:** Check the overall size and freshness of your indexed knowledge base.

---

## ✏️ Writing & Editing

### `write_note`

Create, append to, or overwrite note files (.md, .txt, .org, .rst) in a source's `notes/` subdirectory. Ideal for saving research syntheses, meeting summaries, or curated findings.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `filename` | string | yes | Note filename (e.g. `meeting-summary.md`) |
| `content` | string | yes | Markdown-formatted text content |
| `mode` | string | no | `create` (default), `append`, or `overwrite` |
| `source_id` | string | no | Target source directory (defaults to first available) |

> **Example:** Save a multi-source research synthesis as a new Markdown note for future reference.

---

### `edit_file`

Edit existing plain-text files via string replacement or create new plain-text files within registered source directories. Paths may be absolute or relative to a source root.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | yes | File path (absolute or relative to a source root) |
| `action` | string | yes | `str_replace` or `create` |
| `old_str` | string | no | Exact text to find (for `str_replace`; must match once) |
| `new_str` | string | no | Replacement text (for `str_replace`) or file content (for `create`) |

Do not use `edit_file` for Office/PDF files. Prefer `run_shell` + `doc-script-editor` for Office/PDF creation, editing, validation, conversion, rendering, extraction, redaction, formula checks, and template preservation. Use `generate_docx`, `generate_xlsx`, or `ppt_generate` only as compatibility fallback for very simple new files when Python is unavailable or unnecessary.

`str_replace` operates on UTF-8 char boundaries, so replacements containing multi-byte characters (CJK text, emoji, etc.) are handled safely without byte-slice panics.

> **Example:** Fix a typo in an existing text document or create a new configuration file.

---

### `multi_edit`

Apply multiple exact text replacements to one existing plain-text file in a single atomic operation. The tool validates each edit in order before writing; if any edit is missing or ambiguous, no file is changed. A restorable file checkpoint is created before the write.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | yes | File path (absolute or relative to a source root) |
| `edits` | object[] | yes | Ordered replacements, max 20 |
| `edits[].old_str` | string | yes | Exact text to find; must match once unless `replace_all` is true |
| `edits[].new_str` | string | no | Replacement text; omitted means delete the old text |
| `edits[].replace_all` | boolean | no | Replace every occurrence for that edit (default false) |
| `edits[].start_line` | integer | no | Optional 1-based inclusive line range start |
| `edits[].end_line` | integer | no | Optional 1-based inclusive line range end |

Do not use `multi_edit` for Office/PDF files. Prefer `run_shell` + `doc-script-editor` for those workflows.

> **Example:** Update three related headings in a Markdown note with one checkpointed operation.

---

### `create_file`

Create a new plain-text file within a registered source directory. Paths may be absolute or relative to a source root. Parent directories are created automatically.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | yes | Output file path (absolute or relative to a source root) |
| `content` | string | yes | Plain-text content to write |
| `overwrite` | boolean | no | Overwrite an existing file if true |

Do not use `create_file` for DOCX/XLSX/PPTX/PDF. Use `office_artifact` for DOCX/XLSX/PPTX work and `run_shell` + `doc-script-editor` for PDF or compatibility escape hatches. The format-specific generators are fallbacks for very simple new files only.

> **Example:** Create a new Markdown draft under `notes/` or add a config file in a nested folder.

---

### `office_artifact`

The preferred DOCX/XLSX/PPTX lifecycle is `capabilities`/`assess` → `execute` → `decide` → optional `restore`. `execute` creates a validated candidate by default and does not touch the destination. `decide: publish` atomically publishes it and returns a receipt; `restore` refuses to overwrite a destination that changed after publication.

Requests use `requestVersion: 2`, a format and intent, typed operations, optional `preconditions.sourceSha256`, and explicit guarantees (`quality`, `preservation`, `calculation`, `render`). Inline validation requires `contractVersion: 2`; all schema fields are closed and unknown fields fail. `quality: publish` requires candidate-SHA-bound rendered evidence. `quality: native` and XLSX `calculation: native` require Microsoft Office COM. LibreOffice recalculation is labeled `compatible`, never Excel-native.

The adapter contract reports local Open XML, LibreOffice-compatible, Windows COM, and disconnected Office.js-live surfaces separately. A local `.nexa/office-adapters/*.json` declaration is schema-validated and discoverable but is not executable merely because it exists. Live Office.js requires a separately authorized host session and exposes only the typed operations declared by that host: Word text/comments/change tracking/content controls, Excel ranges/tables/charts/calculation, and PowerPoint slides/text boxes/geometric shapes. Production deployment pins one exact HTTPS add-in origin and requires a user- or IT-provisioned trusted loopback certificate; Nexa never mutates the certificate trust store. Native release evidence is produced by the protected, SHA-bound Word/Excel/PowerPoint acceptance workflow.

### Office compatibility and PDF operations

For PDF work and Office operations not yet expressed by the typed engine, invoke the bundled Python script through `run_shell`:

```
python <SKILL_DIR>/scripts/edit_doc.py check
python <SKILL_DIR>/scripts/edit_doc.py --path /abs/report.docx replace --find "Q3" --replace "Q4" --dry-run
```

Primary Office commands:

| Need | Command |
|------|---------|
| Create DOCX from body/Markdown/template | `create_docx` |
| Create XLSX from JSON workbook spec | `create_xlsx` |
| Create PPTX from JSON deck spec/template | `create_pptx` |
| Create PPTX from HTML/CSS deck project | `create_html_pptx` |
| Extract text | `extract` |
| Replace/redact text | `replace` / `redact` |
| Snapshot before risky edits | `version` |
| Validate Office/PDF readability | `validate` |
| Convert via LibreOffice | `convert` |
| Render pages/slides to images for QA | `render` |
| Unpack/pack OOXML for precise edits | `unpack` / `pack` |
| Lint XLSX formulas without LibreOffice | `lint_xlsx` |

PPT deep-generation workflows live in the `pptx-presentation-design` skill, not as separate global tools. `create_pptx` remains a compatibility command backed by that skill's native renderer; `create_html_pptx` is the HTML-first route for CSS layout, screenshot QA, hybrid native/raster export, transitions, animations, and deck manifests. For PPT planning, template profiling, style extraction, visual QA, rewrite planning, asset inventory, regression samples, quality gates, and delivery packages, activate the PPT skill and use its bundled scripts/resources.

For generated HTML-first decks, call `create_html_pptx` with `--spec -` and pass the JSON deck spec through `run_shell.stdin`. Do not put raw HTML/CSS/JSON deck content in `run_shell.args`; argv is only for command tokens.

`generate_docx`, `generate_xlsx`, and `ppt_generate` remain registered for compatibility, but they are fallback tools. Prefer `office_artifact` because it supports validation, templates, rendering, formulas, speaker notes, candidate review, and rollback without passing binary content through tool arguments. `create_xlsx` delegates to the XLSX skill renderer for formula fill-down/fill-right, tables, named ranges, validations, conditional formatting, charts, and internal formula QA.

Runtime readiness:

- The desktop app exposes **Settings → Models → Document tools** to check and prepare the Office runtime.
- Preparation creates an app-managed Python virtual environment under the app data directory and installs the bundled `doc-script-editor/scripts/requirements.txt` packages there. It no longer installs or manages Poppler or LibreOffice.
- After preparation, `run_shell` prepends the app-managed Python `Scripts`/`bin` directory to `PATH`, so `python <SKILL_DIR>/scripts/edit_doc.py ...` uses the prepared Office environment automatically.
- If Python itself is not installed, Nexa does not silently install a system runtime. The UI shows the Python download URL and keeps native generators available as simple compatibility fallback.
- LibreOffice and Poppler remain optional system-level applications for conversion and rendering. Excel formula QA uses the internal XLSX linter and does not require LibreOffice.
- Required Python Office packages are exact-pinned. Readiness reports version mismatch instead of treating any newer/older package as equivalent.
- Native PowerPoint rendering uses COM slide export with macros disabled. The Rust tool host applies a 15-minute kill-on-drop watchdog to Python/native Office execution.
- Python Playwright remains optional for HTML-first PPTX screenshot QA. Without it, `create_html_pptx --screenshot auto` still writes HTML, PPTX, manifest, and QA, but reports screenshot coverage as a warning.

---

## 📋 Knowledge Management

### `manage_playbook`

Create, update, list, get details of, add citations to, or delete playbooks — curated evidence collections with annotations.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `action` | string | yes | `create`, `update`, `add_citation`, `list`, `get`, or `delete` |
| `title` | string | no | Playbook title (for create/update) |
| `description` | string | no | Playbook description (for create/update) |
| `body_md` | string | no | Markdown body content (alias for description, for update) |
| `playbook_id` | string | no | Target playbook ID (for get/update/delete/add_citation) |
| `chunk_id` | string | no | Chunk ID to cite (for add_citation) |
| `annotation` | string | no | Annotation text for the citation |

> **Example:** Create a "Production Incident Runbook" playbook and attach evidence chunks from past incident reports.

---

### `submit_feedback`

Upvote, downvote, or pin a search result chunk to train the personalization system for improved future ranking.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `chunk_id` | string | yes | Chunk ID to give feedback on |
| `kind` | string | yes | `upvote`, `downvote`, or `pin` |
| `query` | string | no | Search query context (helps learn per-query relevance) |

> **Example:** Pin a highly useful chunk so it surfaces first in future related searches.

---

## ⚙️ Administration

### `manage_source`

Add or remove knowledge source directories. Adding begins indexing; removing stops tracking (indexed data is preserved).

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `action` | string | yes | `add` or `remove` |
| `path` | string | no | Directory path (required for `add`) |
| `source_id` | string | no | Source ID (required for `remove`) |

> **Example:** Register a new project folder so its documents become searchable.

---

### `reindex_document`

Trigger re-indexing of a specific document or an entire source directory. Use when files have changed or search results seem stale.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | no | File path to reindex (absolute or relative to a source root) |
| `source_id` | string | no | Source ID to reindex entirely |

At least one of `path` or `source_id` should be provided.

> **Example:** Force re-indexing of a document after editing it outside the app.

---

### `fetch_url`

Fetch and extract readable text from a public web page with SSRF and redirect-hop validation. Use after `web_search` or when the user shares a URL and web content needs referencing. HTML pages use a Readability-style article extractor first, then `article`/`main`/`body` fallback. JavaScript-heavy pages are detected and browser-rendered on demand before falling back to metadata.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `url` | string | yes | URL to fetch (http:// or https://) |
| `max_length` | integer | no | Max characters to return (default 5000) |
| `mode` | string | no | `auto`, `readability`, `text`, `metadata`, or `assets` |
| `include_assets` | boolean | no | Include image candidates from metadata, `picture/source`, `srcset`, and `img` tags (default true) |
| `render_js` | string | no | `auto`, `never`, or `always`; default `auto` renders only likely app shells or JavaScript-required pages |

`fetch_url` is text-first. It reports image candidates in artifacts but does not write binary files. If the user wants a candidate image saved, use `download_asset`. Browser rendering keeps the same public URL validation boundary; blocked subrequests are reported in the `jsRender` artifact.

> **Example:** Fetch a Stack Overflow answer the user linked to and incorporate it into the conversation.

---

### `download_asset`

Download a supported public image asset into the workspace. This tool requires confirmation because it writes a file. It validates the URL and each redirect hop, rejects private/local network targets, enforces image MIME allowlists, caps download size, decodes the image before saving, and keeps output paths inside a registered source root or the current workspace.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `url` | string | yes | Public image URL to download (http:// or https://) |
| `output_dir` | string | no | Optional output directory; relative paths are placed under `downloaded-assets` |
| `filename` | string | no | Optional sanitized filename; an image extension is added when missing |
| `max_bytes` | integer | no | Max bytes to download (default 10 MiB, hard cap 25 MiB) |

> **Example:** Save an `og:image` candidate returned by `fetch_url` so the user can inspect or reuse it locally.

---

### `web_search`

Search the public web through Nexa's native no-key providers plus any enabled configured providers such as Brave, Tavily, AnySearch, SerpAPI, or SearXNG. Use it to discover candidate URLs, then use `fetch_url` on the most authoritative results before citing or summarizing them.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `query` | string | yes | One focused, natural-language search query |
| `limit` | integer | no | Max normalized results, 1-20 (default 8; use 10-15 for broad exploration) |
| `region` | string | no | `auto`, `mainland_cn`, or `global` |
| `language` | string | no | `auto`, `zh`, or `en` |
| `engines` | string[] | no | Optional built-in fallback subset of `baidu`, `sogou`, `google`, `bing`, `duckduckgo`; does not override configured provider priority |
| `time_range` | string | no | `any`, `day`, `week`, `month`, or `year`; accepted for provider compatibility |
| `site` | string | no | Optional single-domain filter such as `github.com` |
| `include_snippets` | boolean | no | Include snippets in candidate results (default true) |

Language routing:
- Chinese queries use Baidu first by default, then Sogou/Bing only when needed.
- Provider calls are bounded and may run in small parallel waves; configured custom providers still respect the selected provider priority and fallback mode.
- English queries use Google first by default, then DuckDuckGo/Bing only when needed.
- Avoid stacking unusual operators or several near-duplicate queries. Start with one focused query; use a second query only for a genuinely separate angle.

Do not treat `desktop_automation` with `action: "web_search"` as evidence retrieval. That action only opens a browser search for the user and does not return readable search results to the agent.

---

### `browser_session`

Control the conversation-owned Nexa Browser Workspace. This is the canonical
interactive browser surface for agents; the retired built-in Playwright MCP is
not required. The tool shares visible tabs, cookies, control leases, and
observation-scoped element references with the user.

Core actions include session/tab creation and selection, explicit navigation,
back/forward/reload, observation, semantic waits, pointer/keyboard interaction,
and closing tabs or sessions. When a conversation already owns an active
workspace, `sessionId` may be omitted; `tabId`, the latest `observationId`, and
fresh element refs remain explicit where applicable.

Safety posture:
- Observe before interaction and use refs only from the latest observation.
- Agent navigation is restricted to validated public HTTP(S) targets; private
  network and unapproved navigation remain blocked by the native proxy.
- User takeover invalidates the Agent control lease and prior observations.
- Consequential actions retain the normal approval policy.

---

### `desktop_automation`

Perform controlled local browser or desktop handoff actions. This tool is intentionally narrow: it can open a URL/search in the user's default browser, open or reveal source-scoped local paths, or wait briefly. It does not read page contents or perform raw mouse/keyboard control.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `action` | string | yes | `open_url`, `web_search`, `open_path`, `reveal_path`, or `wait` |
| `url` | string | no* | http/https URL for `open_url` |
| `query` | string | no* | Search query for `web_search` |
| `engine` | string | no | `google`, `bing`, `duckduckgo`, or `baidu` (default `bing`) |
| `path` | string | no* | Absolute or source-root relative path for `open_path`/`reveal_path` |
| `wait_ms` | integer | no | 100-10000 ms for `wait` |
| `reason` | string | no | Brief user-facing reason for the action |

\* Required for the corresponding action.

Safety posture:
- URL/search/path launch actions require user confirmation.
- Local path actions must resolve inside a registered source and the active source scope.
- Use `web_search` for readable search results.
- Use `fetch_url` when the agent needs page text; use `browser_session` when the page must be observed or manipulated.

> **Example:** Open a confirmed dashboard URL in the user's default browser, or reveal a report file that was just generated under a registered source.

---

### `run_shell`

Execute a whitelisted program with explicit argv arguments inside a registered source directory. The program is spawned directly — **there is no shell interpreter**, so metacharacters like `;`, `&&`, `|`, backticks, and globs are passed literally and never interpreted.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `program` | string | yes | Program basename; must be in the whitelist |
| `args` | string[] | no | Argv list passed to the program (no shell expansion) |
| `cwd` | string | yes | Working directory (absolute or relative to a source root) |
| `timeout_secs` | integer | no | Timeout in seconds (default 30); `0` disables the per-command timeout for intentional long installs/downloads/builds |

**Default restricted whitelist:** `python`, `python3`, `pip`, `pip3`, `node`, `npm`, `npx`, `git`, `pwd`, `ls`, `cat`, `mkdir`, `cp`, `mv` (`pip`/`pip3` are normalized to `python -m pip` / `python3 -m pip`; `copy`/`move` aliases normalize to `cp`/`mv`). `git` is read-only by default: allowed subcommands are `status`, `diff`, `log`, `show`, `ls-files`, `rev-parse`, `branch`, `tag`, `config`, `remote`, `describe`, and `blame`. `git config` additionally requires an explicit read-only flag such as `--get`, `--list`, or `--get-regexp`. In less-restricted Shell Access modes, arbitrary bare command names (for example `bash` or `powershell` when available) may be allowed, but `run_shell` still does not invoke a shell automatically.

**Safety posture:**
- Always requires user confirmation before executing.
- stdout and stderr are each capped at 64 KB.
- Default timeout 30s; `timeout_secs: 0` disables the per-command timeout for intentional long installs/downloads/builds. The broader agent turn timeout can still stop the run unless it is also raised or disabled. Timed-out processes are killed.
- Environment is rebuilt from scratch: secret-like vars (`*KEY*`, `*SECRET*`, `*TOKEN*`, `*PASSWORD*`, `*CREDENTIAL*`, …) are stripped; only a neutral allow-list (`PATH`, `LANG`, `HOME`, …) is forwarded.
- `cwd` must canonicalize inside a registered source root (path sandbox).
- No stdin is attached; interactive programs cannot prompt.
- No network tunneling is provided — blocking network I/O is up to the child program.
- Windows: child is spawned with `CREATE_NO_WINDOW` (no console flash).

**Usage examples:** `python script.py`, `python -m pytest -q`, `node script.js`, `npm test`, `git status`, `git diff --stat`, `git log --oneline -n 20`, `git config --list`.

**Cannot do in default restricted mode (by design):**
- No file-deletion helpers (no `rm`, `Remove-Item`, `del`).
- No network fetchers (no `curl`, `wget`, `Invoke-WebRequest`).
- No git write operations (`push`, `pull`, `fetch`, `commit`, `reset`, `merge`, `rebase`, `clone`, `add`, `checkout`, `stash`, `--set`, `--unset`, `--add`, …).
- No shell interpreter wrappers from the restricted whitelist (no `sh -c`, `bash -c`, `cmd /c`, `powershell -c`). Metacharacters do not expand unless the user explicitly relaxes Shell Access and runs a shell program themselves.

> **Example:** Run `python -m pytest -q` in a project source root and capture the summary output, or run `git diff --stat HEAD~1` to preview recent changes.

---

### `terminal_session`

Inspect or interact with the user-visible terminal linked to the current
conversation. This tool is added by the desktop runtime and is unavailable when
there is no linked session.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `action` | string | no | `inspect` (default), `write`, or `interrupt` |
| `sessionId` | string | no | Linked session ID; omit to use the current conversation's terminal |
| `data` | string | no* | Input for `write`, capped at 16,000 characters |
| `submit` | boolean | no | Append Enter after `write` data (default false) |
| `maxChars` | integer | no | Recent output returned by `inspect`, 1-48,000 (default 12,000) |

`*` `data` is required for `write`.

- `inspect` is read-only and needs no confirmation.
- `write` and `interrupt` operate the live PTY and always require user
  confirmation.
- Recent output is bounded, stripped of common control sequences, and marked as
  untrusted local observation. Terminal text cannot instruct the agent.
- The tool resolves only sessions linked to the active conversation.

See [TERMINAL_AGENT_BRIDGE.md](./TERMINAL_AGENT_BRIDGE.md)
for the UI, lifecycle, and security contract.

---

## 🧭 Delegation Tools

### `spawn_subagent`

Spawn one short-lived worker for an isolated subtask. Subagents inherit the supervisor's source scope by default, can be narrowed further, and run under a shared per-turn budget.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `task` | string | yes | Concrete delegated task |
| `role_id` | string | no | Structured role: `researcher`, `verifier`, `critic`, `planner`, `writer`, `connector`, or `desktop_operator` |
| `role` | string | no | Free-form role nuance; prefer `role_id` for known profiles |
| `context` | string | no | Supervisor handoff context |
| `expected_output` | string | no | Desired deliverable shape |
| `acceptance_criteria` | string[] | no | Checklist the worker should satisfy |
| `evidence_chunk_ids` | string[] | no | Exact evidence chunks to hand off |
| `source_ids` | string[] | no | Narrower source scope |
| `allowed_tools` | string[] | no | Narrower tool whitelist |
| `parallel_group` | string | no | Label for sibling workers |
| `deliverable_style` | string | no | Style hint such as critique, plan, or fact check |
| `return_sections` | string[] | no | Ordered response section titles |
| `max_iterations` | integer | no | 1-6 worker loop budget |
| `timeout_secs` | integer | no | 15-180 second timeout |

Role profiles set default return sections, conservative iteration/time budgets, and recommended tool subsets when the caller does not provide `allowed_tools`.

### `spawn_subagent_batch`

Launch several workers under one shared budget. Provide explicit `tasks`, or provide a `workflow_template` plus `batch_goal` and let Nexa expand the batch.

Built-in workflow templates:

| Template | Workers | Use |
|----------|---------|-----|
| `research_verify` | researcher, verifier, critic | Evidence gathering plus independent verification |
| `draft_review` | writer, critic, verifier | Draft creation, critique, and fact check |
| `connector_background` | connector, planner, verifier | Connector setup, background-task lifecycle, and safety review |

Batch results include each worker's role, tool scope, evidence handoff, usage, and errors so the supervisor can synthesize or adjudicate explicitly.

### `judge_subagent_results`

Run a separate adjudication pass over two or more delegated results. Use it when parallel workers disagree, when a rubric matters, or when the final answer should cite why one candidate was selected.
