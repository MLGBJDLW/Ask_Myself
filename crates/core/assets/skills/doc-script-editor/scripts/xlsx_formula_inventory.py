"""Canonical, cache-independent XLSX formula inventory shared by all gates."""

from __future__ import annotations

import posixpath
import re
import zipfile
from typing import Any
from xml.etree import ElementTree as ET

REL_NS = "http://schemas.openxmlformats.org/package/2006/relationships"
DOC_REL_NS = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
MAX_FORMULA_XML_BYTES = 32 * 1024 * 1024


def _sheet_parts(archive: zipfile.ZipFile) -> list[tuple[str, str]]:
    if "xl/workbook.xml" not in archive.namelist() or "xl/_rels/workbook.xml.rels" not in archive.namelist():
        return []
    workbook = ET.fromstring(archive.read("xl/workbook.xml"))
    relationships = ET.fromstring(archive.read("xl/_rels/workbook.xml.rels"))
    targets: dict[str, str] = {}
    for relationship in relationships.findall(f"{{{REL_NS}}}Relationship"):
        if relationship.attrib.get("TargetMode", "").casefold() == "external":
            continue
        target = relationship.attrib.get("Target", "")
        if not target:
            continue
        resolved = target.lstrip("/") if target.startswith("/") else posixpath.normpath(f"xl/{target}")
        targets[relationship.attrib.get("Id", "")] = resolved
    result = []
    for sheet in workbook.iter():
        if sheet.tag.rsplit("}", 1)[-1] != "sheet":
            continue
        part = targets.get(sheet.attrib.get(f"{{{DOC_REL_NS}}}id", ""))
        if part:
            result.append((sheet.attrib.get("name", ""), part))
    return result


def _read_xml(archive: zipfile.ZipFile, part: str) -> ET.Element | None:
    try:
        info = archive.getinfo(part)
    except KeyError:
        return None
    if info.file_size > MAX_FORMULA_XML_BYTES:
        return None
    try:
        return ET.fromstring(archive.read(part))
    except ET.ParseError:
        return None


def inventory_xlsx_formulas(archive: zipfile.ZipFile) -> list[dict[str, Any]]:
    """Return every executable formula surface with stable part/location identity."""

    items: list[dict[str, Any]] = []
    for sheet_name, part in _sheet_parts(archive):
        root = _read_xml(archive, part)
        if root is None:
            continue
        cell_formulas: set[int] = set()
        for cell in root.iter():
            if cell.tag.rsplit("}", 1)[-1] != "c":
                continue
            formula = next(
                (child for child in cell if child.tag.rsplit("}", 1)[-1] == "f"),
                None,
            )
            if formula is None:
                continue
            cell_formulas.add(id(formula))
            items.append({
                "kind": "cell",
                "part": part,
                "sheet": sheet_name,
                "cell": cell.attrib.get("r", ""),
                "location": f"{sheet_name}!{cell.attrib.get('r', '')}",
                "formula": "".join(formula.itertext()),
                "type": formula.attrib.get("t", "normal"),
                "ref": formula.attrib.get("ref"),
                "sharedIndex": formula.attrib.get("si"),
            })
        surface_index = 0
        for element in root.iter():
            local = element.tag.rsplit("}", 1)[-1]
            if id(element) in cell_formulas or local not in {"formula", "formula1", "formula2"}:
                continue
            surface_index += 1
            items.append({
                "kind": {
                    "formula1": "data_validation_formula1",
                    "formula2": "data_validation_formula2",
                }.get(local, "conditional_format_formula"),
                "part": part,
                "sheet": sheet_name,
                "cell": "",
                "location": f"{sheet_name}!{local}[{surface_index}]",
                "formula": "".join(element.itertext()),
                "type": local,
                "ref": None,
                "sharedIndex": None,
            })

    workbook = _read_xml(archive, "xl/workbook.xml")
    if workbook is not None:
        for index, element in enumerate(
            (item for item in workbook.iter() if item.tag.rsplit("}", 1)[-1] == "definedName"),
            start=1,
        ):
            name = element.attrib.get("name", "")
            items.append({
                "kind": "defined_name",
                "part": "xl/workbook.xml",
                "sheet": "",
                "cell": "",
                "location": f"definedName:{name or index}",
                "name": name,
                "localSheetId": element.attrib.get("localSheetId"),
                "formula": "".join(element.itertext()),
                "type": "definedName",
                "ref": None,
                "sharedIndex": None,
            })

    formula_part_prefixes = ("xl/charts/", "xl/tables/", "xl/pivotTables/", "xl/pivotCache/")
    formula_tags = {"f", "formula", "calculatedColumnFormula", "totalsRowFormula"}
    for part in sorted(
        name
        for name in archive.namelist()
        if name.endswith(".xml") and name.startswith(formula_part_prefixes)
    ):
        root = _read_xml(archive, part)
        if root is None:
            continue
        for index, element in enumerate(
            (item for item in root.iter() if item.tag.rsplit("}", 1)[-1] in formula_tags),
            start=1,
        ):
            local = element.tag.rsplit("}", 1)[-1]
            items.append({
                "kind": "chart_series" if part.startswith("xl/charts/") else "package_formula",
                "part": part,
                "sheet": "",
                "cell": "",
                "location": f"{part}:{local}[{index}]",
                "formula": "".join(element.itertext()),
                "type": local,
                "ref": None,
                "sharedIndex": None,
            })
    return items


def dangerous_formula_functions(formula: str) -> set[str]:
    normalized = re.sub(r"\s+", "", formula).upper()
    functions = {
        match.group(1)
        for match in re.finditer(
            r"(?:_XLFN\.)?(WEBSERVICE|RTD|DDE|CALL|EXEC|REGISTER\.ID)\(",
            normalized,
        )
    }
    if "|" in formula and "!" in formula:
        functions.add("DDE_PIPE")
    return functions
