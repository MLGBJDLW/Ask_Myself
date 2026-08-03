from __future__ import annotations

import argparse
import contextlib
import hashlib
import io
import json
import os
import tempfile
import unittest
import zipfile
from pathlib import Path
from xml.etree import ElementTree as ET

import edit_doc
import office_artifact_service
from office_artifact_runtime import (
    publish_staged_artifact,
    scan_ooxml_risks,
    staging_path,
    validate_ooxml_package,
)
from office_artifact_service import OfficeArtifactJob, execute_job

CONTENT_TYPES_NS = "http://schemas.openxmlformats.org/package/2006/content-types"
RELATIONSHIPS_NS = "http://schemas.openxmlformats.org/package/2006/relationships"


def _rewrite_zip(
    path: Path,
    replacements: dict[str, bytes],
    additions: dict[str, bytes] | None = None,
    output: Path | None = None,
) -> Path:
    rewritten = output or path.with_name(f"{path.stem}-rewritten{path.suffix}")
    with zipfile.ZipFile(path) as source, zipfile.ZipFile(rewritten, "w") as destination:
        for info in source.infolist():
            destination.writestr(info, replacements.get(info.filename, source.read(info.filename)))
        for name, data in (additions or {}).items():
            destination.writestr(name, data)
    return rewritten


class OfficeArtifactRuntimeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name).resolve()
        self.previous_cwd = Path.cwd()
        os.chdir(self.root)

    def tearDown(self) -> None:
        os.chdir(self.previous_cwd)
        self.temp.cleanup()

    def test_docx_creation_emits_real_hyperlink_and_valid_package(self) -> None:
        path = self.root / "linked.docx"
        args = argparse.Namespace(
            path=str(path),
            template=None,
            font="Calibri",
            title="Linked report",
            subtitle="",
            input_md=None,
            body="Read [Nexa](https://example.com/nexa) for details.",
            footer="",
            author="Nexa",
        )
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(0, edit_doc.cmd_create_docx(args))

        with zipfile.ZipFile(path) as archive:
            document_xml = archive.read("word/document.xml").decode("utf-8")
            relationships = archive.read("word/_rels/document.xml.rels").decode("utf-8")
        self.assertIn("<w:hyperlink", document_xml)
        self.assertIn("https://example.com/nexa", relationships)
        self.assertEqual("pass", validate_ooxml_package(path).status)

    def test_docx_replace_matches_across_runs_and_header_story(self) -> None:
        import docx

        path = self.root / "report.docx"
        document = docx.Document()
        paragraph = document.add_paragraph()
        paragraph.add_run("Q")
        paragraph.add_run("3")
        paragraph.add_run(" Report")
        header = document.sections[0].header.paragraphs[0]
        header.add_run("Q3")
        header.add_run(" Report")
        document.save(path)

        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(0, edit_doc._replace_docx(path, "Q3 Report", "Q4 Review", False))

        reopened = docx.Document(path)
        self.assertEqual("Q4 Review", reopened.paragraphs[0].text)
        self.assertEqual("Q4 Review", reopened.sections[0].header.paragraphs[0].text)
        snapshots = list((self.root / ".nexa" / "doc-history").rglob("report.docx"))
        self.assertEqual(1, len(snapshots))

    def test_pptx_replace_matches_across_runs(self) -> None:
        from pptx import Presentation

        path = self.root / "deck.pptx"
        presentation = Presentation()
        slide = presentation.slides.add_slide(presentation.slide_layouts[5])
        box = slide.shapes.add_textbox(0, 0, 4_000_000, 1_000_000)
        paragraph = box.text_frame.paragraphs[0]
        paragraph.add_run().text = "Q"
        paragraph.add_run().text = "3"
        paragraph.add_run().text = " Results"
        presentation.save(path)

        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(0, edit_doc._replace_pptx(path, "Q3 Results", "Q4 Review", False))

        reopened = Presentation(path)
        self.assertEqual("Q4 Review", reopened.slides[0].shapes[-1].text)
        self.assertEqual("pass", validate_ooxml_package(path).status)

    def test_xlsx_precise_replace_preserves_sensitive_part_bytes(self) -> None:
        import openpyxl

        path = self.root / "model.xlsx"
        workbook = openpyxl.Workbook()
        workbook.active["A1"] = "Q3 Report"
        workbook.save(path)
        workbook.close()

        with zipfile.ZipFile(path) as archive:
            content_types = ET.fromstring(archive.read("[Content_Types].xml"))
        ET.SubElement(
            content_types,
            f"{{{CONTENT_TYPES_NS}}}Override",
            {
                "PartName": "/xl/pivotCache/pivotCacheDefinition1.xml",
                "ContentType": "application/vnd.openxmlformats-officedocument.spreadsheetml.pivotCacheDefinition+xml",
            },
        )
        sensitive = b"<?xml version='1.0' encoding='UTF-8'?><pivot-cache marker='preserve-me'/>"
        path = _rewrite_zip(
            path,
            {"[Content_Types].xml": ET.tostring(content_types, encoding="utf-8", xml_declaration=True)},
            {"xl/pivotCache/pivotCacheDefinition1.xml": sensitive},
            self.root / "model-sensitive.xlsx",
        )
        with zipfile.ZipFile(path) as archive:
            untouched_sheet = archive.read("xl/worksheets/sheet1.xml")

        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(0, edit_doc._replace_xlsx(path, "Q3 Report", "Q4 Review", False))

        reopened = openpyxl.load_workbook(path, read_only=True)
        self.assertEqual("Q4 Review", reopened.active["A1"].value)
        reopened.close()
        with zipfile.ZipFile(path) as archive:
            self.assertEqual(sensitive, archive.read("xl/pivotCache/pivotCacheDefinition1.xml"))
            self.assertNotEqual(untouched_sheet, archive.read("xl/worksheets/sheet1.xml"))
        self.assertEqual("medium", scan_ooxml_risks(path)["riskLevel"])

    def test_xlsx_replace_does_not_reserialize_unmatched_editable_parts(self) -> None:
        import openpyxl

        path = self.root / "selective.xlsx"
        workbook = openpyxl.Workbook()
        workbook.active["A1"] = "Q3 Report"
        workbook.create_sheet("Untouched")["A1"] = "Stable content"
        workbook.save(path)
        workbook.close()
        with zipfile.ZipFile(path) as archive:
            untouched_sheet = archive.read("xl/worksheets/sheet2.xml")

        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(0, edit_doc._replace_xlsx(path, "Q3 Report", "Q4 Review", False))

        with zipfile.ZipFile(path) as archive:
            self.assertEqual(untouched_sheet, archive.read("xl/worksheets/sheet2.xml"))

    @unittest.skipUnless(
        edit_doc._find_soffice() and edit_doc._find_pdftoppm(),
        "LibreOffice and Poppler are required for native recalculation/render smoke",
    )
    def test_xlsx_recalculates_and_renders_with_native_qa_tools(self) -> None:
        import openpyxl

        path = self.root / "calculated.xlsx"
        workbook = openpyxl.Workbook()
        workbook.active["A1"] = 1
        workbook.active["A2"] = 2
        workbook.active["A3"] = "=SUM(A1:A2)"
        workbook.save(path)
        workbook.close()

        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(
                0,
                edit_doc.cmd_recalc_xlsx(argparse.Namespace(path=str(path), allow_risky=False)),
            )
        recalculated = openpyxl.load_workbook(path, data_only=True, read_only=True)
        self.assertEqual(3, recalculated.active["A3"].value)
        recalculated.close()

        render_dir = self.root / "rendered"
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(
                0,
                edit_doc.cmd_render(
                    argparse.Namespace(path=str(path), outdir=str(render_dir), dpi=90, format="png")
                ),
            )
        self.assertTrue(list(render_dir.glob("page*.png")))

    def test_validator_rejects_missing_relationship_target(self) -> None:
        import docx

        path = self.root / "broken.docx"
        document = docx.Document()
        document.add_paragraph("Valid before corruption")
        document.save(path)
        with zipfile.ZipFile(path) as archive:
            rels = ET.fromstring(archive.read("word/_rels/document.xml.rels"))
        ET.SubElement(
            rels,
            f"{{{RELATIONSHIPS_NS}}}Relationship",
            {
                "Id": "rIdMissing",
                "Type": "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image",
                "Target": "media/missing.png",
            },
        )
        broken = self.root / "missing-target.docx"
        replacement = ET.tostring(rels, encoding="utf-8", xml_declaration=True)
        with zipfile.ZipFile(path) as source, zipfile.ZipFile(broken, "w") as destination:
            for info in source.infolist():
                data = replacement if info.filename == "word/_rels/document.xml.rels" else source.read(info.filename)
                destination.writestr(info, data)

        report = validate_ooxml_package(broken)
        self.assertEqual("fail", report.status)
        self.assertTrue(any(issue.code == "relationship.missing_target" for issue in report.errors))

    def test_failed_staged_validation_leaves_original_unchanged(self) -> None:
        import docx

        path = self.root / "safe.docx"
        document = docx.Document()
        document.add_paragraph("Original")
        document.save(path)
        before = hashlib.sha256(path.read_bytes()).hexdigest()
        staged = staging_path(path)
        staged.write_bytes(b"not-an-office-package")

        with self.assertRaises(ValueError):
            publish_staged_artifact(staged, path, self.root, validate=True)
        after = hashlib.sha256(path.read_bytes()).hexdigest()
        self.assertEqual(before, after)

    def test_transactional_job_writes_result_manifest(self) -> None:
        output = self.root / "job.docx"
        payload = {
            "jobVersion": 1,
            "format": "docx",
            "intent": "create_new",
            "output": str(output),
            "operations": [{
                "op": "create",
                "title": "Runtime job",
                "body": "A [linked result](https://example.com/result).",
            }],
            "preservationPolicy": "strict",
            "renderPolicy": "none",
        }
        job = OfficeArtifactJob.from_dict(payload, self.root)
        result, exit_code = execute_job(job, self.root)

        self.assertEqual(0, exit_code, result)
        self.assertTrue(result["ok"])
        self.assertTrue(output.exists())
        manifest = json.loads((self.root / "artifact-manifest.json").read_text(encoding="utf-8"))
        self.assertTrue(manifest["ok"])
        self.assertEqual("nexa-openxml", manifest["backend"])
        self.assertEqual("pass", validate_ooxml_package(output).status)

    def test_job_rejects_nested_operation_paths_outside_workspace(self) -> None:
        outside_spec = self.root.parent / f"{self.root.name}-outside-spec.json"
        outside_spec.write_text("{}", encoding="utf-8")
        try:
            payload = {
                "jobVersion": 1,
                "format": "xlsx",
                "intent": "create_new",
                "output": "blocked.xlsx",
                "operations": [{"op": "create", "spec": f"../{outside_spec.name}"}],
            }
            job = OfficeArtifactJob.from_dict(payload, self.root)
            result, exit_code = execute_job(job, self.root)
        finally:
            outside_spec.unlink(missing_ok=True)

        self.assertEqual(1, exit_code)
        self.assertIn("path escapes workspace", result["error"])
        self.assertFalse((self.root / "blocked.xlsx").exists())

    def test_manifest_failure_rolls_back_newly_published_artifact(self) -> None:
        output = self.root / "rolled-back.docx"
        manifest_directory = self.root / "manifest-directory"
        manifest_directory.mkdir()
        job = OfficeArtifactJob.from_dict(
            {
                "jobVersion": 1,
                "format": "docx",
                "intent": "create_new",
                "output": str(output),
                "manifest": str(manifest_directory),
                "operations": [{"op": "create", "body": "Must roll back"}],
            },
            self.root,
        )

        result, exit_code = execute_job(job, self.root)

        self.assertEqual(1, exit_code)
        self.assertFalse(result["ok"])
        self.assertTrue(result["rollbackApplied"])
        self.assertFalse(output.exists())

    def test_auto_backend_routes_recalculation_and_finalization_by_capability(self) -> None:
        base = {
            "jobVersion": 1,
            "input": str(self.root / "source.xlsx"),
            "output": str(self.root / "result.xlsx"),
            "format": "xlsx",
            "operations": [],
        }
        source = Path(base["input"])
        source.write_bytes(b"placeholder")
        recalculate = OfficeArtifactJob.from_dict(
            {**base, "intent": "edit_existing", "operations": [{"op": "recalculate"}]},
            self.root,
        )
        self.assertEqual("libreoffice", office_artifact_service._select_backend(recalculate))

        docx = self.root / "source.docx"
        docx.write_bytes(b"placeholder")
        finalize = OfficeArtifactJob.from_dict(
            {
                "jobVersion": 1,
                "format": "docx",
                "intent": "finalize",
                "input": str(docx),
                "output": str(self.root / "result.docx"),
            },
            self.root,
        )
        self.assertEqual("windows-com", office_artifact_service._select_backend(finalize))

    def test_transactional_xlsx_job_publishes_final_qa_sidecar(self) -> None:
        output = self.root / "job-model.xlsx"
        spec = self.root / "model-spec.json"
        spec.write_text(
            json.dumps({
                "title": "Job model",
                "sheets": [{
                    "name": "Summary",
                    "start_cell": "A1",
                    "headers": ["Metric", "Value"],
                    "rows": [["Revenue", 42]],
                }],
            }),
            encoding="utf-8",
        )
        payload = {
            "jobVersion": 1,
            "format": "xlsx",
            "intent": "create_new",
            "output": str(output),
            "operations": [{"op": "create", "spec": str(spec)}],
            "renderPolicy": "none",
        }
        job = OfficeArtifactJob.from_dict(payload, self.root)
        result, exit_code = execute_job(job, self.root)

        self.assertEqual(0, exit_code, result)
        qa_path = output.with_suffix(".xlsx.qa.json")
        self.assertTrue(qa_path.exists())
        qa = json.loads(qa_path.read_text(encoding="utf-8"))
        self.assertEqual(str(output), qa["path"])
        self.assertNotIn("nexa-stage", json.dumps(result))


if __name__ == "__main__":
    unittest.main()
