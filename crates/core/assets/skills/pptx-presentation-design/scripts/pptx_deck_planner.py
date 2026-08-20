#!/usr/bin/env python3
"""Turn source material into a renderer-ready PPTX JSON spec."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from typing import Any


URL_RE = re.compile(r"https?://[^\s)\]}>,]+")

INDUSTRY_PROFILES: dict[str, dict[str, Any]] = {
    "healthcare": {
        "tokens": ["healthcare", "patient", "clinical", "hospital", "care", "medical", "provider", "医疗", "患者", "临床", "医院"],
        "theme": "healthcare-trust",
        "background_presets": ["clinical_grid", "soft_geometry", "spotlight"],
        "icons": ["trust", "check", "workflow"],
        "tone": "calm trust, clinical clarity, safety-first decisions",
        "image_usage": "prefer authentic care-context images as soft backgrounds or side-by-side human context",
    },
    "finance": {
        "tokens": ["finance", "capital", "portfolio", "investment", "revenue", "margin", "risk", "market", "金融", "投资", "营收", "利润", "风险", "市场"],
        "theme": "finance-precision",
        "background_presets": ["data_grid", "blueprint_grid", "spotlight"],
        "icons": ["trend", "shield", "signal"],
        "tone": "precise, restrained, numbers-forward decision support",
        "image_usage": "use charts, data grids, and restrained market imagery instead of decorative photos",
    },
    "education": {
        "tokens": ["education", "student", "learning", "school", "curriculum", "training", "course", "教育", "学生", "学习", "学校", "培训", "课程"],
        "theme": "education-bright",
        "background_presets": ["paper_texture", "soft_geometry", "spotlight"],
        "icons": ["spark", "check", "workflow"],
        "tone": "clear, optimistic, accessible learning narrative",
        "image_usage": "use warm learning-context images sparingly and keep instructions readable",
    },
    "technology": {
        "tokens": ["ai", "agent", "software", "api", "developer", "technical", "architecture", "platform", "智能体", "软件", "开发者", "技术", "架构", "平台"],
        "theme": "nexa-dark",
        "background_presets": ["blueprint_grid", "gradient_mesh", "data_grid"],
        "icons": ["network", "signal", "spark"],
        "tone": "technical precision with diagram-first explanation",
        "image_usage": "prefer diagrams, architecture visuals, product screenshots, and annotated flows",
    },
    "industrial": {
        "tokens": ["manufacturing", "factory", "supply", "logistics", "industrial", "operations", "plant", "制造", "工厂", "供应链", "物流", "运营"],
        "theme": "industrial-contrast",
        "background_presets": ["blueprint_grid", "diagonal", "spotlight"],
        "icons": ["workflow", "check", "shield"],
        "tone": "operational, sturdy, safety-and-throughput oriented",
        "image_usage": "use process diagrams and operations photos with strong overlays",
    },
}


def _clean_line(line: str) -> str:
    return re.sub(r"^\s*(?:(?:[-*#•]+)|(?:\d+[.)]))\s+", "", line.strip()).strip()


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
    if any(token in joined for token in ["roadmap", "timeline", "q1", "q2", "q3", "q4", "路线图", "时间线", "里程碑", "第一季度", "第二季度", "第三季度", "第四季度"]):
        return "timeline"
    if any(token in joined for token in ["versus", " vs ", "tradeoff", "pros", "cons", "对比", "比较", "取舍", "优点", "缺点"]):
        return "comparison"
    # Text numbers are not typed chart data. Years, percentages, currencies,
    # and counts may use incompatible units, so deterministic planning keeps
    # them as source text until a caller supplies a chart spec explicitly.
    if index % 4 == 0:
        return "process"
    return "body"


def _infer_industry(points: list[str], audience: str) -> str:
    joined = " ".join(points + [audience]).lower()
    best = "general"
    best_score = 0
    for industry, profile in INDUSTRY_PROFILES.items():
        score = sum(1 for token in profile["tokens"] if token in joined)
        if score > best_score:
            best = industry
            best_score = score
    return best


def _visual_language(industry: str, theme: str | dict[str, Any]) -> dict[str, Any]:
    profile = INDUSTRY_PROFILES.get(industry)
    if profile:
        return {
            "industry": industry,
            "tone": profile["tone"],
            "background_presets": profile["background_presets"],
            "icon_set": profile["icons"],
            "image_usage": profile["image_usage"],
        }
    return {
        "industry": "general",
        "tone": "content-informed, polished, audience-specific narrative",
        "background_presets": ["soft_geometry", "diagonal", "gradient_mesh"],
        "icon_set": ["spark", "check", "trend"],
        "image_usage": "use supplied images by role; otherwise prefer editable visuals over decorative stock imagery",
        "theme": theme if isinstance(theme, str) else "custom",
    }


def _background_style_for(layout: str, index: int, industry: str = "general") -> str:
    profile = INDUSTRY_PROFILES.get(industry)
    if profile:
        presets = list(profile["background_presets"])
        if layout in {"title", "section"}:
            return presets[2] if len(presets) > 2 else presets[0]
        if layout in {"chart", "table"}:
            return presets[0]
        return presets[index % len(presets)]
    if layout in {"title", "section"}:
        return "diagonal"
    if layout == "chart":
        return "gradient_mesh"
    if layout in {"timeline", "process", "comparison", "matrix", "stat"}:
        return "soft_geometry"
    return ["soft_geometry", "diagonal", "gradient_mesh"][index % 3]


def _design_role_for(layout: str, index: int) -> str:
    if layout in {"title", "section", "quote"}:
        return "anchor"
    if layout in {"image_full", "timeline"} or index % 5 == 0:
        return "breathing"
    return "dense"


def _icon_for_slide(layout: str, index: int, industry: str) -> str | None:
    profile = INDUSTRY_PROFILES.get(industry)
    icons = profile["icons"] if profile else ["spark", "check", "trend"]
    if layout in {"title", "section"}:
        return icons[0]
    if layout in {"chart", "stat"}:
        return icons[1 if len(icons) > 1 else 0]
    if layout in {"process", "timeline"}:
        return icons[2 if len(icons) > 2 else 0]
    if index % 3 == 0:
        return icons[index % len(icons)]
    return None


def _with_visual_rhythm(slide: dict[str, Any], index: int, industry: str = "general") -> dict[str, Any]:
    layout = str(slide.get("layout") or "body")
    slide.setdefault("design_role", _design_role_for(str(slide.get("layout") or "body"), index))
    if not slide.get("background") and not slide.get("background_style"):
        slide["background_style"] = _background_style_for(layout, index, industry)
    icon = _icon_for_slide(layout, index, industry)
    if icon and not slide.get("icon"):
        slide["icon"] = icon
    return slide


def _infer_theme(points: list[str], audience: str, requested: str | dict[str, Any] | None) -> str | dict[str, Any]:
    if isinstance(requested, dict):
        return requested
    key = (requested or "auto").strip().lower()
    if key not in {"", "auto"}:
        return requested or "nexa-light"

    industry = _infer_industry(points, audience)
    if industry in INDUSTRY_PROFILES:
        return str(INDUSTRY_PROFILES[industry]["theme"])

    joined = " ".join(points + [audience]).lower()
    if any(token in joined for token in ["launch", "product", "growth", "customer", "market", "go-to-market", "roadmap"]):
        return "product-energy"
    if any(token in joined for token in ["board", "executive", "leadership", "strategy", "revenue", "margin", "investment"]):
        return "consulting-clean"
    if any(token in joined for token in ["ai", "agent", "software", "api", "developer", "technical", "architecture"]):
        return "nexa-dark"
    if any(token in joined for token in ["research", "report", "story", "brand", "culture", "education"]):
        return "editorial-ink"
    return "nexa-light"


def _style_objective(points: list[str], audience: str, theme: str | dict[str, Any]) -> str:
    joined = " ".join(points + [audience]).lower()
    if isinstance(theme, str) and theme in {"consulting-clean", "executive-midnight"}:
        return "decision-oriented executive narrative with clear takeaways and restrained visual rhythm"
    if "product" in joined or "launch" in joined:
        return "product story with energetic contrast, hero moments, and practical next steps"
    if "technical" in joined or "architecture" in joined or "agent" in joined:
        return "technical explanation with diagram-first pages and precise annotations"
    return "content-informed visual narrative with varied page rhythm and editable visuals"


def _design_brief(
    *,
    audience: str,
    theme: str | dict[str, Any],
    industry: str,
    visual_language: dict[str, Any],
    target_slides: int | None,
    points: list[str],
    slide_count: int,
) -> dict[str, Any]:
    style = _style_objective(points, audience, theme)
    theme_label = theme if isinstance(theme, str) else "custom"
    image_usage = (
        "Use user-provided image aliases when present; prefer hero/background roles for covers and side-by-side roles for content pages."
    )
    return {
        "audience": audience,
        "industry": industry,
        "style_objective": style,
        "theme": theme_label,
        "visual_language": visual_language,
        "decision_points": [
            {"name": "canvas", "recommendation": "16:9 widescreen editable PPTX"},
            {"name": "page_count", "recommendation": f"{target_slides or slide_count} slides planned"},
            {"name": "target_audience", "recommendation": audience},
            {"name": "style_objective", "recommendation": style},
            {"name": "color_scheme", "recommendation": f"use {theme_label} as the deck-wide palette"},
            {"name": "icon_usage", "recommendation": f"use {', '.join(visual_language.get('icon_set', [])) or 'one'} as the consistent icon language"},
            {"name": "typography", "recommendation": "large assertion titles, compact body text, muted captions"},
            {"name": "image_usage", "recommendation": visual_language.get("image_usage") or image_usage},
        ],
        "background_plan": {
            "anchor": "full-bleed image or diagonal high-contrast backdrop for cover, section, and close slides",
            "breathing": "dominant visual area with minimal copy and strong negative space",
            "dense": "editable motif behind charts, tables, processes, and comparison layouts",
        },
    }


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
    theme: str | dict[str, Any] = "auto",
    target_slides: int | None = None,
) -> dict[str, Any]:
    lines = _source_lines(text)
    deck_title = title or (lines[0] if lines else "Presentation")
    source_urls = _urls(text)
    points = [line for line in lines if line != deck_title and not URL_RE.fullmatch(line)]
    if not points:
        points = [deck_title]
    industry = _infer_industry(points, audience)
    resolved_theme = _infer_theme(points, audience, theme)
    visual_language = _visual_language(industry, resolved_theme)
    desired_content = max(2, (target_slides or min(8, max(4, len(points) // 3 + 2))) - 2)
    chunk_size = max(2, min(5, math_safe_ceil(len(points), desired_content)))
    slides: list[dict[str, Any]] = [
        _with_visual_rhythm(
            {
                "layout": "title",
                "title": deck_title,
                "subtitle": f"For {audience}",
                "notes": f"Open by framing why {deck_title} matters for {audience}.",
                "links": source_urls[:3],
            },
            0,
            industry,
        ),
        _with_visual_rhythm(
            {
                "layout": "agenda",
                "title": "Discussion Flow",
                "items": [_message_title(point, f"Topic {idx}") for idx, point in enumerate(points[:5], start=1)],
                "notes": "Use this slide to set expectations and the decision path.",
            },
            1,
            industry,
        ),
    ]
    for idx, chunk in enumerate(_chunks(points, chunk_size), start=1):
        title_text = _message_title(chunk[0], f"Point {idx}")
        notes = "Speaker focus: " + " ".join(chunk[:2])
        layout = _layout_for(chunk, idx)
        if layout == "chart":
            slide = _chart_slide(title_text, chunk, notes, source_urls)
        elif layout == "comparison":
            slide = _comparison_slide(title_text, chunk, notes, source_urls)
        elif layout == "timeline":
            slide = _timeline_slide(title_text, chunk, notes, source_urls)
        elif layout == "process":
            slide = _process_slide(title_text, chunk, notes, source_urls)
        else:
            slide = {"layout": "body", "title": title_text, "bullets": chunk[:6], "notes": notes, "links": source_urls}
        slides.append(_with_visual_rhythm(slide, len(slides), industry))
    slides.append(
        _with_visual_rhythm(
            {
                "layout": "section",
                "title": "Recommended Next Step",
                "subtitle": "Confirm the decision, owner, and timing.",
                "notes": "Close with a concrete ask and next action.",
            },
            len(slides),
            industry,
        )
    )
    if target_slides and len(slides) > target_slides:
        slides = slides[: max(1, target_slides - 1)] + [slides[-1]]
    return {
        "theme": resolved_theme,
        "metadata": {
            "source": "pptx_deck_planner",
            "audience": audience,
            "industry": industry,
            "source_links": source_urls,
            "chart_inference": "disabled_without_typed_data",
            "visual_strategy": f"message-title rhythm with {visual_language['tone']} and editable native motifs",
            "design_brief": _design_brief(
                audience=audience,
                theme=resolved_theme,
                industry=industry,
                visual_language=visual_language,
                target_slides=target_slides,
                points=points,
                slide_count=len(slides),
            ),
        },
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
    parser.add_argument("--theme", default="auto", help="Renderer theme preset, or auto")
    args = parser.parse_args()
    source = Path(args.input).read_text(encoding="utf-8")
    spec = plan_deck(source, title=args.title, audience=args.audience, theme=args.theme, target_slides=args.target_slides)
    Path(args.out).write_text(json.dumps(spec, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"created deck spec: {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
