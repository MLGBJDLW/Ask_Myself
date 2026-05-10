#!/usr/bin/env python3
"""Tests for the upgraded PPTX pipeline helpers."""

from __future__ import annotations

import json
import tempfile
import unittest
import zipfile
from pathlib import Path

import pptx_asset_pack
import pptx_deck_planner
import pptx_delivery_pack
import pptx_regression_suite
import pptx_rewrite_plan
import pptx_semantic_rewriter
import pptx_style_profile
import pptx_template_bind
import pptx_visual_qa


PRESENTATION_XML = """<?xml version="1.0" encoding="UTF-8"?>
<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:sldSz cx="12192000" cy="6858000"/>
</p:presentation>
"""


def _shape_xml(x: int, y: int, cx: int, cy: int, text: str, fill: str = "FFFFFF", color: str = "111111") -> str:
    return f"""
    <p:sp>
      <p:spPr>
        <a:xfrm>
          <a:off x="{x}" y="{y}"/>
          <a:ext cx="{cx}" cy="{cy}"/>
        </a:xfrm>
        <a:solidFill><a:srgbClr val="{fill}"/></a:solidFill>
      </p:spPr>
      <p:txBody>
        <a:bodyPr/>
        <a:lstStyle/>
        <a:p><a:r><a:rPr><a:solidFill><a:srgbClr val="{color}"/></a:solidFill></a:rPr><a:t>{text}</a:t></a:r></a:p>
      </p:txBody>
    </p:sp>
    """


def _write_minimal_pptx(path: Path, *, overlapping: bool = False) -> None:
    second_x = 960000 if overlapping else 4600000
    slide_xml = f"""<?xml version="1.0" encoding="UTF-8"?>
<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
       xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <p:cSld><p:spTree>
    {_shape_xml(914400, 914400, 2743200, 914400, "First message")}
    {_shape_xml(second_x, 960000, 2743200, 914400, "Second message")}
  </p:spTree></p:cSld>
</p:sld>
"""
    with zipfile.ZipFile(path, "w") as zf:
        zf.writestr("ppt/presentation.xml", PRESENTATION_XML)
        zf.writestr("ppt/slides/slide1.xml", slide_xml)
        zf.writestr("ppt/media/image1.png", b"fake")
        zf.writestr(
            "ppt/slides/_rels/slide1.xml.rels",
            """<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/source" TargetMode="External"/>
</Relationships>
""",
        )


