#!/usr/bin/env python3
"""Typed PPTX package edits with display-order addressing and exact slide clone."""

from __future__ import annotations

import argparse
import io
import json
import posixpath
import re
import sys
import zipfile
from copy import deepcopy
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath
from typing import Any
from xml.etree import ElementTree as ET


P_NS = "http://schemas.openxmlformats.org/presentationml/2006/main"
A_NS = "http://schemas.openxmlformats.org/drawingml/2006/main"
C_NS = "http://schemas.openxmlformats.org/drawingml/2006/chart"
R_NS = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
REL_NS = "http://schemas.openxmlformats.org/package/2006/relationships"
CT_NS = "http://schemas.openxmlformats.org/package/2006/content-types"
COMMENT_REL = f"{R_NS}/comments"
COMMENT_AUTHORS_REL = f"{R_NS}/commentAuthors"
COMMENT_CONTENT_TYPE = "application/vnd.openxmlformats-officedocument.presentationml.comments+xml"
COMMENT_AUTHORS_CONTENT_TYPE = "application/vnd.openxmlformats-officedocument.presentationml.commentAuthors+xml"
SUPPORTED_OPERATIONS = {
    "set_text", "clone_slide", "insert_slide", "reorder_slides", "set_transition",
    "set_alt_text", "set_speaker_notes", "set_chart_data", "add_comment",
}
DUPLICATE_RELATION_TYPES = {
    "chart", "chartUserShapes", "chartStyle", "chartColorStyle", "package", "oleObject",
    "notesSlide", "comments", "commentAuthors", "diagramData", "diagramLayout",
    "diagramColors", "diagramQuickStyle",
}


class PptxEditError(ValueError):
    pass


def _local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def _rels_path(part: str) -> str:
    parent, filename = part.rsplit("/", 1)
    return f"{parent}/_rels/{filename}.rels"


def _resolve_target(source_part: str, target: str) -> str:
    if target.startswith("/"):
        return posixpath.normpath(target.lstrip("/"))
    return posixpath.normpath(posixpath.join(posixpath.dirname(source_part), target))


def _relative_target(source_part: str, target_part: str) -> str:
    return posixpath.relpath(target_part, posixpath.dirname(source_part))


def _relationship_map(archive: zipfile.ZipFile, part: str) -> dict[str, dict[str, str]]:
    rels_name = _rels_path(part)
    if rels_name not in archive.namelist():
        return {}
    root = ET.fromstring(archive.read(rels_name))
    relationships: dict[str, dict[str, str]] = {}
    for relationship in root:
        if _local(relationship.tag) != "Relationship":
            continue
        relationships[relationship.attrib.get("Id", "")] = {
            "type": relationship.attrib.get("Type", ""),
            "target": relationship.attrib.get("Target", ""),
            "mode": relationship.attrib.get("TargetMode", ""),
        }
    return relationships


def presentation_order(archive: zipfile.ZipFile) -> list[dict[str, str]]:
    presentation = ET.fromstring(archive.read("ppt/presentation.xml"))
    relationships = _relationship_map(archive, "ppt/presentation.xml")
    order: list[dict[str, str]] = []
    for element in presentation.iter():
        if _local(element.tag) != "sldId":
            continue
        relationship_id = element.attrib.get(f"{{{R_NS}}}id", "")
        relationship = relationships.get(relationship_id)
        if not relationship:
            raise PptxEditError(f"unresolved slide relationship: {relationship_id}")
        part = _resolve_target("ppt/presentation.xml", relationship["target"])
        order.append({
            "slideId": element.attrib.get("id", ""),
            "relationshipId": relationship_id,
            "part": part,
        })
    if not order:
        raise PptxEditError("presentation has no display-order slide list")
    return order


def _target_slide(operation: dict[str, Any], order: list[dict[str, str]]) -> dict[str, str]:
    if operation.get("slideId") is not None:
        stable_id = str(operation["slideId"])
        match = next((slide for slide in order if slide["slideId"] == stable_id), None)
        if match:
            return match
        raise PptxEditError(f"slideId not found: {stable_id}")
    index = int(operation.get("slideIndex", 0))
    if index < 1 or index > len(order):
        raise PptxEditError(f"slideIndex out of range: {index}")
    return order[index - 1]


def _allocate_part(source: str, occupied: set[str]) -> str:
    path = PurePosixPath(source)
    stem = path.stem
    suffix = path.suffix
    match = re.match(r"^(.*?)(\d+)$", stem)
    base = match.group(1) if match else f"{stem}_copy"
    numbers = []
    pattern = re.compile(rf"^{re.escape(base)}(\d+){re.escape(suffix)}$")
    for name in occupied:
        candidate = PurePosixPath(name)
        if candidate.parent != path.parent:
            continue
        found = pattern.match(candidate.name)
        if found:
            numbers.append(int(found.group(1)))
    number = max(numbers, default=0) + 1
    candidate = str(path.parent / f"{base}{number}{suffix}")
    while candidate in occupied:
        number += 1
        candidate = str(path.parent / f"{base}{number}{suffix}")
    occupied.add(candidate)
    return candidate


