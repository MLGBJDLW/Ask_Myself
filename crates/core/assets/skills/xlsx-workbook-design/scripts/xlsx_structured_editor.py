#!/usr/bin/env python3
"""Loss-minimizing typed XLSX edits using direct worksheet OOXML patches."""

from __future__ import annotations

import argparse
import json
import math
import posixpath
import re
import sys
import zipfile
from pathlib import Path
from typing import Any
from xml.etree import ElementTree as ET


MAIN_NS = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
REL_NS = "http://schemas.openxmlformats.org/package/2006/relationships"
DOC_REL_NS = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
XML_NS = "http://www.w3.org/XML/1998/namespace"
CT_NS = "http://schemas.openxmlformats.org/package/2006/content-types"
CHART_NS = "http://schemas.openxmlformats.org/drawingml/2006/chart"
DRAWING_NS = "http://schemas.openxmlformats.org/drawingml/2006/main"
TABLE_REL = f"{DOC_REL_NS}/table"
TABLE_CONTENT_TYPE = "application/vnd.openxmlformats-officedocument.spreadsheetml.table+xml"
CELL_RE = re.compile(r"^\$?([A-Z]{1,3})\$?([1-9][0-9]*)$")
RANGE_RE = re.compile(r"^(\$?[A-Z]{1,3}\$?[1-9][0-9]*)(?::(\$?[A-Z]{1,3}\$?[1-9][0-9]*))?$")
SHEET_OPERATIONS = {
    "set_value", "set_formula", "set_range", "clear_range", "set_style",
    "set_data_validation", "create_table",
}
SUPPORTED_OPERATIONS = SHEET_OPERATIONS | {
    "rename_sheet", "set_defined_name", "set_data_validation", "create_table",
    "set_number_format", "set_chart_title", "set_chart_data",
}


class XlsxEditError(ValueError):
    pass


def _local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def _q(local: str) -> str:
    return f"{{{MAIN_NS}}}{local}"


def _column_index(letters: str) -> int:
    value = 0
    for letter in letters:
        value = value * 26 + ord(letter) - 64
    return value


def _column_letters(index: int) -> str:
    letters = ""
    while index:
        index, remainder = divmod(index - 1, 26)
        letters = chr(65 + remainder) + letters
    return letters


def _coordinate(value: str) -> tuple[int, int, str]:
    normalized = value.replace("$", "").upper()
    match = CELL_RE.fullmatch(normalized)
    if not match:
        raise XlsxEditError(f"invalid cell coordinate: {value}")
    column = _column_index(match.group(1))
    row = int(match.group(2))
    if row > 1_048_576 or column > 16_384:
        raise XlsxEditError(f"cell is outside Excel limits: {value}")
    return row, column, f"{match.group(1)}{row}"


def _range_cells(value: str) -> list[str]:
    match = RANGE_RE.fullmatch(value.replace(" ", "").upper())
    if not match:
        raise XlsxEditError(f"invalid range: {value}")
    start_row, start_col, _ = _coordinate(match.group(1))
    end_row, end_col, _ = _coordinate(match.group(2) or match.group(1))
    if end_row < start_row or end_col < start_col:
        raise XlsxEditError(f"range must be top-left to bottom-right: {value}")
    return [
        f"{_column_letters(column)}{row}"
        for row in range(start_row, end_row + 1)
        for column in range(start_col, end_col + 1)
    ]


def _range_shape(value: str) -> tuple[int, int]:
    match = RANGE_RE.fullmatch(value.replace(" ", "").upper())
    if not match:
        raise XlsxEditError(f"invalid range: {value}")
    start_row, start_col, _ = _coordinate(match.group(1))
    end_row, end_col, _ = _coordinate(match.group(2) or match.group(1))
    if end_row < start_row or end_col < start_col:
        raise XlsxEditError(f"range must be top-left to bottom-right: {value}")
    return end_row - start_row + 1, end_col - start_col + 1


def _workbook_sheet_parts(archive: zipfile.ZipFile) -> dict[str, str]:
    workbook = ET.fromstring(archive.read("xl/workbook.xml"))
    relationships = ET.fromstring(archive.read("xl/_rels/workbook.xml.rels"))
    relationship_map: dict[str, str] = {}
    for relationship in relationships:
        if _local(relationship.tag) != "Relationship":
            continue
        relationship_id = relationship.attrib.get("Id", "")
        target = relationship.attrib.get("Target", "")
        if relationship.attrib.get("TargetMode") == "External" or not relationship_id or not target:
            continue
        relationship_map[relationship_id] = posixpath.normpath(
            target.lstrip("/") if target.startswith("/") else f"xl/{target}"
        )
    sheets: dict[str, str] = {}
    for sheet in workbook.iter():
        if _local(sheet.tag) != "sheet":
            continue
        name = sheet.attrib.get("name", "")
        relationship_id = sheet.attrib.get(f"{{{DOC_REL_NS}}}id", "")
        part = relationship_map.get(relationship_id)
        if not name or not part:
            raise XlsxEditError(f"worksheet relationship is unresolved: {name or relationship_id}")
        key = name.casefold()
        if key in sheets:
            raise XlsxEditError(f"duplicate case-insensitive worksheet name: {name}")
        sheets[key] = part
    return sheets


def _validate_sheet_name(name: str) -> None:
    if not name or len(name) > 31 or any(character in name for character in "[]:*?/\\"):
        raise XlsxEditError("worksheet name must be 1-31 characters without []:*?/\\")
    if name.startswith("'") or name.endswith("'"):
        raise XlsxEditError("worksheet name cannot start or end with an apostrophe")


