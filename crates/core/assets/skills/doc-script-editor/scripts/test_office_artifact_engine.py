from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from office_artifact_engine import OfficeArtifactEngine, OfficeArtifactError


class OfficeArtifactEngineTests(unittest.TestCase):
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
            "validation": {"required_text": ["Verified body"]},
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

    def test_path_roles_and_candidate_ids_are_validated(self) -> None:
        destination = self.root / "conflict.docx"
        request = self._docx_request(destination)
        request["delivery"] = {"manifest": str(destination)}
        with self.assertRaisesRegex(OfficeArtifactError, "manifest must be distinct"):
            self.engine.execute(request)
        with self.assertRaisesRegex(OfficeArtifactError, "invalid candidate id"):
            self.engine.decide("../escape", "discard")

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


if __name__ == "__main__":
    unittest.main()
