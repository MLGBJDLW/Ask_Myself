from __future__ import annotations

import json
import re
import tempfile
import unittest
import zipfile
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
            "validation": {"min_slides": 2, "max_slides": 2, "required_text": ["Cloned decision"]},
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
            "validation": {"min_comments": 1, "require_tracked_changes": True},
        })

        self.assertEqual("candidate", outcome["status"])
        self.assertFalse(destination.exists())
        with zipfile.ZipFile(Path(outcome["candidatePath"])) as archive:
            xml = archive.read("word/document.xml")
            self.assertIn(b"commentRangeStart", xml)
            self.assertIn(b":ins", xml)
            self.assertIn(b":del", xml)


if __name__ == "__main__":
    unittest.main()