def _formula_sheet_name(name: str) -> str:
    escaped = name.replace("'", "''")
    return name if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_.]*", name) else f"'{escaped}'"


def _replace_sheet_reference(text: str, old: str, new: str) -> str:
    quoted_old = "'" + old.replace("'", "''") + "'!"
    replacement = _formula_sheet_name(new) + "!"
    text = re.sub(re.escape(quoted_old), replacement, text, flags=re.IGNORECASE)
    if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_.]*", old):
        text = re.sub(
            rf"(?<![A-Za-z0-9_.']){re.escape(old)}!",
            replacement,
            text,
            flags=re.IGNORECASE,
        )
    return text


def _sheet_relationships_name(sheet_part: str) -> str:
    directory, filename = posixpath.split(sheet_part)
    return f"{directory}/_rels/{filename}.rels"


def _insert_worksheet_child(root: ET.Element, child: ET.Element, before_names: set[str]) -> None:
    for existing in list(root):
        if _local(existing.tag) in before_names:
            root.insert(list(root).index(existing), child)
            return
    root.append(child)


def _next_relationship_id(root: ET.Element) -> str:
    used = {child.attrib.get("Id", "") for child in root}
    index = 1
    while f"rId{index}" in used:
        index += 1
    return f"rId{index}"


def _cell_text(root: ET.Element, coordinate: str, shared_strings: list[str]) -> str:
    normalized = coordinate.replace("$", "").upper()
    cell = next(
        (item for item in root.iter() if _local(item.tag) == "c" and item.attrib.get("r", "").upper() == normalized),
        None,
    )
    if cell is None:
        return ""
    if cell.attrib.get("t") == "inlineStr":
        return "".join(item.text or "" for item in cell.iter() if _local(item.tag) == "t")
    value = next((item.text or "" for item in cell if _local(item.tag) == "v"), "")
    if cell.attrib.get("t") == "s" and value.isdigit() and int(value) < len(shared_strings):
        return shared_strings[int(value)]
    return value


def _cell_xfs_count(archive: zipfile.ZipFile) -> int:
    try:
        root = ET.fromstring(archive.read("xl/styles.xml"))
    except KeyError:
        return 1
    for child in root:
        if _local(child.tag) == "cellXfs":
            return int(child.attrib.get("count", len(list(child))) or len(list(child)))
    return 1


def _find_or_create_row(sheet_data: ET.Element, row_number: int) -> ET.Element:
    rows = [child for child in sheet_data if _local(child.tag) == "row"]
    for row in rows:
        current = int(row.attrib.get("r", "0") or 0)
        if current == row_number:
            return row
        if current > row_number:
            new_row = ET.Element(_q("row"), {"r": str(row_number)})
            sheet_data.insert(list(sheet_data).index(row), new_row)
            return new_row
    new_row = ET.Element(_q("row"), {"r": str(row_number)})
    sheet_data.append(new_row)
    return new_row


def _find_or_create_cell(root: ET.Element, coordinate: str) -> ET.Element:
    row_number, column_number, normalized = _coordinate(coordinate)
    sheet_data = next((child for child in root if _local(child.tag) == "sheetData"), None)
    if sheet_data is None:
        raise XlsxEditError("worksheet has no sheetData element")
    row = _find_or_create_row(sheet_data, row_number)
    cells = [child for child in row if _local(child.tag) == "c"]
    for cell in cells:
        _, current_column, current_coordinate = _coordinate(cell.attrib.get("r", ""))
        if current_coordinate == normalized:
            return cell
        if current_column > column_number:
            new_cell = ET.Element(_q("c"), {"r": normalized})
            row.insert(list(row).index(cell), new_cell)
            return new_cell
    new_cell = ET.Element(_q("c"), {"r": normalized})
    row.append(new_cell)
    return new_cell


def _remove_value_children(cell: ET.Element) -> None:
    for child in list(cell):
        if _local(child.tag) in {"f", "v", "is"}:
            cell.remove(child)


def _write_literal(cell: ET.Element, value: Any) -> None:
    _remove_value_children(cell)
    if value is None:
        cell.attrib.pop("t", None)
        return
    if isinstance(value, bool):
        cell.set("t", "b")
        ET.SubElement(cell, _q("v")).text = "1" if value else "0"
        return
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        cell.attrib.pop("t", None)
        ET.SubElement(cell, _q("v")).text = str(value)
        return
    text = str(value)
    cell.set("t", "inlineStr")
    inline = ET.SubElement(cell, _q("is"))
    node = ET.SubElement(inline, _q("t"))
    if text[:1].isspace() or text[-1:].isspace():
        node.set(f"{{{XML_NS}}}space", "preserve")
    node.text = text


def _write_formula(cell: ET.Element, formula: Any, cached_value: Any = None) -> None:
    formula_text = str(formula or "").strip()
    if not formula_text:
        raise XlsxEditError("formula cannot be empty")
    _remove_value_children(cell)
    cell.attrib.pop("t", None)
    ET.SubElement(cell, _q("f")).text = formula_text[1:] if formula_text.startswith("=") else formula_text
    if cached_value is not None:
        if isinstance(cached_value, bool):
            cell.set("t", "b")
            text = "1" if cached_value else "0"
        elif isinstance(cached_value, (int, float)) and not isinstance(cached_value, bool):
            text = str(cached_value)
        else:
            cell.set("t", "str")
            text = str(cached_value)
        ET.SubElement(cell, _q("v")).text = text


