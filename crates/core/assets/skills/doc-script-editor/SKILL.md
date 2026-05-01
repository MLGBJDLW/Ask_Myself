---
name: doc-script-editor
description: Activate when creating, editing, validating, converting, rendering, unpacking, or analyzing DOCX, PPTX, PDF, or XLSX files on disk with Python-backed fidelity — Office creation, template-aware edits, OOXML surgery, text replacement, slide insert, extraction, redaction, snapshotting, validation, conversion, visual QA, formula recalculation, or format-aware document work.
---

## Trigger
Creating, editing, validating, converting, rendering, unpacking, or analyzing a `.docx` / `.pptx` / `.pdf` / `.xlsx` file on disk via the `run_shell` tool invoking `scripts/edit_doc.py` or a short Python script using the bundled requirements.

## Pairing
Use this skill as the execution backend. Pair it with the format skill that carries design and QA rules:

- `docx-document-design` for Word/DOCX reports, memos, proposals, and template-preserving document work
- `pptx-presentation-design` for PowerPoint decks, slides, speaker notes, and template decks
- `xlsx-workbook-design` for Excel workbooks, spreadsheets, dashboards, formulas, and financial models

## When to use
- Creating new DOCX, XLSX, or PPTX files with Python libraries when the result must be a real Office artifact
- Targeted text replace inside a `.docx`, `.pptx`, or `.xlsx` while preserving formatting
- Extracting plain text from a `.pdf` / `.docx` / `.pptx` / `.xlsx` for review or summarization
- Inserting a new slide into an existing `.pptx` at a specific position
- Redacting sensitive substrings across a document
- Validating Office ZIP structure and backend readability after generation/editing
- Converting Office files to PDF or legacy formats via LibreOffice when available
- Rendering DOCX/PPTX/XLSX/PDF pages to images for visual QA when LibreOffice and Poppler are available
- Unpacking/repacking DOCX/PPTX/XLSX OOXML for template-aware edits, comments, relationship fixes, image replacement, or structure repair
- Recalculating XLSX formulas and scanning for residual Excel formula errors
- Creating a versioned snapshot before a risky edit
- Creating a new Office document when the user cares about layout, tables, formulas, speaker notes, charts, template compatibility, or repeatable Python control

## When NOT to use
- Plain text / source files → use `edit_file`

## Critical rule
**NEVER paste file contents, binary bytes, or base64 blobs into tool arguments.** Pass only the absolute `--path` plus operation parameters. The script reads and writes bytes on disk itself.

## Invocation pattern
For this skill, invoke the bundled document script through `run_shell` with `python` (or `python3`). This is a Python backend requirement for the Office/PDF workflow, not a general restriction on other `run_shell` programs or less-restricted shell access modes:

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
5. Create a brand-new Office file with the Python-backed workflow:
   ```
   python <SKILL_DIR>/scripts/edit_doc.py check
   python <SKILL_DIR>/scripts/edit_doc.py --path /abs/source/report.docx create_docx --title "Board Report" --input-md /abs/source/report_content.md
   python <SKILL_DIR>/scripts/edit_doc.py --path /abs/source/model.xlsx create_xlsx --spec /abs/source/workbook_spec.json
   python <SKILL_DIR>/scripts/edit_doc.py --path /abs/source/deck.pptx create_pptx --spec /abs/source/deck_spec.json
   ```
   For complex generation, put a short custom script inside an approved source/workspace path, use `python-docx`, `openpyxl`, or `python-pptx`, and write the final `.docx`/`.xlsx`/`.pptx` directly to disk.
6. Validate and convert after generation:
   ```
   python <SKILL_DIR>/scripts/edit_doc.py --path /abs/source/report.docx validate
   python <SKILL_DIR>/scripts/edit_doc.py --path /abs/source/report.docx convert --to pdf --outdir /abs/source/out
   ```
7. Render pages/slides for visual QA:
   ```
   python <SKILL_DIR>/scripts/edit_doc.py --path /abs/source/deck.pptx render --outdir /abs/source/rendered --dpi 150 --format png
   ```
8. Use OOXML workflow for precise template edits:
   ```
   python <SKILL_DIR>/scripts/edit_doc.py --path /abs/source/template.pptx unpack --outdir /abs/source/template_unpacked --overwrite
   # edit XML/media/relationships inside template_unpacked
   python <SKILL_DIR>/scripts/edit_doc.py --path /abs/source/output.pptx pack --input-dir /abs/source/template_unpacked
   python <SKILL_DIR>/scripts/edit_doc.py --path /abs/source/output.pptx validate
   ```
9. Recalculate and verify Excel formulas:
   ```
   python <SKILL_DIR>/scripts/edit_doc.py --path /abs/source/model.xlsx recalc_xlsx
   python <SKILL_DIR>/scripts/edit_doc.py --path /abs/source/model.xlsx validate
   ```

Always call `check` first in a fresh environment:
```
python <SKILL_DIR>/scripts/edit_doc.py check
```

## Decision tree

1. Existing Office/PDF file? Use `version` first for risky changes, then `replace`, `redact`, `insert_slide`, `extract`, `validate`, or a custom Python script.
2. New DOCX/XLSX/PPTX and Python is available? Use `create_docx`, `create_xlsx`, or `create_pptx` first. Prefer a JSON spec for spreadsheets/decks and a markdown/body input for documents.
3. Need template fidelity, comments, tracked changes, precise image replacement, relationship repair, or layout surgery? Use `unpack` → XML/media edit → `pack` → `validate`; do not use rigid one-shot generators.
4. Need PDF/image preview or conversion QA? Use `render` when Poppler is available, or `convert --to pdf` with LibreOffice then inspect/extract.
5. XLSX contains formulas? Use `recalc_xlsx` after writing formulas, then `validate` to scan for formula errors.
6. Python/LibreOffice unavailable? Explain the missing runtime and what package or converter is needed; do not fall back to deleted native Office generator tools.