def _should_duplicate_relationship(relationship_type: str, target_part: str) -> bool:
    leaf = relationship_type.rsplit("/", 1)[-1]
    return leaf in DUPLICATE_RELATION_TYPES or target_part.startswith((
        "ppt/charts/", "ppt/embeddings/", "ppt/diagrams/", "ppt/notesSlides/", "ppt/comments/"
    ))


def _discover_clone_closure(
    archive: zipfile.ZipFile,
    source_slide: str,
    clone_slide: str,
) -> dict[str, str]:
    occupied = set(archive.namelist())
    mapping = {source_slide: clone_slide}
    queue = [source_slide]
    while queue:
        source_part = queue.pop(0)
        for relationship in _relationship_map(archive, source_part).values():
            if relationship["mode"] == "External" or not relationship["target"]:
                continue
            target = _resolve_target(source_part, relationship["target"])
            if target in mapping:
                continue
            if _should_duplicate_relationship(relationship["type"], target):
                if target not in archive.namelist():
                    raise PptxEditError(f"clone dependency is missing: {target}")
                mapping[target] = _allocate_part(target, occupied)
                queue.append(target)
    return mapping


def _rewrite_relationship_part(
    archive: zipfile.ZipFile,
    old_part: str,
    new_part: str,
    mapping: dict[str, str],
) -> tuple[str, bytes] | None:
    old_rels = _rels_path(old_part)
    if old_rels not in archive.namelist():
        return None
    root = ET.fromstring(archive.read(old_rels))
    for relationship in root:
        if _local(relationship.tag) != "Relationship" or relationship.attrib.get("TargetMode") == "External":
            continue
        old_target = _resolve_target(old_part, relationship.attrib.get("Target", ""))
        new_target = mapping.get(old_target, old_target)
        relationship.set("Target", _relative_target(new_part, new_target))
    return _rels_path(new_part), ET.tostring(root, encoding="utf-8", xml_declaration=True)


def _clone_content_types(
    content_types: bytes,
    mapping: dict[str, str],
) -> bytes:
    root = ET.fromstring(content_types)
    overrides = {
        child.attrib.get("PartName", "").lstrip("/"): child
        for child in root
        if _local(child.tag) == "Override"
    }
    for old_part, new_part in mapping.items():
        source = overrides.get(old_part)
        if source is None:
            continue
        clone = deepcopy(source)
        clone.set("PartName", f"/{new_part}")
        root.append(clone)
    return ET.tostring(root, encoding="utf-8", xml_declaration=True)


def _next_relationship_id(root: ET.Element) -> str:
    used = {child.attrib.get("Id", "") for child in root}
    number = 1
    while f"rId{number}" in used:
        number += 1
    return f"rId{number}"


def _clone_slide(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    additions: dict[str, bytes],
    operation: dict[str, Any],
) -> dict[str, Any]:
    order = presentation_order(archive)
    source = _target_slide(operation, order)
    occupied = set(archive.namelist()) | set(additions)
    clone_part = _allocate_part(source["part"], occupied)
    mapping = _discover_clone_closure(archive, source["part"], clone_part)
    for old_part, new_part in mapping.items():
        additions[new_part] = archive.read(old_part)
        rewritten = _rewrite_relationship_part(archive, old_part, new_part, mapping)
        if rewritten is not None:
            additions[rewritten[0]] = rewritten[1]

    presentation = ET.fromstring(replacements.get("ppt/presentation.xml", archive.read("ppt/presentation.xml")))
    presentation_rels = ET.fromstring(
        replacements.get(
            "ppt/_rels/presentation.xml.rels",
            archive.read("ppt/_rels/presentation.xml.rels"),
        )
    )
    new_relationship_id = _next_relationship_id(presentation_rels)
    source_relationship = next(
        child for child in presentation_rels
        if child.attrib.get("Id") == source["relationshipId"]
    )
    new_relationship = deepcopy(source_relationship)
    new_relationship.set("Id", new_relationship_id)
    new_relationship.set("Target", _relative_target("ppt/presentation.xml", clone_part))
    presentation_rels.append(new_relationship)

    slide_list = next(element for element in presentation.iter() if _local(element.tag) == "sldIdLst")
    slide_elements = [element for element in slide_list if _local(element.tag) == "sldId"]
    new_slide_id = str(max(int(element.attrib.get("id", "255") or 255) for element in slide_elements) + 1)
    source_position = next(
        index for index, element in enumerate(slide_elements)
        if element.attrib.get(f"{{{R_NS}}}id") == source["relationshipId"]
    )
    insertion_index = int(operation.get("afterIndex", source_position + 1))
    if insertion_index < 0 or insertion_index > len(slide_elements):
        raise PptxEditError(f"afterIndex out of range: {insertion_index}")
    clone_element = deepcopy(slide_elements[source_position])
    clone_element.set("id", new_slide_id)
    clone_element.set(f"{{{R_NS}}}id", new_relationship_id)
    slide_list.insert(insertion_index, clone_element)

    replacements["ppt/presentation.xml"] = ET.tostring(presentation, encoding="utf-8", xml_declaration=True)
    replacements["ppt/_rels/presentation.xml.rels"] = ET.tostring(
        presentation_rels, encoding="utf-8", xml_declaration=True
    )
    replacements["[Content_Types].xml"] = _clone_content_types(
        replacements.get("[Content_Types].xml", archive.read("[Content_Types].xml")),
        mapping,
    )
    return {
        "sourceSlideId": source["slideId"],
        "slideId": new_slide_id,
        "part": clone_part,
        "clonedParts": mapping,
    }


