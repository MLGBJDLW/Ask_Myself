#!/usr/bin/env python3
"""Geometry-based visual QA and spec repair hints for PPTX decks.

This script intentionally uses OOXML rather than screen-only heuristics so it
can run in CI and on machines without Office. If rendered slide images are
available, pass their directory to keep a visual artifact trail next to the
structural issues.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
import zipfile
from pathlib import Path
from typing import Any
from xml.etree import ElementTree as ET


NS = {
    "a": "http://schemas.openxmlformats.org/drawingml/2006/main",
    "p": "http://schemas.openxmlformats.org/presentationml/2006/main",
}

EMU_PER_INCH = 914400
MIN_MARGIN_IN = 0.30


def _read_text(zf: zipfile.ZipFile, name: str) -> str:
    try:
        return zf.read(name).decode("utf-8", errors="replace")
    except KeyError:
        return ""


def _parse_xml(text: str):
    if not text:
        return None
    try:
        return ET.fromstring(text)
    except ET.ParseError:
        return None


def _natural_key(name: str) -> tuple[Any, ...]:
    return tuple(int(part) if part.isdigit() else part for part in re.split(r"(\d+)", name))


def _emu_to_in(value: str | int | None) -> float:
    try:
        return int(value or 0) / EMU_PER_INCH
    except (TypeError, ValueError):
        return 0.0


def _presentation_size(zf: zipfile.ZipFile) -> dict[str, float]:
    root = _parse_xml(_read_text(zf, "ppt/presentation.xml"))
    if root is None:
        return {"width": 13.333333, "height": 7.5}
    sld_sz = root.find(".//p:sldSz", NS)
    if sld_sz is None:
        return {"width": 13.333333, "height": 7.5}
    return {"width": _emu_to_in(sld_sz.get("cx")), "height": _emu_to_in(sld_sz.get("cy"))}


def _shape_text(el) -> str:
    if el is None:
        return ""
    return " ".join(t.text or "" for t in el.findall(".//a:t", NS)).strip()


def _paragraph_count(el) -> int:
    if el is None:
        return 0
    count = 0
    for paragraph in el.findall(".//a:p", NS):
        text = " ".join(t.text or "" for t in paragraph.findall(".//a:t", NS)).strip()
        if text:
            count += 1
    return count


def _bbox(el) -> dict[str, float] | None:
    xfrm = el.find(".//a:xfrm", NS)
    if xfrm is None:
        return None
    off = xfrm.find("a:off", NS)
    ext = xfrm.find("a:ext", NS)
    if off is None or ext is None:
        return None
    left = _emu_to_in(off.get("x"))
    top = _emu_to_in(off.get("y"))
    width = _emu_to_in(ext.get("cx"))
    height = _emu_to_in(ext.get("cy"))
    if width <= 0 or height <= 0:
        return None
    return {
        "left": round(left, 3),
        "top": round(top, 3),
        "width": round(width, 3),
        "height": round(height, 3),
        "right": round(left + width, 3),
        "bottom": round(top + height, 3),
    }


def _hex_color(raw: str | None) -> str | None:
    if not raw:
        return None
    value = raw.strip().lstrip("#")
    if len(value) == 6 and re.fullmatch(r"[0-9a-fA-F]{6}", value):
        return value.upper()
    return None


def _first_color(el, pattern: str) -> str | None:
    node = el.find(pattern, NS)
    if node is not None:
        return _hex_color(node.get("val"))
    return None


def _relative_luminance(hex_color: str) -> float:
    values = [int(hex_color[i : i + 2], 16) / 255.0 for i in (0, 2, 4)]
    channels = [v / 12.92 if v <= 0.03928 else ((v + 0.055) / 1.055) ** 2.4 for v in values]
    return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2]


def _contrast_ratio(fg: str, bg: str) -> float:
    hi = max(_relative_luminance(fg), _relative_luminance(bg))
    lo = min(_relative_luminance(fg), _relative_luminance(bg))
    return (hi + 0.05) / (lo + 0.05)


def _area(box: dict[str, float]) -> float:
    return max(0.0, box["width"]) * max(0.0, box["height"])


def _overlap_ratio(a: dict[str, float], b: dict[str, float]) -> float:
    width = max(0.0, min(a["right"], b["right"]) - max(a["left"], b["left"]))
    height = max(0.0, min(a["bottom"], b["bottom"]) - max(a["top"], b["top"]))
    overlap = width * height
    denom = max(0.001, min(_area(a), _area(b)))
    return overlap / denom


def _is_background(box: dict[str, float], slide_size: dict[str, float]) -> bool:
    return box["width"] >= slide_size["width"] * 0.92 and box["height"] >= slide_size["height"] * 0.92


def _shape_kind(el) -> str:
    tag = str(el.tag).split("}", 1)[-1]
    if tag == "pic":
        return "picture"
    if tag == "graphicFrame":
        return "graphic"
    return "shape"


def _slide_shapes(root, slide_size: dict[str, float]) -> list[dict[str, Any]]:
    result: list[dict[str, Any]] = []
    if root is None:
        return result
    for idx, el in enumerate(root.findall(".//p:sp", NS) + root.findall(".//p:pic", NS) + root.findall(".//p:graphicFrame", NS), start=1):
        box = _bbox(el)
        if not box:
            continue
        text = _shape_text(el)
        fill = _first_color(el, ".//p:spPr/a:solidFill/a:srgbClr") or _first_color(el, ".//a:solidFill/a:srgbClr")
        text_color = _first_color(el, ".//a:rPr/a:solidFill/a:srgbClr")
        result.append(
            {
                "id": idx,
                "kind": _shape_kind(el),
                "text": text,
                "text_chars": len(text),
                "paragraphs": _paragraph_count(el),
                "box": box,
                "fill": fill,
                "text_color": text_color,
                "background": _is_background(box, slide_size),
            }
        )
    return result


def _issue(slide: int, code: str, severity: str, message: str, shape_id: int | None = None) -> dict[str, Any]:
    data: dict[str, Any] = {
        "slide": slide,
        "code": code,
        "severity": severity,
        "message": message,
    }
    if shape_id is not None:
        data["shape_id"] = shape_id
    return data


def _analyze_shape(slide_index: int, shape: dict[str, Any], slide_size: dict[str, float]) -> list[dict[str, Any]]:
    issues: list[dict[str, Any]] = []
    box = shape["box"]
    if shape["background"]:
        return issues
    if min(box["left"], box["top"], slide_size["width"] - box["right"], slide_size["height"] - box["bottom"]) < MIN_MARGIN_IN:
        issues.append(_issue(slide_index, "edge_margin", "warn", "shape is close to a slide edge", shape["id"]))
    if shape["text_chars"]:
        chars_per_line = max(8, int(box["width"] * 11))
        max_lines = max(1, int(box["height"] * 2.6))
        needed_lines = math.ceil(shape["text_chars"] / chars_per_line) + max(0, int(shape["paragraphs"]) - 1)
        if needed_lines > max_lines:
            issues.append(
                _issue(
                    slide_index,
                    "text_overflow_risk",
                    "fail",
                    f"text likely needs {needed_lines} lines but box fits about {max_lines}",
                    shape["id"],
                )
            )
        if shape.get("fill") and shape.get("text_color"):
            contrast = _contrast_ratio(shape["text_color"], shape["fill"])
            if contrast < 4.5:
                issues.append(
                    _issue(
                        slide_index,
                        "low_contrast",
                        "fail",
                        f"text contrast is {contrast:.2f}:1, below 4.5:1",
                        shape["id"],
                    )
                )
    return issues


def analyze_pptx(path: Path, render_dir: Path | None = None) -> dict[str, Any]:
    if not path.exists():
        raise FileNotFoundError(path)
    slide_reports: list[dict[str, Any]] = []
    issues: list[dict[str, Any]] = []
    with zipfile.ZipFile(path) as zf:
        slide_size = _presentation_size(zf)
        slide_parts = sorted(
            [name for name in zf.namelist() if re.fullmatch(r"ppt/slides/slide\d+\.xml", name)],
            key=_natural_key,
        )
        for slide_index, slide_name in enumerate(slide_parts, start=1):
            root = _parse_xml(_read_text(zf, slide_name))
            shapes = _slide_shapes(root, slide_size)
            slide_issues: list[dict[str, Any]] = []
            for shape in shapes:
                slide_issues.extend(_analyze_shape(slide_index, shape, slide_size))
            comparable = [shape for shape in shapes if not shape["background"] and _area(shape["box"]) >= 0.05]
            for left_idx, left in enumerate(comparable):
                for right in comparable[left_idx + 1 :]:
                    both_text = bool(left["text_chars"] and right["text_chars"])
                    visual_over_text = (
                        bool(left["text_chars"] or right["text_chars"])
                        and {left["kind"], right["kind"]}.intersection({"picture", "graphic"})
                    )
                    if not (both_text or visual_over_text):
                        continue
                    ratio = _overlap_ratio(left["box"], right["box"])
                    if ratio >= 0.12:
                        slide_issues.append(
                            _issue(
                                slide_index,
                                "shape_overlap",
                                "fail",
                                f"shape {left['id']} overlaps shape {right['id']} by {ratio:.0%}",
                            )
                        )
            if render_dir:
                image_candidates = [
                    render_dir / f"slide-{slide_index:02d}.jpg",
                    render_dir / f"slide-{slide_index:02d}.png",
                    render_dir / f"slide-{slide_index}.jpg",
                    render_dir / f"slide-{slide_index}.png",
                ]
                if not any(candidate.exists() for candidate in image_candidates):
                    slide_issues.append(_issue(slide_index, "missing_render", "warn", "no rendered slide image found"))
            issues.extend(slide_issues)
            slide_reports.append(
                {
                    "index": slide_index,
                    "shape_count": len(shapes),
                    "text_shape_count": sum(1 for shape in shapes if shape["text_chars"]),
                    "issue_count": len(slide_issues),
                    "issues": slide_issues,
                }
            )
    failures = [issue for issue in issues if issue["severity"] == "fail"]
    return {
        "status": "fail" if failures else "pass",
        "path": str(path),
        "slides": len(slide_reports),
        "issue_count": len(issues),
        "failure_count": len(failures),
        "issues": issues,
        "slide_details": slide_reports,
        "render_dir": str(render_dir) if render_dir else None,
    }


def repair_spec(spec: dict[str, Any], *, max_bullets: int = 5) -> dict[str, Any]:
    """Return a renderer spec with dense body slides split into smaller slides."""
    repaired = dict(spec)
    slides = list(spec.get("slides") or [])
    new_slides: list[dict[str, Any]] = []
    for slide in slides:
        if not isinstance(slide, dict):
            continue
        bullets = slide.get("bullets")
        layout = str(slide.get("layout") or "body").lower()
        if layout == "body" and isinstance(bullets, list) and len(bullets) > max_bullets:
            chunks = [bullets[i : i + max_bullets] for i in range(0, len(bullets), max_bullets)]
            for idx, chunk in enumerate(chunks, start=1):
                clone = dict(slide)
                clone["bullets"] = chunk
                if idx > 1:
                    clone["title"] = f"{slide.get('title', 'Continued')} ({idx})"
                clone.setdefault("notes", f"Continuation of {slide.get('title', 'the topic')}.")
                new_slides.append(clone)
        else:
            clone = dict(slide)
            title = str(clone.get("title") or "")
            if len(title) > 95:
                clone["title"] = title[:92].rstrip() + "..."
                clone["notes"] = (str(clone.get("notes") or "") + f"\nFull title: {title}").strip()
            new_slides.append(clone)
    repaired["slides"] = new_slides
    if isinstance(repaired.get("notes_per_slide"), list) and len(repaired["notes_per_slide"]) != len(new_slides):
        repaired.pop("notes_per_slide", None)
    return repaired


def main() -> int:
    parser = argparse.ArgumentParser(description="Run visual QA heuristics for a PPTX deck.")
    parser.add_argument("--path", required=True, help="Path to a .pptx file")
    parser.add_argument("--render-dir", default=None, help="Optional directory containing rendered slide images")
    parser.add_argument("--spec", default=None, help="Optional renderer JSON spec to repair")
    parser.add_argument("--out-spec", default=None, help="Write repaired JSON spec here")
    parser.add_argument("--pretty", action="store_true", help="Pretty-print JSON output")
    args = parser.parse_args()

    report = analyze_pptx(Path(args.path), Path(args.render_dir) if args.render_dir else None)
    if args.spec and args.out_spec:
        spec = json.loads(Path(args.spec).read_text(encoding="utf-8"))
        repaired = repair_spec(spec)
        Path(args.out_spec).write_text(json.dumps(repaired, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        report["repaired_spec"] = str(Path(args.out_spec))
    print(json.dumps(report, ensure_ascii=False, indent=2 if args.pretty else None))
    return 0 if report["status"] == "pass" else 4


if __name__ == "__main__":
    raise SystemExit(main())
