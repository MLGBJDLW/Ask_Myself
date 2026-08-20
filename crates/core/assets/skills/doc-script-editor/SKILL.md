---
name: doc-script-editor
description: Activate when creating, editing, validating, converting, rendering, unpacking, or analyzing DOCX, PPTX, PDF, or XLSX files on disk with Python-backed fidelity — Office creation, template-aware edits, OOXML surgery, text replacement, slide insert, extraction, redaction, snapshotting, validation, conversion, visual QA, formula linting, or format-aware document work.
---

## Trigger
Creating, editing, validating, converting, rendering, unpacking, or analyzing a `.docx` / `.pptx` / `.pdf` / `.xlsx` file on disk. For DOCX/PPTX/XLSX, prefer the `office_artifact` tool and its candidate → decide → receipt lifecycle. Use `scripts/edit_doc.py` for focused compatibility operations and PDF work; `scripts/office_artifact_service.py` remains the jobVersion 1 compatibility layer.

## Pairing
Use this skill as the execution backend. Pair it with the format skill that carries design and QA rules:

- `docx-document-design` for Word/DOCX reports, memos, proposals, and template-preserving document work
- `pptx-presentation-design` for PowerPoint decks, slides, speaker notes, and template decks
- `xlsx-workbook-design` for Excel workbooks, spreadsheets, dashboards, formulas, and financial models

Keep format-specific generation logic in the format skill. In particular, `create_pptx` is a backward-compatible command that delegates to `pptx-presentation-design/scripts/pptx_renderer.py`; `create_html_pptx` delegates to `pptx-presentation-design/scripts/html_deck_renderer.py` for the HTML-first deck route. New PPT layout, theme, and deck-quality work belongs in `pptx-presentation-design`, not this shared dispatcher.

## When to use
- Creating new DOCX, XLSX, or PPTX files with Python libraries when the result must be a real Office artifact
- Targeted text replace inside a `.docx`, `.pptx`, or `.xlsx`, including matches split across DOCX/PPTX runs
- Extracting plain text from a `.pdf` / `.docx` / `.pptx` / `.xlsx` for review or summarization
- Inserting a new slide into an existing `.pptx` at a specific position
- Redacting sensitive substrings across a document
- Secure DOCX textual redaction with package-wide residual scanning and fail-closed media/embedding handling
- Basic DOCX comments and tracked-change lifecycle operations
- Validating Office ZIP structure and backend readability after generation/editing
- Converting Office files to PDF or legacy formats via LibreOffice when available
- Rendering DOCX/PPTX/XLSX/PDF pages to images for visual QA when LibreOffice and Poppler are available
- Unpacking/repacking DOCX/PPTX/XLSX OOXML for template-aware edits, comments, relationship fixes, image replacement, or structure repair
- Recalculating XLSX formulas and scanning for residual Excel formula errors
- Creating a versioned snapshot before a risky edit
- Running multi-step Office work through a staged Job/Result protocol that validates before atomic publish and writes `artifact-manifest.json`
- Creating a new Office document when the user cares about layout, tables, formulas, speaker notes, charts, template compatibility, or repeatable Python control

## When NOT to use
- Plain text / source files → use `edit_file`

## Critical rule
**NEVER paste file contents, binary bytes, or base64 blobs into tool arguments.** Pass only the absolute `--path` plus operation parameters. The script reads and writes bytes on disk itself.

## Tool discipline

- Prefer `office_artifact` for DOCX/PPTX/XLSX create, modify, verify, publish, and restore work. It validates path roles, negotiates guarantees, keeps output as a candidate by default, and publishes only after `decide`.
- Use `create_file`, `edit_file`, or `multi_edit` for durable text inputs: Markdown bodies, JSON specs, CSV data, and reusable Python scripts.
- Use `run_shell` only to execute the bundled renderer/editor scripts or a short command against files that already exist on disk.
- Do not write a large one-off Python program inside a single `run_shell` argument. If custom code is genuinely needed, create a small script file in the workspace, run it, validate the output, then remove only temporary scratch files the user did not ask to keep.
- For new Office binaries, keep a reviewable source artifact next to the output whenever possible: `.md` for DOCX body content, `.json` for PPTX/XLSX specs, plus validation/audit output for layout-sensitive work.
- Large generated specs should never be passed through argv or `python -c`. For generated or transient PPTX/HTML-deck specs, prefer the renderer stdin contract (`--spec -` plus `run_shell.stdin`) so raw HTML/CSS/JSON is not embedded in argv. Create a separate JSON spec file only when the spec is meant to remain as a durable, reviewable source artifact.

