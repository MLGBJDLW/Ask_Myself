#!/usr/bin/env python3
"""Internal execution plan and legacy v1 compatibility for Office artifacts."""

from __future__ import annotations

import argparse
import ctypes
import fnmatch
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import zipfile
from bisect import bisect_right
from dataclasses import dataclass
from pathlib import Path
from typing import Any
from xml.etree import ElementTree as ET

from office_artifact_runtime import (
    office_backend_statuses,
    publish_staged_artifact,
    rollback_published_artifact,
    scan_ooxml_risks,
    snapshot_file,
    staging_path,
    validate_ooxml_package,
    workspace_path,
    write_artifact_manifest,
)

FORMATS = {"docx", "pptx", "xlsx"}
FORMAT_EXTENSIONS = {
    "docx": {".docx", ".docm", ".dotx", ".dotm"},
    "xlsx": {".xlsx", ".xlsm", ".xltx", ".xltm"},
    "pptx": {".pptx", ".pptm", ".potx", ".potm"},
}
INTENTS = {"create_new", "edit_existing", "validate", "recalculate", "finalize"}
PRESERVATION_POLICIES = {"strict", "balanced", "replace"}
RENDER_POLICIES = {"none", "important_surfaces", "all"}
BACKENDS = {"auto", "nexa-openxml", "libreoffice", "officecli", "windows-com"}


@dataclass
class OfficeExecutionPlan:
    job_version: int
    format: str
    intent: str
    input: Path | None
    output: Path
    operations: list[dict[str, Any]]
    preservation_policy: str
    validation_contract: dict[str, Any] | str | None
    render_policy: str
    backend: str
    allow_network_backend: bool
    manifest: Path

    @classmethod
    def from_internal_dict(
        cls, payload: dict[str, Any], workspace_root: Path
    ) -> OfficeExecutionPlan:
        plan_version = payload.get("planVersion")
        if type(plan_version) is not int or plan_version != 1:
            raise ValueError(f"unsupported planVersion: {plan_version}")
        artifact_format = str(payload.get("format", "")).lower()
        if artifact_format not in FORMATS:
            raise ValueError(f"format must be one of: {', '.join(sorted(FORMATS))}")
        intent = str(payload.get("intent", "")).lower()
        if intent not in INTENTS:
            raise ValueError(f"intent must be one of: {', '.join(sorted(INTENTS))}")

        raw_input = payload.get("input")
        input_path = (
            workspace_path(Path(str(raw_input)), workspace_root, must_exist=True)
            if raw_input
            else None
        )
        if intent != "create_new" and input_path is None:
            raise ValueError(f"input is required for intent={intent}")

        raw_output = payload.get("output") or raw_input
        if not raw_output:
            raise ValueError("output is required")
        output = workspace_path(Path(str(raw_output)), workspace_root)
        if output.suffix.lower() not in FORMAT_EXTENSIONS[artifact_format]:
            raise ValueError(f"output suffix must belong to the {artifact_format} format family")
        if input_path is not None and input_path.suffix.lower() not in FORMAT_EXTENSIONS[artifact_format]:
            raise ValueError(f"input suffix must belong to the {artifact_format} format family")
        if input_path is not None and input_path.suffix.lower() != output.suffix.lower():
            raise ValueError("internal execution plan cannot convert macro/template extensions")
        if intent == "create_new" and output.suffix.lower() != f".{artifact_format}":
            raise ValueError("create_new cannot fabricate macro/template package semantics")

        operations = payload.get("operations", [])
        if not isinstance(operations, list) or not all(isinstance(item, dict) for item in operations):
            raise ValueError("operations must be an array of objects")
        preservation = str(payload.get("preservationPolicy", "strict"))
        if preservation not in PRESERVATION_POLICIES:
            raise ValueError("invalid preservationPolicy")
        render_policy = str(payload.get("renderPolicy", "none"))
        if render_policy not in RENDER_POLICIES:
            raise ValueError("invalid renderPolicy")
        backend = str(payload.get("backend", "auto"))
        if backend not in BACKENDS:
            raise ValueError("invalid backend")

        raw_manifest = payload.get("manifest") or str(output.parent / "artifact-manifest.json")
        manifest = workspace_path(Path(str(raw_manifest)), workspace_root)
        artifact_paths = {output}
        if input_path is not None:
            artifact_paths.add(input_path)
        if manifest in artifact_paths:
            raise ValueError("manifest path must be distinct from input and output artifact paths")
        validation_contract = payload.get("validationContract")
        if validation_contract is not None and not isinstance(validation_contract, (dict, str)):
            raise ValueError("validationContract must be an object or a workspace-local JSON path")
        return cls(
            job_version=plan_version,
            format=artifact_format,
            intent=intent,
            input=input_path,
            output=output,
            operations=operations,
            preservation_policy=preservation,
            validation_contract=validation_contract,
            render_policy=render_policy,
            backend=backend,
            allow_network_backend=bool(payload.get("allowNetworkBackend", False)),
            manifest=manifest,
        )

    @classmethod
    def from_legacy_job(
        cls, payload: dict[str, Any], workspace_root: Path
    ) -> OfficeExecutionPlan:
        job_version = payload.get("jobVersion", 1)
        if type(job_version) is not int or job_version != 1:
            raise ValueError(f"unsupported jobVersion: {job_version}")
        translated = {key: value for key, value in payload.items() if key != "jobVersion"}
        translated["planVersion"] = 1
        return cls.from_internal_dict(translated, workspace_root)

    @classmethod
    def from_dict(cls, payload: dict[str, Any], workspace_root: Path) -> OfficeExecutionPlan:
        """Compatibility entry point for the retired public Job v1 protocol."""
        return cls.from_legacy_job(payload, workspace_root)


OfficeArtifactJob = OfficeExecutionPlan


def _load_job(raw: str) -> dict[str, Any]:
    if raw == "-":
        payload = json.load(sys.stdin)
    else:
        path = workspace_path(Path(raw), Path.cwd(), must_exist=True)
        payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise TypeError("job root must be an object")
    return payload


def _run(
    command: list[str],
    *,
    timeout: int = 180,
    cwd: Path | None = None,
) -> subprocess.CompletedProcess[str]:
    kwargs: dict[str, Any] = {
        "text": True,
        "encoding": "utf-8",
        "errors": "replace",
        "capture_output": True,
        "check": False,
        "timeout": timeout,
        "env": {
            **os.environ,
            "NEXA_OFFICE_SKIP_SNAPSHOT": "1",
            "PYTHONUTF8": "1",
            "PYTHONIOENCODING": "utf-8",
        },
        "cwd": str(cwd) if cwd is not None else None,
    }
    if os.name == "nt":
        kwargs["creationflags"] = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    return subprocess.run(command, check=kwargs.pop("check", False), **kwargs)


def _replace_path_strings(value: Any, old: Path, new: Path) -> Any:
    old_text = str(old)
    new_text = str(new)
    escaped_old = old_text.replace("\\", "\\\\")
    escaped_new = new_text.replace("\\", "\\\\")
    if isinstance(value, str):
        return value.replace(escaped_old, escaped_new).replace(old_text, new_text)
    if isinstance(value, list):
        return [_replace_path_strings(item, old, new) for item in value]
    if isinstance(value, dict):
        return {key: _replace_path_strings(item, old, new) for key, item in value.items()}
    return value


def _editor_command(path: Path, command: str, *arguments: str) -> list[str]:
    editor = Path(__file__).with_name("edit_doc.py")
    return [sys.executable, str(editor), "--path", str(path), command, *arguments]


def _run_editor(
    path: Path,
    command: str,
    arguments: list[str],
    actions: list[dict[str, Any]],
    workspace_root: Path,
    *,
    timeout: int = 180,
) -> str:
    completed = _run(
        _editor_command(path, command, *arguments),
        timeout=timeout,
        cwd=workspace_root,
    )
    action = {
        "command": command,
        "status": "ok" if completed.returncode == 0 else "failed",
        "exitCode": completed.returncode,
        "stdout": completed.stdout.strip(),
        "stderr": completed.stderr.strip(),
    }
    actions.append(action)
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip() or "document command failed"
        raise RuntimeError(f"{command}: {detail}")
    return completed.stdout.strip()


def _run_pptxgenjs_author(
    working: Path,
    spec_path: Path,
    actions: list[dict[str, Any]],
    workspace_root: Path,
) -> None:
    configured_node = os.environ.get("NEXA_PPTXGENJS_NODE")
    node = (
        str(Path(configured_node).expanduser().resolve())
        if configured_node and Path(configured_node).expanduser().is_file()
        else shutil.which("node")
    )
    if not node:
        raise RuntimeError("PptxGenJS author adapter requires Node.js")
    adapter = (
        Path(__file__).resolve().parents[2]
        / "pptx-presentation-design"
        / "scripts"
        / "pptxgenjs_adapter.mjs"
    )
    completed = _run(
        [
            node,
            str(adapter),
            "--spec",
            str(spec_path),
            "--out",
            str(working),
            "--workspace",
            str(workspace_root),
        ],
        timeout=300,
        cwd=workspace_root,
    )
    action = {
        "command": "pptxgenjs-author",
        "status": "ok" if completed.returncode == 0 else "failed",
        "exitCode": completed.returncode,
        "stdout": completed.stdout.strip(),
        "stderr": completed.stderr.strip(),
        "engine": "pptxgenjs",
        "engineVersion": "4.0.1",
        "networkPolicy": "local-assets-only",
    }
    actions.append(action)
    if completed.returncode != 0:
        raise RuntimeError(
            "pptxgenjs-author: "
            + (completed.stderr.strip() or completed.stdout.strip() or "author adapter failed")
        )


def _select_backend(job: OfficeExecutionPlan) -> str:
    if job.backend != "auto":
        return job.backend
    needs_recalculation = job.intent in {"recalculate", "finalize"} or any(
        str(operation.get("op", "")).lower() == "recalculate"
        for operation in job.operations
    )
    if job.format == "xlsx" and needs_recalculation:
        return "libreoffice"
    if job.intent == "finalize":
        return "windows-com"
    return "nexa-openxml"


def _backend_status(backend_id: str) -> dict[str, Any]:
    statuses = {item["id"]: item for item in office_backend_statuses()}
    return statuses[backend_id]


def _assert_backend_support(job: OfficeExecutionPlan, backend: str) -> None:
    has_recalculate_operation = any(
        str(operation.get("op", "")).lower() == "recalculate"
        for operation in job.operations
    )
    if backend == "officecli" and job.intent != "create_new":
        raise ValueError("OfficeCLI backend only supports explicit create_new jobs")
    if backend == "libreoffice" and not (
        job.format == "xlsx"
        and (job.intent in {"recalculate", "finalize"} or has_recalculate_operation)
    ):
        raise ValueError("LibreOffice backend is limited to XLSX recalculate/finalize jobs")
    if backend == "windows-com" and job.intent not in {"recalculate", "finalize"}:
        raise ValueError("Windows COM backend is limited to recalculate/finalize jobs")
    if backend == "windows-com" and job.intent == "recalculate" and job.format != "xlsx":
        raise ValueError("Windows COM recalculation supports XLSX only")
    if backend == "nexa-openxml" and job.intent in {"recalculate", "finalize"}:
        raise ValueError(f"Nexa OpenXML cannot satisfy intent={job.intent}; select a native backend")


def _operation_path(
    raw: Any,
    workspace_root: Path,
    *,
    must_exist: bool,
) -> Path:
    if raw is None or not str(raw).strip():
        raise ValueError("operation path cannot be empty")
    return workspace_path(Path(str(raw)), workspace_root, must_exist=must_exist)