def _remove_cell(root: ET.Element, coordinate: str) -> bool:
    _, _, normalized = _coordinate(coordinate)
    sheet_data = next((child for child in root if _local(child.tag) == "sheetData"), None)
    if sheet_data is None:
        return False
    for row in list(sheet_data):
        for cell in list(row):
            if _local(cell.tag) == "c" and cell.attrib.get("r", "").replace("$", "").upper() == normalized:
                row.remove(cell)
                if not any(_local(child.tag) == "c" for child in row):
                    sheet_data.remove(row)
                return True
    return False


def _update_dimension(root: ET.Element) -> None:
    refs = [
        cell.attrib.get("r", "")
        for cell in root.iter()
        if _local(cell.tag) == "c" and cell.attrib.get("r")
    ]
    if not refs:
        bounds = "A1"
    else:
        coordinates = [_coordinate(ref) for ref in refs]
        min_row = min(item[0] for item in coordinates)
        max_row = max(item[0] for item in coordinates)
        min_col = min(item[1] for item in coordinates)
        max_col = max(item[1] for item in coordinates)
        start = f"{_column_letters(min_col)}{min_row}"
        end = f"{_column_letters(max_col)}{max_row}"
        bounds = start if start == end else f"{start}:{end}"
    dimension = next((child for child in root if _local(child.tag) == "dimension"), None)
    if dimension is not None:
        dimension.set("ref", bounds)


def _plan_operations(
    operations: list[dict[str, Any]],
    sheet_parts: dict[str, str],
) -> dict[str, list[dict[str, Any]]]:
    planned: dict[str, list[dict[str, Any]]] = {}
    for index, operation in enumerate(operations):
        name = str(operation.get("op", "")).lower()
        if name not in SUPPORTED_OPERATIONS:
            raise XlsxEditError(f"unsupported XLSX operation at index {index}: {name or '<missing>'}")
        if name not in SHEET_OPERATIONS:
            continue
        sheet_name = str(operation.get("sheet", ""))
        part = sheet_parts.get(sheet_name.casefold())
        if not part:
            raise XlsxEditError(f"worksheet not found: {sheet_name}")
        planned.setdefault(part, []).append(operation)
    return planned


def _rename_sheet(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    sheet_parts: dict[str, str],
    operation: dict[str, Any],
) -> list[str]:
    old = str(operation.get("sheet", ""))
    new = str(operation.get("newName", ""))
    _validate_sheet_name(new)
    if old.casefold() not in sheet_parts:
        raise XlsxEditError(f"worksheet not found: {old}")
    if new.casefold() in sheet_parts and new.casefold() != old.casefold():
        raise XlsxEditError(f"worksheet name already exists: {new}")
    workbook = ET.fromstring(replacements.get("xl/workbook.xml", archive.read("xl/workbook.xml")))
    matched = False
    for sheet in workbook.iter():
        if _local(sheet.tag) == "sheet" and sheet.attrib.get("name", "").casefold() == old.casefold():
            sheet.set("name", new)
            matched = True
            break
    if not matched:
        raise XlsxEditError(f"worksheet not found: {old}")
    for element in workbook.iter():
        if _local(element.tag) == "definedName" and element.text:
            element.text = _replace_sheet_reference(element.text, old, new)
    replacements["xl/workbook.xml"] = ET.tostring(workbook, encoding="utf-8", xml_declaration=True)
    changed = ["xl/workbook.xml"]
    formula_tags = {"f", "formula", "formula1", "formula2", "calculatedColumnFormula", "totalsRowFormula"}
    for part in archive.namelist():
        if (
            not part.startswith("xl/")
            or not part.endswith(".xml")
            or part == "xl/workbook.xml"
        ):
            continue
        root = ET.fromstring(replacements.get(part, archive.read(part)))
        touched = False
        for formula in root.iter():
            if _local(formula.tag) not in formula_tags:
                continue
            updated = _replace_sheet_reference(formula.text or "", old, new)
            if updated != (formula.text or ""):
                formula.text = updated
                touched = True
        if touched:
            replacements[part] = ET.tostring(root, encoding="utf-8", xml_declaration=True)
            changed.append(part)
    target_part = sheet_parts.pop(old.casefold())
    sheet_parts[new.casefold()] = target_part
    return changed