def _append_text_shape(
    shape_tree: ET.Element,
    shape_id: int,
    name: str,
    text: str,
    *,
    x: int,
    y: int,
    cx: int,
    cy: int,
    font_size: int,
    bold: bool = False,
) -> None:
    shape = ET.SubElement(shape_tree, f"{{{P_NS}}}sp")
    non_visual = ET.SubElement(shape, f"{{{P_NS}}}nvSpPr")
    ET.SubElement(non_visual, f"{{{P_NS}}}cNvPr", {"id": str(shape_id), "name": name})
    ET.SubElement(non_visual, f"{{{P_NS}}}cNvSpPr", {"txBox": "1"})
    ET.SubElement(non_visual, f"{{{P_NS}}}nvPr")
    properties = ET.SubElement(shape, f"{{{P_NS}}}spPr")
    transform = ET.SubElement(properties, f"{{{A_NS}}}xfrm")
    ET.SubElement(transform, f"{{{A_NS}}}off", {"x": str(x), "y": str(y)})
    ET.SubElement(transform, f"{{{A_NS}}}ext", {"cx": str(cx), "cy": str(cy)})
    geometry = ET.SubElement(properties, f"{{{A_NS}}}prstGeom", {"prst": "rect"})
    ET.SubElement(geometry, f"{{{A_NS}}}avLst")
    ET.SubElement(properties, f"{{{A_NS}}}noFill")
    text_body = ET.SubElement(shape, f"{{{P_NS}}}txBody")
    ET.SubElement(text_body, f"{{{A_NS}}}bodyPr", {"wrap": "square"})
    ET.SubElement(text_body, f"{{{A_NS}}}lstStyle")
    paragraph = ET.SubElement(text_body, f"{{{A_NS}}}p")
    run = ET.SubElement(paragraph, f"{{{A_NS}}}r")
    run_properties = ET.SubElement(
        run,
        f"{{{A_NS}}}rPr",
        {"lang": "zh-CN", "sz": str(font_size), **({"b": "1"} if bold else {})},
    )
    run_properties.set("dirty", "0")
    ET.SubElement(run, f"{{{A_NS}}}t").text = text
    ET.SubElement(paragraph, f"{{{A_NS}}}endParaRPr", {"lang": "zh-CN", "sz": str(font_size)})


def _new_slide_xml(title: str, body: str) -> bytes:
    slide = ET.Element(f"{{{P_NS}}}sld")
    common = ET.SubElement(slide, f"{{{P_NS}}}cSld")
    shape_tree = ET.SubElement(common, f"{{{P_NS}}}spTree")
    non_visual = ET.SubElement(shape_tree, f"{{{P_NS}}}nvGrpSpPr")
    ET.SubElement(non_visual, f"{{{P_NS}}}cNvPr", {"id": "1", "name": ""})
    ET.SubElement(non_visual, f"{{{P_NS}}}cNvGrpSpPr")
    ET.SubElement(non_visual, f"{{{P_NS}}}nvPr")
    group_properties = ET.SubElement(shape_tree, f"{{{P_NS}}}grpSpPr")
    transform = ET.SubElement(group_properties, f"{{{A_NS}}}xfrm")
    for tag in ("off", "chOff"):
        ET.SubElement(transform, f"{{{A_NS}}}{tag}", {"x": "0", "y": "0"})
    for tag in ("ext", "chExt"):
        ET.SubElement(transform, f"{{{A_NS}}}{tag}", {"cx": "0", "cy": "0"})
    if title:
        _append_text_shape(
            shape_tree, 2, "Nexa title", title,
            x=640_000, y=350_000, cx=10_900_000, cy=800_000,
            font_size=2_800, bold=True,
        )
    if body:
        _append_text_shape(
            shape_tree, 3, "Nexa body", body,
            x=800_000, y=1_450_000, cx=10_500_000, cy=4_500_000,
            font_size=1_800,
        )
    color_map = ET.SubElement(slide, f"{{{P_NS}}}clrMapOvr")
    ET.SubElement(color_map, f"{{{A_NS}}}masterClrMapping")
    return ET.tostring(slide, encoding="utf-8", xml_declaration=True)