def _native_create(
    job: OfficeExecutionPlan,
    working: Path,
    actions: list[dict[str, Any]],
    workspace_root: Path,
) -> None:
    operation = job.operations[0] if job.operations else {}
    if job.format == "docx":
        arguments: list[str] = []
        if operation.get("spec") is not None:
            spec_path = _operation_path(operation["spec"], workspace_root, must_exist=True)
            arguments.extend(["--spec", str(spec_path)])
        mapping = {
            "title": "--title",
            "subtitle": "--subtitle",
            "body": "--body",
            "font": "--font",
            "footer": "--footer",
            "author": "--author",
        }
        for key, flag in mapping.items():
            if operation.get(key) is not None:
                arguments.extend([flag, str(operation[key])])
        for key, flag in (("inputMd", "--input-md"), ("template", "--template")):
            if operation.get(key) is not None:
                path = _operation_path(operation[key], workspace_root, must_exist=True)
                arguments.extend([flag, str(path)])
        _run_editor(working, "create_docx", arguments, actions, workspace_root)
    elif job.format == "xlsx":
        spec = operation.get("spec")
        if not spec:
            raise ValueError("create_new XLSX requires operations[0].spec")
        spec_path = _operation_path(spec, workspace_root, must_exist=True)
        _run_editor(working, "create_xlsx", ["--spec", str(spec_path)], actions, workspace_root)
    else:
        spec = operation.get("spec")
        if not spec:
            raise ValueError("create_new PPTX requires operations[0].spec")
        if operation.get("authorEngine") == "pptxgenjs":
            spec_path = _operation_path(spec, workspace_root, must_exist=True)
            _run_pptxgenjs_author(working, spec_path, actions, workspace_root)
        elif operation.get("htmlFirst"):
            outdir = operation.get("outdir")
            if not outdir:
                raise ValueError("HTML-first PPTX requires outdir")
            spec_path = _operation_path(spec, workspace_root, must_exist=True)
            outdir_path = _operation_path(outdir, workspace_root, must_exist=False)
            arguments = ["--spec", str(spec_path), "--outdir", str(outdir_path)]
            arguments.extend(["--mode", str(operation.get("mode", "hybrid"))])
            arguments.extend(["--screenshot", str(operation.get("screenshot", "auto"))])
            _run_editor(working, "create_html_pptx", arguments, actions, workspace_root, timeout=300)
        else:
            spec_path = _operation_path(spec, workspace_root, must_exist=True)
            arguments = ["--spec", str(spec_path)]
            if operation.get("template"):
                template_path = _operation_path(
                    operation["template"], workspace_root, must_exist=True
                )
                arguments.extend(["--template", str(template_path)])
            _run_editor(working, "create_pptx", arguments, actions, workspace_root)


def _native_operations(
    job: OfficeExecutionPlan,
    working: Path,
    actions: list[dict[str, Any]],
    workspace_root: Path,
) -> tuple[list[str], set[str]]:
    changed: list[str] = []
    authorized_parts: set[str] = set()
    xlsx_typed = {
        "set_value", "set_formula", "set_range", "clear_range", "set_style",
        "rename_sheet", "set_defined_name", "set_data_validation", "create_table",
        "set_number_format", "set_chart_title",
    }
    pptx_typed = {
        "set_text", "clone_slide", "insert_slide", "reorder_slides", "set_transition",
        "set_alt_text", "set_speaker_notes", "add_comment",
    }
    docx_review = {
        "add_comment", "strip_comments", "tracked_replace", "accept_changes", "reject_changes",
        "add_bookmark", "insert_field", "wrap_content_control", "set_protection",
        "bind_template",
    }
    for index, operation in enumerate(job.operations):
        name = str(operation.get("op", "")).lower()
        if name in {"validate", "render", "recalculate"}:
            continue
        allowed_patterns = set(_authorized_part_patterns(job.format, name))
        before_parts = _all_part_hashes(working)
        exact_parts = _exact_operation_parts(job.format, name, operation, working)
        before_exact_payloads = _read_package_parts(working, exact_parts or set())
        declared_parts: set[str] = set()
        element_id = str(operation.get("elementId") or f"/{job.format}/operation[{index}]")
        if job.format == "xlsx" and name in xlsx_typed:
            spec_path: Path | None = None
            try:
                with tempfile.NamedTemporaryFile(
                    mode="w",
                    encoding="utf-8",
                    suffix=".json",
                    prefix=".nexa-xlsx-operation-",
                    dir=working.parent,
                    delete=False,
                ) as handle:
                    json.dump({"operations": [operation]}, handle, ensure_ascii=False)
                    spec_path = Path(handle.name)
                output = _run_editor(
                    working,
                    "edit_xlsx",
                    ["--spec", str(spec_path)],
                    actions,
                    workspace_root,
                )
                payload = json.loads(output)
                changed.extend(str(item) for item in payload.get("changedCells", []))
                declared_parts.update(str(item) for item in payload.get("changedParts", []))
            finally:
                if spec_path is not None:
                    spec_path.unlink(missing_ok=True)
        elif job.format == "docx" and name in docx_review:
            spec_path = None
            try:
                with tempfile.NamedTemporaryFile(
                    mode="w",
                    encoding="utf-8",
                    suffix=".json",
                    prefix=".nexa-docx-review-operation-",
                    dir=working.parent,
                    delete=False,
                ) as handle:
                    json.dump({"operations": [operation]}, handle, ensure_ascii=False)
                    spec_path = Path(handle.name)
                output = _run_editor(
                    working,
                    "review_docx",
                    ["--spec", str(spec_path)],
                    actions,
                    workspace_root,
                )
                payload = json.loads(output)
                changed.extend(str(item) for item in payload.get("changedParts", []))
                declared_parts.update(str(item) for item in payload.get("changedParts", []))
            finally:
                if spec_path is not None:
                    spec_path.unlink(missing_ok=True)
        elif job.format == "pptx" and name in pptx_typed:
            spec_path = None
            try:
                with tempfile.NamedTemporaryFile(
                    mode="w",
                    encoding="utf-8",
                    suffix=".json",
                    prefix=".nexa-pptx-operation-",
                    dir=working.parent,
                    delete=False,
                ) as handle:
                    json.dump({"operations": [operation]}, handle, ensure_ascii=False)
                    spec_path = Path(handle.name)
                output = _run_editor(
                    working,
                    "edit_pptx",
                    ["--spec", str(spec_path)],
                    actions,
                    workspace_root,
                )
                payload = json.loads(output)
                changed.extend(str(item) for item in payload.get("changedParts", []))
                declared_parts.update(str(item) for item in payload.get("changedParts", []))
            finally:
                if spec_path is not None:
                    spec_path.unlink(missing_ok=True)
        elif name in {"replace", "redact", "secure_redact"}:
            find = str(operation.get("find", ""))
            if not find:
                raise ValueError(f"operations[{index}].find is required")
            arguments = ["--find", find]
            if operation.get("replace") is not None:
                arguments.extend(["--replace", str(operation["replace"])])
            if operation.get("expectedSha256") is not None:
                arguments.extend(["--expected-sha256", str(operation["expectedSha256"])])
            if operation.get("expectedMatches") is not None:
                arguments.extend(["--expected-count", str(operation["expectedMatches"])])
            if name == "secure_redact" and any(
                operation.get(field) is not None
                for field in ("scope", "occurrence", "allowStyleMerge")
            ):
                raise ValueError("secure_redact always inspects every textual package story and does not accept scope/occurrence/style-merge options")
            if name != "secure_redact" and operation.get("scope") is not None:
                scope = operation["scope"]
                scope_text = ",".join(str(item) for item in scope) if isinstance(scope, list) else str(scope)
                arguments.extend(["--scope", scope_text])
            if name != "secure_redact" and operation.get("occurrence") is not None:
                arguments.extend(["--occurrence", str(operation["occurrence"])])
            if name != "secure_redact" and operation.get("allowStyleMerge"):
                arguments.append("--allow-style-merge")
            if name == "secure_redact" and operation.get("privacyScrub"):
                arguments.append("--privacy-scrub")
            _run_editor(working, name, arguments, actions, workspace_root)
            changed.append(element_id)
        else:
            raise ValueError(f"unsupported operation: {name or '<missing>'}")
        actual_parts = _changed_part_names(before_parts, _all_part_hashes(working))
        if exact_parts is not None:
            _verify_exact_semantic_scope(
                job.format,
                name,
                operation,
                before_exact_payloads,
                working,
            )
        if exact_parts is not None:
            outside_scope = sorted(actual_parts - exact_parts)
        elif job.format == "pptx" and name == "clone_slide":
            mutable_existing = {
                "ppt/presentation.xml",
                "ppt/_rels/presentation.xml.rels",
                "[Content_Types].xml",
            }
            outside_scope = sorted(
                part
                for part in actual_parts
                if part in before_parts and part not in mutable_existing
            )
        else:
            outside_scope = sorted(
                part for part in actual_parts if not _matches_any_part_pattern(part, allowed_patterns)
            )
        if outside_scope:
            raise RuntimeError(
                f"{job.format} {name} changed package parts outside its allowed scope: "
                + ", ".join(outside_scope)
            )
        if declared_parts and actual_parts != declared_parts:
            raise RuntimeError(
                f"{job.format} {name} changedParts evidence mismatch: "
                + json.dumps({
                    "declared": sorted(declared_parts),
                    "actual": sorted(actual_parts),
                }, ensure_ascii=False)
            )
        authorized_parts.update(actual_parts)
    return changed, authorized_parts


def _officecli_create(job: OfficeExecutionPlan, working: Path, actions: list[dict[str, Any]]) -> None:
    if job.intent != "create_new":
        raise ValueError("OfficeCLI backend is limited to explicit create_new jobs")
    if not job.allow_network_backend:
        raise PermissionError("OfficeCLI requires allowNetworkBackend=true because hosted mode may transmit data")
    status = _backend_status("officecli")
    if status["status"] != "ready" or not status.get("path"):
        raise RuntimeError(status.get("detail") or "OfficeCLI is unavailable")
    operation = job.operations[0] if job.operations else {}
    title = str(operation.get("title") or working.stem)
    prompt = operation.get("prompt")
    if not prompt:
        raise ValueError("OfficeCLI create requires operations[0].prompt")
    with tempfile.TemporaryDirectory(prefix=".nexa-officecli-", dir=working.parent) as tmp:
        output_dir = Path(tmp)
        command = [
            str(status["path"]), "new", job.format, title,
            "--prompt", str(prompt), "--out", str(output_dir), "--json", "--no-publish",
        ]
        completed = _run(command, timeout=600)
        actions.append({
            "command": "officecli",
            "status": "ok" if completed.returncode == 0 else "failed",
            "exitCode": completed.returncode,
            "stdout": completed.stdout.strip(),
            "stderr": completed.stderr.strip(),
        })
        if completed.returncode != 0:
            raise RuntimeError(completed.stderr.strip() or completed.stdout.strip())
        candidates = sorted(output_dir.rglob(f"*.{job.format}"), key=lambda path: path.stat().st_mtime_ns)
        if not candidates:
            raise RuntimeError("OfficeCLI completed without producing the requested artifact")
        shutil.copy2(candidates[-1], working)


def _force_disable_macros(app: Any) -> Any:
    """Disable VBA before opening an untrusted Office artifact.

    Office's AutomationSecurity property uses the MsoAutomationSecurity values;
    3 is msoAutomationSecurityForceDisable. Failure is fatal because silently
    opening with the process default would violate the backend's safety contract.
    """
    try:
        previous = app.AutomationSecurity
        app.AutomationSecurity = 3
        return previous
    except Exception as error:  # noqa: BLE001
        raise RuntimeError(f"could not force-disable Office macros: {error}") from error


def _restore_automation_security(app: Any, previous: Any) -> None:
    try:
        app.AutomationSecurity = previous
    except Exception as error:  # noqa: BLE001
        raise RuntimeError(f"could not restore Office automation security: {error}") from error


def _update_safe_word_fields(document: Any) -> int:
    """Update only local pagination fields; never DDE/LINK/INCLUDE fields."""
    safe_types = {26, 33, 66}  # NUMPAGES, PAGE, SECTIONPAGES
    updated = 0
    for index in range(1, int(document.Fields.Count) + 1):
        field = document.Fields.Item(index)
        if int(field.Type) in safe_types:
            field.Update()
            updated += 1
    return updated