def _set_defined_name(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    operation: dict[str, Any],
) -> str:
    name = str(operation.get("name", ""))
    formula = str(operation.get("formula", ""))
    if re.fullmatch(r"[A-Za-z_\\][A-Za-z0-9_.]*", name) is None or CELL_RE.fullmatch(name.upper()):
        raise XlsxEditError("defined name is not a valid Excel identifier")
    if not formula or re.search(r"(?i)(https?://|file:|\\\\|\[[^\]]+\])", formula):
        raise XlsxEditError("defined name formula must be local and network-closed")
    workbook = ET.fromstring(replacements.get("xl/workbook.xml", archive.read("xl/workbook.xml")))
    sheets = [item for item in workbook.iter() if _local(item.tag) == "sheet"]
    scope = operation.get("scopeSheet")
    local_sheet_id = None
    if scope is not None:
        local_sheet_id = next(
            (index for index, item in enumerate(sheets) if item.attrib.get("name", "").casefold() == str(scope).casefold()),
            None,
        )
        if local_sheet_id is None:
            raise XlsxEditError(f"defined-name scope worksheet not found: {scope}")
    container = next((item for item in workbook if _local(item.tag) == "definedNames"), None)
    if container is None:
        container = ET.Element(_q("definedNames"))
        calc = next((item for item in workbook if _local(item.tag) == "calcPr"), None)
        workbook.insert(list(workbook).index(calc) if calc is not None else len(workbook), container)
    for existing in list(container):
        existing_scope = existing.attrib.get("localSheetId")
        expected_scope = str(local_sheet_id) if local_sheet_id is not None else None
        if existing.attrib.get("name", "").casefold() == name.casefold() and existing_scope == expected_scope:
            container.remove(existing)
    attributes = {"name": name}
    if local_sheet_id is not None:
        attributes["localSheetId"] = str(local_sheet_id)
    ET.SubElement(container, _q("definedName"), attributes).text = formula.lstrip("=")
    replacements["xl/workbook.xml"] = ET.tostring(workbook, encoding="utf-8", xml_declaration=True)
    return "xl/workbook.xml"


def _set_data_validation(
    root: ET.Element,
    operation: dict[str, Any],
) -> None:
    range_ref = str(operation.get("range", ""))
    _range_cells(range_ref)
    validation_type = str(operation.get("validationType", ""))
    if validation_type not in {"whole", "decimal", "list", "date", "time", "textLength", "custom"}:
        raise XlsxEditError("unsupported data validation type")
    container = next((item for item in root if _local(item.tag) == "dataValidations"), None)
    if container is None:
        container = ET.Element(_q("dataValidations"))
        _insert_worksheet_child(
            root,
            container,
            {"hyperlinks", "printOptions", "pageMargins", "pageSetup", "headerFooter", "drawing", "tableParts"},
        )
    attributes = {"type": validation_type, "sqref": range_ref.replace("$", "").upper()}
    for source, target in (
        ("operator", "operator"), ("allowBlank", "allowBlank"),
        ("showErrorMessage", "showErrorMessage"), ("errorTitle", "errorTitle"), ("error", "error"),
    ):
        if source in operation:
            value = operation[source]
            attributes[target] = ("1" if value else "0") if type(value) is bool else str(value)
    validation = ET.SubElement(container, _q("dataValidation"), attributes)
    if operation.get("formula1") is not None:
        ET.SubElement(validation, _q("formula1")).text = str(operation["formula1"]).lstrip("=")
    if operation.get("formula2") is not None:
        ET.SubElement(validation, _q("formula2")).text = str(operation["formula2"]).lstrip("=")
    container.set("count", str(len(list(container))))


def _set_chart_title(data: bytes, title: str) -> bytes:
    root = ET.fromstring(data)
    chart = next((item for item in root.iter() if _local(item.tag) == "chart"), None)
    if chart is None:
        raise XlsxEditError("chart part has no chart element")
    for item in list(chart):
        if _local(item.tag) == "title":
            chart.remove(item)
    title_node = ET.Element(f"{{{CHART_NS}}}title")
    tx = ET.SubElement(title_node, f"{{{CHART_NS}}}tx")
    rich = ET.SubElement(tx, f"{{{CHART_NS}}}rich")
    ET.SubElement(rich, f"{{{DRAWING_NS}}}bodyPr")
    ET.SubElement(rich, f"{{{DRAWING_NS}}}lstStyle")
    paragraph = ET.SubElement(rich, f"{{{DRAWING_NS}}}p")
    run = ET.SubElement(paragraph, f"{{{DRAWING_NS}}}r")
    ET.SubElement(run, f"{{{DRAWING_NS}}}t").text = title
    insert_at = next(
        (index for index, item in enumerate(list(chart)) if _local(item.tag) in {"plotArea", "legend", "plotVisOnly"}),
        len(list(chart)),
    )
    chart.insert(insert_at, title_node)
    return ET.tostring(root, encoding="utf-8", xml_declaration=True)


def _chart_series(root: ET.Element, series_index: int) -> ET.Element:
    series = [item for item in root.iter() if _local(item.tag) == "ser"]
    if series_index < 1 or series_index > len(series):
        raise XlsxEditError(
            f"seriesIndex {series_index} is outside chart series range 1..{len(series)}"
        )
    return series[series_index - 1]


def _series_container(series: ET.Element, names: set[str]) -> ET.Element:
    container = next(
        (item for item in list(series) if _local(item.tag) in names),
        None,
    )
    if container is None:
        raise XlsxEditError(
            "chart series is missing " + "/".join(sorted(names)) + " data"
        )
    return container


def _reference_formula(container: ET.Element) -> str:
    reference = next(
        (
            item
            for item in list(container)
            if _local(item.tag) in {"strRef", "numRef"}
        ),
        None,
    )
    if reference is None:
        raise XlsxEditError("chart data uses inline or unsupported references")
    formula = next(
        (item.text or "" for item in reference if _local(item.tag) == "f"),
        "",
    ).strip()
    if not formula:
        raise XlsxEditError("chart reference formula is missing")
    return formula


