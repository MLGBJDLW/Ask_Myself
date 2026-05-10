#!/usr/bin/env python3
"""Unit tests for pptx_renderer layout validation."""

from __future__ import annotations

import contextlib
import io
import unittest

import pptx_renderer


class PptxRendererValidationTests(unittest.TestCase):
    def test_supported_layouts_include_advanced_editable_slide_families(self) -> None:
        expected = {
            "timeline",
            "process",
            "comparison",
            "matrix",
            "chart",
        }
        self.assertTrue(expected.issubset(pptx_renderer.PPTX_SUPPORTED_LAYOUTS))
        self.assertIn("stacked_column", pptx_renderer.PPTX_SUPPORTED_CHART_TYPES)
        self.assertIn("pie", pptx_renderer.PPTX_SUPPORTED_CHART_TYPES)

    def test_validate_spec_accepts_advanced_layout_payloads(self) -> None:
        spec = {
            "slides": [
                {
                    "layout": "timeline",
                    "title": "Roadmap",
                    "events": [
                        {"date": "Q1", "title": "Research"},
                        {"date": "Q2", "title": "Launch"},
                    ],
                },
                {
                    "layout": "process",
                    "title": "Operating Model",
                    "steps": [
                        {"title": "Intake", "detail": "Capture requirements"},
                        {"title": "Build", "detail": "Create editable assets"},
                    ],
                },
                {
                    "layout": "comparison",
                    "title": "Options",
                    "left": {"heading": "Current", "bullets": ["Manual"]},
                    "right": {"heading": "Target", "bullets": ["Automated"]},
                },
                {
                    "layout": "matrix",
                    "title": "Priority Matrix",
                    "quadrants": [
                        {"title": "Quick wins"},
                        {"title": "Strategic bets"},
                        {"title": "Delegate"},
                        {"title": "Defer"},
                    ],
                },
                {
                    "layout": "chart",
                    "title": "Growth",
                    "categories": ["Q1", "Q2"],
                    "series": [{"name": "Revenue", "values": [10, 14]}],
                    "chart_type": "stacked_column",
                    "data_labels": True,
                },
            ],
            "notes_per_slide": [""] * 5,
        }

        slides, notes = pptx_renderer._validate_spec(spec)

        self.assertEqual(5, len(slides))
        self.assertEqual(5, len(notes))

    def test_validate_spec_rejects_chart_without_data(self) -> None:
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            pptx_renderer._validate_spec(
                {"slides": [{"layout": "chart", "title": "Broken"}]}
            )

    def test_validate_spec_rejects_unknown_chart_type(self) -> None:
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            pptx_renderer._validate_spec(
                {
                    "slides": [
                        {
                            "layout": "chart",
                            "title": "Broken",
                            "categories": ["A"],
                            "series": [{"name": "Value", "values": [1]}],
                            "chart_type": "unknown",
                        }
                    ]
                }
            )

    def test_template_layout_index_resolution(self) -> None:
        class Layouts:
            def __init__(self) -> None:
                self.items = ["Title", "Body"]

            def __len__(self) -> int:
                return len(self.items)

            def __getitem__(self, index: int):
                return self.items[index]

        prs = type("PresentationStub", (), {"slide_layouts": Layouts()})()

        self.assertEqual("Body", pptx_renderer._bound_template_layout(prs, {"template_layout_index": 1}))
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            pptx_renderer._bound_template_layout(prs, {"template_layout_index": 4})

    def test_remove_existing_slides_preserves_layout_collection(self) -> None:
        class SlideId:
            rId = "rId1"

        class Part:
            def __init__(self) -> None:
                self.dropped = []

            def drop_rel(self, rel_id: str) -> None:
                self.dropped.append(rel_id)

        class Slides:
            def __init__(self) -> None:
                self._sldIdLst = [SlideId()]

        prs = type("PresentationStub", (), {"slides": Slides(), "part": Part()})()

        pptx_renderer._remove_existing_slides(prs)

        self.assertEqual([], prs.slides._sldIdLst)
        self.assertEqual(["rId1"], prs.part.dropped)


if __name__ == "__main__":
    unittest.main()
