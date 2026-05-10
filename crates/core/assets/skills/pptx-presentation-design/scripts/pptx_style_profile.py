#!/usr/bin/env python3
"""Extract reusable visual style tokens from a PPTX template or deck."""

from __future__ import annotations

import argparse
import json
import re
import zipfile
from collections import Counter
from pathlib import Path
from typing import Any
from xml.etree import ElementTree as ET


NS = {
    "a": "http://schemas.openxmlformats.org/drawingml/2006/main",
    "p": "http://schemas.openxmlformats.org/presentationml/2006/main",
}


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


def _hex(raw: str | None) -> str | None:
    if not raw:
        return None
    value = raw.strip().lstrip("#")
    if re.fullmatch(r"[0-9a-fA-F]{6}", value):
        return value.upper()
    return None


def _color_value(node) -> str | None:
    if node is None:
        return None
    srgb = node.find(".//a:srgbClr", NS)
    if srgb is not None:
        return _hex(srgb.get("val"))
    sysclr = node.find(".//a:sysClr", NS)
    if sysclr is not None:
        return _hex(sysclr.get("lastClr"))
    return None


def _theme_part_names(zf: zipfile.ZipFile) -> list[str]:
    return sorted(name for name in zf.namelist() if re.fullmatch(r"ppt/theme/theme\d+\.xml", name))


def _extract_theme(root) -> dict[str, Any]:
    colors: dict[str, str] = {}
    fonts: dict[str, str] = {}
    if root is None:
        return {"colors": colors, "fonts": fonts}
    clr_scheme = root.find(".//a:clrScheme", NS)
    if clr_scheme is not None:
        for child in list(clr_scheme):
            key = str(child.tag).split("}", 1)[-1]
            value = _color_value(child)
            if value:
                colors[key] = value
    font_scheme = root.find(".//a:fontScheme", NS)
    if font_scheme is not None:
        major = font_scheme.find(".//a:majorFont/a:latin", NS)
        minor = font_scheme.find(".//a:minorFont/a:latin", NS)
        if major is not None and major.get("typeface"):
            fonts["title_font"] = str(major.get("typeface"))
        if minor is not None and minor.get("typeface"):
            fonts["body_font"] = str(minor.get("typeface"))
    return {"colors": colors, "fonts": fonts}


def _scan_slide_colors(root) -> Counter[str]:
    found: Counter[str] = Counter()
    if root is None:
        return found
    for node in root.findall(".//a:srgbClr", NS):
        value = _hex(node.get("val"))
        if value:
            found[value] += 1
    return found


def _renderer_theme(theme: dict[str, Any], sampled_colors: Counter[str]) -> dict[str, str]:
    colors = dict(theme.get("colors") or {})
    fonts = dict(theme.get("fonts") or {})
    frequent = [value for value, _ in sampled_colors.most_common()]
    return {
        "primary_color": colors.get("accent1") or (frequent[0] if frequent else "2563EB"),
        "accent_color": colors.get("accent2") or (frequent[1] if len(frequent) > 1 else "0F766E"),
        "background_color": colors.get("lt1") or "FFFFFF",
        "surface_color": colors.get("lt2") or "F8FAFC",
        "muted_surface_color": colors.get("accent6") or "E2E8F0",
        "text_color": colors.get("dk1") or "111827",
        "muted_text_color": colors.get("dk2") or "64748B",
        "title_color": colors.get("dk1") or "0F172A",
        "inverse_text_color": colors.get("lt1") or "FFFFFF",
        "title_font": fonts.get("title_font") or "Aptos Display",
        "body_font": fonts.get("body_font") or "Aptos",
    }


def profile_style(path: Path) -> dict[str, Any]:
    if not path.exists():
        raise FileNotFoundError(path)
    with zipfile.ZipFile(path) as zf:
        theme_names = _theme_part_names(zf)
        theme = _extract_theme(_parse_xml(_read_text(zf, theme_names[0]))) if theme_names else {"colors": {}, "fonts": {}}
        sampled: Counter[str] = Counter()
        for name in zf.namelist():
            if re.fullmatch(r"ppt/(slides|slideLayouts|slideMasters)/.+\.xml", name):
                sampled.update(_scan_slide_colors(_parse_xml(_read_text(zf, name))))
    return {
        "path": str(path),
        "theme_parts": theme_names,
        "theme_colors": theme.get("colors", {}),
        "theme_fonts": theme.get("fonts", {}),
        "sampled_colors": [{"color": color, "count": count} for color, count in sampled.most_common(12)],
        "renderer_theme": _renderer_theme(theme, sampled),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Extract PPTX style tokens and a renderer theme suggestion.")
    parser.add_argument("--path", required=True, help="Path to a .pptx file")
    parser.add_argument("--pretty", action="store_true", help="Pretty-print JSON output")
    args = parser.parse_args()
    print(json.dumps(profile_style(Path(args.path)), ensure_ascii=False, indent=2 if args.pretty else None))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
