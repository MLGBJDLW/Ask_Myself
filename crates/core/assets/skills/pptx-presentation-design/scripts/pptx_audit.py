#!/usr/bin/env python3
"""Audit a PPTX package and print a compact JSON structural summary."""

from __future__ import annotations

import argparse
import json
import re
import sys
import zipfile
from pathlib import Path
from xml.etree import ElementTree as ET


NS = {
    "a": "http://schemas.openxmlformats.org/drawingml/2006/main",
    "p": "http://schemas.openxmlformats.org/presentationml/2006/main",
    "rel": "http://schemas.openxmlformats.org/package/2006/relationships",
}


def read_text(zf: zipfile.ZipFile, name: str) -> str:
    try:
        return zf.read(name).decode("utf-8", errors="replace")
    except KeyError:
        return ""


def parse_xml(text: str):
    if not text:
        return None
    try:
        return ET.fromstring(text)
    except ET.ParseError:
        return None


def natural_key(name: str) -> tuple:
    return tuple(int(part) if part.isdigit() else part for part in re.split(r"(\d+)", name))


def local_rels_path(part_name: str) -> str:
    parent, filename = part_name.rsplit("/", 1)
    return f"{parent}/_rels/{filename}.rels"


def rel_targets(zf: zipfile.ZipFile, part_name: str) -> list[dict[str, str]]:
    rels = parse_xml(read_text(zf, local_rels_path(part_name)))
    out: list[dict[str, str]] = []
    if rels is None:
        return out
    base = part_name.rsplit("/", 1)[0]
    for rel in rels.findall("rel:Relationship", NS):
        target = rel.attrib.get("Target", "")
        mode = rel.attrib.get("TargetMode", "")
        rel_type = rel.attrib.get("Type", "")
        if mode != "External" and not target.startswith("/"):
            target = f"{base}/{target}"
        out.append({"type": rel_type, "target": target, "mode": mode, "target_mode": mode or "Internal"})
    return out


def slide_text(root) -> str:
    if root is None:
        return ""
    return " ".join(t.text or "" for t in root.findall(".//a:t", NS)).strip()


def nonempty_text_paragraphs(root) -> int:
    if root is None:
        return 0
    count = 0
    for paragraph in root.findall(".//a:p", NS):
        text = " ".join(t.text or "" for t in paragraph.findall(".//a:t", NS)).strip()
        if text:
            count += 1
    return count


def count_placeholders_without_text(root) -> int:
    if root is None:
        return 0
    empty = 0
    for shape in root.findall(".//p:sp", NS):
        if shape.find(".//p:ph", NS) is None:
            continue
        text = " ".join(t.text or "" for t in shape.findall(".//a:t", NS)).strip()
        if not text:
            empty += 1
    return empty


def full_slide_picture_count(root, slide_size: dict[str, int] | None) -> int:
    if root is None or not slide_size:
        return 0
    count = 0
    slide_cx = slide_size.get("cx") or 0
    slide_cy = slide_size.get("cy") or 0
    if not slide_cx or not slide_cy:
        return 0
    for pic in root.findall(".//p:pic", NS):
        xfrm = pic.find(".//a:xfrm", NS)
        if xfrm is None:
            continue
        ext = xfrm.find("a:ext", NS)
        if ext is None:
            continue
        cx = int(ext.attrib.get("cx", "0") or 0)
        cy = int(ext.attrib.get("cy", "0") or 0)
        if cx >= slide_cx * 0.9 and cy >= slide_cy * 0.9:
            count += 1
    return count


def presentation_size(zf: zipfile.ZipFile) -> dict[str, int] | None:
    root = parse_xml(read_text(zf, "ppt/presentation.xml"))
    if root is None:
        return None
    size = root.find("p:sldSz", NS)
    if size is None:
        return None
    return {
        "cx": int(size.attrib.get("cx", "0") or 0),
        "cy": int(size.attrib.get("cy", "0") or 0),
    }


