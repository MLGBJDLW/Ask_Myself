#!/usr/bin/env python3
"""Audit an XLSX package and print a compact JSON structural summary."""

from __future__ import annotations

import argparse
import json
import posixpath
import re
import sys
import zipfile
from pathlib import Path
from xml.etree import ElementTree as ET


NS = {
    "main": "http://schemas.openxmlformats.org/spreadsheetml/2006/main",
    "rel": "http://schemas.openxmlformats.org/package/2006/relationships",
    "r": "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    "c": "http://schemas.openxmlformats.org/drawingml/2006/chart",
}

FORMULA_ERROR_VALUES = {
    "#REF!", "#VALUE!", "#DIV/0!", "#NAME?", "#N/A", "#NULL!", "#NUM!",
    "#SPILL!", "#CALC!", "#FIELD!", "#BLOCKED!", "#UNKNOWN!", "#CONNECT!", "#BUSY!",
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


def natural_key(name: str) -> tuple:
    return tuple(int(part) if part.isdigit() else part for part in re.split(r"(\d+)", name))


def workbook_sheets(zf: zipfile.ZipFile) -> list[dict[str, str]]:
    root = parse_xml(read_text(zf, "xl/workbook.xml"))
    if root is None:
        return []
    rels = parse_xml(read_text(zf, "xl/_rels/workbook.xml.rels"))
    rel_map: dict[str, str] = {}
    if rels is not None:
        for rel in rels.findall("rel:Relationship", NS):
            relationship_id = rel.attrib.get("Id", "")
            target = rel.attrib.get("Target", "")
            if relationship_id and target and rel.attrib.get("TargetMode") != "External":
                rel_map[relationship_id] = posixpath.normpath(
                    target.lstrip("/") if target.startswith("/") else f"xl/{target}"
                )
    sheets = []
    for sheet in root.findall(".//main:sheet", NS):
        relationship_id = sheet.attrib.get(f"{{{NS['r']}}}id", "")
        sheets.append(
            {
                "name": sheet.attrib.get("name", ""),
                "state": sheet.attrib.get("state", "visible"),
                "sheet_id": sheet.attrib.get("sheetId", ""),
                "relationship_id": relationship_id,
                "part": rel_map.get(relationship_id, ""),
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
        if value is not None and (value.text or ""):
            error_value = value.text or ""
            formula_errors.append({
                "cell": cell.attrib.get("r", ""),
                "value": error_value,
                "known": error_value in FORMULA_ERROR_VALUES,
            })

    panes = root.findall(".//main:pane", NS)
    tables = sum(1 for rel in rels if "/tables/" in rel["target"])
    drawings = sum(1 for rel in rels if "/drawings/" in rel["target"])
    external_rels = sum(1 for rel in rels if rel["mode"] == "External")
    return {
        "name": name,
        "part": sheet_part,
        "type": "worksheet",
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


def chartsheet_summary(zf: zipfile.ZipFile, sheet_part: str, name: str, state: str) -> dict:
    root = parse_xml(read_text(zf, sheet_part))
    rels = worksheet_rels(zf, sheet_part)
    return {
        "name": name,
        "part": sheet_part,
        "type": "chartsheet",
        "state": state,
        "parse_error": root is None,
        "drawings": sum(1 for rel in rels if "/drawing" in rel["type"]),
        "external_relationships": sum(1 for rel in rels if rel["mode"] == "External"),
    }


def _range_cells(value: str) -> list[str]:
    def column_index(letters: str) -> int:
        index = 0
        for letter in letters:
            index = index * 26 + ord(letter) - 64
        return index

    def column_letters(index: int) -> str:
        letters = ""
        while index:
            index, remainder = divmod(index - 1, 26)
            letters = chr(65 + remainder) + letters
        return letters

    normalized = value.replace("$", "").upper()
    match = re.fullmatch(
        r"(?P<start>[A-Z]{1,3}[1-9][0-9]*)(?::(?P<end>[A-Z]{1,3}[1-9][0-9]*))?",
        normalized,
    )
    if match is None:
        raise ValueError(f"invalid range: {value}")
    start = re.fullmatch(r"([A-Z]{1,3})([1-9][0-9]*)", match.group("start"))
    end = re.fullmatch(
        r"([A-Z]{1,3})([1-9][0-9]*)",
        match.group("end") or match.group("start"),
    )
    assert start is not None and end is not None
    min_col, min_row = column_index(start.group(1)), int(start.group(2))
    max_col, max_row = column_index(end.group(1)), int(end.group(2))
    if max_col > 16_384 or max_row > 1_048_576:
        raise ValueError(f"range exceeds Excel limits: {value}")
    if min_col > max_col or min_row > max_row:
        raise ValueError(f"range must be top-left to bottom-right: {value}")
    return [
        f"{column_letters(column)}{row}"
        for row in range(min_row, max_row + 1)
        for column in range(min_col, max_col + 1)
    ]


def _chart_range(formula: str, sheet_parts: dict[str, str]) -> tuple[str, str, list[str]]:
    normalized = formula.strip().lstrip("=")
    if "[" in normalized or "]" in normalized:
        raise ValueError("external workbook reference")
    match = re.fullmatch(
        r"(?:'(?P<quoted>(?:[^']|'')+)'|(?P<plain>[^!]+))!"
        r"(?P<range>\$?[A-Z]{1,3}\$?[1-9][0-9]*(?::\$?[A-Z]{1,3}\$?[1-9][0-9]*)?)",
        normalized,
    )
    if match is None:
        raise ValueError("unsupported local range formula")
    sheet = (match.group("quoted") or match.group("plain") or "").replace("''", "'")
    part = sheet_parts.get(sheet.casefold())
    if part is None or not part.startswith("xl/worksheets/"):
        raise ValueError(f"worksheet is missing or is not cell-backed: {sheet}")
    return sheet, part, _range_cells(match.group("range"))


def _worksheet_values(
    zf: zipfile.ZipFile,
    part: str,
    cells: list[str],
    shared_strings: list[str],
) -> tuple[list[object], list[str]]:
    root = parse_xml(read_text(zf, part))
    if root is None:
        return [], [f"worksheet XML is invalid: {part}"]
    cell_map = {
        item.attrib.get("r", "").replace("$", "").upper(): item
        for item in root.findall(".//main:c", NS)
    }
    values: list[object] = []
    unresolved: list[str] = []
    for coordinate in cells:
        cell = cell_map.get(coordinate.upper())
        if cell is None:
            values.append(None)
            continue
        formula = cell.find("main:f", NS)
        value = cell.find("main:v", NS)
        if formula is not None and (value is None or value.text is None):
            unresolved.append(coordinate)
            values.append(None)
            continue
        cell_type = cell.attrib.get("t", "")
        if cell_type == "inlineStr":
            values.append("".join(item.text or "" for item in cell.findall(".//main:t", NS)))
        elif cell_type == "s" and value is not None and (value.text or "").isdigit():
            index = int(value.text or "0")
            values.append(shared_strings[index] if index < len(shared_strings) else "")
        elif cell_type == "b":
            values.append((value.text or "0") == "1" if value is not None else False)
        elif value is None or value.text in {None, ""}:
            values.append(None)
        elif cell_type in {"str", "e"}:
            values.append(value.text or "")
        else:
            try:
                values.append(float(value.text or "0"))
            except ValueError:
                values.append(value.text or "")
    return values, unresolved


def _chart_values_equal(cached: str | None, expected: object) -> bool:
    cached_text = "" if cached is None else str(cached)
    if expected is None:
        return cached_text == ""
    if isinstance(expected, bool):
        return cached_text.casefold() in ({"true", "1"} if expected else {"false", "0"})
    if isinstance(expected, (int, float)):
        try:
            return abs(float(cached_text) - float(expected)) <= 1e-9
        except ValueError:
            return False
    return cached_text == str(expected)


def validate_chart_sources(
    zf: zipfile.ZipFile,
    chart_parts: list[str],
    sheets: list[dict[str, str]],
    shared_strings: list[str],
) -> tuple[list[str], list[str], list[dict[str, object]]]:
    errors: list[str] = []
    warnings: list[str] = []
    details: list[dict[str, object]] = []
    sheet_parts = {
        item["name"].casefold(): item["part"]
        for item in sheets
        if item.get("name") and item.get("part")
    }
    for chart_part in chart_parts:
        root = parse_xml(read_text(zf, chart_part))
        if root is None:
            errors.append(f"chart XML is invalid: {chart_part}")
            continue
        series_details: list[dict[str, object]] = []
        for series_index, series in enumerate(root.findall(".//c:ser", NS), start=1):
            references = []
            for role, names in (
                ("title", {"tx"}),
                ("categories", {"cat", "xVal"}),
                ("values", {"val", "yVal"}),
            ):
                container = next(
                    (item for item in list(series) if item.tag.rsplit("}", 1)[-1] in names),
                    None,
                )
                if container is None:
                    continue
                reference = next(
                    (
                        item
                        for item in list(container)
                        if item.tag.rsplit("}", 1)[-1] in {"strRef", "numRef"}
                    ),
                    None,
                )
                if reference is None:
                    continue
                formula = reference.find("c:f", NS)
                cache = next(
                    (
                        item
                        for item in list(reference)
                        if item.tag.rsplit("}", 1)[-1] in {"strCache", "numCache"}
                    ),
                    None,
                )
                formula_text = (formula.text or "").strip() if formula is not None else ""
                if not formula_text:
                    errors.append(
                        f"chart reference formula is missing: {chart_part} series {series_index} {role}"
                    )
                    continue
                try:
                    sheet, part, cells = _chart_range(formula_text, sheet_parts)
                except ValueError as error:
                    errors.append(
                        f"chart source formula is unsupported: {chart_part} series {series_index} "
                        f"{role} ({error})"
                    )
                    continue
                expected, unresolved = _worksheet_values(zf, part, cells, shared_strings)
                references.append({
                    "role": role,
                    "formula": formula_text,
                    "sheet": sheet,
                    "cells": cells,
                    "cachePoints": len(cache.findall("c:pt", NS)) if cache is not None else 0,
                    "unresolvedFormulaCells": unresolved,
                })
                if unresolved:
                    warnings.append(
                        f"chart source cache cannot be verified without recalculation: {chart_part} "
                        f"series {series_index} {role} ({', '.join(unresolved)})"
                    )
                    continue
                if cache is None:
                    warnings.append(
                        f"chart cache is absent: {chart_part} series {series_index} {role}"
                    )
                    continue
                cached = {
                    int(point.attrib.get("idx", "-1")): (
                        point.find("c:v", NS).text
                        if point.find("c:v", NS) is not None
                        else ""
                    )
                    for point in cache.findall("c:pt", NS)
                }
                point_count = cache.find("c:ptCount", NS)
                declared = int(point_count.attrib.get("val", "0")) if point_count is not None else len(cached)
                if declared != len(expected):
                    errors.append(
                        f"chart cache/source count mismatch: {chart_part} series {series_index} "
                        f"{role} cache={declared} source={len(expected)}"
                    )
                    continue
                for point_index, expected_value in enumerate(expected):
                    if not _chart_values_equal(cached.get(point_index, ""), expected_value):
                        errors.append(
                            f"chart cache/source value mismatch: {chart_part} series {series_index} "
                            f"{role} point={point_index} cache={cached.get(point_index, '')!r} "
                            f"source={expected_value!r}"
                        )
            series_details.append({"seriesIndex": series_index, "references": references})
        details.append({"part": chart_part, "series": series_details})
    return errors, warnings, details


def audit(path: Path) -> dict:
    warnings: list[str] = []
    with zipfile.ZipFile(path) as zf:
        names = set(zf.namelist())
        physical_sheet_parts = sorted(
            [name for name in names if re.match(r"xl/worksheets/sheet\d+\.xml$", name)],
            key=natural_key,
        )
        sheets = workbook_sheets(zf)
        sheet_summaries = []
        referenced_parts: set[str] = set()
        for index, sheet_meta in enumerate(sheets):
            sheet_part = sheet_meta.get("part", "")
            if not sheet_part or sheet_part not in names:
                warnings.append(
                    f"{sheet_meta.get('name', f'Sheet{index + 1}')} has an unresolved worksheet relationship"
                )
                continue
            referenced_parts.add(sheet_part)
            summary = (
                chartsheet_summary(
                    zf,
                    sheet_part,
                    sheet_meta.get("name", f"Sheet{index + 1}"),
                    sheet_meta.get("state", "visible"),
                )
                if sheet_part.startswith("xl/chartsheets/")
                else worksheet_summary(
                    zf,
                    sheet_part,
                    sheet_meta.get("name", f"Sheet{index + 1}"),
                    sheet_meta.get("state", "visible"),
                )
            )
            if summary.get("formula_errors"):
                warnings.append(f"{summary['name']} has formula errors")
            if summary.get("rows", 0) > 20 and not summary.get("has_autofilter"):
                warnings.append(f"{summary['name']} has many rows without autofilter")
            if summary.get("external_relationships"):
                warnings.append(f"{summary['name']} has external relationships")
            sheet_summaries.append(summary)
        orphan_parts = sorted(set(physical_sheet_parts) - referenced_parts, key=natural_key)
        if orphan_parts:
            warnings.append("orphan worksheet parts: " + ", ".join(orphan_parts))

        shared_strings = parse_xml(read_text(zf, "xl/sharedStrings.xml"))
        shared_string_values = [
            "".join(item.text or "" for item in value.findall(".//main:t", NS))
            for value in (list(shared_strings) if shared_strings is not None else [])
            if value.tag.rsplit("}", 1)[-1] == "si"
        ]
        calc_chain_present = "xl/calcChain.xml" in names
        workbook = parse_xml(read_text(zf, "xl/workbook.xml"))
        calc_mode = ""
        defined_names = []
        if workbook is not None:
            calc_pr = workbook.find("main:calcPr", NS)
            if calc_pr is not None:
                calc_mode = calc_pr.attrib.get("calcMode", "")
            for item in workbook.findall(".//main:definedName", NS):
                defined_names.append({
                    "name": item.attrib.get("name", ""),
                    "local_sheet_id": item.attrib.get("localSheetId"),
                    "formula": item.text or "",
                })
        table_details = []
        for part in sorted(name for name in names if re.fullmatch(r"xl/tables/table\d+\.xml", name)):
            table = parse_xml(read_text(zf, part))
            if table is not None:
                table_details.append({
                    "part": part,
                    "name": table.attrib.get("name", ""),
                    "display_name": table.attrib.get("displayName", ""),
                    "range": table.attrib.get("ref", ""),
                })
        chart_parts = sorted(name for name in names if re.fullmatch(r"xl/charts/chart\d+\.xml", name))
        chart_details = []
        for part in chart_parts:
            chart = parse_xml(read_text(zf, part))
            title = ""
            if chart is not None:
                title = "".join(
                    item.text or "" for item in chart.iter()
                    if item.tag.rsplit("}", 1)[-1] == "t"
                )
            chart_details.append({"part": part, "title": title})
        chart_errors, chart_warnings, chart_sources = validate_chart_sources(
            zf, chart_parts, sheets, shared_string_values
        )
        warnings.extend(chart_warnings)
        for detail in chart_details:
            source = next(
                (item for item in chart_sources if item["part"] == detail["part"]),
                None,
            )
            if source is not None:
                detail["series"] = source["series"]

        return {
            "path": str(path),
            "format": "xlsx",
            "package_parts": len(names),
            "sheets": len(sheet_summaries),
            "sheet_details": sheet_summaries,
            "orphan_sheet_parts": orphan_parts,
            "shared_strings": int(shared_strings.attrib.get("count", "0"))
            if shared_strings is not None
            else 0,
            "calc_chain_present": calc_chain_present,
            "calc_mode": calc_mode,
            "defined_names": defined_names,
            "table_details": table_details,
            "chart_details": chart_details,
            "chart_validation_errors": chart_errors,
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
    if path.suffix.lower() not in {".xlsx", ".xlsm", ".xltx", ".xltm"}:
        print(f"Expected .xlsx/.xlsm/.xltx/.xltm file: {path}", file=sys.stderr)
        return 3
    if not zipfile.is_zipfile(path):
        print(f"Not a valid OOXML zip package: {path}", file=sys.stderr)
        return 3

    result = audit(path)
    print(json.dumps(result, ensure_ascii=False, indent=2 if args.pretty else None))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
