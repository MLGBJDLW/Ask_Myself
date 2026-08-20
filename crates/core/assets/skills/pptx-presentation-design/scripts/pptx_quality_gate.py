#!/usr/bin/env python3
"""Quality gate for editable PPTX decks.

Consumes the deterministic inventory from pptx_audit.py and turns it into a
publishability signal that can be used by local smoke tests or CI.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path
from typing import Any

import pptx_audit


PASS = "pass"
FAIL = "fail"
WARN = "warn"


def _slide_indices(slides: list[dict[str, Any]], predicate) -> list[int]:
    return [int(slide.get("index", 0)) for slide in slides if predicate(slide)]


def _ratio(count: int, total: int) -> float:
    if total <= 0:
        return 1.0
    return count / total


def _check(name: str, status: str, metric: Any, threshold: Any, detail: str = "") -> dict[str, Any]:
    return {
        "name": name,
        "status": status,
        "metric": metric,
        "threshold": threshold,
        "detail": detail,
    }


def evaluate_audit(
    report: dict[str, Any],
    *,
    visual_report: dict[str, Any] | None = None,
    strict: bool = False,
    require_notes: bool = False,
    min_slides: int = 1,
    max_warnings: int = 0,
    min_notes_coverage: float | None = None,
    max_text_chars: int | None = None,
    max_text_paragraphs: int | None = None,
    rendered_previews: list[str] | None = None,
    require_render: bool = False,
) -> dict[str, Any]:
    slides = list(report.get("slide_details") or [])
    content_slides = [slide for slide in slides if int(slide.get("index", 0)) > 1]
    warnings = list(report.get("warnings") or [])

    if strict:
        require_notes = True
        max_warnings = min(max_warnings, 0)
        max_text_chars = max_text_chars or 800
        max_text_paragraphs = max_text_paragraphs or 14
    else:
        max_text_chars = max_text_chars or 900
        max_text_paragraphs = max_text_paragraphs or 16

    if min_notes_coverage is None:
        min_notes_coverage = 0.8 if require_notes else 0.0

    failures: list[str] = []
    cautions: list[str] = []
    checks: list[dict[str, Any]] = []

    slide_count = int(report.get("slides") or len(slides))
    if slide_count < min_slides:
        failures.append(f"slide count below minimum: {slide_count} < {min_slides}")
        checks.append(_check("slide_count", FAIL, slide_count, min_slides))
    else:
        checks.append(_check("slide_count", PASS, slide_count, min_slides))

    validation_errors = list(report.get("validation_errors") or [])
    if validation_errors:
        failures.extend(f"package validation: {message}" for message in validation_errors)
        checks.append(_check("package_graph", FAIL, len(validation_errors), 0))
    else:
        checks.append(_check("package_graph", PASS, 0, 0))

    render_paths = [Path(path) for path in (rendered_previews or [])]
    existing_render_paths = [path for path in render_paths if path.is_file()]
    render_hashes = {
        str(path): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in existing_render_paths
    }
    render_coverage = _ratio(len(existing_render_paths), slide_count)
    if require_render and len(existing_render_paths) != slide_count:
        failures.append(
            f"final PPTX render coverage incomplete: {len(existing_render_paths)} != {slide_count}"
        )
        checks.append(_check("final_pptx_render", FAIL, len(existing_render_paths), slide_count))
    elif existing_render_paths:
        status = PASS if len(existing_render_paths) == slide_count else WARN
        if status == WARN:
            cautions.append("final PPTX render coverage is partial")
        checks.append(_check("final_pptx_render", status, len(existing_render_paths), slide_count))
    else:
        checks.append(_check("final_pptx_render", WARN, 0, slide_count))
        if slide_count:
            cautions.append("final PPTX render evidence is absent")

    if len(warnings) > max_warnings:
        failures.append(f"audit warnings exceed budget: {len(warnings)} > {max_warnings}")
        checks.append(_check("audit_warnings", FAIL, len(warnings), max_warnings))
    else:
        checks.append(_check("audit_warnings", PASS, len(warnings), max_warnings))

    missing_visual = _slide_indices(content_slides, lambda slide: not bool(slide.get("has_visual_anchor")))
    if missing_visual:
        failures.append(f"slides missing visual anchors: {', '.join(map(str, missing_visual))}")
        checks.append(_check("visual_anchors", FAIL, len(content_slides) - len(missing_visual), len(content_slides)))
    else:
        checks.append(_check("visual_anchors", PASS, len(content_slides), len(content_slides)))

    full_slide_images = _slide_indices(
        slides,
        lambda slide: int(slide.get("full_slide_pictures") or 0) > 0 and int(slide.get("text_chars") or 0) < 40,
    )
    if full_slide_images:
        failures.append(f"low-editability full-slide images: {', '.join(map(str, full_slide_images))}")
        checks.append(_check("editable_content", FAIL, len(full_slide_images), 0))
    else:
        checks.append(_check("editable_content", PASS, 0, 0))

    empty_placeholders = _slide_indices(slides, lambda slide: int(slide.get("empty_placeholders") or 0) > 0)
    if empty_placeholders:
        failures.append(f"slides with empty placeholders: {', '.join(map(str, empty_placeholders))}")
        checks.append(_check("empty_placeholders", FAIL, len(empty_placeholders), 0))
    else:
        checks.append(_check("empty_placeholders", PASS, 0, 0))

    dense_slides = _slide_indices(
        slides,
        lambda slide: int(slide.get("text_chars") or 0) > max_text_chars
        or (
            int(slide.get("text_paragraphs") or 0) > max_text_paragraphs
            and int(slide.get("text_chars") or 0) > 220
        ),
    )
    if dense_slides:
        failures.append(f"text-dense slides: {', '.join(map(str, dense_slides))}")
        checks.append(_check("text_density", FAIL, len(dense_slides), 0))
    else:
        checks.append(_check("text_density", PASS, 0, 0))

    noted = sum(1 for slide in slides if int(slide.get("notes_relationships") or 0) > 0)
    notes_coverage = _ratio(noted, len(slides))
    if notes_coverage < min_notes_coverage:
        failures.append(f"speaker notes coverage below {round(min_notes_coverage * 100)}%")
        checks.append(_check("speaker_notes", FAIL, round(notes_coverage, 3), min_notes_coverage))
    elif require_notes:
        checks.append(_check("speaker_notes", PASS, round(notes_coverage, 3), min_notes_coverage))
    else:
        status = PASS if notes_coverage else WARN
        if status == WARN and slide_count > 3:
            cautions.append("speaker notes are absent")
        checks.append(_check("speaker_notes", status, round(notes_coverage, 3), min_notes_coverage))

    visual_failures = 0
    visual_issues = 0
    if visual_report:
        visual_failures = int(visual_report.get("failure_count") or 0)
        visual_issues = int(visual_report.get("issue_count") or 0)
        if visual_failures:
            failures.append(f"visual QA failures: {visual_failures}")
            checks.append(_check("visual_qa", FAIL, visual_failures, 0))
        else:
            checks.append(_check("visual_qa", PASS, visual_failures, 0))

    chart_slides = sum(1 for slide in slides if int(slide.get("chart_relationships") or 0) > 0)
    picture_slides = sum(1 for slide in slides if int(slide.get("image_relationships") or 0) > 0)
    visual_anchor_ratio = _ratio(
        sum(1 for slide in content_slides if bool(slide.get("has_visual_anchor"))),
        len(content_slides),
    )

    score = 100
    score -= max(0, len(warnings) - max_warnings) * 8
    score -= len(missing_visual) * 14
    score -= len(full_slide_images) * 14
    score -= len(empty_placeholders) * 10
    score -= len(dense_slides) * 12
    score -= visual_failures * 12
    if notes_coverage < min_notes_coverage:
        score -= int((min_notes_coverage - notes_coverage) * 30)
    score = max(0, min(100, score))

    status = FAIL if failures else PASS
    return {
        "status": status,
        "score": score,
        "failures": failures,
        "cautions": cautions,
        "checks": checks,
        "metrics": {
            "slides": slide_count,
            "content_slides": len(content_slides),
            "warnings": len(warnings),
            "notes_coverage": round(notes_coverage, 3),
            "visual_anchor_ratio": round(visual_anchor_ratio, 3),
            "chart_slides": chart_slides,
            "picture_slides": picture_slides,
            "visual_issues": visual_issues,
            "visual_failures": visual_failures,
            "render_coverage": round(render_coverage, 3),
        },
        "render_evidence": {
            "required": require_render,
            "expectedSlides": slide_count,
            "renderedSlides": len(existing_render_paths),
            "files": render_hashes,
        },
        "audit": report,
        "visual_qa": visual_report,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Run a publishability quality gate for a PPTX deck.")
    parser.add_argument("--path", required=True, help="Path to a .pptx file")
    parser.add_argument("--pretty", action="store_true", help="Pretty-print JSON output")
    parser.add_argument("--strict", action="store_true", help="Require speaker notes and stricter text-density limits")
    parser.add_argument("--require-notes", action="store_true", help="Require at least 80% speaker-note coverage")
    parser.add_argument("--min-slides", type=int, default=1, help="Minimum expected slide count")
    parser.add_argument("--max-warnings", type=int, default=0, help="Maximum allowed audit warnings")
    parser.add_argument("--min-notes-coverage", type=float, default=None, help="Override required notes coverage, 0.0-1.0")
    parser.add_argument("--visual-qa", default=None, help="Optional JSON report from pptx_visual_qa.py")
    parser.add_argument("--render-dir", default=None, help="Directory containing final-PPTX slide images")
    parser.add_argument("--require-render", action="store_true", help="Fail unless final-PPTX render count equals display slide count")
    args = parser.parse_args()

    report = pptx_audit.audit(Path(args.path))
    visual_report = json.loads(Path(args.visual_qa).read_text(encoding="utf-8")) if args.visual_qa else None
    rendered_previews = []
    if args.render_dir:
        render_dir = Path(args.render_dir)
        rendered_previews = [
            str(path) for path in sorted(render_dir.iterdir())
            if path.is_file() and path.suffix.lower() in {".png", ".jpg", ".jpeg"}
        ]
    result = evaluate_audit(
        report,
        visual_report=visual_report,
        strict=args.strict,
        require_notes=args.require_notes,
        min_slides=args.min_slides,
        max_warnings=args.max_warnings,
        min_notes_coverage=args.min_notes_coverage,
        rendered_previews=rendered_previews,
        require_render=args.require_render,
    )
    print(json.dumps(result, ensure_ascii=False, indent=2 if args.pretty else None))
    return 0 if result["status"] == PASS else 4


if __name__ == "__main__":
    raise SystemExit(main())
