#!/usr/bin/env python3
"""Unit tests for pptx_template_profile helpers."""

from __future__ import annotations

import unittest

import pptx_template_profile


class PptxTemplateProfileTests(unittest.TestCase):
    def test_scores_layouts_for_common_template_purposes(self) -> None:
        placeholders = [
            {"type": "title"},
            {"type": "body"},
            {"type": "body"},
        ]

        scores = pptx_template_profile._score_layout(placeholders, graphics=0)

        self.assertGreater(scores["body"], 0)
        self.assertGreater(scores["comparison"], 0)
        self.assertGreater(scores["title"], 0)

    def test_best_recommendations_choose_highest_scored_layout(self) -> None:
        layouts = [
            {
                "index": 0,
                "name": "Title Slide",
                "scores": {"title": 9, "body": 1, "section": 8, "table": 1, "chart": 1, "comparison": 1},
            },
            {
                "index": 1,
                "name": "Title and Content",
                "scores": {"title": 5, "body": 7, "section": 2, "table": 5, "chart": 5, "comparison": 6},
            },
        ]

        recommendations = pptx_template_profile._best_recommendations(layouts)

        self.assertEqual(0, recommendations["title"]["layout_index"])
        self.assertEqual(1, recommendations["body"]["layout_index"])
        self.assertEqual(1, recommendations["comparison"]["layout_index"])


if __name__ == "__main__":
    unittest.main()