def _windows_process_ids(executable: str) -> set[int]:
    if os.name != "nt":
        return set()

    class PROCESSENTRY32W(ctypes.Structure):
        _fields_ = [
            ("dwSize", ctypes.c_ulong),
            ("cntUsage", ctypes.c_ulong),
            ("th32ProcessID", ctypes.c_ulong),
            ("th32DefaultHeapID", ctypes.c_size_t),
            ("th32ModuleID", ctypes.c_ulong),
            ("cntThreads", ctypes.c_ulong),
            ("th32ParentProcessID", ctypes.c_ulong),
            ("pcPriClassBase", ctypes.c_long),
            ("dwFlags", ctypes.c_ulong),
            ("szExeFile", ctypes.c_wchar * 260),
        ]

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.CreateToolhelp32Snapshot.argtypes = [ctypes.c_ulong, ctypes.c_ulong]
    kernel32.CreateToolhelp32Snapshot.restype = ctypes.c_void_p
    kernel32.Process32FirstW.argtypes = [ctypes.c_void_p, ctypes.POINTER(PROCESSENTRY32W)]
    kernel32.Process32FirstW.restype = ctypes.c_int
    kernel32.Process32NextW.argtypes = [ctypes.c_void_p, ctypes.POINTER(PROCESSENTRY32W)]
    kernel32.Process32NextW.restype = ctypes.c_int
    kernel32.CloseHandle.argtypes = [ctypes.c_void_p]
    kernel32.CloseHandle.restype = ctypes.c_int
    snapshot = kernel32.CreateToolhelp32Snapshot(0x00000002, 0)
    if snapshot in {None, 0, ctypes.c_void_p(-1).value}:
        raise ctypes.WinError(ctypes.get_last_error())
    ids: set[int] = set()
    try:
        entry = PROCESSENTRY32W()
        entry.dwSize = ctypes.sizeof(entry)
        success = kernel32.Process32FirstW(snapshot, ctypes.byref(entry))
        while success:
            if str(entry.szExeFile).casefold() == executable.casefold():
                ids.add(int(entry.th32ProcessID))
            success = kernel32.Process32NextW(snapshot, ctypes.byref(entry))
    finally:
        kernel32.CloseHandle(snapshot)
    return ids


def _assign_office_process_to_kill_job(
    app: Any,
    process_id_override: int | None = None,
) -> tuple[int, int] | None:
    """Bind a COM Office server to a kill-on-close Job Object on Windows."""
    if os.name != "nt":
        return None
    raw_hwnd = getattr(app, "Hwnd", 0) or getattr(app, "HWND", 0) or 0
    if callable(raw_hwnd):
        try:
            raw_hwnd = raw_hwnd()
        except Exception:  # noqa: BLE001 - dynamic COM members may be pseudo-methods
            raw_hwnd = 0
    hwnd = int(raw_hwnd or 0)

    class IO_COUNTERS(ctypes.Structure):
        _fields_ = [(name, ctypes.c_ulonglong) for name in (
            "ReadOperationCount", "WriteOperationCount", "OtherOperationCount",
            "ReadTransferCount", "WriteTransferCount", "OtherTransferCount",
        )]

    class JOBOBJECT_BASIC_LIMIT_INFORMATION(ctypes.Structure):
        _fields_ = [
            ("PerProcessUserTimeLimit", ctypes.c_longlong),
            ("PerJobUserTimeLimit", ctypes.c_longlong),
            ("LimitFlags", ctypes.c_ulong),
            ("MinimumWorkingSetSize", ctypes.c_size_t),
            ("MaximumWorkingSetSize", ctypes.c_size_t),
            ("ActiveProcessLimit", ctypes.c_ulong),
            ("Affinity", ctypes.c_size_t),
            ("PriorityClass", ctypes.c_ulong),
            ("SchedulingClass", ctypes.c_ulong),
        ]

    class JOBOBJECT_EXTENDED_LIMIT_INFORMATION(ctypes.Structure):
        _fields_ = [
            ("BasicLimitInformation", JOBOBJECT_BASIC_LIMIT_INFORMATION),
            ("IoInfo", IO_COUNTERS),
            ("ProcessMemoryLimit", ctypes.c_size_t),
            ("JobMemoryLimit", ctypes.c_size_t),
            ("PeakProcessMemoryUsed", ctypes.c_size_t),
            ("PeakJobMemoryUsed", ctypes.c_size_t),
        ]

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    kernel32.CreateJobObjectW.argtypes = [ctypes.c_void_p, ctypes.c_wchar_p]
    kernel32.CreateJobObjectW.restype = ctypes.c_void_p
    kernel32.SetInformationJobObject.argtypes = [
        ctypes.c_void_p, ctypes.c_int, ctypes.c_void_p, ctypes.c_ulong,
    ]
    kernel32.SetInformationJobObject.restype = ctypes.c_int
    kernel32.OpenProcess.argtypes = [ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
    kernel32.OpenProcess.restype = ctypes.c_void_p
    kernel32.AssignProcessToJobObject.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
    kernel32.AssignProcessToJobObject.restype = ctypes.c_int
    kernel32.CloseHandle.argtypes = [ctypes.c_void_p]
    kernel32.CloseHandle.restype = ctypes.c_int
    user32.GetWindowThreadProcessId.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_ulong)]
    user32.GetWindowThreadProcessId.restype = ctypes.c_ulong
    process_id = ctypes.c_ulong(process_id_override or 0)
    if not process_id.value:
        if not hwnd:
            raise RuntimeError("Office COM application did not expose a process window handle")
        if not user32.GetWindowThreadProcessId(hwnd, ctypes.byref(process_id)):
            raise RuntimeError("could not resolve Office COM process id")
    job = kernel32.CreateJobObjectW(None, None)
    if not job:
        raise ctypes.WinError(ctypes.get_last_error())
    process = 0
    try:
        limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION()
        limits.BasicLimitInformation.LimitFlags = 0x00002000  # KILL_ON_JOB_CLOSE
        if not kernel32.SetInformationJobObject(
            job, 9, ctypes.byref(limits), ctypes.sizeof(limits)
        ):
            raise ctypes.WinError(ctypes.get_last_error())
        process = kernel32.OpenProcess(0x0100 | 0x0001, False, process_id.value)
        if not process:
            raise ctypes.WinError(ctypes.get_last_error())
        if not kernel32.AssignProcessToJobObject(job, process):
            raise ctypes.WinError(ctypes.get_last_error())
        return int(job), int(process)
    except Exception:
        if process:
            kernel32.CloseHandle(process)
        kernel32.CloseHandle(job)
        raise


def _close_office_kill_job(handles: tuple[int, int] | None) -> None:
    if handles is None or os.name != "nt":
        return
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.CloseHandle.argtypes = [ctypes.c_void_p]
    kernel32.CloseHandle.restype = ctypes.c_int
    job, process = handles
    kernel32.CloseHandle(process)
    kernel32.CloseHandle(job)


def _guard_office_process(
    app: Any,
    *,
    executable: str,
    existing_pids: set[int],
) -> tuple[int, int] | None:
    try:
        try:
            return _assign_office_process_to_kill_job(app)
        except RuntimeError as error:
            if "window handle" not in str(error):
                raise
            new_pids = _windows_process_ids(executable) - existing_pids
            if len(new_pids) != 1:
                raise RuntimeError(
                    f"could not identify the isolated {executable} COM process: {sorted(new_pids)}"
                ) from error
            return _assign_office_process_to_kill_job(app, new_pids.pop())
    except Exception:
        try:
            app.Quit()
        finally:
            raise


def _wait_for_excel_calculation(
    app: Any,
    timeout_seconds: float = 120.0,
) -> str:
    """Wait until Excel reports xlDone; xlPending never proves recalculation."""
    deadline = time.monotonic() + timeout_seconds
    while True:
        try:
            state = int(app.CalculationState)
        except Exception as error:  # noqa: BLE001
            raise RuntimeError(f"could not read Excel calculation state: {error}") from error
        if state == 0:
            return "done"
        if time.monotonic() >= deadline:
            raise TimeoutError(
                f"Excel calculation did not reach xlDone within {timeout_seconds:g} seconds"
            )
        try:
            import pythoncom  # type: ignore

            pythoncom.PumpWaitingMessages()
        except (ImportError, OSError):
            pass
        time.sleep(0.1)


def _clear_xlsx_formula_caches(path: Path) -> int:
    """Remove every formula cache so post-COM coverage cannot be stale."""
    staged = staging_path(path)
    removed = 0
    try:
        with zipfile.ZipFile(path) as source, zipfile.ZipFile(staged, "w") as destination:
            for info in source.infolist():
                data = source.read(info.filename)
                if info.filename.startswith("xl/worksheets/") and info.filename.endswith(".xml"):
                    data, part_removed = _remove_formula_caches_xml(data)
                    removed += part_removed
                elif info.filename == "xl/workbook.xml":
                    data = _prepare_xlsx_calc_properties_xml(data)
                destination.writestr(info, data)
        os.replace(staged, path)
    finally:
        staged.unlink(missing_ok=True)
    return removed


def _prepare_xlsx_calc_properties_xml(data: bytes) -> bytes:
    encoding = "utf-16" if data.startswith((b"\xff\xfe", b"\xfe\xff")) else "utf-8-sig"
    text = data.decode(encoding)
    match = re.search(r"<(?P<prefix>[A-Za-z_][\w.-]*:)?calcPr\b[^>]*/?>", text)
    attributes = {
        "calcMode": "auto",
        "calcOnSave": "1",
        "fullCalcOnLoad": "0",
        "forceFullCalc": "0",
    }
    if match:
        tag = match.group(0)
        for name, value in attributes.items():
            if re.search(rf"\b{re.escape(name)}=", tag):
                tag = re.sub(
                    rf"\b{re.escape(name)}=(['\"]).*?\1",
                    f'{name}="{value}"',
                    tag,
                )
            else:
                tag = tag[:-2] + f' {name}="{value}"/>' if tag.endswith("/>") else tag[:-1] + f' {name}="{value}">'
        text = text[:match.start()] + tag + text[match.end():]
    else:
        closing = re.search(r"</(?P<prefix>[A-Za-z_][\w.-]*:)?workbook\s*>", text)
        if closing is None:
            raise RuntimeError("XLSX workbook XML has no workbook closing tag")
        prefix = closing.group("prefix") or ""
        tag = f'<{prefix}calcPr ' + " ".join(f'{key}="{value}"' for key, value in attributes.items()) + "/>"
        text = text[:closing.start()] + tag + text[closing.start():]
    encoded = text.encode("utf-16" if encoding == "utf-16" else "utf-8")
    return b"\xef\xbb\xbf" + encoded if encoding == "utf-8-sig" and data.startswith(b"\xef\xbb\xbf") else encoded


def _remove_formula_caches_xml(data: bytes) -> tuple[bytes, int]:
    """Losslessly remove formula `<v>` elements without namespace reserialization."""
    if data.startswith((b"\xff\xfe", b"\xfe\xff")):
        encoding = "utf-16"
    else:
        encoding = "utf-8-sig"
    text = data.decode(encoding)
    removed = 0
    cell_pattern = re.compile(
        r"(<(?P<prefix>[A-Za-z_][\w.-]*:)?c\b[^>]*>)(?P<body>.*?)(</(?P=prefix)c>)",
        re.DOTALL,
    )

    def rewrite_cell(match: re.Match[str]) -> str:
        nonlocal removed
        prefix = match.group("prefix") or ""
        body = match.group("body")
        if not re.search(rf"<{re.escape(prefix)}f(?:\s|>)", body):
            return match.group(0)
        value_pattern = re.compile(
            rf"<{re.escape(prefix)}v(?:\s[^>]*)?(?:/>|>.*?</{re.escape(prefix)}v>)",
            re.DOTALL,
        )
        rewritten, count = value_pattern.subn("", body, count=1)
        removed += count
        return match.group(1) + rewritten + match.group(4)

    rewritten = cell_pattern.sub(rewrite_cell, text)
    if encoding == "utf-16":
        return rewritten.encode("utf-16"), removed
    bom = data.startswith(b"\xef\xbb\xbf")
    encoded = rewritten.encode("utf-8")
    return (b"\xef\xbb\xbf" + encoded if bom else encoded), removed


def _sensitive_part_hashes(path: Path, risk: dict[str, Any]) -> dict[str, str]:
    names = {
        str(name)
        for parts in risk.get("features", {}).values()
        for name in parts
    }
    if not names:
        return {}
    with zipfile.ZipFile(path) as archive:
        available = set(archive.namelist())
        return {
            name: hashlib.sha256(archive.read(name)).hexdigest()
            for name in sorted(names & available)
        }


