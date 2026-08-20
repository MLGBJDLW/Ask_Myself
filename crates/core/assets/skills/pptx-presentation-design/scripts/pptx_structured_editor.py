#!/usr/bin/env python3
"""Typed PPTX package edits with display-order addressing and exact slide clone."""

from __future__ import annotations

import argparse
import json
import posixpath
import re
import sys
import zipfile
from copy import deepcopy
from pathlib import Path, PurePosixPath
from typing import Any
from xml.etree import ElementTree as ET


P_NS = "http://schemas.openxmlformats.org/presentationml/2006/main"
A_NS = "http://schemas.openxmlformats.org/drawingml/2006/main"
R_NS = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
REL_NS = "http://schemas.openxmlformats.org/package/2006/relationships"
CT_NS = "http://schemas.openxmlformats.org/package/2006/content-types"
SUPPORTED_OPERATIONS = {"set_text", "clone_slide", "reorder_slides", "set_transition"}
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
            elif name == "set_text":
                detail = _set_text(archive, replacements, operation)
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
