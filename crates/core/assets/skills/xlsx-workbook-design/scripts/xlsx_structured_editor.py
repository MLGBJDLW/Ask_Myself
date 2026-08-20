#!/usr/bin/env python3
"""Loss-minimizing typed XLSX edits using direct worksheet OOXML patches."""

from __future__ import annotations

import argparse
import json
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
CELL_RE = re.compile(r"^\$?([A-Z]{1,3})\$?([1-9][0-9]*)$")
RANGE_RE = re.compile(r"^(\$?[A-Z]{1,3}\$?[1-9][0-9]*)(?::(\$?[A-Z]{1,3}\$?[1-9][0-9]*))?$")
SUPPORTED_OPERATIONS = {"set_value", "set_formula", "set_range", "clear_range", "set_style"}


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
        sheet_name = str(operation.get("sheet", ""))
        part = sheet_parts.get(sheet_name.casefold())
        if not part:
            raise XlsxEditError(f"worksheet not found: {sheet_name}")
        planned.setdefault(part, []).append(operation)
    return planned


def patch_xlsx(
    source: Path,
    output: Path,
    operations: list[dict[str, Any]],
) -> dict[str, Any]:
    changed: list[str] = []
    with zipfile.ZipFile(source) as archive:
        sheet_parts = _workbook_sheet_parts(archive)
        style_count = _cell_xfs_count(archive)
        plan = _plan_operations(operations, sheet_parts)
        replacements: dict[str, bytes] = {}
        for part, part_operations in plan.items():
            root = ET.fromstring(archive.read(part))
            for operation in part_operations:
                name = str(operation["op"]).lower()
                sheet_name = str(operation["sheet"])
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
                    cells = _range_cells(str(operation.get("range", "")))
                    values = operation.get("values")
                    if not isinstance(values, list) or not all(isinstance(row, list) for row in values):
                        raise XlsxEditError("set_range values must be a two-dimensional array")
                    flat_values = [value for row in values for value in row]
                    if len(flat_values) != len(cells):
                        raise XlsxEditError(
                            f"set_range value count {len(flat_values)} does not match range cells {len(cells)}"
                        )
                    for cell_ref, value in zip(cells, flat_values, strict=True):
                        _write_literal(_find_or_create_cell(root, cell_ref), value)
                        changed.append(f"{sheet_name}!{cell_ref}")
                elif name == "clear_range":
                    for cell_ref in _range_cells(str(operation.get("range", ""))):
                        if _remove_cell(root, cell_ref):
                            changed.append(f"{sheet_name}!{cell_ref}")
                else:
                    style_id = int(operation.get("styleId", -1))
                    if style_id < 0 or style_id >= style_count:
                        raise XlsxEditError(
                            f"styleId {style_id} is outside workbook cellXfs range 0..{style_count - 1}"
                        )
                    for cell_ref in _range_cells(str(operation.get("range") or operation.get("cell") or "")):
                        _find_or_create_cell(root, cell_ref).set("s", str(style_id))
                        changed.append(f"{sheet_name}!{cell_ref}")
            _update_dimension(root)
            replacements[part] = ET.tostring(root, encoding="utf-8", xml_declaration=True)

        output.parent.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(output, "w") as destination:
            for info in archive.infolist():
                destination.writestr(info, replacements.get(info.filename, archive.read(info.filename)))
    return {
        "kind": "xlsxStructuredEdit",
        "operations": len(operations),
        "changedCells": sorted(set(changed)),
        "changedParts": sorted(replacements),
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