def _insert_slide(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    additions: dict[str, bytes],
    operation: dict[str, Any],
) -> dict[str, Any]:
    order = presentation_order_from_bytes(archive, replacements)
    after = int(operation.get("after", 0))
    if after < 0 or after > len(order):
        raise PptxEditError(f"after out of range: {after}")
    reference = order[max(0, min(len(order) - 1, after - 1))]
    layout_relationship = next(
        (
            relationship
            for relationship in _relationship_map(archive, reference["part"]).values()
            if relationship["type"].rsplit("/", 1)[-1] == "slideLayout"
            and relationship["mode"] != "External"
        ),
        None,
    )
    if layout_relationship is None:
        raise PptxEditError("insert_slide requires an existing internal slide layout relationship")
    layout_part = _resolve_target(reference["part"], layout_relationship["target"])
    if layout_part not in archive.namelist():
        raise PptxEditError(f"insert_slide layout is missing: {layout_part}")

    occupied = set(archive.namelist()) | set(additions)
    slide_part = _allocate_part(order[0]["part"], occupied)
    additions[slide_part] = _new_slide_xml(
        str(operation.get("title", "")),
        str(operation.get("body", "")),
    )
    slide_relationships = ET.Element(f"{{{REL_NS}}}Relationships")
    ET.SubElement(slide_relationships, f"{{{REL_NS}}}Relationship", {
        "Id": "rId1",
        "Type": f"{R_NS}/slideLayout",
        "Target": _relative_target(slide_part, layout_part),
    })
    additions[_rels_path(slide_part)] = ET.tostring(
        slide_relationships, encoding="utf-8", xml_declaration=True
    )

    presentation = ET.fromstring(
        replacements.get("ppt/presentation.xml", archive.read("ppt/presentation.xml"))
    )
    presentation_rels = ET.fromstring(
        replacements.get(
            "ppt/_rels/presentation.xml.rels",
            archive.read("ppt/_rels/presentation.xml.rels"),
        )
    )
    relationship_id = _next_relationship_id(presentation_rels)
    ET.SubElement(presentation_rels, f"{{{REL_NS}}}Relationship", {
        "Id": relationship_id,
        "Type": f"{R_NS}/slide",
        "Target": _relative_target("ppt/presentation.xml", slide_part),
    })
    slide_list = next(element for element in presentation.iter() if _local(element.tag) == "sldIdLst")
    slide_elements = [element for element in slide_list if _local(element.tag) == "sldId"]
    slide_id = str(max(int(element.attrib.get("id", "255") or 255) for element in slide_elements) + 1)
    new_slide = ET.Element(
        f"{{{P_NS}}}sldId",
        {"id": slide_id, f"{{{R_NS}}}id": relationship_id},
    )
    slide_list.insert(after, new_slide)
    replacements["ppt/presentation.xml"] = ET.tostring(
        presentation, encoding="utf-8", xml_declaration=True
    )
    replacements["ppt/_rels/presentation.xml.rels"] = ET.tostring(
        presentation_rels, encoding="utf-8", xml_declaration=True
    )
    replacements["[Content_Types].xml"] = _clone_content_types(
        replacements.get("[Content_Types].xml", archive.read("[Content_Types].xml")),
        {order[0]["part"]: slide_part},
    )
    return {"slideId": slide_id, "part": slide_part, "insertedAfter": after}


def _find_shape(root: ET.Element, operation: dict[str, Any]) -> ET.Element:
    shape_id = str(operation.get("shapeId", ""))
    shape_name = str(operation.get("shapeName", ""))
    if not shape_id and not shape_name:
        raise PptxEditError("set_text requires shapeId or shapeName")
    candidates = [element for element in root.iter() if _local(element.tag) in {"sp", "graphicFrame", "pic"}]
    matches = []
    for shape in candidates:
        properties = next((element for element in shape.iter() if _local(element.tag) == "cNvPr"), None)
        if properties is None:
            continue
        if shape_id and properties.attrib.get("id") == shape_id:
            matches.append(shape)
        elif shape_name and properties.attrib.get("name") == shape_name:
            matches.append(shape)
    if len(matches) != 1:
        raise PptxEditError(f"shape target must match exactly once; found {len(matches)}")
    return matches[0]


def _pptx_chart_targets(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    operation: dict[str, Any],
) -> dict[str, str]:
    order = presentation_order_from_bytes(archive, replacements)
    slide = _target_slide(operation, order)
    slide_root = ET.fromstring(
        replacements.get(slide["part"], archive.read(slide["part"]))
    )
    shape = _find_shape(slide_root, operation)
    chart_nodes = [item for item in shape.iter() if _local(item.tag) == "chart"]
    if len(chart_nodes) != 1:
        raise PptxEditError(
            f"target shape must contain exactly one chart reference; found {len(chart_nodes)}"
        )
    relationship_id = chart_nodes[0].attrib.get(f"{{{R_NS}}}id", "")
    relationship = _relationship_map(archive, slide["part"]).get(relationship_id)
    if (
        relationship is None
        or relationship["mode"] == "External"
        or relationship["type"].rsplit("/", 1)[-1] != "chart"
    ):
        raise PptxEditError("target chart relationship is missing, external, or invalid")
    chart_part = _resolve_target(slide["part"], relationship["target"])
    if chart_part not in archive.namelist():
        raise PptxEditError(f"target chart part is missing: {chart_part}")
    requested_chart_part = str(operation.get("chartPart", ""))
    if requested_chart_part and requested_chart_part != chart_part:
        raise PptxEditError(
            f"chartPart does not match the targeted shape: {requested_chart_part} != {chart_part}"
        )
    embedded = [
        _resolve_target(chart_part, item["target"])
        for item in _relationship_map(archive, chart_part).values()
        if item["mode"] != "External"
        and item["type"].rsplit("/", 1)[-1] == "package"
        and item["target"].lower().endswith(".xlsx")
    ]
    if len(embedded) != 1 or embedded[0] not in archive.namelist():
        raise PptxEditError(
            f"chart must have exactly one existing embedded XLSX package; found {len(embedded)}"
        )
    return {
        "slideId": slide["slideId"],
        "slidePart": slide["part"],
        "chartPart": chart_part,
        "workbookPart": embedded[0],
    }


