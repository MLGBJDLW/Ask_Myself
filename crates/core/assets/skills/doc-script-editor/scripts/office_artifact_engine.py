#!/usr/bin/env python3
"""OfficeArtifactEngine v2: one transactional interface for DOCX/PPTX/XLSX.

The engine deliberately separates candidate creation from publication. Format,
calculation, rendering, validation, and transaction decisions stay behind this
module so callers express outcomes and guarantees instead of shell pipelines.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import sys
import uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Protocol

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
from office_artifact_service import OfficeArtifactJob, execute_job


REQUEST_VERSION = 2
FORMATS = {"docx", "pptx", "xlsx"}
INTENTS = {"create", "modify", "verify"}
QUALITY_LEVELS = {"draft", "standard", "publish", "native"}
CALCULATION_LEVELS = {"not_required", "static", "compatible", "native"}
DELIVERY_MODES = {"candidate", "publish"}
CANDIDATE_ID_RE = re.compile(r"^[0-9a-f]{32}$")


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
            "retryable": self.retryable,
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
    })),
    "xlsx": LocalOpenXmlFormatAdapter("xlsx", frozenset({
        "create", "replace", "redact", "validate", "render", "recalculate",
        "set_value", "set_formula", "set_range", "clear_range", "set_style",
    })),
    "pptx": LocalOpenXmlFormatAdapter("pptx", frozenset({
        "create", "replace", "redact", "insert_slide", "validate", "render",
        "set_text", "clone_slide", "reorder_slides", "set_transition",
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
        version = int(payload.get("requestVersion", REQUEST_VERSION))
        if version != REQUEST_VERSION:
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
        if source is not None and source.suffix.lower() != f".{artifact_format}":
            raise OfficeArtifactError("request.source_format", f"source must end with .{artifact_format}")

        raw_destination = payload.get("destination")
        if intent == "verify" and raw_destination is None and source is not None:
            raw_destination = source.with_name(f"{source.stem}-verified{source.suffix}")
        if raw_destination is None:
            raise OfficeArtifactError("request.destination_required", "destination is required")
        destination = workspace_path(Path(str(raw_destination)), workspace_root)
        if destination.suffix.lower() != f".{artifact_format}":
            raise OfficeArtifactError(
                "request.destination_format",
                f"destination must end with .{artifact_format}",
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

        guarantees = payload.get("guarantees", {})
        if not isinstance(guarantees, dict):
            raise OfficeArtifactError("request.invalid_guarantees", "guarantees must be an object")
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

        delivery = payload.get("delivery", {})
        if not isinstance(delivery, dict):
            raise OfficeArtifactError("request.invalid_delivery", "delivery must be an object")
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
            validation=payload.get("validation"),
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

    def capabilities(self) -> dict[str, Any]:
        backends = office_backend_statuses()
        return {
            "kind": "officeArtifactCapabilities",
            "requestVersion": REQUEST_VERSION,
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
            "lifecycle": ["assess", "execute", "decide", "restore"],
        }

    def assess(self, payload: dict[str, Any]) -> dict[str, Any]:
        request = ArtifactRequest.from_dict(payload, self.workspace_root)
        backend = self._backend_for(request)
        readiness = next(item for item in office_backend_statuses() if item["id"] == backend)
        blockers: list[dict[str, Any]] = []
        source_profile: dict[str, Any] | None = None
        if backend != "nexa-openxml" and readiness["status"] != "ready":
            blockers.append({
                "code": "backend.unavailable",
                "backend": backend,
                "detail": readiness.get("detail"),
            })
        if request.render != "none":
            statuses = {item["id"]: item for item in office_backend_statuses()}
            if statuses["libreoffice"]["status"] != "ready":
                blockers.append({
                    "code": "render.backend_unavailable",
                    "backend": "libreoffice",
                    "detail": statuses["libreoffice"].get("detail"),
                })
        if request.source is not None:
            risk = scan_ooxml_risks(request.source)
            source_profile = {"risk": risk}
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
        return {
            "kind": "officeArtifactAssessment",
            "requestVersion": REQUEST_VERSION,
            "format": request.format,
            "intent": request.intent,
            "adapter": FORMAT_ADAPTERS[request.format].id,
            "backend": backend,
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
        candidate_path = candidate_dir / f"artifact.{request.format}"
        candidate_manifest = candidate_dir / "execution.json"
        state_path = candidate_dir / "state.json"
        v1_payload = self._v1_payload(request, candidate_path, candidate_manifest)
        state = {
            "kind": "officeArtifactCandidateState",
            "version": 1,
            "candidateId": candidate_id,
            "status": "executing",
            "createdAt": _utc_now(),
            "destination": str(request.destination),
            "requestedManifest": str(request.manifest),
            "candidatePath": str(candidate_path),
            "destinationExistedAtExecute": request.destination.exists(),
            "destinationBaseSha256": _sha256(request.destination) if request.destination.exists() else None,
            "request": payload,
        }
        write_artifact_manifest(state_path, state, self.workspace_root)
        try:
            job = OfficeArtifactJob.from_dict(v1_payload, self.workspace_root)
            execution, exit_code = execute_job(job, self.workspace_root)
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
            state.update({
                "status": "candidate",
                "execution": execution,
                "assessment": assessment,
                "candidateSha256": _sha256(candidate_path),
                "updatedAt": _utc_now(),
            })
            write_artifact_manifest(state_path, state, self.workspace_root)
            outcome = self._candidate_outcome(state)
            if request.delivery_mode == "publish":
                return self.decide(candidate_id, "publish")
            return outcome
        except Exception:
            state["status"] = "failed"
            state["updatedAt"] = _utc_now()
            write_artifact_manifest(state_path, state, self.workspace_root)
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
            write_artifact_manifest(state_path, state, self.workspace_root)
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
        if _sha256(candidate) != state.get("candidateSha256"):
            raise OfficeArtifactError(
                "candidate.hash_mismatch",
                "candidate changed after verification; execute it again",
            )
        lock_path = self._acquire_destination_lock(destination, candidate_id)
        staged: Path | None = None
        snapshot = None
        sidecar_records: list[dict[str, Any]] = []
        published = False
        receipt_path: Path | None = None
        receipt: dict[str, Any] | None = None
        try:
            self._assert_destination_precondition(state, destination)
            destination_existed = destination.exists()
            if destination_existed and os.environ.get("NEXA_OFFICE_SKIP_SNAPSHOT") == "1":
                raise OfficeArtifactError(
                    "transaction.snapshot_disabled",
                    "refusing to overwrite an existing destination while snapshots are disabled",
                )
            staged = staging_path(destination)
            shutil.copy2(candidate, staged)
            snapshot, validation = publish_staged_artifact(
                staged,
                destination,
                self.workspace_root,
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
                qa_existed = destination_qa.exists()
                qa_snapshot, _ = publish_staged_artifact(
                    staged_qa,
                    destination_qa,
                    self.workspace_root,
                    validate=False,
                )
                sidecar_records.append({
                    "path": str(destination_qa),
                    "snapshot": str(qa_snapshot) if qa_snapshot else None,
                    "existedBefore": qa_existed,
                })
            receipt_id = uuid.uuid4().hex
            receipt_path = self.receipts_root / f"{receipt_id}.json"
            outcome = self._candidate_outcome(state)
            outcome.update({
                "status": "published",
                "path": str(destination),
                "receiptId": receipt_id,
                "sha256": _sha256(destination),
            })
            requested_manifest = workspace_path(
                Path(state["requestedManifest"]), self.workspace_root
            )
            manifest_existed = requested_manifest.exists()
            manifest_snapshot = snapshot_file(requested_manifest, self.workspace_root)
            if manifest_existed and manifest_snapshot is None:
                raise OfficeArtifactError(
                    "transaction.manifest_snapshot_missing",
                    "existing delivery manifest could not be snapshotted",
                )
            write_artifact_manifest(requested_manifest, outcome, self.workspace_root)
            sidecar_records.append({
                "path": str(requested_manifest),
                "snapshot": str(manifest_snapshot) if manifest_snapshot else None,
                "existedBefore": manifest_existed,
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
                "existedBefore": destination_existed,
                "sidecars": sidecar_records,
                "candidateId": candidate_id,
            }
            write_artifact_manifest(receipt_path, receipt, self.workspace_root)
            state.update({
                "status": "published",
                "receiptId": receipt_id,
                "publishedAt": receipt["publishedAt"],
                "destinationSha256": receipt["destinationSha256"],
                "validation": validation.to_dict() if validation is not None else None,
            })
            write_artifact_manifest(state_path, state, self.workspace_root)
            return outcome
        except Exception as error:
            if staged is not None:
                staged.unlink(missing_ok=True)
            for record in reversed(sidecar_records):
                rollback_published_artifact(
                    Path(record["path"]),
                    Path(record["snapshot"]) if record.get("snapshot") else None,
                    self.workspace_root,
                )
            if published:
                rollback_published_artifact(destination, snapshot, self.workspace_root)
            if receipt_path is not None and receipt_path.exists():
                failure_receipt = receipt or {
                    "kind": "officeArtifactReceipt",
                    "version": 1,
                    "receiptId": receipt_path.stem,
                    "candidateId": candidate_id,
                }
                failure_receipt.update({
                    "status": "rolled_back",
                    "rolledBackAt": _utc_now(),
                    "error": f"{type(error).__name__}: {error}",
                })
                write_artifact_manifest(receipt_path, failure_receipt, self.workspace_root)
            raise
        finally:
            lock_path.unlink(missing_ok=True)

    def restore(self, receipt_id: str) -> dict[str, Any]:
        if not CANDIDATE_ID_RE.fullmatch(receipt_id):
            raise OfficeArtifactError("receipt.invalid_id", "invalid receipt id")
        receipt_path = self.receipts_root / f"{receipt_id}.json"
        if not receipt_path.is_file():
            raise OfficeArtifactError("receipt.not_found", f"receipt not found: {receipt_id}")
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
        if receipt.get("status") != "published":
            raise OfficeArtifactError(
                "receipt.invalid_state",
                f"receipt {receipt_id} is in state {receipt.get('status')}",
            )
        destination = workspace_path(Path(receipt["destination"]), self.workspace_root)
        if destination.exists() and _sha256(destination) != receipt.get("destinationSha256"):
            raise OfficeArtifactError(
                "restore.destination_changed",
                "destination changed after publication; refusing to overwrite newer work",
            )
        for record in reversed(receipt.get("sidecars", [])):
            if record.get("existedBefore") and not record.get("snapshot"):
                raise OfficeArtifactError(
                    "restore.sidecar_snapshot_missing",
                    f"cannot restore pre-existing sidecar without snapshot: {record['path']}",
                )
            rollback_published_artifact(
                Path(record["path"]),
                Path(record["snapshot"]) if record.get("snapshot") else None,
                self.workspace_root,
            )
        snapshot = Path(receipt["snapshot"]) if receipt.get("snapshot") else None
        if receipt.get("existedBefore") and snapshot is None:
            raise OfficeArtifactError(
                "restore.snapshot_missing",
                "cannot restore pre-existing destination without snapshot",
            )
        rollback_published_artifact(destination, snapshot, self.workspace_root)
        receipt.update({"status": "restored", "restoredAt": _utc_now()})
        write_artifact_manifest(receipt_path, receipt, self.workspace_root)
        return {
            "kind": "officeArtifactOutcome",
            "status": "restored",
            "receiptId": receipt_id,
            "path": str(destination),
            "restoredSnapshot": str(snapshot) if snapshot else None,
        }

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
            json.dump({
                "kind": "officeArtifactDestinationLock",
                "candidateId": candidate_id,
                "destination": str(destination),
                "pid": os.getpid(),
                "createdAt": _utc_now(),
            }, handle, ensure_ascii=False)
        return lock_path

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
        if request.quality == "native" or request.calculation == "native":
            return "windows-com"
        if request.calculation == "compatible":
            return "libreoffice"
        return "nexa-openxml"

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

    def _v1_payload(
        self,
        request: ArtifactRequest,
        candidate_path: Path,
        manifest: Path,
    ) -> dict[str, Any]:
        operations = list(request.operations)
        if request.calculation == "compatible" and not any(
            str(operation.get("op", "")).lower() == "recalculate"
            for operation in operations
        ):
            operations.append({"op": "recalculate"})
        return {
            "jobVersion": 1,
            "format": request.format,
            "intent": "create_new" if request.intent == "create" else "edit_existing",
            "input": str(request.source) if request.source else None,
            "output": str(candidate_path),
            "operations": operations,
            "preservationPolicy": request.preservation,
            "validationContract": request.validation,
            "renderPolicy": request.render,
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
            "jobVersion": 1,
            "format": request.format,
            "intent": "finalize",
            "input": str(candidate_path),
            "output": str(candidate_path),
            "operations": [],
            "preservationPolicy": request.preservation,
            "validationContract": request.validation,
            "renderPolicy": request.render,
            "backend": "windows-com",
            "manifest": str(native_manifest),
        }
        native, exit_code = execute_job(
            OfficeArtifactJob.from_dict(native_payload, self.workspace_root),
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
            "renderedPreviews": (
                native_execution.get("renderedPreviews", [])
                if isinstance(native_execution, dict)
                else openxml_execution.get("renderedPreviews", [])
            ),
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


def _utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


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
        choices=["capabilities", "assess", "execute", "decide", "restore"],
    )
    parser.add_argument("--request", help="Absolute request JSON path or '-' for stdin")
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