## Adopted Office-skill patterns

- Keep the useful parts: Python Office libraries, OOXML unpack/pack escape hatch, isolated LibreOffice conversion profiles, visual render QA, XLSX formula recalculation, and explicit validation.
- Do not use external hard-coded skill paths, external author names, assumptions that every binary is preinstalled, or Node-first DOCX/PPTX generation as the default.
- Do not paste binary/base64 Office content into tool calls. All Office bytes stay on disk and are passed by absolute path.

## Better-than-openclaw principles
- **Diff preview** — `--dry-run` on `replace` / `redact` prints a unified diff instead of mutating the file
- **Sidecar versioning** — `version` subcommand writes `.nexa/doc-history/<name>/v{N}/<file>` snapshots
- **Undo stack** — every snapshot is addressable by version number, nothing is clobbered in place
- **Chunked streaming** — `extract` truncates > 50 KB output with a clear notice so large docs don't blow the context
- **Capability check** — `check` subcommand reports available/missing backends with exit code 2 if core deps are absent
- **Validate after write** — `validate` opens the file with its backend and checks Office ZIP integrity
- **Visual QA** — `render` converts Office/PDF pages to PNG/JPEG images with isolated LibreOffice profiles
- **Conversion QA** — `convert` uses LibreOffice headless with an isolated user profile for PDF previews and format conversion
- **OOXML escape hatch** — `unpack` / `pack` make low-level template and relationship fixes possible without passing binary data through tool arguments
- **Formula safety** — `recalc_xlsx` uses LibreOffice when available and reports Excel formula errors as structured JSON

## Dependencies
In the desktop app, first prefer `prepare_document_tools` when that tool is available. Call `action: "check"` to inspect readiness, then call `action: "prepare"` with `include_optional_tools: false` for missing required Python dependencies. Ask the user before retrying with `include_optional_tools: true` because LibreOffice/Poppler setup may download binaries or touch system package managers. The same flow is exposed in Settings → Models → Document tools → Prepare. It creates an app-managed virtual environment, installs the bundled requirements there, attempts optional tool setup (app-managed Poppler on Windows, system package-manager install for LibreOffice/Poppler when available), and makes `run_shell` prefer those managed paths automatically.

For CLI/dev environments, install before first Office/PDF operation (only what's needed for the target format):
```
python -m pip install -r <SKILL_DIR>/scripts/requirements.txt
```
Optional for format conversion / PDF rendering: `libreoffice`, Poppler. These are best prepared through the desktop readiness flow because they are system or app-managed binaries rather than Python packages.

## Handling missing dependencies
Before first use, or when the user targets an unfamiliar file type, run:

```
python <SKILL_DIR>/scripts/edit_doc.py check
```

The `check` subcommand lists each backend as `OK (version)` or `MISSING`. If any required backend is missing:

1. In the desktop app, invoke `prepare_document_tools` when available; otherwise ask the user to run Settings → Models → Document tools → Prepare first.
2. In CLI/dev contexts, tell the user (in their language) which packages are missing and ask permission to install them.
3. If approved, invoke `run_shell` with:
   ```
   python -m pip install <pkg1> <pkg2> ...
   ```
   Prefer `python -m pip install` over `pip install` so dependencies land in the same interpreter/environment that will run `edit_doc.py`. `run_shell` may normalize `pip`/`pip3` to `python -m pip`, but the explicit form is clearer and more portable.
4. Re-run `check` to confirm, then proceed with the original operation.
5. If install fails (network / permissions / no pip): relay stderr verbatim and suggest the user either install Python (https://python.org/downloads) or run `pip install <pkg>` manually in their own terminal.

Only install backends the user actually needs — don't pull `python-pptx` for a pure docx edit.

### Operation → backend matrix

| Operation      | File type        | Required backend |
|----------------|------------------|------------------|
| create_docx    | .docx            | python-docx      |
| create_xlsx    | .xlsx            | openpyxl         |
| create_pptx    | .pptx            | python-pptx      |
| unpack         | .docx/.pptx/.xlsx | (none)           |
| pack           | .docx/.pptx/.xlsx | (none)           |
| replace        | .docx            | python-docx      |
| replace        | .pptx            | python-pptx      |
| replace        | .xlsx            | openpyxl         |
| redact         | .docx / .pptx    | python-docx / python-pptx |
| extract        | .docx            | python-docx      |
| extract        | .pptx            | python-pptx      |
| extract        | .xlsx            | openpyxl         |
| extract        | .pdf             | pypdf            |
| insert_slide   | .pptx            | python-pptx      |
| render         | Office/PDF       | LibreOffice + Poppler |
| recalc_xlsx    | .xlsx            | LibreOffice + openpyxl |
| validate       | .docx/.pptx/.xlsx/.pdf | matching backend |
| convert        | Office/PDF       | LibreOffice      |
| version        | any              | (none)           |

When a backend is missing at runtime, subcommands exit `2` with `MISSING_DEP: <pkg>` on stderr plus the exact `python -m pip install <pkg>` hint.

## Exit codes
- `0` success
- `1` generic error
- `2` missing dependency (prints `MISSING_DEP: <pkg>`)
- `3` bad input / path validation failed