def _parse_chart_range(
    formula: str,
    sheet_parts: dict[str, str],
) -> dict[str, Any]:
    normalized = formula.strip().lstrip("=")
    if "[" in normalized or "]" in normalized:
        raise XlsxEditError("chart data references must stay inside the current workbook")
    match = re.fullmatch(
        r"(?:'(?P<quoted>(?:[^']|'')+)'|(?P<plain>[^!]+))!"
        r"(?P<range>\$?[A-Z]{1,3}\$?[1-9][0-9]*(?::\$?[A-Z]{1,3}\$?[1-9][0-9]*)?)",
        normalized,
    )
    if match is None:
        raise XlsxEditError(f"chart data reference must be a local cell range: {formula}")
    sheet_name = (match.group("quoted") or match.group("plain") or "").replace("''", "'")
    part = sheet_parts.get(sheet_name.casefold())
    if part is None or not part.startswith("xl/worksheets/"):
        raise XlsxEditError(f"chart data worksheet not found or unsupported: {sheet_name}")
    cells = _range_cells(match.group("range"))
    return {
        "formula": normalized,
        "sheet": sheet_name,
        "part": part,
        "cells": cells,
    }


def _chart_data_targets(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    sheet_parts: dict[str, str],
    operation: dict[str, Any],
) -> dict[str, dict[str, Any]]:
    chart_part = str(operation.get("chartPart", ""))
    if (
        re.fullmatch(r"xl/charts/chart[0-9]+\.xml", chart_part) is None
        or chart_part not in archive.namelist()
    ):
        raise XlsxEditError("chartPart must identify an existing xl/charts/chartN.xml part")
    root = ET.fromstring(replacements.get(chart_part, archive.read(chart_part)))
    series = _chart_series(root, int(operation.get("seriesIndex", 0)))
    categories = _series_container(series, {"cat", "xVal"})
    values = _series_container(series, {"val", "yVal"})
    targets = {
        "categories": _parse_chart_range(
            str(operation.get("categoryRange") or _reference_formula(categories)),
            sheet_parts,
        ),
        "values": _parse_chart_range(
            str(operation.get("valueRange") or _reference_formula(values)),
            sheet_parts,
        ),
    }
    if operation.get("seriesName") is not None:
        title = next(
            (item for item in list(series) if _local(item.tag) == "tx"),
            None,
        )
        if title is not None:
            try:
                title_target = _parse_chart_range(_reference_formula(title), sheet_parts)
            except XlsxEditError as error:
                if "inline or unsupported" not in str(error):
                    raise
            else:
                if len(title_target["cells"]) != 1:
                    raise XlsxEditError("chart series title reference must contain one cell")
                targets["seriesName"] = title_target
    occupied: dict[tuple[str, str], str] = {}
    for role, target in targets.items():
        for cell in target["cells"]:
            key = (str(target["part"]), cell)
            if key in occupied:
                raise XlsxEditError(
                    f"chart data target ranges overlap at {target['sheet']}!{cell} "
                    f"({occupied[key]} and {role})"
                )
            occupied[key] = role
    return targets


def _cache_text(value: Any, *, numeric: bool) -> str:
    if numeric:
        if type(value) not in {int, float} or not math.isfinite(float(value)):
            raise XlsxEditError("chart values must be finite JSON numbers")
        return str(value)
    if isinstance(value, (dict, list)) or (
        isinstance(value, float) and not math.isfinite(value)
    ):
        raise XlsxEditError("chart categories must be finite scalar JSON values")
    if value is None:
        return ""
    if isinstance(value, bool):
        return "TRUE" if value else "FALSE"
    return str(value)


def _replace_chart_reference(
    container: ET.Element,
    formula: str,
    values: list[Any],
    *,
    numeric: bool,
) -> None:
    format_code = "General"
    for reference in list(container):
        if _local(reference.tag) not in {"strRef", "numRef"}:
            continue
        existing_cache = next(
            (
                item
                for item in reference
                if _local(item.tag) in {"strCache", "numCache"}
            ),
            None,
        )
        if existing_cache is not None:
            existing_format = next(
                (item.text or "" for item in existing_cache if _local(item.tag) == "formatCode"),
                "",
            )
            if existing_format:
                format_code = existing_format
        container.remove(reference)
    reference = ET.Element(f"{{{CHART_NS}}}{'numRef' if numeric else 'strRef'}")
    ET.SubElement(reference, f"{{{CHART_NS}}}f").text = formula.lstrip("=")
    cache = ET.SubElement(reference, f"{{{CHART_NS}}}{'numCache' if numeric else 'strCache'}")
    if numeric:
        ET.SubElement(cache, f"{{{CHART_NS}}}formatCode").text = format_code
    ET.SubElement(cache, f"{{{CHART_NS}}}ptCount", {"val": str(len(values))})
    for index, value in enumerate(values):
        point = ET.SubElement(cache, f"{{{CHART_NS}}}pt", {"idx": str(index)})
        ET.SubElement(point, f"{{{CHART_NS}}}v").text = _cache_text(
            value, numeric=numeric
        )
    container.insert(0, reference)


def _write_chart_source_values(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    target: dict[str, Any],
    values: list[Any],
) -> list[str]:
    cells = list(target["cells"])
    if len(cells) != len(values):
        raise XlsxEditError(
            f"chart {target['sheet']} range has {len(cells)} cells but received {len(values)} values"
        )
    part = str(target["part"])
    root = ET.fromstring(replacements.get(part, archive.read(part)))
    for cell, value in zip(cells, values, strict=True):
        _write_literal(_find_or_create_cell(root, cell), value)
    _update_dimension(root)
    replacements[part] = ET.tostring(root, encoding="utf-8", xml_declaration=True)
    return [f"{target['sheet']}!{cell}" for cell in cells]