def _set_chart_data(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    operation: dict[str, Any],
) -> dict[str, Any]:
    categories = operation.get("categories")
    values = operation.get("values")
    if not isinstance(categories, list) or not categories:
        raise PptxEditError("set_chart_data categories must be a non-empty array")
    if not isinstance(values, list) or not values:
        raise PptxEditError("set_chart_data values must be a non-empty array")
    if len(categories) != len(values):
        raise PptxEditError("set_chart_data categories and values must have the same length")
    xlsx_scripts = Path(__file__).resolve().parents[2] / "xlsx-workbook-design" / "scripts"
    if str(xlsx_scripts) not in sys.path:
        sys.path.insert(0, str(xlsx_scripts))
    from xlsx_structured_editor import (  # type: ignore
        XlsxEditError,
        _cache_text,
        _chart_series,
        _parse_chart_range,
        _reference_formula,
        _replace_chart_reference,
        _series_container,
        _workbook_sheet_parts,
        _write_chart_source_values,
    )

    try:
        for value in values:
            _cache_text(value, numeric=True)
        for category in categories:
            _cache_text(category, numeric=False)
        targets = _pptx_chart_targets(archive, replacements, operation)
        chart_root = ET.fromstring(
            replacements.get(targets["chartPart"], archive.read(targets["chartPart"]))
        )
        series = _chart_series(chart_root, int(operation.get("seriesIndex", 0)))
        workbook_data = replacements.get(
            targets["workbookPart"], archive.read(targets["workbookPart"])
        )
        workbook_input = io.BytesIO(workbook_data)
        with zipfile.ZipFile(workbook_input) as workbook_archive:
            sheet_parts = _workbook_sheet_parts(workbook_archive)
            category_container = _series_container(series, {"cat", "xVal"})
            value_container = _series_container(series, {"val", "yVal"})
            chart_targets = {
                "categories": _parse_chart_range(
                    str(operation.get("categoryRange") or _reference_formula(category_container)),
                    sheet_parts,
                ),
                "values": _parse_chart_range(
                    str(operation.get("valueRange") or _reference_formula(value_container)),
                    sheet_parts,
                ),
            }
            title = next(
                (item for item in list(series) if _local(item.tag) == "tx"),
                None,
            )
            if operation.get("seriesName") is not None and title is not None:
                try:
                    title_target = _parse_chart_range(
                        _reference_formula(title), sheet_parts
                    )
                except XlsxEditError as error:
                    if "inline or unsupported" not in str(error):
                        raise
                else:
                    if len(title_target["cells"]) != 1:
                        raise PptxEditError(
                            "chart series title reference must contain one cell"
                        )
                    chart_targets["seriesName"] = title_target
            occupied: dict[tuple[str, str], str] = {}
            for role, target in chart_targets.items():
                for cell in target["cells"]:
                    key = (str(target["part"]), cell)
                    if key in occupied:
                        raise PptxEditError(
                            f"chart source targets overlap at {target['sheet']}!{cell} "
                            f"({occupied[key]} and {role})"
                        )
                    occupied[key] = role
            workbook_replacements: dict[str, bytes] = {}
            changed_cells = _write_chart_source_values(
                workbook_archive,
                workbook_replacements,
                chart_targets["categories"],
                categories,
            )
            changed_cells.extend(
                _write_chart_source_values(
                    workbook_archive,
                    workbook_replacements,
                    chart_targets["values"],
                    values,
                )
            )
            _replace_chart_reference(
                category_container,
                str(chart_targets["categories"]["formula"]),
                categories,
                numeric=all(type(item) in {int, float} for item in categories),
            )
            _replace_chart_reference(
                value_container,
                str(chart_targets["values"]["formula"]),
                values,
                numeric=True,
            )
            if operation.get("seriesName") is not None:
                series_name = str(operation["seriesName"])
                if title is None:
                    title = ET.Element(f"{{{C_NS}}}tx")
                    insert_at = next(
                        (
                            index
                            for index, child in enumerate(list(series))
                            if _local(child.tag) not in {"idx", "order"}
                        ),
                        len(list(series)),
                    )
                    series.insert(insert_at, title)
                if "seriesName" in chart_targets:
                    changed_cells.extend(
                        _write_chart_source_values(
                            workbook_archive,
                            workbook_replacements,
                            chart_targets["seriesName"],
                            [series_name],
                        )
                    )
                    _replace_chart_reference(
                        title,
                        str(chart_targets["seriesName"]["formula"]),
                        [series_name],
                        numeric=False,
                    )
                else:
                    for child in list(title):
                        title.remove(child)
                    ET.SubElement(title, f"{{{C_NS}}}v").text = series_name
            workbook_output = io.BytesIO()
            with zipfile.ZipFile(workbook_output, "w") as output_archive:
                for info in workbook_archive.infolist():
                    output_archive.writestr(
                        info,
                        workbook_replacements.get(
                            info.filename, workbook_archive.read(info.filename)
                        ),
                    )
        replacements[targets["workbookPart"]] = workbook_output.getvalue()
        replacements[targets["chartPart"]] = ET.tostring(
            chart_root, encoding="utf-8", xml_declaration=True
        )
    except XlsxEditError as error:
        raise PptxEditError(str(error)) from error
    return {
        **targets,
        "seriesIndex": int(operation["seriesIndex"]),
        "points": len(values),
        "changedCells": changed_cells,
    }


