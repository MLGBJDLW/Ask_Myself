#!/usr/bin/env python3
"""Generate and optionally render PPTX regression samples."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path
from typing import Any


def sample_specs() -> dict[str, dict[str, Any]]:
    base_theme = "nexa-light"
    return {
        "executive_brief": {
            "theme": base_theme,
            "slides": [
                {"layout": "title", "title": "Executive Brief", "subtitle": "Decision-ready summary", "notes": "Frame the ask."},
                {"layout": "comparison", "title": "Current vs Target", "left": {"heading": "Current", "bullets": ["Manual review", "Late QA"]}, "right": {"heading": "Target", "bullets": ["Automated checks", "Fast iteration"]}, "notes": "Explain why the target state matters."},
                {"layout": "section", "title": "Recommendation", "subtitle": "Adopt the automated PPT pipeline.", "notes": "Close with the next decision."},
            ],
        },
        "data_dashboard": {
            "theme": base_theme,
            "slides": [
                {"layout": "title", "title": "Metrics Dashboard", "subtitle": "Editable native charts", "notes": "Introduce metrics."},
                {"layout": "chart", "title": "Pipeline Growth", "categories": ["Q1", "Q2", "Q3", "Q4"], "series": [{"name": "Revenue", "values": [10, 14, 18, 24]}], "chart_type": "column", "data_labels": True, "notes": "Call out the trend."},
                {"layout": "table", "title": "Operating Snapshot", "table": {"headers": ["Metric", "Value"], "rows": [["Cycle time", "3.2d"], ["Quality score", "96%"]]}, "notes": "Use table for precise values."},
            ],
        },
        "roadmap": {
            "theme": "nexa-dark",
            "slides": [
                {"layout": "title", "title": "Roadmap", "subtitle": "Milestones and owners", "notes": "Set context."},
                {"layout": "timeline", "title": "Delivery Plan", "events": [{"date": "Q1", "title": "Template profiling"}, {"date": "Q2", "title": "Visual QA"}, {"date": "Q3", "title": "Delivery pack"}], "notes": "Walk through sequence."},
            ],
        },
    }


def write_specs(out_dir: Path) -> list[Path]:
    out_dir.mkdir(parents=True, exist_ok=True)
    written: list[Path] = []
    for name, spec in sample_specs().items():
        path = out_dir / f"{name}.json"
        path.write_text(json.dumps(spec, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        written.append(path)
    return written


def run_suite(out_dir: Path, renderer: Path) -> dict[str, Any]:
    specs = write_specs(out_dir / "specs")
    decks_dir = out_dir / "decks"
    decks_dir.mkdir(parents=True, exist_ok=True)
    runs: list[dict[str, Any]] = []
    for spec_path in specs:
        deck_path = decks_dir / f"{spec_path.stem}.pptx"
        cmd = [sys.executable, str(renderer), "--path", str(deck_path.resolve()), "--spec", str(spec_path.resolve())]
        proc = subprocess.run(cmd, cwd=Path.cwd(), text=True, capture_output=True)
        runs.append({"spec": str(spec_path), "deck": str(deck_path), "returncode": proc.returncode, "stderr": proc.stderr.strip()})
    return {"status": "pass" if all(run["returncode"] == 0 for run in runs) else "fail", "runs": runs}


def main() -> int:
    parser = argparse.ArgumentParser(description="Write or run PPTX regression samples.")
    parser.add_argument("--out-dir", required=True, help="Directory for generated regression artifacts")
    parser.add_argument("--run", action="store_true", help="Render samples with pptx_renderer.py")
    parser.add_argument("--renderer", default=str(Path(__file__).with_name("pptx_renderer.py")), help="Renderer script path")
    parser.add_argument("--pretty", action="store_true", help="Pretty-print JSON output")
    args = parser.parse_args()
    out_dir = Path(args.out_dir)
    result = run_suite(out_dir, Path(args.renderer)) if args.run else {"status": "pass", "specs": [str(path) for path in write_specs(out_dir)]}
    print(json.dumps(result, ensure_ascii=False, indent=2 if args.pretty else None))
    return 0 if result["status"] == "pass" else 4


if __name__ == "__main__":
    raise SystemExit(main())
