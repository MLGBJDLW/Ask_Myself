#!/usr/bin/env python3
"""OfficeArtifactEngine v2: one transactional interface for DOCX/PPTX/XLSX.

The engine deliberately separates candidate creation from publication. Format,
calculation, rendering, validation, and transaction decisions stay behind this
module so callers express outcomes and guarantees instead of shell pipelines.
"""

from __future__ import annotations

import argparse
import ctypes
import hashlib
import hmac
import json
import os
import re
import shutil
import secrets
import sys
import time
import uuid
import zipfile
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Protocol
from xml.etree import ElementTree as ET

from office_artifact_runtime import (
    office_dependency_lock_status,
    office_backend_statuses,
    office_python_dependency_statuses,
    publish_staged_artifact,
    rollback_published_artifact,
    run_openxml_sdk_validator,
    scan_ooxml_risks,
    snapshot_file,
    staging_path,
    validate_ooxml_package,
    workspace_path,
    write_artifact_manifest,
)
from office_artifact_service import OfficeExecutionPlan, execute_plan
from office_synthetic_preview import create_synthetic_preview
from office_schema import SchemaViolation, validate_schema_file


REQUEST_VERSION = 2
FORMATS = {"docx", "pptx", "xlsx"}
FORMAT_EXTENSIONS = {
    "docx": {".docx", ".docm", ".dotx", ".dotm"},
    "xlsx": {".xlsx", ".xlsm", ".xltx", ".xltm"},
    "pptx": {".pptx", ".pptm", ".potx", ".potm"},
}
INTENTS = {"create", "modify", "verify"}
QUALITY_LEVELS = {"draft", "standard", "publish", "native"}
CALCULATION_LEVELS = {"not_required", "static", "compatible", "native"}
DELIVERY_MODES = {"candidate", "publish"}
CANDIDATE_ID_RE = re.compile(r"^[0-9a-f]{32}$")
REQUEST_KEYS = {
    "requestVersion", "format", "intent", "source", "destination", "operations",
    "guarantees", "validation", "delivery", "preconditions",
}
GUARANTEE_KEYS = {"quality", "preservation", "calculation", "render"}
DELIVERY_KEYS = {"mode", "manifest"}
PRECONDITION_KEYS = {"sourceSha256"}
COMMON_OPERATION_KEYS = {"op", "elementId"}
OPERATION_KEYS: dict[str, dict[str, set[str]]] = {
    "docx": {
        "create": {"spec", "title", "subtitle", "body", "font", "footer", "author", "inputMd", "template"},
        "replace": {"find", "replace", "expectedSha256", "expectedMatches", "scope", "occurrence", "allowStyleMerge"},
        "redact": {"find", "replace", "expectedSha256", "expectedMatches", "scope", "occurrence", "allowStyleMerge"},
        "secure_redact": {"find", "replace", "expectedSha256", "expectedMatches", "privacyScrub"},
        "add_comment": {"find", "comment", "author", "initials", "date", "occurrence"},
        "strip_comments": set(),
        "tracked_replace": {"find", "replace", "author", "date", "occurrence"},
        "add_bookmark": {"find", "bookmarkName", "occurrence"},
        "insert_field": {"find", "instruction", "displayText", "occurrence"},
        "wrap_content_control": {"find", "tag", "title", "lock", "occurrence"},
        "set_protection": {"mode"},
        "accept_changes": set(),
        "reject_changes": set(),
        "validate": set(),
        "render": set(),
    },
    "xlsx": {
        "create": {"spec"},
        "replace": {"find", "replace", "expectedSha256", "expectedMatches"},
        "redact": {"find", "replace", "expectedSha256", "expectedMatches"},
        "set_value": {"sheet", "cell", "value"},
        "set_formula": {"sheet", "cell", "formula", "cachedValue"},
        "set_range": {"sheet", "range", "values"},
        "clear_range": {"sheet", "range"},
        "set_style": {"sheet", "cell", "range", "styleId"},
        "rename_sheet": {"sheet", "newName"},
        "set_defined_name": {"name", "formula", "scopeSheet"},
        "set_data_validation": {"sheet", "range", "validationType", "operator", "formula1", "formula2", "allowBlank", "showErrorMessage", "errorTitle", "error"},
        "create_table": {"sheet", "range", "name", "columns", "styleName"},
        "set_number_format": {"sheet", "cell", "range", "formatCode", "baseStyleId"},
        "set_chart_title": {"chartPart", "title"},
        "recalculate": set(),
        "validate": set(),
        "render": set(),
    },
    "pptx": {
        "create": {"spec", "template", "htmlFirst", "outdir", "mode", "screenshot", "title", "prompt", "authorEngine"},
        "replace": {"find", "replace", "expectedSha256", "expectedMatches"},
        "redact": {"find", "replace", "expectedSha256", "expectedMatches"},
        "insert_slide": {"after", "title", "body"},
        "set_text": {"slideId", "slideIndex", "shapeId", "shapeName", "text"},
        "clone_slide": {"slideId", "slideIndex", "afterIndex"},
        "reorder_slides": {"order"},
        "set_transition": {"slideId", "slideIndex", "transition", "speed", "direction"},
        "set_alt_text": {"slideId", "slideIndex", "shapeId", "shapeName", "altText", "title"},
        "set_speaker_notes": {"slideId", "slideIndex", "text"},
        "add_comment": {"slideId", "slideIndex", "comment", "author", "initials", "date", "x", "y"},
        "validate": set(),
        "render": set(),
    },
}
REQUIRED_OPERATION_KEYS: dict[str, set[str]] = {
    "set_value": {"sheet", "cell", "value"},
    "set_formula": {"sheet", "cell", "formula"},
    "set_range": {"sheet", "range", "values"},
    "clear_range": {"sheet", "range"},
    "reorder_slides": {"order"},
    "secure_redact": {"find"},
    "add_comment": {"find", "comment"},
    "tracked_replace": {"find", "replace"},
    "add_bookmark": {"find", "bookmarkName"},
    "insert_field": {"find", "instruction"},
    "wrap_content_control": {"find", "tag"},
    "set_protection": {"mode"},
    "rename_sheet": {"sheet", "newName"},
    "set_defined_name": {"name", "formula"},
    "set_data_validation": {"sheet", "range", "validationType"},
    "create_table": {"sheet", "range", "name"},
    "set_number_format": {"sheet", "formatCode"},
    "set_chart_title": {"chartPart", "title"},
    "set_alt_text": {"altText"},
    "set_speaker_notes": {"text"},
}
SAFE_FIELD_INSTRUCTION_RE = re.compile(
    r"(?i)(PAGE|NUMPAGES|SECTIONPAGES|TOC(?:\s+\\[A-Za-z]+(?:\s+[^\\]+)?)|"
    r"REF\s+[A-Za-z_][A-Za-z0-9_]{0,39}(?:\s+\\[A-Za-z]+)*|"
    r"SEQ\s+[A-Za-z_][A-Za-z0-9_]{0,39}(?:\s+\\[A-Za-z]+(?:\s+\S+)?)*)"
)


class OfficeArtifactError(Exception):
    def __init__(
        self,
        code: str,
        message: str,
        *,
        retryable: bool = False,
        details: dict[str, Any] | None = None,
    ) -> None:
        super().__init__(message)
        self.code = code
        self.retryable = retryable
        self.details = details or {}

    def to_dict(self) -> dict[str, Any]:
        return {
            "kind": "officeArtifactError",
            "code": self.code,
            "message": str(self),
            "stage": self.code.split(".", 1)[0],
            "retryable": self.retryable,
            "evidencePaths": self.details.get("evidencePaths", []),
            "details": self.details,
        }


class FormatAdapter(Protocol):
    id: str
    format: str

    def supports(self, operation: dict[str, Any]) -> bool: ...


@dataclass(frozen=True)
class LocalOpenXmlFormatAdapter:
    format: str
    supported_operations: frozenset[str]
    id: str = "nexa-openxml"

    def supports(self, operation: dict[str, Any]) -> bool:
        return str(operation.get("op", "")).lower() in self.supported_operations


FORMAT_ADAPTERS: dict[str, LocalOpenXmlFormatAdapter] = {
    "docx": LocalOpenXmlFormatAdapter("docx", frozenset({
        "create", "replace", "redact", "secure_redact", "validate", "render",
        "add_comment", "strip_comments", "tracked_replace", "accept_changes", "reject_changes",
        "add_bookmark", "insert_field", "wrap_content_control", "set_protection",
    })),
    "xlsx": LocalOpenXmlFormatAdapter("xlsx", frozenset({
        "create", "replace", "redact", "validate", "render", "recalculate",
        "set_value", "set_formula", "set_range", "clear_range", "set_style",
        "rename_sheet", "set_defined_name", "set_data_validation", "create_table",
        "set_number_format", "set_chart_title",
    })),
    "pptx": LocalOpenXmlFormatAdapter("pptx", frozenset({
        "create", "replace", "redact", "insert_slide", "validate", "render",
        "set_text", "clone_slide", "reorder_slides", "set_transition",
        "set_alt_text", "set_speaker_notes", "add_comment",
    })),
}


