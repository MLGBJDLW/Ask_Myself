#!/usr/bin/env python3
"""Transactional Job/Result protocol for DOCX, PPTX, and XLSX artifacts."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import zipfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any

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
INTENTS = {"create_new", "edit_existing", "validate", "recalculate", "finalize"}
PRESERVATION_POLICIES = {"strict", "balanced", "replace"}
RENDER_POLICIES = {"none", "important_surfaces", "all"}
BACKENDS = {"auto", "nexa-openxml", "libreoffice", "officecli", "windows-com"}


@dataclass
class OfficeArtifactJob:
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
    def from_dict(cls, payload: dict[str, Any], workspace_root: Path) -> OfficeArtifactJob:
        job_version = int(payload.get("jobVersion", 1))
        if job_version != 1:
            raise ValueError(f"unsupported jobVersion: {job_version}")
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
        if output.suffix.lower() != f".{artifact_format}":
            raise ValueError(f"output suffix must be .{artifact_format}")
        if input_path is not None and input_path.suffix.lower() != f".{artifact_format}":
            raise ValueError(f"input suffix must be .{artifact_format}")

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
        return cls(
            job_version=job_version,
            format=artifact_format,
            intent=intent,
            input=input_path,
            output=output,
            operations=operations,
            preservation_policy=preservation,
            validation_contract=payload.get("validationContract"),
            render_policy=render_policy,
            backend=backend,
            allow_network_backend=bool(payload.get("allowNetworkBackend", False)),
            manifest=manifest,
        )


def _load_job(raw: str) -> dict[str, Any]:
    if raw == "-":
        payload = json.load(sys.stdin)
    else:
        path = workspace_path(Path(raw), Path.cwd(), must_exist=True)
        payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise TypeError("job root must be an object")
    return payload


def _run(command: list[str], *, timeout: int = 180) -> subprocess.CompletedProcess[str]:
    kwargs: dict[str, Any] = {
        "text": True,
        "capture_output": True,
        "check": False,
        "timeout": timeout,
        "env": {**os.environ, "NEXA_OFFICE_SKIP_SNAPSHOT": "1"},
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
    *,
    timeout: int = 180,
) -> str:
    completed = _run(_editor_command(path, command, *arguments), timeout=timeout)
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


def _select_backend(job: OfficeArtifactJob) -> str:
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


def _assert_backend_support(job: OfficeArtifactJob, backend: str) -> None:
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
    job: OfficeArtifactJob,
    working: Path,
    actions: list[dict[str, Any]],
    workspace_root: Path,
) -> None:
    operation = job.operations[0] if job.operations else {}
    if job.format == "docx":
        arguments: list[str] = []
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
        _run_editor(working, "create_docx", arguments, actions)
    elif job.format == "xlsx":
        spec = operation.get("spec")
        if not spec:
            raise ValueError("create_new XLSX requires operations[0].spec")
        spec_path = _operation_path(spec, workspace_root, must_exist=True)
        _run_editor(working, "create_xlsx", ["--spec", str(spec_path)], actions)
    else:
        spec = operation.get("spec")
        if not spec:
            raise ValueError("create_new PPTX requires operations[0].spec")
        if operation.get("htmlFirst"):
            outdir = operation.get("outdir")
            if not outdir:
                raise ValueError("HTML-first PPTX requires outdir")
            spec_path = _operation_path(spec, workspace_root, must_exist=True)
            outdir_path = _operation_path(outdir, workspace_root, must_exist=False)
            arguments = ["--spec", str(spec_path), "--outdir", str(outdir_path)]
            arguments.extend(["--mode", str(operation.get("mode", "hybrid"))])
            arguments.extend(["--screenshot", str(operation.get("screenshot", "auto"))])
            _run_editor(working, "create_html_pptx", arguments, actions, timeout=300)
        else:
            spec_path = _operation_path(spec, workspace_root, must_exist=True)
            arguments = ["--spec", str(spec_path)]
            if operation.get("template"):
                template_path = _operation_path(
                    operation["template"], workspace_root, must_exist=True
                )
                arguments.extend(["--template", str(template_path)])
            _run_editor(working, "create_pptx", arguments, actions)


def _native_operations(job: OfficeArtifactJob, working: Path, actions: list[dict[str, Any]]) -> list[str]:
    changed: list[str] = []
    for index, operation in enumerate(job.operations):
        name = str(operation.get("op", "")).lower()
        element_id = str(operation.get("elementId") or f"/{job.format}/operation[{index}]")
        if name in {"replace", "redact"}:
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
            _run_editor(working, name, arguments, actions)
            changed.append(element_id)
        elif name == "insert_slide":
            if job.format != "pptx":
                raise ValueError("insert_slide only supports PPTX")
            arguments = [
                "--after", str(operation.get("after", 0)),
                "--title", str(operation.get("title", "")),
                "--body", str(operation.get("body", "")),
            ]
            _run_editor(working, name, arguments, actions)
            changed.append(element_id)
        elif name in {"validate", "render", "recalculate"}:
            continue
        else:
            raise ValueError(f"unsupported operation: {name or '<missing>'}")
    return changed


def _officecli_create(job: OfficeArtifactJob, working: Path, actions: list[dict[str, Any]]) -> None:
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


def _preservation_evidence(
    source: Path,
    candidate: Path,
    risk: dict[str, Any],
) -> dict[str, Any]:
    before = _sensitive_part_hashes(source, risk)
    after = _sensitive_part_hashes(candidate, risk)
    missing = sorted(set(before) - set(after))
    changed = sorted(
        name for name, digest in before.items()
        if name in after and after[name] != digest
    )
    unchanged = sorted(
        name for name, digest in before.items()
        if after.get(name) == digest
    )
    verified_features = sorted(
        feature
        for feature, names in risk.get("features", {}).items()
        if names and all(str(name) in unchanged for name in names)
    )
    return {
        "verified": not missing and not changed,
        "method": "sha256-sensitive-package-parts",
        "sourceParts": len(before),
        "unchangedParts": unchanged,
        "changedParts": changed,
        "missingParts": missing,
        "verifiedFeatures": verified_features,
    }


def _windows_com_finalize(path: Path, artifact_format: str, actions: list[dict[str, Any]]) -> None:
    try:
        import win32com.client  # type: ignore
    except (ImportError, OSError) as error:
        raise RuntimeError(f"Microsoft Office COM is unavailable: {error}") from error

    if artifact_format == "xlsx":
        app = win32com.client.DispatchEx("Excel.Application")
        app.Visible = False
        app.DisplayAlerts = False
        document = None
        previous_security = _force_disable_macros(app)
        try:
            document = app.Workbooks.Open(str(path.resolve()), UpdateLinks=0, ReadOnly=False)
            app.CalculateFullRebuild()
            document.Save()
        finally:
            try:
                if document is not None:
                    document.Close(SaveChanges=True)
            finally:
                try:
                    _restore_automation_security(app, previous_security)
                finally:
                    app.Quit()
    elif artifact_format == "docx":
        app = win32com.client.DispatchEx("Word.Application")
        app.Visible = False
        app.DisplayAlerts = 0
        document = None
        previous_security = _force_disable_macros(app)
        try:
            document = app.Documents.Open(str(path.resolve()), ReadOnly=False, AddToRecentFiles=False)
            document.Fields.Update()
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
                    app.Quit()
    else:
        app = win32com.client.DispatchEx("PowerPoint.Application")
        document = None
        previous_security = _force_disable_macros(app)
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
                    app.Quit()
    actions.append({"command": "windows-com-finalize", "status": "ok", "exitCode": 0})


def _contract_path(
    job: OfficeArtifactJob,
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


def execute_job(job: OfficeArtifactJob, workspace_root: Path) -> tuple[dict[str, Any], int]:
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
    job.output.parent.mkdir(parents=True, exist_ok=True)
    input_risk = scan_ooxml_risks(job.input) if job.input is not None else None
    if input_risk:
        if input_risk["riskLevel"] == "high" and job.preservation_policy == "strict":
            result["warnings"].append(
                "High-risk package features detected; Nexa will use staged precise edits and validate before publish."
            )

    try:
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
            result["changedElements"] = _native_operations(job, working, actions)

        needs_recalculation = job.intent in {"recalculate", "finalize"} or any(
            str(operation.get("op", "")).lower() == "recalculate"
            for operation in job.operations
        )
        if needs_recalculation and backend != "windows-com":
            if job.format != "xlsx":
                raise ValueError("recalculate currently supports XLSX only")
            arguments = ["--allow-risky"] if job.preservation_policy == "replace" else []
            _run_editor(working, "recalc_xlsx", arguments, actions, timeout=300)

        if job.input is not None and input_risk is not None:
            preservation = _preservation_evidence(job.input, working, input_risk)
            result["preservationEvidence"] = preservation
            result["preservedFeatures"] = preservation["verifiedFeatures"]
            if job.preservation_policy == "strict" and not preservation["verified"]:
                raise RuntimeError(
                    "strict preservation failed: "
                    + json.dumps({
                        "changedParts": preservation["changedParts"],
                        "missingParts": preservation["missingParts"],
                    }, ensure_ascii=False)
                )

        with tempfile.TemporaryDirectory(prefix=".nexa-office-job-", dir=job.output.parent) as tmp:
            temporary_root = Path(tmp)
            contract = _contract_path(job, temporary_root, workspace_root)
            if contract is not None and job.format == "xlsx":
                _run_editor(working, "lint_xlsx", ["--contract", str(contract)], actions)

            validation_arguments = ["--json"]
            if contract is not None:
                validation_arguments.extend(["--contract", str(contract)])
            validation_output = _run_editor(working, "validate", validation_arguments, actions)
            result["validation"] = json.loads(validation_output)

            if job.render_policy != "none":
                render_dir = job.output.parent / f"{job.output.stem}-rendered"
                _run_editor(working, "render", ["--outdir", str(render_dir)], actions, timeout=300)
                result["renderedPreviews"] = [
                    str(path) for path in sorted(render_dir.glob("page*")) if path.is_file()
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
        job = OfficeArtifactJob.from_dict(payload, Path.cwd())
        result, exit_code = execute_job(job, Path.cwd())
        print(json.dumps(result, ensure_ascii=False, indent=2))
        return exit_code
    except Exception as error:  # noqa: BLE001
        print(json.dumps({"ok": False, "error": f"{type(error).__name__}: {error}"}, ensure_ascii=False, indent=2))
        return 1


if __name__ == "__main__":
    sys.exit(main())
