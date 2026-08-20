from __future__ import annotations

import json
import hmac
import re
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest import mock

import office_artifact_engine
from office_artifact_engine import OfficeArtifactEngine, OfficeArtifactError


class OfficeArtifactEngineTests(unittest.TestCase):
    def test_windows_process_liveness_probe_never_uses_kill(self) -> None:
        kernel = mock.MagicMock()
        kernel.OpenProcess.return_value = 11

        def write_still_active(process, output):
            output._obj.value = 259
            return 1

        kernel.GetExitCodeProcess.side_effect = write_still_active
        kernel.CloseHandle.return_value = 1
        with mock.patch.object(office_artifact_engine.os, "name", "nt"), mock.patch.object(
            office_artifact_engine.ctypes, "WinDLL", return_value=kernel
        ), mock.patch.object(office_artifact_engine.os, "kill", side_effect=AssertionError("must not kill")):
            self.assertTrue(office_artifact_engine._process_is_alive(4242))
        kernel.CloseHandle.assert_called_once_with(11)

    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name).resolve()
        self.engine = OfficeArtifactEngine(self.root)

    def tearDown(self) -> None:
        self.temp.cleanup()

    def _docx_request(self, destination: Path) -> dict:
        return {
            "requestVersion": 2,
            "format": "docx",
            "intent": "create",
            "destination": str(destination),
            "operations": [{
                "op": "create",
                "title": "Candidate report",
                "body": "Verified body",
            }],
            "guarantees": {
                "quality": "standard",
                "preservation": "strict",
                "render": "none",
            },
            "validation": {"contractVersion": 2, "required_text": ["Verified body"]},
        }

    def test_execute_publish_restore_lifecycle_keeps_destination_gated(self) -> None:
        destination = self.root / "delivery.docx"
        outcome = self.engine.execute(self._docx_request(destination))

        self.assertEqual("candidate", outcome["status"])
        self.assertFalse(destination.exists())
        candidate = Path(outcome["candidatePath"])
        self.assertTrue(candidate.exists())

        published = self.engine.decide(outcome["candidateId"], "publish")
        self.assertEqual("published", published["status"])
        self.assertTrue(destination.exists())
        self.assertTrue((self.root / "delivery.docx.manifest.json").exists())

        restored = self.engine.restore(published["receiptId"])
        self.assertEqual("restored", restored["status"])
        self.assertFalse(destination.exists())

    def test_restore_refuses_to_overwrite_newer_destination(self) -> None:
        destination = self.root / "delivery.docx"
        candidate = self.engine.execute(self._docx_request(destination))
        published = self.engine.decide(candidate["candidateId"], "publish")
        destination.write_bytes(destination.read_bytes() + b"newer")

        with self.assertRaisesRegex(OfficeArtifactError, "changed after publication"):
            self.engine.restore(published["receiptId"])

    def test_restore_refuses_changed_sidecar_and_tampered_receipt(self) -> None:
        destination = self.root / "delivery.docx"
        candidate = self.engine.execute(self._docx_request(destination))
        published = self.engine.decide(candidate["candidateId"], "publish")
        manifest = self.root / "delivery.docx.manifest.json"
        manifest.write_text('{"user":"newer manifest"}\n', encoding="utf-8")

        with self.assertRaisesRegex(OfficeArtifactError, "sidecar changed"):
            self.engine.restore(published["receiptId"])
        self.assertEqual('{"user":"newer manifest"}\n', manifest.read_text(encoding="utf-8"))
        self.assertTrue(destination.exists())

        # A separate publication proves that changing any receipt field is
        # rejected before a path or snapshot is trusted.
        second_destination = self.root / "second.docx"
        second_candidate = self.engine.execute(self._docx_request(second_destination))
        second = self.engine.decide(second_candidate["candidateId"], "publish")
        receipt_path = (
            self.root
            / ".nexa"
            / "office-artifacts"
            / "receipts"
            / f"{second['receiptId']}.json"
        )
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
        receipt["destination"] = str(self.root / "forged.docx")
        receipt_path.write_text(json.dumps(receipt, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        with self.assertRaisesRegex(OfficeArtifactError, "HMAC|integrity"):
            self.engine.restore(second["receiptId"])
        self.assertTrue(second_destination.exists())

    def test_publish_uses_destination_cas_and_exclusive_lock(self) -> None:
        destination = self.root / "cas.docx"
        candidate = self.engine.execute(self._docx_request(destination))
        destination.write_bytes(b"external writer")
        with self.assertRaisesRegex(OfficeArtifactError, "existence changed"):
            self.engine.decide(candidate["candidateId"], "publish")

        destination.unlink()
        lock = self.engine._acquire_destination_lock(destination, "f" * 32)
        try:
            with self.assertRaisesRegex(OfficeArtifactError, "owns the destination lock"):
                self.engine.decide(candidate["candidateId"], "publish")
        finally:
            lock.unlink(missing_ok=True)

    def test_shared_manifest_role_cas_prevents_cross_destination_overwrite(self) -> None:
        manifest = self.root / "shared-manifest.json"
        first_request = self._docx_request(self.root / "first.docx")
        second_request = self._docx_request(self.root / "second.docx")
        first_request["delivery"] = {"manifest": str(manifest)}
        second_request["delivery"] = {"manifest": str(manifest)}
        first = self.engine.execute(first_request)
        second = self.engine.execute(second_request)

        self.engine.decide(first["candidateId"], "publish")
        first_manifest = manifest.read_bytes()
        with self.assertRaisesRegex(OfficeArtifactError, "role existence changed"):
            self.engine.decide(second["candidateId"], "publish")
        self.assertEqual(first_manifest, manifest.read_bytes())
        self.assertFalse((self.root / "second.docx").exists())

    def test_candidate_state_hmac_blocks_destination_and_manifest_retargeting(self) -> None:
        destination = self.root / "signed.docx"
        forged_destination = self.root / "forged.docx"
        candidate = self.engine.execute(self._docx_request(destination))
        state_path = Path(candidate["candidatePath"]).parent / "state.json"
        state = json.loads(state_path.read_text(encoding="utf-8"))
        state["destination"] = str(forged_destination)
        state["requestedManifest"] = str(self.root / "source-code.py")
        state_path.write_text(json.dumps(state), encoding="utf-8")

        with self.assertRaisesRegex(OfficeArtifactError, "candidate state HMAC"):
            self.engine.decide(candidate["candidateId"], "publish")
        self.assertFalse(destination.exists())
        self.assertFalse(forged_destination.exists())

    def test_restore_reinstates_existing_destination_and_manifest(self) -> None:
        import docx

        destination = self.root / "existing.docx"
        manifest = self.root / "existing-manifest.json"
        original = docx.Document()
        original.add_paragraph("Original destination")
        original.save(destination)
        original_hash = destination.read_bytes()
        manifest.write_text('{"status":"old"}\n', encoding="utf-8")
        old_manifest = manifest.read_bytes()
        request = self._docx_request(destination)
        request["delivery"] = {"manifest": str(manifest)}

        candidate = self.engine.execute(request)
        published = self.engine.decide(candidate["candidateId"], "publish")
        self.assertNotEqual(original_hash, destination.read_bytes())
        receipt = json.loads(
            (self.root / ".nexa" / "office-artifacts" / "receipts" / f"{published['receiptId']}.json")
            .read_text(encoding="utf-8")
        )
        self.assertTrue(receipt["existedBefore"])
        self.assertIsNotNone(receipt["snapshot"])

        self.engine.restore(published["receiptId"])
        self.assertEqual(original_hash, destination.read_bytes())
        self.assertEqual(old_manifest, manifest.read_bytes())

    def test_restore_journal_recovers_partial_multi_role_restore(self) -> None:
        import docx

        destination = self.root / "restore-crash.docx"
        manifest = self.root / "restore-crash-manifest.json"
        original = docx.Document()
        original.add_paragraph("Original destination")
        original.save(destination)
        original_bytes = destination.read_bytes()
        manifest.write_text('{"status":"original"}\n', encoding="utf-8")
        original_manifest = manifest.read_bytes()
        request = self._docx_request(destination)
        request["delivery"] = {"manifest": str(manifest)}
        candidate = self.engine.execute(request)
        published = self.engine.decide(candidate["candidateId"], "publish")
        published_bytes = destination.read_bytes()
        real_rollback = office_artifact_engine.rollback_published_artifact

        def fail_destination(target, snapshot, workspace_root):
            if Path(target).resolve() == destination.resolve():
                raise OSError("injected destination restore failure")
            return real_rollback(target, snapshot, workspace_root)

        with mock.patch.object(
            office_artifact_engine,
            "rollback_published_artifact",
            side_effect=fail_destination,
        ):
            with self.assertRaisesRegex(OSError, "injected destination"):
                self.engine.restore(published["receiptId"])

        self.assertEqual(original_manifest, manifest.read_bytes())
        self.assertEqual(published_bytes, destination.read_bytes())
        journals = self.root / ".nexa" / "office-artifacts" / "journals"
        self.assertEqual(1, len(list(journals.glob("restore-*.json"))))

        with mock.patch.object(office_artifact_engine, "_process_is_alive", return_value=False):
            recovered_engine = OfficeArtifactEngine(self.root)
        self.assertEqual(original_bytes, destination.read_bytes())
        self.assertEqual(original_manifest, manifest.read_bytes())
        self.assertEqual([], list(journals.glob("restore-*.json")))
        receipt = json.loads(
            (self.root / ".nexa" / "office-artifacts" / "receipts" / f"{published['receiptId']}.json")
            .read_text(encoding="utf-8")
        )
        self.assertEqual("restored", receipt["status"])
        self.assertTrue(
            hmac.compare_digest(
                receipt["integrity"]["value"],
                recovered_engine._receipt_integrity(receipt)["value"],
            )
        )

    def test_restore_recovery_rejects_valid_receipt_replay_before_mutation(self) -> None:
        first = self.engine.execute(self._docx_request(self.root / "first-replay.docx"))
        first_published = self.engine.decide(first["candidateId"], "publish")
        second = self.engine.execute(self._docx_request(self.root / "second-replay.docx"))
        second_published = self.engine.decide(second["candidateId"], "publish")

        receipts = self.root / ".nexa" / "office-artifacts" / "receipts"
        first_receipt_path = receipts / f"{first_published['receiptId']}.json"
        second_receipt_path = receipts / f"{second_published['receiptId']}.json"
        first_receipt = json.loads(first_receipt_path.read_text(encoding="utf-8"))
        first_destination = Path(first_receipt["destination"])
        published_bytes = first_destination.read_bytes()

        # Simulate internally inconsistent, but individually authenticated,
        # candidate state. Recovery must bind all three records before it
        # performs the first destructive restore step.
        first_state_path, first_state = self.engine._load_candidate(first["candidateId"])
        first_state["receiptId"] = second_published["receiptId"]
        first_state["receiptSha256"] = office_artifact_engine._sha256(second_receipt_path)
        self.engine._write_candidate_state(first_state_path, first_state)

        journal = {
            "kind": "officeArtifactRestoreJournal",
            "version": 1,
            "status": "active",
            "pid": 2_147_483_647,
            "candidateId": first["candidateId"],
            "receiptId": first_published["receiptId"],
            "destination": str(first_destination),
            "lockRolePaths": [str(first_destination)],
            "roles": [{
                "path": str(first_destination),
                "publishedSha256": first_receipt["destinationSha256"],
                "snapshot": None,
                "snapshotSha256": None,
                "restoredSha256": None,
                "existedBefore": False,
                "restored": False,
            }],
        }
        journals = self.root / ".nexa" / "office-artifacts" / "journals"
        journals.mkdir(parents=True, exist_ok=True)
        journal_path = journals / f"restore-{first_published['receiptId']}.json"
        self.engine._write_journal(journal_path, journal)

        with mock.patch.object(office_artifact_engine, "_process_is_alive", return_value=False):
            OfficeArtifactEngine(self.root)

        self.assertEqual(published_bytes, first_destination.read_bytes())
        blocked = json.loads(journal_path.read_text(encoding="utf-8"))
        self.assertEqual("recovery_blocked", blocked["status"])
        self.assertIn("binding failed", blocked["recoveryBlockers"][0])

    def test_manifest_fault_after_artifact_publish_rolls_back_every_role(self) -> None:
        import docx

        destination = self.root / "fault.docx"
        manifest = self.root / "fault-manifest.json"
        original = docx.Document()
        original.add_paragraph("Before fault")
        original.save(destination)
        before = destination.read_bytes()
        manifest.write_text('{"status":"before"}\n', encoding="utf-8")
        manifest_before = manifest.read_bytes()
        request = self._docx_request(destination)
        request["delivery"] = {"manifest": str(manifest)}
        candidate = self.engine.execute(request)
        real_publish = self.engine._journal_publish_role

        def fail_requested_manifest(journal_path, journal, staged, target, *, validate):
            if Path(target).resolve() == manifest.resolve():
                raise OSError("injected manifest fault")
            return real_publish(journal_path, journal, staged, target, validate=validate)

        with mock.patch.object(
            self.engine,
            "_journal_publish_role",
            side_effect=fail_requested_manifest,
        ):
            with self.assertRaisesRegex(OSError, "injected manifest fault"):
                self.engine.decide(candidate["candidateId"], "publish")

        self.assertEqual(before, destination.read_bytes())
        self.assertEqual(manifest_before, manifest.read_bytes())
        state = json.loads(
            (Path(candidate["candidatePath"]).parent / "state.json").read_text(encoding="utf-8")
        )
        self.assertEqual("candidate", state["status"])
        self.assertEqual([], list((self.root / ".nexa" / "office-artifacts" / "locks").glob("*.lock")))

    def test_startup_recovers_crash_after_artifact_publication(self) -> None:
        destination = self.root / "crash.docx"
        destination.write_bytes(b"before")
        snapshot = office_artifact_engine.snapshot_file(destination, self.root)
        self.assertIsNotNone(snapshot)
        destination.write_bytes(b"published")
        candidate_id = "b" * 32
        journals = self.root / ".nexa" / "office-artifacts" / "journals"
        journals.mkdir(parents=True, exist_ok=True)
        journal = {
            "kind": "officeArtifactPublishJournal",
            "version": 1,
            "candidateId": candidate_id,
            "status": "active",
            "pid": 2_147_483_647,
            "destination": str(destination),
            "roles": [{
                "path": str(destination),
                "existedBefore": True,
                "preexistingSha256": office_artifact_engine.hashlib.sha256(b"before").hexdigest(),
                "snapshot": str(snapshot),
                "snapshotSha256": office_artifact_engine._sha256(snapshot),
                "intendedSha256": office_artifact_engine.hashlib.sha256(b"published").hexdigest(),
                "published": True,
            }],
        }
        journal["integrity"] = self.engine._journal_integrity(journal)
        journal_path = journals / f"{candidate_id}.json"
        journal_path.write_text(json.dumps(journal), encoding="utf-8")

        OfficeArtifactEngine(self.root)
        self.assertEqual(b"before", destination.read_bytes())
        self.assertFalse(journal_path.exists())

    def test_startup_quarantines_forged_journal_without_touching_user_files(self) -> None:
        destination = self.root / "important.docx"
        destination.write_bytes(b"keep me")
        candidate_id = "c" * 32
        journals = self.root / ".nexa" / "office-artifacts" / "journals"
        journals.mkdir(parents=True, exist_ok=True)
        forged = {
            "kind": "officeArtifactPublishJournal",
            "version": 1,
            "candidateId": candidate_id,
            "status": "active",
            "pid": 2_147_483_647,
            "destination": str(destination),
            "roles": [{
                "path": str(destination),
                "existedBefore": False,
                "snapshot": None,
                "intendedSha256": office_artifact_engine._sha256(destination),
            }],
            "integrity": {"algorithm": "HMAC-SHA256", "value": "0" * 64},
        }
        journal_path = journals / f"{candidate_id}.json"
        journal_path.write_text(json.dumps(forged), encoding="utf-8")

        OfficeArtifactEngine(self.root)
        self.assertEqual(b"keep me", destination.read_bytes())
        self.assertFalse(journal_path.exists())
        self.assertEqual(1, len(list(journals.glob(f"{candidate_id}.json.invalid-*.quarantine"))))

    def test_startup_removes_signed_dead_orphan_lock_without_journal(self) -> None:
        destination = self.root / "orphan.docx"
        lock_path = self.engine._acquire_destination_lock(destination, "d" * 32)
        lock = json.loads(lock_path.read_text(encoding="utf-8"))
        lock["pid"] = 2_147_483_647
        lock["integrity"] = self.engine._lock_integrity(lock)
        lock_path.write_text(json.dumps(lock), encoding="utf-8")

        with mock.patch.object(office_artifact_engine, "_process_is_alive", return_value=False):
            OfficeArtifactEngine(self.root)
        self.assertFalse(lock_path.exists())

    def test_assessment_reports_unsatisfied_publish_render_backend(self) -> None:
        request = self._docx_request(self.root / "publish.docx")
        request["guarantees"]["quality"] = "publish"
        request["guarantees"].pop("render")
        assessment = self.engine.assess(request)

        backend_status = {item["id"]: item for item in self.engine.capabilities()["backends"]}
        if backend_status["libreoffice"]["status"] == "ready":
            self.assertTrue(assessment["ready"])
        else:
            self.assertFalse(assessment["ready"])
            self.assertIn("render.backend_unavailable", {item["code"] for item in assessment["blockers"]})

        request = self._docx_request(self.root / "publish-bypass.docx")
        request["guarantees"].update({"quality": "publish", "render": "none"})
        with self.assertRaisesRegex(OfficeArtifactError, "requires final candidate render"):
            self.engine.assess(request)

    def test_path_roles_and_candidate_ids_are_validated(self) -> None:
        destination = self.root / "conflict.docx"
        request = self._docx_request(destination)
        request["delivery"] = {"manifest": str(destination)}
        with self.assertRaisesRegex(OfficeArtifactError, "manifest must be distinct"):
            self.engine.execute(request)
        request = self._docx_request(destination)
        request["delivery"] = {
            "manifest": str(
                self.root
                / ".nexa"
                / "office-artifacts"
                / "receipts"
                / f"{'a' * 32}.json"
            )
        }
        with self.assertRaisesRegex(OfficeArtifactError, "reserved .nexa state"):
            self.engine.assess(request)
        xlsx_destination = self.root / "conflict.xlsx"
        spec = self.root / "conflict-spec.json"
        spec.write_text(
            json.dumps({"sheets": [{"name": "Sheet1", "rows": [["value"]]}]}),
            encoding="utf-8",
        )
        request = {
            "requestVersion": 2,
            "format": "xlsx",
            "intent": "create",
            "destination": str(xlsx_destination),
            "operations": [{"op": "create", "spec": str(spec)}],
        }
        request["delivery"] = {"manifest": str(xlsx_destination.with_suffix(".xlsx.qa.json"))}
        with self.assertRaisesRegex(OfficeArtifactError, "XLSX QA sidecar"):
            self.engine.assess(request)
        request["delivery"] = {"manifest": str(spec)}
        with self.assertRaisesRegex(OfficeArtifactError, "cannot overwrite request input"):
            self.engine.assess(request)
        with self.assertRaisesRegex(OfficeArtifactError, "invalid candidate id"):
            self.engine.decide("../escape", "discard")

    def test_request_operation_and_contract_schemas_reject_unknown_fields(self) -> None:
        destination = self.root / "strict.docx"
        request = self._docx_request(destination)
        request["typo"] = True
        with self.assertRaisesRegex(OfficeArtifactError, "unknown field.*request"):
            self.engine.assess(request)

        request = self._docx_request(destination)
        request["operations"][0]["titel"] = "typo"
        with self.assertRaisesRegex(OfficeArtifactError, r"operations\[0\]"):
            self.engine.assess(request)

        request = self._docx_request(destination)
        request["validation"] = {"contractVersion": 2, "required_tex": ["missing t"]}
        with self.assertRaisesRegex(OfficeArtifactError, "unknown field.*validation"):
            self.engine.assess(request)

        request = self._docx_request(destination)
        request["validation"] = 7
        with self.assertRaisesRegex(OfficeArtifactError, "validation must be"):
            self.engine.assess(request)

        request = self._docx_request(destination)
        request["validation"] = {"contractVersion": 2.9, "required_text": "BA"}
        with self.assertRaisesRegex(OfficeArtifactError, "contractVersion must be 2"):
            self.engine.assess(request)

        request = self._docx_request(destination)
        request["validation"] = {"contractVersion": 2, "required_text": "BA"}
        with self.assertRaisesRegex(OfficeArtifactError, "array of strings"):
            self.engine.assess(request)

        contract_path = self.root / "missing-version-contract.json"
        contract_path.write_text(json.dumps({"required_text": ["Verified body"]}), encoding="utf-8")
        request = self._docx_request(destination)
        request["validation"] = str(contract_path)
        with self.assertRaisesRegex(OfficeArtifactError, "contractVersion is required"):
            self.engine.assess(request)

        request = self._docx_request(destination)
        request["operations"] = [{
            "op": "replace",
            "find": "a",
            "replace": "b",
            "allowStyleMerge": "false",
        }]
        with self.assertRaisesRegex(OfficeArtifactError, "allowStyleMerge must be a boolean"):
            self.engine.assess(request)

        request = self._docx_request(destination)
        request["requestVersion"] = 2.9
        with self.assertRaisesRegex(OfficeArtifactError, "requestVersion must be 2"):
            self.engine.assess(request)

    def test_source_sha_precondition_is_format_wide(self) -> None:
        source = self.root / "precondition.docx"
        source.write_bytes(b"changed")
        request = {
            "requestVersion": 2,
            "format": "docx",
            "intent": "verify",
            "source": str(source),
            "destination": str(self.root / "verified.docx"),
            "operations": [],
            "preconditions": {"sourceSha256": "0" * 64},
        }
        with self.assertRaisesRegex(OfficeArtifactError, "source SHA-256"):
            self.engine.assess(request)

    def test_capabilities_validate_external_adapter_declarations_without_loading_code(self) -> None:
        adapter_dir = self.root / ".nexa" / "office-adapters"
        adapter_dir.mkdir(parents=True)
        (adapter_dir / "valid.json").write_text(json.dumps({
            "adapterVersion": 1,
            "id": "example-live",
            "deployment": "live-officejs",
            "formats": ["docx"],
            "operations": ["set_text"],
            "guarantees": {"preservation": ["native"], "calculation": [], "render": []},
            "limitations": ["declaration only"],
            "requires": ["officejs-host"],
        }), encoding="utf-8")
        (adapter_dir / "invalid.json").write_text('{"adapterVersion":2,"id":"Bad ID"}', encoding="utf-8")

        declarations = self.engine.capabilities()["externalAdapterDeclarations"]
        by_name = {Path(item["manifestPath"]).name: item for item in declarations}
        self.assertEqual("declared-not-loaded", by_name["valid.json"]["status"])
        self.assertEqual("invalid", by_name["invalid.json"]["status"])

    def test_xlsx_render_evidence_requires_surface_manifest_bound_to_candidate_sha(self) -> None:
        try:
            from PIL import Image
        except ImportError:
            self.skipTest("Pillow is not installed")
        candidate_dir = self.root / ".nexa" / "office-artifacts" / "candidates" / ("a" * 32)
        render_dir = candidate_dir / "artifact-rendered"
        render_dir.mkdir(parents=True)
        candidate = candidate_dir / "artifact.xlsx"
        candidate.write_bytes(b"candidate-bytes")
        pages = [render_dir / "sheet-001-page-1.png", render_dir / "sheet-002-page-1.png"]
        for index, page in enumerate(pages, start=1):
            Image.new("RGB", (640, 360), (40 * index, 80, 140)).save(page)
        manifest = {
            "kind": "officeRenderManifest",
            "format": "xlsx",
            "artifactSha256": office_artifact_engine._sha256(candidate),
            "expectedSurfaces": 2,
            "renderedSurfaces": 2,
            "complete": True,
        }
        (render_dir / "render-manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
        execution = {"renderedPreviews": [str(page) for page in pages]}

        evidence = self.engine._render_evidence(candidate, execution, "xlsx", "all")
        self.assertTrue(evidence["complete"])
        self.assertEqual(2, evidence["expectedSurfaces"])
        self.assertEqual(2, evidence["renderedSurfaces"])

        candidate.write_bytes(b"tampered")
        stale = self.engine._render_evidence(candidate, execution, "xlsx", "all")
        self.assertFalse(stale["complete"])

    def test_discard_removes_only_owned_candidate_directory(self) -> None:
        destination = self.root / "discarded.docx"
        keep = self.root / "keep.txt"
        keep.write_text("safe", encoding="utf-8")
        candidate = self.engine.execute(self._docx_request(destination))
        candidate_dir = Path(candidate["candidatePath"]).parent

        discarded = self.engine.decide(candidate["candidateId"], "discard")
        self.assertEqual("discarded", discarded["status"])
        self.assertFalse(candidate_dir.exists())
        self.assertEqual("safe", keep.read_text(encoding="utf-8"))

    def test_published_manifest_is_machine_readable_outcome(self) -> None:
        destination = self.root / "manifested.docx"
        request = self._docx_request(destination)
        request["delivery"] = {"mode": "publish"}
        outcome = self.engine.execute(request)

        manifest = json.loads((self.root / "manifested.docx.manifest.json").read_text(encoding="utf-8"))
        self.assertEqual("published", outcome["status"])
        self.assertEqual(outcome["receiptId"], manifest["receiptId"])

    def test_typed_xlsx_edits_are_literal_formula_safe_and_part_precise(self) -> None:
        try:
            import openpyxl  # type: ignore
        except ImportError:
            self.skipTest("openpyxl is not installed")
        import zipfile

        source = self.root / "source.xlsx"
        destination = self.root / "result.xlsx"
        workbook = openpyxl.Workbook()
        workbook.active.title = "Inputs"
        workbook.active["A1"] = "old"
        workbook.create_sheet("Untouched")["A1"] = "stable"
        workbook.save(source)
        workbook.close()
        with zipfile.ZipFile(source) as archive:
            untouched_before = archive.read("xl/worksheets/sheet2.xml")

        outcome = self.engine.execute({
            "requestVersion": 2,
            "format": "xlsx",
            "intent": "modify",
            "source": str(source),
            "destination": str(destination),
            "operations": [
                {"op": "set_value", "sheet": "inputs", "cell": "A1", "value": "=WEBSERVICE(\"https://example.invalid\")"},
                {"op": "set_formula", "sheet": "Inputs", "cell": "B1", "formula": "=1+1"},
                {"op": "set_range", "sheet": "Inputs", "range": "A2:B2", "values": [[3, 4]]},
                {"op": "set_style", "sheet": "Inputs", "range": "A1:B2", "styleId": 0},
            ],
            "guarantees": {"quality": "standard", "preservation": "strict", "render": "none"},
        })

        candidate = Path(outcome["candidatePath"])
        self.assertFalse(destination.exists())
        self.assertEqual("static", outcome["calculationEvidence"]["profile"])
        self.assertFalse(outcome["calculationEvidence"]["excelNative"])
        workbook = openpyxl.load_workbook(candidate, data_only=False)
        try:
            sheet = workbook["Inputs"]
            self.assertEqual("s", sheet["A1"].data_type)
            self.assertEqual("=WEBSERVICE(\"https://example.invalid\")", sheet["A1"].value)
            self.assertEqual("f", sheet["B1"].data_type)
            self.assertEqual("=1+1", sheet["B1"].value)
            self.assertEqual([3, 4], [sheet["A2"].value, sheet["B2"].value])
        finally:
            workbook.close()
        with zipfile.ZipFile(candidate) as archive:
            self.assertEqual(untouched_before, archive.read("xl/worksheets/sheet2.xml"))

    def test_assessment_requires_excel_native_for_dynamic_array_formulas(self) -> None:
        try:
            import openpyxl  # type: ignore
        except ImportError:
            self.skipTest("openpyxl is not installed")
        source = self.root / "dynamic.xlsx"
        workbook = openpyxl.Workbook()
        workbook.active["A1"] = 1
        workbook.active["A2"] = "=_xlfn.FILTER(A1:A1,A1:A1>0)"
        workbook.save(source)
        workbook.close()
        request = {
            "requestVersion": 2,
            "format": "xlsx",
            "intent": "modify",
            "source": str(source),
            "destination": str(self.root / "dynamic-result.xlsx"),
            "operations": [{"op": "set_value", "sheet": "Sheet", "cell": "B1", "value": 2}],
            "guarantees": {"calculation": "compatible", "quality": "standard", "render": "none"},
        }

        assessment = self.engine.assess(request)

        self.assertFalse(assessment["ready"])
        self.assertIn(
            "calculation.excel_native_required",
            {blocker["code"] for blocker in assessment["blockers"]},
        )
        self.assertIn(
            "function:FILTER",
            assessment["sourceProfile"]["formulaProfile"]["nativeFeatures"],
        )

    def test_docx_spec_v2_runs_through_candidate_validation(self) -> None:
        spec_path = self.root / "report-spec.json"
        destination = self.root / "professional.docx"
        spec_path.write_text(json.dumps({
            "schemaVersion": 2,
            "preset": "memo",
            "title": "Decision memo",
            "language": "en-US",
            "footer": {"text": "Internal", "pageNumber": True},
            "blocks": [
                {"type": "heading", "level": 1, "text": "Recommendation"},
                {"type": "paragraph", "text": "Approve the controlled rollout."},
                {
                    "type": "table",
                    "headers": ["Owner", "Date"],
                    "rows": [["Operations", "2026-08-20"]],
                    "columnWidths": [3.0, 2.0],
                    "repeatHeader": True,
                },
            ],
        }), encoding="utf-8")
        outcome = self.engine.execute({
            "requestVersion": 2,
            "format": "docx",
            "intent": "create",
            "destination": str(destination),
            "operations": [{"op": "create", "spec": str(spec_path)}],
            "guarantees": {"quality": "standard", "render": "none"},
            "validation": {
                "contractVersion": 2,
                "required_text": ["Approve the controlled rollout."],
                "min_tables": 1,
                "required_styles": ["Heading 1"],
                "no_heading_level_skips": True,
                "require_table_header_rows": True,
                "require_fixed_table_layout": True,
                "required_language": "en-US",
            },
        })
        self.assertEqual("candidate", outcome["status"])
        self.assertFalse(destination.exists())
        self.assertTrue(Path(outcome["candidatePath"]).exists())

    def test_pptx_exact_clone_copies_chart_workbook_and_targets_shape_by_id(self) -> None:
        try:
            from pptx import Presentation
            from pptx.chart.data import ChartData
            from pptx.enum.chart import XL_CHART_TYPE
            from pptx.util import Inches
        except ImportError:
            self.skipTest("python-pptx is not installed")
        import zipfile

        source = self.root / "source-deck.pptx"
        destination = self.root / "result-deck.pptx"
        presentation = Presentation()
        slide = presentation.slides.add_slide(presentation.slide_layouts[5])
        title = slide.shapes.add_textbox(Inches(1), Inches(0.5), Inches(5), Inches(0.7))
        title.name = "Decision title"
        title.text = "Original decision"
        chart_data = ChartData()
        chart_data.categories = ["A", "B"]
        chart_data.add_series("Revenue", (10, 20))
        slide.shapes.add_chart(
            XL_CHART_TYPE.COLUMN_CLUSTERED,
            Inches(1), Inches(1.5), Inches(6), Inches(3.5),
            chart_data,
        )
        presentation.save(source)
        shape_id = title.shape_id

        outcome = self.engine.execute({
            "requestVersion": 2,
            "format": "pptx",
            "intent": "modify",
            "source": str(source),
            "destination": str(destination),
            "operations": [
                {"op": "clone_slide", "slideIndex": 1},
                {"op": "set_text", "slideIndex": 2, "shapeId": shape_id, "text": "Cloned decision"},
                {"op": "set_transition", "slideIndex": 2, "transition": "fade", "speed": "fast"},
            ],
            "guarantees": {"quality": "standard", "preservation": "strict", "render": "none"},
            "validation": {"contractVersion": 2, "min_slides": 2, "max_slides": 2, "required_text": ["Cloned decision"]},
        })

        candidate = Path(outcome["candidatePath"])
        self.assertFalse(destination.exists())
        cloned = Presentation(candidate)
        try:
            self.assertEqual(2, len(cloned.slides))
            source_title = next(shape for shape in cloned.slides[0].shapes if shape.shape_id == shape_id)
            clone_title = next(shape for shape in cloned.slides[1].shapes if shape.shape_id == shape_id)
            self.assertEqual("Original decision", source_title.text)
            self.assertEqual("Cloned decision", clone_title.text)
        finally:
            del cloned
        with zipfile.ZipFile(candidate) as archive:
            names = set(archive.namelist())
            chart_parts = sorted(name for name in names if re.fullmatch(r"ppt/charts/chart\d+\.xml", name))
            workbook_parts = sorted(name for name in names if name.startswith("ppt/embeddings/") and name.endswith(".xlsx"))
            self.assertEqual(2, len(chart_parts))
            self.assertEqual(2, len(workbook_parts))
            self.assertEqual(archive.read(chart_parts[0]), archive.read(chart_parts[1]))
            self.assertEqual(archive.read(workbook_parts[0]), archive.read(workbook_parts[1]))
            self.assertIn(b"transition", archive.read("ppt/slides/slide2.xml"))

    def test_docx_review_operations_are_candidate_gated_and_contract_checked(self) -> None:
        import docx

        source = self.root / "review-source.docx"
        destination = self.root / "review-result.docx"
        document = docx.Document()
        document.add_paragraph("Approve old wording")
        document.save(source)

        outcome = self.engine.execute({
            "requestVersion": 2,
            "format": "docx",
            "intent": "modify",
            "source": str(source),
            "destination": str(destination),
            "operations": [
                {"op": "add_comment", "find": "Approve", "comment": "Owner confirmation required."},
                {"op": "tracked_replace", "find": "old", "replace": "new", "author": "Nexa"},
            ],
            "guarantees": {"quality": "standard", "preservation": "strict", "render": "none"},
            "validation": {"contractVersion": 2, "min_comments": 1, "require_tracked_changes": True},
        })

        self.assertEqual("candidate", outcome["status"])
        self.assertFalse(destination.exists())
        with zipfile.ZipFile(Path(outcome["candidatePath"])) as archive:
            xml = archive.read("word/document.xml")
            self.assertIn(b"commentRangeStart", xml)
            self.assertIn(b":ins", xml)
            self.assertIn(b":del", xml)
        state_text = (Path(outcome["candidatePath"]).parent / "state.json").read_text(
            encoding="utf-8"
        )
        self.assertNotIn("Owner confirmation required.", state_text)
        self.assertNotIn('"find": "Approve"', state_text)
        self.assertNotIn('"replace": "new"', state_text)
        self.assertIn("requestSha256", state_text)


if __name__ == "__main__":
    unittest.main()