def _all_part_hashes(path: Path) -> dict[str, str]:
    with zipfile.ZipFile(path) as archive:
        return {
            info.filename: hashlib.sha256(archive.read(info.filename)).hexdigest()
            for info in archive.infolist()
            if not info.is_dir()
        }


def _authorized_part_patterns(artifact_format: str, operation: str) -> tuple[str, ...]:
    if artifact_format == "xlsx":
        if operation in {"set_value", "set_formula", "set_range", "clear_range", "set_style"}:
            return ("xl/worksheets/*.xml",)
        if operation == "rename_sheet":
            return ("xl/workbook.xml", "xl/worksheets/*.xml")
        if operation == "set_defined_name":
            return ("xl/workbook.xml",)
        if operation == "set_data_validation":
            return ("xl/worksheets/*.xml",)
        if operation == "create_table":
            return (
                "xl/worksheets/*.xml", "xl/worksheets/_rels/*.rels",
                "xl/tables/*.xml", "[Content_Types].xml",
            )
        if operation == "set_number_format":
            return ("xl/worksheets/*.xml", "xl/styles.xml")
        if operation == "set_chart_title":
            return ("xl/charts/*.xml",)
        if operation in {"replace", "redact"}:
            return (
                "xl/worksheets/*.xml", "xl/sharedStrings.xml", "xl/workbook.xml",
                "xl/comments*.xml",
            )
        if operation == "recalculate":
            return ("xl/worksheets/*.xml", "xl/workbook.xml", "xl/calcChain.xml")
    if artifact_format == "docx":
        if operation in {"replace", "redact", "tracked_replace", "accept_changes", "reject_changes"}:
            return (
                "word/document.xml", "word/header*.xml", "word/footer*.xml",
                "word/comments*.xml", "word/footnotes.xml", "word/endnotes.xml",
            )
        if operation == "secure_redact":
            return (
                "word/*.xml", "docProps/core.xml", "docProps/custom.xml",
            )
        if operation in {"add_comment", "strip_comments"}:
            return (
                "word/document.xml", "word/comments*.xml", "word/_rels/*.rels",
                "[Content_Types].xml",
            )
        if operation in {"add_bookmark", "insert_field", "wrap_content_control", "bind_template"}:
            return ("word/document.xml",)
        if operation == "set_protection":
            return ("word/settings.xml",)
    if artifact_format == "pptx":
        if operation in {"set_text", "set_transition"}:
            return ("ppt/slides/*.xml",)
        if operation == "set_alt_text":
            return ("ppt/slides/*.xml",)
        if operation == "set_speaker_notes":
            return ("ppt/notesSlides/*.xml",)
        if operation == "add_comment":
            return (
                "ppt/commentAuthors.xml", "ppt/comments/*.xml",
                "ppt/slides/_rels/*.rels", "ppt/_rels/presentation.xml.rels",
                "[Content_Types].xml",
            )
        if operation == "reorder_slides":
            return ("ppt/presentation.xml",)
        if operation in {"replace", "redact"}:
            return ("ppt/slides/*.xml", "ppt/notesSlides/*.xml", "ppt/comments/*.xml")
        if operation in {"clone_slide", "insert_slide"}:
            return (
                "ppt/presentation.xml", "ppt/_rels/presentation.xml.rels",
                "ppt/slides/*", "ppt/notesSlides/*", "ppt/comments/*", "ppt/charts/*",
                "ppt/embeddings/*", "ppt/diagrams/*", "ppt/media/*", "[Content_Types].xml",
            )
    return ()


def _matches_any_part_pattern(part: str, patterns: set[str]) -> bool:
    return any(part == pattern or fnmatch.fnmatchcase(part, pattern) for pattern in patterns)


def _changed_part_names(before: dict[str, str], after: dict[str, str]) -> set[str]:
    return {
        name
        for name in set(before) | set(after)
        if before.get(name) != after.get(name)
    }


def _exact_operation_parts(
    artifact_format: str,
    operation: str,
    payload: dict[str, Any],
    path: Path,
) -> set[str] | None:
    skills_root = Path(__file__).resolve().parents[2]
    if operation in {"replace", "redact"}:
        patterns = set(_authorized_part_patterns(artifact_format, operation))
        with zipfile.ZipFile(path) as archive:
            return {
                name
                for name in archive.namelist()
                if _matches_any_part_pattern(name, patterns)
            }
    if artifact_format == "xlsx" and operation in {
        "set_value", "set_formula", "set_range", "clear_range", "set_style",
    }:
        scripts = skills_root / "xlsx-workbook-design" / "scripts"
        if str(scripts) not in sys.path:
            sys.path.insert(0, str(scripts))
        from xlsx_structured_editor import _workbook_sheet_parts  # type: ignore

        with zipfile.ZipFile(path) as archive:
            parts = _workbook_sheet_parts(archive)
        sheet = str(payload.get("sheet", "")).casefold()
        if sheet not in parts:
            raise RuntimeError(f"worksheet not found for strict operation: {payload.get('sheet')}")
        return {parts[sheet]}
    if artifact_format == "xlsx" and operation in {"set_data_validation", "set_number_format", "create_table"}:
        scripts = skills_root / "xlsx-workbook-design" / "scripts"
        if str(scripts) not in sys.path:
            sys.path.insert(0, str(scripts))
        from xlsx_structured_editor import _sheet_relationships_name, _workbook_sheet_parts  # type: ignore

        with zipfile.ZipFile(path) as archive:
            parts = _workbook_sheet_parts(archive)
            sheet = str(payload.get("sheet", "")).casefold()
            if sheet not in parts:
                raise RuntimeError(f"worksheet not found for strict operation: {payload.get('sheet')}")
            target = parts[sheet]
            if operation == "set_data_validation":
                return {target}
            if operation == "set_number_format":
                return {target, "xl/styles.xml"}
            table_indexes = [
                int(match.group(1))
                for name in archive.namelist()
                if (match := re.fullmatch(r"xl/tables/table([0-9]+)\.xml", name))
            ]
            index = 1
            while index in table_indexes:
                index += 1
            return {
                target,
                _sheet_relationships_name(target),
                f"xl/tables/table{index}.xml",
                "[Content_Types].xml",
            }
    if artifact_format == "xlsx" and operation == "set_defined_name":
        return {"xl/workbook.xml"}
    if artifact_format == "xlsx" and operation == "set_chart_title":
        return {str(payload.get("chartPart", ""))}
    if artifact_format == "xlsx" and operation == "rename_sheet":
        old = str(payload.get("sheet", ""))
        exact = {"xl/workbook.xml"}
        formula_tags = {
            "f", "formula", "formula1", "formula2",
            "calculatedColumnFormula", "totalsRowFormula",
        }
        with zipfile.ZipFile(path) as archive:
            for name in archive.namelist():
                if (
                    not name.startswith("xl/")
                    or not name.endswith(".xml")
                    or name == "xl/workbook.xml"
                ):
                    continue
                root = ET.fromstring(archive.read(name))
                if any(
                    re.search(rf"(?i)(?:'{re.escape(old)}'|{re.escape(old)})!", formula.text or "")
                    for formula in root.iter()
                    if formula.tag.rsplit("}", 1)[-1] in formula_tags
                ):
                    exact.add(name)
        return exact
    if artifact_format == "docx" and operation in {
        "add_bookmark", "insert_field", "wrap_content_control", "bind_template",
    }:
        return {"word/document.xml"}
    if artifact_format == "docx" and operation == "set_protection":
        return {"word/settings.xml"}
    if artifact_format == "pptx" and operation in {"set_text", "set_transition", "set_alt_text"}:
        scripts = skills_root / "pptx-presentation-design" / "scripts"
        if str(scripts) not in sys.path:
            sys.path.insert(0, str(scripts))
        from pptx_structured_editor import _target_slide, presentation_order  # type: ignore

        with zipfile.ZipFile(path) as archive:
            slide = _target_slide(payload, presentation_order(archive))
        return {slide["part"]}
    if artifact_format == "pptx" and operation in {"clone_slide", "insert_slide"}:
        scripts = skills_root / "pptx-presentation-design" / "scripts"
        if str(scripts) not in sys.path:
            sys.path.insert(0, str(scripts))
        from pptx_structured_editor import (  # type: ignore
            _allocate_part,
            _discover_clone_closure,
            _rels_path,
            _target_slide,
            presentation_order,
        )

        with zipfile.ZipFile(path) as archive:
            if operation == "clone_slide":
                source = _target_slide(payload, presentation_order(archive))
                clone_part = _allocate_part(source["part"], set(archive.namelist()))
                mapping = _discover_clone_closure(archive, source["part"], clone_part)
                exact = set(mapping.values())
                for old_part, new_part in mapping.items():
                    if _rels_path(old_part) in archive.namelist():
                        exact.add(_rels_path(new_part))
            else:
                order = presentation_order(archive)
                new_part = _allocate_part(order[0]["part"], set(archive.namelist()))
                exact = {new_part, _rels_path(new_part)}
        exact.update({
            "ppt/presentation.xml",
            "ppt/_rels/presentation.xml.rels",
            "[Content_Types].xml",
        })
        return exact
    if artifact_format == "pptx" and operation in {"set_speaker_notes", "add_comment"}:
        scripts = skills_root / "pptx-presentation-design" / "scripts"
        if str(scripts) not in sys.path:
            sys.path.insert(0, str(scripts))
        from pptx_structured_editor import (  # type: ignore
            COMMENT_AUTHORS_REL,
            COMMENT_REL,
            _allocate_part,
            _relationship_map,
            _rels_path,
            _resolve_target,
            _target_slide,
            presentation_order,
        )

        with zipfile.ZipFile(path) as archive:
            slide = _target_slide(payload, presentation_order(archive))
            slide_relationships = _relationship_map(archive, slide["part"])
            if operation == "set_speaker_notes":
                notes = next(
                    (
                        _resolve_target(slide["part"], relationship["target"])
                        for relationship in slide_relationships.values()
                        if relationship["type"].rsplit("/", 1)[-1] == "notesSlide"
                    ),
                    None,
                )
                if not notes:
                    raise RuntimeError("speaker-notes target has no notes relationship")
                return {notes}
            presentation_relationships = _relationship_map(archive, "ppt/presentation.xml")
            author = next(
                (
                    _resolve_target("ppt/presentation.xml", relationship["target"])
                    for relationship in presentation_relationships.values()
                    if relationship["type"] == COMMENT_AUTHORS_REL
                ),
                "ppt/commentAuthors.xml",
            )
            comment = next(
                (
                    _resolve_target(slide["part"], relationship["target"])
                    for relationship in slide_relationships.values()
                    if relationship["type"] == COMMENT_REL
                ),
                None,
            )
            if comment is None:
                comment = _allocate_part("ppt/comments/comment1.xml", set(archive.namelist()))
            return {
                author,
                comment,
                _rels_path(slide["part"]),
                "ppt/_rels/presentation.xml.rels",
                "[Content_Types].xml",
            }
    if artifact_format == "pptx" and operation == "reorder_slides":
        return {"ppt/presentation.xml"}
    return None


def _read_package_parts(path: Path, names: set[str]) -> dict[str, bytes]:
    with zipfile.ZipFile(path) as archive:
        return {
            name: archive.read(name)
            for name in archive.namelist()
            if name in names
        }


def _replacement_fragments(
    fragments: list[str],
    find: str,
    replacement: str,
    occurrence: int | None,
) -> tuple[list[str], int]:
    combined = "".join(fragments)
    starts: list[int] = []
    cursor = 0
    for fragment in fragments:
        starts.append(cursor)
        cursor += len(fragment)
    matches: list[tuple[int, int]] = []
    cursor = 0
    while find:
        start = combined.find(find, cursor)
        if start < 0:
            break
        matches.append((start, start + len(find)))
        cursor = start + len(find)
    selected = matches if occurrence is None else (
        [matches[occurrence - 1]] if 1 <= occurrence <= len(matches) else []
    )
    result = list(fragments)
    for start, end in reversed(selected):
        start_index = max(0, bisect_right(starts, start) - 1)
        end_index = max(0, bisect_right(starts, end - 1) - 1)
        start_offset = start - starts[start_index]
        end_offset = end - starts[end_index]
        if start_index == end_index:
            result[start_index] = (
                result[start_index][:start_offset]
                + replacement
                + result[start_index][end_offset:]
            )
            continue
        result[start_index] = result[start_index][:start_offset] + replacement
        for index in range(start_index + 1, end_index):
            result[index] = ""
        result[end_index] = result[end_index][end_offset:]
    return result, len(matches)


