#!/usr/bin/env python3
"""Basic DOCX comments and tracked-change lifecycle using direct WordprocessingML."""

from __future__ import annotations

import argparse
import json
import re
import zipfile
from copy import deepcopy
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from xml.etree import ElementTree as ET


W_NS = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
R_NS = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
REL_NS = "http://schemas.openxmlformats.org/package/2006/relationships"
CT_NS = "http://schemas.openxmlformats.org/package/2006/content-types"
XML_NS = "http://www.w3.org/XML/1998/namespace"
COMMENTS_REL = f"{R_NS}/comments"
COMMENTS_CONTENT_TYPE = "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml"
SUPPORTED_OPERATIONS = {"add_comment", "strip_comments", "tracked_replace", "accept_changes", "reject_changes"}


class DocxReviewError(ValueError):
    pass


def _q(local: str) -> str:
    return f"{{{W_NS}}}{local}"


def _local(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def _parent_map(root: ET.Element) -> dict[ET.Element, ET.Element]:
    return {child: parent for parent in root.iter() for child in parent}


def _run_signature(run: ET.Element) -> bytes:
    properties = next((child for child in run if _local(child.tag) == "rPr"), None)
    return ET.tostring(properties, encoding="utf-8") if properties is not None else b""


def _run_is_simple(run: ET.Element) -> bool:
    return all(_local(child.tag) in {"rPr", "t", "delText"} for child in run)


def _clone_run_with_text(run: ET.Element, text: str, *, deleted: bool = False) -> ET.Element:
    clone = ET.Element(_q("r"))
    properties = next((child for child in run if _local(child.tag) == "rPr"), None)
    if properties is not None:
        clone.append(deepcopy(properties))
    text_node = ET.SubElement(clone, _q("delText" if deleted else "t"))
    if text[:1].isspace() or text[-1:].isspace():
        text_node.set(f"{{{XML_NS}}}space", "preserve")
    text_node.text = text
    return clone


def _paragraph_match(
    root: ET.Element,
    find: str,
    occurrence: int,
) -> tuple[ET.Element, list[ET.Element], int, int]:
    matches: list[tuple[ET.Element, list[ET.Element], int, int]] = []
    parents = _parent_map(root)
    for paragraph in [element for element in root.iter() if _local(element.tag) == "p"]:
        nodes = [element for element in paragraph.iter() if _local(element.tag) == "t"]
        combined = "".join(node.text or "" for node in nodes)
        cursor = 0
        while True:
            start = combined.find(find, cursor)
            if start < 0:
                break
            end = start + len(find)
            matches.append((paragraph, nodes, start, end))
            cursor = end
    if occurrence < 1 or occurrence > len(matches):
        raise DocxReviewError(
            f"target occurrence {occurrence} is outside 1..{len(matches)}"
        )
    paragraph, nodes, start, end = matches[occurrence - 1]
    if not nodes:
        raise DocxReviewError("target paragraph has no text nodes")
    for node in nodes:
        run = parents.get(node)
        if run is None or _local(run.tag) != "r" or parents.get(run) is not paragraph:
            raise DocxReviewError("target crosses a hyperlink or unsupported nested container")
        if not _run_is_simple(run):
            raise DocxReviewError("target run contains unsupported non-text children")
    return paragraph, nodes, start, end


def _replace_match_with_elements(
    paragraph: ET.Element,
    nodes: list[ET.Element],
    start: int,
    end: int,
    middle: list[ET.Element],
) -> None:
    parents = _parent_map(paragraph)
    starts: list[int] = []
    cursor = 0
    for node in nodes:
        starts.append(cursor)
        cursor += len(node.text or "")
    start_index = max(index for index, value in enumerate(starts) if value <= start)
    end_index = max(index for index, value in enumerate(starts) if value < end)
    start_run = parents[nodes[start_index]]
    end_run = parents[nodes[end_index]]
    target_runs: list[ET.Element] = []
    for node in nodes[start_index : end_index + 1]:
        run = parents[node]
        if run not in target_runs:
            target_runs.append(run)
    if len({_run_signature(run) for run in target_runs}) > 1:
        raise DocxReviewError("target crosses incompatible run formatting")
    start_text = nodes[start_index].text or ""
    end_text = nodes[end_index].text or ""
    prefix = start_text[: start - starts[start_index]]
    suffix = end_text[end - starts[end_index] :]
    insertion_index = list(paragraph).index(start_run)
    for run in target_runs:
        paragraph.remove(run)
    additions: list[ET.Element] = []
    if prefix:
        additions.append(_clone_run_with_text(start_run, prefix))
    additions.extend(middle)
    if suffix:
        additions.append(_clone_run_with_text(end_run, suffix))
    for offset, element in enumerate(additions):
        paragraph.insert(insertion_index + offset, element)


def _next_word_id(roots: list[ET.Element]) -> str:
    identifiers = []
    for root in roots:
        for element in root.iter():
            value = element.attrib.get(f"{{{W_NS}}}id")
            if value is not None and str(value).lstrip("-").isdigit():
                identifiers.append(int(value))
    return str(max(identifiers, default=-1) + 1)


def _add_comment(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    additions: dict[str, bytes],
    operation: dict[str, Any],
) -> dict[str, Any]:
    document = ET.fromstring(replacements.get("word/document.xml", archive.read("word/document.xml")))
    comments_data = replacements.get("word/comments.xml")
    if comments_data is None and "word/comments.xml" in archive.namelist():
        comments_data = archive.read("word/comments.xml")
    comments = ET.fromstring(comments_data) if comments_data else ET.Element(_q("comments"))
    comment_id = _next_word_id([document, comments])
    find = str(operation.get("find", ""))
    if not find:
        raise DocxReviewError("add_comment requires find")
    paragraph, nodes, start, end = _paragraph_match(
        document,
        find,
        int(operation.get("occurrence", 1)),
    )
    parents = _parent_map(paragraph)
    template_run = parents[nodes[max(index for index, value in enumerate([
        sum(len(node.text or "") for node in nodes[:position]) for position in range(len(nodes))
    ]) if value <= start)]]
    marker_start = ET.Element(_q("commentRangeStart"), {_q("id"): comment_id})
    marker_end = ET.Element(_q("commentRangeEnd"), {_q("id"): comment_id})
    reference_run = ET.Element(_q("r"))
    properties = ET.SubElement(reference_run, _q("rPr"))
    ET.SubElement(properties, _q("rStyle"), {_q("val"): "CommentReference"})
    ET.SubElement(reference_run, _q("commentReference"), {_q("id"): comment_id})
    match_run = _clone_run_with_text(template_run, find)
    _replace_match_with_elements(
        paragraph,
        nodes,
        start,
        end,
        [marker_start, match_run, marker_end, reference_run],
    )
    comment = ET.SubElement(comments, _q("comment"), {
        _q("id"): comment_id,
        _q("author"): str(operation.get("author", "Nexa")),
        _q("initials"): str(operation.get("initials", "NX")),
        _q("date"): str(operation.get("date") or datetime.now(timezone.utc).isoformat()),
    })
    comment_paragraph = ET.SubElement(comment, _q("p"))
    comment_run = ET.SubElement(comment_paragraph, _q("r"))
    ET.SubElement(comment_run, _q("t")).text = str(operation.get("comment", ""))
    replacements["word/document.xml"] = ET.tostring(document, encoding="utf-8", xml_declaration=True)
    if comments_data:
        replacements["word/comments.xml"] = ET.tostring(comments, encoding="utf-8", xml_declaration=True)
    else:
        additions["word/comments.xml"] = ET.tostring(comments, encoding="utf-8", xml_declaration=True)

    rels_name = "word/_rels/document.xml.rels"
    rels = ET.fromstring(replacements.get(rels_name, archive.read(rels_name)))
    if not any(child.attrib.get("Type") == COMMENTS_REL for child in rels):
        used = {child.attrib.get("Id", "") for child in rels}
        number = 1
        while f"rId{number}" in used:
            number += 1
        ET.SubElement(rels, f"{{{REL_NS}}}Relationship", {
            "Id": f"rId{number}", "Type": COMMENTS_REL, "Target": "comments.xml",
        })
        replacements[rels_name] = ET.tostring(rels, encoding="utf-8", xml_declaration=True)
    content_types = ET.fromstring(
        replacements.get("[Content_Types].xml", archive.read("[Content_Types].xml"))
    )
    if not any(child.attrib.get("PartName") == "/word/comments.xml" for child in content_types):
        ET.SubElement(content_types, f"{{{CT_NS}}}Override", {
            "PartName": "/word/comments.xml", "ContentType": COMMENTS_CONTENT_TYPE,
        })
        replacements["[Content_Types].xml"] = ET.tostring(
            content_types, encoding="utf-8", xml_declaration=True
        )
    return {"commentId": comment_id, "anchor": find}


def _tracked_replace(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    operation: dict[str, Any],
) -> dict[str, Any]:
    document = ET.fromstring(replacements.get("word/document.xml", archive.read("word/document.xml")))
    find = str(operation.get("find", ""))
    replacement = str(operation.get("replace", ""))
    if not find:
        raise DocxReviewError("tracked_replace requires find")
    paragraph, nodes, start, end = _paragraph_match(
        document, find, int(operation.get("occurrence", 1))
    )
    parents = _parent_map(paragraph)
    template_run = parents[nodes[0]]
    revision_id = _next_word_id([document])
    common = {
        _q("id"): revision_id,
        _q("author"): str(operation.get("author", "Nexa")),
        _q("date"): str(operation.get("date") or datetime.now(timezone.utc).isoformat()),
    }
    deletion = ET.Element(_q("del"), common)
    deletion.append(_clone_run_with_text(template_run, find, deleted=True))
    insertion = ET.Element(_q("ins"), common)
    insertion.append(_clone_run_with_text(template_run, replacement))
    _replace_match_with_elements(paragraph, nodes, start, end, [deletion, insertion])
    replacements["word/document.xml"] = ET.tostring(document, encoding="utf-8", xml_declaration=True)
    return {"revisionId": revision_id, "before": find, "after": replacement}


def _resolve_revisions(data: bytes, accept: bool) -> tuple[bytes, int]:
    root = ET.fromstring(data)
    count = 0
    while True:
        parents = _parent_map(root)
        target = next(
            (element for element in root.iter() if _local(element.tag) in {"ins", "del"}),
            None,
        )
        if target is None:
            break
        parent = parents[target]
        index = list(parent).index(target)
        keep = (_local(target.tag) == "ins") == accept
        if keep:
            children = list(target)
            for child in children:
                for text_node in child.iter():
                    if _local(text_node.tag) == "delText":
                        text_node.tag = _q("t")
                parent.insert(index, child)
                index += 1
        parent.remove(target)
        count += 1
    return ET.tostring(root, encoding="utf-8", xml_declaration=True), count


def _strip_comments(
    archive: zipfile.ZipFile,
    replacements: dict[str, bytes],
    deletions: set[str],
) -> dict[str, Any]:
    removed_markers = 0
    for name in archive.namelist():
        if not name.startswith("word/") or not name.endswith(".xml") or "/comments" in name:
            continue
        root = ET.fromstring(replacements.get(name, archive.read(name)))
        parents = _parent_map(root)
        changed = False
        for element in list(root.iter()):
            if _local(element.tag) in {"commentRangeStart", "commentRangeEnd", "commentReference"}:
                parent = parents.get(element)
                if parent is not None:
                    parent.remove(element)
                    removed_markers += 1
                    changed = True
        if changed:
            replacements[name] = ET.tostring(root, encoding="utf-8", xml_declaration=True)
    comment_parts = {
        name for name in archive.namelist()
        if name.startswith("word/comments") or "/comments" in name and name.endswith(".xml")
    }
    deletions.update(comment_parts)
    rels_name = "word/_rels/document.xml.rels"
    rels = ET.fromstring(replacements.get(rels_name, archive.read(rels_name)))
    for relationship in list(rels):
        if "comments" in relationship.attrib.get("Type", ""):
            rels.remove(relationship)
    replacements[rels_name] = ET.tostring(rels, encoding="utf-8", xml_declaration=True)
    content_types = ET.fromstring(
        replacements.get("[Content_Types].xml", archive.read("[Content_Types].xml"))
    )
    for override in list(content_types):
        if "comments" in override.attrib.get("PartName", ""):
            content_types.remove(override)
    replacements["[Content_Types].xml"] = ET.tostring(
        content_types, encoding="utf-8", xml_declaration=True
    )
    return {"removedMarkers": removed_markers, "removedParts": sorted(comment_parts)}


def patch_docx_reviews(source: Path, output: Path, operations: list[dict[str, Any]]) -> dict[str, Any]:
    replacements: dict[str, bytes] = {}
    additions: dict[str, bytes] = {}
    deletions: set[str] = set()
    receipts: list[dict[str, Any]] = []
    with zipfile.ZipFile(source) as archive:
        for index, operation in enumerate(operations):
            name = str(operation.get("op", "")).lower()
            if name not in SUPPORTED_OPERATIONS:
                raise DocxReviewError(f"unsupported review operation at index {index}: {name}")
            if name == "add_comment":
                detail = _add_comment(archive, replacements, additions, operation)
            elif name == "strip_comments":
                detail = _strip_comments(archive, replacements, deletions)
            elif name == "tracked_replace":
                detail = _tracked_replace(archive, replacements, operation)
            else:
                accept = name == "accept_changes"
                changed_parts = []
                revisions = 0
                for part in archive.namelist():
                    if not part.startswith("word/") or not part.endswith(".xml"):
                        continue
                    data, count = _resolve_revisions(replacements.get(part, archive.read(part)), accept)
                    if count:
                        replacements[part] = data
                        changed_parts.append(part)
                        revisions += count
                detail = {"revisions": revisions, "changedParts": changed_parts}
            receipts.append({"op": name, "detail": detail})
        output.parent.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(output, "w") as destination:
            existing = set()
            for info in archive.infolist():
                existing.add(info.filename)
                if info.filename in deletions:
                    continue
                destination.writestr(info, replacements.get(info.filename, archive.read(info.filename)))
            for name, data in sorted(additions.items()):
                if name not in existing and name not in deletions:
                    destination.writestr(name, data)
    return {
        "kind": "docxReviewEdit",
        "operations": receipts,
        "changedParts": sorted(set(replacements) | set(additions) | deletions),
    }


def extract_comments(path: Path) -> dict[str, Any]:
    with zipfile.ZipFile(path) as archive:
        if "word/comments.xml" not in archive.namelist():
            return {"kind": "docxComments", "comments": []}
        root = ET.fromstring(archive.read("word/comments.xml"))
    comments = []
    for comment in root:
        if _local(comment.tag) != "comment":
            continue
        comments.append({
            "id": comment.attrib.get(_q("id"), ""),
            "author": comment.attrib.get(_q("author"), ""),
            "date": comment.attrib.get(_q("date"), ""),
            "text": "".join(
                element.text or "" for element in comment.iter() if _local(element.tag) == "t"
            ),
        })
    return {"kind": "docxComments", "comments": comments}


def main() -> int:
    parser = argparse.ArgumentParser(description="Apply or inspect DOCX review operations")
    parser.add_argument("--path", required=True)
    parser.add_argument("--out")
    parser.add_argument("--spec")
    parser.add_argument("--extract-comments", action="store_true")
    args = parser.parse_args()
    source = Path(args.path).resolve()
    try:
        if args.extract_comments:
            result = extract_comments(source)
        else:
            if not args.out or not args.spec:
                raise DocxReviewError("--out and --spec are required for edits")
            payload = json.loads(Path(args.spec).read_text(encoding="utf-8"))
            operations = payload.get("operations", payload) if isinstance(payload, dict) else payload
            if not isinstance(operations, list) or not all(isinstance(item, dict) for item in operations):
                raise DocxReviewError("operations must be an array of objects")
            result = patch_docx_reviews(source, Path(args.out).resolve(), operations)
    except (OSError, KeyError, zipfile.BadZipFile, ET.ParseError, json.JSONDecodeError, DocxReviewError) as error:
        print(f"DOCX_REVIEW_FAILED: {error}")
        return 1
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