def _set_chart_data(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    sheet_parts: dict[str, str],
    operation: dict[str, Any],
) -> list[str]:
    categories = operation.get("categories")
    values = operation.get("values")
    if not isinstance(categories, list) or not categories:
        raise XlsxEditError("set_chart_data categories must be a non-empty array")
    if not isinstance(values, list) or not values:
        raise XlsxEditError("set_chart_data values must be a non-empty array")
    if len(categories) != len(values):
        raise XlsxEditError("set_chart_data categories and values must have the same length")
    for value in values:
        _cache_text(value, numeric=True)
    for category in categories:
        _cache_text(category, numeric=False)

    chart_part = str(operation["chartPart"])
    root = ET.fromstring(replacements.get(chart_part, archive.read(chart_part)))
    series = _chart_series(root, int(operation["seriesIndex"]))
    targets = _chart_data_targets(
        archive, replacements, sheet_parts, operation
    )
    changed = _write_chart_source_values(
        archive, replacements, targets["categories"], categories
    )
    changed.extend(
        _write_chart_source_values(archive, replacements, targets["values"], values)
    )
    _replace_chart_reference(
        _series_container(series, {"cat", "xVal"}),
        str(targets["categories"]["formula"]),
        categories,
        numeric=all(type(item) in {int, float} for item in categories),
    )
    _replace_chart_reference(
        _series_container(series, {"val", "yVal"}),
        str(targets["values"]["formula"]),
        values,
        numeric=True,
    )
    if operation.get("seriesName") is not None:
        series_name = str(operation["seriesName"])
        title = next(
            (item for item in list(series) if _local(item.tag) == "tx"),
            None,
        )
        if title is None:
            title = ET.Element(f"{{{CHART_NS}}}tx")
            insert_at = next(
                (
                    index
                    for index, child in enumerate(list(series))
                    if _local(child.tag) not in {"idx", "order"}
                ),
                len(list(series)),
            )
            series.insert(insert_at, title)
        if "seriesName" in targets:
            changed.extend(
                _write_chart_source_values(
                    archive, replacements, targets["seriesName"], [series_name]
                )
            )
            _replace_chart_reference(
                title,
                str(targets["seriesName"]["formula"]),
                [series_name],
                numeric=False,
            )
        else:
            for child in list(title):
                title.remove(child)
            ET.SubElement(title, f"{{{CHART_NS}}}v").text = series_name
    replacements[chart_part] = ET.tostring(
        root, encoding="utf-8", xml_declaration=True
    )
    changed.append(chart_part)
    return changed


def _create_table(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    additions: dict[str, bytes],
    sheet_part: str,
    root: ET.Element,
    operation: dict[str, Any],
) -> str:
    name = str(operation.get("name", ""))
    if re.fullmatch(r"[A-Za-z_\\][A-Za-z0-9_.]*", name) is None or CELL_RE.fullmatch(name.upper()):
        raise XlsxEditError("table name is not a valid Excel identifier")
    cells = _range_cells(str(operation.get("range", "")))
    start_row, start_column, _ = _coordinate(cells[0])
    end_row, end_column, _ = _coordinate(cells[-1])
    if start_row == end_row:
        raise XlsxEditError("table range requires a header row and at least one data row")
    width = end_column - start_column + 1
    shared_strings: list[str] = []
    if "xl/sharedStrings.xml" in archive.namelist():
        shared_root = ET.fromstring(archive.read("xl/sharedStrings.xml"))
        shared_strings = [
            "".join(item.text or "" for item in value.iter() if _local(item.tag) == "t")
            for value in shared_root
            if _local(value.tag) == "si"
        ]
    columns = operation.get("columns")
    if columns is None:
        columns = [
            _cell_text(root, f"{_column_letters(column)}{start_row}", shared_strings)
            for column in range(start_column, end_column + 1)
        ]
    if not isinstance(columns, list) or len(columns) != width or not all(str(item) for item in columns):
        raise XlsxEditError(f"table columns must contain {width} non-empty names")
    if len({str(item).casefold() for item in columns}) != len(columns):
        raise XlsxEditError("table column names must be case-insensitively unique")

    existing_table_parts = sorted(
        name for name in archive.namelist() if re.fullmatch(r"xl/tables/table[0-9]+\.xml", name)
    )
    table_ids = []
    table_names = set()
    for part in existing_table_parts + sorted(name for name in additions if name.startswith("xl/tables/")):
        table = ET.fromstring(additions.get(part, archive.read(part) if part in archive.namelist() else b""))
        table_ids.append(int(table.attrib.get("id", "0") or 0))
        table_names.update({table.attrib.get("name", "").casefold(), table.attrib.get("displayName", "").casefold()})
    if name.casefold() in table_names:
        raise XlsxEditError(f"table name already exists: {name}")
    table_id = max(table_ids, default=0) + 1
    part_index = 1
    while f"xl/tables/table{part_index}.xml" in set(archive.namelist()) | set(additions):
        part_index += 1
    table_part = f"xl/tables/table{part_index}.xml"
    range_ref = str(operation["range"]).replace("$", "").upper()
    table = ET.Element(_q("table"), {
        "id": str(table_id), "name": name, "displayName": name,
        "ref": range_ref, "headerRowCount": "1",
    })
    ET.SubElement(table, _q("autoFilter"), {"ref": range_ref})
    table_columns = ET.SubElement(table, _q("tableColumns"), {"count": str(width)})
    for index, column in enumerate(columns, start=1):
        ET.SubElement(table_columns, _q("tableColumn"), {"id": str(index), "name": str(column)})
    ET.SubElement(table, _q("tableStyleInfo"), {
        "name": str(operation.get("styleName", "TableStyleMedium2")),
        "showFirstColumn": "0", "showLastColumn": "0",
        "showRowStripes": "1", "showColumnStripes": "0",
    })
    additions[table_part] = ET.tostring(table, encoding="utf-8", xml_declaration=True)

    rels_name = _sheet_relationships_name(sheet_part)
    if rels_name in replacements:
        rels = ET.fromstring(replacements[rels_name])
    elif rels_name in additions:
        rels = ET.fromstring(additions[rels_name])
    elif rels_name in archive.namelist():
        rels = ET.fromstring(replacements.get(rels_name, archive.read(rels_name)))
    else:
        rels = ET.Element(f"{{{REL_NS}}}Relationships")
    relationship_id = _next_relationship_id(rels)
    ET.SubElement(rels, f"{{{REL_NS}}}Relationship", {
        "Id": relationship_id, "Type": TABLE_REL,
        "Target": f"../tables/{posixpath.basename(table_part)}",
    })
    target_store = replacements if rels_name in archive.namelist() else additions
    target_store[rels_name] = ET.tostring(rels, encoding="utf-8", xml_declaration=True)

    table_parts = next((item for item in root if _local(item.tag) == "tableParts"), None)
    if table_parts is None:
        table_parts = ET.Element(_q("tableParts"))
        root.append(table_parts)
    ET.SubElement(table_parts, _q("tablePart"), {f"{{{DOC_REL_NS}}}id": relationship_id})
    table_parts.set("count", str(len(list(table_parts))))

    content_types = ET.fromstring(
        replacements.get("[Content_Types].xml", archive.read("[Content_Types].xml"))
    )
    ET.SubElement(content_types, f"{{{CT_NS}}}Override", {
        "PartName": f"/{table_part}", "ContentType": TABLE_CONTENT_TYPE,
    })
    replacements["[Content_Types].xml"] = ET.tostring(
        content_types, encoding="utf-8", xml_declaration=True
    )
    return table_part


