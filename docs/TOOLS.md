# Tool Reference

Ask Myself ships with 20 built-in tools that the AI agent calls autonomously during conversations. Every tool operates locally against your indexed knowledge base.

---

## 🔍 Search & Retrieval

### `search_knowledge_base`

Hybrid full-text (BM25) and vector search across all indexed content. Returns evidence cards with content, source paths, relevance scores, and chunk IDs for citation. Supports batch queries via the `queries` parameter for synonym/variant expansion in a single call.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `query` | string | yes | Concise noun-phrase search query |
| `queries` | string[] | no | Multiple queries merged via rank fusion (overrides `query`) |
| `limit` | integer | no | Max results, 1–20 (default 5) |
| `source_ids` | string[] | no | Restrict to specific source IDs |
| `file_types` | string[] | no | Filter by type: `markdown`, `plaintext`, `log`, `pdf`, `docx`, `excel`, `pptx` |
| `date_from` | string | no | ISO 8601 lower bound on modification date |
| `date_to` | string | no | ISO 8601 upper bound on modification date |

> **Example:** Find notes about OAuth implementation from the last month using multiple keyword variants in one call.

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

### `read_file`

Read file content from the knowledge base with optional line range. The file must reside within a registered source directory.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | yes | Absolute or relative file path |
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
| `path` | string | yes | Directory path (must be within a registered source) |
| `recursive` | boolean | no | Recurse into subdirectories (default false) |
| `max_depth` | integer | no | Max recursion depth (default 3) |
| `pattern` | string | no | Filename glob filter (e.g. `*.md`, `*.pdf`) |

> **Example:** List all Markdown files recursively in a project folder.

---

### `get_document_info`

Get detailed metadata about a single document — file path, size, modification time, chunk count, indexing status, and source information.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | no* | File path of the document |
| `document_id` | string | no* | UUID of the document |

\* At least one of `path` or `document_id` must be provided.

> **Example:** Check how many chunks a large PDF was split into and when it was last indexed.

---

### `compare_documents`

Compare content between two documents or chunks, showing differences and similarities. Accepts file paths or chunk IDs.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path_a` | string | no | File path of the first document |
| `path_b` | string | no | File path of the second document |
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

Edit existing files via string replacement or create new files within registered source directories. Supports all text-based file types.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | yes | File path (absolute or relative to a source) |
| `action` | string | yes | `str_replace` or `create` |
| `old_str` | string | no | Exact text to find (for `str_replace`; must match once) |
| `new_str` | string | no | Replacement text (for `str_replace`) or file content (for `create`) |

> **Example:** Fix a typo in an existing document or create a new configuration file.

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
| `path` | string | no | File path to reindex |
| `source_id` | string | no | Source ID to reindex entirely |

At least one of `path` or `source_id` should be provided.

> **Example:** Force re-indexing of a document after editing it outside the app.

---

### `fetch_url`

Fetch and extract text content from a web page (HTML stripped). Use when the user shares a URL or web content needs referencing.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `url` | string | yes | URL to fetch (http:// or https://) |
| `max_length` | integer | no | Max characters to return (default 5000) |

> **Example:** Fetch a Stack Overflow answer the user linked to and incorporate it into the conversation.
