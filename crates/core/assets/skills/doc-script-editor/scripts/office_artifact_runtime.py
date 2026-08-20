"""Shared Office artifact validation, transaction, risk, and backend primitives.

This module intentionally stays dependency-light.  It validates the OPC/OOXML
package before a staged artifact is published and exposes optional backends via
capability preflight without silently enabling network or native automation.
"""

from __future__ import annotations

import hashlib
import importlib.metadata
import json
import os
import platform
import posixpath
import re
import shutil
import stat
import subprocess
import uuid
import zipfile
from abc import ABC, abstractmethod
from dataclasses import asdict, dataclass, field
from pathlib import Path, PurePosixPath
from typing import Any
from urllib.parse import unquote, urlsplit
from xml.etree import ElementTree as ET

CONTENT_TYPES_NS = "http://schemas.openxmlformats.org/package/2006/content-types"
RELATIONSHIPS_NS = "http://schemas.openxmlformats.org/package/2006/relationships"
MAIN_PARTS = {
    ".docx": "word/document.xml",
    ".docm": "word/document.xml",
    ".dotx": "word/document.xml",
    ".dotm": "word/document.xml",
    ".pptx": "ppt/presentation.xml",
    ".pptm": "ppt/presentation.xml",
    ".potx": "ppt/presentation.xml",
    ".potm": "ppt/presentation.xml",
    ".xlsx": "xl/workbook.xml",
    ".xlsm": "xl/workbook.xml",
    ".xltx": "xl/workbook.xml",
    ".xltm": "xl/workbook.xml",
}
OFFICE_SUFFIXES = set(MAIN_PARTS)
MAX_PACKAGE_PARTS = 20_000
MAX_PACKAGE_UNCOMPRESSED_BYTES = 2 * 1024 * 1024 * 1024
MAX_PART_UNCOMPRESSED_BYTES = 512 * 1024 * 1024
MAX_COMPRESSION_RATIO = 500.0
MAX_XML_PART_BYTES = 32 * 1024 * 1024
MAX_XML_TOTAL_BYTES = 256 * 1024 * 1024
PINNED_PYTHON_DEPENDENCIES = {
    "python-docx": "1.2.0",
    "python-pptx": "1.0.2",
    "pypdf": "6.10.0",
    "openpyxl": "3.1.5",
}
FORMAT_PYTHON_DEPENDENCIES = {
    "docx": {"python-docx"},
    "pptx": {"python-pptx"},
    "xlsx": {"openpyxl"},
}


def office_python_dependency_statuses(
    artifact_format: str | None = None,
    needs: set[str] | None = None,
) -> list[dict[str, Any]]:
    selected = set(PINNED_PYTHON_DEPENDENCIES)
    if artifact_format is not None:
        selected = set(FORMAT_PYTHON_DEPENDENCIES.get(artifact_format, set()))
    if needs and "pdf" in needs:
        selected.add("pypdf")
    statuses = []
    for distribution, expected in PINNED_PYTHON_DEPENDENCIES.items():
        if distribution not in selected:
            continue
        try:
            actual = importlib.metadata.version(distribution)
            status = "ready" if actual == expected else "version-mismatch"
        except importlib.metadata.PackageNotFoundError:
            actual = None
            status = "missing"
        statuses.append({
            "id": distribution,
            "expectedVersion": expected,
            "actualVersion": actual,
            "status": status,
        })
    return statuses


@dataclass
class ValidationIssue:
    code: str
    message: str
    part: str | None = None


@dataclass
class ValidationReport:
    status: str = "pass"
    errors: list[ValidationIssue] = field(default_factory=list)
    warnings: list[ValidationIssue] = field(default_factory=list)
    checks: dict[str, Any] = field(default_factory=dict)

    def error(self, code: str, message: str, part: str | None = None) -> None:
        self.errors.append(ValidationIssue(code, message, part))
        self.status = "fail"

    def warning(self, code: str, message: str, part: str | None = None) -> None:
        self.warnings.append(ValidationIssue(code, message, part))
        if self.status == "pass":
            self.status = "warn"

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass
class BackendStatus:
    id: str
    label: str
    status: str
    capabilities: list[str]
    local: bool
    detail: str | None = None
    version: str | None = None
    path: str | None = None
    requires_explicit_network_consent: bool = False

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


