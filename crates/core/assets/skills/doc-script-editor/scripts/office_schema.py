"""Dependency-free executable subset of JSON Schema 2020-12 used by Office v2.

The Office request and validation schemas intentionally use a closed subset so
the same checked-in schema can be enforced without runtime schema downloads.
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class SchemaViolation(ValueError):
    path: str
    message: str

    def __str__(self) -> str:
        return f"{self.path}: {self.message}"


def _json_identity(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def _is_type(value: Any, expected: str) -> bool:
    return {
        "object": isinstance(value, dict),
        "array": isinstance(value, list),
        "string": isinstance(value, str),
        "integer": type(value) is int,
        "number": type(value) in {int, float},
        "boolean": type(value) is bool,
        "null": value is None,
    }.get(expected, False)


class LocalSchemaValidator:
    def __init__(self, schema_path: Path):
        self.schema_path = schema_path.resolve()
        self.documents: dict[Path, dict[str, Any]] = {}

    def validate(self, instance: Any) -> None:
        self._validate(instance, self._load(self.schema_path), "$", self.schema_path)

    def _load(self, path: Path) -> dict[str, Any]:
        path = path.resolve()
        if path not in self.documents:
            payload = json.loads(path.read_text(encoding="utf-8"))
            if not isinstance(payload, dict):
                raise SchemaViolation("$schema", f"schema must be an object: {path}")
            self.documents[path] = payload
        return self.documents[path]

    def _resolve(self, reference: str, current: Path) -> tuple[dict[str, Any], Path]:
        file_part, _, fragment = reference.partition("#")
        target_path = (current.parent / file_part).resolve() if file_part else current
        target: Any = self._load(target_path)
        if fragment:
            if not fragment.startswith("/"):
                raise SchemaViolation("$schema", f"unsupported JSON pointer: {reference}")
            for raw in fragment[1:].split("/"):
                token = raw.replace("~1", "/").replace("~0", "~")
                if not isinstance(target, dict) or token not in target:
                    raise SchemaViolation("$schema", f"unresolved JSON pointer: {reference}")
                target = target[token]
        if not isinstance(target, dict):
            raise SchemaViolation("$schema", f"schema reference is not an object: {reference}")
        return target, target_path

    def _matches(self, instance: Any, schema: dict[str, Any], path: str, current: Path) -> bool:
        try:
            self._validate(instance, schema, path, current)
            return True
        except SchemaViolation:
            return False

    def _validate(self, instance: Any, schema: dict[str, Any], path: str, current: Path) -> None:
        if "$ref" in schema:
            resolved, resolved_path = self._resolve(str(schema["$ref"]), current)
            self._validate(instance, resolved, path, resolved_path)
            return
        expected = schema.get("type")
        if isinstance(expected, str) and not _is_type(instance, expected):
            raise SchemaViolation(path, f"expected {expected}")
        if isinstance(expected, list) and not any(_is_type(instance, item) for item in expected):
            raise SchemaViolation(path, f"expected one of types {expected}")
        if "const" in schema and _json_identity(instance) != _json_identity(schema["const"]):
            raise SchemaViolation(path, f"must equal {schema['const']!r}")
        if "enum" in schema and all(
            _json_identity(instance) != _json_identity(choice) for choice in schema["enum"]
        ):
            raise SchemaViolation(path, f"must be one of {schema['enum']!r}")

        for subschema in schema.get("allOf", []):
            self._validate(instance, subschema, path, current)
        if "anyOf" in schema and not any(
            self._matches(instance, subschema, path, current) for subschema in schema["anyOf"]
        ):
            raise SchemaViolation(path, "must match at least one anyOf branch")
        if "oneOf" in schema:
            matches = sum(
                self._matches(instance, subschema, path, current) for subschema in schema["oneOf"]
            )
            if matches != 1:
                raise SchemaViolation(path, f"must match exactly one oneOf branch; matched {matches}")
        if "if" in schema and self._matches(instance, schema["if"], path, current):
            if "then" in schema:
                self._validate(instance, schema["then"], path, current)
        elif "else" in schema:
            self._validate(instance, schema["else"], path, current)

        if isinstance(instance, dict):
            missing = [key for key in schema.get("required", []) if key not in instance]
            if missing:
                raise SchemaViolation(path, f"missing required field(s): {', '.join(missing)}")
            properties = schema.get("properties", {})
            if schema.get("additionalProperties") is False:
                unknown = sorted(set(instance) - set(properties))
                if unknown:
                    raise SchemaViolation(path, f"unknown field(s): {', '.join(unknown)}")
            for key, subschema in properties.items():
                if key in instance:
                    self._validate(instance[key], subschema, f"{path}.{key}", current)
        if isinstance(instance, list):
            if len(instance) < int(schema.get("minItems", 0)):
                raise SchemaViolation(path, f"requires at least {schema['minItems']} items")
            if schema.get("uniqueItems"):
                identities = [_json_identity(item) for item in instance]
                if len(identities) != len(set(identities)):
                    raise SchemaViolation(path, "array items must be unique")
            if isinstance(schema.get("items"), dict):
                for index, item in enumerate(instance):
                    self._validate(item, schema["items"], f"{path}[{index}]", current)
        if isinstance(instance, str):
            if len(instance) < int(schema.get("minLength", 0)):
                raise SchemaViolation(path, f"requires at least {schema['minLength']} characters")
            if "pattern" in schema and re.search(str(schema["pattern"]), instance) is None:
                raise SchemaViolation(path, f"does not match pattern {schema['pattern']}")
        if type(instance) in {int, float} and "minimum" in schema and instance < schema["minimum"]:
            raise SchemaViolation(path, f"must be >= {schema['minimum']}")


def validate_schema_file(instance: Any, schema_path: Path) -> None:
    LocalSchemaValidator(schema_path).validate(instance)
