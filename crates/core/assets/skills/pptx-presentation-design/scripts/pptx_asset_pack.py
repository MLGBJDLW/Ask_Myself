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


def _catalog_ref(value: Any) -> str | None:
    if isinstance(value, str) and value.strip():
        return value.strip()
    if isinstance(value, dict):
        for key in ("path", "url", "src", "image", "image_path", "image_url", "file"):
            item = value.get(key)
            if isinstance(item, str) and item.strip():
                return item.strip()
    return None


def _normalize_image_catalog(images: Any) -> dict[str, str]:
    catalog: dict[str, str] = {}
    if isinstance(images, dict):
        for key, value in images.items():
            ref = _catalog_ref(value)
            if ref:
                catalog[str(key).strip().lstrip("@")] = ref
    elif isinstance(images, list):
        for value in images:
            if not isinstance(value, dict):
                continue
            alias = value.get("id") or value.get("name") or value.get("key") or value.get("alias")
            ref = _catalog_ref(value)
            if alias and ref:
                catalog[str(alias).strip().lstrip("@")] = ref
    return catalog


def _image_catalog_records(images: Any) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    if isinstance(images, dict):
        for key, value in images.items():
            ref = _catalog_ref(value)
            if not ref:
                continue
            role = value.get("role") if isinstance(value, dict) else None
            records.append({"alias": str(key).strip().lstrip("@"), "ref": ref, "declared_role": role})
    elif isinstance(images, list):
        for value in images:
            if not isinstance(value, dict):
                continue
            alias = value.get("id") or value.get("name") or value.get("key") or value.get("alias")
            ref = _catalog_ref(value)
            if alias and ref:
                records.append({"alias": str(alias).strip().lstrip("@"), "ref": ref, "declared_role": value.get("role")})
    return records


def _image_dimensions(path: Path) -> tuple[int, int] | None:
    try:
        from PIL import Image  # type: ignore
    except ImportError:
        return None
    try:
        with Image.open(path) as image:
            return image.size
    except Exception:
        return None


def _image_semantics(record: dict[str, Any], workspace: Path) -> dict[str, Any]:
    ref = str(record.get("ref") or "")
    result: dict[str, Any] = {
        "alias": record.get("alias"),
        "ref": ref,
        "declared_role": record.get("declared_role"),
    }
    if URL_RE.match(ref):
        result.update({"source": "remote", "orientation": "unknown", "recommended_usage": record.get("declared_role") or "remote_image"})
        return result

    path = Path(ref)
    if not path.is_absolute():
        path = workspace / path
    result["path"] = str(path)
    size = _image_dimensions(path)
    if not size:
        result.update({"source": "local", "orientation": "unknown", "recommended_usage": record.get("declared_role") or "inline_image"})
        return result

    width, height = size
    ratio = width / max(1, height)
    if ratio >= 1.25:
        orientation = "landscape"
    elif ratio <= 0.8:
        orientation = "portrait"
    else:
        orientation = "balanced"

    role = str(record.get("declared_role") or "").strip().lower()
    if role in {"background", "hero", "cover", "full_bleed", "full-bleed"}:
        usage = "background"
    elif orientation == "portrait":
        usage = "inline_portrait"
    elif orientation == "landscape":
        usage = "background"
    else:
        usage = "inline_image"

    result.update(
        {
            "source": "local",
            "width": width,
            "height": height,
            "aspect_ratio": round(ratio, 3),
            "orientation": orientation,
            "recommended_usage": usage,
            "crop_guidance": "cover center crop" if usage == "background" else "contain or side-by-side crop",
        }
    )
    return result


def _catalog_lookup(value: Any, catalog: dict[str, str]) -> Any:
    if not isinstance(value, str):
        return value
    raw = value.strip()
    key = raw[1:] if raw.startswith("@") else raw
    return catalog.get(key, value)


def _apply_image_catalog(value: Any, catalog: dict[str, str]) -> Any:
    if not catalog:
        return value
    if isinstance(value, dict):
        resolved = {
            str(key): _apply_image_catalog(item, catalog) if isinstance(item, (dict, list)) else item
            for key, item in value.items()
        }
        if "image_id" in resolved and not any(resolved.get(key) for key in ("image", "image_path", "image_url")):
            resolved["image"] = _catalog_lookup(resolved["image_id"], catalog)
        if "background_image_id" in resolved and not any(
            resolved.get(key) for key in ("background_image", "background_image_path", "background_image_url")
        ):
            resolved["background_image"] = _catalog_lookup(resolved["background_image_id"], catalog)
        for key in (
            "image",
            "image_path",
            "image_url",
            "background_image",
            "background_image_path",
            "background_image_url",
        ):
            if key in resolved:
                resolved[key] = _catalog_lookup(resolved[key], catalog)
        return resolved
    if isinstance(value, list):
        return [_apply_image_catalog(item, catalog) for item in value]
    return value


def validate_spec_assets(spec_path: Path, workspace_root: Path | None = None) -> dict[str, Any]:
    workspace = (workspace_root or Path.cwd()).resolve()
    spec = json.loads(spec_path.read_text(encoding="utf-8"))
    image_records = _image_catalog_records(spec.get("images"))
    spec = _apply_image_catalog(spec, _normalize_image_catalog(spec.get("images")))
    missing: list[str] = []
    links: list[str] = []
    local_assets: list[dict[str, Any]] = []
    seen_assets: set[str] = set()

    def visit(value: Any) -> None:
        if isinstance(value, dict):
            for key, item in value.items():
                if key in {
                    "image",
                    "image_path",
                    "image_url",
                    "background_image",
                    "background_image_path",
                    "background_image_url",
                } and isinstance(item, str):
                    if URL_RE.match(item):
                        links.append(item)
                    else:
                        p = Path(item)
                        if not p.is_absolute():
                            p = workspace / p
                        if p.exists():
                            asset_path = str(p)
                            if asset_path not in seen_assets:
                                local_assets.append({"path": asset_path, "bytes": p.stat().st_size})
                                seen_assets.add(asset_path)
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
        "image_catalog": [_image_semantics(record, workspace) for record in image_records],
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