def audit(path: Path) -> dict:
    warnings: list[str] = []
    with zipfile.ZipFile(path) as zf:
        names = set(zf.namelist())
        slide_size = presentation_size(zf)
        slide_names = sorted(
            [name for name in names if re.match(r"ppt/slides/slide\d+\.xml$", name)],
            key=natural_key,
        )
        layouts = [name for name in names if re.match(r"ppt/slideLayouts/slideLayout\d+\.xml$", name)]
        masters = [name for name in names if re.match(r"ppt/slideMasters/slideMaster\d+\.xml$", name)]
        themes = [name for name in names if re.match(r"ppt/theme/theme\d+\.xml$", name)]

        slides = []
        for index, slide_name in enumerate(slide_names, start=1):
            root = parse_xml(read_text(zf, slide_name))
            rels = rel_targets(zf, slide_name)
            text = slide_text(root)
            chart_count = sum(1 for rel in rels if "/charts/" in rel["target"])
            image_count = sum(1 for rel in rels if "/media/" in rel["target"])
            notes_count = sum(1 for rel in rels if "notesSlide" in rel["type"])
            external_count = sum(1 for rel in rels if rel.get("target_mode") == "External")
            hyperlink_count = sum(1 for rel in rels if "hyperlink" in rel["type"])
            graphic_frames = len(root.findall(".//p:graphicFrame", NS)) if root is not None else 0
            pictures = len(root.findall(".//p:pic", NS)) if root is not None else 0
            shapes = len(root.findall(".//p:sp", NS)) if root is not None else 0
            empty_placeholders = count_placeholders_without_text(root)
            paragraph_count = nonempty_text_paragraphs(root)
            full_slide_pictures = full_slide_picture_count(root, slide_size)
            has_visual_anchor = any([chart_count, image_count, graphic_frames, pictures]) or shapes >= 5
            if index > 1 and not has_visual_anchor:
                warnings.append(f"slide {index} has no visual anchor")
            if index > 1 and not text:
                warnings.append(f"slide {index} has no extractable text")
            if text and len(text) > 900:
                warnings.append(f"slide {index} is text dense ({len(text)} chars)")
            if paragraph_count > 12 and len(text) > 220:
                warnings.append(f"slide {index} has many text paragraphs ({paragraph_count})")
            if empty_placeholders:
                warnings.append(f"slide {index} has {empty_placeholders} empty placeholder(s)")
            if full_slide_pictures and len(text) < 40:
                warnings.append(f"slide {index} may be a low-editability full-slide image")
            slides.append(
                {
                    "index": index,
                    "part": slide_name,
                    "text": text[:1200],
                    "text_chars": len(text),
                    "shapes": shapes,
                    "pictures": pictures,
                    "graphic_frames": graphic_frames,
                    "image_relationships": image_count,
                    "chart_relationships": chart_count,
                    "notes_relationships": notes_count,
                    "external_relationships": external_count,
                    "hyperlink_relationships": hyperlink_count,
                    "empty_placeholders": empty_placeholders,
                    "text_paragraphs": paragraph_count,
                    "has_visual_anchor": has_visual_anchor,
                    "full_slide_pictures": full_slide_pictures,
                }
            )

        if len(slides) > 3 and not any(slide["notes_relationships"] for slide in slides):
            warnings.append("deck has no speaker notes")

        return {
            "path": str(path),
            "format": "pptx",
            "package_parts": len(names),
            "slides": len(slides),
            "layouts": len(layouts),
            "masters": len(masters),
            "themes": len(themes),
            "slide_size": slide_size,
            "slide_details": slides,
            "external_links": sum(slide["external_relationships"] for slide in slides),
            "hyperlinks": sum(slide["hyperlink_relationships"] for slide in slides),
            "warnings": warnings,
        }


def main() -> int:
    parser = argparse.ArgumentParser(description="Audit PPTX OOXML structure.")
    parser.add_argument("--path", required=True, help="Path to a .pptx file")
    parser.add_argument("--pretty", action="store_true", help="Pretty-print JSON")
    args = parser.parse_args()

    path = Path(args.path).expanduser().resolve()
    if not path.exists():
        print(f"File not found: {path}", file=sys.stderr)
        return 3
    if path.suffix.lower() != ".pptx":
        print(f"Expected .pptx file: {path}", file=sys.stderr)
        return 3
    if not zipfile.is_zipfile(path):
        print(f"Not a valid OOXML zip package: {path}", file=sys.stderr)
        return 3

    result = audit(path)
    print(json.dumps(result, ensure_ascii=False, indent=2 if args.pretty else None))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
