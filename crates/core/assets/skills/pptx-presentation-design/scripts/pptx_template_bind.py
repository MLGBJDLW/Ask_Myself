#!/usr/bin/env python3
"""Bind a renderer spec to a PPTX template profile.

The binding step learns reusable structure from a template: recommended layout
indices, placeholder intent, and style tokens. It does not copy slide content;
it annotates the spec so the renderer can choose native template layouts and
fill placeholders.
"""

from __future__ import annotations

import argparse
import copy
import json
from pathlib import Path
from typing import Any

import pptx_style_profile
import pptx_template_profile


PURPOSE_BY_LAYOUT = {
    "title": "title",
    "section": "section",
    "table": "table",
    "chart": "chart",
    "comparison": "comparison",
    "matrix": "comparison",
    "two_column": "comparison",
    "agenda": "body",
    "body": "body",
    "timeline": "body",
    "process": "body",
    "stat": "body",
    "quote": "body",
    "image_full": "body",
}


def _purpose(layout: str) -> str:
    return PURPOSE_BY_LAYOUT.get(layout.lower(), "body")


def _recommendation(profile: dict[str, Any], purpose: str) -> dict[str, Any] | None:
    recommendations = profile.get("recommendations") or {}
    return recommendations.get(purpose) or recommendations.get("body")


def bind_spec_to_template(
    spec: dict[str, Any],
    template_profile: dict[str, Any],
    *,
    style_profile: dict[str, Any] | None = None,
    apply_style: bool = True,
) -> dict[str, Any]:
    bound = copy.deepcopy(spec)
    metadata = dict(bound.get("metadata") or {})
    metadata["template_binding"] = {
        "source": "pptx_template_bind",
        "template": template_profile.get("path"),
        "layouts": template_profile.get("layouts"),
    }
    bound["metadata"] = metadata
    if apply_style and style_profile:
        bound["theme"] = style_profile.get("renderer_theme") or bound.get("theme")

    slides = bound.get("slides") or []
    if not isinstance(slides, list):
        return bound
    for slide in slides:
        if not isinstance(slide, dict):
            continue
        layout_name = str(slide.get("layout") or "body").lower()
        purpose = _purpose(layout_name)
        rec = _recommendation(template_profile, purpose)
        if not rec:
            continue
        slide["template_layout_index"] = int(rec["layout_index"])
        slide["template_layout_name"] = str(rec.get("layout_name") or "")
        slide["template_binding"] = {
            "purpose": purpose,
            "strategy": "fill-placeholders",
            "score": rec.get("score"),
        }
    return bound


def bind_spec_file(template: Path, spec_path: Path, *, apply_style: bool = True) -> dict[str, Any]:
    spec = json.loads(spec_path.read_text(encoding="utf-8"))
    profile = pptx_template_profile.profile_template(template)
    style = pptx_style_profile.profile_style(template) if apply_style else None
    return bind_spec_to_template(spec, profile, style_profile=style, apply_style=apply_style)


def main() -> int:
    parser = argparse.ArgumentParser(description="Bind a renderer spec to a PPTX template layout/style profile.")
    parser.add_argument("--template", required=True, help="Path to a .pptx template")
    parser.add_argument("--spec", required=True, help="Input renderer JSON spec")
    parser.add_argument("--out", required=True, help="Output bound JSON spec")
    parser.add_argument("--no-style", action="store_true", help="Do not inject renderer_theme from the template")
    parser.add_argument("--pretty", action="store_true", help="Pretty-print JSON output to stdout")
    args = parser.parse_args()

    bound = bind_spec_file(Path(args.template), Path(args.spec), apply_style=not args.no_style)
    Path(args.out).write_text(json.dumps(bound, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    if args.pretty:
        print(json.dumps(bound, ensure_ascii=False, indent=2))
    else:
        print(f"created bound spec: {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
