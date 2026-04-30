#!/usr/bin/env python3
"""Audit a DOCX package and print a compact JSON structural summary."""

from __future__ import annotations

import argparse
import json
import re
import sys
import zipfile
from pathlib import Path
from xml.etree import ElementTree as ET


NS = {
    "w": "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
    "r": "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
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


def count_tag(text: str, tag: str) -> int:
    return len(re.findall(rf"<(?:\w+:)?{re.escape(tag)}(?:\s|>|/)", text))


def count_style_types(styles_xml: str) -> dict[str, int]:
    root = parse_xml(styles_xml)
    counts = {"paragraph": 0, "character": 0, "table": 0, "numbering": 0}
    if root is None:
        return counts
    for style in root.findall("w:style", NS):
        style_type = style.attrib.get(f"{{{NS['w']}}}type")
        if style_type in counts:
            counts[style_type] += 1
    return counts


def count_relationships(zf: zipfile.ZipFile) -> dict[str, int]:
    external = 0
    total = 0
    for name in zf.namelist():
        if not name.endswith(".rels"):
            continue
        rels = parse_xml(read_text(zf, name))
        if rels is None:
            continue
        for rel in rels.findall("rel:Relationship", NS):
            total += 1
            if rel.attrib.get("TargetMode") == "External":
                external += 1
    return {"total": total, "external": external}


def audit(path: Path) -> dict:
    warnings: list[str] = []
    with zipfile.ZipFile(path) as zf:
        names = set(zf.namelist())
        document_xml = read_text(zf, "word/document.xml")
        styles_xml = read_text(zf, "word/styles.xml")
        comments_xml = read_text(zf, "word/comments.xml")

        if not document_xml:
            warnings.append("missing word/document.xml")
        if "word/styles.xml" not in names:
            warnings.append("missing word/styles.xml")
        if count_tag(document_xml, "altChunk"):
            warnings.append("contains altChunk embedded content")

        tracked_changes = {
            "insertions": count_tag(document_xml, "ins"),
            "deletions": count_tag(document_xml, "del"),
            "move_from": count_tag(document_xml, "moveFrom"),
            "move_to": count_tag(document_xml, "moveTo"),
        }
        if any(tracked_changes.values()):
            warnings.append("tracked changes present")

        relationships = count_relationships(zf)
        if relationships["external"]:
            warnings.append("external relationships present")

        media = [name for name in names if name.startswith("word/media/")]
        headers = [name for name in names if re.match(r"word/header\d+\.xml$", name)]
        footers = [name for name in names if re.match(r"word/footer\d+\.xml$", name)]

        return {
            "path": str(path),
            "format": "docx",
            "package_parts": len(names),
            "paragraphs": count_tag(document_xml, "p"),
            "tables": count_tag(document_xml, "tbl"),
            "sections": count_tag(document_xml, "sectPr"),
            "images": len(media),
            "headers": len(headers),
            "footers": len(footers),
            "comments": count_tag(comments_xml, "comment"),
            "tracked_changes": tracked_changes,
            "styles": count_style_types(styles_xml),
            "relationships": relationships,
            "warnings": warnings,
        }


def main() -> int:
    parser = argparse.ArgumentParser(description="Audit DOCX OOXML structure.")
    parser.add_argument("--path", required=True, help="Path to a .docx file")
    parser.add_argument("--pretty", action="store_true", help="Pretty-print JSON")
    args = parser.parse_args()

    path = Path(args.path).expanduser().resolve()
    if not path.exists():
        print(f"File not found: {path}", file=sys.stderr)
        return 3
    if path.suffix.lower() != ".docx":
        print(f"Expected .docx file: {path}", file=sys.stderr)
        return 3
    if not zipfile.is_zipfile(path):
        print(f"Not a valid OOXML zip package: {path}", file=sys.stderr)
        return 3

    result = audit(path)
    print(json.dumps(result, ensure_ascii=False, indent=2 if args.pretty else None))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
