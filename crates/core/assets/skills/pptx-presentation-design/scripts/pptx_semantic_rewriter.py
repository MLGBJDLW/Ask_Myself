#!/usr/bin/env python3
"""Create a semantic rewrite spec from existing PPTX content or notes."""

from __future__ import annotations

import argparse
import json
import re
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any

import pptx_audit


ROLE_ORDER = ["context", "problem", "evidence", "options", "recommendation", "plan", "risk", "appendix"]

ROLE_KEYWORDS = {
    "context": {"overview", "background", "market", "customer", "current", "today", "context"},
    "problem": {"problem", "challenge", "gap", "issue", "pain", "risk", "constraint", "bottleneck"},
    "evidence": {"data", "growth", "revenue", "metric", "kpi", "increase", "decrease", "%", "$", "score"},
    "options": {"option", "alternative", "versus", "vs", "tradeoff", "scenario", "choice"},
    "recommendation": {"recommend", "proposal", "should", "decision", "ask", "priority", "recommendation"},
    "plan": {"plan", "roadmap", "timeline", "milestone", "q1", "q2", "q3", "q4", "next"},
    "risk": {"risk", "dependency", "mitigation", "blocker", "concern", "assumption"},
}


URL_RE = re.compile(r"https?://[^\s)\]}>,]+")
SENTENCE_RE = re.compile(r"(?<=[.!?。！？])\s+|\n+")
NUMBER_RE = re.compile(r"(?<![\w.])-?\$?\d+(?:[,.]\d{3})*(?:\.\d+)?%?")


def _tokens(text: str) -> list[str]:
    return re.findall(r"[\w%$]+", text.lower())


def classify_text(text: str) -> str:
    tokens = _tokens(text)
    joined = " ".join(tokens)
    scores: Counter[str] = Counter()
    for role, keywords in ROLE_KEYWORDS.items():
        for keyword in keywords:
            if keyword in joined:
                scores[role] += 2 if keyword in {"%", "$"} else 1
    if NUMBER_RE.search(text):
        scores["evidence"] += 2
    if not scores:
        return "appendix"
    return scores.most_common(1)[0][0]


def _sentences(text: str) -> list[str]:
    parts = [part.strip(" -\t") for part in SENTENCE_RE.split(text) if part.strip(" -\t")]
    return [part for part in parts if len(part) > 2]


def _dedupe(items: list[str]) -> list[str]:
    seen: set[str] = set()
    out: list[str] = []
    for item in items:
        key = re.sub(r"\W+", " ", item.lower()).strip()
        if key and key not in seen:
            seen.add(key)
            out.append(item)
    return out


def _short_title(text: str, fallback: str) -> str:
    text = URL_RE.sub("", text).strip()
    if not text:
        return fallback
    first = re.split(r"[.;:。！？]", text, maxsplit=1)[0].strip()
    return (first or text)[:82]


def _numeric_pairs(sentences: list[str]) -> tuple[list[str], list[float]]:
    categories: list[str] = []
    values: list[float] = []
    for sentence in sentences:
        matches = list(NUMBER_RE.finditer(sentence))
        for match_index, match in enumerate(matches, start=1):
            raw = match.group(0).replace("$", "").replace(",", "").replace("%", "")
            try:
                value = float(raw)
            except ValueError:
                continue
            label = _short_title(sentence, f"Metric {len(categories) + 1}")[:20]
            if len(matches) > 1:
                label = f"{label} {match_index}"
            categories.append(label[:24])
            values.append(value)
    return categories, values


def _source_links(text: str) -> list[str]:
    return list(dict.fromkeys(URL_RE.findall(text)))


def _slide_texts_from_report(report: dict[str, Any]) -> list[dict[str, Any]]:
    slides: list[dict[str, Any]] = []
    for slide in report.get("slide_details") or []:
        text = str(slide.get("text") or "").strip()
        if not text:
            continue
        slides.append({"index": slide.get("index"), "text": text, "role": classify_text(text)})
    return slides


