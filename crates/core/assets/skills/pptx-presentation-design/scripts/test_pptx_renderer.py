#!/usr/bin/env python3
"""Unit tests for pptx_renderer layout validation."""

from __future__ import annotations

import contextlib
import io
import json
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

import pptx_renderer


class PptxRendererValidationTests(unittest.TestCase):
    def test_theme_presets_include_distinct_design_directions(self) -> None:
        for preset in [
            "consulting-clean",
            "executive-midnight",
            "editorial-ink",
            "product-energy",
            "healthcare-trust",
            "finance-precision",
            "education-bright",
        ]:
            theme = pptx_renderer._normalize_theme(preset)
            self.assertIn("background_style", theme)
            self.assertRegex(theme["primary_color"], r"^[0-9A-F]{6}$")

    def test_background_presets_include_industry_and_texture_styles(self) -> None:
        for style in ["blueprint_grid", "paper_texture", "clinical_grid", "data_grid", "spotlight"]:
            self.assertIn(style, pptx_renderer.PPTX_BACKGROUND_STYLES)

    def test_image_catalog_supports_dict_and_list_aliases(self) -> None:
        catalog = pptx_renderer._normalize_image_catalog(
            {
                "hero": {"path": "assets/hero.png"},
                "diagram": "assets/diagram.png",
                "photo": {"url": "https://example.com/photo.jpg"},
            }
        )
        list_catalog = pptx_renderer._normalize_image_catalog(
            [
                {"id": "cover", "path": "assets/cover.png"},
                {"name": "inline", "src": "assets/inline.png"},
            ]
        )

        self.assertEqual("assets/hero.png", catalog["hero"])
        self.assertEqual("assets/diagram.png", catalog["diagram"])
        self.assertEqual("https://example.com/photo.jpg", catalog["photo"])
        self.assertEqual("assets/cover.png", list_catalog["cover"])
        self.assertEqual("assets/inline.png", list_catalog["inline"])

    def test_icon_catalog_supports_builtin_and_asset_aliases(self) -> None:
        catalog = pptx_renderer._normalize_icon_catalog(
            {
                "risk": "shield",
                "growth": {"name": "trend"},
                "logo": {"path": "assets/logo.png"},
            }
        )

        self.assertEqual("shield", catalog["risk"])
        self.assertEqual("trend", catalog["growth"])
        self.assertEqual("assets/logo.png", catalog["logo"])

    def test_apply_image_catalog_resolves_background_and_foreground_aliases(self) -> None:
        slide = {
            "layout": "two_column",
            "background": {"image_id": "hero"},
            "left": {"heading": "Why", "image_id": "diagram"},
            "right": {"heading": "Now", "image": "@photo"},
        }
        resolved = pptx_renderer._apply_image_catalog_to_slide(
            slide,
            {
                "hero": "assets/hero.png",
                "diagram": "assets/diagram.png",
                "photo": "assets/photo.png",
            },
        )

        self.assertEqual("assets/hero.png", resolved["background"]["image"])
        self.assertEqual("assets/diagram.png", resolved["left"]["image"])
        self.assertEqual("assets/photo.png", resolved["right"]["image"])

    def test_apply_icon_catalog_resolves_slide_and_item_aliases(self) -> None:
        slide = {
            "layout": "process",
            "title": "Flow",
            "icon_id": "workflow",
            "steps": [
                {"title": "Plan", "icon_id": "idea"},
                {"title": "Launch", "icon": "@growth"},
            ],
        }
        resolved = pptx_renderer._apply_icon_catalog_to_slide(
            slide,
            {
                "workflow": "network",
                "idea": "spark",
                "growth": "trend",
            },
        )

        self.assertEqual("network", resolved["icon"])
        self.assertEqual("spark", resolved["steps"][0]["icon"])
        self.assertEqual("trend", resolved["steps"][1]["icon"])

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

    def test_read_json_accepts_stdin_spec(self) -> None:
        previous_stdin = sys.stdin
        try:
            sys.stdin = io.StringIO(json.dumps({"slides": [{"layout": "title", "title": "Demo"}]}))
            spec = pptx_renderer._read_json("-")
        finally:
            sys.stdin = previous_stdin

        self.assertEqual("Demo", spec["slides"][0]["title"])

    def test_create_pptx_accepts_slide_background_image(self) -> None:
        try:
            from PIL import Image
        except ImportError:
            self.skipTest("Pillow is not installed")

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bg = root / "bg.png"
            Image.new("RGB", (160, 90), (12, 36, 64)).save(bg)
            spec_path = root / "spec.json"
            out_path = root / "deck.pptx"
            spec_path.write_text(
                json.dumps(
                    {
                        "theme": "nexa-dark",
                        "slides": [
                            {
                                "layout": "body",
                                "title": "Background Image",
                                "bullets": ["Full-bleed background", "Editable foreground"],
                                "background": {
                                    "image_path": str(bg),
                                    "overlay_transparency": 35,
                                    "style": "none",
                                },
                            }
                        ],
                    }
                ),
                encoding="utf-8",
            )

            pptx_renderer.create_pptx_from_spec(str(out_path), str(spec_path), workspace_root=root)

            self.assertTrue(out_path.exists())
            with zipfile.ZipFile(out_path) as zf:
                media = [name for name in zf.namelist() if name.startswith("ppt/media/")]
            self.assertTrue(media)

    def test_create_pptx_accepts_image_catalog_for_background_and_inline_image(self) -> None:
        try:
            from PIL import Image
        except ImportError:
            self.skipTest("Pillow is not installed")

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            hero = root / "hero.png"
            diagram = root / "diagram.png"
            Image.new("RGB", (160, 90), (24, 48, 92)).save(hero)
            Image.new("RGB", (90, 90), (220, 120, 40)).save(diagram)
            spec_path = root / "spec.json"
            out_path = root / "deck.pptx"
            spec_path.write_text(
                json.dumps(
                    {
                        "theme": "product-energy",
                        "images": {
                            "hero": {"path": str(hero)},
                            "diagram": str(diagram),
                        },
                        "slides": [
                            {
                                "layout": "title",
                                "title": "Image Catalog",
                                "background_image_id": "hero",
                            },
                            {
                                "layout": "body",
                                "title": "Inline Image",
                                "bullets": ["Foreground image comes from an alias"],
                                "image_id": "diagram",
                            },
                        ],
                    }
                ),
                encoding="utf-8",
            )

            pptx_renderer.create_pptx_from_spec(str(out_path), str(spec_path), workspace_root=root)

            with zipfile.ZipFile(out_path) as zf:
                media = [name for name in zf.namelist() if name.startswith("ppt/media/")]
            self.assertGreaterEqual(len(media), 2)

    def test_create_pptx_accepts_icon_catalog_and_new_background_presets(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            spec_path = root / "spec.json"
            out_path = root / "deck.pptx"
            spec_path.write_text(
                json.dumps(
                    {
                        "theme": "finance-precision",
                        "icons": {"risk": "shield", "growth": "trend"},
                        "slides": [
                            {
                                "layout": "body",
                                "title": "Icon And Grid",
                                "bullets": ["Consistent built-in icon language", "Editable grid background"],
                                "icon_id": "risk",
                                "background_style": "data_grid",
                            },
                            {
                                "layout": "process",
                                "title": "Signal Flow",
                                "background_style": "blueprint_grid",
                                "steps": [
                                    {"title": "Sense", "detail": "Monitor signal", "icon_id": "growth"},
                                    {"title": "Act", "detail": "Allocate capital", "icon": "check"},
                                ],
                            },
                        ],
                    }
                ),
                encoding="utf-8",
            )

            pptx_renderer.create_pptx_from_spec(str(out_path), str(spec_path), workspace_root=root)

            self.assertTrue(out_path.exists())
            with zipfile.ZipFile(out_path) as zf:
                xml = "\n".join(
                    zf.read(name).decode("utf-8", errors="replace")
                    for name in zf.namelist()
                    if name.startswith("ppt/slides/slide") and name.endswith(".xml")
                )
            self.assertIn("icon-risk", xml)
            self.assertIn("icon-growth", xml)


if __name__ == "__main__":
    unittest.main()