class OfficeBackend(ABC):
    """Capability contract shared by local and explicitly selected backends."""

    id: str

    @abstractmethod
    def preflight(self) -> BackendStatus:
        raise NotImplementedError


class NexaOpenXmlBackend(OfficeBackend):
    id = "nexa-openxml"

    def preflight(self) -> BackendStatus:
        return BackendStatus(
            id=self.id,
            label="Nexa OpenXML",
            status="ready",
            capabilities=["create", "edit", "inspect", "validate", "transaction"],
            local=True,
            detail="Request readiness is evaluated per format and required operation.",
        )


def find_soffice() -> str | None:
    candidates = ["soffice", "libreoffice"]
    if os.name == "nt":
        candidates.extend(
            [
                r"C:\Program Files\LibreOffice\program\soffice.exe",
                r"C:\Program Files (x86)\LibreOffice\program\soffice.exe",
            ]
        )
    for candidate in candidates:
        found = shutil.which(candidate)
        if found:
            return found
        path = Path(candidate)
        if path.is_file():
            return str(path)
    return None


def _command_version(program: str, args: list[str]) -> str | None:
    try:
        completed = subprocess.run(
            [program, *args],
            text=True,
            capture_output=True,
            check=False,
            timeout=5,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if completed.returncode != 0:
        return None
    text = (completed.stdout or completed.stderr).strip()
    return text.splitlines()[0] if text else None


class LibreOfficeBackend(OfficeBackend):
    id = "libreoffice"

    def preflight(self) -> BackendStatus:
        executable = find_soffice()
        if not executable:
            return BackendStatus(
                id=self.id,
                label="LibreOffice",
                status="missing",
                capabilities=["recalculate", "convert", "render"],
                local=True,
                detail="Install LibreOffice and expose soffice on PATH.",
            )
        return BackendStatus(
            id=self.id,
            label="LibreOffice",
            status="ready",
            capabilities=["recalculate", "convert", "render"],
            local=True,
            path=executable,
            version=_command_version(executable, ["--version"]),
        )


class OfficeCliBackend(OfficeBackend):
    id = "officecli"

    def preflight(self) -> BackendStatus:
        executable = shutil.which("officecli")
        if not executable:
            return BackendStatus(
                id=self.id,
                label="OfficeCLI",
                status="missing",
                capabilities=["create"],
                local=False,
                detail="Optional binary is not installed. Nexa never installs or selects it implicitly.",
                requires_explicit_network_consent=True,
            )
        return BackendStatus(
            id=self.id,
            label="OfficeCLI",
            status="ready",
            capabilities=["create"],
            local=False,
            path=executable,
            version=_command_version(executable, ["--version"]),
            detail="Hosted mode may transmit prompts or files; explicit job consent is required.",
            requires_explicit_network_consent=True,
        )


class WindowsComBackend(OfficeBackend):
    id = "windows-com"

    def preflight(self) -> BackendStatus:
        if platform.system() != "Windows":
            return BackendStatus(
                id=self.id,
                label="Microsoft Office COM",
                status="unsupported",
                capabilities=["finalize", "recalculate", "render"],
                local=True,
                detail="Windows-only optional finalizer.",
            )
        try:
            import win32com.client  # type: ignore  # noqa: F401
        except (ImportError, OSError) as error:
            return BackendStatus(
                id=self.id,
                label="Microsoft Office COM",
                status="missing",
                capabilities=["finalize", "recalculate", "render"],
                local=True,
                detail=f"pywin32 or Microsoft Office is unavailable: {error}",
            )
        return BackendStatus(
            id=self.id,
            label="Microsoft Office COM",
            status="ready",
            capabilities=["finalize", "recalculate", "render"],
            local=True,
            detail="Optional native finalizer; never used without an explicit job backend.",
        )


def office_backend_statuses() -> list[dict[str, Any]]:
    return [
        backend.preflight().to_dict()
        for backend in (
            NexaOpenXmlBackend(),
            LibreOfficeBackend(),
            OfficeCliBackend(),
            WindowsComBackend(),
        )
    ]


def _relationship_source_part(rels_name: str) -> str | None:
    if rels_name == "_rels/.rels":
        return None
    marker = "/_rels/"
    if marker not in rels_name or not rels_name.endswith(".rels"):
        return None
    prefix, leaf = rels_name.split(marker, 1)
    return f"{prefix}/{leaf[:-5]}"


def _relationship_target(source_part: str | None, target: str) -> str | None:
    parsed = urlsplit(target)
    raw_path = unquote(parsed.path).replace("\\", "/")
    if not raw_path:
        return None
    if raw_path.startswith("/"):
        normalized = posixpath.normpath(raw_path.lstrip("/"))
    else:
        base = posixpath.dirname(source_part) if source_part else ""
        normalized = posixpath.normpath(posixpath.join(base, raw_path))
    if normalized == ".." or normalized.startswith("../"):
        return None
    return str(PurePosixPath(normalized))


def _windows_package_path_key(name: str) -> tuple[str | None, str | None]:
    reserved = {"con", "prn", "aux", "nul"} | {
        f"{prefix}{number}" for prefix in ("com", "lpt") for number in range(1, 10)
    }
    segments: list[str] = []
    for segment in name.split("/"):
        if not segment:
            return None, "empty Windows path segment"
        if segment.endswith((".", " ")):
            return None, "Windows path segment ends in dot or space"
        if any(character in segment for character in '<>:"|?*'):
            return None, "Windows path segment contains a reserved character"
        stem = segment.split(".", 1)[0].casefold()
        if stem in reserved:
            return None, "Windows path segment uses a reserved device name"
        segments.append(segment.casefold())
    return "/".join(segments), None


def validate_ooxml_package(path: Path) -> ValidationReport:
    """Validate ZIP integrity, XML parseability, content types, and rel targets."""

    report = ValidationReport()
    suffix = path.suffix.lower()
    if suffix not in OFFICE_SUFFIXES:
        report.error("format.unsupported", f"Unsupported Office package suffix: {suffix}")
        return report
    if not path.is_file():
        report.error("file.missing", f"Artifact does not exist: {path}")
        return report
    if not zipfile.is_zipfile(path):
        report.error("zip.invalid", "Artifact is not a valid ZIP package")
        return report

    try:
        with zipfile.ZipFile(path) as archive:
            infos = [info for info in archive.infolist() if not info.is_dir()]
            member_names = [info.filename for info in infos]
            names = set(member_names)
            windows_path_keys: dict[str, str] = {}
            if len(infos) > MAX_PACKAGE_PARTS:
                report.error(
                    "zip.part_budget",
                    f"Package has {len(infos)} parts; maximum is {MAX_PACKAGE_PARTS}",
                )
            total_uncompressed = sum(info.file_size for info in infos)
            if total_uncompressed > MAX_PACKAGE_UNCOMPRESSED_BYTES:
                report.error(
                    "zip.uncompressed_budget",
                    "Package uncompressed size exceeds safety budget",
                )
            for info in infos:
                name = info.filename
                normalized = name.replace("\\", "/")
                pure = PurePosixPath(normalized)
                if normalized != name or pure.is_absolute() or ".." in pure.parts:
                    report.error(
                        "zip.unsafe_path",
                        f"ZIP member path is unsafe: {name}",
                        name,
                    )
                windows_key, windows_error = _windows_package_path_key(normalized)
                if windows_error:
                    report.error("zip.windows_path", windows_error, name)
                elif windows_key in windows_path_keys:
                    report.error(
                        "zip.windows_path_collision",
                        f"ZIP member collides on Windows with {windows_path_keys[windows_key]}",
                        name,
                    )
                elif windows_key is not None:
                    windows_path_keys[windows_key] = name
                if info.flag_bits & 0x1:
                    report.error("zip.encrypted_part", "Encrypted ZIP parts are unsupported", name)
                unix_mode = (info.external_attr >> 16) & 0o170000
                if unix_mode == stat.S_IFLNK:
                    report.error("zip.symlink", "Symbolic-link ZIP members are unsupported", name)
                if info.file_size > MAX_PART_UNCOMPRESSED_BYTES:
                    report.error(
                        "zip.part_size_budget",
                        "ZIP member exceeds uncompressed part-size budget",
                        name,
                    )
                if name.endswith((".xml", ".rels")) and info.file_size > MAX_XML_PART_BYTES:
                    report.error(
                        "xml.part_budget",
                        "XML part exceeds the parser safety budget",
                        name,
                    )
                ratio = (
                    float("inf")
                    if info.file_size and info.compress_size == 0
                    else info.file_size / max(1, info.compress_size)
                )
                if ratio > MAX_COMPRESSION_RATIO:
                    report.error(
                        "zip.compression_ratio",
                        f"ZIP member compression ratio {ratio:.1f} exceeds {MAX_COMPRESSION_RATIO:.0f}",
                        name,
                    )
            report.checks["uncompressedBytes"] = total_uncompressed
            total_xml_bytes = sum(
                info.file_size for info in infos if info.filename.endswith((".xml", ".rels"))
            )
            report.checks["xmlBytes"] = total_xml_bytes
            if total_xml_bytes > MAX_XML_TOTAL_BYTES:
                report.error("xml.total_budget", "Total XML size exceeds the parser safety budget")
            report.checks["maxCompressionRatio"] = max(
                (
                    info.file_size / max(1, info.compress_size)
                    for info in infos
                ),
                default=0.0,
            )
            if len(names) != len(member_names):
                duplicates = sorted(
                    name for name in names if member_names.count(name) > 1
                )
                for name in duplicates:
                    report.error(
                        "package.duplicate_part",
                        f"Package contains a duplicate part name: {name}",
                        name,
                    )
            report.checks["parts"] = len(names)
            if report.status == "fail":
                return report
            bad_member = archive.testzip()
            if bad_member:
                report.error("zip.crc", "ZIP member failed its CRC check", bad_member)

            required = {"[Content_Types].xml", "_rels/.rels", MAIN_PARTS[suffix]}
            for name in sorted(required - names):
                report.error("package.required_part", f"Required package part is missing: {name}", name)

            parsed_xml: dict[str, ET.Element] = {}
            for name in sorted(names):
                if not name.endswith((".xml", ".rels")):
                    continue
                try:
                    xml_bytes = archive.read(name)
                    lowered = xml_bytes.lower()
                    if b"<!doctype" in lowered or b"<!entity" in lowered:
                        report.error(
                            "xml.dtd_forbidden",
                            "DTD and entity declarations are forbidden in Office package XML",
                            name,
                        )
                        continue
                    parsed_xml[name] = ET.fromstring(xml_bytes)
                except (ET.ParseError, KeyError) as error:
                    report.error("xml.parse", f"XML part cannot be parsed: {error}", name)
            report.checks["xmlParts"] = len(parsed_xml)

            defaults: set[str] = set()
            overrides: set[str] = set()
            content_root = parsed_xml.get("[Content_Types].xml")
            if content_root is not None:
                for child in content_root:
                    local = child.tag.rsplit("}", 1)[-1]
                    if local == "Default" and child.attrib.get("Extension"):
                        defaults.add(child.attrib["Extension"].lower())
                    elif local == "Override" and child.attrib.get("PartName"):
                        overrides.add(child.attrib["PartName"].lstrip("/"))
                for name in sorted(names):
                    if name == "[Content_Types].xml" or name.endswith(".rels"):
                        continue
                    extension = PurePosixPath(name).suffix.lower().lstrip(".")
                    if name not in overrides and extension not in defaults:
                        report.error(
                            "content_types.missing",
                            "Package part has no Default or Override content type",
                            name,
                        )
                report.checks["contentTypeDefaults"] = len(defaults)
                report.checks["contentTypeOverrides"] = len(overrides)

            relationship_count = 0
            external_count = 0
            for rels_name, root in parsed_xml.items():
                if not rels_name.endswith(".rels"):
                    continue
                source_part = _relationship_source_part(rels_name)
                seen_ids: set[str] = set()
                for relationship in root:
                    if relationship.tag.rsplit("}", 1)[-1] != "Relationship":
                        continue
                    relationship_count += 1
                    rel_id = relationship.attrib.get("Id", "")
                    if not rel_id:
                        report.error("relationship.id", "Relationship is missing Id", rels_name)
                    elif rel_id in seen_ids:
                        report.error(
                            "relationship.duplicate_id",
                            f"Duplicate relationship Id: {rel_id}",
                            rels_name,
                        )
                    seen_ids.add(rel_id)

                    if relationship.attrib.get("TargetMode", "").lower() == "external":
                        external_count += 1
                        continue
                    target = relationship.attrib.get("Target", "")
                    resolved = _relationship_target(source_part, target)
                    if resolved is None:
                        report.error(
                            "relationship.target",
                            f"Relationship target is empty or escapes the package: {target!r}",
                            rels_name,
                        )
                    elif resolved not in names:
                        report.error(
                            "relationship.missing_target",
                            f"Relationship target does not exist: {resolved}",
                            rels_name,
                        )

            report.checks["relationships"] = relationship_count
            report.checks["externalRelationships"] = external_count
    except (OSError, zipfile.BadZipFile) as error:
        report.error("zip.open", f"Office package cannot be opened: {error}")

    return report


def scan_ooxml_risks(path: Path) -> dict[str, Any]:
    """Inventory preservation-sensitive package features before broad edits."""

    features: dict[str, list[str]] = {
        "macros": [],
        "signatures": [],
        "externalLinks": [],
        "connections": [],
        "pivotCaches": [],
        "slicers": [],
        "dataModel": [],
        "embeddedObjects": [],
        "xlmMacros": [],
        "externalFormulaFunctions": [],
        "unsafeExternalRelationships": [],
    }
    if not zipfile.is_zipfile(path):
        return {"riskLevel": "invalid", "features": features, "sensitiveParts": 0}

    external_relationship_details: list[dict[str, str]] = []
    external_formula_details: list[dict[str, Any]] = []
    with zipfile.ZipFile(path) as archive:
        for info in archive.infolist():
            name = info.filename
            lowered = name.lower()
            if "vbaproject" in lowered or lowered.endswith("vbadata.xml"):
                features["macros"].append(name)
            if "_xmlsignatures/" in lowered or "signature" in lowered:
                features["signatures"].append(name)
            if "externallinks/" in lowered:
                features["externalLinks"].append(name)
            if lowered.endswith("connections.xml") or "/connections/" in lowered:
                features["connections"].append(name)
            if "pivotcache" in lowered:
                features["pivotCaches"].append(name)
            if "slicer" in lowered or "timeline" in lowered:
                features["slicers"].append(name)
            if "model/" in lowered or lowered.endswith("item.data"):
                features["dataModel"].append(name)
            if "/embeddings/" in lowered or "/oleobjects/" in lowered:
                features["embeddedObjects"].append(name)
            if "xl/macrosheets/" in lowered or "xl/intlmacrosheets/" in lowered:
                features["xlmMacros"].append(name)
            if (
                lowered.startswith("xl/worksheets/")
                and lowered.endswith(".xml")
                and info.file_size <= MAX_XML_PART_BYTES
            ):
                try:
                    root = ET.fromstring(archive.read(name))
                except ET.ParseError:
                    continue
                functions: set[str] = set()
                for element in root.iter():
                    if element.tag.rsplit("}", 1)[-1] != "f":
                        continue
                    formula = "".join(element.itertext())
                    normalized = re.sub(r"\s+", "", formula).upper()
                    functions.update(
                        match.group(1)
                        for match in re.finditer(
                            r"(?:_XLFN\.)?(WEBSERVICE|RTD|DDE|CALL|EXEC|REGISTER\.ID)\(",
                            normalized,
                        )
                    )
                    if "|" in formula and "!" in formula:
                        functions.add("DDE_PIPE")
                if functions:
                    features["externalFormulaFunctions"].append(name)
                    external_formula_details.append({
                        "part": name,
                        "functions": sorted(functions),
                    })
            if lowered.endswith(".rels") and info.file_size <= MAX_XML_PART_BYTES:
                try:
                    relationships = ET.fromstring(archive.read(name))
                except ET.ParseError:
                    continue
                for relationship in relationships:
                    if (
                        relationship.tag.rsplit("}", 1)[-1] != "Relationship"
                        or relationship.attrib.get("TargetMode", "").lower() != "external"
                    ):
                        continue
                    relation_type = relationship.attrib.get("Type", "")
                    detail = {
                        "part": name,
                        "type": relation_type,
                        "target": relationship.attrib.get("Target", ""),
                    }
                    external_relationship_details.append(detail)
                    if not relation_type.lower().endswith("/hyperlink"):
                        features["unsafeExternalRelationships"].append(name)

    sensitive_parts = sum(len(parts) for parts in features.values())
    high_risk = any(
        features[key]
        for key in (
            "macros", "signatures", "externalLinks", "dataModel",
            "xlmMacros", "externalFormulaFunctions",
            "unsafeExternalRelationships",
        )
    )
    risk_level = "high" if high_risk else "medium" if sensitive_parts else "low"
    return {
        "riskLevel": risk_level,
        "features": {key: sorted(set(value)) for key, value in features.items()},
        "sensitiveParts": sensitive_parts,
        "externalRelationshipDetails": external_relationship_details,
        "externalFormulaDetails": external_formula_details,
    }


def workspace_path(path: Path, workspace_root: Path, *, must_exist: bool = False) -> Path:
    root = workspace_root.expanduser().resolve()
    candidate = path.expanduser()
    resolved = (candidate if candidate.is_absolute() else root / candidate).resolve()
    try:
        resolved.relative_to(root)
    except ValueError as error:
        raise ValueError(f"path escapes workspace: {resolved}") from error
    if must_exist and not resolved.exists():
        raise FileNotFoundError(resolved)
    return resolved


def staging_path(target: Path) -> Path:
    target.parent.mkdir(parents=True, exist_ok=True)
    return target.with_name(f".{target.stem}.nexa-stage-{uuid.uuid4().hex}{target.suffix}")


def snapshot_file(path: Path, workspace_root: Path) -> Path | None:
    if os.environ.get("NEXA_OFFICE_SKIP_SNAPSHOT") == "1" or not path.exists():
        return None
    relative = path.resolve().relative_to(workspace_root.resolve()).as_posix()
    key = hashlib.sha256(relative.encode("utf-8")).hexdigest()[:10]
    history_root = workspace_root / ".nexa" / "doc-history" / f"{path.name}-{key}"
    history_root.mkdir(parents=True, exist_ok=True)
    versions = sorted(
        (
            child
            for child in history_root.iterdir()
            if child.is_dir() and child.name.startswith("v") and child.name[1:].isdigit()
        ),
        key=lambda item: int(item.name[1:]),
    )
    version = int(versions[-1].name[1:]) + 1 if versions else 1
    destination_dir = history_root / f"v{version}"
    destination_dir.mkdir()
    destination = destination_dir / path.name
    shutil.copy2(path, destination)
    return destination


def publish_staged_artifact(
    staged: Path,
    target: Path,
    workspace_root: Path,
    *,
    validate: bool = True,
) -> tuple[Path | None, ValidationReport | None]:
    staged = workspace_path(staged, workspace_root, must_exist=True)
    target = workspace_path(target, workspace_root)
    if staged.parent != target.parent:
        raise ValueError("staging must use the target directory for atomic publication")
    report = validate_ooxml_package(staged) if validate and staged.suffix.lower() in OFFICE_SUFFIXES else None
    if report is not None and report.status == "fail":
        raise ValueError(json.dumps(report.to_dict(), ensure_ascii=False))
    snapshot = snapshot_file(target, workspace_root)
    os.replace(staged, target)
    return snapshot, report


def rollback_published_artifact(
    target: Path,
    snapshot: Path | None,
    workspace_root: Path,
) -> None:
    """Restore the pre-publish artifact, or remove a newly published target."""

    target = workspace_path(target, workspace_root)
    if snapshot is None:
        target.unlink(missing_ok=True)
        return
    snapshot = workspace_path(snapshot, workspace_root, must_exist=True)
    staged = staging_path(target)
    try:
        shutil.copy2(snapshot, staged)
        os.replace(staged, target)
    finally:
        staged.unlink(missing_ok=True)


def write_artifact_manifest(path: Path, payload: dict[str, Any], workspace_root: Path) -> Path:
    path = workspace_path(path, workspace_root)
    path.parent.mkdir(parents=True, exist_ok=True)
    staged = staging_path(path)
    try:
        staged.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        os.replace(staged, path)
    finally:
        staged.unlink(missing_ok=True)
    return path