@dataclass(frozen=True)
class ArtifactRequest:
    format: str
    intent: str
    source: Path | None
    destination: Path
    operations: list[dict[str, Any]]
    quality: str
    preservation: str
    calculation: str
    render: str
    validation: dict[str, Any] | str | None
    delivery_mode: str
    manifest: Path

    @classmethod
    def from_dict(cls, payload: dict[str, Any], workspace_root: Path) -> "ArtifactRequest":
        _reject_unknown_keys(payload, REQUEST_KEYS, "request")
        version = payload.get("requestVersion", REQUEST_VERSION)
        if type(version) is not int or version != REQUEST_VERSION:
            raise OfficeArtifactError(
                "request.unsupported_version",
                f"requestVersion must be {REQUEST_VERSION}",
            )
        artifact_format = str(payload.get("format", "")).lower()
        if artifact_format not in FORMATS:
            raise OfficeArtifactError("request.invalid_format", "format must be docx, pptx, or xlsx")
        intent = str(payload.get("intent", "")).lower()
        if intent not in INTENTS:
            raise OfficeArtifactError("request.invalid_intent", "intent must be create, modify, or verify")

        raw_source = payload.get("source")
        source = workspace_path(Path(str(raw_source)), workspace_root, must_exist=True) if raw_source else None
        if intent != "create" and source is None:
            raise OfficeArtifactError("request.source_required", f"source is required for intent={intent}")
        if source is not None and source.suffix.lower() not in FORMAT_EXTENSIONS[artifact_format]:
            raise OfficeArtifactError(
                "request.source_format",
                f"source extension must belong to the {artifact_format} format family",
            )
        preconditions = payload.get("preconditions", {})
        if not isinstance(preconditions, dict):
            raise OfficeArtifactError("request.invalid_preconditions", "preconditions must be an object")
        _reject_unknown_keys(preconditions, PRECONDITION_KEYS, "preconditions")
        expected_source_sha = preconditions.get("sourceSha256")
        if intent != "create" and not expected_source_sha:
            raise OfficeArtifactError(
                "precondition.source_sha_required",
                "modify/verify requires preconditions.sourceSha256 from inspect so the source is CAS-bound",
                details={"requiredAction": "inspect", "field": "preconditions.sourceSha256"},
            )
        if expected_source_sha and (
            not isinstance(expected_source_sha, str)
            or re.fullmatch(r"[0-9A-Fa-f]{64}", expected_source_sha) is None
        ):
            raise OfficeArtifactError(
                "schema.precondition_type",
                "preconditions.sourceSha256 must be a 64-character hexadecimal SHA-256",
            )
        if expected_source_sha and source is not None:
            actual_source_sha = _sha256(source)
            if actual_source_sha.lower() != str(expected_source_sha).lower():
                raise OfficeArtifactError(
                    "precondition.source_changed",
                    "source SHA-256 does not match the inspected artifact",
                    retryable=True,
                    details={
                        "expectedSha256": str(expected_source_sha),
                        "actualSha256": actual_source_sha,
                    },
                )

        raw_destination = payload.get("destination")
        if intent == "verify" and raw_destination is None and source is not None:
            raw_destination = source.with_name(f"{source.stem}-verified{source.suffix}")
        if raw_destination is None:
            raise OfficeArtifactError("request.destination_required", "destination is required")
        destination = workspace_path(Path(str(raw_destination)), workspace_root)
        if destination.suffix.lower() not in FORMAT_EXTENSIONS[artifact_format]:
            raise OfficeArtifactError(
                "request.destination_format",
                f"destination extension must belong to the {artifact_format} format family",
            )
        if intent == "create" and destination.suffix.lower() != f".{artifact_format}":
            raise OfficeArtifactError(
                "request.create_extension",
                "create cannot fabricate macro/template semantics; create the base format or modify an existing macro/template package",
            )
        if source is not None and source.suffix.lower() != destination.suffix.lower():
            raise OfficeArtifactError(
                "request.extension_conversion_unsupported",
                "modify/verify preserves the exact Office extension; explicit macro/template conversion is unsupported",
            )

        raw_operations = payload.get("operations", [])
        if not isinstance(raw_operations, list) or not all(isinstance(item, dict) for item in raw_operations):
            raise OfficeArtifactError("request.invalid_operations", "operations must be an array of objects")
        operations = list(raw_operations)
        if intent == "create" and not operations:
            raise OfficeArtifactError("request.operations_required", "create requires at least one operation")
        adapter = FORMAT_ADAPTERS[artifact_format]
        unsupported = [str(item.get("op", "")) for item in operations if not adapter.supports(item)]
        if unsupported:
            raise OfficeArtifactError(
                "capability.unsupported_operation",
                f"{artifact_format} adapter does not support: {', '.join(unsupported)}",
                details={"adapter": adapter.id, "operations": unsupported},
            )
        for index, operation in enumerate(operations):
            _validate_operation(artifact_format, operation, index)
        if any(str(operation.get("op", "")).lower() == "secure_redact" for operation in operations):
            if destination.exists():
                raise OfficeArtifactError(
                    "secure_redact.new_destination_required",
                    "secure_redact must publish to a new destination so plaintext rollback snapshots are never retained",
                )

        guarantees = payload.get("guarantees", {})
        if not isinstance(guarantees, dict):
            raise OfficeArtifactError("request.invalid_guarantees", "guarantees must be an object")
        _reject_unknown_keys(guarantees, GUARANTEE_KEYS, "guarantees")
        quality = str(guarantees.get("quality", "standard")).lower()
        if quality not in QUALITY_LEVELS:
            raise OfficeArtifactError("request.invalid_quality", "quality must be draft, standard, publish, or native")
        preservation = str(guarantees.get("preservation", "strict")).lower()
        if preservation not in {"strict", "balanced", "replace"}:
            raise OfficeArtifactError("request.invalid_preservation", "invalid preservation guarantee")
        calculation = str(guarantees.get("calculation", "static" if artifact_format == "xlsx" else "not_required")).lower()
        if calculation not in CALCULATION_LEVELS:
            raise OfficeArtifactError("request.invalid_calculation", "invalid calculation guarantee")
        if artifact_format != "xlsx" and calculation != "not_required":
            raise OfficeArtifactError("request.calculation_format", "calculation guarantees only apply to xlsx")

        default_render = "all" if quality in {"publish", "native"} else "none"
        render = str(guarantees.get("render", default_render)).lower()
        if render not in {"none", "important_surfaces", "all"}:
            raise OfficeArtifactError("request.invalid_render", "invalid render guarantee")
        if quality in {"publish", "native"} and render == "none":
            raise OfficeArtifactError(
                "request.render_required",
                f"quality={quality} requires final candidate render evidence",
            )

        delivery = payload.get("delivery", {})
        if not isinstance(delivery, dict):
            raise OfficeArtifactError("request.invalid_delivery", "delivery must be an object")
        _reject_unknown_keys(delivery, DELIVERY_KEYS, "delivery")
        delivery_mode = str(delivery.get("mode", "candidate")).lower()
        if delivery_mode not in DELIVERY_MODES:
            raise OfficeArtifactError("request.invalid_delivery_mode", "delivery.mode must be candidate or publish")
        raw_manifest = delivery.get("manifest") or destination.with_suffix(f"{destination.suffix}.manifest.json")
        manifest = workspace_path(Path(str(raw_manifest)), workspace_root)
        if manifest in {destination, source}:
            raise OfficeArtifactError(
                "path.role_conflict",
                "delivery manifest must be distinct from source and destination",
            )
        internal_state_root = (workspace_root / ".nexa").resolve()
        protected_roles = {
            "source": source,
            "destination": destination,
            "manifest": manifest,
        }
        for role, role_path in protected_roles.items():
            if role_path is not None and (
                role_path == internal_state_root or internal_state_root in role_path.parents
            ):
                raise OfficeArtifactError(
                    "path.internal_state_conflict",
                    f"{role} cannot target Nexa's reserved .nexa state",
                    details={"role": role, "path": str(role_path)},
                )
        if artifact_format == "xlsx" and manifest == destination.with_suffix(".xlsx.qa.json"):
            raise OfficeArtifactError(
                "path.role_conflict",
                "delivery manifest must be distinct from the XLSX QA sidecar",
            )
        validation = payload.get("validation")
        if validation is not None and not isinstance(validation, (dict, str)):
            raise OfficeArtifactError(
                "request.invalid_validation",
                "validation must be a contract object or a workspace-local JSON path",
            )
        if isinstance(validation, dict):
            _validate_contract_shape(artifact_format, validation)
        elif isinstance(validation, str):
            validation_path = workspace_path(Path(validation), workspace_root, must_exist=True)
            try:
                loaded_validation = json.loads(validation_path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError) as error:
                raise OfficeArtifactError(
                    "validation.contract_invalid",
                    f"validation contract JSON cannot be loaded: {error}",
                ) from error
            if not isinstance(loaded_validation, dict):
                raise OfficeArtifactError(
                    "validation.contract_invalid",
                    "validation contract file root must be an object",
                )
            _validate_contract_shape(artifact_format, loaded_validation)
            validation = str(validation_path)

        input_roles: dict[str, Path] = {}
        if source is not None:
            input_roles["source"] = source
        for index, operation in enumerate(operations):
            for field in ("spec", "inputMd", "template"):
                if operation.get(field):
                    input_roles[f"operations[{index}].{field}"] = workspace_path(
                        Path(str(operation[field])), workspace_root, must_exist=True
                    )
        if isinstance(validation, str):
            input_roles["validation"] = Path(validation)
        output_roles = {
            "destination": destination,
            "manifest": manifest,
            **(
                {"xlsxQa": destination.with_suffix(".xlsx.qa.json")}
                if artifact_format == "xlsx"
                else {}
            ),
        }
        reserved = (workspace_root / ".nexa").resolve()
        for role, path in {**input_roles, **output_roles}.items():
            if path == reserved or reserved in path.parents:
                raise OfficeArtifactError(
                    "path.internal_state_conflict",
                    f"{role} cannot target Nexa's reserved .nexa state",
                )
        for input_role, input_path in input_roles.items():
            for output_role, output_path in output_roles.items():
                if input_path == output_path and not (
                    input_role == "source" and output_role == "destination"
                ):
                    raise OfficeArtifactError(
                        "path.role_conflict",
                        f"{output_role} cannot overwrite request input {input_role}",
                    )
        request_schema = (
            Path(__file__).resolve().parent.parent
            / "references"
            / "office-artifact-request-v2.schema.json"
        )
        try:
            validate_schema_file(payload, request_schema)
        except (OSError, json.JSONDecodeError, SchemaViolation) as error:
            raise OfficeArtifactError(
                "schema.request_invalid",
                f"request failed executable JSON Schema validation: {error}",
            ) from error
        return cls(
            format=artifact_format,
            intent=intent,
            source=source,
            destination=destination,
            operations=operations,
            quality=quality,
            preservation=preservation,
            calculation=calculation,
            render=render,
            validation=validation,
            delivery_mode=delivery_mode,
            manifest=manifest,
        )


