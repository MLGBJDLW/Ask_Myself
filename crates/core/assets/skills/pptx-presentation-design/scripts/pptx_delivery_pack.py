#!/usr/bin/env python3
"""Create a delivery package for a finished PPTX deck."""

from __future__ import annotations

import argparse
import json
import shutil
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import pptx_asset_pack
import pptx_audit
import pptx_quality_gate
import pptx_visual_qa


def create_delivery_pack(
    path: Path,
    out_dir: Path,
    *,
    spec: Path | None = None,
    strict: bool = False,
) -> dict[str, Any]:
    if not path.exists():
        raise FileNotFoundError(path)
    out_dir.mkdir(parents=True, exist_ok=True)
    copied = out_dir / path.name
    shutil.copy2(path, copied)

    audit = pptx_audit.audit(path)
    gate = pptx_quality_gate.evaluate_audit(audit, strict=strict, require_notes=strict)
    visual = pptx_visual_qa.analyze_pptx(path)
    assets: dict[str, Any] = {"pptx": pptx_asset_pack.inventory_pptx_assets(path)}
    if spec:
        assets["spec"] = pptx_asset_pack.validate_spec_assets(spec, Path.cwd())

    files = {
        "pptx": str(copied),
        "audit": str(out_dir / "audit.json"),
        "quality_gate": str(out_dir / "quality_gate.json"),
        "visual_qa": str(out_dir / "visual_qa.json"),
        "assets": str(out_dir / "assets.json"),
    }
    (out_dir / "audit.json").write_text(json.dumps(audit, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    (out_dir / "quality_gate.json").write_text(json.dumps(gate, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    (out_dir / "visual_qa.json").write_text(json.dumps(visual, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    (out_dir / "assets.json").write_text(json.dumps(assets, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    manifest = {
        "created_at": datetime.now(timezone.utc).isoformat(),
        "source": str(path),
        "status": "pass" if gate["status"] == "pass" and visual["status"] == "pass" and assets.get("spec", {}).get("status", "pass") == "pass" else "fail",
        "files": files,
        "scores": {"quality_gate": gate.get("score"), "visual_failures": visual.get("failure_count")},
    }
    (out_dir / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description="Create a PPTX delivery package with QA artifacts.")
    parser.add_argument("--path", required=True, help="Path to a .pptx file")
    parser.add_argument("--out-dir", required=True, help="Output package directory")
    parser.add_argument("--spec", default=None, help="Optional renderer JSON spec")
    parser.add_argument("--strict", action="store_true", help="Use strict quality gate rules")
    parser.add_argument("--pretty", action="store_true", help="Pretty-print JSON output")
    args = parser.parse_args()
    manifest = create_delivery_pack(Path(args.path), Path(args.out_dir), spec=Path(args.spec) if args.spec else None, strict=args.strict)
    print(json.dumps(manifest, ensure_ascii=False, indent=2 if args.pretty else None))
    return 0 if manifest["status"] == "pass" else 4


if __name__ == "__main__":
    raise SystemExit(main())
