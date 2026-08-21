#!/usr/bin/env python3
"""Tests for DOCX Spec v2 rendering."""

from __future__ import annotations

import tempfile
import unittest
import zipfile
from pathlib import Path

import docx_renderer


class DocxRendererTests(unittest.TestCase):
    def test_renders_styles_table_geometry_headers_fields_and_image_alt_text(self) -> None:
        try:
            from PIL import Image
            import docx
        except ImportError:
            self.skipTest("Pillow and python-docx are required")
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            image = root / "chart.png"
            Image.new("RGB", (320, 160), (60, 110, 160)).save(image)
            output = root / "report.docx"
            spec = {
                "schemaVersion": 2,
                "preset": "executive",
                "title": "董事会报告",
                "subtitle": "Board Report",
                "author": "Nexa QA",
                "language": "zh-CN",
                "page": {"marginLeft": 0.8, "marginRight": 0.8},
                "header": {"text": "Confidential"},
                "footer": {"text": "Board", "pageNumber": True},
                "blocks": [
                    {"type": "heading", "level": 1, "text": "执行摘要"},
                    {"type": "paragraph", "text": "See [source](https://example.com/source)."},
                    {
                        "type": "table",
                        "headers": ["Metric", "Value"],
                        "rows": [["Revenue", "100"], ["Margin", "42%"]],
                        "columnWidths": [3.6, 2.2],
                        "repeatHeader": True,
                        "allowRowBreaks": False,
                    },
                    {
                        "type": "image",
                        "path": str(image),
                        "width": 4.0,
                        "altText": "蓝色收入趋势图",
                        "caption": "Figure 1 — Revenue",
                    },
                    {"type": "callout", "kind": "risk", "text": "Risk requires a decision."},
                ],
            }

            result = docx_renderer.render_docx(spec, output, root)
            self.assertEqual(1, result["metrics"]["tables"])
            self.assertEqual(1, result["metrics"]["images"])
            document = docx.Document(output)
            self.assertEqual("Nexa QA", document.core_properties.author)
            self.assertEqual("Aptos", document.styles["Normal"].font.name)
            self.assertEqual("Heading 1", next(p for p in document.paragraphs if p.text == "执行摘要").style.name)
            self.assertAlmostEqual(0.8, document.sections[0].left_margin.inches, places=2)
            with zipfile.ZipFile(output) as archive:
                document_xml = archive.read("word/document.xml").decode("utf-8")
                footer_xml = archive.read("word/footer1.xml").decode("utf-8")
                rels = archive.read("word/_rels/document.xml.rels").decode("utf-8")
            self.assertIn("tblHeader", document_xml)
            self.assertIn("cantSplit", document_xml)
            self.assertIn("蓝色收入趋势图", document_xml)
            self.assertIn(" PAGE ", footer_xml)
            self.assertIn("https://example.com/source", rels)

    def test_rejects_unknown_fields_and_missing_alt_text(self) -> None:
        with self.assertRaisesRegex(docx_renderer.DocxSpecError, "unknown field"):
            docx_renderer.validate_spec({"schemaVersion": 2, "blocks": [], "typo": True})
        with self.assertRaisesRegex(docx_renderer.DocxSpecError, "altText is required"):
            docx_renderer.validate_spec({
                "schemaVersion": 2,
                "blocks": [{"type": "image", "path": "image.png"}],
            })


if __name__ == "__main__":
    unittest.main()
