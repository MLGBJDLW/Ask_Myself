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


if __name__ == "__main__":
    unittest.main()
