#!/usr/bin/env python3
"""Tests for the XLSX model renderer."""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

import xlsx_model_renderer


class XlsxModelRendererTests(unittest.TestCase):
    def test_create_xlsx_from_spec_writes_tables_formulas_and_qa(self) -> None:
        try:
            import openpyxl  # type: ignore
        except ImportError:
            self.skipTest("openpyxl is not installed")

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            spec_path = root / "model_spec.json"
            out_path = root / "model.xlsx"
            spec = {
                "title": "Revenue Model",
                "active_sheet": "Summary",
                "qa": {"min_formulas": 4},
                "sheets": [
                    {
                        "name": "Summary",
                        "title": "Revenue Model",
                        "start_cell": "B4",
                        "headers": ["Metric", "2024", "2025", "2026"],
                        "rows": [
                            ["Revenue", 100, 120, 144],
                            ["Gross Margin", 0.42, 0.44, 0.45],
                            ["Gross Profit", None, None, None],
                        ],
                        "table": {"name": "SummaryTable"},
                        "formulas": [
                            {
                                "cell": "C6",
                                "formula": "=C4*C5",
                                "fill_down": {"to_cell": "E6"},
                                "number_format": "$#,##0",
                            },
                            {
                                "cell": "B8",
                                "formula": "=SUM(SummaryTable[2024])",
                                "number_format": "$#,##0",
                            },
                        ],
                        "named_ranges": [{"name": "BaseRevenue", "cell": "C4"}],
                        "validations": [
                            {
                                "range": "C5:E5",
                                "type": "decimal",
                                "operator": "between",
                                "formula1": "0",
                                "formula2": "1",
                            }
                        ],
                        "conditional_formats": [
                            {
                                "range": "C6:E6",
                                "type": "cellIs",
                                "operator": "lessThan",
                                "formula": ["0"],
                                "fill": "FCE4D6",
                            }
                        ],
                        "column_widths": {"B": 20, "C": 14, "D": 14, "E": 14},
                    }
                ],
            }
            spec_path.write_text(json.dumps(spec), encoding="utf-8")

            result = xlsx_model_renderer.create_xlsx_from_spec(
                out_path,
                spec_path,
                workspace_root=root,
            )

            self.assertEqual("pass", result["qa"]["status"])
            self.assertEqual(4, result["qa"]["formulas"])
            self.assertTrue(out_path.exists())
            self.assertTrue(out_path.with_suffix(".xlsx.qa.json").exists())

            wb = openpyxl.load_workbook(out_path, data_only=False)
            try:
                ws = wb["Summary"]
                self.assertEqual("=C4*C5", ws["C6"].value)
                self.assertEqual("=D4*D5", ws["D6"].value)
                self.assertEqual("=E4*E5", ws["E6"].value)
                self.assertEqual("=SUM(SummaryTable[2024])", ws["B8"].value)
                self.assertIn("SummaryTable", ws.tables)
                self.assertIn("BaseRevenue", wb.defined_names)
                self.assertTrue(wb.calculation.fullCalcOnLoad)
            finally:
                wb.close()

    def test_formula_lint_flags_missing_sheet_and_ref_error(self) -> None:
        try:
            import openpyxl  # type: ignore
        except ImportError:
            self.skipTest("openpyxl is not installed")

        wb = openpyxl.Workbook()
        ws = wb.active
        ws.title = "Model"
        ws["A1"] = "=Missing!A1+#REF!"

        qa = xlsx_model_renderer.audit_workbook_formulas(wb)
        self.assertEqual("fail", qa["status"])
        messages = " ".join(issue["message"] for issue in qa["issues"])
        self.assertIn("missing sheet reference", messages)
        self.assertIn("#REF!", messages)

    def test_rows_keep_leading_equals_literal_and_formula_specs_explicit(self) -> None:
        try:
            import openpyxl  # type: ignore
        except ImportError:
            self.skipTest("openpyxl is not installed")

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            spec_path = root / "literal_formula_spec.json"
            out_path = root / "literal_formula.xlsx"
            spec_path.write_text(
                json.dumps({
                    "sheets": [{
                        "name": "Inputs",
                        "headers": ["Untrusted text", "Explicit formula"],
                        "rows": [["=WEBSERVICE(\"https://example.invalid/\")", None]],
                        "formulas": [{"cell": "B2", "formula": "=1+1"}],
                    }],
                }),
                encoding="utf-8",
            )

            xlsx_model_renderer.create_xlsx_from_spec(out_path, spec_path, workspace_root=root)
            wb = openpyxl.load_workbook(out_path, data_only=False)
            try:
                ws = wb["Inputs"]
                self.assertEqual("=WEBSERVICE(\"https://example.invalid/\")", ws["A2"].value)
                self.assertEqual("s", ws["A2"].data_type)
                self.assertEqual("=1+1", ws["B2"].value)
                self.assertEqual("f", ws["B2"].data_type)
            finally:
                wb.close()


if __name__ == "__main__":
    unittest.main()
