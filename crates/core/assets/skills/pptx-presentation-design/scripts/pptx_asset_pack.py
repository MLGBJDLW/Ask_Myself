#!/usr/bin/env python3
"""Inventory PPTX media, external links, and renderer-spec assets."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import zipfile
from pathlib import Path
from typing import Any
from xml.etree import ElementTree as ET


REL_NS = {"rel": "http://schemas.openxmlformats.org/package/2006/relationships"}
URL_RE = re.compile(r"https?://[^\s)\]}>,]+")


def _parse_xml(text: str):
    if not text:
        return None
    try:
        return ET.fromstring(text)
    except ET.ParseError:
        return None


def _read_text(zf: zipfile.ZipFile, name: str) -> str:
    try:
        return zf.read(name).decode("utf-8", errors="replace")
    except KeyError:
        return ""


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _rels(zf: zipfile.ZipFile) -> list[dict[str, str]]:
    rels: list[dict[str, str]] = []
    for name in zf.namelist():
        if not name.endswith(".rels"):
            continue
        root = _parse_xml(_read_text(zf, name))
        if root is None:
            continue
        for rel in root.findall("rel:Relationship", REL_NS):
            rels.append(
                {
                    "part": name,
                    "id": rel.get("Id") or "",
                    "type": rel.get("Type") or "",
                    "target": rel.get("Target") or "",
                    "target_mode": rel.get("TargetMode") or "Internal",
                }
            )
    return rels


def inventory_pptx_assets(path: Path) -> dict[str, Any]:
    if not path.exists():
        raise FileNotFoundError(path)
    media: list[dict[str, Any]] = []
    with zipfile.ZipFile(path) as zf:
        for name in sorted(n for n in zf.namelist() if n.startswith("ppt/media/")):
            data = zf.read(name)
            media.append({"path": name, "bytes": len(data), "sha256": _sha256(data)})
        rels = _rels(zf)
    external = [rel for rel in rels if rel.get("target_mode") == "External" or URL_RE.match(rel.get("target", ""))]
    return {
        "path": str(path),
        "media": media,
        "media_count": len(media),
        "external_links": external,
        "external_link_count": len(external),
    }


def validate_spec_assets(spec_path: Path, workspace_root: Path | None = None) -> dict[str, Any]:
    workspace = (workspace_root or Path.cwd()).resolve()
    spec = json.loads(spec_path.read_text(encoding="utf-8"))
    missing: list[str] = []
    links: list[str] = []
    local_assets: list[dict[str, Any]] = []

    def visit(value: Any) -> None:
        if isinstance(value, dict):
            for key, item in value.items():
                if key in {"image", "image_path", "image_url"} and isinstance(item, str):
                    if URL_RE.match(item):
                        links.append(item)
                    else:
                        p = Path(item)
                        if not p.is_absolute():
                            p = workspace / p
                        if p.exists():
                            local_assets.append({"path": str(p), "bytes": p.stat().st_size})
                        else:
                            missing.append(str(p))
                elif key in {"links", "citations"} and isinstance(item, list):
                    for link in item:
                        if isinstance(link, str) and URL_RE.match(link):
                            links.append(link)
                visit(item)
        elif isinstance(value, list):
            for item in value:
                visit(item)

    visit(spec)
    dedup_links = list(dict.fromkeys(links))
    return {
        "spec": str(spec_path),
        "local_assets": local_assets,
        "missing_assets": missing,
        "links": dedup_links,
        "status": "fail" if missing else "pass",
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Inventory PPTX assets and renderer-spec dependencies.")
    parser.add_argument("--path", default=None, help="Path to a .pptx file")
    parser.add_argument("--spec", default=None, help="Renderer JSON spec to validate")
    parser.add_argument("--pretty", action="store_true", help="Pretty-print JSON output")
    args = parser.parse_args()
    result: dict[str, Any] = {}
    if args.path:
        result["pptx"] = inventory_pptx_assets(Path(args.path))
    if args.spec:
        result["spec"] = validate_spec_assets(Path(args.spec))
    if not result:
        parser.error("provide --path and/or --spec")
    print(json.dumps(result, ensure_ascii=False, indent=2 if args.pretty else None))
    status = result.get("spec", {}).get("status")
    return 4 if status == "fail" else 0


if __name__ == "__main__":
    raise SystemExit(main())