def _slide_texts_from_plaintext(text: str) -> list[dict[str, Any]]:
    chunks = [chunk.strip() for chunk in re.split(r"\n\s*\n+", text) if chunk.strip()]
    if not chunks:
        chunks = [text.strip()]
    return [{"index": idx, "text": chunk, "role": classify_text(chunk)} for idx, chunk in enumerate(chunks, start=1)]


def _group_slides(slides: list[dict[str, Any]]) -> dict[str, list[str]]:
    grouped: dict[str, list[str]] = defaultdict(list)
    for slide in slides:
        grouped[str(slide["role"])].extend(_sentences(str(slide["text"])))
    return {role: _dedupe(items) for role, items in grouped.items()}


def _role_slide(role: str, points: list[str], links: list[str]) -> dict[str, Any]:
    title = {
        "context": "Context that matters",
        "problem": "The core problem to solve",
        "evidence": "Evidence behind the decision",
        "options": "Options and tradeoffs",
        "recommendation": "Recommended direction",
        "plan": "Execution plan",
        "risk": "Risks and mitigations",
        "appendix": "Supporting detail",
    }.get(role, "Key point")
    notes = "Rewrite basis: " + " ".join(points[:2])
    if role == "evidence":
        categories, values = _numeric_pairs(points)
        if len(categories) >= 2:
            return {
                "layout": "chart",
                "title": title,
                "categories": categories[:8],
                "series": [{"name": "Value", "values": values[:8]}],
                "chart_type": "column",
            "data_labels": True,
            "notes": notes,
            "links": links,
            "semantic_role": role,
        }
    if role == "options" and points:
        option_points = list(points)
        if len(option_points) == 1:
            split = [part.strip(" .") for part in re.split(r"\bversus\b|\bvs\b|\bbut\b|;", option_points[0], flags=re.IGNORECASE) if part.strip(" .")]
            if len(split) >= 2:
                option_points = split
        midpoint = max(1, len(option_points) // 2)
        return {
            "layout": "comparison",
            "title": title,
            "left": {"heading": "Option A", "bullets": option_points[:midpoint][:4]},
            "right": {"heading": "Option B", "bullets": option_points[midpoint:][:4] or option_points[:midpoint][:4]},
            "notes": notes,
            "links": links,
            "semantic_role": role,
        }
    if role == "plan":
        return {
            "layout": "timeline",
            "title": title,
            "events": [{"date": f"Step {idx}", "title": _short_title(point, f"Step {idx}")} for idx, point in enumerate(points[:5], start=1)],
            "notes": notes,
            "links": links,
            "semantic_role": role,
        }
    if role == "recommendation":
        return {
            "layout": "section",
            "title": title,
            "subtitle": _short_title(points[0], "Confirm the recommendation.") if points else "Confirm the recommendation.",
            "notes": notes,
            "links": links,
            "semantic_role": role,
        }
    return {"layout": "body", "title": title, "bullets": points[:5], "notes": notes, "links": links, "semantic_role": role}


def _prune_rewrite_slides(slides: list[dict[str, Any]], target_slides: int) -> list[dict[str, Any]]:
    if len(slides) <= target_slides:
        return slides
    if target_slides <= 2:
        return slides[:target_slides]
    priority = {
        "evidence": 0,
        "options": 1,
        "recommendation": 2,
        "plan": 3,
        "problem": 4,
        "risk": 5,
        "context": 6,
        "appendix": 7,
    }
    head = slides[:2] if len(slides) > 2 else slides[:1]
    tail = [slides[-1]]
    middle = list(enumerate(slides[len(head) : -1], start=len(head)))
    slots = max(0, target_slides - len(head) - len(tail))
    selected_indices = {
        index
        for index, _slide in sorted(
            middle,
            key=lambda item: (priority.get(str(item[1].get("semantic_role")), 9), item[0]),
        )[:slots]
    }
    selected = [slide for index, slide in middle if index in selected_indices]
    return head + selected + tail


def semantic_rewrite_from_report(
    report: dict[str, Any],
    *,
    target_slides: int | None = None,
    audience: str = "executive",
    theme: str | dict[str, Any] = "nexa-light",
) -> dict[str, Any]:
    slides = _slide_texts_from_report(report)
    if not slides:
        slides = [{"index": 1, "text": "Presentation rewrite", "role": "context"}]
    all_text = "\n".join(slide["text"] for slide in slides)
    links = _source_links(all_text)
    grouped = _group_slides(slides)
    deck_title = _short_title(slides[0]["text"], "Semantic Rewrite")

    rewritten: list[dict[str, Any]] = [
        {
            "layout": "title",
            "title": deck_title,
            "subtitle": f"Condensed for {audience}",
            "notes": "Open with the synthesized decision path, not the original slide order.",
            "links": links[:4],
            "semantic_role": "title",
        }
    ]
    agenda_items = [role.title() for role in ROLE_ORDER if role in grouped and grouped[role]]
    if agenda_items:
        rewritten.append({"layout": "agenda", "title": "Narrative Path", "items": agenda_items[:6], "notes": "Preview the rewritten storyline.", "semantic_role": "agenda"})
    for role in ROLE_ORDER:
        points = grouped.get(role) or []
        if not points:
            continue
        rewritten.append(_role_slide(role, points, links[:4]))
    rewritten.append(
        {
            "layout": "section",
            "title": "Decision And Next Step",
            "subtitle": "Confirm owner, timing, and success metric.",
            "notes": "Close with a concrete decision and immediate next action.",
            "links": links[:4],
            "semantic_role": "close",
        }
    )
    if target_slides and len(rewritten) > target_slides:
        rewritten = _prune_rewrite_slides(rewritten, target_slides)
    return {
        "theme": theme,
        "metadata": {
            "source": "pptx_semantic_rewriter",
            "audience": audience,
            "source_slide_count": report.get("slides"),
            "source_links": links,
        },
        "slides": rewritten,
    }


def semantic_rewrite_from_text(
    text: str,
    *,
    target_slides: int | None = None,
    audience: str = "executive",
    theme: str | dict[str, Any] = "nexa-light",
) -> dict[str, Any]:
    report = {
        "slides": 0,
        "slide_details": _slide_texts_from_plaintext(text),
    }
    return semantic_rewrite_from_report(report, target_slides=target_slides, audience=audience, theme=theme)


def main() -> int:
    parser = argparse.ArgumentParser(description="Create a semantically rewritten PPTX renderer spec.")
    parser.add_argument("--path", default=None, help="Path to an existing .pptx deck")
    parser.add_argument("--input", default=None, help="Plain text or markdown source file")
    parser.add_argument("--out", required=True, help="Output renderer JSON spec")
    parser.add_argument("--target-slides", type=int, default=None, help="Target slide count")
    parser.add_argument("--audience", default="executive", help="Target audience")
    parser.add_argument("--theme", default="nexa-light", help="Renderer theme")
    parser.add_argument("--pretty", action="store_true", help="Pretty-print JSON output to stdout")
    args = parser.parse_args()

    if args.path:
        spec = semantic_rewrite_from_report(
            pptx_audit.audit(Path(args.path)),
            target_slides=args.target_slides,
            audience=args.audience,
            theme=args.theme,
        )
    elif args.input:
        spec = semantic_rewrite_from_text(
            Path(args.input).read_text(encoding="utf-8"),
            target_slides=args.target_slides,
            audience=args.audience,
            theme=args.theme,
        )
    else:
        parser.error("provide --path or --input")
    Path(args.out).write_text(json.dumps(spec, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    if args.pretty:
        print(json.dumps(spec, ensure_ascii=False, indent=2))
    else:
        print(f"created semantic rewrite spec: {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
