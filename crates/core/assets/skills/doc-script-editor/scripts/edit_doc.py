#!/usr/bin/env python3
"""edit_doc.py — Skill-bundled document editor for DOCX/PPTX/PDF/XLSX.

Invoked via the app's `run_shell` tool. Reads/writes files on disk;
never accepts document content over argv. Lazy-imports backend libs
so `check` works with nothing installed.

Exit codes:
  0 success
  1 generic error
  2 missing dependency
  3 bad input / path validation failed
"""
from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import zipfile
from bisect import bisect_right
from pathlib import Path
from typing import Any
from xml.etree import ElementTree as ET

from office_artifact_runtime import (
    office_backend_statuses,
    publish_staged_artifact,
    scan_ooxml_risks,
    staging_path,
    validate_ooxml_package,
    write_artifact_manifest,
)

MAX_EXTRACT_BYTES = 50 * 1024
HISTORY_DIR = ".nexa/doc-history"
EXCEL_ERRORS = ("#VALUE!", "#DIV/0!", "#REF!", "#NAME?", "#NULL!", "#NUM!", "#N/A")
UNPACK_MARKER = ".nexa-ooxml-unpack.json"


# ---------------------------------------------------------------------------
# Utilities
# ---------------------------------------------------------------------------

def _suppress_windows_error_dialogs() -> None:
    if os.name != "nt":
        return
    try:
        import ctypes
        kernel32 = ctypes.windll.kernel32
        sem_failcriticalerrors = 0x0001
        sem_nogpfaulterrorbox = 0x0002
        sem_noopenfileerrorbox = 0x8000
        kernel32.SetErrorMode(
            sem_failcriticalerrors | sem_nogpfaulterrorbox | sem_noopenfileerrorbox
        )
    except Exception:
        pass


_suppress_windows_error_dialogs()