def _create_number_format_style(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    operation: dict[str, Any],
) -> int:
    format_code = str(operation.get("formatCode", ""))
    if not format_code or len(format_code) > 255:
        raise XlsxEditError("formatCode must be 1-255 characters")
    styles_name = "xl/styles.xml"
    if styles_name not in archive.namelist():
        raise XlsxEditError("set_number_format requires xl/styles.xml")
    root = ET.fromstring(replacements.get(styles_name, archive.read(styles_name)))
    num_formats = next((item for item in root if _local(item.tag) == "numFmts"), None)
    if num_formats is None:
        num_formats = ET.Element(_q("numFmts"), {"count": "0"})
        cell_style_xfs = next((item for item in root if _local(item.tag) == "cellStyleXfs"), None)
        root.insert(list(root).index(cell_style_xfs) if cell_style_xfs is not None else 0, num_formats)
    existing_ids = [
        int(item.attrib.get("numFmtId", "0") or 0)
        for item in num_formats if _local(item.tag) == "numFmt"
    ]
    number_format_id = max([163, *existing_ids]) + 1
    ET.SubElement(num_formats, _q("numFmt"), {
        "numFmtId": str(number_format_id), "formatCode": format_code,
    })
    num_formats.set("count", str(len(list(num_formats))))
    cell_xfs = next((item for item in root if _local(item.tag) == "cellXfs"), None)
    if cell_xfs is None or not list(cell_xfs):
        raise XlsxEditError("styles.xml has no cellXfs")
    base_style_id = int(operation.get("baseStyleId", 0))
    if base_style_id < 0 or base_style_id >= len(list(cell_xfs)):
        raise XlsxEditError("baseStyleId is outside workbook cellXfs")
    base = dict(list(cell_xfs)[base_style_id].attrib)
    base.update({"numFmtId": str(number_format_id), "applyNumberFormat": "1"})
    ET.SubElement(cell_xfs, _q("xf"), base)
    cell_xfs.set("count", str(len(list(cell_xfs))))
    replacements[styles_name] = ET.tostring(root, encoding="utf-8", xml_declaration=True)
    return len(list(cell_xfs)) - 1


