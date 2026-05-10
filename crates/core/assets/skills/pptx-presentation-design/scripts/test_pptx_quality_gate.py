#!/usr/bin/env python3
"""Unit tests for pptx_quality_gate."""

from __future__ import annotations

import unittest

import pptx_quality_gate


def _slide(index: int, **overrides):
    data = {
        "index": index,
        "text_chars": 120,
        "text_paragraphs": 4,
        "has_visual_anchor": True,
        "full_slide_pictures": 0,
        "empty_placeholders": 0,
        "notes_relationships": 1,
        "chart_relationships": 0,
    }
    data.update(overrides)
    return data


class PptxQualityGateTests(unittest.TestCase):
    def test_passes_clean_editable_deck(self) -> None:
        report = {
            "slides": 4,
            "warnings": [],
            "slide_details": [_slide(1), _slide(2), _slide(3), _slide(4)],
        }

        result = pptx_quality_gate.evaluate_audit(report, require_notes=True)

        self.assertEqual("pass", result["status"])
        self.assertEqual([], result["failures"])
        self.assertGreaterEqual(result["score"], 90)

    def test_fails_deck_with_missing_visual_anchor(self) -> None:
        report = {
            "slides": 3,
            "warnings": [],
            "slide_details": [_slide(1), _slide(2, has_visual_anchor=False), _slide(3)],
        }

        result = pptx_quality_gate.evaluate_audit(report)

        self.assertEqual("fail", result["status"])
        self.assertIn("slides missing visual anchors: 2", result["failures"])

    def test_fails_required_notes_coverage(self) -> None:
        report = {
            "slides": 4,
            "warnings": [],
            "slide_details": [
                _slide(1, notes_relationships=0),
                _slide(2, notes_relationships=1),
                _slide(3, notes_relationships=0),
                _slide(4, notes_relationships=0),
            ],
        }

        result = pptx_quality_gate.evaluate_audit(report, require_notes=True)

        self.assertEqual("fail", result["status"])
        self.assertIn("speaker notes coverage below 80%", result["failures"])

    def test_fails_warning_budget(self) -> None:
        report = {
            "slides": 2,
            "warnings": ["slide 2 is text dense (1000 chars)"],
            "slide_details": [_slide(1), _slide(2)],
        }

        result = pptx_quality_gate.evaluate_audit(report, max_warnings=0)

        self.assertEqual("fail", result["status"])
        self.assertIn("audit warnings exceed budget: 1 > 0", result["failures"])

    def test_fails_visual_qa_report(self) -> None:
        report = {
            "slides": 2,
            "warnings": [],
            "slide_details": [_slide(1), _slide(2)],
        }
        visual = {"status": "fail", "failure_count": 2, "issue_count": 3}

        result = pptx_quality_gate.evaluate_audit(report, visual_report=visual)

        self.assertEqual("fail", result["status"])
        self.assertIn("visual QA failures: 2", result["failures"])
        self.assertEqual(2, result["metrics"]["visual_failures"])


if __name__ == "__main__":
    unittest.main()
