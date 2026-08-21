from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path
from xml.etree import ElementTree as ET


ROOT = Path(__file__).resolve().parent


def load_module(name: str, filename: str):
    spec = importlib.util.spec_from_file_location(name, ROOT / filename)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"unable to load {filename}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


render_manifest = load_module("nexa_office_render_manifest", "render_manifest.py")
serve_https = load_module("nexa_office_serve_https", "serve_https.py")


class OfficeAddInDeploymentTests(unittest.TestCase):
    def test_origin_is_canonical_and_exact(self) -> None:
        self.assertEqual(
            "https://office.example.com",
            render_manifest.normalize_origin("https://OFFICE.example.com:443/"),
        )
        self.assertEqual(
            "https://localhost:3000",
            render_manifest.normalize_origin("https://localhost:3000"),
        )
        for invalid in (
            "http://office.example.com",
            "https://user@office.example.com",
            "https://office.example.com/path",
            "https://office.example.com?query=1",
            "https://office.example.com\\@evil.example",
            "https://office.example.com:invalid",
            "https://office.example.com\n",
        ):
            with self.subTest(origin=invalid):
                with self.assertRaises(ValueError):
                    render_manifest.normalize_origin(invalid)

    def test_manifest_render_is_valid_and_refuses_implicit_overwrite(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            output = Path(temporary_directory) / "manifest.xml"
            rendered = render_manifest.render_manifest(
                "https://office.example.com", output
            )
            text = rendered.read_text(encoding="utf-8")
            self.assertNotIn("{{ORIGIN}}", text)
            self.assertIn("https://office.example.com/taskpane.html", text)
            ET.fromstring(text)
            with self.assertRaises(FileExistsError):
                render_manifest.render_manifest("https://office.example.com", output)

    def test_https_server_exposes_only_runtime_assets(self) -> None:
        self.assertEqual(
            {
                "/taskpane.html",
                "/taskpane.js",
                "/support.html",
                "/icon.png",
            },
            set(serve_https.ALLOWED_ASSET_PATHS),
        )
        self.assertNotIn("/render_manifest.py", serve_https.ALLOWED_ASSET_PATHS)
        self.assertNotIn("/serve_https.py", serve_https.ALLOWED_ASSET_PATHS)

    def test_https_server_rejects_keys_inside_asset_root_before_loading(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            directory = Path(temporary_directory).resolve()
            certificate = directory / "certificate.pem"
            private_key = directory / "private-key.pem"
            certificate.write_text("not a certificate", encoding="utf-8")
            private_key.write_text("not a key", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "outside the served asset directory"):
                serve_https.serve(
                    directory, certificate, private_key, "127.0.0.1", 3000
                )


if __name__ == "__main__":
    unittest.main()