def _apply_sheet_operation(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    additions: dict[str, bytes],
    sheet_parts: dict[str, str],
    operation: dict[str, Any],
    style_count: int,
    changed: list[str],
) -> None:
    name = str(operation["op"]).lower()
    sheet_name = str(operation.get("sheet", ""))
    part = sheet_parts.get(sheet_name.casefold())
    if not part:
        raise XlsxEditError(f"worksheet not found: {sheet_name}")
    root = ET.fromstring(replacements.get(part, archive.read(part)))
    if name == "set_value":
        cell_ref = str(operation.get("cell", ""))
        _write_literal(_find_or_create_cell(root, cell_ref), operation.get("value"))
        changed.append(f"{sheet_name}!{cell_ref.replace('$', '').upper()}")
    elif name == "set_formula":
        cell_ref = str(operation.get("cell", ""))
        _write_formula(
            _find_or_create_cell(root, cell_ref),
            operation.get("formula"),
            operation.get("cachedValue"),
        )
        changed.append(f"{sheet_name}!{cell_ref.replace('$', '').upper()}")
    elif name == "set_range":
        range_ref = str(operation.get("range", ""))
        cells = _range_cells(range_ref)
        rows, columns = _range_shape(range_ref)
        values = operation.get("values")
        if not isinstance(values, list) or not all(isinstance(row, list) for row in values):
            raise XlsxEditError("set_range values must be a two-dimensional array")
        if len(values) != rows or any(len(row) != columns for row in values):
            actual = "x".join(
                [str(len(values)), "/".join(str(len(row)) for row in values) or "0"]
            )
            raise XlsxEditError(
                f"set_range values shape {actual} does not match range shape {rows}x{columns}"
            )
        flat_values = [value for row in values for value in row]
        for cell_ref, value in zip(cells, flat_values, strict=True):
            _write_literal(_find_or_create_cell(root, cell_ref), value)
            changed.append(f"{sheet_name}!{cell_ref}")
    elif name == "clear_range":
        for cell_ref in _range_cells(str(operation.get("range", ""))):
            if _remove_cell(root, cell_ref):
                changed.append(f"{sheet_name}!{cell_ref}")
    elif name == "set_style":
        style_id = int(operation.get("styleId", -1))
        if style_id < 0 or style_id >= style_count:
            raise XlsxEditError(
                f"styleId {style_id} is outside workbook cellXfs range 0..{style_count - 1}"
            )
        for cell_ref in _range_cells(str(operation.get("range") or operation.get("cell") or "")):
            _find_or_create_cell(root, cell_ref).set("s", str(style_id))
            changed.append(f"{sheet_name}!{cell_ref}")
    elif name == "set_data_validation":
        _set_data_validation(root, operation)
        changed.append(f"{sheet_name}!validation:{operation.get('range')}")
    elif name == "create_table":
        table_part = _create_table(
            archive, replacements, additions, part, root, operation
        )
        changed.append(f"{sheet_name}!table:{operation.get('name')}")
        changed.append(table_part)
    else:
        raise XlsxEditError(f"unsupported sheet operation: {name}")
    _update_dimension(root)
    replacements[part] = ET.tostring(root, encoding="utf-8", xml_declaration=True)


def patch_xlsx(
    source: Path,
    output: Path,
    operations: list[dict[str, Any]],
) -> dict[str, Any]:
    changed: list[str] = []
    with zipfile.ZipFile(source) as archive:
        sheet_parts = _workbook_sheet_parts(archive)
        replacements: dict[str, bytes] = {}
        additions: dict[str, bytes] = {}
        style_count = _cell_xfs_count(archive)
        for index, operation in enumerate(operations):
            name = str(operation.get("op", "")).lower()
            if name not in SUPPORTED_OPERATIONS:
                raise XlsxEditError(
                    f"unsupported XLSX operation at index {index}: {name or '<missing>'}"
                )
            if name == "rename_sheet":
                changed.extend(_rename_sheet(archive, replacements, sheet_parts, operation))
            elif name == "set_defined_name":
                changed.append(_set_defined_name(archive, replacements, operation))
            elif name == "set_chart_title":
                part = str(operation.get("chartPart", ""))
                if re.fullmatch(r"xl/charts/chart[0-9]+\.xml", part) is None or part not in archive.namelist():
                    raise XlsxEditError("chartPart must identify an existing xl/charts/chartN.xml part")
                replacements[part] = _set_chart_title(
                    replacements.get(part, archive.read(part)), str(operation.get("title", ""))
                )
                changed.append(part)
            elif name == "set_chart_data":
                changed.extend(
                    _set_chart_data(archive, replacements, sheet_parts, operation)
                )
            elif name == "set_number_format":
                style_id = _create_number_format_style(archive, replacements, operation)
                style_count = max(style_count, style_id + 1)
                _apply_sheet_operation(
                    archive,
                    replacements,
                    additions,
                    sheet_parts,
                    {
                    "op": "set_style",
                    "sheet": operation.get("sheet"),
                    "cell": operation.get("cell"),
                    "range": operation.get("range"),
                    "styleId": style_id,
                    },
                    style_count,
                    changed,
                )
            else:
                _apply_sheet_operation(
                    archive,
                    replacements,
                    additions,
                    sheet_parts,
                    operation,
                    style_count,
                    changed,
                )

        output.parent.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(output, "w") as destination:
            for info in archive.infolist():
                destination.writestr(info, replacements.get(info.filename, archive.read(info.filename)))
            for name, data in sorted(additions.items()):
                if name not in archive.namelist():
                    destination.writestr(name, data)
    return {
        "kind": "xlsxStructuredEdit",
        "operations": len(operations),
        "changedCells": sorted(set(changed)),
        "changedParts": sorted(set(replacements) | set(additions)),
        "preservation": "all non-target package parts copied byte-for-byte",
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Apply typed direct-OOXML XLSX edits")
    parser.add_argument("--path", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--spec", required=True)
    args = parser.parse_args()
    source = Path(args.path).expanduser().resolve()
    output = Path(args.out).expanduser().resolve()
    payload = json.loads(Path(args.spec).read_text(encoding="utf-8"))
    operations = payload.get("operations", payload) if isinstance(payload, dict) else payload
    if not isinstance(operations, list) or not all(isinstance(item, dict) for item in operations):
        print("operations must be an array of objects", file=sys.stderr)
        return 3
    try:
        result = patch_xlsx(source, output, operations)
    except (OSError, KeyError, zipfile.BadZipFile, ET.ParseError, XlsxEditError) as error:
        print(f"XLSX_EDIT_FAILED: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