class OfficeArtifactEngine:
    def __init__(self, workspace_root: Path) -> None:
        self.workspace_root = workspace_root.resolve()
        self.state_root = self.workspace_root / ".nexa" / "office-artifacts"
        self.candidates_root = self.state_root / "candidates"
        self.receipts_root = self.state_root / "receipts"
        self.locks_root = self.state_root / "locks"
        self.journals_root = self.state_root / "journals"
        self.integrity_root = _office_integrity_root()
        self.integrity_key = _load_or_create_integrity_key(self.integrity_root)
        self._recover_incomplete_journals()
        self._recover_orphan_locks()

    def capabilities(self) -> dict[str, Any]:
        backends = office_backend_statuses()
        local_operations = sorted(
            {operation for adapter in FORMAT_ADAPTERS.values() for operation in adapter.supported_operations}
            - {"render", "recalculate"}
        )
        return {
            "kind": "officeArtifactCapabilities",
            "requestVersion": REQUEST_VERSION,
            "adapterContractVersion": 1,
            "formats": {
                name: {
                    "adapter": adapter.id,
                    "operations": sorted(adapter.supported_operations),
                }
                for name, adapter in FORMAT_ADAPTERS.items()
            },
            "qualityLevels": sorted(QUALITY_LEVELS),
            "calculationLevels": sorted(CALCULATION_LEVELS),
            "backends": backends,
            "pythonDependencies": office_python_dependency_statuses(),
            "pythonDependencyLock": office_dependency_lock_status(),
            "formatReadiness": {
                artifact_format: office_python_dependency_statuses(artifact_format)
                for artifact_format in sorted(FORMATS)
            },
            "adapters": [
                {
                    "adapterVersion": 1,
                    "id": "nexa-openxml",
                    "deployment": "local-file",
                    "formats": sorted(FORMAT_ADAPTERS),
                    "operations": local_operations,
                    "guarantees": {
                        "preservation": ["strict", "balanced", "replace"],
                        "calculation": ["not_required", "static"],
                        "render": ["none"],
                    },
                    "limitations": [
                        "Does not prove Microsoft Office native layout without a render/native adapter.",
                        "Does not execute macros or refresh external data.",
                    ],
                    "requires": ["python"],
                },
                {
                    "adapterVersion": 1,
                    "id": "pptxgenjs",
                    "deployment": "local-file",
                    "formats": ["pptx"],
                    "operations": ["create", "masters", "charts", "svg", "media"],
                    "guarantees": {
                        "preservation": ["replace"],
                        "calculation": ["not_required"],
                        "render": ["none"],
                    },
                    "limitations": [
                        "New-deck author only; existing decks stay on the OOXML patch/clone adapter.",
                        "Local reviewed assets only; ICNS/JXL/HEIF and remote resources are blocked.",
                    ],
                    "requires": ["node", "pptxgenjs@4.0.1"],
                },
                {
                    "adapterVersion": 1,
                    "id": "openxml-sdk",
                    "deployment": "local-file",
                    "formats": ["docx", "xlsx", "pptx"],
                    "operations": ["schema-validate"],
                    "guarantees": {
                        "preservation": ["strict", "balanced", "replace"],
                        "calculation": [],
                        "render": [],
                    },
                    "limitations": [
                        "Validates Open XML schema and semantic constraints; it does not render layout.",
                    ],
                    "requires": ["DocumentFormat.OpenXml@3.5.1", ".NET 8 runtime"],
                },
                {
                    "adapterVersion": 1,
                    "id": "libreoffice",
                    "deployment": "native-host",
                    "formats": ["docx", "xlsx", "pptx"],
                    "operations": ["render", "recalculate"],
                    "guarantees": {
                        "preservation": ["balanced"],
                        "calculation": ["compatible"],
                        "render": ["important_surfaces", "all"],
                    },
                    "limitations": ["Compatible output is not Microsoft Office-native evidence."],
                    "requires": ["libreoffice", "poppler"],
                },
                {
                    "adapterVersion": 1,
                    "id": "windows-com",
                    "deployment": "native-host",
                    "formats": ["docx", "xlsx", "pptx"],
                    "operations": ["finalize", "recalculate", "render"],
                    "guarantees": {
                        "preservation": ["balanced"],
                        "calculation": ["native"],
                        "render": ["all"],
                    },
                    "limitations": [
                        "Explicit local Windows adapter; not an unattended server backend.",
                        "Requires desktop Microsoft Office and an app-owned COM watchdog process.",
                    ],
                    "requires": ["microsoft-office", "pywin32"],
                },
                {
                    "adapterVersion": 1,
                    "id": "officejs-live",
                    "deployment": "live-officejs",
                    "formats": ["docx", "xlsx", "pptx"],
                    "operations": [
                        "read-current-document", "set-text", "set-range", "add-comment",
                        "insert-slide", "native-object-edit",
                    ],
                    "guarantees": {
                        "preservation": ["native-host"],
                        "calculation": ["native"],
                        "render": ["host-visible"],
                    },
                    "limitations": [
                        "Requires a separately connected and user-authorized Office.js add-in host.",
                        "Not available through the local-file engine until a host session is registered.",
                    ],
                    "requires": ["officejs-host-session", "requirement-set-negotiation", "user-consent"],
                    "status": "not-connected",
                },
            ],
            "externalAdapterDeclarations": self._external_adapter_declarations(),
            "schemas": {
                "request": "references/office-artifact-request-v2.schema.json",
                "validation": "references/office-validation-contract-v2.schema.json",
                "adapter": "references/office-adapter-manifest-v1.schema.json",
                "liveHost": "references/office-host-adapter-v1.schema.json",
            },
            "lifecycle": ["inspect", "assess", "execute", "decide", "restore"],
        }

    def inspect(self, source: str, requested_format: str | None = None) -> dict[str, Any]:
        path = workspace_path(Path(source), self.workspace_root, must_exist=True)
        suffix = path.suffix.lower()
        inferred = next(
            (name for name, extensions in FORMAT_EXTENSIONS.items() if suffix in extensions),
            None,
        )
        artifact_format = (requested_format or inferred or "").lower()
        if artifact_format not in FORMATS:
            raise OfficeArtifactError(
                "inspect.unsupported_format",
                f"could not infer a supported Office format from {path.name}",
            )
        if inferred and inferred != artifact_format:
            raise OfficeArtifactError(
                "inspect.format_mismatch",
                f"requested format {artifact_format} does not match {path.name}",
            )
        structural = validate_ooxml_package(path).to_dict()
        if structural.get("status") == "fail":
            raise OfficeArtifactError(
                "inspect.structural_failed",
                "artifact failed OOXML package validation",
                details={"structural": structural},
            )
        profile = self._inspect_format_profile(path, artifact_format)
        return {
            "kind": "officeArtifactInspection",
            "requestVersion": REQUEST_VERSION,
            "format": artifact_format,
            "source": str(path),
            "sha256": _sha256(path),
            "structural": structural,
            "risk": scan_ooxml_risks(path),
            "profile": profile,
        }

    def _inspect_format_profile(self, path: Path, artifact_format: str) -> dict[str, Any]:
        skills_root = Path(__file__).resolve().parents[2]
        if artifact_format == "pptx":
            scripts = skills_root / "pptx-presentation-design" / "scripts"
            if str(scripts) not in sys.path:
                sys.path.insert(0, str(scripts))
            from pptx_audit import audit  # type: ignore

            return audit(path)
        if artifact_format == "xlsx":
            scripts = skills_root / "xlsx-workbook-design" / "scripts"
            if str(scripts) not in sys.path:
                sys.path.insert(0, str(scripts))
            from xlsx_audit import audit  # type: ignore

            profile = audit(path)
            profile["formulaProfile"] = self._xlsx_formula_profile(path)
            return profile

        word_ns = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
        with zipfile.ZipFile(path) as archive:
            names = set(archive.namelist())
            document = ET.fromstring(archive.read("word/document.xml"))
            styles: dict[str, str] = {}
            if "word/styles.xml" in names:
                styles_root = ET.fromstring(archive.read("word/styles.xml"))
                for style in styles_root.iter(f"{{{word_ns}}}style"):
                    style_id = style.attrib.get(f"{{{word_ns}}}styleId", "")
                    name = next(
                        (
                            item.attrib.get(f"{{{word_ns}}}val", "")
                            for item in style
                            if item.tag == f"{{{word_ns}}}name"
                        ),
                        style_id,
                    )
                    if style_id:
                        styles[style_id] = name
            document_paragraphs = list(document.iter(f"{{{word_ns}}}p"))
            headings = []
            for index, paragraph in enumerate(document_paragraphs):
                text = "".join(
                    item.text or "" for item in paragraph.iter(f"{{{word_ns}}}t")
                )
                style_id = next(
                    (
                        item.attrib.get(f"{{{word_ns}}}val", "")
                        for item in paragraph.iter(f"{{{word_ns}}}pStyle")
                    ),
                    "",
                )
                heading_match = re.fullmatch(r"Heading([1-9])", style_id, re.IGNORECASE)
                style_name = (
                    f"Heading {heading_match.group(1)}"
                    if heading_match
                    else styles.get(style_id, style_id)
                )
                if text.strip() and (
                    style_name.casefold().startswith("heading")
                    or style_id.casefold().startswith("heading")
                ):
                    headings.append({"index": index, "style": style_name, "text": text})
            story_parts = [
                name
                for name in archive.namelist()
                if name == "word/document.xml"
                or re.fullmatch(
                    r"word/(?:header[0-9]+|footer[0-9]+|comments(?:Extended)?|footnotes|endnotes)\.xml",
                    name,
                    re.IGNORECASE,
                )
            ]
            preview_lines = []
            for name in story_parts:
                root = ET.fromstring(archive.read(name))
                text = "".join(item.text or "" for item in root.iter(f"{{{word_ns}}}t"))
                if text.strip():
                    preview_lines.append(text)
        return {
            "paragraphs": len(document_paragraphs),
            "tables": sum(1 for _ in document.iter(f"{{{word_ns}}}tbl")),
            "sections": max(1, sum(1 for _ in document.iter(f"{{{word_ns}}}sectPr"))),
            "headings": headings,
            "textPreview": "\n".join(preview_lines)[:4000],
            "profileEngine": "direct-openxml",
        }

    def assess(self, payload: dict[str, Any]) -> dict[str, Any]:
        request = ArtifactRequest.from_dict(payload, self.workspace_root)
        adapter_plan = self._adapter_plan(request)
        backend = str(adapter_plan["primaryAdapter"])
        statuses = {item["id"]: item for item in office_backend_statuses()}
        adapter_manifests = {
            item["id"]: item for item in self.capabilities()["adapters"]
        }
        consent_required = [
            adapter_id for adapter_id in adapter_plan["requiredAdapters"]
            if statuses[adapter_id].get("requires_explicit_network_consent")
        ]
        limitations = {
            adapter_id: adapter_manifests.get(adapter_id, {}).get("limitations", [])
            for adapter_id in adapter_plan["requiredAdapters"]
        }
        blockers: list[dict[str, Any]] = []
        source_profile: dict[str, Any] | None = None
        for adapter_id in adapter_plan["requiredAdapters"]:
            readiness = statuses[adapter_id]
            if readiness["status"] != "ready":
                blockers.append({
                    "code": "backend.unavailable",
                    "backend": adapter_id,
                    "steps": [
                        step["step"] for step in adapter_plan["steps"]
                        if step["adapter"] == adapter_id
                    ],
                    "detail": readiness.get("detail"),
                })
        openxml_dependencies = office_python_dependency_statuses(request.format)
        unavailable_dependencies = [
            item for item in openxml_dependencies if item["status"] != "ready"
        ]
        if unavailable_dependencies:
            blockers.append({
                "code": "dependency.unavailable",
                "backend": "nexa-openxml",
                "format": request.format,
                "dependencies": unavailable_dependencies,
            })
        if request.source is not None:
            source_validation = validate_ooxml_package(request.source)
            if source_validation.status == "fail":
                blockers.append({
                    "code": "source.structural_invalid",
                    "detail": "source package failed safety/structure preflight",
                    "validation": source_validation.to_dict(),
                })
                for blocker in blockers:
                    blocker.setdefault("message", blocker.get("detail", blocker["code"]))
                return {
                    "kind": "officeArtifactAssessment",
                    "requestVersion": REQUEST_VERSION,
                    "status": "blocked",
                    "format": request.format,
                    "intent": request.intent,
                    "adapter": FORMAT_ADAPTERS[request.format].id,
                    "backend": backend,
                    "adapterPlan": adapter_plan,
                    "consentRequired": consent_required,
                    "limitations": limitations,
                    "quality": request.quality,
                    "guarantees": {
                        "preservation": request.preservation,
                        "calculation": request.calculation,
                        "render": request.render,
                    },
                    "ready": False,
                    "blockers": blockers,
                    "sourceProfile": None,
                }
            risk = scan_ooxml_risks(request.source)
            source_profile = {"risk": risk}
            if (
                request.preservation == "strict"
                and (request.quality == "native" or request.calculation == "native")
                and request.intent != "verify"
            ):
                blockers.append({
                    "code": "preservation.native_roundtrip_not_strict",
                    "backend": "windows-com",
                    "detail": (
                        "Microsoft Office native open/save may rewrite package parts; "
                        "use balanced preservation or local OpenXML strict editing."
                    ),
                })
            executable_excel_features = {
                key: risk["features"].get(key, [])
                for key in (
                    "xlmMacros", "externalFormulaFunctions", "externalLinks",
                    "connections", "dataModel", "unsafeExternalRelationships",
                )
                if risk["features"].get(key)
            }
            if (
                request.format == "xlsx"
                and executable_excel_features
                and (
                    request.calculation in {"compatible", "native"}
                    or request.quality == "native"
                )
            ):
                blockers.append({
                    "code": "calculation.external_execution_blocked",
                    "detail": (
                        "Native/compatible calculation is network-closed and will not execute "
                        "XLM or external-data formula functions."
                    ),
                    "features": executable_excel_features,
                })
            if (
                "windows-com" in adapter_plan["requiredAdapters"]
                and risk["features"].get("unsafeExternalRelationships")
            ):
                blockers.append({
                    "code": "native.external_relationship_blocked",
                    "detail": (
                        "Native Office open/render is fail-closed for external templates, "
                        "images, OLE/package links, media, and data relationships."
                    ),
                    "relationships": risk.get("externalRelationshipDetails", []),
                })
            if (
                request.preservation == "strict"
                and risk["features"].get("signatures")
                and request.intent != "verify"
            ):
                blockers.append({
                    "code": "preservation.digital_signature",
                    "detail": "Any package mutation invalidates the digital signature.",
                    "parts": risk["features"]["signatures"],
                })
            if request.format == "xlsx":
                formula_profile = self._xlsx_formula_profile(request.source)
                source_profile["formulaProfile"] = formula_profile
                if request.calculation == "compatible":
                    sensitive = {
                        name: parts for name, parts in risk["features"].items() if parts
                    }
                    if sensitive:
                        blockers.append({
                            "code": "calculation.compatible_roundtrip_risk",
                            "backend": "libreoffice",
                            "features": sensitive,
                        })
                    if formula_profile["requiresExcelNative"]:
                        blockers.append({
                            "code": "calculation.excel_native_required",
                            "features": formula_profile["nativeFeatures"],
                        })
        for blocker in blockers:
            blocker.setdefault("message", blocker.get("detail", blocker["code"]))
        return {
            "kind": "officeArtifactAssessment",
            "requestVersion": REQUEST_VERSION,
            "status": "ready" if not blockers else "blocked",
            "format": request.format,
            "intent": request.intent,
            "adapter": FORMAT_ADAPTERS[request.format].id,
            "backend": backend,
            "adapterPlan": adapter_plan,
            "consentRequired": consent_required,
            "limitations": limitations,
            "quality": request.quality,
            "guarantees": {
                "preservation": request.preservation,
                "calculation": request.calculation,
                "render": request.render,
            },
            "ready": not blockers,
            "blockers": blockers,
            "sourceProfile": source_profile,
        }

    def execute(self, payload: dict[str, Any]) -> dict[str, Any]:
        request = ArtifactRequest.from_dict(payload, self.workspace_root)
        assessment = self.assess(payload)
        if not assessment["ready"]:
            raise OfficeArtifactError(
                "capability.unsatisfied",
                "requested guarantees cannot be satisfied by available adapters",
                retryable=True,
                details={"blockers": assessment["blockers"]},
            )
        candidate_id = uuid.uuid4().hex
        candidate_dir = self.candidates_root / candidate_id
        candidate_dir.mkdir(parents=True, exist_ok=False)
        candidate_path = candidate_dir / f"artifact{request.destination.suffix.lower()}"
        candidate_manifest = candidate_dir / "execution.json"
        state_path = candidate_dir / "state.json"
        execution_plan = self._execution_plan_payload(request, candidate_path, candidate_manifest)
        state = {
            "kind": "officeArtifactCandidateState",
            "version": 1,
            "candidateId": candidate_id,
            "status": "executing",
            "createdAt": _utc_now(),
            "pid": os.getpid(),
            "destination": str(request.destination),
            "requestedManifest": str(request.manifest),
            "candidatePath": str(candidate_path),
            "destinationExistedAtExecute": request.destination.exists(),
            "destinationBaseSha256": _sha256(request.destination) if request.destination.exists() else None,
            "publishRoleBases": [
                {
                    "role": role,
                    "path": str(path),
                    "existed": path.exists(),
                    "sha256": _sha256(path) if path.exists() else None,
                }
                for role, path in (
                    ("destination", request.destination),
                    ("manifest", request.manifest),
                    *(([("xlsxQa", request.destination.with_suffix(".xlsx.qa.json"))]) if request.format == "xlsx" else []),
                )
            ],
            # Candidate state is durable.  Persist only a content hash and
            # non-sensitive routing metadata; replacement targets, comments,
            # redaction needles, and validation literals must never become a
            # second plaintext copy of user content.
            "requestSha256": _json_sha256(payload),
            "requestSummary": {
                "format": request.format,
                "intent": request.intent,
                "quality": request.quality,
                "preservation": request.preservation,
                "calculation": request.calculation,
                "render": request.render,
                "deliveryMode": request.delivery_mode,
                "operations": [str(item.get("op", "")).lower() for item in request.operations],
                "validationSha256": _json_sha256(request.validation)
                if request.validation is not None
                else None,
            },
        }
        self._write_candidate_state(state_path, state)
        try:
            plan = OfficeExecutionPlan.from_internal_dict(execution_plan, self.workspace_root)
            execution, exit_code = execute_plan(plan, self.workspace_root)
            if exit_code != 0 or not execution.get("ok"):
                raise OfficeArtifactError(
                    "execution.failed",
                    str(execution.get("error") or "Office artifact execution failed"),
                    details={"execution": execution},
                )
            if request.quality == "native" or request.calculation == "native":
                execution = self._native_finalize(request, candidate_path, candidate_dir, execution)
            validation = validate_ooxml_package(candidate_path)
            if validation.status == "fail":
                raise OfficeArtifactError(
                    "validation.structural_failed",
                    "candidate failed final OOXML validation",
                    details={"validation": validation.to_dict()},
                )
            if request.quality in {"publish", "native"}:
                schema_validation = run_openxml_sdk_validator(candidate_path)
                if schema_validation.get("status") != "pass":
                    raise OfficeArtifactError(
                        "validation.openxml_schema_failed",
                        "candidate failed Microsoft Open XML SDK schema validation",
                        details={"schemaValidation": schema_validation},
                    )
                execution["schemaValidation"] = schema_validation
            try:
                execution["syntheticPreview"] = create_synthetic_preview(
                    candidate_path,
                    candidate_dir / "synthetic-preview",
                )
            except (OSError, ValueError, RuntimeError, ImportError, zipfile.BadZipFile) as error:
                execution.setdefault("warnings", []).append(
                    f"Synthetic structural preview unavailable: {type(error).__name__}: {error}"
                )
            candidate_sha256 = _sha256(candidate_path)
            render_evidence = self._render_evidence(
                candidate_path,
                execution,
                request.format,
                request.render,
            )
            if request.render != "none" and not render_evidence["complete"]:
                raise OfficeArtifactError(
                    "render.incomplete_evidence",
                    "requested render guarantee did not produce complete candidate-bound evidence",
                    details={"renderEvidence": render_evidence},
                )
            state.update({
                "status": "candidate",
                "execution": execution,
                "assessment": assessment,
                "candidateSha256": candidate_sha256,
                "renderEvidence": render_evidence,
                "updatedAt": _utc_now(),
            })
            self._write_candidate_state(state_path, state)
            outcome = self._candidate_outcome(state)
            if request.delivery_mode == "publish":
                return self.decide(candidate_id, "publish")
            return outcome
        except Exception:
            state["status"] = "failed"
            state["updatedAt"] = _utc_now()
            self._write_candidate_state(state_path, state)
            raise

    def decide(self, candidate_id: str, decision: str) -> dict[str, Any]:
        state_path, state = self._load_candidate(candidate_id)
        if state.get("status") != "candidate":
            raise OfficeArtifactError(
                "candidate.invalid_state",
                f"candidate {candidate_id} is in state {state.get('status')}",
            )
        decision = decision.lower()
        if decision == "discard":
            candidate_dir = state_path.parent
            state["status"] = "discarded"
            state["updatedAt"] = _utc_now()
            self._write_candidate_state(state_path, state)
            shutil.rmtree(candidate_dir)
            return {
                "kind": "officeArtifactOutcome",
                "status": "discarded",
                "candidateId": candidate_id,
            }
        if decision != "publish":
            raise OfficeArtifactError("decision.invalid", "decision must be publish or discard")

        candidate = workspace_path(Path(state["candidatePath"]), self.workspace_root, must_exist=True)
        destination = workspace_path(Path(state["destination"]), self.workspace_root)
        requested_manifest = workspace_path(Path(state["requestedManifest"]), self.workspace_root)
        artifact_format = str(state.get("requestSummary", {}).get("format", ""))
        if (
            artifact_format not in FORMATS
            or candidate.suffix.lower() not in FORMAT_EXTENSIONS[artifact_format]
        ):
            raise OfficeArtifactError("candidate.invalid_state_file", "candidate format binding is invalid")
        role_paths = [destination, requested_manifest]
        destination_qa = destination.with_suffix(".xlsx.qa.json") if artifact_format == "xlsx" else None
        if destination_qa is not None:
            role_paths.append(destination_qa)
        self._validate_public_role_paths(role_paths)
        if _sha256(candidate) != state.get("candidateSha256"):
            raise OfficeArtifactError(
                "candidate.hash_mismatch",
                "candidate changed after verification; execute it again",
            )
        lock_paths = self._acquire_role_locks(role_paths, candidate_id)
        staged: Path | None = None
        snapshot = None
        sidecar_records: list[dict[str, Any]] = []
        published = False
        receipt_path: Path | None = None
        receipt: dict[str, Any] | None = None
        self.journals_root.mkdir(parents=True, exist_ok=True)
        journal_path = self.journals_root / f"{candidate_id}.json"
        journal: dict[str, Any] = {
            "kind": "officeArtifactPublishJournal",
            "version": 1,
            "candidateId": candidate_id,
            "status": "active",
            "createdAt": _utc_now(),
            "pid": os.getpid(),
            "ownerToken": secrets.token_hex(16),
            "destination": str(destination),
            "lockRolePaths": [str(path) for path in role_paths],
            "roles": [],
        }
        journal_active = False
        try:
            self._write_journal(journal_path, journal)
            journal_active = True
            self._assert_publish_role_preconditions(state, role_paths)
            destination_existed = destination.exists()
            if destination_existed and os.environ.get("NEXA_OFFICE_SKIP_SNAPSHOT") == "1":
                raise OfficeArtifactError(
                    "transaction.snapshot_disabled",
                    "refusing to overwrite an existing destination while snapshots are disabled",
                )
            staged = staging_path(destination)
            shutil.copy2(candidate, staged)
            snapshot, validation, destination_role = self._journal_publish_role(
                journal_path,
                journal,
                staged,
                destination,
                validate=True,
            )
            if destination_existed and snapshot is None:
                raise OfficeArtifactError(
                    "transaction.snapshot_missing",
                    "existing destination was published without a restorable snapshot",
                )
            published = True
            candidate_qa = candidate.with_suffix(".xlsx.qa.json")
            if candidate_qa.exists():
                destination_qa = destination.with_suffix(".xlsx.qa.json")
                staged_qa = staging_path(destination_qa)
                qa_payload = json.loads(candidate_qa.read_text(encoding="utf-8"))
                qa_payload["path"] = str(destination)
                qa_payload["qaPath"] = str(destination_qa)
                staged_qa.write_text(
                    json.dumps(qa_payload, ensure_ascii=False, indent=2) + "\n",
                    encoding="utf-8",
                )
                qa_snapshot, _, qa_role = self._journal_publish_role(
                    journal_path,
                    journal,
                    staged_qa,
                    destination_qa,
                    validate=False,
                )
                sidecar_records.append({
                    "path": str(destination_qa),
                    "snapshot": str(qa_snapshot) if qa_snapshot else None,
                    "snapshotSha256": _sha256(qa_snapshot) if qa_snapshot else None,
                    "existedBefore": qa_role["existedBefore"],
                    "publishedSha256": _sha256(destination_qa),
                })
            receipt_id = uuid.uuid4().hex
            receipt_path = self.receipts_root / f"{receipt_id}.json"
            journal["receiptId"] = receipt_id
            self._write_journal(journal_path, journal)
            outcome = self._candidate_outcome(state)
            outcome.update({
                "status": "published",
                "path": str(destination),
                "receiptId": receipt_id,
                "sha256": _sha256(destination),
            })
            staged_manifest = staging_path(requested_manifest)
            staged = staged_manifest
            staged_manifest.write_text(
                json.dumps(outcome, ensure_ascii=False, indent=2) + "\n",
                encoding="utf-8",
            )
            manifest_snapshot, _, manifest_role = self._journal_publish_role(
                journal_path,
                journal,
                staged_manifest,
                requested_manifest,
                validate=False,
            )
            sidecar_records.append({
                "path": str(requested_manifest),
                "snapshot": str(manifest_snapshot) if manifest_snapshot else None,
                "snapshotSha256": _sha256(manifest_snapshot) if manifest_snapshot else None,
                "existedBefore": manifest_role["existedBefore"],
                "publishedSha256": _sha256(requested_manifest),
            })
            receipt = {
                "kind": "officeArtifactReceipt",
                "version": 1,
                "receiptId": receipt_id,
                "status": "published",
                "publishedAt": _utc_now(),
                "destination": str(destination),
                "destinationSha256": _sha256(destination),
                "snapshot": str(snapshot) if snapshot else None,
                "snapshotSha256": _sha256(snapshot) if snapshot else None,
                "existedBefore": destination_role["existedBefore"],
                "sidecars": sidecar_records,
                "candidateId": candidate_id,
                "requestSha256": state.get("requestSha256"),
            }
            receipt["integrity"] = self._receipt_integrity(receipt)
            write_artifact_manifest(receipt_path, receipt, self.workspace_root)
            receipt_sha256 = _sha256(receipt_path)
            state.update({
                "status": "published",
                "receiptId": receipt_id,
                "publishedAt": receipt["publishedAt"],
                "destinationSha256": receipt["destinationSha256"],
                "receiptSha256": receipt_sha256,
                "validation": validation.to_dict() if validation is not None else None,
            })
            self._write_candidate_state(state_path, state)
            journal["status"] = "committed"
            journal["committedAt"] = _utc_now()
            self._write_journal(journal_path, journal)
            journal_path.unlink(missing_ok=True)
            journal_active = False
            return outcome
        except Exception as error:
            if staged is not None:
                staged.unlink(missing_ok=True)
            if journal_active and journal_path.is_file():
                journal["pid"] = 0
                journal["status"] = "active"
                journal["error"] = f"{type(error).__name__}: {error}"
                journal["updatedAt"] = _utc_now()
                try:
                    self._write_journal(journal_path, journal)
                    recovery_status = self._recover_publish_journal(
                        journal_path,
                        journal,
                        rollback_state="candidate",
                    )
                    if recovery_status == "committed":
                        _, recovered_state = self._load_candidate(candidate_id)
                        recovered_outcome = self._candidate_outcome(recovered_state)
                        recovered_outcome.update({
                            "status": "published",
                            "path": str(destination),
                            "receiptId": recovered_state.get("receiptId"),
                            "sha256": _sha256(destination),
                            "recovery": {
                                "status": "committed",
                                "originalError": f"{type(error).__name__}: {error}",
                            },
                        })
                        journal_active = False
                        return recovered_outcome
                except Exception as recovery_error:
                    try:
                        blocked = json.loads(journal_path.read_text(encoding="utf-8"))
                        blocked["pid"] = 0
                        blocked["status"] = "recovery_blocked"
                        blocked["recoveryBlockers"] = [
                            f"{type(recovery_error).__name__}: {recovery_error}"
                        ]
                        blocked["updatedAt"] = _utc_now()
                        self._write_journal(journal_path, blocked)
                    except Exception:
                        pass
                journal_active = journal_path.exists()
            raise
        finally:
            if not journal_active:
                for lock_path in lock_paths:
                    lock_path.unlink(missing_ok=True)

    def restore(self, receipt_id: str) -> dict[str, Any]:
        if not CANDIDATE_ID_RE.fullmatch(receipt_id):
            raise OfficeArtifactError("receipt.invalid_id", "invalid receipt id")
        receipt_path = self.receipts_root / f"{receipt_id}.json"
        if not receipt_path.is_file():
            raise OfficeArtifactError("receipt.not_found", f"receipt not found: {receipt_id}")
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
        if not hmac.compare_digest(
            str(receipt.get("integrity", {}).get("value", "")),
            self._receipt_integrity(receipt)["value"],
        ):
            raise OfficeArtifactError(
                "receipt.integrity_failed",
                "receipt HMAC validation failed",
            )
        candidate_id = str(receipt.get("candidateId", ""))
        state_path, state = self._load_candidate(candidate_id)
        if (
            state.get("receiptId") != receipt_id
            or state.get("receiptSha256") != _sha256(receipt_path)
        ):
            raise OfficeArtifactError(
                "receipt.integrity_failed",
                "receipt content does not match the candidate's committed receipt hash",
            )
        if receipt.get("status") != "published":
            raise OfficeArtifactError(
                "receipt.invalid_state",
                f"receipt {receipt_id} is in state {receipt.get('status')}",
            )
        destination = workspace_path(Path(receipt["destination"]), self.workspace_root)
        sidecars = receipt.get("sidecars", [])
        if not isinstance(sidecars, list) or not all(isinstance(item, dict) for item in sidecars):
            raise OfficeArtifactError("receipt.integrity_failed", "receipt sidecars must be objects")
        role_paths = [destination] + [
            workspace_path(Path(str(record.get("path", ""))), self.workspace_root)
            for record in sidecars
        ]
        self._validate_public_role_paths(role_paths)
        lock_paths = self._acquire_role_locks(role_paths, f"restore:{receipt_id}")
        restore_journal_path = self.journals_root / f"restore-{receipt_id}.json"
        journal_active = False
        try:
            if not destination.exists() or _sha256(destination) != receipt.get("destinationSha256"):
                raise OfficeArtifactError(
                    "restore.destination_changed",
                    "destination changed after publication; refusing to overwrite newer work",
                )
            resolved_sidecars: list[tuple[dict[str, Any], Path, Path | None]] = []
            for record in sidecars:
                if not isinstance(record, dict):
                    raise OfficeArtifactError("receipt.integrity_failed", "invalid receipt sidecar record")
                sidecar = workspace_path(Path(str(record.get("path", ""))), self.workspace_root)
                expected = record.get("publishedSha256")
                if not expected or not sidecar.is_file() or _sha256(sidecar) != expected:
                    raise OfficeArtifactError(
                        "restore.sidecar_changed",
                        f"published sidecar changed after publication: {sidecar}",
                    )
                if record.get("existedBefore") and not record.get("snapshot"):
                    raise OfficeArtifactError(
                        "restore.sidecar_snapshot_missing",
                        f"cannot restore pre-existing sidecar without snapshot: {sidecar}",
                    )
                sidecar_snapshot = None
                if record.get("snapshot"):
                    sidecar_snapshot = workspace_path(
                        Path(str(record["snapshot"])),
                        self.workspace_root,
                        must_exist=True,
                    )
                    if _sha256(sidecar_snapshot) != record.get("snapshotSha256"):
                        raise OfficeArtifactError(
                            "restore.snapshot_changed",
                            f"sidecar snapshot failed SHA-256 verification: {sidecar_snapshot}",
                        )
                resolved_sidecars.append((record, sidecar, sidecar_snapshot))
            snapshot = None
            if receipt.get("snapshot"):
                snapshot = workspace_path(
                    Path(str(receipt["snapshot"])),
                    self.workspace_root,
                    must_exist=True,
                )
                if _sha256(snapshot) != receipt.get("snapshotSha256"):
                    raise OfficeArtifactError(
                        "restore.snapshot_changed",
                        f"destination snapshot failed SHA-256 verification: {snapshot}",
                    )
            if receipt.get("existedBefore") and snapshot is None:
                raise OfficeArtifactError(
                    "restore.snapshot_missing",
                    "cannot restore pre-existing destination without snapshot",
                )
            restore_roles = [
                {
                    "path": str(sidecar),
                    "publishedSha256": record["publishedSha256"],
                    "snapshot": str(sidecar_snapshot) if sidecar_snapshot else None,
                    "snapshotSha256": record.get("snapshotSha256"),
                    "restoredSha256": record.get("snapshotSha256"),
                    "existedBefore": bool(record.get("existedBefore")),
                    "restored": False,
                }
                for record, sidecar, sidecar_snapshot in reversed(resolved_sidecars)
            ]
            restore_roles.append({
                "path": str(destination),
                "publishedSha256": receipt["destinationSha256"],
                "snapshot": str(snapshot) if snapshot else None,
                "snapshotSha256": receipt.get("snapshotSha256"),
                "restoredSha256": receipt.get("snapshotSha256"),
                "existedBefore": bool(receipt.get("existedBefore")),
                "restored": False,
            })
            restore_journal = {
                "kind": "officeArtifactRestoreJournal",
                "version": 1,
                "status": "active",
                "pid": os.getpid(),
                "ownerToken": secrets.token_hex(16),
                "candidateId": candidate_id,
                "receiptId": receipt_id,
                "requestSha256": state.get("requestSha256"),
                "publishedReceiptSha256": _sha256(receipt_path),
                "destination": str(destination),
                "lockRolePaths": [str(path) for path in role_paths],
                "createdAt": _utc_now(),
                "roles": restore_roles,
            }
            self.journals_root.mkdir(parents=True, exist_ok=True)
            self._write_journal(restore_journal_path, restore_journal)
            journal_active = True
            for role in restore_roles:
                self._apply_restore_role(role)
                role["restored"] = True
                role["restoredAt"] = _utc_now()
                self._write_journal(restore_journal_path, restore_journal)
            receipt.update({"status": "restored", "restoredAt": _utc_now()})
            receipt["integrity"] = self._receipt_integrity(receipt)
            write_artifact_manifest(receipt_path, receipt, self.workspace_root)
            state.update({
                "status": "restored",
                "restoredAt": receipt["restoredAt"],
                "receiptSha256": _sha256(receipt_path),
            })
            self._write_candidate_state(state_path, state)
            restore_journal["status"] = "committed"
            restore_journal["committedAt"] = _utc_now()
            self._write_journal(restore_journal_path, restore_journal)
            restore_journal_path.unlink(missing_ok=True)
            journal_active = False
            return {
                "kind": "officeArtifactOutcome",
                "status": "restored",
                "receiptId": receipt_id,
                "path": str(destination),
                "restoredSnapshot": str(snapshot) if snapshot else None,
            }
        except Exception as error:
            if journal_active and restore_journal_path.is_file():
                # A normal I/O exception has the same partial-state risk as a
                # process crash. Attempt the signed, idempotent recovery path
                # immediately; if the underlying fault persists, the active
                # journal and locks remain for startup recovery by a dead-owner
                # process. Never perform an unjournaled best-effort rollback.
                try:
                    restore_journal["pid"] = 0
                    restore_journal["updatedAt"] = _utc_now()
                    self._write_journal(restore_journal_path, restore_journal)
                    recovery_status = self._recover_restore_journal(
                        restore_journal_path,
                        restore_journal,
                    )
                    journal_active = restore_journal_path.exists()
                    if recovery_status == "restored":
                        return {
                            "kind": "officeArtifactOutcome",
                            "status": "restored",
                            "receiptId": receipt_id,
                            "path": str(destination),
                            "restoredSnapshot": str(snapshot) if snapshot else None,
                            "recovery": {
                                "status": "committed",
                                "originalError": f"{type(error).__name__}: {error}",
                            },
                        }
                except Exception:
                    journal_active = True
            raise
        finally:
            if not journal_active:
                for lock_path in lock_paths:
                    lock_path.unlink(missing_ok=True)

    def _apply_restore_role(self, role: dict[str, Any]) -> None:
        target = workspace_path(Path(str(role["path"])), self.workspace_root)
        snapshot = (
            workspace_path(Path(str(role["snapshot"])), self.workspace_root, must_exist=True)
            if role.get("snapshot")
            else None
        )
        rollback_published_artifact(target, snapshot, self.workspace_root)

    def _recover_restore_journal(self, path: Path, journal: dict[str, Any]) -> str:
        receipt_id = str(journal.get("receiptId", ""))
        candidate_id = str(journal.get("candidateId", ""))
        if not CANDIDATE_ID_RE.fullmatch(receipt_id) or not CANDIDATE_ID_RE.fullmatch(candidate_id):
            raise OfficeArtifactError("journal.integrity_failed", "restore journal identifiers are invalid")
        receipt_path = self.receipts_root / f"{receipt_id}.json"
        if not receipt_path.is_file():
            raise OfficeArtifactError("receipt.not_found", "restore journal receipt is missing")
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
        actual_mac = str(receipt.get("integrity", {}).get("value", ""))
        if not hmac.compare_digest(actual_mac, self._receipt_integrity(receipt)["value"]):
            raise OfficeArtifactError("receipt.integrity_failed", "restore journal receipt HMAC failed")
        state_path, state = self._load_candidate(candidate_id)
        receipt_status = receipt.get("status")
        current_receipt_sha = _sha256(receipt_path)
        if (
            receipt_status not in {"published", "restored"}
            or receipt.get("candidateId") != candidate_id
            or receipt.get("destination") != journal.get("destination")
            or receipt.get("requestSha256") != state.get("requestSha256")
            or receipt.get("requestSha256") != journal.get("requestSha256")
            or state.get("receiptId") != receipt_id
        ):
            raise OfficeArtifactError(
                "receipt.integrity_failed",
                "restore journal, candidate state, and receipt binding failed",
            )
        published_receipt_sha = journal.get("publishedReceiptSha256")
        if receipt_status == "published" and (
            state.get("status") != "published"
            or current_receipt_sha != published_receipt_sha
            or state.get("receiptSha256") != published_receipt_sha
        ):
            raise OfficeArtifactError(
                "receipt.integrity_failed",
                "published receipt does not match the restore journal checkpoint",
            )
        if receipt_status == "restored" and (
            state.get("status") not in {"published", "restored"}
            or (
                state.get("status") == "published"
                and state.get("receiptSha256") != published_receipt_sha
            )
            or (
                state.get("status") == "restored"
                and state.get("receiptSha256") != current_receipt_sha
            )
        ):
            raise OfficeArtifactError(
                "receipt.integrity_failed",
                "restored receipt does not match the candidate state checkpoint",
            )
        roles = journal.get("roles")
        if not isinstance(roles, list) or not roles:
            raise OfficeArtifactError("journal.integrity_failed", "restore journal roles are invalid")
        pending: list[dict[str, Any]] = []
        blockers: list[str] = []
        for role in roles:
            if not isinstance(role, dict):
                blockers.append("invalid restore role")
                continue
            target = workspace_path(Path(str(role.get("path", ""))), self.workspace_root)
            snapshot = None
            if role.get("snapshot"):
                snapshot = workspace_path(
                    Path(str(role["snapshot"])), self.workspace_root, must_exist=True
                )
                if _sha256(snapshot) != role.get("snapshotSha256"):
                    blockers.append(f"snapshot hash mismatch: {snapshot}")
                    continue
            current_sha = _sha256(target) if target.is_file() else None
            if current_sha == role.get("publishedSha256"):
                pending.append(role)
            elif current_sha == role.get("restoredSha256") and (
                role.get("existedBefore") or current_sha is None
            ):
                role["restored"] = True
            else:
                blockers.append(f"restore target changed: {target}")
        if blockers:
            journal["pid"] = 0
            journal["status"] = "recovery_blocked"
            journal["recoveryBlockers"] = blockers
            journal["updatedAt"] = _utc_now()
            self._write_journal(path, journal)
            return "blocked"
        for role in pending:
            self._apply_restore_role(role)
            role["restored"] = True
            role["restoredAt"] = _utc_now()
            self._write_journal(path, journal)
        if receipt_status == "published":
            receipt.update({"status": "restored", "restoredAt": _utc_now()})
            receipt["integrity"] = self._receipt_integrity(receipt)
            write_artifact_manifest(receipt_path, receipt, self.workspace_root)
        state.update({
            "status": "restored",
            "restoredAt": receipt["restoredAt"],
            "receiptSha256": _sha256(receipt_path),
        })
        self._write_candidate_state(state_path, state)
        journal["status"] = "committed"
        journal["committedAt"] = _utc_now()
        self._write_journal(path, journal)
        self._remove_stale_lock(journal)
        path.unlink(missing_ok=True)
        return "restored"

    def _journal_publish_role(
        self,
        journal_path: Path,
        journal: dict[str, Any],
        staged: Path,
        target: Path,
        *,
        validate: bool,
    ) -> tuple[Path | None, Any, dict[str, Any]]:
        staged = workspace_path(staged, self.workspace_root, must_exist=True)
        target = workspace_path(target, self.workspace_root)
        if staged.parent != target.parent:
            raise OfficeArtifactError(
                "transaction.non_atomic_staging",
                "journaled publication requires staging beside the target",
            )
        validation = validate_ooxml_package(staged) if validate else None
        if validation is not None and validation.status == "fail":
            raise OfficeArtifactError(
                "validation.structural_failed",
                "staged publication failed OOXML validation",
                details={"validation": validation.to_dict()},
            )
        existed = target.exists()
        preexisting_sha = _sha256(target) if existed else None
        snapshot = snapshot_file(target, self.workspace_root)
        if existed and snapshot is None:
            raise OfficeArtifactError(
                "transaction.snapshot_missing",
                f"cannot journal publication without a snapshot: {target}",
            )
        role = {
            "path": str(target),
            "existedBefore": existed,
            "preexistingSha256": preexisting_sha,
            "snapshot": str(snapshot) if snapshot else None,
            "snapshotSha256": _sha256(snapshot) if snapshot else None,
            "intendedSha256": _sha256(staged),
            "published": False,
        }
        journal["roles"].append(role)
        self._write_journal(journal_path, journal)
        os.replace(staged, target)
        role["published"] = True
        role["publishedAt"] = _utc_now()
        self._write_journal(journal_path, journal)
        return snapshot, validation, role

    def _recover_publish_journal(
        self,
        path: Path,
        journal: dict[str, Any],
        *,
        rollback_state: str = "recovered_rolled_back",
    ) -> str:
        candidate_id = str(journal.get("candidateId", ""))
        if not CANDIDATE_ID_RE.fullmatch(candidate_id):
            raise OfficeArtifactError("journal.integrity_failed", "publish journal candidate id is invalid")
        state_path = self.candidates_root / candidate_id / "state.json"
        try:
            _, state = self._load_candidate(candidate_id)
        except OfficeArtifactError:
            state = {}
        receipt_id = str(state.get("receiptId") or journal.get("receiptId") or "")
        if state.get("status") == "published" and CANDIDATE_ID_RE.fullmatch(receipt_id):
            receipt_path = self.receipts_root / f"{receipt_id}.json"
            if receipt_path.is_file():
                receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
                actual_mac = str(receipt.get("integrity", {}).get("value", ""))
                if (
                    hmac.compare_digest(actual_mac, self._receipt_integrity(receipt)["value"])
                    and receipt.get("status") == "published"
                    and receipt.get("candidateId") == candidate_id
                    and receipt.get("destination") == journal.get("destination")
                    and state.get("receiptId") == receipt_id
                    and state.get("receiptSha256") == _sha256(receipt_path)
                    and receipt.get("requestSha256") == state.get("requestSha256")
                ):
                    self._remove_stale_lock(journal)
                    path.unlink(missing_ok=True)
                    return "committed"

        roles = journal.get("roles", [])
        if not isinstance(roles, list):
            raise ValueError("journal roles must be an array")
        recovery: list[tuple[Path, Path | None, bool]] = []
        blockers: list[str] = []
        for role in roles:
            if not isinstance(role, dict):
                blockers.append("invalid role record")
                continue
            target = workspace_path(Path(str(role.get("path", ""))), self.workspace_root)
            snapshot = None
            if role.get("snapshot"):
                snapshot = workspace_path(
                    Path(str(role["snapshot"])),
                    self.workspace_root,
                    must_exist=True,
                )
                if _sha256(snapshot) != role.get("snapshotSha256"):
                    blockers.append(f"snapshot hash mismatch: {snapshot}")
                    continue
            current_sha = _sha256(target) if target.is_file() else None
            if current_sha == role.get("intendedSha256"):
                recovery.append((target, snapshot, True))
            elif (
                bool(role.get("existedBefore"))
                and current_sha == role.get("preexistingSha256")
            ) or (not role.get("existedBefore") and current_sha is None):
                recovery.append((target, snapshot, False))
            else:
                blockers.append(f"target changed during recovery: {target}")
        if blockers:
            journal["pid"] = 0
            journal["status"] = "recovery_blocked"
            journal["recoveryBlockers"] = blockers
            journal["updatedAt"] = _utc_now()
            self._write_journal(path, journal)
            return "blocked"
        for target, snapshot, should_restore in reversed(recovery):
            if should_restore:
                rollback_published_artifact(target, snapshot, self.workspace_root)
        if state:
            state.update({"status": rollback_state, "updatedAt": _utc_now()})
            self._write_candidate_state(state_path, state)
        if CANDIDATE_ID_RE.fullmatch(receipt_id):
            receipt_path = self.receipts_root / f"{receipt_id}.json"
            if receipt_path.is_file():
                receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
                receipt.update({"status": rollback_state, "rolledBackAt": _utc_now()})
                receipt["integrity"] = self._receipt_integrity(receipt)
                write_artifact_manifest(receipt_path, receipt, self.workspace_root)
        self._remove_stale_lock(journal)
        path.unlink(missing_ok=True)
        return "rolled_back"

    def _recover_incomplete_journals(self) -> None:
        if not self.journals_root.is_dir():
            return
        for path in sorted(self.journals_root.glob("*.json")):
            try:
                journal = json.loads(path.read_text(encoding="utf-8"))
                if journal.get("kind") not in {
                    "officeArtifactPublishJournal",
                    "officeArtifactRestoreJournal",
                }:
                    continue
                actual_mac = str(journal.get("integrity", {}).get("value", ""))
                if not hmac.compare_digest(actual_mac, self._journal_integrity(journal)["value"]):
                    path.rename(path.with_name(f"{path.name}.invalid-{uuid.uuid4().hex}.quarantine"))
                    continue
                status = journal.get("status")
                if status in {"committed", "rolled_back", "recovered_rolled_back"}:
                    path.unlink(missing_ok=True)
                    continue
                if status not in {"active", "recovery_blocked"}:
                    path.rename(path.with_name(f"{path.name}.invalid-{uuid.uuid4().hex}.quarantine"))
                    continue
                pid = int(journal.get("pid", 0) or 0)
                if status == "active" and pid and _process_is_alive(pid):
                    continue
                if journal.get("kind") == "officeArtifactRestoreJournal":
                    self._recover_restore_journal(path, journal)
                    continue
                self._recover_publish_journal(path, journal)
            except Exception as error:  # noqa: BLE001
                try:
                    journal = json.loads(path.read_text(encoding="utf-8"))
                    journal["pid"] = 0
                    journal["status"] = "recovery_blocked"
                    journal["recoveryBlockers"] = [f"{type(error).__name__}: {error}"]
                    journal["updatedAt"] = _utc_now()
                    self._write_journal(path, journal)
                except Exception:
                    continue

    def _remove_stale_lock(self, journal: dict[str, Any]) -> None:
        raw_paths = journal.get("lockRolePaths") or [journal.get("destination")]
        for raw_path in raw_paths:
            destination = workspace_path(Path(str(raw_path)), self.workspace_root)
            key = hashlib.sha256(str(destination).casefold().encode("utf-8")).hexdigest()[:24]
            lock_path = self.locks_root / f"{key}.lock"
            if not lock_path.is_file():
                continue
            try:
                lock = json.loads(lock_path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                continue
            actual_mac = str(lock.get("integrity", {}).get("value", ""))
            if not hmac.compare_digest(actual_mac, self._lock_integrity(lock)["value"]):
                continue
            if lock.get("candidateId") in {
                journal.get("candidateId"),
                f"restore:{journal.get('receiptId')}",
            }:
                lock_path.unlink(missing_ok=True)

    def _receipt_integrity(self, receipt: dict[str, Any]) -> dict[str, str]:
        payload = {key: value for key, value in receipt.items() if key != "integrity"}
        encoded = json.dumps(
            payload,
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
        return {
            "algorithm": "HMAC-SHA256",
            "value": hmac.new(self.integrity_key, encoded, hashlib.sha256).hexdigest(),
        }

    def _state_integrity(self, state: dict[str, Any]) -> dict[str, str]:
        payload = {key: value for key, value in state.items() if key != "integrity"}
        encoded = json.dumps(
            payload,
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
        return {
            "algorithm": "HMAC-SHA256",
            "value": hmac.new(self.integrity_key, encoded, hashlib.sha256).hexdigest(),
        }

    def _write_candidate_state(self, path: Path, state: dict[str, Any]) -> None:
        state["integrity"] = self._state_integrity(state)
        write_artifact_manifest(path, state, self.workspace_root)

    def _journal_integrity(self, journal: dict[str, Any]) -> dict[str, str]:
        payload = {key: value for key, value in journal.items() if key != "integrity"}
        encoded = json.dumps(
            payload,
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
        return {
            "algorithm": "HMAC-SHA256",
            "value": hmac.new(self.integrity_key, encoded, hashlib.sha256).hexdigest(),
        }

    def _write_journal(self, path: Path, journal: dict[str, Any]) -> None:
        journal["integrity"] = self._journal_integrity(journal)
        write_artifact_manifest(path, journal, self.workspace_root)

    def _lock_integrity(self, lock: dict[str, Any]) -> dict[str, str]:
        payload = {key: value for key, value in lock.items() if key != "integrity"}
        encoded = json.dumps(
            payload,
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
        return {
            "algorithm": "HMAC-SHA256",
            "value": hmac.new(self.integrity_key, encoded, hashlib.sha256).hexdigest(),
        }

    def _recover_orphan_locks(self) -> None:
        if not self.locks_root.is_dir():
            return
        protected_locks: set[Path] = set()
        if self.journals_root.is_dir():
            for journal_path in self.journals_root.glob("*.json"):
                try:
                    journal = json.loads(journal_path.read_text(encoding="utf-8"))
                    if journal.get("kind") not in {
                        "officeArtifactPublishJournal",
                        "officeArtifactRestoreJournal",
                    } or journal.get("status") not in {"active", "recovery_blocked"}:
                        continue
                    actual_mac = str(journal.get("integrity", {}).get("value", ""))
                    if not hmac.compare_digest(
                        actual_mac,
                        self._journal_integrity(journal)["value"],
                    ):
                        continue
                    raw_paths = journal.get("lockRolePaths") or [journal.get("destination")]
                    for raw_path in raw_paths:
                        destination = workspace_path(Path(str(raw_path)), self.workspace_root)
                        key = hashlib.sha256(
                            str(destination).casefold().encode("utf-8")
                        ).hexdigest()[:24]
                        protected_locks.add(self.locks_root / f"{key}.lock")
                except (OSError, json.JSONDecodeError, TypeError, ValueError, OfficeArtifactError):
                    continue
        for path in self.locks_root.glob("*.lock"):
            try:
                lock = json.loads(path.read_text(encoding="utf-8"))
                actual_mac = str(lock.get("integrity", {}).get("value", ""))
                if not hmac.compare_digest(actual_mac, self._lock_integrity(lock)["value"]):
                    path.rename(path.with_name(f"{path.name}.invalid-{uuid.uuid4().hex}.quarantine"))
                    continue
                if path in protected_locks:
                    continue
                if not _process_is_alive(int(lock.get("pid", 0) or 0)):
                    path.unlink(missing_ok=True)
            except (OSError, json.JSONDecodeError, TypeError, ValueError):
                continue

    def _acquire_destination_lock(self, destination: Path, candidate_id: str) -> Path:
        self.locks_root.mkdir(parents=True, exist_ok=True)
        key = hashlib.sha256(str(destination).casefold().encode("utf-8")).hexdigest()[:24]
        lock_path = self.locks_root / f"{key}.lock"
        try:
            descriptor = os.open(lock_path, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
        except FileExistsError as error:
            detail = lock_path.read_text(encoding="utf-8", errors="replace") if lock_path.exists() else ""
            raise OfficeArtifactError(
                "publish.conflict",
                "another Office artifact publication owns the destination lock",
                retryable=True,
                details={"destination": str(destination), "lock": detail},
            ) from error
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            lock = {
                "kind": "officeArtifactDestinationLock",
                "candidateId": candidate_id,
                "destination": str(destination),
                "pid": os.getpid(),
                "createdAt": _utc_now(),
                "ownerToken": secrets.token_hex(16),
            }
            lock["integrity"] = self._lock_integrity(lock)
            json.dump(lock, handle, ensure_ascii=False)
        return lock_path

    def _acquire_role_locks(self, paths: list[Path], owner_id: str) -> list[Path]:
        resolved = sorted(
            {workspace_path(path, self.workspace_root) for path in paths},
            key=lambda path: str(path).casefold(),
        )
        acquired: list[Path] = []
        try:
            for path in resolved:
                acquired.append(self._acquire_destination_lock(path, owner_id))
            return acquired
        except Exception:
            for lock in acquired:
                lock.unlink(missing_ok=True)
            raise

    def _validate_public_role_paths(self, paths: list[Path]) -> None:
        canonical = [workspace_path(path, self.workspace_root) for path in paths]
        if len(set(canonical)) != len(canonical):
            raise OfficeArtifactError("path.role_conflict", "publication roles must be distinct")
        reserved = (self.workspace_root / ".nexa").resolve()
        for path in canonical:
            if path == reserved or reserved in path.parents:
                raise OfficeArtifactError(
                    "path.internal_state_conflict",
                    f"publication role cannot target Nexa's reserved .nexa state: {path}",
                )

    def _assert_publish_role_preconditions(
        self,
        state: dict[str, Any],
        paths: list[Path],
    ) -> None:
        records = state.get("publishRoleBases")
        if not isinstance(records, list):
            raise OfficeArtifactError("candidate.invalid_state_file", "publish role bases are missing")
        by_path = {
            str(workspace_path(Path(str(record.get("path", ""))), self.workspace_root)): record
            for record in records
            if isinstance(record, dict)
        }
        expected_paths = {str(workspace_path(path, self.workspace_root)) for path in paths}
        if set(by_path) != expected_paths:
            raise OfficeArtifactError("candidate.invalid_state_file", "publish role graph changed")
        for path_text, record in by_path.items():
            path = Path(path_text)
            existed = bool(record.get("existed"))
            if path.exists() != existed:
                raise OfficeArtifactError(
                    "publish.role_changed",
                    f"publication role existence changed after execute: {path}",
                    retryable=True,
                )
            if existed and _sha256(path) != record.get("sha256"):
                raise OfficeArtifactError(
                    "publish.role_changed",
                    f"publication role content changed after execute: {path}",
                    retryable=True,
                )

    def _assert_destination_precondition(
        self,
        state: dict[str, Any],
        destination: Path,
    ) -> None:
        existed = bool(state.get("destinationExistedAtExecute"))
        if destination.exists() != existed:
            raise OfficeArtifactError(
                "publish.destination_changed",
                "destination existence changed after candidate execution",
                retryable=True,
            )
        if existed:
            current = _sha256(destination)
            if current != state.get("destinationBaseSha256"):
                raise OfficeArtifactError(
                    "publish.destination_changed",
                    "destination content changed after candidate execution",
                    retryable=True,
                    details={
                        "expectedSha256": state.get("destinationBaseSha256"),
                        "actualSha256": current,
                    },
                )

    def _backend_for(self, request: ArtifactRequest) -> str:
        return str(self._adapter_plan(request)["primaryAdapter"])

    def _adapter_plan(self, request: ArtifactRequest) -> dict[str, Any]:
        pptxgenjs_author = (
            request.format == "pptx"
            and request.intent == "create"
            and any(
                str(item.get("authorEngine", "")) == "pptxgenjs"
                for item in request.operations
            )
        )
        steps: list[dict[str, Any]] = [{
            "step": "format",
            "adapter": "pptxgenjs" if pptxgenjs_author else "nexa-openxml",
            "operations": [str(item.get("op", "")).lower() for item in request.operations],
            "guarantee": request.preservation,
        }]
        if request.calculation == "compatible":
            steps.append({
                "step": "calculation",
                "adapter": "libreoffice",
                "operations": ["recalculate"],
                "guarantee": "compatible",
            })
        elif request.calculation == "native":
            steps.append({
                "step": "calculation",
                "adapter": "windows-com",
                "operations": ["recalculate"],
                "guarantee": "native",
            })
        elif request.format == "xlsx":
            steps.append({
                "step": "calculation",
                "adapter": "nexa-openxml",
                "operations": ["formula-inventory", "cache-inspection"],
                "guarantee": request.calculation,
            })
        if request.quality == "native" and not any(
            step["adapter"] == "windows-com" for step in steps
        ):
            steps.append({
                "step": "finalize",
                "adapter": "windows-com",
                "operations": ["native-open-save"],
                "guarantee": "native",
            })
        if request.render != "none":
            native_render = request.quality == "native" or request.calculation == "native"
            steps.append({
                "step": "render",
                "adapter": "windows-com" if native_render else "libreoffice",
                "operations": ["render"],
                "guarantee": request.render,
            })
        steps.append({
            "step": "validate",
            "adapter": "nexa-openxml",
            "operations": ["opc", "semantic-contract", "allowed-diff"],
            "guarantee": request.quality,
        })
        if request.quality in {"publish", "native"}:
            steps.append({
                "step": "schema-validate",
                "adapter": "openxml-sdk",
                "operations": ["OpenXmlValidator"],
                "guarantee": "microsoft365-schema",
            })
        required = sorted({str(step["adapter"]) for step in steps})
        primary = next(
            (
                str(step["adapter"])
                for step in steps
                if step["step"] in {"finalize", "calculation"}
                and step["adapter"] != "nexa-openxml"
            ),
            "nexa-openxml",
        )
        return {
            "solverVersion": 1,
            "primaryAdapter": primary,
            "requiredAdapters": required,
            "steps": steps,
        }

    def _xlsx_formula_profile(self, path: Path) -> dict[str, Any]:
        skills_root = Path(__file__).resolve().parents[2]
        renderer_dir = skills_root / "xlsx-workbook-design" / "scripts"
        if str(renderer_dir) not in sys.path:
            sys.path.insert(0, str(renderer_dir))
        from xlsx_model_renderer import inspect_formula_inventory  # type: ignore

        inventory = inspect_formula_inventory(path)
        native_features: set[str] = set()
        dynamic_functions = {
            "FILTER", "SORT", "SORTBY", "UNIQUE", "SEQUENCE", "RANDARRAY",
            "XLOOKUP", "XMATCH", "LET", "LAMBDA", "TAKE", "DROP", "CHOOSECOLS", "CHOOSEROWS",
        }
        for formula in inventory["formulas"]:
            text = str(formula.get("formula", "")).upper()
            for function in dynamic_functions:
                if re.search(rf"(?:_XLFN\.)?{function}\s*\(", text):
                    native_features.add(f"function:{function}")
            if "#" in text:
                native_features.add("spill-reference")
            formula_type = str(formula.get("type", "normal"))
            if formula_type in {"array", "dataTable"}:
                native_features.add(f"formula-type:{formula_type}")
        return {
            "formulaCells": inventory["formulaCells"],
            "formulaKinds": inventory["formulaKinds"],
            "fingerprint": inventory["fingerprint"],
            "requiresExcelNative": bool(native_features),
            "nativeFeatures": sorted(native_features),
        }

    def _execution_plan_payload(
        self,
        request: ArtifactRequest,
        candidate_path: Path,
        manifest: Path,
    ) -> dict[str, Any]:
        operations = [dict(operation) for operation in request.operations]
        if request.format == "pptx":
            for operation in operations:
                if operation.get("htmlFirst"):
                    operation["outdir"] = str(candidate_path.parent / "html-project")
        if request.calculation == "compatible" and not any(
            str(operation.get("op", "")).lower() == "recalculate"
            for operation in operations
        ):
            operations.append({"op": "recalculate"})
        return {
            "planVersion": 1,
            "format": request.format,
            "intent": "create_new" if request.intent == "create" else "edit_existing",
            "input": str(request.source) if request.source else None,
            "output": str(candidate_path),
            "operations": operations,
            "preservationPolicy": request.preservation,
            "validationContract": request.validation,
            "renderPolicy": (
                "none"
                if request.quality == "native" or request.calculation == "native"
                else request.render
            ),
            # The local adapter performs the structural edit/create first. Its
            # guarded recalculation step then invokes LibreOffice when the
            # compatible guarantee is requested, including create workflows.
            "backend": "nexa-openxml",
            "manifest": str(manifest),
        }

    def _native_finalize(
        self,
        request: ArtifactRequest,
        candidate_path: Path,
        candidate_dir: Path,
        execution: dict[str, Any],
    ) -> dict[str, Any]:
        native_manifest = candidate_dir / "native-finalization.json"
        native_payload = {
            "planVersion": 1,
            "format": request.format,
            "intent": "finalize",
            "input": str(candidate_path),
            "output": str(candidate_path),
            "operations": [],
            "preservationPolicy": (
                "balanced" if request.intent == "create" else request.preservation
            ),
            "validationContract": request.validation,
            "renderPolicy": request.render,
            "backend": "windows-com",
            "manifest": str(native_manifest),
        }
        native, exit_code = execute_plan(
            OfficeExecutionPlan.from_internal_dict(native_payload, self.workspace_root),
            self.workspace_root,
        )
        if exit_code != 0 or not native.get("ok"):
            raise OfficeArtifactError(
                "native.finalization_failed",
                str(native.get("error") or "native Office finalization failed"),
                details={"native": native},
            )
        return {"openXml": execution, "native": native}

    def _load_candidate(self, candidate_id: str) -> tuple[Path, dict[str, Any]]:
        if not CANDIDATE_ID_RE.fullmatch(candidate_id):
            raise OfficeArtifactError("candidate.invalid_id", "invalid candidate id")
        state_path = self.candidates_root / candidate_id / "state.json"
        if not state_path.is_file():
            raise OfficeArtifactError("candidate.not_found", f"candidate not found: {candidate_id}")
        state = json.loads(state_path.read_text(encoding="utf-8"))
        if state.get("candidateId") != candidate_id or state.get("kind") != "officeArtifactCandidateState":
            raise OfficeArtifactError("candidate.invalid_state_file", "candidate state marker is invalid")
        actual_mac = str(state.get("integrity", {}).get("value", ""))
        if not hmac.compare_digest(actual_mac, self._state_integrity(state)["value"]):
            raise OfficeArtifactError(
                "candidate.integrity_failed",
                "candidate state HMAC validation failed",
            )
        return state_path, state

    def _candidate_outcome(self, state: dict[str, Any]) -> dict[str, Any]:
        execution = state.get("execution", {})
        openxml_execution = execution.get("openXml", execution) if isinstance(execution, dict) else {}
        native_execution = execution.get("native") if isinstance(execution, dict) else None
        calculation = self._calculation_evidence(openxml_execution, native_execution)
        return {
            "kind": "officeArtifactOutcome",
            "requestVersion": REQUEST_VERSION,
            "status": state.get("status"),
            "candidateId": state["candidateId"],
            "candidatePath": state["candidatePath"],
            "destination": state["destination"],
            "sha256": state.get("candidateSha256"),
            "assessment": state.get("assessment"),
            "validation": (
                native_execution.get("validation")
                if isinstance(native_execution, dict)
                else openxml_execution.get("validation")
            ),
            "preservationEvidence": openxml_execution.get("preservationEvidence"),
            "calculationEvidence": calculation,
            "nativeEvidence": self._native_host_evidence(native_execution),
            "renderedPreviews": (
                native_execution.get("renderedPreviews", [])
                if isinstance(native_execution, dict)
                else openxml_execution.get("renderedPreviews", [])
            ),
            "renderEvidence": state.get("renderEvidence"),
            "schemaValidation": execution.get("schemaValidation"),
            "syntheticPreview": execution.get("syntheticPreview"),
            "warnings": list(openxml_execution.get("warnings", [])) + (
                list(native_execution.get("warnings", [])) if isinstance(native_execution, dict) else []
            ),
        }

    def _calculation_evidence(
        self,
        openxml_execution: dict[str, Any],
        native_execution: dict[str, Any] | None,
    ) -> dict[str, Any] | None:
        if isinstance(native_execution, dict) and native_execution.get("format") == "xlsx":
            native_action = next(
                (
                    action for action in native_execution.get("actions", [])
                    if action.get("command") == "windows-com-finalize"
                ),
                {},
            )
            return {
                "level": "native",
                "engine": "microsoft-excel-com",
                "engineVersion": native_action.get("engineVersion"),
                "profile": "excel-native",
                "excelNative": True,
                "macros": native_action.get("macros", "force-disabled"),
                "externalLinks": native_action.get("externalLinks", "update-disabled"),
                "calculationState": native_action.get("calculationState"),
                "cacheEvidence": native_action.get("cacheEvidence"),
                "cachedErrors": native_action.get("cachedErrors"),
                "formulaFingerprintBefore": native_action.get("formulaFingerprintBefore"),
                "formulaFingerprintAfter": native_action.get("formulaFingerprintAfter"),
            }
        for action in openxml_execution.get("actions", []):
            if action.get("command") != "recalc_xlsx" or not action.get("stdout"):
                continue
            try:
                recalculation = json.loads(action["stdout"])
            except json.JSONDecodeError:
                continue
            if isinstance(recalculation.get("calculation"), dict):
                return recalculation["calculation"]
        validation = openxml_execution.get("validation")
        if isinstance(validation, dict):
            backend = validation.get("backend", validation)
            if isinstance(backend, dict):
                formula_qa = backend.get("formulaQa")
                if isinstance(formula_qa, dict) and isinstance(formula_qa.get("calculation"), dict):
                    evidence = dict(formula_qa["calculation"])
                    evidence.setdefault("engine", "none")
                    evidence.setdefault("profile", "static")
                    evidence.setdefault("excelNative", False)
                    return evidence
                if isinstance(backend.get("calculation"), dict):
                    evidence = dict(backend["calculation"])
                    evidence.setdefault("engine", "none")
                    evidence.setdefault("profile", "static")
                    evidence.setdefault("excelNative", False)
                    return evidence
        return None

    def _native_host_evidence(
        self,
        native_execution: dict[str, Any] | None,
    ) -> dict[str, Any] | None:
        if not isinstance(native_execution, dict):
            return None
        finalize = next(
            (
                action for action in native_execution.get("actions", [])
                if action.get("command") == "windows-com-finalize"
            ),
            None,
        )
        if not isinstance(finalize, dict):
            return None
        return {
            "kind": "officeNativeEvidence",
            "engine": finalize.get("engine"),
            "engineVersion": finalize.get("engineVersion"),
            "nativeOpenSave": bool(finalize.get("nativeOpenSave")),
            "macros": finalize.get("macros"),
            "externalLinks": finalize.get("externalLinks"),
        }

    def _render_evidence(
        self,
        candidate_path: Path,
        execution: dict[str, Any],
        artifact_format: str,
        render_policy: str,
    ) -> dict[str, Any]:
        openxml_execution = execution.get("openXml", execution) if isinstance(execution, dict) else {}
        native_execution = execution.get("native") if isinstance(execution, dict) else None
        source = native_execution if isinstance(native_execution, dict) else openxml_execution
        previews = [Path(str(value)) for value in source.get("renderedPreviews", [])]
        candidate_root = candidate_path.parent.resolve()
        files: list[dict[str, Any]] = []
        outside: list[str] = []
        for preview in previews:
            resolved = preview.resolve()
            try:
                resolved.relative_to(candidate_root)
            except ValueError:
                outside.append(str(resolved))
                continue
            if resolved.is_file():
                files.append({
                    "path": str(resolved),
                    "sha256": _sha256(resolved),
                    "bytes": resolved.stat().st_size,
                })
        expected_surfaces: int | None = None
        rendered_surfaces = len(files)
        surface_manifest: dict[str, Any] | None = None
        if artifact_format == "pptx":
            validation = source.get("validation")
            if isinstance(validation, dict):
                backend = validation.get("backend", validation)
                if isinstance(backend, dict):
                    package_graph = backend.get("packageGraph")
                    if isinstance(package_graph, dict):
                        expected_surfaces = int(package_graph.get("slides", 0))
                    elif isinstance(backend.get("backend"), dict):
                        expected_surfaces = int(backend["backend"].get("slides", 0))
        elif artifact_format == "xlsx" and previews:
            manifest_path = previews[0].parent / "render-manifest.json"
            if manifest_path.is_file():
                try:
                    surface_manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
                    expected_surfaces = int(surface_manifest.get("expectedSurfaces", 0))
                    rendered_surfaces = int(surface_manifest.get("renderedSurfaces", 0))
                except (OSError, json.JSONDecodeError, TypeError, ValueError):
                    surface_manifest = None
        if render_policy == "none":
            complete = not files and not outside
        elif artifact_format == "pptx" and expected_surfaces is not None:
            complete = len(files) == expected_surfaces and not outside and expected_surfaces > 0
        elif artifact_format == "xlsx":
            complete = bool(
                surface_manifest
                and surface_manifest.get("complete")
                and surface_manifest.get("artifactSha256") == _sha256(candidate_path)
                and expected_surfaces
                and rendered_surfaces == expected_surfaces
                and not outside
            )
        else:
            complete = bool(files) and not outside
        visual_qa: dict[str, Any] | None = None
        if files:
            from office_visual_qa import analyze_rendered_images

            visual_qa = analyze_rendered_images([Path(item["path"]) for item in files])
            complete = complete and visual_qa.get("status") in {"pass", "warn"}
        return {
            "kind": "officeRenderEvidence",
            "policy": render_policy,
            "artifactSha256": _sha256(candidate_path),
            "format": artifact_format,
            "expectedSurfaces": expected_surfaces,
            "renderedSurfaces": rendered_surfaces,
            "complete": complete,
            "files": files,
            "outsideCandidatePaths": outside,
            "renderer": (
                next(
                    (
                        renderer
                        for command, renderer in (
                            ("windows-com-render-docx", "microsoft-word-native"),
                            ("windows-com-render-xlsx", "microsoft-excel-native"),
                            ("windows-com-render-pptx", "microsoft-powerpoint-native"),
                        )
                        if any(
                            action.get("command") == command
                            for action in source.get("actions", [])
                        )
                    ),
                    "libreoffice-compatible" if files else None,
                )
            ),
            "surfaceManifest": surface_manifest,
            "visualQa": visual_qa,
        }

    def _external_adapter_declarations(self) -> list[dict[str, Any]]:
        root = self.workspace_root / ".nexa" / "office-adapters"
        if not root.is_dir():
            return []
        declarations: list[dict[str, Any]] = []
        allowed = {
            "adapterVersion", "id", "deployment", "formats", "operations", "guarantees",
            "limitations", "requires",
        }
        seen: set[str] = set()
        for path in sorted(root.glob("*.json")):
            try:
                payload = json.loads(path.read_text(encoding="utf-8"))
                if not isinstance(payload, dict):
                    raise ValueError("manifest root must be an object")
                unknown = sorted(set(payload) - allowed)
                if unknown:
                    raise ValueError("unknown fields: " + ", ".join(unknown))
                if int(payload.get("adapterVersion", 0)) != 1:
                    raise ValueError("adapterVersion must be 1")
                adapter_id = str(payload.get("id", ""))
                if not re.fullmatch(r"[a-z0-9][a-z0-9-]*", adapter_id):
                    raise ValueError("invalid adapter id")
                if adapter_id in seen:
                    raise ValueError("duplicate adapter id")
                seen.add(adapter_id)
                declarations.append({
                    **payload,
                    "manifestPath": str(path),
                    "status": "declared-not-loaded",
                })
            except (OSError, json.JSONDecodeError, ValueError) as error:
                declarations.append({
                    "manifestPath": str(path),
                    "status": "invalid",
                    "error": str(error),
                })
        return declarations


def _utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


def _process_is_alive(pid: int) -> bool:
    if pid <= 0:
        return False
    if pid == os.getpid():
        return True
    if os.name == "nt":
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.OpenProcess.argtypes = [ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
        kernel32.OpenProcess.restype = ctypes.c_void_p
        kernel32.GetExitCodeProcess.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_ulong)]
        kernel32.GetExitCodeProcess.restype = ctypes.c_int
        kernel32.CloseHandle.argtypes = [ctypes.c_void_p]
        kernel32.CloseHandle.restype = ctypes.c_int
        process = kernel32.OpenProcess(0x1000 | 0x00100000, False, pid)
        if not process:
            return ctypes.get_last_error() != 87  # invalid PID is dead; access denied is live
        try:
            exit_code = ctypes.c_ulong()
            if not kernel32.GetExitCodeProcess(process, ctypes.byref(exit_code)):
                return True
            return exit_code.value == 259  # STILL_ACTIVE
        finally:
            kernel32.CloseHandle(process)
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except (PermissionError, OSError):
        return True
    return True


def _office_integrity_root() -> Path:
    configured = os.environ.get("NEXA_OFFICE_INTEGRITY_ROOT")
    if configured:
        return Path(configured).expanduser().resolve()
    if os.name == "nt" and os.environ.get("LOCALAPPDATA"):
        return Path(os.environ["LOCALAPPDATA"]) / "Nexa" / "office-artifact-integrity"
    data_home = os.environ.get("XDG_DATA_HOME")
    return (
        Path(data_home).expanduser() / "nexa" / "office-artifact-integrity"
        if data_home
        else Path.home() / ".local" / "share" / "nexa" / "office-artifact-integrity"
    )


def _load_or_create_integrity_key(root: Path) -> bytes:
    root.mkdir(parents=True, exist_ok=True)
    key_path = root / "receipt-hmac.key"
    try:
        descriptor = os.open(key_path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    except FileExistsError:
        key = b""
        for _ in range(100):
            try:
                key = key_path.read_bytes()
            except OSError:
                key = b""
            if len(key) == 32:
                break
            time.sleep(0.02)
    else:
        key = secrets.token_bytes(32)
        try:
            os.write(descriptor, key)
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
    if len(key) != 32:
        raise OfficeArtifactError(
            "integrity.key_invalid",
            f"Office receipt integrity key must be 32 bytes: {key_path}",
        )
    return key


def _json_sha256(value: Any) -> str:
    encoded = json.dumps(
        value,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _reject_unknown_keys(value: dict[str, Any], allowed: set[str], location: str) -> None:
    unknown = sorted(set(value) - allowed)
    if unknown:
        raise OfficeArtifactError(
            "schema.unknown_field",
            f"unknown field(s) at {location}: {', '.join(unknown)}",
            details={"location": location, "unknownFields": unknown},
        )


def _validate_operation(artifact_format: str, operation: dict[str, Any], index: int) -> None:
    if not isinstance(operation.get("op"), str):
        raise OfficeArtifactError("schema.operation_type", f"operations[{index}].op must be a string")
    name = operation["op"].lower()
    allowed = COMMON_OPERATION_KEYS | OPERATION_KEYS[artifact_format].get(name, set())
    _reject_unknown_keys(operation, allowed, f"operations[{index}]")
    required_keys = set(REQUIRED_OPERATION_KEYS.get(name, set()))
    if artifact_format == "pptx" and name == "add_comment":
        required_keys = {"comment"}
    missing = sorted(
        key for key in required_keys
        if key not in operation
    )
    if name == "create" and not any(key in operation for key in ("spec", "body", "inputMd", "prompt")):
        missing.append("spec/body/inputMd/prompt")
    if name in {"replace", "redact"} and not operation.get("find"):
        missing.append("find")
    if name == "insert_field":
        instruction = " ".join(str(operation.get("instruction", "")).split())
        if instruction and SAFE_FIELD_INSTRUCTION_RE.fullmatch(instruction) is None:
            raise OfficeArtifactError(
                "schema.unsafe_field_instruction",
                "insert_field instruction must match the PAGE/NUMPAGES/SECTIONPAGES/TOC/REF/SEQ allowlist",
                details={"location": f"operations[{index}].instruction"},
            )
    if artifact_format != "docx" and name in {"replace", "redact"}:
        unsupported_controls = sorted(
            field
            for field in {"scope", "occurrence", "allowStyleMerge"}
            if field in operation
        )
        if unsupported_controls:
            raise OfficeArtifactError(
                "schema.unsupported_field",
                f"{artifact_format} {name} does not support DOCX-only control field(s): "
                + ", ".join(unsupported_controls),
                details={
                    "location": f"operations[{index}]",
                    "unsupportedFields": unsupported_controls,
                },
            )
    if artifact_format == "pptx" and name in {
        "set_text", "clone_slide", "set_transition", "set_alt_text",
        "set_speaker_notes", "add_comment",
    } and not any(key in operation for key in ("slideId", "slideIndex")):
        missing.append("slideId/slideIndex")
    if artifact_format == "pptx" and name in {"set_text", "set_alt_text"} and not any(
        key in operation for key in ("shapeId", "shapeName")
    ):
        missing.append("shapeId/shapeName")
    if name in {"set_style", "set_number_format"} and not any(
        key in operation for key in ("cell", "range")
    ):
        missing.append("cell/range")
    if missing:
        raise OfficeArtifactError(
            "schema.missing_field",
            f"missing required field(s) at operations[{index}]: {', '.join(missing)}",
        )
    boolean_fields = {"allowStyleMerge", "privacyScrub", "htmlFirst"}
    integer_fields = {"expectedMatches", "occurrence", "slideIndex", "afterIndex", "after", "styleId", "baseStyleId", "x", "y"}
    string_fields = {
        "elementId", "spec", "title", "subtitle", "body", "font", "footer", "author",
        "inputMd", "template", "find", "replace", "expectedSha256", "scope", "comment",
        "initials", "date", "sheet", "cell", "range", "formula", "shapeName", "text",
        "transition", "speed", "direction", "outdir", "mode", "screenshot", "prompt",
        "bookmarkName", "instruction", "displayText", "tag", "lock",
        "newName", "name", "scopeSheet", "validationType", "operator", "formula1", "formula2",
        "errorTitle", "error", "styleName", "formatCode", "chartPart",
        "altText",
        "authorEngine",
    }
    boolean_fields.update({"allowBlank", "showErrorMessage"})
    for field in boolean_fields & set(operation):
        if type(operation[field]) is not bool:
            raise OfficeArtifactError(
                "schema.operation_type",
                f"operations[{index}].{field} must be a boolean",
            )
    for field in integer_fields & set(operation):
        value = operation[field]
        minimum = 1 if field in {"occurrence", "slideIndex"} else 0
        if type(value) is not int or value < minimum:
            raise OfficeArtifactError(
                "schema.operation_type",
                f"operations[{index}].{field} must be an integer >= {minimum}",
            )
    for field in string_fields & set(operation):
        if field == "scope" and isinstance(operation[field], list):
            if all(isinstance(item, str) for item in operation[field]):
                continue
        if not isinstance(operation[field], str):
            raise OfficeArtifactError(
                "schema.operation_type",
                f"operations[{index}].{field} must be a string",
            )
    for field in {"slideId", "shapeId"} & set(operation):
        if type(operation[field]) not in {int, str}:
            raise OfficeArtifactError(
                "schema.operation_type",
                f"operations[{index}].{field} must be a string or integer",
            )
    if "order" in operation and (
        not isinstance(operation["order"], list)
        or not all(type(item) in {int, str} for item in operation["order"])
    ):
        raise OfficeArtifactError(
            "schema.operation_type",
            f"operations[{index}].order must be an array of slide ids",
        )
    if "values" in operation and (
        not isinstance(operation["values"], list)
        or not all(isinstance(row, list) for row in operation["values"])
    ):
        raise OfficeArtifactError(
            "schema.operation_type",
            f"operations[{index}].values must be a two-dimensional array",
        )
    if "columns" in operation and (
        not isinstance(operation["columns"], list)
        or not all(isinstance(item, str) and item for item in operation["columns"])
    ):
        raise OfficeArtifactError(
            "schema.operation_type",
            f"operations[{index}].columns must be an array of non-empty strings",
        )
    if "expectedSha256" in operation and not re.fullmatch(
        r"[0-9A-Fa-f]{64}", operation["expectedSha256"]
    ):
        raise OfficeArtifactError(
            "schema.operation_type",
            f"operations[{index}].expectedSha256 must be a SHA-256 hex digest",
        )


CONTRACT_KEYS: dict[str, set[str]] = {
    "docx": {
        "contractVersion", "required_text", "forbidden_text", "min_paragraphs", "min_tables",
        "required_styles", "no_heading_level_skips", "require_alt_text",
        "require_table_header_rows", "require_fixed_table_layout", "required_language",
        "min_comments", "require_tracked_changes", "require_no_tracked_changes",
    },
    "pptx": {
        "contractVersion", "required_text", "forbidden_text", "min_slides", "max_slides",
        "required_slide_titles", "require_speaker_notes",
    },
    "xlsx": {
        "contractVersion", "required_sheets", "required_named_ranges", "no_numeric_hardcodes_in",
        "min_rows", "sentinels", "require_formula_cache", "tie_outs", "reconciliations",
        "formula_patterns", "required_provenance",
    },
}


def _validate_contract_shape(artifact_format: str, contract: dict[str, Any]) -> None:
    _reject_unknown_keys(contract, CONTRACT_KEYS[artifact_format], "validation")
    if "contractVersion" not in contract:
        raise OfficeArtifactError(
            "schema.missing_field",
            "validation.contractVersion is required for requestVersion 2",
        )
    if type(contract["contractVersion"]) is not int or contract["contractVersion"] != 2:
        raise OfficeArtifactError(
            "schema.contract_version",
            "validation.contractVersion must be 2",
        )
    string_arrays = {
        "required_text", "forbidden_text", "required_sheets", "required_named_ranges",
        "no_numeric_hardcodes_in", "required_styles", "required_slide_titles",
    }
    object_arrays = {"tie_outs", "reconciliations", "formula_patterns"}
    nonnegative_integers = {"min_rows", "min_paragraphs", "min_tables", "min_comments", "min_slides", "max_slides"}
    booleans = {
        "require_formula_cache", "no_heading_level_skips", "require_alt_text",
        "require_table_header_rows", "require_fixed_table_layout", "require_tracked_changes",
        "require_no_tracked_changes", "require_speaker_notes",
    }
    for key in string_arrays & set(contract):
        value = contract[key]
        if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
            raise OfficeArtifactError("schema.contract_type", f"validation.{key} must be an array of strings")
    for key in object_arrays & set(contract):
        value = contract[key]
        if not isinstance(value, list) or not all(isinstance(item, dict) for item in value):
            raise OfficeArtifactError("schema.contract_type", f"validation.{key} must be an array of objects")
    for key in nonnegative_integers & set(contract):
        if key == "min_rows":
            value = contract[key]
            if not isinstance(value, dict) or not all(
                isinstance(name, str) and type(count) is int and count >= 0
                for name, count in value.items()
            ):
                raise OfficeArtifactError(
                    "schema.contract_type",
                    "validation.min_rows must map sheet names to non-negative integers",
                )
            continue
        value = contract[key]
        if type(value) is not int or value < 0:
            raise OfficeArtifactError("schema.contract_type", f"validation.{key} must be a non-negative integer")
    for key in booleans & set(contract):
        if type(contract[key]) is not bool:
            raise OfficeArtifactError("schema.contract_type", f"validation.{key} must be a boolean")
    if "required_language" in contract and not isinstance(contract["required_language"], str):
        raise OfficeArtifactError("schema.contract_type", "validation.required_language must be a string")
    for key in {"sentinels", "required_provenance"} & set(contract):
        if not isinstance(contract[key], dict):
            raise OfficeArtifactError("schema.contract_type", f"validation.{key} must be an object")
    if (
        type(contract.get("min_slides")) is int
        and type(contract.get("max_slides")) is int
        and contract["min_slides"] > contract["max_slides"]
    ):
        raise OfficeArtifactError(
            "schema.contract_range",
            "validation.min_slides cannot exceed max_slides",
        )


def _sha256(path: Path) -> str:
    import hashlib

    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _read_request(raw: str | None) -> dict[str, Any]:
    if raw is None:
        return {}
    if raw == "-":
        payload = json.load(sys.stdin)
    else:
        path = workspace_path(Path(raw), Path.cwd(), must_exist=True)
        payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise OfficeArtifactError("request.invalid_root", "request root must be an object")
    return payload


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Run OfficeArtifactEngine v2")
    parser.add_argument(
        "--action",
        required=True,
        choices=["capabilities", "inspect", "assess", "execute", "decide", "restore"],
    )
    parser.add_argument("--request", help="Absolute request JSON path or '-' for stdin")
    parser.add_argument("--source", help="Office artifact path for inspect")
    parser.add_argument("--format", choices=sorted(FORMATS), help="Optional format assertion for inspect")
    parser.add_argument("--candidate-id")
    parser.add_argument("--decision", choices=["publish", "discard"])
    parser.add_argument("--receipt-id")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    engine = OfficeArtifactEngine(Path.cwd())
    try:
        if args.action == "capabilities":
            result = engine.capabilities()
        elif args.action == "inspect":
            if not args.source:
                raise OfficeArtifactError("inspect.source_required", "inspect requires source")
            result = engine.inspect(args.source, args.format)
        elif args.action == "assess":
            result = engine.assess(_read_request(args.request))
        elif args.action == "execute":
            result = engine.execute(_read_request(args.request))
        elif args.action == "decide":
            if not args.candidate_id or not args.decision:
                raise OfficeArtifactError("decision.arguments_required", "decide requires candidate id and decision")
            result = engine.decide(args.candidate_id, args.decision)
        else:
            if not args.receipt_id:
                raise OfficeArtifactError("restore.receipt_required", "restore requires receipt id")
            result = engine.restore(args.receipt_id)
        print(json.dumps(result, ensure_ascii=False, indent=2))
        return 0
    except OfficeArtifactError as error:
        print(json.dumps(error.to_dict(), ensure_ascii=False, indent=2))
        return 1
    except Exception as error:  # noqa: BLE001
        wrapped = OfficeArtifactError(
            "engine.internal",
            f"{type(error).__name__}: {error}",
        )
        print(json.dumps(wrapped.to_dict(), ensure_ascii=False, indent=2))
        return 1


if __name__ == "__main__":
    sys.exit(main())
