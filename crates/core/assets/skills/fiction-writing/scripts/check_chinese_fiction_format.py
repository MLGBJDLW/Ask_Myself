#!/usr/bin/env python3
"""Check Chinese fiction draft length and mobile paragraph density."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

CJK_RE = re.compile(r"[\u3400-\u4dbf\u4e00-\u9fff]")
MARKDOWN_META_RE = re.compile(r"^\s*(#|>|[-*+]\s|\d+\.\s)")


def cjk_count(text: str) -> int:
    return len(CJK_RE.findall(text))


def strip_markdown_meta(text: str) -> str:
    lines: list[str] = []
    in_frontmatter = False
    for raw_line in text.splitlines():
        line = raw_line.rstrip()
        if line.strip() == "---":
            in_frontmatter = not in_frontmatter
            continue
        if in_frontmatter or MARKDOWN_META_RE.match(line):
            continue
        lines.append(line)
    return "\n".join(lines)


def paragraphs(text: str) -> list[str]:
    body = strip_markdown_meta(text)
    return [part.strip() for part in re.split(r"\n\s*\n+", body) if part.strip()]


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Check Chinese fiction character count and paragraph length."
    )
    parser.add_argument("path", type=Path)
    parser.add_argument("--min", dest="min_chars", type=int, default=None)
    parser.add_argument("--max", dest="max_chars", type=int, default=None)
    parser.add_argument("--max-paragraph", type=int, default=120)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()

    text = args.path.read_text(encoding="utf-8")
    paras = paragraphs(text)
    total = cjk_count("\n".join(paras))
    long_paragraphs = [
        {"index": idx + 1, "chars": cjk_count(para)}
        for idx, para in enumerate(paras)
        if cjk_count(para) > args.max_paragraph
    ]

    issues: list[str] = []
    if args.min_chars is not None and total < args.min_chars:
        issues.append(f"below_min_chars:{total}<{args.min_chars}")
    if args.max_chars is not None and total > args.max_chars:
        issues.append(f"above_max_chars:{total}>{args.max_chars}")
    if long_paragraphs:
        issues.append(f"long_paragraphs:{len(long_paragraphs)}")

    report = {
        "path": str(args.path),
        "chars": total,
        "paragraphs": len(paras),
        "maxParagraph": args.max_paragraph,
        "longParagraphs": long_paragraphs,
        "passed": not issues,
        "issues": issues,
    }

    if args.json:
        print(json.dumps(report, ensure_ascii=False, indent=2))
    else:
        status = "PASS" if report["passed"] else "FAIL"
        print(f"{status}: {total} Chinese chars, {len(paras)} paragraphs")
        for item in long_paragraphs[:20]:
            print(f"paragraph {item['index']}: {item['chars']} chars")
        if len(long_paragraphs) > 20:
            print(f"... {len(long_paragraphs) - 20} more long paragraphs")

    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