## Preferred OfficeArtifactEngine v2 pattern

1. Call `office_artifact` with `action: "capabilities"` or `action: "assess"` when native calculation, final Office validation, or rendered evidence may be required.
2. Call `action: "execute"` with `requestVersion: 2`, `format`, `intent`, source/destination, typed `operations`, `guarantees`, and optional `validation`. Leave `delivery.mode` as `candidate` for review-sensitive work.
3. Inspect the returned validation, preservation, calculation, and rendered-preview evidence. Never describe a missing evidence class as passed.
4. Call `action: "decide"` with `publish` or `discard`. Publication returns a receipt.
5. Call `action: "restore"` with that receipt only when rollback is requested. Restore fails closed if the destination changed after publication.

Example request body:

```json
{
  "requestVersion": 2,
  "format": "docx",
  "intent": "modify",
  "source": "/abs/source/report.docx",
  "destination": "/abs/source/report-revised.docx",
  "operations": [{
    "op": "replace",
    "find": "Q3",
    "replace": "Q4",
    "expectedSha256": "<hash-from-inspection>",
    "expectedMatches": 1
  }],
  "guarantees": {
    "quality": "standard",
    "preservation": "strict",
    "render": "none"
  },
  "validation": {"required_text": ["Q4"]},
  "delivery": {"mode": "candidate"}
}
```

`quality: "publish"` requires rendered evidence; `quality: "native"` additionally requires Microsoft Office COM. `calculation: "compatible"` uses LibreOffice and must be labeled compatible, not Excel-native. `calculation: "native"` requires Excel COM. If `assess.ready` is false, lower the guarantee only with user agreement or install/enable the required backend.

Every inline validation object in requestVersion 2 must include `contractVersion: 2`. Request, guarantee, delivery, operation, and format-specific contract fields are closed schemas: a typo is a failure, never an ignored option. Use `preconditions.sourceSha256` for any workflow that starts from an inspected source. Canonical schemas are in `references/office-artifact-request-v2.schema.json`, `references/office-validation-contract-v2.schema.json`, and `references/office-adapter-manifest-v1.schema.json`.

Rendered evidence is stored under the owned candidate directory and includes the candidate SHA plus per-image hashes and deterministic visual QA. PPTX requires one final image per display-order slide. XLSX `all` renders each visible worksheet through an isolated temporary surface copy and records worksheet-to-image coverage; it remains LibreOffice-compatible evidence, not Excel-native evidence. A connected Office.js host is a separate deployment surface described by `references/office-host-adapter-v1.schema.json`; absence is reported as `not-connected` rather than silently falling back.