def _xml_text_groups(
    artifact_format: str,
    part: str,
    root: ET.Element,
) -> list[tuple[str, list[ET.Element]]]:
    if artifact_format == "docx":
        word_ns = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
        parent = {child: owner for owner in root.iter() for child in owner}
        groups: list[tuple[str, list[ET.Element]]] = []
        lowered = part.casefold()
        for paragraph in root.iter(f"{{{word_ns}}}p"):
            nodes = list(paragraph.iter(f"{{{word_ns}}}t"))
            if not nodes:
                continue
            if "/header" in lowered:
                scope = "header"
            elif "/footer" in lowered:
                scope = "footer"
            elif "/comments" in lowered:
                scope = "comments"
            elif "/footnotes" in lowered:
                scope = "footnotes"
            elif "/endnotes" in lowered:
                scope = "endnotes"
            else:
                ancestors: set[str] = set()
                current = parent.get(paragraph)
                while current is not None:
                    ancestors.add(current.tag.rsplit("}", 1)[-1])
                    current = parent.get(current)
                if "txbxContent" in ancestors:
                    scope = "textbox"
                elif "tc" in ancestors:
                    scope = "table"
                else:
                    scope = "body"
            groups.append((scope, nodes))
        return groups
    if artifact_format == "pptx":
        drawing_ns = "http://schemas.openxmlformats.org/drawingml/2006/main"
        return [
            ("all", nodes)
            for paragraph in root.iter(f"{{{drawing_ns}}}p")
            if (nodes := list(paragraph.iter(f"{{{drawing_ns}}}t")))
        ]
    if artifact_format == "xlsx":
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
        groups = []
        for element in root.iter():
            if element.tag in container_tags:
                nodes = list(element.iter(text_tag))
                if nodes:
                    groups.append(("all", nodes))
            elif element.tag in scalar_tags:
                groups.append(("all", [element]))
        return groups
    return []


def _canonical_xml(root: ET.Element) -> str:
    return ET.canonicalize(ET.tostring(root, encoding="unicode"))


def _verify_replace_semantic_scope(
    artifact_format: str,
    payload: dict[str, Any],
    before: dict[str, bytes],
    after: dict[str, bytes],
) -> None:
    find = str(payload.get("find", ""))
    replacement = str(payload.get("replace", ""))
    occurrence = payload.get("occurrence")
    if occurrence is not None:
        occurrence = int(occurrence)
    scopes: set[str] | None = None
    if artifact_format == "docx" and payload.get("scope") is not None:
        raw_scope = payload["scope"]
        scopes = {
            str(item).strip().casefold()
            for item in (raw_scope if isinstance(raw_scope, list) else str(raw_scope).split(","))
            if str(item).strip()
        }

    parsed: list[
        tuple[str, ET.Element, ET.Element, list[tuple[str, list[ET.Element]]], list[tuple[str, list[ET.Element]]]]
    ] = []
    total_matches = 0
    for part, before_bytes in before.items():
        before_root = ET.fromstring(before_bytes)
        after_root = ET.fromstring(after[part])
        before_groups = _xml_text_groups(artifact_format, part, before_root)
        after_groups = _xml_text_groups(artifact_format, part, after_root)
        if len(before_groups) != len(after_groups):
            raise RuntimeError("strict replace changed the number of textual containers")
        for (before_scope, before_nodes), (after_scope, after_nodes) in zip(
            before_groups, after_groups, strict=True
        ):
            if before_scope != after_scope or len(before_nodes) != len(after_nodes):
                raise RuntimeError("strict replace changed textual container structure")
            if scopes is None or before_scope in scopes:
                _, count = _replacement_fragments(
                    [node.text or "" for node in before_nodes],
                    find,
                    replacement,
                    None,
                )
                total_matches += count
        parsed.append((part, before_root, after_root, before_groups, after_groups))

    global_start = 1
    for part, before_root, after_root, before_groups, after_groups in parsed:
        for group_index, ((scope, before_nodes), (_, after_nodes)) in enumerate(
            zip(before_groups, after_groups, strict=True)
        ):
            selected_scope = scopes is None or scope in scopes
            fragments = [node.text or "" for node in before_nodes]
            _, group_matches = _replacement_fragments(fragments, find, replacement, None)
            local_occurrence = None
            should_apply = selected_scope and occurrence is None
            if selected_scope and occurrence is not None and global_start <= occurrence < global_start + group_matches:
                local_occurrence = occurrence - global_start + 1
                should_apply = True
            expected = fragments
            if should_apply and group_matches:
                expected, _ = _replacement_fragments(
                    fragments,
                    find,
                    replacement,
                    local_occurrence,
                )
            actual = [node.text or "" for node in after_nodes]
            if actual != expected:
                raise RuntimeError(
                    f"strict {artifact_format} replace changed text outside the requested scope "
                    f"at {part} group {group_index}"
                )
            if selected_scope:
                global_start += group_matches
            for node_index, (before_node, after_node) in enumerate(
                zip(before_nodes, after_nodes, strict=True)
            ):
                if selected_scope:
                    marker = f"__NEXA_VERIFIED_TEXT_{group_index}_{node_index}__"
                    before_node.text = marker
                    after_node.text = marker
        if _canonical_xml(before_root) != _canonical_xml(after_root):
            raise RuntimeError(
                f"strict {artifact_format} replace changed non-text semantics in {part}"
            )
    if occurrence is not None and not 1 <= occurrence <= total_matches:
        raise RuntimeError("strict replace occurrence is outside the verified match inventory")


def _verify_exact_semantic_scope(
    artifact_format: str,
    operation: str,
    payload: dict[str, Any],
    before: dict[str, bytes],
    after_path: Path,
) -> None:
    if not before:
        return
    after = _read_package_parts(after_path, set(before))
    if set(after) != set(before):
        raise RuntimeError("strict semantic scope lost a target package part")
    if operation in {"replace", "redact"}:
        _verify_replace_semantic_scope(artifact_format, payload, before, after)
        return
    if artifact_format == "xlsx":
        if operation in {
            "rename_sheet", "set_defined_name", "set_data_validation", "create_table",
            "set_number_format", "set_chart_title",
        }:
            if operation == "rename_sheet":
                scripts = Path(__file__).resolve().parents[2] / "xlsx-workbook-design" / "scripts"
                if str(scripts) not in sys.path:
                    sys.path.insert(0, str(scripts))
                from xlsx_structured_editor import _replace_sheet_reference  # type: ignore

                old = str(payload.get("sheet", ""))
                new = str(payload.get("newName", ""))
                formula_tags = {
                    "f", "formula", "formula1", "formula2",
                    "calculatedColumnFormula", "totalsRowFormula",
                }
                for name, data in before.items():
                    expected_root = ET.fromstring(data)
                    if name == "xl/workbook.xml":
                        for element in expected_root.iter():
                            local = element.tag.rsplit("}", 1)[-1]
                            if (
                                local == "sheet"
                                and element.attrib.get("name", "").casefold() == old.casefold()
                            ):
                                element.set("name", new)
                            elif local == "definedName" and element.text:
                                element.text = _replace_sheet_reference(element.text, old, new)
                    else:
                        for element in expected_root.iter():
                            if element.tag.rsplit("}", 1)[-1] in formula_tags and element.text:
                                element.text = _replace_sheet_reference(element.text, old, new)
                    if _canonical_xml(expected_root) != _canonical_xml(ET.fromstring(after[name])):
                        raise RuntimeError(
                            f"strict XLSX rename changed semantics beyond sheet references in {name}"
                        )
            with zipfile.ZipFile(after_path) as archive:
                if operation == "set_defined_name":
                    workbook = ET.fromstring(archive.read("xl/workbook.xml"))
                    if not any(
                        item.attrib.get("name", "").casefold()
                        == str(payload.get("name", "")).casefold()
                        for item in workbook.iter()
                        if item.tag.rsplit("}", 1)[-1] == "definedName"
                    ):
                        raise RuntimeError("strict XLSX defined-name operation did not create its target")
                elif operation == "set_chart_title":
                    part = str(payload.get("chartPart", ""))
                    root = ET.fromstring(archive.read(part))
                    text = "".join(
                        item.text or "" for item in root.iter()
                        if item.tag.rsplit("}", 1)[-1] == "t"
                    )
                    if str(payload.get("title", "")) not in text:
                        raise RuntimeError("strict XLSX chart-title operation did not create its target")
                elif operation == "create_table":
                    if not any(
                        name.startswith("xl/tables/table")
                        and str(payload.get("name", "")).encode("utf-8") in archive.read(name)
                        for name in archive.namelist()
                    ):
                        raise RuntimeError("strict XLSX table operation did not create its target")
                elif operation == "set_number_format":
                    if str(payload.get("formatCode", "")).encode("utf-8") not in archive.read("xl/styles.xml"):
                        raise RuntimeError("strict XLSX number format operation did not create its target")
                elif operation == "set_data_validation":
                    if str(payload.get("range", "")).replace("$", "").upper().encode("utf-8") not in b"".join(
                        archive.read(name) for name in archive.namelist()
                        if name.startswith("xl/worksheets/") and name.endswith(".xml")
                    ):
                        raise RuntimeError("strict XLSX validation operation did not create its target")
                elif operation == "rename_sheet":
                    workbook = ET.fromstring(archive.read("xl/workbook.xml"))
                    if not any(
                        item.attrib.get("name", "").casefold()
                        == str(payload.get("newName", "")).casefold()
                        for item in workbook.iter()
                        if item.tag.rsplit("}", 1)[-1] == "sheet"
                    ):
                        raise RuntimeError("strict XLSX rename operation did not create its target")
            return
        scripts = Path(__file__).resolve().parents[2] / "xlsx-workbook-design" / "scripts"
        if str(scripts) not in sys.path:
            sys.path.insert(0, str(scripts))
        from xlsx_structured_editor import _range_cells  # type: ignore

        target = str(payload.get("range") or payload.get("cell") or "")
        target_cells = set(_range_cells(target))

        def normalized(data: bytes) -> bytes:
            root = ET.fromstring(data)
            for parent in root.iter():
                for child in list(parent):
                    local = child.tag.rsplit("}", 1)[-1]
                    if local == "dimension":
                        parent.remove(child)
                    elif local == "c" and child.attrib.get("r", "").replace("$", "").upper() in target_cells:
                        parent.remove(child)
            for sheet_data in [item for item in root.iter() if item.tag.rsplit("}", 1)[-1] == "sheetData"]:
                for row in list(sheet_data):
                    if not any(child.tag.rsplit("}", 1)[-1] == "c" for child in row):
                        sheet_data.remove(row)
            return ET.tostring(root, encoding="utf-8")

        if any(normalized(before[name]) != normalized(after[name]) for name in before):
            raise RuntimeError("strict XLSX edit changed cells or sheet semantics outside the requested target")
        return
    if artifact_format == "pptx":
        p_ns = "http://schemas.openxmlformats.org/presentationml/2006/main"
        a_ns = "http://schemas.openxmlformats.org/drawingml/2006/main"

        if operation == "add_comment":
            with zipfile.ZipFile(after_path) as archive:
                if not any(
                    name.startswith("ppt/comments/")
                    and str(payload.get("comment", "")).encode("utf-8") in archive.read(name)
                    for name in archive.namelist()
                ):
                    raise RuntimeError("strict PPTX comment operation did not create its target")
            return
        if operation in {"clone_slide", "insert_slide"}:
            # The exact copy-on-write closure is computed from the pre-edit
            # relationship graph by `_exact_operation_parts`; no existing
            # slide, layout, master, media, or embedded object may change.
            return

        def normalized(data: bytes) -> bytes:
            root = ET.fromstring(data)
            if operation == "set_transition":
                for parent in root.iter():
                    for child in list(parent):
                        if child.tag == f"{{{p_ns}}}transition":
                            parent.remove(child)
            elif operation == "reorder_slides":
                for slide_list in root.iter(f"{{{p_ns}}}sldIdLst"):
                    children = list(slide_list)
                    for child in children:
                        slide_list.remove(child)
                    for child in sorted(children, key=lambda item: int(item.attrib.get("id", "0"))):
                        slide_list.append(child)
            elif operation == "set_text":
                shape_id = str(payload.get("shapeId", ""))
                shape_name = str(payload.get("shapeName", ""))
                matched = False
                for shape in root.iter(f"{{{p_ns}}}sp"):
                    properties = shape.find(f".//{{{p_ns}}}cNvPr")
                    if properties is None:
                        continue
                    if (
                        (shape_id and properties.attrib.get("id") == shape_id)
                        or (shape_name and properties.attrib.get("name") == shape_name)
                    ):
                        matched = True
                        for text in shape.iter(f"{{{a_ns}}}t"):
                            text.text = "__NEXA_TARGET_TEXT__"
                if not matched:
                    raise RuntimeError("strict PPTX set_text target shape disappeared")
            elif operation == "set_alt_text":
                shape_id = str(payload.get("shapeId", ""))
                shape_name = str(payload.get("shapeName", ""))
                matched = False
                for shape in root.iter(f"{{{p_ns}}}sp"):
                    properties = shape.find(f".//{{{p_ns}}}cNvPr")
                    if properties is None:
                        continue
                    if (
                        (shape_id and properties.attrib.get("id") == shape_id)
                        or (shape_name and properties.attrib.get("name") == shape_name)
                    ):
                        properties.attrib.pop("descr", None)
                        properties.attrib.pop("title", None)
                        matched = True
                if not matched:
                    raise RuntimeError("strict PPTX set_alt_text target shape disappeared")
            elif operation == "set_speaker_notes":
                body_shape = next(
                    (
                        shape for shape in root.iter(f"{{{p_ns}}}sp")
                        if any(
                            placeholder.attrib.get("type") == "body"
                            for placeholder in shape.iter(f"{{{p_ns}}}ph")
                        )
                    ),
                    None,
                )
                if body_shape is None:
                    raise RuntimeError("strict PPTX speaker-notes body disappeared")
                for text in body_shape.iter(f"{{{a_ns}}}t"):
                    text.text = "__NEXA_TARGET_NOTES__"
            return ET.tostring(root, encoding="utf-8")

        if any(normalized(before[name]) != normalized(after[name]) for name in before):
            raise RuntimeError("strict PPTX edit changed slide semantics outside the requested target")


