from __future__ import annotations

import unittest
from pathlib import Path

from office_schema import SchemaViolation, validate_schema_file


class OfficeSchemaTests(unittest.TestCase):
    def setUp(self) -> None:
        self.schema = (
            Path(__file__).resolve().parent.parent
            / "references"
            / "office-artifact-request-v2.schema.json"
        )

    def test_format_discriminator_executes_closed_operation_schema(self) -> None:
        valid = {
            "requestVersion": 2,
            "format": "xlsx",
            "intent": "modify",
            "source": "source.xlsx",
            "destination": "destination.xlsx",
            "preconditions": {"sourceSha256": "0" * 64},
            "operations": [{
                "op": "set_formula",
                "sheet": "Summary",
                "cell": "B2",
                "formula": "=SUM(A1:A2)",
                "cachedValue": 3,
            }],
        }
        validate_schema_file(valid, self.schema)

        invalid = {**valid, "operations": [{
            "op": "set_formula",
            "sheet": "Summary",
            "cell": "B2",
            "formula": "=SUM(A1:A2)",
            "allowStyleMerge": "false",
        }]}
        with self.assertRaisesRegex(SchemaViolation, "oneOf branch"):
            validate_schema_file(invalid, self.schema)

    def test_exact_integer_versions_and_operation_ids_are_not_coerced(self) -> None:
        request = {
            "requestVersion": 2.9,
            "format": "pptx",
            "intent": "modify",
            "source": "source.pptx",
            "destination": "destination.pptx",
            "operations": [{"op": "set_text", "slideId": {}, "shapeId": 2, "text": "x"}],
        }
        with self.assertRaises(SchemaViolation):
            validate_schema_file(request, self.schema)


if __name__ == "__main__":
    unittest.main()
