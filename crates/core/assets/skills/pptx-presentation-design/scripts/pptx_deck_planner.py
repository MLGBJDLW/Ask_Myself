#!/usr/bin/env python3
"""Turn source material into a renderer-ready PPTX JSON spec."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from typing import Any


URL_RE = re.compile(r"https?://[^\s)\]}>,]+")


def _clean_line(line: str) -> str:
    return re.sub(r"^[-*#\d.\s]+", "", line.strip()).strip()


def _source_lines(text: str) -> list[str]:
    lines = [_clean_line(line) for line in text.splitlines()]
    return [line for line in lines if line]


def _urls(text: str) -> list[str]:
    seen: set[str] = set()
    result: list[str] = []
    for match in URL_RE.findall(text):
        if match not in seen:
            seen.add(match)
            result.append(match)
    return result


def _chunks(items: list[str], size: int) -> list[list[str]]:
    return [items[i : i + size] for i in range(0, len(items), size)] or [[]]


def _message_title(point: str, fallback: str) -> str:
    point = re.sub(URL_RE, "", point).strip()
    if len(point) <= 78:
        return point or fallback
    parts = re.split(r"[.;:]", point, maxsplit=1)
    return (parts[0] or point[:75]).strip()[:78]


def _layout_for(points: list[str], index: int) -> str:
    joined = " ".join(points).lower()
    if any(token in joined for token in ["roadmap", "timeline", "q1", "q2", "q3", "q4"]):
        return "timeline"
    if any(token in joined for token in ["versus", " vs ", "tradeoff", "pros", "cons"]):
        return "comparison"
    if sum(1 for point in points if re.search(r"\d+%|\$?\d+[,.]?\d*", point)) >= 2:
        return "chart"
    if index % 4 == 0:
        return "process"
    return "body"


def _chart_slide(title: str, points: list[str], notes: str, links: list[str]) -> dict[str, Any]:
    categories: list[str] = []
    values: list[float] = []
    for point in points:
        number = re.search(r"(-?\d+(?:\.\d+)?)\s*%?", point)
        if not number:
            continue
        categories.append(_message_title(point, f"Metric {len(categories) + 1}")[:22])
        values.append(float(number.group(1)))
    if len(categories) < 2:
        return {"layout": "body", "title": title, "bullets": points, "notes": notes, "links": links}
    return {
        "layout": "chart",
        "title": title,
        "categories": categories[:8],
        "series": [{"name": "Value", "values": values[:8]}],
        "chart_type": "column",
        "data_labels": True,
        "notes": notes,
        "links": links,
    }


def _comparison_slide(title: str, points: list[str], notes: str, links: list[str]) -> dict[str, Any]:
    midpoint = max(1, len(points) // 2)
    return {
        "layout": "comparison",
        "title": title,
        "left": {"heading": "Current", "bullets": points[:midpoint]},
        "right": {"heading": "Target", "bullets": points[midpoint:] or points[:midpoint]},
        "notes": notes,
        "links": links,
    }


def _timeline_slide(title: str, points: list[str], notes: str, links: list[str]) -> dict[str, Any]:
    events = [{"date": f"Step {idx}", "title": _message_title(point, f"Step {idx}")} for idx, point in enumerate(points[:5], start=1)]
    return {"layout": "timeline", "title": title, "events": events, "notes": notes, "links": links}


def _process_slide(title: str, points: list[str], notes: str, links: list[str]) -> dict[str, Any]:
    steps = [{"title": _message_title(point, f"Step {idx}"), "detail": point} for idx, point in enumerate(points[:5], start=1)]
    return {"layout": "process", "title": title, "steps": steps, "notes": notes, "links": links}


def plan_deck(
    text: str,
    *,
    title: str | None = None,
    audience: str = "general",
    theme: str | dict[str, Any] = "nexa-light",
    target_slides: int | None = None,
) -> dict[str, Any]:
    lines = _source_lines(text)
    deck_title = title or (lines[0] if lines else "Presentation")
    source_urls = _urls(text)
    points = [line for line in lines if line != deck_title and not URL_RE.fullmatch(line)]
    if not points:
        points = [deck_title]
    desired_content = max(2, (target_slides or min(8, max(4, len(points) // 3 + 2))) - 2)
    chunk_size = max(2, min(5, math_safe_ceil(len(points), desired_content)))
    slides: list[dict[str, Any]] = [
        {
            "layout": "title",
            "title": deck_title,
            "subtitle": f"For {audience}",
            "notes": f"Open by framing why {deck_title} matters for {audience}.",
            "links": source_urls[:3],
        },
        {
            "layout": "agenda",
            "title": "Discussion Flow",
            "items": [_message_title(point, f"Topic {idx}") for idx, point in enumerate(points[:5], start=1)],
            "notes": "Use this slide to set expectations and the decision path.",
        },
    ]
    for idx, chunk in enumerate(_chunks(points, chunk_size), start=1):
        title_text = _message_title(chunk[0], f"Point {idx}")
        notes = "Speaker focus: " + " ".join(chunk[:2])
        layout = _layout_for(chunk, idx)
        if layout == "chart":
            slides.append(_chart_slide(title_text, chunk, notes, source_urls))
        elif layout == "comparison":
            slides.append(_comparison_slide(title_text, chunk, notes, source_urls))
        elif layout == "timeline":
            slides.append(_timeline_slide(title_text, chunk, notes, source_urls))
        elif layout == "process":
            slides.append(_process_slide(title_text, chunk, notes, source_urls))
        else:
            slides.append({"layout": "body", "title": title_text, "bullets": chunk[:6], "notes": notes, "links": source_urls})
    slides.append(
        {
            "layout": "section",
            "title": "Recommended Next Step",
            "subtitle": "Confirm the decision, owner, and timing.",
            "notes": "Close with a concrete ask and next action.",
        }
    )
    if target_slides and len(slides) > target_slides:
        slides = slides[: max(1, target_slides - 1)] + [slides[-1]]
    return {
        "theme": theme,
        "metadata": {"source": "pptx_deck_planner", "audience": audience, "source_links": source_urls},
        "slides": slides,
    }


def math_safe_ceil(total: int, groups: int) -> int:
    if groups <= 0:
        return max(1, total)
    return max(1, (total + groups - 1) // groups)


def main() -> int:
    parser = argparse.ArgumentParser(description="Create a PPTX renderer JSON spec from source text.")
    parser.add_argument("--input", required=True, help="Path to source text or JSON")
    parser.add_argument("--out", required=True, help="Output JSON spec path")
    parser.add_argument("--title", default=None, help="Override deck title")
    parser.add_argument("--audience", default="general", help="Target audience")
    parser.add_argument("--target-slides", type=int, default=None, help="Approximate slide count")
    parser.add_argument("--theme", default="nexa-light", help="Renderer theme preset")
    args = parser.parse_args()
    source = Path(args.input).read_text(encoding="utf-8")
    spec = plan_deck(source, title=args.title, audience=args.audience, theme=args.theme, target_slides=args.target_slides)
    Path(args.out).write_text(json.dumps(spec, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"created deck spec: {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
