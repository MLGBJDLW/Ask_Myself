#!/usr/bin/env python3
"""Small deterministic QA layer for rendered Office page/slide images."""

from __future__ import annotations

import hashlib
from pathlib import Path
from typing import Any


def analyze_rendered_images(paths: list[Path]) -> dict[str, Any]:
    try:
        from PIL import Image, ImageStat  # type: ignore
    except ImportError:
        return {
            "kind": "officeVisualQa",
            "status": "unavailable",
            "failures": ["Pillow is unavailable"],
            "images": [],
        }
    failures: list[str] = []
    warnings: list[str] = []
    images: list[dict[str, Any]] = []
    hashes: dict[str, list[str]] = {}
    for path in paths:
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        hashes.setdefault(digest, []).append(str(path))
        try:
            with Image.open(path) as image:
                image.load()
                width, height = image.size
                sample = image.convert("L")
                sample.thumbnail((160, 160))
                stats = ImageStat.Stat(sample)
                mean = float(stats.mean[0])
                deviation = float(stats.stddev[0])
                blank = mean >= 248.0 and deviation <= 1.5
                too_small = width < 320 or height < 180
                if blank:
                    failures.append(f"render is nearly blank: {path}")
                if too_small:
                    failures.append(f"render dimensions are too small: {path} ({width}x{height})")
                images.append({
                    "path": str(path),
                    "sha256": digest,
                    "width": width,
                    "height": height,
                    "grayscaleMean": round(mean, 3),
                    "grayscaleStdDev": round(deviation, 3),
                    "nearlyBlank": blank,
                    "tooSmall": too_small,
                })
        except Exception as error:  # noqa: BLE001
            failures.append(f"render cannot be decoded: {path}: {type(error).__name__}: {error}")
    duplicates = [members for members in hashes.values() if len(members) > 1]
    if duplicates:
        warnings.append("multiple rendered surfaces are byte-identical")
    return {
        "kind": "officeVisualQa",
        "status": "fail" if failures else "warn" if warnings else "pass",
        "failures": failures,
        "warnings": warnings,
        "duplicateGroups": duplicates,
        "images": images,
    }