## Compatibility script pattern
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
   python <SKILL_DIR>/scripts/edit_doc.py --path /abs/source/html_deck.pptx create_html_pptx --spec - --outdir /abs/source/html_deck_project --mode hybrid --screenshot auto
   ```
   For `create_html_pptx` with `--spec -`, pass the HTML-deck JSON spec through the `run_shell.stdin` field. The `run_shell.args` array should contain only argv tokens, never the raw HTML/CSS/JSON payload. The PPTX commands are wrappers around the PPT skill renderers. `create_xlsx` delegates to the XLSX skill renderer, so complex workbooks should be driven by a reviewable JSON spec rather than one-off Python. Use `create_html_pptx` when the deck needs web-grade layout/CSS exploration plus a PPTX export; it writes `source/*.html`, `manifest.json`, and `qa.json` alongside the final deck.

   `run_shell` shape for generated HTML-first decks:
   ```json
   {
     "program": "python",
     "args": [
       "<SKILL_DIR>/scripts/edit_doc.py",
       "--path",
       "/abs/source/html_deck.pptx",
       "create_html_pptx",
       "--spec",
       "-",
       "--outdir",
       "/abs/source/html_deck_project",
       "--mode",
       "hybrid",
       "--screenshot",
       "auto"
     ],
     "cwd": "/abs/source",
     "timeout_secs": 120,
     "stdin": "{ \"slides\": [/* HTML deck JSON spec */] }"
   }
   ```
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
9. Lint and verify Excel formulas without LibreOffice:
   ```
   python <SKILL_DIR>/scripts/edit_doc.py --path /abs/source/model.xlsx lint_xlsx
   python <SKILL_DIR>/scripts/edit_doc.py --path /abs/source/model.xlsx validate
   ```

10. Run a legacy transactional Office job only for jobVersion 1 callers:
   ```
   python <SKILL_DIR>/scripts/office_artifact_service.py --preflight
   python <SKILL_DIR>/scripts/office_artifact_service.py --job /abs/source/office-job.json
   ```
   Use `jobVersion: 1`, `format`, `intent`, `input`/`output`, `operations`, `preservationPolicy`, `validationContract`, and `renderPolicy`. New work should use `office_artifact` requestVersion 2. The default `nexa-openxml` backend is local. `officecli` is an optional installed binary for explicit `create_new` jobs only and also requires `allowNetworkBackend: true`; never select it implicitly because hosted mode may transmit prompts or files. Microsoft Office COM is an explicit Windows-only finalizer.

Always call `check` first in a fresh environment:
```
python <SKILL_DIR>/scripts/edit_doc.py check
```

## Decision tree

1. Existing DOCX/PPTX/XLSX file? Prefer `office_artifact` for multi-step or risky changes. Use direct commands only for compatibility or a tightly scoped operation. Existing PDF file? Use the direct PDF commands.
2. New DOCX/XLSX/PPTX and Python is available? Use `create_docx`, `create_xlsx`, `create_pptx`, or `create_html_pptx` first. Prefer a JSON spec for spreadsheets/decks and a markdown/body input for documents.
3. Need template fidelity, comments, tracked changes, precise image replacement, relationship repair, or layout surgery? Use `unpack` → XML/media edit → `pack` → `validate`; do not use rigid one-shot generators.
4. Need PDF/image preview or conversion QA? Use `render` when system Poppler is already available, or `convert --to pdf` with system LibreOffice already available, then inspect/extract.
5. XLSX contains formulas? Use `lint_xlsx` after writing formulas, optionally with `--contract`. Use `recalc_xlsx` only when real cached values are required and LibreOffice is available; it blocks preservation-sensitive round-trips unless explicitly reviewed.
6. Python unavailable? Prepare the Python runtime first. If LibreOffice/Poppler are unavailable, explain that conversion/render QA needs those system tools rather than asking the app to install them.

## Adopted Office-skill patterns

- Keep the useful parts: Python Office libraries, OOXML unpack/pack escape hatch, isolated LibreOffice profiles for conversion/render only, visual render QA, internal XLSX formula linting, and explicit validation.
- Do not use external hard-coded skill paths, external author names, assumptions that every binary is preinstalled, or Node-first DOCX/PPTX generation as the default.
- Do not paste binary/base64 Office content into tool calls. All Office bytes stay on disk and are passed by absolute path.

## Better-than-openclaw principles
- **Diff preview** — `--dry-run` on `replace` / `redact` prints a unified diff instead of mutating the file
- **Sidecar versioning** — `version` subcommand writes `.nexa/doc-history/<name>/v{N}/<file>` snapshots
- **Undo stack** — every snapshot is addressable by version number, nothing is clobbered in place
- **Chunked streaming** — `extract` truncates > 50 KB output with a clear notice so large docs don't blow the context
- **Capability check** — `check` subcommand reports available/missing backends with exit code 2 if core deps are absent
- **Validate after write** — `validate` checks ZIP/CRC, XML parseability, required parts, Content Types, duplicate/missing relationships, formula errors, and backend readability
- **Visual QA** — `render` converts Office/PDF pages to PNG/JPEG images with isolated LibreOffice profiles
- **HTML-first PPTX** — `create_html_pptx` keeps the deck source as reviewable HTML/CSS, optionally captures Playwright screenshots, exports hybrid native/raster PPTX, and writes manifest/QA JSON
- **Conversion QA** — `convert` uses LibreOffice headless with an isolated user profile for PDF previews and format conversion
- **OOXML escape hatch** — `unpack` / `pack` make low-level template and relationship fixes possible without passing binary data through tool arguments
- **Formula safety** — `lint_xlsx` supports workbook contracts and risk inventory; `recalc_xlsx` performs a real guarded LibreOffice open/save, verifies the rewritten artifact, scans cached errors, and publishes only after validation
- **Transactional runtime** — `office_artifact` and `office_artifact_engine.py` expose assess/execute/decide/restore, typed errors, capability routing, candidate gating, receipts, hash-guarded restore, and evidence-rich manifests; `office_artifact_service.py` preserves jobVersion 1 compatibility
- **Typed format edits** — direct-OOXML XLSX value/formula/range/style edits, PPTX stable slide/shape edits and dependency-cloning, and DOCX review operations run behind the same candidate contract
- **Secure redaction boundary** — `redact` is visible-story replacement; `secure_redact` alone may claim package-text absence, and it blocks uninspectable media/embeddings
- **Golden/fault corpus** — `tests/golden` and `test_office_artifact_golden.py` exercise all three formats through the public interface, bind contract/render evidence to artifact SHA, and prove failures do not publish
- **Native boundary** — Excel/Word/PowerPoint COM force-disable macros before opening; Excel disables link updates and waits for calculation; PowerPoint native quality exports deterministic per-slide images; the Rust host enforces a 15-minute kill-on-drop watchdog

## Dependencies
In the desktop app, first prefer `prepare_document_tools` when that tool is available. Call `action: "check"` to inspect readiness, then call `action: "prepare"` for missing required Python dependencies. The same flow is exposed in Settings → Models → Document tools. It creates an app-managed virtual environment, installs the exact pinned versions in bundled `requirements.txt`, and makes `run_shell` prefer that managed Python path automatically. A version mismatch is not reported as ready. It does not install or manage Poppler or LibreOffice.

For CLI/dev environments, install before first Office/PDF operation (only what's needed for the target format):
```
python -m pip install -r <SKILL_DIR>/scripts/requirements.txt
```
Optional for format conversion / PDF rendering: system `libreoffice` and Poppler. Optional for HTML-first PPTX screenshot QA: Python `playwright` plus browser installation. Install optional tools only when the task specifically needs conversion, render QA, or screenshot QA.

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
| create_html_pptx | .pptx          | python-pptx; optional Playwright for screenshots |
| unpack         | .docx/.pptx/.xlsx | (none)           |
| pack           | .docx/.pptx/.xlsx | (none)           |
| replace        | .docx            | python-docx      |
| replace        | .pptx            | python-pptx      |
| replace        | .xlsx            | precise OOXML ZIP/XML edit (no broad openpyxl round-trip) |
| redact         | .docx / .pptx    | python-docx / python-pptx |
| extract        | .docx            | python-docx      |
| extract        | .pptx            | python-pptx      |
| extract        | .xlsx            | openpyxl         |
| extract        | .pdf             | pypdf            |
| insert_slide   | .pptx            | python-pptx      |
| render         | Office/PDF       | LibreOffice + Poppler |
| lint_xlsx      | .xlsx            | openpyxl         |
| recalc_xlsx    | .xlsx            | LibreOffice + openpyxl formula/error verification |
| validate       | .docx/.pptx/.xlsx/.pdf | stdlib OOXML validator + matching backend |
| office_artifact_service | .docx/.pptx/.xlsx | matching selected backend |
| convert        | Office/PDF       | LibreOffice      |
| version        | any              | (none)           |

When a backend is missing at runtime, subcommands exit `2` with `MISSING_DEP: <pkg>` on stderr plus the exact `python -m pip install <pkg>` hint.

## Exit codes
- `0` success
- `1` generic error
- `2` missing dependency (prints `MISSING_DEP: <pkg>`)
- `3` bad input / path validation failed
