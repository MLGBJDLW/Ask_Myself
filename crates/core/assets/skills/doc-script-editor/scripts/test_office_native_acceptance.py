from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest import mock

import office_native_acceptance


class OfficeNativeAcceptanceTests(unittest.TestCase):
    def test_acceptance_evidence_binds_native_actions_and_render_hashes(self) -> None:
        class Validation:
            status = "pass"

            @staticmethod
            def to_dict():
                return {"status": "pass", "errors": [], "warnings": [], "checks": {}}

        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            artifact = root / "artifacts" / "native.docx"
            artifact.parent.mkdir()
            artifact.write_bytes(b"source artifact")

            def finalize(path, artifact_format, actions):
                self.assertEqual("docx", artifact_format)
                path.write_bytes(b"native artifact")
                actions.append({
                    "command": "windows-com-finalize",
                    "status": "ok",
                    "engine": "microsoft-word-com",
                    "engineVersion": "16.0",
                })

            def render(path, outdir, actions):
                self.assertEqual(artifact, path)
                outdir.mkdir(parents=True)
                image = outdir / "page-001.png"
                image.write_bytes(b"rendered page")
                actions.append({
                    "command": "windows-com-render-docx",
                    "status": "ok",
                    "engine": "microsoft-word-com",
                    "engineVersion": "16.0",
                })
                return [image]

            with mock.patch.object(
                office_native_acceptance.office_artifact_service,
                "_windows_com_finalize",
                side_effect=finalize,
            ), mock.patch.object(
                office_native_acceptance.office_artifact_service,
                "_windows_com_render_docx",
                side_effect=render,
            ), mock.patch.object(
                office_native_acceptance,
                "validate_ooxml_package",
                return_value=Validation(),
            ):
                result = office_native_acceptance.accept_artifact(
                    "docx", artifact, root, lambda path: self.assertEqual(artifact, path)
                )

            self.assertNotEqual(result["sourceSha256"], result["artifactSha256"])
            self.assertEqual(1, len(result["renderedSurfaces"]))
            self.assertRegex(result["renderedSurfaces"][0]["sha256"], r"^[0-9a-f]{64}$")
            self.assertEqual(2, len(result["actions"]))


if __name__ == "__main__":
    unittest.main()