def _run_subprocess(cmd: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
    if os.name == "nt":
        kwargs["creationflags"] = kwargs.get("creationflags", 0) | getattr(
            subprocess,
            "CREATE_NO_WINDOW",
            0,
        )
    return subprocess.run(cmd, check=kwargs.pop("check", False), **kwargs)


def _die(msg: str, code: int = 1) -> None:
    print(msg, file=sys.stderr)
    sys.exit(code)


def _missing(pkg: str) -> None:
    print(f"MISSING_DEP: {pkg}", file=sys.stderr)
    print(f"Install with: python -m pip install {pkg}", file=sys.stderr)
    sys.exit(2)


def _validate_path(raw: str, must_exist: bool = True) -> Path:
    if not raw:
        _die("ERROR: --path is required", 3)
    p = Path(raw)
    if not p.is_absolute():
        _die(f"ERROR: --path must be absolute: {raw}", 3)
    try:
        resolved = p.resolve()
        cwd = Path.cwd().resolve()
        # Basic traversal guard: resolved path must live under cwd.
        resolved.relative_to(cwd)
    except ValueError:
        _die(f"ERROR: path escapes workspace: {raw}", 3)
    except OSError as e:
        _die(f"ERROR: cannot resolve path: {e}", 3)
    if must_exist and not resolved.exists():
        _die(f"ERROR: file not found: {resolved}", 3)
    return resolved


def _validate_output_path(raw: str, suffixes: set[str]) -> Path:
    p = _validate_path(raw, must_exist=False)
    if _ext(p) not in suffixes:
        _die(f"ERROR: output path must end with one of: {', '.join(sorted(suffixes))}", 3)
    p.parent.mkdir(parents=True, exist_ok=True)
    return p


def _validate_output_dir(raw: str, *, allow_existing: bool = True) -> Path:
    if not raw:
        _die("ERROR: output directory is required", 3)
    p = Path(raw)
    if not p.is_absolute():
        _die(f"ERROR: output directory must be absolute: {raw}", 3)
    try:
        resolved = p.resolve()
        workspace_root = Path.cwd().resolve()
        resolved.relative_to(workspace_root)
    except ValueError:
        _die(f"ERROR: output directory escapes workspace: {raw}", 3)
    except OSError as e:
        _die(f"ERROR: cannot resolve output directory: {e}", 3)
    if resolved == workspace_root:
        _die("ERROR: output directory cannot be the workspace root", 3)
    if resolved.exists() and not allow_existing:
        _die(f"ERROR: output directory already exists: {resolved}", 3)
    resolved.mkdir(parents=True, exist_ok=True)
    return resolved


def _ext(p: Path) -> str:
    return p.suffix.lower().lstrip(".")


def _staged_copy(path: Path) -> Path:
    staged = staging_path(path)
    shutil.copy2(path, staged)
    return staged


def _publish_edit(staged: Path, path: Path) -> tuple[Path | None, dict[str, Any] | None]:
    try:
        snapshot, report = publish_staged_artifact(staged, path, Path.cwd(), validate=True)
    except ValueError as error:
        staged.unlink(missing_ok=True)
        _die(f"VALIDATION_FAILED: staged artifact was not published\n{error}", 1)
    return snapshot, report.to_dict() if report is not None else None


def _parse_pages(spec: str | None, total: int) -> list[int]:
    if not spec:
        return list(range(total))
    out: list[int] = []
    for part in spec.split(","):
        part = part.strip()
        if not part:
            continue
        if "-" in part:
            a, b = part.split("-", 1)
            out.extend(range(int(a) - 1, int(b)))
        else:
            out.append(int(part) - 1)
    return [i for i in out if 0 <= i < total]


def _truncate(text: str) -> str:
    raw = text.encode("utf-8")
    if len(raw) <= MAX_EXTRACT_BYTES:
        return text
    cut = raw[:MAX_EXTRACT_BYTES].decode("utf-8", errors="ignore")
    return cut + f"\n\n[TRUNCATED: output exceeded {MAX_EXTRACT_BYTES} bytes]"


def _read_json(path: str) -> dict[str, Any]:
    spec_path = _validate_path(path)
    with spec_path.open("r", encoding="utf-8") as f:
        data = json.load(f)
    if not isinstance(data, dict):
        _die("ERROR: JSON spec root must be an object", 3)
    return data


def _read_text(path: str) -> str:
    text_path = _validate_path(path)
    return text_path.read_text(encoding="utf-8")


def _find_soffice() -> str | None:
    for name in ("soffice", "soffice.com", "libreoffice"):
        found = shutil.which(name)
        if found:
            return found
    for candidate in (
        r"C:\Program Files\LibreOffice\program\soffice.exe",
        r"C:\Program Files\LibreOffice\program\soffice.com",
        r"C:\Program Files (x86)\LibreOffice\program\soffice.exe",
    ):
        if Path(candidate).exists():
            return candidate
    return None


def _find_pdftoppm() -> str | None:
    for name in ("pdftoppm", "pdftoppm.exe"):
        found = shutil.which(name)
        if found:
            return found
    return None


def _soffice_env() -> dict[str, str]:
    env = os.environ.copy()
    env.setdefault("SAL_USE_VCLPLUGIN", "svp")
    return env


def _soffice_base_cmd(soffice: str, profile_dir: Path) -> list[str]:
    return [
        soffice,
        f"-env:UserInstallation={profile_dir.resolve().as_uri()}",
        "--headless",
        "--invisible",
        "--norestore",
        "--nolockcheck",
        "--nodefault",
    ]


def _run_soffice_convert(path: Path, to: str, outdir: Path) -> subprocess.CompletedProcess[str]:
    soffice = _find_soffice()
    if not soffice:
        _die("MISSING_DEP: LibreOffice/soffice\nInstall LibreOffice and ensure soffice is on PATH.", 2)
    with tempfile.TemporaryDirectory(prefix="nexa-lo-profile-") as profile:
        cmd = [
            *_soffice_base_cmd(soffice, Path(profile)),
            "--convert-to",
            to,
            "--outdir",
            str(outdir),
            str(path),
        ]
        return _run_subprocess(
            cmd,
            text=True,
            capture_output=True,
            check=False,
            env=_soffice_env(),
        )


def _expected_converted_path(input_path: Path, outdir: Path, to: str) -> Path:
    ext = to.split(":", 1)[0].split()[0].lstrip(".")
    return outdir / f"{input_path.stem}.{ext}"


# ---------------------------------------------------------------------------
# check
# ---------------------------------------------------------------------------

def cmd_check(args: argparse.Namespace) -> int:
    backends = [
        ("python-docx", "docx"),
        ("python-pptx", "pptx"),
        ("pypdf", "pypdf"),
        ("openpyxl", "openpyxl"),
    ]
    missing_core = []
    results: list[dict[str, Any]] = []
    if not args.json:
        print(f"python: {sys.version.split()[0]}")
    for display, mod in backends:
        try:
            imported = __import__(mod)
            ver = getattr(imported, "__version__", "unknown")
            results.append({
                "id": display,
                "module": mod,
                "status": "ok",
                "version": str(ver),
                "required": True,
            })
            if not args.json:
                print(f"  {display:<14} OK      ({ver})")
        except ImportError:
            results.append({
                "id": display,
                "module": mod,
                "status": "missing",
                "required": True,
            })
            if not args.json:
                print(f"  {display:<14} MISSING")
            missing_core.append(display)
        except Exception as e:  # noqa: BLE001
            # Backend present but broken (e.g. numpy ABI mismatch). Treat as missing.
            results.append({
                "id": display,
                "module": mod,
                "status": "broken",
                "required": True,
                "detail": f"{type(e).__name__}: {e}",
            })
            if not args.json:
                print(f"  {display:<14} BROKEN  ({type(e).__name__}: {e})")
            missing_core.append(display)
    soffice = _find_soffice()
    if soffice:
        results.append({
            "id": "LibreOffice",
            "status": "ok",
            "path": soffice,
            "required": False,
        })
        if not args.json:
            print(f"  LibreOffice    OK      ({soffice})")
    else:
        results.append({
            "id": "LibreOffice",
            "status": "missing",
            "required": False,
            "detail": "needed for convert/render QA",
        })
        if not args.json:
            print("  LibreOffice    MISSING (needed for convert/render QA)")
    pdftoppm = _find_pdftoppm()
    if pdftoppm:
        results.append({
            "id": "Poppler",
            "status": "ok",
            "path": pdftoppm,
            "required": False,
        })
        if not args.json:
            print(f"  Poppler        OK      ({pdftoppm})")
    else:
        results.append({
            "id": "Poppler",
            "status": "missing",
            "required": False,
            "detail": "needed for render QA",
        })
        if not args.json:
            print("  Poppler        MISSING (needed for render QA)")
    try:
        imported = __import__("playwright")
        ver = getattr(imported, "__version__", "unknown")
        results.append({
            "id": "Playwright",
            "module": "playwright",
            "status": "ok",
            "version": str(ver),
            "required": False,
            "detail": "needed for HTML-first PPTX screenshot QA",
        })
        if not args.json:
            print(f"  Playwright     OK      ({ver})")
    except ImportError:
        results.append({
            "id": "Playwright",
            "module": "playwright",
            "status": "missing",
            "required": False,
            "detail": "needed for HTML-first PPTX screenshot QA",
        })
        if not args.json:
            print("  Playwright     MISSING (needed for HTML-first PPTX screenshot QA)")
    except Exception as e:  # noqa: BLE001
        results.append({
            "id": "Playwright",
            "module": "playwright",
            "status": "broken",
            "required": False,
            "detail": f"{type(e).__name__}: {e}",
        })
        if not args.json:
            print(f"  Playwright     BROKEN  ({type(e).__name__}: {e})")
    office_backends = office_backend_statuses()
    if not args.json:
        print("\nOffice artifact backends:")
        for backend in office_backends:
            detail = f" ({backend['detail']})" if backend.get("detail") else ""
            print(f"  {backend['label']:<22} {backend['status'].upper()}{detail}")
    if args.json:
        payload = {
            "python": {
                "version": sys.version.split()[0],
                "executable": sys.executable,
            },
            "status": "ok" if not missing_core else "missing",
            "dependencies": results,
            "officeBackends": office_backends,
        }
        print(json.dumps(payload, ensure_ascii=False, indent=2))
    if missing_core:
        if not args.json:
            print(
                "\nInstall missing deps with:\n"
                f"  python -m pip install {' '.join(missing_core)}"
            )
        return 2
    return 0


# ---------------------------------------------------------------------------
# extract
# ---------------------------------------------------------------------------

def _extract_docx(path: Path) -> str:
    try:
        import docx  # type: ignore
    except ImportError:
        _missing("python-docx")
    doc = docx.Document(str(path))
    return "\n".join(p.text for p in doc.paragraphs)


def _extract_pptx(path: Path, pages: str | None) -> str:
    try:
        from pptx import Presentation  # type: ignore
    except ImportError:
        _missing("python-pptx")
    prs = Presentation(str(path))
    indices = _parse_pages(pages, len(prs.slides))
    out = []
    for i in indices:
        slide = prs.slides[i]
        out.append(f"--- Slide {i + 1} ---")
        for shape in slide.shapes:
            if shape.has_text_frame:
                for para in shape.text_frame.paragraphs:
                    out.append(para.text)
    return "\n".join(out)


def _extract_pdf(path: Path, pages: str | None) -> str:
    try:
        from pypdf import PdfReader  # type: ignore
    except ImportError:
        _missing("pypdf")
    reader = PdfReader(str(path))
    indices = _parse_pages(pages, len(reader.pages))
    out = []
    for i in indices:
        out.append(f"--- Page {i + 1} ---")
        out.append(reader.pages[i].extract_text() or "")
    return "\n".join(out)


def _extract_xlsx(path: Path, sheets: str | None) -> str:
    try:
        import openpyxl  # type: ignore
    except ImportError:
        _missing("openpyxl")
    wb = openpyxl.load_workbook(str(path), data_only=False, read_only=True)
    wanted = {s.strip() for s in sheets.split(",")} if sheets else None
    out: list[str] = []
    for ws in wb.worksheets:
        if wanted and ws.title not in wanted:
            continue
        out.append(f"--- Sheet: {ws.title} ---")
        for row in ws.iter_rows(values_only=True):
            values = ["" if cell is None else str(cell) for cell in row]
            if any(v.strip() for v in values):
                out.append("\t".join(values).rstrip())
    return "\n".join(out)


def cmd_extract(args: argparse.Namespace) -> int:
    path = _validate_path(args.path)
    ext = _ext(path)
    if ext == "docx":
        text = _extract_docx(path)
    elif ext == "pptx":
        text = _extract_pptx(path, args.pages)
    elif ext == "pdf":
        text = _extract_pdf(path, args.pages)
    elif ext == "xlsx":
        text = _extract_xlsx(path, args.sheets)
    else:
        _die(f"ERROR: extract does not support .{ext}", 3)
    sys.stdout.write(_truncate(text))
    if not text.endswith("\n"):
        sys.stdout.write("\n")
    return 0


# ---------------------------------------------------------------------------
# replace / redact (shared core)
# ---------------------------------------------------------------------------

def _replace_across_text_nodes(nodes: list[Any], find: str, replace: str, apply: bool) -> int:
    fragments = [node.text or "" for node in nodes]
    combined = "".join(fragments)
    if not combined or find not in combined:
        return 0

    starts: list[int] = []
    cursor = 0
    for fragment in fragments:
        starts.append(cursor)
        cursor += len(fragment)

    matches: list[tuple[int, int]] = []
    cursor = 0
    while True:
        start = combined.find(find, cursor)
        if start < 0:
            break
        matches.append((start, start + len(find)))
        cursor = start + len(find)

    if not apply:
        return len(matches)

    for start, end in reversed(matches):
        start_index = max(0, bisect_right(starts, start) - 1)
        end_index = max(0, bisect_right(starts, end - 1) - 1)
        start_offset = start - starts[start_index]
        end_offset = end - starts[end_index]
        start_text = nodes[start_index].text or ""
        end_text = nodes[end_index].text or ""
        if start_index == end_index:
            nodes[start_index].text = start_text[:start_offset] + replace + start_text[end_offset:]
            continue
        nodes[start_index].text = start_text[:start_offset] + replace
        for index in range(start_index + 1, end_index):
            nodes[index].text = ""
        nodes[end_index].text = end_text[end_offset:]
    return len(matches)


def _docx_text_groups(doc) -> list[list[Any]]:
    from docx.oxml.ns import qn  # type: ignore

    groups: list[list[Any]] = []
    seen_roots: set[int] = set()
    for part in doc.part.package.parts:
        root = getattr(part, "element", None)
        if root is None:
            root = getattr(part, "_element", None)
        if root is None or id(root) in seen_roots:
            continue
        seen_roots.add(id(root))
        for paragraph in root.iter(qn("w:p")):
            nodes = list(paragraph.iter(qn("w:t")))
            if nodes:
                groups.append(nodes)
    return groups


def _assert_expected_match_count(count: int, expected_count: int | None) -> None:
    if expected_count is not None and count != expected_count:
        _die(
            f"PRECONDITION_FAILED: expected {expected_count} text match(es), found {count}",
            1,
        )


def _replace_docx(
    path: Path,
    find: str,
    replace: str,
    dry_run: bool,
    expected_count: int | None = None,
) -> int:
    try:
        import docx  # type: ignore
    except ImportError:
        _missing("python-docx")
    working = path if dry_run else _staged_copy(path)
    doc = docx.Document(str(working))
    groups = _docx_text_groups(doc)
    before_lines = ["".join(node.text or "" for node in nodes) for nodes in groups]
    count = sum(_replace_across_text_nodes(nodes, find, replace, apply=False) for nodes in groups)
    if expected_count is not None and count != expected_count and not dry_run:
        working.unlink(missing_ok=True)
    _assert_expected_match_count(count, expected_count)
    if not dry_run and count:
        for nodes in groups:
            _replace_across_text_nodes(nodes, find, replace, apply=True)
    if dry_run:
        after_lines = [line.replace(find, replace) for line in before_lines]
        diff = difflib.unified_diff(
            before_lines, after_lines,
            fromfile=str(path), tofile=f"{path} (preview)", lineterm="",
        )
        sys.stdout.write("\n".join(diff) + "\n")
        print(f"\n[DRY-RUN] matches: {count}")
        return 0
    if count == 0:
        working.unlink(missing_ok=True)
        print(f"replaced 0 occurrence(s) in {path}")
        return 0
    doc.save(str(working))
    snapshot, validation = _publish_edit(working, path)
    print(json.dumps({
        "replaced": count,
        "path": str(path),
        "snapshot": str(snapshot) if snapshot else None,
        "validation": validation,
    }, ensure_ascii=False, indent=2))
    return 0


def _pptx_text_groups(prs) -> list[list[Any]]:
    drawing_ns = "http://schemas.openxmlformats.org/drawingml/2006/main"
    paragraph_tag = f"{{{drawing_ns}}}p"
    text_tag = f"{{{drawing_ns}}}t"
    groups: list[list[Any]] = []
    seen_roots: set[int] = set()
    for part in prs.part.package.iter_parts():
        root = getattr(part, "element", None)
        if root is None:
            root = getattr(part, "_element", None)
        if root is None or id(root) in seen_roots:
            continue
        seen_roots.add(id(root))
        for paragraph in root.iter(paragraph_tag):
            nodes = list(paragraph.iter(text_tag))
            if nodes:
                groups.append(nodes)
    return groups


def _replace_pptx(
    path: Path,
    find: str,
    replace: str,
    dry_run: bool,
    expected_count: int | None = None,
) -> int:
    try:
        from pptx import Presentation  # type: ignore
    except ImportError:
        _missing("python-pptx")
    working = path if dry_run else _staged_copy(path)
    prs = Presentation(str(working))
    groups = _pptx_text_groups(prs)
    before_lines = ["".join(node.text or "" for node in nodes) for nodes in groups]
    count = sum(_replace_across_text_nodes(nodes, find, replace, apply=False) for nodes in groups)
    if expected_count is not None and count != expected_count and not dry_run:
        working.unlink(missing_ok=True)
    _assert_expected_match_count(count, expected_count)
    if not dry_run and count:
        for nodes in groups:
            _replace_across_text_nodes(nodes, find, replace, apply=True)
    if dry_run:
        before = "\n".join(before_lines)
        after = before.replace(find, replace)
        diff = difflib.unified_diff(
            before.splitlines(), after.splitlines(),
            fromfile=str(path), tofile=f"{path} (preview)", lineterm="",
        )
        sys.stdout.write("\n".join(diff) + "\n")
        print(f"\n[DRY-RUN] matches: {count}")
        return 0
    if count == 0:
        working.unlink(missing_ok=True)
        print(f"replaced 0 occurrence(s) in {path}")
        return 0
    prs.save(str(working))
    snapshot, validation = _publish_edit(working, path)
    print(json.dumps({
        "replaced": count,
        "path": str(path),
        "snapshot": str(snapshot) if snapshot else None,
        "validation": validation,
    }, ensure_ascii=False, indent=2))
    return 0


def _replace_xlsx(
    path: Path,
    find: str,
    replace: str,
    dry_run: bool,
    expected_count: int | None = None,
) -> int:
    spreadsheet_ns = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
    text_tag = f"{{{spreadsheet_ns}}}t"
    container_tags = {
        f"{{{spreadsheet_ns}}}si",
        f"{{{spreadsheet_ns}}}is",
        f"{{{spreadsheet_ns}}}comment",
    }
    scalar_tags = {
        f"{{{spreadsheet_ns}}}f",
        f"{{{spreadsheet_ns}}}definedName",
    }
    editable_parts = (
        "xl/sharedStrings.xml",
        "xl/workbook.xml",
        "xl/worksheets/",
        "xl/comments",
    )
    before_lines: list[str] = []
    after_lines: list[str] = []
    count = 0
    staged = _staged_copy(path) if not dry_run else None
    source = staged or path
    repacked = staging_path(source) if not dry_run else None
    try:
        with zipfile.ZipFile(source) as archive:
            destination = zipfile.ZipFile(repacked, "w") if repacked is not None else None
            try:
                for info in archive.infolist():
                    data = archive.read(info.filename)
                    if info.filename.startswith(editable_parts) and info.filename.endswith(".xml"):
                        root = ET.fromstring(data)
                        groups: list[list[Any]] = []
                        part_matches = 0
                        for element in root.iter():
                            if element.tag in container_tags:
                                nodes = list(element.iter(text_tag))
                                if nodes:
                                    groups.append(nodes)
                            elif element.tag in scalar_tags:
                                groups.append([element])
                        for nodes in groups:
                            before = "".join(node.text or "" for node in nodes)
                            matches = _replace_across_text_nodes(
                                nodes, find, replace, apply=not dry_run
                            )
                            if matches:
                                count += matches
                                part_matches += matches
                                before_lines.append(f"{info.filename}: {before}")
                                after_lines.append(
                                    f"{info.filename}: {before.replace(find, replace)}"
                                )
                        if not dry_run and part_matches:
                            data = ET.tostring(root, encoding="utf-8", xml_declaration=True)
                    if destination is not None:
                        destination.writestr(info, data)
            finally:
                if destination is not None:
                    destination.close()
        if not dry_run and repacked is not None and staged is not None:
            os.replace(repacked, staged)
    except Exception:
        if repacked is not None:
            repacked.unlink(missing_ok=True)
        if staged is not None:
            staged.unlink(missing_ok=True)
        raise
    if dry_run:
        _assert_expected_match_count(count, expected_count)
        diff = difflib.unified_diff(
            before_lines, after_lines,
            fromfile=str(path), tofile=f"{path} (preview)", lineterm="",
        )
        sys.stdout.write("\n".join(diff) + "\n")
        print(f"\n[DRY-RUN] matches: {count}")
        return 0
    if expected_count is not None and count != expected_count:
        staged.unlink(missing_ok=True)
        _assert_expected_match_count(count, expected_count)
    if count == 0:
        staged.unlink(missing_ok=True)
        print(f"replaced 0 occurrence(s) in {path}")
        return 0
    snapshot, validation = _publish_edit(staged, path)
    print(json.dumps({
        "replaced": count,
        "path": str(path),
        "snapshot": str(snapshot) if snapshot else None,
        "preservationRisk": scan_ooxml_risks(path),
        "validation": validation,
    }, ensure_ascii=False, indent=2))
    return 0


def cmd_replace(args: argparse.Namespace) -> int:
    path = _validate_path(args.path)
    if not args.find:
        _die("ERROR: --find is required", 3)
    expected_sha256 = getattr(args, "expected_sha256", None)
    if expected_sha256:
        actual_sha256 = hashlib.sha256(path.read_bytes()).hexdigest()
        if actual_sha256.lower() != str(expected_sha256).lower():
            _die(
                "PRECONDITION_FAILED: artifact SHA-256 changed since it was inspected "
                f"(expected {expected_sha256}, actual {actual_sha256})",
                1,
            )
    expected_count = getattr(args, "expected_count", None)
    if expected_count is not None and expected_count < 0:
        _die("ERROR: --expected-count cannot be negative", 3)
    ext = _ext(path)
    if ext == "docx":
        return _replace_docx(path, args.find, args.replace or "", args.dry_run, expected_count)
    if ext == "pptx":
        return _replace_pptx(path, args.find, args.replace or "", args.dry_run, expected_count)
    if ext == "xlsx":
        return _replace_xlsx(path, args.find, args.replace or "", args.dry_run, expected_count)
    _die(f"ERROR: replace supports .docx/.pptx/.xlsx only (got .{ext})", 3)
    return 1


def cmd_redact(args: argparse.Namespace) -> int:
    # redact is replace with a default mask token
    args.replace = args.replace if args.replace is not None else "[REDACTED]"
    return cmd_replace(args)


# ---------------------------------------------------------------------------
# insert_slide
# ---------------------------------------------------------------------------

def cmd_insert_slide(args: argparse.Namespace) -> int:
    path = _validate_path(args.path)
    if _ext(path) != "pptx":
        _die("ERROR: insert_slide requires a .pptx file", 3)
    try:
        from pptx import Presentation  # type: ignore
        from pptx.util import Inches  # type: ignore
    except ImportError:
        _missing("python-pptx")
    staged = _staged_copy(path)
    prs = Presentation(str(staged))
    layout = prs.slide_layouts[1] if len(prs.slide_layouts) > 1 else prs.slide_layouts[0]
    slide = prs.slides.add_slide(layout)
    # Populate title/body if placeholders exist.
    if slide.shapes.title is not None:
        slide.shapes.title.text = args.title or ""
    if args.body:
        body_placeholder = None
        for ph in slide.placeholders:
            if ph.placeholder_format.idx == 1:
                body_placeholder = ph
                break
        if body_placeholder is None:
            left = top = Inches(1)
            width = Inches(8)
            height = Inches(5)
            body_placeholder = slide.shapes.add_textbox(left, top, width, height)
        body_placeholder.text_frame.text = args.body

    # Reorder: move new slide to position after --after.
    after = max(0, int(args.after))
    xml_slides = prs.slides._sldIdLst
    slides = list(xml_slides)
    new_el = slides[-1]
    xml_slides.remove(new_el)
    xml_slides.insert(after, new_el)

    prs.save(str(staged))
    snapshot, validation = _publish_edit(staged, path)
    print(json.dumps({
        "insertedAfter": after,
        "path": str(path),
        "snapshot": str(snapshot) if snapshot else None,
        "validation": validation,
    }, ensure_ascii=False, indent=2))
    return 0


# ---------------------------------------------------------------------------
# version
# ---------------------------------------------------------------------------

def cmd_version(args: argparse.Namespace) -> int:
    path = _validate_path(args.path)
    root = Path.cwd() / HISTORY_DIR / path.name
    root.mkdir(parents=True, exist_ok=True)
    existing = sorted(
        (p for p in root.iterdir() if p.is_dir() and p.name.startswith("v")),
        key=lambda p: int(p.name[1:]) if p.name[1:].isdigit() else 0,
    )
    next_n = 1
    if existing:
        last = existing[-1].name
        try:
            next_n = int(last[1:]) + 1
        except ValueError:
            next_n = len(existing) + 1
    dest_dir = root / f"v{next_n}"
    dest_dir.mkdir()
    dest = dest_dir / path.name
    shutil.copy2(path, dest)
    print(f"v{next_n} -> {dest}")
    return 0


# ---------------------------------------------------------------------------
# create_docx / create_xlsx / create_pptx
# ---------------------------------------------------------------------------

def _numbered_markdown_text(stripped: str) -> str | None:
    prefix, sep, rest = stripped.partition(".")
    if sep and prefix.isdigit() and rest.startswith(" "):
        return rest.strip()
    return None


def _docx_add_hyperlink(paragraph, label: str, url: str) -> None:
    from docx.opc.constants import RELATIONSHIP_TYPE as RT  # type: ignore
    from docx.oxml import OxmlElement  # type: ignore
    from docx.oxml.ns import qn  # type: ignore

    relationship_id = paragraph.part.relate_to(url, RT.HYPERLINK, is_external=True)
    hyperlink = OxmlElement("w:hyperlink")
    hyperlink.set(qn("r:id"), relationship_id)
    run = OxmlElement("w:r")
    properties = OxmlElement("w:rPr")
    color = OxmlElement("w:color")
    color.set(qn("w:val"), "0563C1")
    underline = OxmlElement("w:u")
    underline.set(qn("w:val"), "single")
    properties.extend([color, underline])
    text = OxmlElement("w:t")
    text.text = label
    run.extend([properties, text])
    hyperlink.append(run)
    paragraph._p.append(hyperlink)


def _docx_add_inline_markdown(paragraph, text: str) -> None:
    """Add common inline Markdown as Word runs instead of literal markers."""
    i = 0
    plain_start = 0

    def flush_plain(end: int) -> None:
        nonlocal plain_start
        if end > plain_start:
            paragraph.add_run(text[plain_start:end])
        plain_start = end

    def add_styled(content: str, *, bold=False, italic=False, strike=False, code=False) -> None:
        if not content:
            return
        run = paragraph.add_run(content)
        run.bold = bold or None
        run.italic = italic or None
        run.font.strike = strike or None
        if code:
            run.font.name = "Consolas"

    while i < len(text):
        if text[i] == "\\" and i + 1 < len(text):
            flush_plain(i)
            paragraph.add_run(text[i + 1])
            i += 2
            plain_start = i
            continue

        if text.startswith(("**", "__"), i):
            marker = text[i:i + 2]
            end = text.find(marker, i + 2)
            if end != -1:
                flush_plain(i)
                add_styled(text[i + 2:end], bold=True)
                i = end + 2
                plain_start = i
                continue

        if text.startswith("~~", i):
            end = text.find("~~", i + 2)
            if end != -1:
                flush_plain(i)
                add_styled(text[i + 2:end], strike=True)
                i = end + 2
                plain_start = i
                continue

        if text[i] == "`":
            end = text.find("`", i + 1)
            if end != -1:
                flush_plain(i)
                add_styled(text[i + 1:end], code=True)
                i = end + 1
                plain_start = i
                continue

        if text[i] in {"*", "_"}:
            marker = text[i]
            end = text.find(marker, i + 1)
            if end != -1:
                flush_plain(i)
                add_styled(text[i + 1:end], italic=True)
                i = end + 1
                plain_start = i
                continue

        if text[i] == "[":
            label_end = text.find("](", i + 1)
            if label_end != -1:
                url_end = text.find(")", label_end + 2)
                if url_end != -1:
                    flush_plain(i)
                    label = text[i + 1:label_end]
                    url = text[label_end + 2:url_end]
                    if url and url.lower().startswith(("https://", "http://", "mailto:")):
                        _docx_add_hyperlink(paragraph, label or url, url)
                    elif label:
                        paragraph.add_run(label)
                        if url:
                            paragraph.add_run(f" ({url})")
                    elif url:
                        paragraph.add_run(url)
                    i = url_end + 1
                    plain_start = i
                    continue

        i += 1

    flush_plain(len(text))


def _docx_add_markdown(doc, markdown: str) -> None:
    lines = markdown.splitlines()
    i = 0
    while i < len(lines):
        line = lines[i]
        stripped = line.strip()
        if not stripped:
            doc.add_paragraph()
            i += 1
            continue
        if stripped.startswith("#"):
            level = len(stripped) - len(stripped.lstrip("#"))
            if 1 <= level <= 6 and len(stripped) > level and stripped[level].isspace():
                heading = doc.add_heading(level=min(level, 4))
                _docx_add_inline_markdown(heading, stripped[level:].strip())
                i += 1
                continue
        if stripped.startswith("|") and stripped.endswith("|"):
            table_lines: list[str] = []
            while i < len(lines) and lines[i].strip().startswith("|") and lines[i].strip().endswith("|"):
                table_lines.append(lines[i].strip())
                i += 1
            rows = [[cell.strip() for cell in row.strip("|").split("|")] for row in table_lines]
            rows = [
                row for row in rows
                if not all(cell and set(cell) <= {"-", ":", " "} for cell in row)
            ]
            if rows:
                table = doc.add_table(rows=len(rows), cols=max(len(r) for r in rows))
                table.style = "Table Grid"
                for ri, row in enumerate(rows):
                    for ci, value in enumerate(row):
                        cell = table.rows[ri].cells[ci]
                        cell.text = ""
                        _docx_add_inline_markdown(cell.paragraphs[0], value)
                        if ri == 0:
                            for paragraph in table.rows[ri].cells[ci].paragraphs:
                                for run in paragraph.runs:
                                    run.bold = True
                continue
        numbered = _numbered_markdown_text(stripped)
        if stripped.startswith(("- ", "* ", "• ")):
            p = doc.add_paragraph(style="List Bullet")
            _docx_add_inline_markdown(p, stripped[2:].strip())
        elif numbered is not None:
            p = doc.add_paragraph(style="List Number")
            _docx_add_inline_markdown(p, numbered)
        elif stripped.startswith("> "):
            p = doc.add_paragraph()
            p.style = "Intense Quote"
            _docx_add_inline_markdown(p, stripped[2:].strip())
        else:
            p = doc.add_paragraph()
            _docx_add_inline_markdown(p, stripped)
        i += 1


def cmd_create_docx(args: argparse.Namespace) -> int:
    try:
        import docx  # type: ignore
    except ImportError:
        _missing("python-docx")
    path = _validate_output_path(args.path, {"docx"})
    staged = staging_path(path)
    doc = docx.Document(str(_validate_path(args.template))) if args.template else docx.Document()
    if args.font:
        doc.styles["Normal"].font.name = args.font
    if args.title:
        doc.add_heading(args.title, 0)
    if args.subtitle:
        p = doc.add_paragraph(args.subtitle)
        p.style = "Subtitle" if "Subtitle" in [s.name for s in doc.styles] else p.style
    if args.input_md:
        _docx_add_markdown(doc, _read_text(args.input_md))
    elif args.body:
        _docx_add_markdown(doc, args.body)
    if args.footer:
        for section in doc.sections:
            section.footer.paragraphs[0].text = args.footer
    if args.author:
        doc.core_properties.author = args.author
    try:
        doc.save(str(staged))
        snapshot, validation = _publish_edit(staged, path)
    finally:
        staged.unlink(missing_ok=True)
    print(json.dumps({
        "created": str(path),
        "snapshot": str(snapshot) if snapshot else None,
        "validation": validation,
    }, ensure_ascii=False, indent=2))
    return 0


def cmd_create_xlsx(args: argparse.Namespace) -> int:
    path = _validate_output_path(args.path, {"xlsx"})
    staged = staging_path(path)
    staged_qa = staged.with_suffix(".xlsx.qa.json")
    spec_path = _validate_path(args.spec)
    create_xlsx_from_spec, _, _ = _load_xlsx_renderer()
    try:
        result = create_xlsx_from_spec(
            staged,
            spec_path,
            workspace_root=Path.cwd(),
        )
        if result["qa"]["status"] == "fail":
            staged.unlink(missing_ok=True)
            staged_qa.unlink(missing_ok=True)
            print(json.dumps(result, ensure_ascii=False, indent=2))
            return 4
        snapshot, validation = _publish_edit(staged, path)
    except BaseException:
        staged_qa.unlink(missing_ok=True)
        raise
    finally:
        staged.unlink(missing_ok=True)
    final_qa = path.with_suffix(".xlsx.qa.json")
    result["path"] = str(path)
    result["qaPath"] = str(final_qa)
    result["artifact"] = {
        "path": str(path),
        "snapshot": str(snapshot) if snapshot else None,
        "validation": validation,
    }
    if staged_qa.exists():
        write_artifact_manifest(final_qa, result, Path.cwd())
        staged_qa.unlink(missing_ok=True)
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0


def _load_xlsx_renderer():
    skills_root = Path(__file__).resolve().parents[2]
    renderer_dir = skills_root / "xlsx-workbook-design" / "scripts"
    renderer_path = renderer_dir / "xlsx_model_renderer.py"
    if not renderer_path.exists():
        _die(f"ERROR: XLSX renderer not found: {renderer_path}", 3)
    if str(renderer_dir) not in sys.path:
        sys.path.insert(0, str(renderer_dir))
    try:
        from xlsx_model_renderer import (  # type: ignore
            audit_xlsx_formula_integrity,
            create_xlsx_from_spec,
            inspect_formula_cache,
        )
    except ImportError as exc:
        _die(f"ERROR: failed to load XLSX renderer: {exc}", 1)
    return create_xlsx_from_spec, audit_xlsx_formula_integrity, inspect_formula_cache


def _load_pptx_renderer():
    skills_root = Path(__file__).resolve().parents[2]
    renderer_dir = skills_root / "pptx-presentation-design" / "scripts"
    renderer_path = renderer_dir / "pptx_renderer.py"
    if not renderer_path.exists():
        _die(f"ERROR: PPTX renderer not found: {renderer_path}", 3)
    if str(renderer_dir) not in sys.path:
        sys.path.insert(0, str(renderer_dir))
    try:
        from pptx_renderer import create_pptx_from_spec  # type: ignore
    except ImportError as exc:
        _die(f"ERROR: failed to load PPTX renderer: {exc}", 1)
    return create_pptx_from_spec


def _load_html_deck_renderer():
    skills_root = Path(__file__).resolve().parents[2]
    renderer_dir = skills_root / "pptx-presentation-design" / "scripts"
    renderer_path = renderer_dir / "html_deck_renderer.py"
    if not renderer_path.exists():
        _die(f"ERROR: HTML deck renderer not found: {renderer_path}", 3)
    if str(renderer_dir) not in sys.path:
        sys.path.insert(0, str(renderer_dir))
    try:
        from html_deck_renderer import render_html_deck  # type: ignore
    except ImportError as exc:
        _die(f"ERROR: failed to load HTML deck renderer: {exc}", 1)
    return render_html_deck


def cmd_create_pptx(args: argparse.Namespace) -> int:
    path = _validate_output_path(args.path, {"pptx"})
    staged = staging_path(path)
    create_pptx_from_spec = _load_pptx_renderer()
    try:
        create_pptx_from_spec(str(staged), args.spec, args.template, Path.cwd())
        snapshot, validation = _publish_edit(staged, path)
    finally:
        staged.unlink(missing_ok=True)
    print(json.dumps({
        "created": str(path),
        "snapshot": str(snapshot) if snapshot else None,
        "validation": validation,
    }, ensure_ascii=False, indent=2))
    return 0


def cmd_create_html_pptx(args: argparse.Namespace) -> int:
    if not args.path:
        _die("ERROR: --path is required for create_html_pptx", 3)
    if args.spec != "-":
        args.spec = str(_validate_path(args.spec))
    args.outdir = str(_validate_output_dir(args.outdir))
    path = _validate_output_path(args.path, {"pptx"})
    staged = staging_path(path)
    render_html_deck = _load_html_deck_renderer()
    try:
        result = render_html_deck(
            spec_path=args.spec,
            out_dir=args.outdir,
            pptx_path=str(staged),
            mode=args.mode,
            screenshot=args.screenshot,
            workspace_root=Path.cwd(),
        )
        if result["qa"]["status"] == "fail":
            staged.unlink(missing_ok=True)
            print(json.dumps(result, ensure_ascii=False, indent=2))
            return 4
        snapshot, validation = _publish_edit(staged, path)
    finally:
        staged.unlink(missing_ok=True)
    result["artifact"] = {
        "path": str(path),
        "snapshot": str(snapshot) if snapshot else None,
        "validation": validation,
    }
    pptx_result = result.get("manifest", {}).get("pptx")
    if isinstance(pptx_result, dict):
        pptx_result["path"] = str(path)
        write_artifact_manifest(Path(result["manifestPath"]), result["manifest"], Path.cwd())
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0


# ---------------------------------------------------------------------------
# ooxml / render / recalc / validate / convert
# ---------------------------------------------------------------------------

def cmd_unpack(args: argparse.Namespace) -> int:
    path = _validate_path(args.path)
    if _ext(path) not in {"docx", "pptx", "xlsx"}:
        _die("ERROR: unpack supports .docx/.pptx/.xlsx only", 3)
    outdir = _validate_output_dir(args.outdir, allow_existing=True)
    if any(outdir.iterdir()):
        if not args.overwrite:
            _die(f"ERROR: output directory is not empty: {outdir}. Pass --overwrite to replace it.", 3)
        marker = outdir / UNPACK_MARKER
        try:
            marker_payload = json.loads(marker.read_text(encoding="utf-8"))
        except (FileNotFoundError, OSError, json.JSONDecodeError):
            _die(
                f"ERROR: refusing to overwrite unmanaged directory: {outdir}. "
                f"Only directories created by this unpack command may be replaced.",
                3,
            )
        if marker_payload.get("kind") != "nexa-ooxml-unpack" or marker_payload.get("version") != 1:
            _die(f"ERROR: invalid managed-unpack marker in: {outdir}", 3)
        shutil.rmtree(outdir)
        outdir.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(path) as zf:
        for member in zf.infolist():
            target = (outdir / member.filename).resolve()
            try:
                target.relative_to(outdir)
            except ValueError:
                _die(f"ERROR: unsafe ZIP member escapes output directory: {member.filename}", 3)
        zf.extractall(outdir)
        xml_count = sum(
            1 for member in zf.infolist()
            if member.filename.lower().endswith((".xml", ".rels"))
        )
    (outdir / UNPACK_MARKER).write_text(
        json.dumps({
            "kind": "nexa-ooxml-unpack",
            "version": 1,
            "source": str(path),
            "sourceSha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        }, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    print(f"unpacked {path.name} -> {outdir} ({xml_count} XML parts preserved byte-for-byte)")
    return 0


def cmd_pack(args: argparse.Namespace) -> int:
    input_dir = _validate_path(args.input_dir)
    if not input_dir.is_dir():
        _die(f"ERROR: input directory is not a directory: {input_dir}", 3)
    if not (input_dir / "[Content_Types].xml").exists():
        _die("ERROR: input directory does not look like an unpacked Office document", 3)
    path = _validate_output_path(args.path, {"docx", "pptx", "xlsx"})
    staged = staging_path(path)
    try:
        with zipfile.ZipFile(staged, "w", zipfile.ZIP_DEFLATED) as zf:
            for item in sorted(input_dir.rglob("*")):
                if item.is_file() and item.name != UNPACK_MARKER:
                    zf.write(item, item.relative_to(input_dir).as_posix())
        snapshot, validation = _publish_edit(staged, path)
    finally:
        staged.unlink(missing_ok=True)
    print(json.dumps({
        "packedFrom": str(input_dir),
        "path": str(path),
        "snapshot": str(snapshot) if snapshot else None,
        "validation": validation,
    }, ensure_ascii=False, indent=2))
    return 0


def _convert_to_pdf(path: Path, outdir: Path) -> Path:
    completed = _run_soffice_convert(path, "pdf", outdir)
    if completed.returncode != 0:
        stderr = completed.stderr.strip() or completed.stdout.strip() or "LibreOffice conversion failed"
        _die(stderr, completed.returncode or 1)
    pdf = _expected_converted_path(path, outdir, "pdf")
    if not pdf.exists():
        matches = sorted(outdir.glob("*.pdf"))
        if matches:
            return matches[0]
        _die(f"ERROR: LibreOffice did not produce a PDF in {outdir}", 1)
    return pdf


def cmd_render(args: argparse.Namespace) -> int:
    path = _validate_path(args.path)
    if _ext(path) not in {"docx", "pptx", "xlsx", "pdf"}:
        _die("ERROR: render supports .docx/.pptx/.xlsx/.pdf only", 3)
    pdftoppm = _find_pdftoppm()
    if not pdftoppm:
        _die("MISSING_DEP: Poppler/pdftoppm\nInstall Poppler and ensure pdftoppm is on PATH.", 2)
    outdir = _validate_output_dir(args.outdir, allow_existing=True)
    image_format = args.format.lower()
    if image_format not in {"png", "jpeg"}:
        _die("ERROR: --format must be png or jpeg", 3)
    for pattern in ("page*.png", "page*.jpg", "page*.jpeg", "page*.ppm"):
        for old_preview in outdir.glob(pattern):
            if old_preview.is_file():
                old_preview.unlink()
    with tempfile.TemporaryDirectory(prefix="nexa-render-") as tmp:
        pdf = path if _ext(path) == "pdf" else _convert_to_pdf(path, Path(tmp))
        prefix = outdir / "page"
        cmd = [
            pdftoppm,
            f"-{image_format}",
            "-r",
            str(args.dpi),
            str(pdf),
            str(prefix),
        ]
        completed = _run_subprocess(cmd, text=True, capture_output=True, check=False)
    if completed.stdout:
        print(completed.stdout.strip())
    if completed.stderr:
        print(completed.stderr.strip(), file=sys.stderr)
    if completed.returncode != 0:
        return completed.returncode
    images = sorted(outdir.glob(f"page*.{'jpg' if image_format == 'jpeg' else 'png'}"))
    if not images:
        _die(f"ERROR: renderer completed without producing page images in {outdir}", 1)
    print(f"rendered {len(images)} page image(s) to {outdir}")
    for image in images[:20]:
        print(image)
    if len(images) > 20:
        print(f"... {len(images) - 20} more")
    return 0


def _scan_xlsx_formula_errors(path: Path) -> tuple[int, dict[str, list[str]]]:
    try:
        import openpyxl  # type: ignore
    except ImportError:
        _missing("openpyxl")
    wb = openpyxl.load_workbook(str(path), data_only=True, read_only=True)
    found: dict[str, list[str]] = {err: [] for err in EXCEL_ERRORS}
    try:
        for sheet_name in wb.sheetnames:
            for row in wb[sheet_name].iter_rows():
                for cell in row:
                    if isinstance(cell.value, str):
                        for err in EXCEL_ERRORS:
                            if err in cell.value:
                                found[err].append(f"{sheet_name}!{cell.coordinate}")
                                break
    finally:
        wb.close()
    total = sum(len(locations) for locations in found.values())
    return total, {err: locs for err, locs in found.items() if locs}


def _count_xlsx_formulas(path: Path) -> int:
    try:
        import openpyxl  # type: ignore
    except ImportError:
        _missing("openpyxl")
    wb = openpyxl.load_workbook(str(path), data_only=False, read_only=True)
    count = 0
    try:
        for sheet_name in wb.sheetnames:
            for row in wb[sheet_name].iter_rows():
                for cell in row:
                    if isinstance(cell.value, str) and cell.value.startswith("="):
                        count += 1
    finally:
        wb.close()
    return count


def cmd_recalc_xlsx(args: argparse.Namespace) -> int:
    path = _validate_path(args.path)
    if _ext(path) != "xlsx":
        _die("ERROR: recalc_xlsx requires a .xlsx file", 3)
    soffice = _find_soffice()
    if not soffice:
        _die(
            "MISSING_DEP: LibreOffice/soffice\n"
            "Use lint_xlsx for static checks or install LibreOffice for real recalculation.",
            2,
        )

    risk = scan_ooxml_risks(path)
    unsafe_features = {
        key: parts
        for key, parts in risk["features"].items()
        if parts and key in {"macros", "signatures", "externalLinks", "pivotCaches", "dataModel"}
    }
    if unsafe_features and not args.allow_risky:
        print(json.dumps({
            "status": "blocked",
            "recalculated": False,
            "reason": "LibreOffice round-trip could damage preservation-sensitive workbook features.",
            "risk": risk,
            "hint": "Use Excel COM/precise OOXML, or pass --allow-risky only after explicit review.",
        }, ensure_ascii=False, indent=2))
        return 5

    _, audit_xlsx_formula_integrity, inspect_formula_cache = _load_xlsx_renderer()
    before_hash = hashlib.sha256(path.read_bytes()).hexdigest()
    with tempfile.TemporaryDirectory(prefix=".nexa-recalc-", dir=path.parent) as tmp:
        root = Path(tmp)
        source_dir = root / "source"
        output_dir = root / "output"
        profile_dir = root / "profile"
        source_dir.mkdir()
        output_dir.mkdir()
        source = source_dir / path.name
        shutil.copy2(path, source)
        cmd = [
            *_soffice_base_cmd(soffice, profile_dir),
            "--convert-to",
            "xlsx:Calc MS Excel 2007 XML",
            "--outdir",
            str(output_dir),
            str(source),
        ]
        completed = _run_subprocess(
            cmd,
            text=True,
            capture_output=True,
            check=False,
            env=_soffice_env(),
            timeout=120,
        )
        recalculated = output_dir / path.name
        if completed.returncode != 0 or not recalculated.is_file():
            detail = completed.stderr.strip() or completed.stdout.strip() or "no output file"
            _die(f"ERROR: LibreOffice recalculation failed: {detail}", 1)

        structural = validate_ooxml_package(recalculated)
        if structural.status == "fail":
            print(json.dumps({
                "status": "fail",
                "recalculated": False,
                "reason": "LibreOffice output failed OOXML validation and was not published.",
                "validation": structural.to_dict(),
            }, ensure_ascii=False, indent=2))
            return 1
        formula_qa = audit_xlsx_formula_integrity(recalculated)
        cached_error_total, cached_errors = _scan_xlsx_formula_errors(recalculated)
        if formula_qa["status"] == "fail" or cached_error_total:
            print(json.dumps({
                "status": "fail",
                "recalculated": False,
                "reason": "Recalculated workbook failed formula QA and was not published.",
                "formula_qa": formula_qa,
                "cached_formula_errors": cached_errors,
            }, ensure_ascii=False, indent=2))
            return 1
        staged = staging_path(path)
        shutil.copy2(recalculated, staged)
        snapshot, validation = _publish_edit(staged, path)

    after_hash = hashlib.sha256(path.read_bytes()).hexdigest()
    result = {
        "status": formula_qa["status"],
        "backend": "libreoffice",
        "recalculated": True,
        "rewritten": True,
        "contentChanged": before_hash != after_hash,
        "formula_qa": formula_qa,
        "calculation": inspect_formula_cache(path),
        "cached_formula_errors": cached_errors,
        "total_formulas": _count_xlsx_formulas(path),
        "risk": risk,
        "snapshot": str(snapshot) if snapshot else None,
        "validation": validation,
    }
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0


def cmd_lint_xlsx(args: argparse.Namespace) -> int:
    path = _validate_path(args.path)
    if _ext(path) != "xlsx":
        _die("ERROR: lint_xlsx requires a .xlsx file", 3)
    _, audit_xlsx_formula_integrity, inspect_formula_cache = _load_xlsx_renderer()
    formula_qa = audit_xlsx_formula_integrity(path)
    total_errors, cached_errors = _scan_xlsx_formula_errors(path)
    contract_result = _validate_xlsx_contract(path, args.contract) if args.contract else None
    result = {
        "status": "fail" if (
            formula_qa["status"] == "fail"
            or total_errors
            or (contract_result is not None and contract_result["status"] == "fail")
        ) else formula_qa["status"],
        "formula_qa": formula_qa,
        "calculation": inspect_formula_cache(path),
        "cached_formula_errors": cached_errors,
        "preservationRisk": scan_ooxml_risks(path),
        "contract": contract_result,
        "note": "No LibreOffice or Excel process was used.",
    }
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0 if result["status"] != "fail" else 1


def _validate_xlsx_contract(path: Path, contract_path: str) -> dict[str, Any]:
    try:
        import openpyxl  # type: ignore
    except ImportError:
        _missing("openpyxl")

    contract = _read_json(contract_path)
    _, _, inspect_formula_cache = _load_xlsx_renderer()
    wb = openpyxl.load_workbook(str(path), data_only=False, read_only=False)
    values_wb = openpyxl.load_workbook(str(path), data_only=True, read_only=True)
    errors: list[dict[str, Any]] = []
    checks: dict[str, Any] = {}
    try:
        required_sheets = [str(name) for name in contract.get("required_sheets", [])]
        missing_sheets = [name for name in required_sheets if name not in wb.sheetnames]
        checks["requiredSheets"] = {"required": required_sheets, "missing": missing_sheets}
        for name in missing_sheets:
            errors.append({"code": "sheet.missing", "sheet": name})

        required_names = [str(name) for name in contract.get("required_named_ranges", [])]
        existing_names = {str(item.name) for item in wb.defined_names.values()}
        missing_names = [name for name in required_names if name not in existing_names]
        checks["requiredNamedRanges"] = {"required": required_names, "missing": missing_names}
        for name in missing_names:
            errors.append({"code": "named_range.missing", "name": name})

        hardcode_findings: dict[str, list[str]] = {}
        for sheet_name in contract.get("no_numeric_hardcodes_in", []):
            if sheet_name not in wb.sheetnames:
                continue
            locations: list[str] = []
            for row in wb[sheet_name].iter_rows():
                for cell in row:
                    if isinstance(cell.value, (int, float)) and not isinstance(cell.value, bool):
                        locations.append(cell.coordinate)
            if locations:
                hardcode_findings[str(sheet_name)] = locations[:200]
                errors.append({
                    "code": "formula.numeric_hardcode",
                    "sheet": str(sheet_name),
                    "locations": locations[:200],
                    "truncated": len(locations) > 200,
                })
        checks["numericHardcodes"] = hardcode_findings

        row_checks: dict[str, Any] = {}
        for sheet_name, minimum in contract.get("min_rows", {}).items():
            if sheet_name not in wb.sheetnames:
                continue
            actual = wb[sheet_name].max_row
            row_checks[str(sheet_name)] = {"minimum": int(minimum), "actual": actual}
            if actual < int(minimum):
                errors.append({
                    "code": "rows.minimum",
                    "sheet": str(sheet_name),
                    "minimum": int(minimum),
                    "actual": actual,
                })
        checks["minimumRows"] = row_checks

        sentinel_checks: dict[str, Any] = {}
        for reference, expected in contract.get("sentinels", {}).items():
            sheet_name, separator, coordinate = str(reference).rpartition("!")
            if not separator or sheet_name not in values_wb.sheetnames:
                errors.append({"code": "sentinel.invalid_reference", "reference": reference})
                continue
            actual = values_wb[sheet_name][coordinate].value
            matches = actual == expected
            if isinstance(actual, (int, float)) and isinstance(expected, (int, float)):
                matches = abs(float(actual) - float(expected)) <= 1e-9
            sentinel_checks[str(reference)] = {
                "expected": expected,
                "actual": actual,
                "matches": matches,
            }
            if not matches:
                errors.append({
                    "code": "sentinel.mismatch",
                    "reference": reference,
                    "expected": expected,
                    "actual": actual,
                })
        checks["sentinels"] = sentinel_checks
        calculation = inspect_formula_cache(path)
        checks["formulaCache"] = calculation
        if contract.get("require_formula_cache") and calculation["coverage"] < 1.0:
            errors.append({
                "code": "formula.cache_missing",
                "formulaCells": calculation["formulaCells"],
                "cachedFormulaCells": calculation["cachedFormulaCells"],
            })
    finally:
        wb.close()
        values_wb.close()

    return {"status": "fail" if errors else "pass", "errors": errors, "checks": checks}


def _contract_text_checks(text: str, contract: dict[str, Any]) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    errors: list[dict[str, Any]] = []
    required = [str(item) for item in contract.get("required_text", [])]
    missing = [item for item in required if item not in text]
    forbidden = [str(item) for item in contract.get("forbidden_text", [])]
    present = [item for item in forbidden if item in text]
    for item in missing:
        errors.append({"code": "required_text.missing", "text": item})
    for item in present:
        errors.append({"code": "forbidden_text.present", "text": item})
    return errors, {
        "requiredText": {"required": required, "missing": missing},
        "forbiddenText": {"forbidden": forbidden, "present": present},
    }


def _validate_docx_contract(path: Path, contract_path: str) -> dict[str, Any]:
    try:
        import docx  # type: ignore
    except ImportError:
        _missing("python-docx")
    contract = _read_json(contract_path)
    document = docx.Document(str(path))
    stories: list[str] = [paragraph.text for paragraph in document.paragraphs]
    for table in document.tables:
        stories.extend(cell.text for row in table.rows for cell in row.cells)
    for section in document.sections:
        for story in (section.header, section.footer):
            stories.extend(paragraph.text for paragraph in story.paragraphs)
            for table in story.tables:
                stories.extend(cell.text for row in table.rows for cell in row.cells)
    errors, checks = _contract_text_checks("\n".join(stories), contract)
    measurements = {
        "paragraphs": len(document.paragraphs),
        "tables": len(document.tables),
    }
    checks["measurements"] = measurements
    for key, actual in measurements.items():
        minimum = contract.get(f"min_{key}")
        if minimum is not None and actual < int(minimum):
            errors.append({
                "code": f"{key}.minimum",
                "minimum": int(minimum),
                "actual": actual,
            })
    return {"status": "fail" if errors else "pass", "errors": errors, "checks": checks}


def _validate_pptx_contract(path: Path, contract_path: str) -> dict[str, Any]:
    try:
        from pptx import Presentation  # type: ignore
    except ImportError:
        _missing("python-pptx")
    contract = _read_json(contract_path)
    presentation = Presentation(str(path))
    fragments: list[str] = []
    titles: list[str] = []
    slides_with_notes = 0
    for slide in presentation.slides:
        if slide.shapes.title is not None:
            titles.append(slide.shapes.title.text)
        if slide.has_notes_slide:
            notes_text = "\n".join(
                shape.text for shape in slide.notes_slide.shapes
                if getattr(shape, "has_text_frame", False)
            ).strip()
            if notes_text:
                slides_with_notes += 1
                fragments.append(notes_text)
        for shape in slide.shapes:
            if getattr(shape, "has_text_frame", False):
                fragments.append(shape.text)
            if getattr(shape, "has_table", False):
                fragments.extend(cell.text for row in shape.table.rows for cell in row.cells)
    errors, checks = _contract_text_checks("\n".join(fragments), contract)
    slide_count = len(presentation.slides)
    checks["measurements"] = {
        "slides": slide_count,
        "slidesWithNotes": slides_with_notes,
        "titles": titles,
    }
    if contract.get("min_slides") is not None and slide_count < int(contract["min_slides"]):
        errors.append({
            "code": "slides.minimum",
            "minimum": int(contract["min_slides"]),
            "actual": slide_count,
        })
    if contract.get("max_slides") is not None and slide_count > int(contract["max_slides"]):
        errors.append({
            "code": "slides.maximum",
            "maximum": int(contract["max_slides"]),
            "actual": slide_count,
        })
    required_titles = [str(item) for item in contract.get("required_slide_titles", [])]
    missing_titles = [item for item in required_titles if item not in titles]
    checks["requiredSlideTitles"] = {"required": required_titles, "missing": missing_titles}
    for title in missing_titles:
        errors.append({"code": "slide_title.missing", "title": title})
    if contract.get("require_speaker_notes") and slides_with_notes != slide_count:
        errors.append({
            "code": "speaker_notes.missing",
            "slides": slide_count,
            "slidesWithNotes": slides_with_notes,
        })
    return {"status": "fail" if errors else "pass", "errors": errors, "checks": checks}


def _validate_artifact_contract(path: Path, contract_path: str) -> dict[str, Any]:
    if _ext(path) == "xlsx":
        return _validate_xlsx_contract(path, contract_path)
    if _ext(path) == "docx":
        return _validate_docx_contract(path, contract_path)
    if _ext(path) == "pptx":
        return _validate_pptx_contract(path, contract_path)
    _die("ERROR: validation contracts require DOCX, PPTX, or XLSX", 3)


def cmd_validate(args: argparse.Namespace) -> int:
    path = _validate_path(args.path)
    ext = _ext(path)
    result: dict[str, Any] = {"format": ext, "path": str(path), "status": "pass"}
    if ext in {"docx", "pptx", "xlsx"}:
        structural = validate_ooxml_package(path)
        result["structural"] = structural.to_dict()
        if structural.status == "fail":
            result["status"] = "fail"
            print(json.dumps(result, ensure_ascii=False, indent=2))
            return 1
    if ext == "docx":
        try:
            import docx  # type: ignore
        except ImportError:
            _missing("python-docx")
        doc = docx.Document(str(path))
        result["backend"] = {
            "id": "python-docx",
            "paragraphs": len(doc.paragraphs),
            "tables": len(doc.tables),
        }
    elif ext == "pptx":
        try:
            from pptx import Presentation  # type: ignore
        except ImportError:
            _missing("python-pptx")
        prs = Presentation(str(path))
        result["backend"] = {"id": "python-pptx", "slides": len(prs.slides)}
    elif ext == "xlsx":
        try:
            import openpyxl  # type: ignore
        except ImportError:
            _missing("openpyxl")
        wb = openpyxl.load_workbook(str(path), data_only=False, read_only=True)
        sheet_count = len(wb.worksheets)
        sheet_names = ",".join(wb.sheetnames)
        wb.close()
        total_errors, errors = _scan_xlsx_formula_errors(path)
        formula_count = _count_xlsx_formulas(path)
        result["backend"] = {
            "id": "openpyxl",
            "sheets": sheet_count,
            "sheetNames": sheet_names.split(",") if sheet_names else [],
            "formulas": formula_count,
            "formulaErrors": total_errors,
        }
        if errors:
            result["status"] = "fail"
            result["cachedFormulaErrors"] = errors
            print(json.dumps(result, ensure_ascii=False, indent=2))
            return 1
        try:
            _, audit_xlsx_formula_integrity, inspect_formula_cache = _load_xlsx_renderer()
            formula_qa = audit_xlsx_formula_integrity(path)
            result["formulaQa"] = formula_qa
            result["calculation"] = inspect_formula_cache(path)
            if formula_qa["status"] == "fail":
                result["status"] = "fail"
                print(json.dumps(result, ensure_ascii=False, indent=2))
                return 1
        except SystemExit:
            raise
        except Exception as exc:  # noqa: BLE001
            result["formulaQaWarning"] = f"{type(exc).__name__}: {exc}"
            if result["status"] == "pass":
                result["status"] = "warn"
    elif ext == "pdf":
        try:
            from pypdf import PdfReader  # type: ignore
        except ImportError:
            _missing("pypdf")
        reader = PdfReader(str(path))
        result["backend"] = {"id": "pypdf", "pages": len(reader.pages)}
    else:
        _die(f"ERROR: validate does not support .{ext}", 3)
    if getattr(args, "contract", None):
        contract = _validate_artifact_contract(path, args.contract)
        result["contract"] = contract
        if contract["status"] == "fail":
            result["status"] = "fail"
    if args.json:
        print(json.dumps(result, ensure_ascii=False, indent=2))
    else:
        backend = result.get("backend", {})
        print(f"VALID {ext.upper()} backend={backend.get('id', 'structural')} status={result['status']}")
    return 0 if result["status"] != "fail" else 1


def cmd_convert(args: argparse.Namespace) -> int:
    path = _validate_path(args.path)
    outdir = Path(args.outdir).resolve() if args.outdir else path.parent
    try:
        outdir.relative_to(Path.cwd().resolve())
    except ValueError:
        _die(f"ERROR: --outdir escapes workspace: {outdir}", 3)
    outdir.mkdir(parents=True, exist_ok=True)
    completed = _run_soffice_convert(path, args.to, outdir)
    if completed.stdout:
        print(completed.stdout.strip())
    if completed.stderr:
        print(completed.stderr.strip(), file=sys.stderr)
    if completed.returncode != 0:
        return completed.returncode
    print(f"converted {path.name} -> .{args.to} in {outdir}")
    return 0


# ---------------------------------------------------------------------------
# argparse
# ---------------------------------------------------------------------------

def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="edit_doc.py",
        description="Edit existing DOCX/PPTX/PDF/XLSX documents from run_shell.",
    )
    p.add_argument("--path", help="Absolute path to the target file")
    sub = p.add_subparsers(dest="cmd", required=True)

    p_check = sub.add_parser("check", help="Report available backends")
    p_check.add_argument("--json", action="store_true", help="Emit machine-readable readiness JSON")
    p_check.set_defaults(func=cmd_check)

    p_rep = sub.add_parser("replace", help="Replace text (docx/pptx/xlsx)")
    p_rep.add_argument("--find", required=True)
    p_rep.add_argument("--replace", default="")
    p_rep.add_argument("--dry-run", action="store_true")
    p_rep.add_argument("--expected-sha256", default=None, help="Fail if the artifact changed since inspection")
    p_rep.add_argument("--expected-count", type=int, default=None, help="Fail unless exactly this many matches exist")
    p_rep.set_defaults(func=cmd_replace)

    p_red = sub.add_parser("redact", help="Redact text (docx/pptx/xlsx)")
    p_red.add_argument("--find", required=True)
    p_red.add_argument("--replace", default=None)
    p_red.add_argument("--dry-run", action="store_true")
    p_red.add_argument("--expected-sha256", default=None, help="Fail if the artifact changed since inspection")
    p_red.add_argument("--expected-count", type=int, default=None, help="Fail unless exactly this many matches exist")
    p_red.set_defaults(func=cmd_redact)

    p_ext = sub.add_parser("extract", help="Extract plain text (docx/pdf/pptx/xlsx)")
    p_ext.add_argument("--pages", default=None, help="e.g. 1-3 or 1,3,5")
    p_ext.add_argument("--sheets", default=None, help="Comma-separated XLSX sheet names")
    p_ext.set_defaults(func=cmd_extract)

    p_ins = sub.add_parser("insert_slide", help="Insert a slide into a pptx")
    p_ins.add_argument("--after", type=int, default=0)
    p_ins.add_argument("--title", default="")
    p_ins.add_argument("--body", default="")
    p_ins.set_defaults(func=cmd_insert_slide)

    p_ver = sub.add_parser("version", help="Snapshot file to .nexa/doc-history")
    p_ver.set_defaults(func=cmd_version)

    p_cd = sub.add_parser("create_docx", help="Create a DOCX using python-docx")
    p_cd.add_argument("--title", default="")
    p_cd.add_argument("--subtitle", default="")
    p_cd.add_argument("--body", default="")
    p_cd.add_argument("--input-md", default=None, help="Absolute path to markdown source")
    p_cd.add_argument("--template", default=None, help="Optional absolute .docx template path")
    p_cd.add_argument("--font", default="Calibri")
    p_cd.add_argument("--footer", default="")
    p_cd.add_argument("--author", default="Nexa")
    p_cd.set_defaults(func=cmd_create_docx)

    p_cx = sub.add_parser("create_xlsx", help="Create an XLSX workbook from a JSON spec")
    p_cx.add_argument("--spec", required=True, help="Absolute path to workbook JSON spec")
    p_cx.set_defaults(func=cmd_create_xlsx)

    p_cp = sub.add_parser("create_pptx", help="Create a PPTX presentation from a JSON spec")
    p_cp.add_argument("--spec", required=True, help="Absolute path to deck JSON spec, or '-' to read JSON from stdin")
    p_cp.add_argument("--template", default=None, help="Optional absolute .pptx template path")
    p_cp.set_defaults(func=cmd_create_pptx)

    p_chp = sub.add_parser("create_html_pptx", help="Create a PPTX from an HTML-first deck spec")
    p_chp.add_argument("--spec", required=True, help="Absolute path to HTML deck JSON spec, or '-' to read JSON from stdin")
    p_chp.add_argument("--outdir", required=True, help="Absolute output project directory for HTML, render artifacts, manifest, and QA")
    p_chp.add_argument("--mode", choices=["hybrid", "native", "raster"], default="hybrid")
    p_chp.add_argument("--screenshot", choices=["auto", "require", "skip"], default="auto")
    p_chp.set_defaults(func=cmd_create_html_pptx)

    p_unpack = sub.add_parser("unpack", help="Unpack DOCX/PPTX/XLSX into editable OOXML")
    p_unpack.add_argument("--outdir", required=True, help="Absolute output directory")
    p_unpack.add_argument("--overwrite", action="store_true")
    p_unpack.set_defaults(func=cmd_unpack)

    p_pack = sub.add_parser("pack", help="Pack an unpacked OOXML directory back into DOCX/PPTX/XLSX")
    p_pack.add_argument("--input-dir", required=True, help="Absolute unpacked OOXML directory")
    p_pack.set_defaults(func=cmd_pack)

    p_render = sub.add_parser("render", help="Render DOCX/PPTX/XLSX/PDF pages or slides to images for visual QA")
    p_render.add_argument("--outdir", required=True, help="Absolute output directory for page images")
    p_render.add_argument("--dpi", type=int, default=150)
    p_render.add_argument("--format", default="png", choices=["png", "jpeg"])
    p_render.set_defaults(func=cmd_render)

    p_lint = sub.add_parser("lint_xlsx", help="Lint XLSX formulas without LibreOffice or Excel automation")
    p_lint.add_argument(
        "--contract",
        default=None,
        help="Optional absolute JSON workbook contract with sheets, names, hardcodes, rows, and sentinels",
    )
    p_lint.set_defaults(func=cmd_lint_xlsx)

    p_recalc = sub.add_parser("recalc_xlsx", help="Recalculate and resave XLSX with LibreOffice")
    p_recalc.add_argument(
        "--allow-risky",
        action="store_true",
        help="Allow LibreOffice round-trip after reviewing preservation-sensitive features",
    )
    p_recalc.set_defaults(func=cmd_recalc_xlsx)

    p_val = sub.add_parser("validate", help="Validate OOXML structure, relationships, content types, and backend open")
    p_val.add_argument("--json", action="store_true", help="Emit machine-readable validation JSON")
    p_val.add_argument(
        "--contract",
        default=None,
        help="Optional absolute JSON contract for format-specific content and structure checks",
    )
    p_val.set_defaults(func=cmd_validate)

    p_conv = sub.add_parser("convert", help="Convert via LibreOffice headless")
    p_conv.add_argument("--to", required=True, help="Output extension/filter, e.g. pdf, docx, xlsx")
    p_conv.add_argument("--outdir", default=None, help="Optional absolute output directory")
    p_conv.set_defaults(func=cmd_convert)

    return p


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        return args.func(args)
    except SystemExit:
        raise
    except Exception as e:  # noqa: BLE001
        print(f"ERROR: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
