#!/usr/bin/env python3
"""Profile PPTX templates and recommend layout usage.

This script reads OOXML directly so it can run before python-pptx template
editing. It is intended for template-aware deck generation: inspect masters,
layouts, placeholder types, and layout suitability before rendering slides.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import zipfile
from pathlib import Path
from typing import Any
from xml.etree import ElementTree as ET


NS = {
    "a": "http://schemas.openxmlformats.org/drawingml/2006/main",
    "p": "http://schemas.openxmlformats.org/presentationml/2006/main",
    "r": "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    "rel": "http://schemas.openxmlformats.org/package/2006/relationships",
}


LAYOUT_NAME_RE = re.compile(r"ppt/slideLayouts/slideLayout(\d+)\.xml$")


def _read_text(zf: zipfile.ZipFile, name: str) -> str:
    try:
        return zf.read(name).decode("utf-8", errors="replace")
    except KeyError:
        return ""


def _parse_xml(text: str) -> ET.Element | None:
    if not text:
        return None
    try:
        return ET.fromstring(text)
    except ET.ParseError:
        return None


def _natural_key(path: str) -> tuple[str, int]:
    match = LAYOUT_NAME_RE.match(path)
    if match:
        return ("slideLayout", int(match.group(1)))
    return (path, 0)


def _rel_path_for_part(part: str) -> str:
    parent, name = part.rsplit("/", 1)
    return f"{parent}/_rels/{name}.rels"


def _rels(zf: zipfile.ZipFile, part: str) -> list[dict[str, str]]:
    root = _parse_xml(_read_text(zf, _rel_path_for_part(part)))
    if root is None:
        return []
    return [
        {
            "id": rel.attrib.get("Id", ""),
            "type": rel.attrib.get("Type", ""),
            "target": rel.attrib.get("Target", ""),
        }
        for rel in root.findall("rel:Relationship", NS)
    ]


def _layout_name(root: ET.Element | None, fallback: str) -> str:
    if root is None:
        return fallback
    c_sld = root.find("p:cSld", NS)
    if c_sld is not None and c_sld.attrib.get("name"):
        return c_sld.attrib["name"]
    return fallback


def _placeholder_inventory(root: ET.Element | None) -> list[dict[str, Any]]:
    if root is None:
        return []
    placeholders: list[dict[str, Any]] = []
    for shape in root.findall(".//p:sp", NS):
        nv_pr = shape.find(".//p:nvPr", NS)
        ph = nv_pr.find("p:ph", NS) if nv_pr is not None else None
        if ph is None:
            continue
        name_node = shape.find(".//p:cNvPr", NS)
        xfrm = shape.find(".//a:xfrm", NS)
        off = xfrm.find("a:off", NS) if xfrm is not None else None
        ext = xfrm.find("a:ext", NS) if xfrm is not None else None
        placeholders.append(
            {
                "type": ph.attrib.get("type", "body"),
                "idx": ph.attrib.get("idx"),
                "name": name_node.attrib.get("name", "") if name_node is not None else "",
                "x": int(off.attrib.get("x", "0")) if off is not None else None,
                "y": int(off.attrib.get("y", "0")) if off is not None else None,
                "cx": int(ext.attrib.get("cx", "0")) if ext is not None else None,
                "cy": int(ext.attrib.get("cy", "0")) if ext is not None else None,
            }
        )
    return placeholders


def _score_layout(placeholders: list[dict[str, Any]], graphics: int) -> dict[str, int]:
    types = [str(item.get("type") or "body") for item in placeholders]
    title_like = sum(1 for item in types if item in {"title", "ctrTitle", "subTitle"})
    body_like = sum(1 for item in types if item in {"body", "obj", "content", "pic"})
    table_like = sum(1 for item in types if item in {"tbl", "obj", "content"})
    chart_like = sum(1 for item in types if item in {"chart", "obj", "content"}) + graphics
    simple_body_bonus = 8 if 1 <= body_like <= 2 else 0
    crowding_penalty = max(0, body_like - 2) * 3
    return {
        "title": 4 * title_like + body_like,
        "body": 6 * title_like + simple_body_bonus - crowding_penalty,
        "section": 5 * title_like - body_like,
        "table": 3 * table_like + min(body_like, 2) + title_like,
        "chart": 3 * chart_like + min(body_like, 2) + title_like,
        "comparison": 2 * body_like + max(0, len(placeholders) - 2),
    }


def _best_recommendations(layouts: list[dict[str, Any]]) -> dict[str, Any]:
    recommendations: dict[str, Any] = {}
    for purpose in ["title", "body", "section", "table", "chart", "comparison"]:
        candidates = sorted(
            layouts,
            key=lambda item: (item["scores"].get(purpose, 0), -item["index"]),
            reverse=True,
        )
        if candidates and candidates[0]["scores"].get(purpose, 0) > 0:
            recommendations[purpose] = {
                "layout_index": candidates[0]["index"],
                "layout_name": candidates[0]["name"],
                "score": candidates[0]["scores"].get(purpose, 0),
            }
    return recommendations


def profile_template(path: Path) -> dict[str, Any]:
    with zipfile.ZipFile(path) as zf:
        names = set(zf.namelist())
        layout_parts = sorted(
            [name for name in names if LAYOUT_NAME_RE.match(name)],
            key=_natural_key,
        )
        slide_parts = sorted(
            [name for name in names if re.match(r"ppt/slides/slide\d+\.xml$", name)],
            key=lambda value: int(re.search(r"slide(\d+)\.xml$", value).group(1)),
        )
        layouts: list[dict[str, Any]] = []
        for zero_index, part in enumerate(layout_parts):
            root = _parse_xml(_read_text(zf, part))
            placeholders = _placeholder_inventory(root)
            rels = _rels(zf, part)
            graphics = len(root.findall(".//p:graphicFrame", NS)) if root is not None else 0
            layouts.append(
                {
                    "index": zero_index,
                    "part": part,
                    "name": _layout_name(root, f"Layout {zero_index}"),
                    "placeholder_count": len(placeholders),
                    "placeholder_types": sorted(
                        {str(item.get("type") or "body") for item in placeholders}
                    ),
                    "placeholders": placeholders,
                    "image_relationships": sum(1 for rel in rels if "/media/" in rel["target"]),
                    "chart_relationships": sum(1 for rel in rels if "/charts/" in rel["target"]),
                    "graphic_frames": graphics,
                    "scores": _score_layout(placeholders, graphics),
                }
            )

        return {
            "path": str(path),
            "format": "pptx-template-profile",
            "slides": len(slide_parts),
            "layouts": len(layouts),
            "masters": len([name for name in names if re.match(r"ppt/slideMasters/slideMaster\d+\.xml$", name)]),
            "themes": len([name for name in names if re.match(r"ppt/theme/theme\d+\.xml$", name)]),
            "layout_details": layouts,
            "recommendations": _best_recommendations(layouts),
        }


def main() -> int:
    parser = argparse.ArgumentParser(description="Profile PPTX template layouts and placeholders.")
    parser.add_argument("--path", required=True, help="Path to a .pptx template or deck")
    parser.add_argument("--pretty", action="store_true", help="Pretty-print JSON output")
    args = parser.parse_args()

    path = Path(args.path)
    if not path.exists() or path.suffix.lower() != ".pptx":
        print("ERROR: --path must point to an existing .pptx file", file=sys.stderr)
        return 3
    result = profile_template(path)
    print(json.dumps(result, ensure_ascii=False, indent=2 if args.pretty else None))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
