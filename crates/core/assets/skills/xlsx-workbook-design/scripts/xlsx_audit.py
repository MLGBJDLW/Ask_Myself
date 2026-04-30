#!/usr/bin/env python3
"""Audit an XLSX package and print a compact JSON structural summary."""

from __future__ import annotations

import argparse
import json
import re
import sys
import zipfile
from pathlib import Path
from xml.etree import ElementTree as ET


NS = {
    "main": "http://schemas.openxmlformats.org/spreadsheetml/2006/main",
    "rel": "http://schemas.openxmlformats.org/package/2006/relationships",
    "r": "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
}

FORMULA_ERROR_VALUES = {"#REF!", "#VALUE!", "#DIV/0!", "#NAME?", "#N/A", "#NULL!", "#NUM!"}


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


def natural_key(name: str) -> tuple:
    return tuple(int(part) if part.isdigit() else part for part in re.split(r"(\d+)", name))


def workbook_sheets(zf: zipfile.ZipFile) -> list[dict[str, str]]:
    root = parse_xml(read_text(zf, "xl/workbook.xml"))
    if root is None:
        return []
    sheets = []
    for sheet in root.findall(".//main:sheet", NS):
        sheets.append(
            {
                "name": sheet.attrib.get("name", ""),
                "state": sheet.attrib.get("state", "visible"),
                "sheet_id": sheet.attrib.get("sheetId", ""),
                "relationship_id": sheet.attrib.get(f"{{{NS['r']}}}id", ""),
            }
        )
    return sheets


def worksheet_rels(zf: zipfile.ZipFile, sheet_part: str) -> list[dict[str, str]]:
    parent, filename = sheet_part.rsplit("/", 1)
    rels_name = f"{parent}/_rels/{filename}.rels"
    root = parse_xml(read_text(zf, rels_name))
    if root is None:
        return []
    out = []
    for rel in root.findall("rel:Relationship", NS):
        target = rel.attrib.get("Target", "")
        mode = rel.attrib.get("TargetMode", "")
        if mode != "External" and not target.startswith("/"):
            target = f"{parent}/{target}"
        out.append(
            {
                "type": rel.attrib.get("Type", ""),
                "target": target,
                "mode": mode,
            }
        )
    return out


def worksheet_summary(zf: zipfile.ZipFile, sheet_part: str, name: str, state: str) -> dict:
    root = parse_xml(read_text(zf, sheet_part))
    rels = worksheet_rels(zf, sheet_part)
    if root is None:
        return {"name": name, "part": sheet_part, "state": state, "parse_error": True}

    dimension = root.find("main:dimension", NS)
    formulas = root.findall(".//main:f", NS)
    rows = root.findall(".//main:row", NS)
    cells = root.findall(".//main:c", NS)
    formula_errors = []
    for cell in cells:
        if cell.attrib.get("t") != "e":
            continue
        value = cell.find("main:v", NS)
        if value is not None and (value.text or "") in FORMULA_ERROR_VALUES:
            formula_errors.append({"cell": cell.attrib.get("r", ""), "value": value.text})

    panes = root.findall(".//main:pane", NS)
    tables = sum(1 for rel in rels if "/tables/" in rel["target"])
    drawings = sum(1 for rel in rels if "/drawings/" in rel["target"])
    external_rels = sum(1 for rel in rels if rel["mode"] == "External")
    return {
        "name": name,
        "part": sheet_part,
        "state": state,
        "dimension": dimension.attrib.get("ref", "") if dimension is not None else "",
        "rows": len(rows),
        "cells": len(cells),
        "formulas": len(formulas),
        "formula_errors": formula_errors,
        "tables": tables,
        "drawings": drawings,
        "has_autofilter": root.find(".//main:autoFilter", NS) is not None,
        "has_frozen_pane": any(pane.attrib.get("state") == "frozen" for pane in panes),
        "external_relationships": external_rels,
    }


def audit(path: Path) -> dict:
    warnings: list[str] = []
    with zipfile.ZipFile(path) as zf:
        names = set(zf.namelist())
        sheet_parts = sorted(
            [name for name in names if re.match(r"xl/worksheets/sheet\d+\.xml$", name)],
            key=natural_key,
        )
        sheets = workbook_sheets(zf)
        sheet_summaries = []
        for index, sheet_part in enumerate(sheet_parts):
            sheet_meta = sheets[index] if index < len(sheets) else {}
            summary = worksheet_summary(
                zf,
                sheet_part,
                sheet_meta.get("name", f"Sheet{index + 1}"),
                sheet_meta.get("state", "visible"),
            )
            if summary.get("formula_errors"):
                warnings.append(f"{summary['name']} has formula errors")
            if summary.get("rows", 0) > 20 and not summary.get("has_autofilter"):
                warnings.append(f"{summary['name']} has many rows without autofilter")
            if summary.get("external_relationships"):
                warnings.append(f"{summary['name']} has external relationships")
            sheet_summaries.append(summary)

        shared_strings = parse_xml(read_text(zf, "xl/sharedStrings.xml"))
        calc_chain_present = "xl/calcChain.xml" in names
        workbook = parse_xml(read_text(zf, "xl/workbook.xml"))
        calc_mode = ""
        if workbook is not None:
            calc_pr = workbook.find("main:calcPr", NS)
            if calc_pr is not None:
                calc_mode = calc_pr.attrib.get("calcMode", "")

        return {
            "path": str(path),
            "format": "xlsx",
            "package_parts": len(names),
            "sheets": len(sheet_summaries),
            "sheet_details": sheet_summaries,
            "shared_strings": int(shared_strings.attrib.get("count", "0"))
            if shared_strings is not None
            else 0,
            "calc_chain_present": calc_chain_present,
            "calc_mode": calc_mode,
            "warnings": warnings,
        }


def main() -> int:
    parser = argparse.ArgumentParser(description="Audit XLSX OOXML structure.")
    parser.add_argument("--path", required=True, help="Path to a .xlsx file")
    parser.add_argument("--pretty", action="store_true", help="Pretty-print JSON")
    args = parser.parse_args()

    path = Path(args.path).expanduser().resolve()
    if not path.exists():
        print(f"File not found: {path}", file=sys.stderr)
        return 3
    if path.suffix.lower() != ".xlsx":
        print(f"Expected .xlsx file: {path}", file=sys.stderr)
        return 3
    if not zipfile.is_zipfile(path):
        print(f"Not a valid OOXML zip package: {path}", file=sys.stderr)
        return 3

    result = audit(path)
    print(json.dumps(result, ensure_ascii=False, indent=2 if args.pretty else None))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
