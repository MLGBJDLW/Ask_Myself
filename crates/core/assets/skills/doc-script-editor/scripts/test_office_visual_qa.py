from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import office_visual_qa


class OfficeVisualQaTests(unittest.TestCase):
    def test_flags_blank_and_accepts_nonblank_render(self) -> None:
        try:
            from PIL import Image, ImageDraw
        except ImportError:
            self.skipTest("Pillow is not installed")
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            blank = root / "blank.png"
            nonblank = root / "nonblank.png"
            Image.new("RGB", (640, 360), "white").save(blank)
            image = Image.new("RGB", (640, 360), "white")
            draw = ImageDraw.Draw(image)
            draw.rectangle((40, 40, 400, 240), fill=(20, 80, 160))
            image.save(nonblank)

            blank_result = office_visual_qa.analyze_rendered_images([blank])
            nonblank_result = office_visual_qa.analyze_rendered_images([nonblank])

            self.assertEqual("fail", blank_result["status"])
            self.assertEqual("pass", nonblank_result["status"])


if __name__ == "__main__":
    unittest.main()