def _set_text(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    operation: dict[str, Any],
) -> dict[str, Any]:
    order = presentation_order_from_bytes(archive, replacements)
    slide = _target_slide(operation, order)
    root = ET.fromstring(replacements.get(slide["part"], archive.read(slide["part"])))
    shape = _find_shape(root, operation)
    nodes = [element for element in shape.iter() if _local(element.tag) == "t"]
    if not nodes:
        raise PptxEditError("target shape has no text nodes")
    before = "".join(node.text or "" for node in nodes)
    nodes[0].text = str(operation.get("text", ""))
    for node in nodes[1:]:
        node.text = ""
    replacements[slide["part"]] = ET.tostring(root, encoding="utf-8", xml_declaration=True)
    return {"slideId": slide["slideId"], "part": slide["part"], "before": before, "after": str(operation.get("text", ""))}


def _set_alt_text(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    operation: dict[str, Any],
) -> dict[str, Any]:
    order = presentation_order_from_bytes(archive, replacements)
    slide = _target_slide(operation, order)
    root = ET.fromstring(replacements.get(slide["part"], archive.read(slide["part"])))
    shape = _find_shape(root, operation)
    properties = next((item for item in shape.iter() if _local(item.tag) == "cNvPr"), None)
    if properties is None:
        raise PptxEditError("target shape has no non-visual properties")
    before = properties.attrib.get("descr", "")
    properties.set("descr", str(operation.get("altText", "")))
    if operation.get("title") is not None:
        properties.set("title", str(operation["title"]))
    replacements[slide["part"]] = ET.tostring(root, encoding="utf-8", xml_declaration=True)
    return {"slideId": slide["slideId"], "part": slide["part"], "before": before, "after": str(operation.get("altText", ""))}


def _set_speaker_notes(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    operation: dict[str, Any],
) -> dict[str, Any]:
    order = presentation_order_from_bytes(archive, replacements)
    slide = _target_slide(operation, order)
    relationships = _relationship_map(archive, slide["part"])
    notes_target = next(
        (
            _resolve_target(slide["part"], relationship["target"])
            for relationship in relationships.values()
            if relationship["type"].rsplit("/", 1)[-1] == "notesSlide"
        ),
        None,
    )
    if not notes_target or notes_target not in archive.namelist():
        raise PptxEditError("set_speaker_notes requires an existing notes slide relationship")
    root = ET.fromstring(replacements.get(notes_target, archive.read(notes_target)))
    body_shape = next(
        (
            shape for shape in root.iter(f"{{{P_NS}}}sp")
            if any(
                item.attrib.get("type") == "body"
                for item in shape.iter(f"{{{P_NS}}}ph")
            )
        ),
        None,
    )
    if body_shape is None:
        raise PptxEditError("notes slide has no body placeholder")
    text_nodes = list(body_shape.iter(f"{{{A_NS}}}t"))
    if not text_nodes:
        raise PptxEditError("notes body placeholder has no text run")
    before = "".join(item.text or "" for item in text_nodes)
    text_nodes[0].text = str(operation.get("text", ""))
    for item in text_nodes[1:]:
        item.text = ""
    replacements[notes_target] = ET.tostring(root, encoding="utf-8", xml_declaration=True)
    return {"slideId": slide["slideId"], "part": notes_target, "before": before, "after": str(operation.get("text", ""))}


