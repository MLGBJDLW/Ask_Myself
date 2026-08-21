from __future__ import annotations

import tempfile
import unittest
import zipfile
from pathlib import Path

import xlsx_structured_editor


class XlsxStructuredEditorTests(unittest.TestCase):
    def _source(self, root: Path) -> Path:
        import openpyxl
        from openpyxl.chart import BarChart, Reference

        path = root / "source.xlsx"
        workbook = openpyxl.Workbook()
        data = workbook.active
        data.title = "Summary"
        data.append(["Metric", "Value"])
        data.append(["Revenue", 100])
        data.append(["Cost", 40])
        inputs = workbook.create_sheet("Inputs")
        inputs["A1"] = "=Summary!B2"
        chart = BarChart()
        chart.add_data(Reference(data, min_col=2, min_row=1, max_row=3), titles_from_data=True)
        data.add_chart(chart, "D2")
        workbook.save(path)
        return path

    def test_object_level_sheet_name_validation_table_format_and_chart_edits(self) -> None:
        import openpyxl

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = self._source(root)
            output = root / "edited.xlsx"
            result = xlsx_structured_editor.patch_xlsx(source, output, [
                {"op": "rename_sheet", "sheet": "Summary", "newName": "Data"},
                {"op": "set_defined_name", "name": "TotalRef", "formula": "Data!$B$2"},
                {
                    "op": "set_data_validation", "sheet": "Data", "range": "B2:B3",
                    "validationType": "whole", "operator": "between",
                    "formula1": "0", "formula2": "1000", "allowBlank": False,
                },
                {"op": "create_table", "sheet": "Data", "range": "A1:B3", "name": "DataTable"},
                {"op": "set_number_format", "sheet": "Data", "range": "B2:B3", "formatCode": "0.00"},
                {"op": "set_chart_title", "chartPart": "xl/charts/chart1.xml", "title": "Updated metrics"},
            ])

            self.assertIn("xl/workbook.xml", result["changedParts"])
            self.assertIn("xl/styles.xml", result["changedParts"])
            self.assertIn("xl/tables/table1.xml", result["changedParts"])
            self.assertIn("xl/charts/chart1.xml", result["changedParts"])
            workbook = openpyxl.load_workbook(output, data_only=False)
            self.assertIn("Data", workbook.sheetnames)
            self.assertNotIn("Summary", workbook.sheetnames)
            self.assertEqual("=Data!B2", workbook["Inputs"]["A1"].value)
            self.assertEqual("0.00", workbook["Data"]["B2"].number_format)
            self.assertIn("DataTable", workbook["Data"].tables)
            self.assertEqual(1, len(workbook["Data"].data_validations.dataValidation))
            self.assertIn("TotalRef", {item.name for item in workbook.defined_names.values()})
            workbook.close()
            with zipfile.ZipFile(output) as archive:
                chart_xml = archive.read("xl/charts/chart1.xml")
                self.assertIn(b"Updated metrics", chart_xml)
                self.assertIn(b"Data!", chart_xml)
                self.assertNotIn(b"Summary!", chart_xml)

    def test_defined_names_and_sheet_names_reject_external_or_ambiguous_identifiers(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = self._source(root)
            with self.assertRaisesRegex(xlsx_structured_editor.XlsxEditError, "network-closed"):
                xlsx_structured_editor.patch_xlsx(source, root / "external.xlsx", [{
                    "op": "set_defined_name", "name": "Remote", "formula": "[other.xlsx]Sheet1!A1",
                }])
            with self.assertRaisesRegex(xlsx_structured_editor.XlsxEditError, "worksheet name"):
                xlsx_structured_editor.patch_xlsx(source, root / "invalid.xlsx", [{
                    "op": "rename_sheet", "sheet": "Summary", "newName": "bad/name",
                }])

    def test_rename_sheet_rewrites_chart_series_categories_and_headers(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = self._source(root)
            output = root / "chart-renamed.xlsx"
            result = xlsx_structured_editor.patch_xlsx(source, output, [{
                "op": "rename_sheet", "sheet": "Summary", "newName": "Source Data",
            }])
            self.assertIn("xl/charts/chart1.xml", result["changedParts"])
            with zipfile.ZipFile(output) as archive:
                chart_xml = archive.read("xl/charts/chart1.xml")
                self.assertIn(b"'Source Data'!", chart_xml)
                self.assertNotIn(b"Summary!", chart_xml)


if __name__ == "__main__":
    unittest.main()