def _preservation_evidence(
    source: Path,
    candidate: Path,
    risk: dict[str, Any],
    authorized_parts: set[str] | None = None,
) -> dict[str, Any]:
    before = _all_part_hashes(source)
    after = _all_part_hashes(candidate)
    authorized_parts = authorized_parts or set()
    missing = sorted(set(before) - set(after))
    added = sorted(set(after) - set(before))
    changed = sorted(
        name for name, digest in before.items()
        if name in after and after[name] != digest
    )
    unchanged = sorted(
        name for name, digest in before.items()
        if after.get(name) == digest
    )
    modified = sorted(set(changed) | set(missing) | set(added))
    unauthorized = sorted(part for part in modified if part not in authorized_parts)
    sensitive_before = _sensitive_part_hashes(source, risk)
    sensitive_after = _sensitive_part_hashes(candidate, risk)
    sensitive_unchanged = {
        name for name, digest in sensitive_before.items() if sensitive_after.get(name) == digest
    }
    verified_features = sorted(
        feature
        for feature, names in risk.get("features", {}).items()
        if names and all(str(name) in sensitive_unchanged for name in names)
    )
    return {
        "verified": not unauthorized,
        "method": "sha256-all-package-parts-allowed-diff",
        "sourceParts": len(before),
        "unchangedParts": unchanged,
        "changedParts": changed,
        "addedParts": added,
        "missingParts": missing,
        "authorizedParts": sorted(authorized_parts),
        "unauthorizedParts": unauthorized,
        "verifiedFeatures": verified_features,
    }


def _assert_native_network_closed(path: Path, artifact_format: str) -> dict[str, Any]:
    validation = validate_ooxml_package(path)
    if validation.status == "fail":
        raise RuntimeError("native Office preflight rejected unsafe package structure")
    risk = scan_ooxml_risks(path)
    blocked = {
        "unsafeExternalRelationships": risk["features"].get("unsafeExternalRelationships", []),
    }
    if artifact_format == "xlsx":
        for key in (
            "xlmMacros", "externalFormulaFunctions", "externalLinks",
            "connections", "dataModel",
        ):
            if risk["features"].get(key):
                blocked[key] = risk["features"][key]
    blocked = {key: value for key, value in blocked.items() if value}
    if blocked:
        raise RuntimeError(
            "native Office network/executable-content preflight blocked package: "
            + json.dumps(blocked, ensure_ascii=False)
        )
    return risk


def _windows_com_finalize(path: Path, artifact_format: str, actions: list[dict[str, Any]]) -> None:
    _assert_native_network_closed(path, artifact_format)
    try:
        import win32com.client  # type: ignore
    except (ImportError, OSError) as error:
        raise RuntimeError(f"Microsoft Office COM is unavailable: {error}") from error

    if artifact_format == "xlsx":
        skills_root = Path(__file__).resolve().parents[2]
        renderer_dir = skills_root / "xlsx-workbook-design" / "scripts"
        if str(renderer_dir) not in sys.path:
            sys.path.insert(0, str(renderer_dir))
        from xlsx_model_renderer import (  # type: ignore
            inspect_formula_cache,
            inspect_formula_errors,
            inspect_formula_inventory,
        )

        formula_before = inspect_formula_inventory(path)
        cache_before_invalidation = inspect_formula_cache(path)
        invalidated_formula_caches = _clear_xlsx_formula_caches(path)
        cache_after_invalidation = inspect_formula_cache(path)
        if cache_after_invalidation["cachedFormulaCells"] != 0:
            raise RuntimeError("could not invalidate every XLSX formula cache before native calculation")
        existing_pids = _windows_process_ids("EXCEL.EXE")
        app = win32com.client.DispatchEx("Excel.Application")
        kill_job = _guard_office_process(
            app, executable="EXCEL.EXE", existing_pids=existing_pids
        )
        app.Visible = False
        app.DisplayAlerts = False
        app.EnableEvents = False
        app.AskToUpdateLinks = False
        previous_calculation = None
        calculation_changed = False
        document = None
        previous_security = _force_disable_macros(app)
        office_version = str(getattr(app, "Version", "unknown"))
        try:
            document = app.Workbooks.Open(str(path.resolve()), UpdateLinks=0, ReadOnly=False)
            current_calculation = int(getattr(app, "Calculation", -4105))
            previous_calculation = (
                current_calculation if current_calculation in {-4105, -4135, 2} else None
            )
            if current_calculation != -4105:
                app.Calculation = -4105  # xlCalculationAutomatic
                calculation_changed = True
            app.CalculateBeforeSave = True
            document.ForceFullCalculation = False
            for worksheet_index in range(1, int(document.Worksheets.Count) + 1):
                document.Worksheets.Item(worksheet_index).Calculate()
            calculation_state = _wait_for_excel_calculation(app)
            document.Save()
        finally:
            try:
                if calculation_changed and previous_calculation is not None:
                    app.Calculation = previous_calculation
                if document is not None:
                    document.Close(SaveChanges=True)
            finally:
                try:
                    _restore_automation_security(app, previous_security)
                finally:
                    try:
                        app.Quit()
                    finally:
                        _close_office_kill_job(kill_job)
        formula_after = inspect_formula_inventory(path)
        cache_evidence = inspect_formula_cache(path)
        cache_evidence = {
            **cache_evidence,
            "nativeRecalculationProven": True,
            "proof": "formula-caches-invalidated-before-microsoft-excel-com-worksheets-calculate-save-xlDone",
            "preOpenFormulaCells": cache_before_invalidation["formulaCells"],
            "preOpenCachedFormulaCells": cache_before_invalidation["cachedFormulaCells"],
            "preOpenCacheCoverage": cache_before_invalidation["coverage"],
            "invalidatedFormulaCaches": invalidated_formula_caches,
        }
        cached_errors = inspect_formula_errors(path)
        if formula_before["fingerprint"] != formula_after["fingerprint"]:
            raise RuntimeError(
                "Excel-native recalculation changed formula definitions; candidate was rejected"
            )
        if cache_evidence["coverage"] < 1.0:
            raise RuntimeError(
                "Excel-native recalculation did not populate every formula cache: "
                f"{cache_evidence['cachedFormulaCells']}/{cache_evidence['formulaCells']}"
            )
        if cached_errors["count"]:
            raise RuntimeError(
                "Excel-native recalculation produced cached formula errors: "
                + json.dumps(cached_errors["byValue"], ensure_ascii=False)
            )
    elif artifact_format == "docx":
        existing_pids = _windows_process_ids("WINWORD.EXE")
        app = win32com.client.DispatchEx("Word.Application")
        kill_job = _guard_office_process(
            app, executable="WINWORD.EXE", existing_pids=existing_pids
        )
        app.Visible = False
        app.DisplayAlerts = 0
        app.Options.UpdateLinksAtOpen = False
        app.Options.UpdateFieldsAtPrint = False
        document = None
        previous_security = _force_disable_macros(app)
        office_version = str(getattr(app, "Version", "unknown"))
        try:
            document = app.Documents.Open(
                str(path.resolve()),
                ConfirmConversions=False,
                ReadOnly=False,
                AddToRecentFiles=False,
                OpenAndRepair=False,
                NoEncodingDialog=True,
            )
            safe_fields_updated = _update_safe_word_fields(document)
            document.Repaginate()
            document.Save()
        finally:
            try:
                if document is not None:
                    document.Close(SaveChanges=True)
            finally:
                try:
                    _restore_automation_security(app, previous_security)
                finally:
                    try:
                        app.Quit()
                    finally:
                        _close_office_kill_job(kill_job)
    else:
        existing_pids = _windows_process_ids("POWERPNT.EXE")
        app = win32com.client.DispatchEx("PowerPoint.Application")
        kill_job = _guard_office_process(
            app, executable="POWERPNT.EXE", existing_pids=existing_pids
        )
        document = None
        previous_security = _force_disable_macros(app)
        office_version = str(getattr(app, "Version", "unknown"))
        try:
            document = app.Presentations.Open(str(path.resolve()), WithWindow=False)
            document.Save()
        finally:
            try:
                if document is not None:
                    document.Close()
            finally:
                try:
                    _restore_automation_security(app, previous_security)
                finally:
                    try:
                        app.Quit()
                    finally:
                        _close_office_kill_job(kill_job)
    native_engine = {
        "xlsx": "microsoft-excel-com",
        "docx": "microsoft-word-com",
        "pptx": "microsoft-powerpoint-com",
    }[artifact_format]
    action = {
        "command": "windows-com-finalize",
        "status": "ok",
        "exitCode": 0,
        "engine": native_engine,
        "engineVersion": office_version,
        "nativeOpenSave": True,
        "macros": "force-disabled",
        "safeFieldsUpdated": safe_fields_updated if artifact_format == "docx" else None,
    }
    if artifact_format == "xlsx":
        action.update({
            "calculationProfile": "excel-native",
            "externalLinks": "update-disabled",
            "calculationState": calculation_state,
            "formulaFingerprintBefore": formula_before["fingerprint"],
            "formulaFingerprintAfter": formula_after["fingerprint"],
            "cacheEvidence": cache_evidence,
            "cachedErrors": cached_errors,
        })
    actions.append(action)


def _collect_powerpoint_export_images(
    raw_dir: Path,
    outdir: Path,
    expected_slides: int,
) -> list[Path]:
    def export_key(path: Path) -> tuple[int, str]:
        match = re.search(r"(\d+)$", path.stem)
        return (int(match.group(1)) if match else 2**31 - 1, path.name.casefold())

    images = sorted(
        (
            path for path in raw_dir.iterdir()
            if path.is_file() and path.suffix.lower() in {".png", ".jpg", ".jpeg"}
        ),
        key=export_key,
    )
    if len(images) != expected_slides:
        raise RuntimeError(
            f"PowerPoint exported {len(images)} slide images; expected {expected_slides}"
        )
    outdir.mkdir(parents=True, exist_ok=True)
    for old in outdir.glob("slide-*.png"):
        if old.is_file():
            old.unlink()
    outputs: list[Path] = []
    for index, source in enumerate(images, start=1):
        destination = outdir / f"slide-{index:03d}.png"
        shutil.copy2(source, destination)
        outputs.append(destination)
    return outputs