def _add_comment(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    additions: dict[str, bytes],
    operation: dict[str, Any],
) -> dict[str, Any]:
    order = presentation_order_from_bytes(archive, replacements)
    slide = _target_slide(operation, order)
    author_name = str(operation.get("author", "Nexa"))
    initials = str(operation.get("initials", "NX"))

    presentation_rels_name = "ppt/_rels/presentation.xml.rels"
    presentation_rels = ET.fromstring(
        replacements.get(presentation_rels_name, archive.read(presentation_rels_name))
    )
    authors_relationship = next(
        (item for item in presentation_rels if item.attrib.get("Type") == COMMENT_AUTHORS_REL),
        None,
    )
    if authors_relationship is None:
        authors_part = "ppt/commentAuthors.xml"
        authors_relationship_id = _next_relationship_id(presentation_rels)
        ET.SubElement(presentation_rels, f"{{{REL_NS}}}Relationship", {
            "Id": authors_relationship_id,
            "Type": COMMENT_AUTHORS_REL,
            "Target": "commentAuthors.xml",
        })
        authors = ET.Element(f"{{{P_NS}}}cmAuthorLst")
        additions[authors_part] = ET.tostring(authors, encoding="utf-8", xml_declaration=True)
        replacements[presentation_rels_name] = ET.tostring(
            presentation_rels, encoding="utf-8", xml_declaration=True
        )
    else:
        authors_part = _resolve_target("ppt/presentation.xml", authors_relationship.attrib["Target"])
    authors_data = additions.get(authors_part) or replacements.get(authors_part)
    if authors_data is None:
        authors_data = archive.read(authors_part)
    authors = ET.fromstring(authors_data)
    author = next(
        (item for item in authors if item.attrib.get("name", "").casefold() == author_name.casefold()),
        None,
    )
    if author is None:
        author_id = str(max([int(item.attrib.get("id", "-1")) for item in authors] or [-1]) + 1)
        author = ET.SubElement(authors, f"{{{P_NS}}}cmAuthor", {
            "id": author_id, "name": author_name, "initials": initials,
            "lastIdx": "0", "clrIdx": str(int(author_id) % 8),
        })
    author_id = author.attrib.get("id", "0")
    comment_index = int(author.attrib.get("lastIdx", "0") or 0) + 1
    author.set("lastIdx", str(comment_index))
    target_store = additions if authors_part in additions else replacements
    target_store[authors_part] = ET.tostring(authors, encoding="utf-8", xml_declaration=True)

    slide_rels_name = _rels_path(slide["part"])
    if slide_rels_name in replacements:
        slide_rels = ET.fromstring(replacements[slide_rels_name])
    elif slide_rels_name in archive.namelist():
        slide_rels = ET.fromstring(archive.read(slide_rels_name))
    else:
        slide_rels = ET.Element(f"{{{REL_NS}}}Relationships")
    comment_relationship = next(
        (item for item in slide_rels if item.attrib.get("Type") == COMMENT_REL),
        None,
    )
    if comment_relationship is None:
        occupied = set(archive.namelist()) | set(additions)
        comment_part = _allocate_part("ppt/comments/comment1.xml", occupied)
        relationship_id = _next_relationship_id(slide_rels)
        ET.SubElement(slide_rels, f"{{{REL_NS}}}Relationship", {
            "Id": relationship_id, "Type": COMMENT_REL,
            "Target": _relative_target(slide["part"], comment_part),
        })
        comments = ET.Element(f"{{{P_NS}}}cmLst")
        additions[comment_part] = ET.tostring(comments, encoding="utf-8", xml_declaration=True)
    else:
        comment_part = _resolve_target(slide["part"], comment_relationship.attrib["Target"])
    comments_data = additions.get(comment_part) or replacements.get(comment_part)
    if comments_data is None:
        comments_data = archive.read(comment_part)
    comments = ET.fromstring(comments_data)
    comment = ET.SubElement(comments, f"{{{P_NS}}}cm", {
        "authorId": author_id,
        "dt": str(operation.get("date") or datetime.now(timezone.utc).isoformat()),
        "idx": str(comment_index),
    })
    ET.SubElement(comment, f"{{{P_NS}}}pos", {
        "x": str(int(operation.get("x", 0))), "y": str(int(operation.get("y", 0))),
    })
    ET.SubElement(comment, f"{{{P_NS}}}text").text = str(operation.get("comment", ""))
    target_store = additions if comment_part in additions else replacements
    target_store[comment_part] = ET.tostring(comments, encoding="utf-8", xml_declaration=True)
    rel_store = replacements if slide_rels_name in archive.namelist() else additions
    rel_store[slide_rels_name] = ET.tostring(slide_rels, encoding="utf-8", xml_declaration=True)

    content_types = ET.fromstring(
        replacements.get("[Content_Types].xml", archive.read("[Content_Types].xml"))
    )
    for part, content_type in (
        (authors_part, COMMENT_AUTHORS_CONTENT_TYPE),
        (comment_part, COMMENT_CONTENT_TYPE),
    ):
        if not any(item.attrib.get("PartName") == f"/{part}" for item in content_types):
            ET.SubElement(content_types, f"{{{CT_NS}}}Override", {
                "PartName": f"/{part}", "ContentType": content_type,
            })
    replacements["[Content_Types].xml"] = ET.tostring(
        content_types, encoding="utf-8", xml_declaration=True
    )
    return {"slideId": slide["slideId"], "commentPart": comment_part, "authorId": author_id, "index": comment_index}


def presentation_order_from_bytes(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
) -> list[dict[str, str]]:
    presentation = ET.fromstring(replacements.get("ppt/presentation.xml", archive.read("ppt/presentation.xml")))
    rels_root = ET.fromstring(
        replacements.get("ppt/_rels/presentation.xml.rels", archive.read("ppt/_rels/presentation.xml.rels"))
    )
    rel_map = {
        child.attrib.get("Id", ""): child.attrib.get("Target", "")
        for child in rels_root
        if _local(child.tag) == "Relationship"
    }
    order = []
    for element in presentation.iter():
        if _local(element.tag) != "sldId":
            continue
        relationship_id = element.attrib.get(f"{{{R_NS}}}id", "")
        target = rel_map.get(relationship_id)
        if not target:
            raise PptxEditError(f"unresolved slide relationship: {relationship_id}")
        order.append({
            "slideId": element.attrib.get("id", ""),
            "relationshipId": relationship_id,
            "part": _resolve_target("ppt/presentation.xml", target),
        })
    return order


