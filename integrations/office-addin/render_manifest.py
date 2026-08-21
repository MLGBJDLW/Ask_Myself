#!/usr/bin/env python3
"""Render an Office add-in manifest for one exact trusted HTTPS origin."""

from __future__ import annotations

import argparse
import ipaddress
from pathlib import Path
from urllib.parse import urlsplit
from xml.etree import ElementTree as ET


def normalize_origin(value: str) -> str:
    if not value or any(character.isspace() or ord(character) < 0x20 for character in value):
        raise ValueError("origin must not contain whitespace or control characters")
    if "\\" in value:
        raise ValueError("origin must use URL forward slashes")
    parsed = urlsplit(value)
    try:
        host = parsed.hostname
        port = parsed.port
    except ValueError as error:
        raise ValueError(f"origin contains an invalid port: {error}") from error
    if (
        parsed.scheme != "https"
        or host is None
        or parsed.username is not None
        or parsed.password is not None
        or parsed.path not in {"", "/"}
        or parsed.query
        or parsed.fragment
    ):
        raise ValueError(
            "origin must be an exact trusted HTTPS origin without credentials, path, query, or fragment"
        )
    try:
        canonical_host = ipaddress.ip_address(host).compressed
        if ":" in canonical_host:
            canonical_host = f"[{canonical_host}]"
    except ValueError:
        try:
            canonical_host = host.encode("idna").decode("ascii").lower()
        except UnicodeError as error:
            raise ValueError(f"origin contains an invalid hostname: {error}") from error
        if not canonical_host or canonical_host.startswith(".") or canonical_host.endswith("."):
            raise ValueError("origin contains an invalid hostname")
    authority = canonical_host if port in {None, 443} else f"{canonical_host}:{port}"
    return f"https://{authority}"


def render_manifest(origin: str, output: Path, *, force: bool = False) -> Path:
    normalized = normalize_origin(origin)
    template = Path(__file__).with_name("manifest.template.xml").read_text(encoding="utf-8")
    rendered = template.replace("{{ORIGIN}}", normalized)
    if "{{ORIGIN}}" in rendered:
        raise ValueError("manifest template contains an unresolved origin placeholder")
    ET.fromstring(rendered)
    output = output.expanduser().resolve()
    if output.exists() and not force:
        raise FileExistsError(f"refusing to overwrite existing manifest without --force: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(rendered, encoding="utf-8")
    return output


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--origin", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args()
    try:
        output = render_manifest(args.origin, Path(args.output), force=args.force)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