def _pdftoppm_command(arguments: list[str]) -> list[str]:
    executable = shutil.which("pdftoppm") or shutil.which("pdftoppm.exe")
    if not executable:
        raise RuntimeError("native DOCX/XLSX rendering requires Poppler pdftoppm")
    path = Path(executable)
    if os.name == "nt" and path.suffix.lower() in {".cmd", ".bat"}:
        candidates = [
            path.parent.parent / "Library" / "bin" / "pdftoppm.exe",
            path.parents[2] / "native" / "poppler" / "Library" / "bin" / "pdftoppm.exe",
        ]
        binary = next((candidate for candidate in candidates if candidate.is_file()), None)
        if binary is None:
            raise RuntimeError("refusing to execute pdftoppm through a shell wrapper")
        path = binary
    if path.suffix.lower() != ".exe" and os.name == "nt":
        raise RuntimeError("pdftoppm renderer must resolve to a native executable")
    return [str(path), *arguments]


def _render_pdf_pages(pdf: Path, outdir: Path, prefix: str) -> list[Path]:
    outdir.mkdir(parents=True, exist_ok=True)
    output_prefix = outdir / prefix
    completed = subprocess.run(
        _pdftoppm_command(["-png", "-r", "144", str(pdf), str(output_prefix)]),
        text=True,
        capture_output=True,
        check=False,
        timeout=180,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            completed.stderr.strip() or completed.stdout.strip() or "pdftoppm failed"
        )
    pages = sorted(outdir.glob(f"{prefix}-*.png"))
    if not pages:
        raise RuntimeError(f"PDF render produced no pages: {pdf}")
    return pages


def _windows_com_render_docx(
    path: Path,
    outdir: Path,
    actions: list[dict[str, Any]],
) -> list[Path]:
    _assert_native_network_closed(path, "docx")
    try:
        import win32com.client  # type: ignore
    except (ImportError, OSError) as error:
        raise RuntimeError(f"Microsoft Word COM is unavailable: {error}") from error
    existing_pids = _windows_process_ids("WINWORD.EXE")
    app = win32com.client.DispatchEx("Word.Application")
    kill_job = _guard_office_process(
        app, executable="WINWORD.EXE", existing_pids=existing_pids
    )
    app.Visible = False
    app.DisplayAlerts = 0
    app.Options.UpdateLinksAtOpen = False
    app.Options.UpdateFieldsAtPrint = False
    previous_security = _force_disable_macros(app)
    office_version = str(getattr(app, "Version", "unknown"))
    document = None
    try:
        document = app.Documents.Open(
            str(path.resolve()),
            ConfirmConversions=False,
            ReadOnly=True,
            AddToRecentFiles=False,
            OpenAndRepair=False,
            NoEncodingDialog=True,
        )
        with tempfile.TemporaryDirectory(prefix=".nexa-word-render-", dir=outdir.parent) as raw:
            pdf = Path(raw) / "document.pdf"
            document.ExportAsFixedFormat(
                OutputFileName=str(pdf),
                ExportFormat=17,
                OpenAfterExport=False,
                OptimizeFor=0,
                Range=0,
                Item=0,
                IncludeDocProps=True,
                KeepIRM=False,
                CreateBookmarks=0,
                DocStructureTags=True,
                BitmapMissingFonts=True,
                UseISO19005_1=False,
            )
            pages = _render_pdf_pages(pdf, outdir, "page")
    finally:
        try:
            if document is not None:
                document.Close(SaveChanges=False)
        finally:
            try:
                _restore_automation_security(app, previous_security)
            finally:
                try:
                    app.Quit()
                finally:
                    _close_office_kill_job(kill_job)
    actions.append({
        "command": "windows-com-render-docx",
        "status": "ok",
        "engine": "microsoft-word-com",
        "engineVersion": office_version,
        "pages": len(pages),
        "macros": "force-disabled",
        "externalLinks": "update-disabled",
    })
    return pages


def _xlsx_visible_surface_inventory(path: Path) -> list[dict[str, str]]:
    relationship_ns = "http://schemas.openxmlformats.org/package/2006/relationships"
    document_rel_ns = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
    with zipfile.ZipFile(path) as archive:
        workbook = ET.fromstring(archive.read("xl/workbook.xml"))
        relationships = ET.fromstring(archive.read("xl/_rels/workbook.xml.rels"))
    rel_types = {
        item.attrib.get("Id", ""): item.attrib.get("Type", "").rsplit("/", 1)[-1]
        for item in relationships.findall(f"{{{relationship_ns}}}Relationship")
    }
    inventory: list[dict[str, str]] = []
    for sheet in workbook.iter():
        if sheet.tag.rsplit("}", 1)[-1] != "sheet" or sheet.attrib.get("state", "visible") != "visible":
            continue
        relationship_id = sheet.attrib.get(f"{{{document_rel_ns}}}id", "")
        surface_type = rel_types.get(relationship_id, "")
        if surface_type not in {"worksheet", "chartsheet"}:
            raise RuntimeError(
                f"unsupported visible Excel native render surface: {sheet.attrib.get('name')} ({surface_type})"
            )
        inventory.append({
            "stableId": sheet.attrib.get("sheetId", ""),
            "name": sheet.attrib.get("name", ""),
            "type": surface_type,
        })
    if not inventory:
        raise RuntimeError("XLSX has no visible worksheet or chart-sheet surfaces")
    return inventory


def _windows_com_render_xlsx(
    path: Path,
    outdir: Path,
    actions: list[dict[str, Any]],
) -> list[Path]:
    _assert_native_network_closed(path, "xlsx")
    expected_inventory = _xlsx_visible_surface_inventory(path)
    try:
        import win32com.client  # type: ignore
    except (ImportError, OSError) as error:
        raise RuntimeError(f"Microsoft Excel COM is unavailable: {error}") from error
    existing_pids = _windows_process_ids("EXCEL.EXE")
    app = win32com.client.DispatchEx("Excel.Application")
    kill_job = _guard_office_process(
        app, executable="EXCEL.EXE", existing_pids=existing_pids
    )
    app.Visible = True
    app.WindowState = 2  # xlMinimized; screen rendering without a foreground window
    app.DisplayAlerts = False
    app.EnableEvents = False
    app.AskToUpdateLinks = False
    previous_security = _force_disable_macros(app)
    office_version = str(getattr(app, "Version", "unknown"))
    document = None
    outputs: list[Path] = []
    sheets: list[dict[str, Any]] = []
    try:
        document = app.Workbooks.Open(str(path.resolve()), UpdateLinks=0, ReadOnly=True)
        outdir.mkdir(parents=True, exist_ok=True)
        visible_index = 0
        for index in range(1, int(document.Sheets.Count) + 1):
            sheet = document.Sheets.Item(index)
            if int(sheet.Visible) != -1:  # xlSheetVisible
                continue
            visible_index += 1
            expected = next(
                item for item in expected_inventory if item["name"] == str(sheet.Name)
            )
            png = outdir / f"sheet-{visible_index:03d}.png"
            if expected["type"] == "chartsheet":
                sheet.Activate()
                time.sleep(0.2)
                active_chart = getattr(app, "ActiveChart", None)
                if active_chart is None:
                    raise RuntimeError(f"Excel did not activate chart sheet: {sheet.Name}")
                width = min(1600.0, max(640.0, float(active_chart.ChartArea.Width)))
                height = min(1200.0, max(360.0, float(active_chart.ChartArea.Height)))
                active_chart.CopyPicture(Appearance=1, Format=2)  # xlScreen, xlPicture
                try:
                    import pythoncom  # type: ignore

                    pythoncom.PumpWaitingMessages()
                except (ImportError, OSError):
                    pass
                time.sleep(0.2)
                scratch = None
                chart_object = None
                try:
                    scratch = app.Workbooks.Add()
                    scratch_sheet = scratch.Worksheets.Item(1)
                    chart_object = scratch_sheet.ChartObjects().Add(0, 0, width, height)
                    chart_object.Activate()
                    chart_object.Chart.Paste()
                    time.sleep(0.2)
                    exported = bool(chart_object.Chart.Export(str(png), "PNG"))
                finally:
                    if chart_object is not None:
                        chart_object.Delete()
                    if scratch is not None:
                        scratch.Close(SaveChanges=False)
                    app.CutCopyMode = False
            else:
                used_range = sheet.UsedRange
                width = min(1600.0, max(640.0, float(used_range.Width)))
                height = min(1200.0, max(360.0, float(used_range.Height)))
                sheet.Activate()
                used_range.Select()
                app.Goto(used_range, True)
                used_range.CopyPicture(Appearance=1, Format=2)  # xlScreen, xlPicture
                try:
                    import pythoncom  # type: ignore

                    pythoncom.PumpWaitingMessages()
                except (ImportError, OSError):
                    pass
                time.sleep(0.2)
                chart_object = sheet.ChartObjects().Add(0, 0, width, height)
                try:
                    chart_object.Activate()
                    chart_object.Chart.Paste()
                    time.sleep(0.2)
                    exported = bool(chart_object.Chart.Export(str(png), "PNG"))
                finally:
                    chart_object.Delete()
                    app.CutCopyMode = False
            if not exported or not png.is_file() or png.stat().st_size == 0:
                raise RuntimeError(f"Excel native image export failed for sheet: {sheet.Name}")
            outputs.append(png)
            sheets.append({
                "index": index,
                "name": str(sheet.Name),
                "stableId": expected["stableId"],
                "type": expected["type"],
                "files": [png.name],
            })
    finally:
        try:
            if document is not None:
                document.Close(SaveChanges=False)
        finally:
            try:
                _restore_automation_security(app, previous_security)
            finally:
                try:
                    app.Quit()
                finally:
                    _close_office_kill_job(kill_job)
    if not sheets or not outputs:
        raise RuntimeError("Excel native render produced no visible worksheet pages")
    if {item["name"] for item in sheets} != {item["name"] for item in expected_inventory}:
        raise RuntimeError("Excel native render surface inventory does not match workbook OOXML")
    manifest = {
        "kind": "xlsxRenderSurfaceManifest",
        "renderer": "microsoft-excel-native",
        "artifactSha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "expectedSheets": len(expected_inventory),
        "renderedSheets": len(sheets),
        "expectedSurfaces": len(outputs),
        "renderedSurfaces": len(outputs),
        "complete": True,
        "sheets": sheets,
    }
    (outdir / "render-manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    actions.append({
        "command": "windows-com-render-xlsx",
        "status": "ok",
        "engine": "microsoft-excel-com",
        "engineVersion": office_version,
        "sheets": len(sheets),
        "pages": len(outputs),
        "macros": "force-disabled",
        "externalLinks": "update-disabled",
    })
    return outputs


def _windows_com_render_pptx(
    path: Path,
    outdir: Path,
    actions: list[dict[str, Any]],
) -> list[Path]:
    _assert_native_network_closed(path, "pptx")
    try:
        import win32com.client  # type: ignore
    except (ImportError, OSError) as error:
        raise RuntimeError(f"Microsoft PowerPoint COM is unavailable: {error}") from error
    existing_pids = _windows_process_ids("POWERPNT.EXE")
    app = win32com.client.DispatchEx("PowerPoint.Application")
    kill_job = _guard_office_process(
        app, executable="POWERPNT.EXE", existing_pids=existing_pids
    )
    document = None
    previous_security = _force_disable_macros(app)
    office_version = str(getattr(app, "Version", "unknown"))
    try:
        document = app.Presentations.Open(str(path.resolve()), WithWindow=False)
        expected_slides = int(document.Slides.Count)
        with tempfile.TemporaryDirectory(
            prefix=".nexa-powerpoint-render-",
            dir=outdir.parent,
        ) as raw:
            export_dir = Path(raw) / "slides"
            document.Export(str(export_dir), "PNG", 1600, 900)
            outputs = _collect_powerpoint_export_images(
                export_dir, outdir, expected_slides
            )
    finally:
        try:
            if document is not None:
                document.Close()
        finally:
            try:
                _restore_automation_security(app, previous_security)
            finally:
                try:
                    app.Quit()
                finally:
                    _close_office_kill_job(kill_job)
    actions.append({
        "command": "windows-com-render-pptx",
        "status": "ok",
        "exitCode": 0,
        "engine": "microsoft-powerpoint-com",
        "engineVersion": office_version,
        "renderProfile": "powerpoint-native",
        "slides": len(outputs),
        "macros": "force-disabled",
    })
    return outputs


