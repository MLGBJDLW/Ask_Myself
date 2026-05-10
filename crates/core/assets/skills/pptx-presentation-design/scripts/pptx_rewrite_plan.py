#!/usr/bin/env python3
"""Plan how to rewrite, condense, or beautify an existing PPTX deck."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

import pptx_audit
import pptx_semantic_rewriter


def _slide_summary(slide: dict[str, Any]) -> str:
    text = str(slide.get("text") or "").strip()
    return text[:240] if text else f"Slide {slide.get('index')} has limited extractable text."


def build_rewrite_plan(report: dict[str, Any], *, target_slides: int | None = None, audience: str = "executive") -> dict[str, Any]:
    slides = list(report.get("slide_details") or [])
    warnings = list(report.get("warnings") or [])
    target = target_slides or min(12, max(4, len(slides)))
    actions: list[dict[str, Any]] = []
    for slide in slides:
        index = int(slide.get("index") or 0)
        slide_actions: list[str] = []
        if int(slide.get("text_chars") or 0) > 750:
            slide_actions.append("split or condense dense text")
        if not bool(slide.get("has_visual_anchor")) and index > 1:
            slide_actions.append("replace bullets with chart, process, comparison, or stat callout")
        if int(slide.get("empty_placeholders") or 0) > 0:
            slide_actions.append("remove unused placeholders")
        if int(slide.get("full_slide_pictures") or 0) > 0 and int(slide.get("text_chars") or 0) < 40:
            slide_actions.append("rebuild as editable shapes unless poster-style output is intentional")
        if slide_actions:
            actions.append({"slide": index, "actions": slide_actions})
    semantic_report = dict(report)
    semantic_report["slide_details"] = [
        {**slide, "text": _slide_summary(slide)} for slide in slides
    ]
    spec = pptx_semantic_rewriter.semantic_rewrite_from_report(
        semantic_report,
        audience=audience,
        target_slides=target,
    )
    return {
        "status": "rewrite-recommended" if actions or warnings else "light-polish",
        "target_slides": target,
        "warnings": warnings,
        "actions": actions,
        "recommended_spec": spec,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Create a rewrite plan for an existing PPTX deck.")
    parser.add_argument("--path", required=True, help="Path to a .pptx file")
    parser.add_argument("--target-slides", type=int, default=None, help="Target slide count for condensed rewrite")
    parser.add_argument("--audience", default="executive", help="Target audience")
    parser.add_argument("--out-spec", default=None, help="Write recommended renderer spec here")
    parser.add_argument("--pretty", action="store_true", help="Pretty-print JSON output")
    args = parser.parse_args()
    report = pptx_audit.audit(Path(args.path))
    plan = build_rewrite_plan(report, target_slides=args.target_slides, audience=args.audience)
    if args.out_spec:
        Path(args.out_spec).write_text(json.dumps(plan["recommended_spec"], ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(plan, ensure_ascii=False, indent=2 if args.pretty else None))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