def _reorder_slides(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    operation: dict[str, Any],
) -> dict[str, Any]:
    requested = [str(value) for value in operation.get("order", [])]
    current = presentation_order_from_bytes(archive, replacements)
    current_ids = [slide["slideId"] for slide in current]
    if sorted(requested) != sorted(current_ids) or len(requested) != len(current_ids):
        raise PptxEditError("reorder_slides order must contain every stable slideId exactly once")
    presentation = ET.fromstring(replacements.get("ppt/presentation.xml", archive.read("ppt/presentation.xml")))
    slide_list = next(element for element in presentation.iter() if _local(element.tag) == "sldIdLst")
    by_id = {element.attrib.get("id", ""): element for element in list(slide_list) if _local(element.tag) == "sldId"}
    for element in list(slide_list):
        if _local(element.tag) == "sldId":
            slide_list.remove(element)
    for stable_id in requested:
        slide_list.append(by_id[stable_id])
    replacements["ppt/presentation.xml"] = ET.tostring(presentation, encoding="utf-8", xml_declaration=True)
    return {"before": current_ids, "after": requested}


def _set_transition(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    operation: dict[str, Any],
) -> dict[str, Any]:
    order = presentation_order_from_bytes(archive, replacements)
    slide = _target_slide(operation, order)
    root = ET.fromstring(replacements.get(slide["part"], archive.read(slide["part"])))
    for child in list(root):
        if _local(child.tag) == "transition":
            root.remove(child)
    transition_type = str(operation.get("transition", "fade"))
    if transition_type not in {"fade", "push", "wipe", "split", "cut"}:
        raise PptxEditError(f"unsupported transition: {transition_type}")
    transition = ET.Element(f"{{{P_NS}}}transition")
    speed = operation.get("speed")
    if speed in {"slow", "med", "fast"}:
        transition.set("spd", str(speed))
    child = ET.SubElement(transition, f"{{{P_NS}}}{transition_type}")
    if operation.get("direction"):
        child.set("dir", str(operation["direction"]))
    insert_at = next(
        (index for index, child_element in enumerate(list(root)) if _local(child_element.tag) in {"timing", "extLst"}),
        len(list(root)),
    )
    root.insert(insert_at, transition)
    replacements[slide["part"]] = ET.tostring(root, encoding="utf-8", xml_declaration=True)
    return {"slideId": slide["slideId"], "transition": transition_type}


def patch_pptx(source: Path, output: Path, operations: list[dict[str, Any]]) -> dict[str, Any]:
    replacements: dict[str, bytes] = {}
    additions: dict[str, bytes] = {}
    receipts: list[dict[str, Any]] = []
    with zipfile.ZipFile(source) as archive:
        for index, operation in enumerate(operations):
            name = str(operation.get("op", "")).lower()
            if name not in SUPPORTED_OPERATIONS:
                raise PptxEditError(f"unsupported PPTX operation at index {index}: {name or '<missing>'}")
            if name == "clone_slide":
                detail = _clone_slide(archive, replacements, additions, operation)
            elif name == "insert_slide":
                detail = _insert_slide(archive, replacements, additions, operation)
            elif name == "set_text":
                detail = _set_text(archive, replacements, operation)
            elif name == "set_alt_text":
                detail = _set_alt_text(archive, replacements, operation)
            elif name == "set_speaker_notes":
                detail = _set_speaker_notes(archive, replacements, operation)
            elif name == "set_chart_data":
                detail = _set_chart_data(archive, replacements, operation)
            elif name == "add_comment":
                detail = _add_comment(archive, replacements, additions, operation)
            elif name == "reorder_slides":
                detail = _reorder_slides(archive, replacements, operation)
            else:
                detail = _set_transition(archive, replacements, operation)
            receipts.append({"op": name, "detail": detail})
        output.parent.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(output, "w") as destination:
            existing = set()
            for info in archive.infolist():
                existing.add(info.filename)
                destination.writestr(info, replacements.get(info.filename, archive.read(info.filename)))
            for name, data in sorted(additions.items()):
                if name not in existing:
                    destination.writestr(name, data)
    return {
        "kind": "pptxStructuredEdit",
        "operations": receipts,
        "changedParts": sorted(set(replacements) | set(additions)),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Apply typed PPTX package edits")
    parser.add_argument("--path", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--spec", required=True)
    args = parser.parse_args()
    payload = json.loads(Path(args.spec).read_text(encoding="utf-8"))
    operations = payload.get("operations", payload) if isinstance(payload, dict) else payload
    if not isinstance(operations, list) or not all(isinstance(item, dict) for item in operations):
        print("operations must be an array of objects", file=sys.stderr)
        return 3
    try:
        result = patch_pptx(Path(args.path).resolve(), Path(args.out).resolve(), operations)
    except (OSError, KeyError, zipfile.BadZipFile, ET.ParseError, PptxEditError) as error:
        print(f"PPTX_EDIT_FAILED: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