def _contract_path(
    job: OfficeExecutionPlan,
    temporary_root: Path,
    workspace_root: Path,
) -> Path | None:
    if job.validation_contract is None:
        return None
    if isinstance(job.validation_contract, str):
        return workspace_path(Path(job.validation_contract), workspace_root, must_exist=True)
    path = temporary_root / "workbook-contract.json"
    path.write_text(json.dumps(job.validation_contract, ensure_ascii=False, indent=2), encoding="utf-8")
    return path


def _rollback_job_outputs(
    output: Path,
    output_snapshot: Path | None,
    output_published: bool,
    auxiliaries: list[tuple[Path, Path | None]],
    workspace_root: Path,
) -> list[str]:
    errors: list[str] = []
    for path, snapshot in reversed(auxiliaries):
        try:
            rollback_published_artifact(path, snapshot, workspace_root)
        except Exception as error:  # noqa: BLE001
            errors.append(f"{path}: {type(error).__name__}: {error}")
    if output_published:
        try:
            rollback_published_artifact(output, output_snapshot, workspace_root)
        except Exception as error:  # noqa: BLE001
            errors.append(f"{output}: {type(error).__name__}: {error}")
    return errors


def execute_plan(job: OfficeExecutionPlan, workspace_root: Path) -> tuple[dict[str, Any], int]:
    backend = _select_backend(job)
    status = _backend_status(backend)
    result: dict[str, Any] = {
        "ok": False,
        "jobVersion": job.job_version,
        "format": job.format,
        "intent": job.intent,
        "backend": backend,
        "changedElements": [],
        "preservedFeatures": [],
        "preservationEvidence": None,
        "warnings": [],
        "validation": None,
        "renderedPreviews": [],
        "rollbackSnapshot": None,
        "rollbackApplied": False,
        "actions": [],
    }
    actions: list[dict[str, Any]] = result["actions"]
    working = staging_path(job.output)
    output_snapshot: Path | None = None
    output_published = False
    published_auxiliaries: list[tuple[Path, Path | None]] = []
    authorized_parts: set[str] = set()
    job.output.parent.mkdir(parents=True, exist_ok=True)
    if job.input is not None:
        input_validation = validate_ooxml_package(job.input)
        if input_validation.status == "fail":
            result["validation"] = {"source": input_validation.to_dict()}
            result["error"] = "source Office package failed safety/structure preflight"
            return result, 1
    input_risk = scan_ooxml_risks(job.input) if job.input is not None else None
    if input_risk:
        if input_risk["riskLevel"] == "high" and job.preservation_policy == "strict":
            result["warnings"].append(
                "High-risk package features detected; Nexa will use staged precise edits and validate before publish."
            )

    try:
        mutating_operations = [
            operation
            for operation in job.operations
            if str(operation.get("op", "")).lower() not in {"validate", "render"}
        ]
        if job.intent in {"recalculate", "finalize"}:
            mutating_operations.append({"op": job.intent})
        if (
            input_risk is not None
            and input_risk["features"].get("signatures")
            and job.preservation_policy == "strict"
            and mutating_operations
        ):
            raise RuntimeError(
                "strict preservation blocks edits to digitally signed Office packages because any package mutation invalidates the signature"
            )
        if backend != "nexa-openxml" and status["status"] != "ready":
            raise RuntimeError(status.get("detail") or f"backend {backend} is not ready")
        _assert_backend_support(job, backend)
        if job.input is not None:
            shutil.copy2(job.input, working)

        if backend == "officecli":
            _officecli_create(job, working, actions)
        elif job.intent == "create_new":
            if backend != "nexa-openxml":
                raise ValueError(f"backend {backend} cannot create {job.format} artifacts")
            _native_create(job, working, actions, workspace_root)
        elif backend == "windows-com":
            _windows_com_finalize(working, job.format, actions)
        else:
            result["changedElements"], authorized_parts = _native_operations(
                job, working, actions, workspace_root
            )

        needs_recalculation = job.intent in {"recalculate", "finalize"} or any(
            str(operation.get("op", "")).lower() == "recalculate"
            for operation in job.operations
        )
        if needs_recalculation and backend != "windows-com":
            if job.format != "xlsx":
                raise ValueError("recalculate currently supports XLSX only")
            arguments = ["--allow-risky"] if job.preservation_policy == "replace" else []
            before_recalculation = _all_part_hashes(working)
            _run_editor(working, "recalc_xlsx", arguments, actions, workspace_root, timeout=300)
            recalculated_parts = _changed_part_names(
                before_recalculation,
                _all_part_hashes(working),
            )
            allowed_recalculation = set(_authorized_part_patterns(job.format, "recalculate"))
            outside_recalculation = sorted(
                part for part in recalculated_parts
                if not _matches_any_part_pattern(part, allowed_recalculation)
            )
            if outside_recalculation and job.preservation_policy == "strict":
                raise RuntimeError(
                    "recalculation changed package parts outside its strict scope: "
                    + ", ".join(outside_recalculation)
                )
            authorized_parts.update(recalculated_parts - set(outside_recalculation))

        if job.input is not None and input_risk is not None:
            preservation = _preservation_evidence(
                job.input,
                working,
                input_risk,
                authorized_parts,
            )
            result["preservationEvidence"] = preservation
            result["preservedFeatures"] = preservation["verifiedFeatures"]
            if job.preservation_policy == "strict" and not preservation["verified"]:
                raise RuntimeError(
                    "strict preservation failed: "
                    + json.dumps({
                        "changedParts": preservation["changedParts"],
                        "missingParts": preservation["missingParts"],
                        "addedParts": preservation["addedParts"],
                        "unauthorizedParts": preservation["unauthorizedParts"],
                    }, ensure_ascii=False)
                )

        with tempfile.TemporaryDirectory(prefix=".nexa-office-job-", dir=job.output.parent) as tmp:
            temporary_root = Path(tmp)
            contract = _contract_path(job, temporary_root, workspace_root)
            if contract is not None and job.format == "xlsx":
                _run_editor(
                    working,
                    "lint_xlsx",
                    ["--contract", str(contract)],
                    actions,
                    workspace_root,
                )

            validation_arguments = ["--json"]
            if contract is not None:
                validation_arguments.extend(["--contract", str(contract)])
            validation_output = _run_editor(
                working,
                "validate",
                validation_arguments,
                actions,
                workspace_root,
            )
            result["validation"] = json.loads(validation_output)

            if job.render_policy != "none":
                render_dir = job.output.parent / f"{job.output.stem}-rendered"
                if backend == "windows-com":
                    native_renderer = {
                        "docx": _windows_com_render_docx,
                        "xlsx": _windows_com_render_xlsx,
                        "pptx": _windows_com_render_pptx,
                    }[job.format]
                    result["renderedPreviews"] = [
                        str(path) for path in native_renderer(working, render_dir, actions)
                    ]
                else:
                    render_arguments = ["--outdir", str(render_dir)]
                    if job.format == "xlsx":
                        render_arguments.extend([
                            "--sheets",
                            "all" if job.render_policy == "all" else "active",
                        ])
                    _run_editor(
                        working,
                        "render",
                        render_arguments,
                        actions,
                        workspace_root,
                        timeout=300,
                    )
                    result["renderedPreviews"] = [
                        str(path)
                        for path in sorted(render_dir.iterdir())
                        if path.is_file() and path.suffix.lower() in {".png", ".jpg", ".jpeg"}
                    ]

        final_validation = validate_ooxml_package(working)
        if final_validation.status == "fail":
            raise RuntimeError(json.dumps(final_validation.to_dict(), ensure_ascii=False))
        output_snapshot, publish_validation = publish_staged_artifact(
            working, job.output, workspace_root, validate=True
        )
        output_published = True
        result["rollbackSnapshot"] = str(output_snapshot) if output_snapshot else None
        backend_validation = result["validation"]
        result["validation"] = {
            "structural": publish_validation.to_dict() if publish_validation is not None else None,
            "backend": backend_validation,
        }

        staged_xlsx_qa = working.with_suffix(".xlsx.qa.json")
        if job.format == "xlsx" and staged_xlsx_qa.exists():
            final_xlsx_qa = job.output.with_suffix(".xlsx.qa.json")
            qa_payload = json.loads(staged_xlsx_qa.read_text(encoding="utf-8"))
            qa_payload["path"] = str(job.output)
            qa_payload["qaPath"] = str(final_xlsx_qa)
            auxiliary_snapshot = snapshot_file(final_xlsx_qa, workspace_root)
            write_artifact_manifest(final_xlsx_qa, qa_payload, workspace_root)
            published_auxiliaries.append((final_xlsx_qa, auxiliary_snapshot))
            staged_xlsx_qa.unlink(missing_ok=True)

        if job.format == "pptx" and job.intent == "create_new" and job.operations:
            operation = job.operations[0]
            if operation.get("htmlFirst") and operation.get("outdir"):
                project_manifest = workspace_path(
                    Path(str(operation["outdir"])) / "manifest.json",
                    workspace_root,
                    must_exist=True,
                )
                project_payload = json.loads(project_manifest.read_text(encoding="utf-8"))
                if isinstance(project_payload.get("pptx"), dict):
                    project_payload["pptx"]["path"] = str(job.output)
                    auxiliary_snapshot = snapshot_file(project_manifest, workspace_root)
                    write_artifact_manifest(project_manifest, project_payload, workspace_root)
                    published_auxiliaries.append((project_manifest, auxiliary_snapshot))

        result["ok"] = True
        exit_code = 0
    except Exception as error:  # noqa: BLE001
        working.unlink(missing_ok=True)
        working.with_suffix(".xlsx.qa.json").unlink(missing_ok=True)
        rollback_errors = _rollback_job_outputs(
            job.output,
            output_snapshot,
            output_published,
            published_auxiliaries,
            workspace_root,
        )
        result["rollbackApplied"] = output_published and not rollback_errors
        if rollback_errors:
            result["warnings"].append("Rollback errors: " + "; ".join(rollback_errors))
        result["error"] = f"{type(error).__name__}: {error}"
        exit_code = 1

    result = _replace_path_strings(result, working, job.output)
    result["manifestPath"] = str(job.manifest)
    try:
        write_artifact_manifest(job.manifest, result, workspace_root)
    except Exception as error:  # noqa: BLE001
        if exit_code == 0:
            rollback_errors = _rollback_job_outputs(
                job.output,
                output_snapshot,
                output_published,
                published_auxiliaries,
                workspace_root,
            )
            result["ok"] = False
            result["rollbackApplied"] = not rollback_errors
            result["error"] = f"Artifact manifest publish failed: {type(error).__name__}: {error}"
            if rollback_errors:
                result["warnings"].append("Rollback errors: " + "; ".join(rollback_errors))
            exit_code = 1
        else:
            result["warnings"].append(
                f"Artifact failure manifest could not be written: {type(error).__name__}: {error}"
            )
    return result, exit_code


execute_job = execute_plan


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Run a transactional Office artifact job")
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--job", help="Absolute job JSON path, or '-' for stdin")
    group.add_argument("--preflight", action="store_true", help="Report available Office backends")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.preflight:
        print(json.dumps({"jobVersion": 1, "backends": office_backend_statuses()}, ensure_ascii=False, indent=2))
        return 0
    try:
        payload = _load_job(args.job)
        job = OfficeExecutionPlan.from_legacy_job(payload, Path.cwd())
        result, exit_code = execute_plan(job, Path.cwd())
        print(json.dumps(result, ensure_ascii=False, indent=2))
        return exit_code
    except Exception as error:  # noqa: BLE001
        print(json.dumps({"ok": False, "error": f"{type(error).__name__}: {error}"}, ensure_ascii=False, indent=2))
        return 1


if __name__ == "__main__":
    sys.exit(main())
