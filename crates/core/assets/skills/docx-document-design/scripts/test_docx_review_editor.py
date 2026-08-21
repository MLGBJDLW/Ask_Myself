#!/usr/bin/env python3
"""Tests for basic DOCX review lifecycle."""

from __future__ import annotations

import tempfile
import unittest
import zipfile
from pathlib import Path

import docx_review_editor


class DocxReviewEditorTests(unittest.TestCase):
    def _source(self, root: Path, text: str = "Review target here") -> Path:
        import docx

        path = root / "source.docx"
        document = docx.Document()
        document.add_paragraph(text)
        document.save(path)
        return path

    def test_comment_add_extract_and_strip_lifecycle(self) -> None:
        import docx

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = self._source(root)
            commented = root / "commented.docx"
            stripped = root / "stripped.docx"

            result = docx_review_editor.patch_docx_reviews(source, commented, [{
                "op": "add_comment",
                "find": "target",
                "comment": "Confirm this wording.",
                "author": "Reviewer",
                "initials": "RV",
            }])
            comments = docx_review_editor.extract_comments(commented)

            self.assertIn("word/comments.xml", result["changedParts"])
            self.assertEqual("Confirm this wording.", comments["comments"][0]["text"])
            self.assertEqual("Reviewer", comments["comments"][0]["author"])
            self.assertEqual("Review target here", docx.Document(commented).paragraphs[0].text)
            with zipfile.ZipFile(commented) as archive:
                document_xml = archive.read("word/document.xml")
                self.assertIn(b"commentRangeStart", document_xml)
                self.assertIn(b"commentReference", document_xml)

            docx_review_editor.patch_docx_reviews(commented, stripped, [{"op": "strip_comments"}])
            self.assertEqual([], docx_review_editor.extract_comments(stripped)["comments"])
            self.assertEqual("Review target here", docx.Document(stripped).paragraphs[0].text)
            with zipfile.ZipFile(stripped) as archive:
                self.assertNotIn("word/comments.xml", archive.namelist())

    def test_tracked_replace_accept_and_reject_views(self) -> None:
        import docx

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = self._source(root, "Approve old wording")
            redline = root / "redline.docx"
            accepted = root / "accepted.docx"
            rejected = root / "rejected.docx"

            docx_review_editor.patch_docx_reviews(source, redline, [{
                "op": "tracked_replace",
                "find": "old",
                "replace": "new",
                "author": "Editor",
            }])
            with zipfile.ZipFile(redline) as archive:
                document_xml = archive.read("word/document.xml")
            self.assertIn(b"<ns0:del", document_xml)
            self.assertIn(b"<ns0:ins", document_xml)
            self.assertIn(b"delText", document_xml)

            docx_review_editor.patch_docx_reviews(redline, accepted, [{"op": "accept_changes"}])
            docx_review_editor.patch_docx_reviews(redline, rejected, [{"op": "reject_changes"}])
            self.assertEqual("Approve new wording", docx.Document(accepted).paragraphs[0].text)
            self.assertEqual("Approve old wording", docx.Document(rejected).paragraphs[0].text)
            with zipfile.ZipFile(accepted) as archive:
                self.assertNotIn(b"<ns0:ins", archive.read("word/document.xml"))
                self.assertNotIn(b"<ns0:del", archive.read("word/document.xml"))

    def test_bookmark_field_content_control_and_protection_are_native_word_objects(self) -> None:
        import docx

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "structured.docx"
            document = docx.Document()
            document.add_paragraph("Bookmark target")
            document.add_paragraph("Field placeholder")
            document.add_paragraph("Controlled text")
            document.save(source)
            output = root / "structured-output.docx"

            result = docx_review_editor.patch_docx_reviews(source, output, [
                {"op": "add_bookmark", "find": "Bookmark", "bookmarkName": "DecisionAnchor"},
                {"op": "insert_field", "find": "placeholder", "instruction": "REF DecisionAnchor", "displayText": "Bookmark"},
                {"op": "wrap_content_control", "find": "Controlled", "tag": "decision", "title": "Decision", "lock": "content"},
                {"op": "set_protection", "mode": "trackedChanges"},
            ])

            self.assertEqual(
                ["word/document.xml", "word/settings.xml"],
                result["changedParts"],
            )
            with zipfile.ZipFile(output) as archive:
                document_xml = archive.read("word/document.xml")
                settings_xml = archive.read("word/settings.xml")
            self.assertIn(b"bookmarkStart", document_xml)
            self.assertIn(b"DecisionAnchor", document_xml)
            self.assertIn(b"fldSimple", document_xml)
            self.assertIn(b"REF DecisionAnchor", document_xml)
            self.assertIn(b"sdtContent", document_xml)
            self.assertIn(b"sdtContentLocked", document_xml)
            self.assertIn(b"documentProtection", settings_xml)
            self.assertIn(b"trackedChanges", settings_xml)

    def test_template_binding_targets_content_control_tags_and_bookmark_names(self) -> None:
        import docx

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "template.docx"
            structured = root / "structured.docx"
            bound = root / "bound.docx"
            document = docx.Document()
            document.add_paragraph("Customer placeholder")
            document.add_paragraph("Period placeholder")
            document.save(source)
            docx_review_editor.patch_docx_reviews(source, structured, [
                {
                    "op": "wrap_content_control",
                    "find": "Customer placeholder",
                    "tag": "customer_name",
                },
                {
                    "op": "add_bookmark",
                    "find": "Period placeholder",
                    "bookmarkName": "ReportPeriod",
                },
            ])
            result = docx_review_editor.patch_docx_reviews(structured, bound, [{
                "op": "bind_template",
                "bindings": {
                    "customer_name": "Nexa Labs",
                    "ReportPeriod": "2026 H2",
                },
            }])
            details = result["operations"][0]["detail"]["bindings"]
            self.assertEqual({"content_control", "bookmark"}, {item["target"] for item in details})
            with zipfile.ZipFile(bound) as archive:
                document_xml = archive.read("word/document.xml")
                self.assertIn(b"Nexa Labs", document_xml)
                self.assertIn(b"2026 H2", document_xml)
                self.assertNotIn(b"Customer placeholder", document_xml)
                self.assertNotIn(b"Period placeholder", document_xml)

    def test_field_allowlist_rejects_external_or_executable_instructions(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = self._source(root, "Unsafe field")
            with self.assertRaisesRegex(docx_review_editor.DocxReviewError, "safe.*allowlist"):
                docx_review_editor.patch_docx_reviews(source, root / "unsafe.docx", [{
                    "op": "insert_field",
                    "find": "Unsafe",
                    "instruction": "INCLUDETEXT https://example.invalid/secret",
                }])


if __name__ == "__main__":
    unittest.main()
