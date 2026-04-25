---
name: doc-script-editor
description: Activate when creating or editing DOCX, PPTX, PDF, or XLSX files on disk with Python-backed fidelity — text replace, paragraph/slide insert, text extraction, redaction, snapshotting, or format-aware document work.
---

## Trigger
Creating or editing a `.docx` / `.pptx` / `.pdf` / `.xlsx` file on disk via the `run_shell` tool invoking `scripts/edit_doc.py` or a short Python script using the bundled requirements.

## When to use
- Targeted text replace inside a `.docx` or `.pptx` while preserving formatting
- Extracting plain text from a `.pdf` / `.docx` / `.pptx` for review or summarization
- Inserting a new slide into an existing `.pptx` at a specific position
- Redacting sensitive substrings across a document
- Creating a versioned snapshot before a risky edit
- Creating a new Office document when the user cares about layout, tables, formulas, speaker notes, charts, template compatibility, or repeatable Python control

## When NOT to use
- Plain text / source files → use `edit_file`
- Simple brand-new Office files where `generate_docx` / `generate_xlsx` / `ppt_generate` covers the requested structure
- Simple one-off text edits in a docx/pptx/xlsx where `edit_document` already works — that path is faster and needs no Python

## Critical rule
**NEVER paste file contents, binary bytes, or base64 blobs into tool arguments.** Pass only the absolute `--path` plus operation parameters. The script reads and writes bytes on disk itself.

## Invocation pattern
All commands run through `run_shell` with `python` (or `python3`):

1. DOCX text replace (with preview first):
   ```
   python <SKILL_DIR>/scripts/edit_doc.py --path /abs/report.docx replace --find "Q3" --replace "Q4" --dry-run
   python <SKILL_DIR>/scripts/edit_doc.py --path /abs/report.docx replace --find "Q3" --replace "Q4"
   ```
2. PDF text extract for review:
   ```
   python <SKILL_DIR>/scripts/edit_doc.py --path /abs/whitepaper.pdf extract --pages 1-3
   ```
3. PPTX slide insert:
   ```
   python <SKILL_DIR>/scripts/edit_doc.py --path /abs/deck.pptx insert_slide --after 2 --title "Results" --body "Revenue up 18% QoQ"
   ```
4. Redact confidential strings in DOCX:
   ```
   python <SKILL_DIR>/scripts/edit_doc.py --path /abs/memo.docx redact --find "confidential" --replace "[REDACTED]"
   ```
5. Create a brand-new Office file when native generators are too limited:
   ```
   python <SKILL_DIR>/scripts/edit_doc.py check
   python /abs/source/scripts/build_report.py
   ```
   Put the custom script inside an approved source/workspace path, use `python-docx`, `openpyxl`, or `python-pptx`, and write the final `.docx`/`.xlsx`/`.pptx` directly to disk.

Always call `check` first in a fresh environment:
```
python <SKILL_DIR>/scripts/edit_doc.py check
```

## Better-than-openclaw principles
- **Diff preview** — `--dry-run` on `replace` / `redact` prints a unified diff instead of mutating the file
- **Sidecar versioning** — `version` subcommand writes `.nexa/doc-history/<name>/v{N}/<file>` snapshots
- **Undo stack** — every snapshot is addressable by version number, nothing is clobbered in place
- **Chunked streaming** — `extract` truncates > 50 KB output with a clear notice so large docs don't blow the context
- **Capability check** — `check` subcommand reports available/missing backends with exit code 2 if core deps are absent

## Dependencies
Install before first Office/PDF operation (only what's needed for the target format):
```
python -m pip install -r <SKILL_DIR>/scripts/requirements.txt
```
Optional for format conversion / PDF rendering: `libreoffice`, `pypandoc`.

## Handling missing dependencies
Before first use, or when the user targets an unfamiliar file type, run:

```
python <SKILL_DIR>/scripts/edit_doc.py check
```

The `check` subcommand lists each backend as `OK (version)` or `MISSING`. If any required backend is missing:

1. Tell the user (in their language) which packages are missing and ask permission to install them.
2. If approved, invoke `run_shell` with:
   ```
   python -m pip install <pkg1> <pkg2> ...
   ```
   Use `python -m pip install` rather than `pip install` — `run_shell` whitelists `python`, not `pip`, and `-m pip` is the canonical way to reach pip for the same interpreter.
3. Re-run `check` to confirm, then proceed with the original operation.
4. If install fails (network / permissions / no pip): relay stderr verbatim and suggest the user either install Python (https://python.org/downloads) or run `pip install <pkg>` manually in their own terminal.

Only install backends the user actually needs — don't pull `python-pptx` for a pure docx edit.

### Operation → backend matrix

| Operation      | File type        | Required backend |
|----------------|------------------|------------------|
| replace        | .docx            | python-docx      |
| replace        | .pptx            | python-pptx      |
| redact         | .docx / .pptx    | python-docx / python-pptx |
| extract        | .docx            | python-docx      |
| extract        | .pptx            | python-pptx      |
| extract        | .pdf             | pypdf            |
| insert_slide   | .pptx            | python-pptx      |
| version        | any              | (none)           |

When a backend is missing at runtime, subcommands exit `2` with `MISSING_DEP: <pkg>` on stderr plus the exact `python -m pip install <pkg>` hint.

## Exit codes
- `0` success
- `1` generic error
- `2` missing dependency (prints `MISSING_DEP: <pkg>`)
- `3` bad input / path validation failed
