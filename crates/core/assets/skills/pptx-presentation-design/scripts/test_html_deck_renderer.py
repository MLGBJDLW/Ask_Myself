#!/usr/bin/env python3
"""Tests for the HTML-first PPTX renderer."""

from __future__ import annotations

import json
import contextlib
import io
import tempfile
import unittest
import zipfile
from pathlib import Path

import html_deck_renderer


class HtmlDeckRendererTests(unittest.TestCase):
    def test_render_html_deck_writes_project_pptx_animation_and_qa(self) -> None:
        try:
            import pptx  # noqa: F401
        except ImportError:
            self.skipTest("python-pptx is not installed")

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            spec_path = root / "html_deck.json"
            project_dir = root / "project"
            pptx_path = root / "deck.pptx"
            spec = {
                "title": "HTML Pipeline Demo",
                "slide_size": "ppt169",
                "theme": {
                    "background_color": "F8FAFC",
                    "text_color": "111827",
                    "accent_color": "F97316",
                },
                "slides": [
                    {
                        "id": "cover",
                        "title": "HTML-first Deck",
                        "html": "<h1 class='hero'>HTML-first Deck</h1>",
                        "background": "0F172A",
                        "transition": {"type": "fade", "speed": "fast"},
                        "elements": [
                            {
                                "type": "rect",
                                "id": "panel",
                                "x": 0.8,
                                "y": 0.8,
                                "w": 4.2,
                                "h": 0.16,
                                "fill": "F97316",
                                "line": "none",
                            },
                            {
                                "type": "text",
                                "id": "headline",
                                "text": "HTML-first Deck",
                                "x": 0.8,
                                "y": 1.35,
                                "w": 8.0,
                                "h": 1.0,
                                "font_size": 38,
                                "bold": True,
                                "color": "FFFFFF",
                                "animation": {"effect": "fade", "duration_ms": 350},
                            },
                        ],
                    },
                    {
                        "id": "proof",
                        "title": "Hybrid export",
                        "html": "<h1>Hybrid export</h1><p>Native text stays editable.</p>",
                        "elements": [
                            {
                                "type": "text",
                                "id": "body",
                                "text": "Native text stays editable.",
                                "x": 1.0,
                                "y": 1.2,
                                "w": 7.8,
                                "h": 0.6,
                                "font_size": 24,
                                "color": "111827",
                            },
                            {
                                "type": "ellipse",
                                "id": "signal",
                                "x": 9.4,
                                "y": 1.0,
                                "w": 1.2,
                                "h": 1.2,
                                "fill": "2563EB",
                                "line": "none",
                            },
                        ],
                        "animations": [{"target": "signal", "effect": "zoom", "delay_ms": 120}],
                    },
                ],
            }
            spec_path.write_text(json.dumps(spec), encoding="utf-8")

            result = html_deck_renderer.render_html_deck(
                spec_path=str(spec_path),
                out_dir=str(project_dir),
                pptx_path=str(pptx_path),
                mode="hybrid",
                screenshot="skip",
                workspace_root=root,
            )

            self.assertTrue((project_dir / "source" / "deck.html").exists())
            self.assertTrue((project_dir / "source" / "slides" / "slide_01.html").exists())
            self.assertTrue((project_dir / "manifest.json").exists())
            self.assertTrue((project_dir / "qa.json").exists())
            self.assertTrue(pptx_path.exists())
            self.assertEqual("warn", result["qa"]["status"])
            self.assertEqual(1.0, result["manifest"]["pptx"]["metrics"]["editabilityScore"])
            self.assertEqual(2, result["manifest"]["pptx"]["metrics"]["animationTargets"])

            with zipfile.ZipFile(pptx_path) as zf:
                slide1 = zf.read("ppt/slides/slide1.xml").decode("utf-8")
                slide2 = zf.read("ppt/slides/slide2.xml").decode("utf-8")

            self.assertIn("<p:transition", slide1)
            self.assertIn("<p:timing", slide1)
            self.assertIn("<p:animEffect", slide1)
            self.assertIn("<p:timing", slide2)

    def test_validate_spec_rejects_empty_slides(self) -> None:
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            html_deck_renderer._validate_spec({"slides": []})


if __name__ == "__main__":
    unittest.main()