def _write_theme_pptx(path: Path) -> None:
    theme_xml = """<?xml version="1.0" encoding="UTF-8"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <a:themeElements>
    <a:clrScheme name="Custom">
      <a:dk1><a:srgbClr val="101010"/></a:dk1>
      <a:lt1><a:srgbClr val="FFFFFF"/></a:lt1>
      <a:accent1><a:srgbClr val="AA0000"/></a:accent1>
      <a:accent2><a:srgbClr val="00AA00"/></a:accent2>
    </a:clrScheme>
    <a:fontScheme name="Fonts">
      <a:majorFont><a:latin typeface="Georgia"/></a:majorFont>
      <a:minorFont><a:latin typeface="Arial"/></a:minorFont>
    </a:fontScheme>
  </a:themeElements>
</a:theme>
"""
    with zipfile.ZipFile(path, "w") as zf:
        zf.writestr("ppt/presentation.xml", PRESENTATION_XML)
        zf.writestr("ppt/theme/theme1.xml", theme_xml)
        zf.writestr("ppt/slides/slide1.xml", "<p:sld xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"/>")


class PptxUpgradePipelineTests(unittest.TestCase):
    def test_visual_qa_detects_overlap_and_repairs_dense_spec(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            deck = Path(tmp) / "deck.pptx"
            _write_minimal_pptx(deck, overlapping=True)

            report = pptx_visual_qa.analyze_pptx(deck)
            repaired = pptx_visual_qa.repair_spec(
                {"slides": [{"layout": "body", "title": "Dense", "bullets": list("ABCDEFG")}]}
            )

            self.assertEqual("fail", report["status"])
            self.assertTrue(any(issue["code"] == "shape_overlap" for issue in report["issues"]))
            self.assertEqual(2, len(repaired["slides"]))

    def test_style_profile_extracts_renderer_theme(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            deck = Path(tmp) / "theme.pptx"
            _write_theme_pptx(deck)

            profile = pptx_style_profile.profile_style(deck)

            self.assertEqual("AA0000", profile["renderer_theme"]["primary_color"])
            self.assertEqual("Georgia", profile["renderer_theme"]["title_font"])

    def test_deck_planner_preserves_links_and_selects_rich_layouts(self) -> None:
        text = """Growth Plan
Revenue increased 12%
Margin improved 18%
Roadmap Q1 launch
Source https://example.com/report
"""
        spec = pptx_deck_planner.plan_deck(text, audience="leadership", target_slides=5)

        self.assertEqual("pptx_deck_planner", spec["metadata"]["source"])
        self.assertIn("https://example.com/report", spec["metadata"]["source_links"])
        self.assertTrue(any(slide["layout"] in {"chart", "timeline"} for slide in spec["slides"]))

    def test_asset_pack_inventories_media_links_and_missing_spec_assets(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            deck = root / "deck.pptx"
            _write_minimal_pptx(deck)
            spec = root / "spec.json"
            spec.write_text(json.dumps({"slides": [{"image_path": "missing.png", "links": ["https://example.com"]}]}), encoding="utf-8")

            pptx_assets = pptx_asset_pack.inventory_pptx_assets(deck)
            spec_assets = pptx_asset_pack.validate_spec_assets(spec, root)

            self.assertEqual(1, pptx_assets["media_count"])
            self.assertEqual(1, pptx_assets["external_link_count"])
            self.assertEqual("fail", spec_assets["status"])

    def test_rewrite_plan_and_regression_samples_are_renderer_ready(self) -> None:
        report = {
            "warnings": ["slide 2 has no visual anchor"],
            "slide_details": [
                {"index": 1, "text": "Title", "has_visual_anchor": True, "text_chars": 20},
                {"index": 2, "text": "A long operating model summary", "has_visual_anchor": False, "text_chars": 900},
            ],
        }

        plan = pptx_rewrite_plan.build_rewrite_plan(report, target_slides=4)
        samples = pptx_regression_suite.sample_specs()

        self.assertEqual("rewrite-recommended", plan["status"])
        self.assertIn("recommended_spec", plan)
        self.assertIn("data_dashboard", samples)
        self.assertEqual("pptx_semantic_rewriter", plan["recommended_spec"]["metadata"]["source"])

    def test_template_bind_adds_layout_indices_and_style_tokens(self) -> None:
        spec = {"slides": [{"layout": "title", "title": "Quarterly Review"}, {"layout": "chart", "title": "Growth", "categories": ["Q1"], "series": [{"name": "Value", "values": [1]}]}]}
        template_profile = {
            "path": "template.pptx",
            "layouts": 2,
            "recommendations": {
                "title": {"layout_index": 0, "layout_name": "Title Slide", "score": 12},
                "chart": {"layout_index": 1, "layout_name": "Chart", "score": 10},
                "body": {"layout_index": 1, "layout_name": "Body", "score": 8},
            },
        }
        style_profile = {"renderer_theme": {"primary_color": "AA0000", "body_font": "Arial"}}

        bound = pptx_template_bind.bind_spec_to_template(spec, template_profile, style_profile=style_profile)

        self.assertEqual(0, bound["slides"][0]["template_layout_index"])
        self.assertEqual(1, bound["slides"][1]["template_layout_index"])
        self.assertEqual("AA0000", bound["theme"]["primary_color"])

    def test_semantic_rewriter_builds_decision_story(self) -> None:
        report = {
            "slides": 4,
            "slide_details": [
                {"index": 1, "text": "Market context and current customer demand are shifting."},
                {"index": 2, "text": "Revenue increased 12% and retention improved 18%."},
                {"index": 3, "text": "Option A reduces risk versus Option B but creates a tradeoff."},
                {"index": 4, "text": "Recommendation: prioritize Q2 launch and confirm next milestone."},
            ],
        }

        spec = pptx_semantic_rewriter.semantic_rewrite_from_report(report, target_slides=6)

        layouts = {slide["layout"] for slide in spec["slides"]}
        self.assertEqual("pptx_semantic_rewriter", spec["metadata"]["source"])
        self.assertIn("chart", layouts)
        self.assertIn("comparison", layouts)
        self.assertIn("timeline", layouts)

    def test_delivery_pack_writes_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            deck = root / "deck.pptx"
            out_dir = root / "delivery"
            _write_minimal_pptx(deck, overlapping=False)

            manifest = pptx_delivery_pack.create_delivery_pack(deck, out_dir)

            self.assertTrue((out_dir / "manifest.json").exists())
            self.assertEqual(str(deck), manifest["source"])


if __name__ == "__main__":
    unittest.main()
