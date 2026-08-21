from __future__ import annotations

import tempfile
import unittest
import zipfile
from pathlib import Path
from xml.etree import ElementTree as ET

import pptx_structured_editor


class PptxStructuredEditorTests(unittest.TestCase):
    def _source(self, root: Path) -> tuple[Path, str, int]:
        from pptx import Presentation

        path = root / "source.pptx"
        presentation = Presentation()
        slide = presentation.slides.add_slide(presentation.slide_layouts[1])
        slide.shapes.title.text = "Decision"
        slide.placeholders[1].text = "Evidence"
        slide.notes_slide.notes_text_frame.text = "Original notes"
        title_shape_id = slide.shapes.title.shape_id
        presentation.save(path)
        with zipfile.ZipFile(path) as archive:
            stable_id = pptx_structured_editor.presentation_order(archive)[0]["slideId"]
        return path, stable_id, title_shape_id

    def test_notes_comments_and_alt_text_are_real_powerpoint_parts(self) -> None:
        from pptx import Presentation

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source, slide_id, shape_id = self._source(root)
            output = root / "edited.pptx"
            result = pptx_structured_editor.patch_pptx(source, output, [
                {"op": "set_alt_text", "slideId": slide_id, "shapeId": shape_id, "altText": "Decision title", "title": "Decision"},
                {"op": "set_speaker_notes", "slideId": slide_id, "text": "Updated presenter evidence"},
                {"op": "add_comment", "slideId": slide_id, "comment": "Confirm the decision owner", "author": "Reviewer", "initials": "RV"},
            ])

            self.assertIn("ppt/commentAuthors.xml", result["changedParts"])
            self.assertTrue(any(part.startswith("ppt/comments/comment") for part in result["changedParts"]))
            presentation = Presentation(output)
            self.assertEqual("Updated presenter evidence", presentation.slides[0].notes_slide.notes_text_frame.text)
            self.assertEqual(
                "Decision title",
                presentation.slides[0].shapes.title._element.nvSpPr.cNvPr.attrib["descr"],
            )
            with zipfile.ZipFile(output) as archive:
                authors = archive.read("ppt/commentAuthors.xml")
                comment_part = next(name for name in archive.namelist() if name.startswith("ppt/comments/comment"))
                comments = archive.read(comment_part)
                self.assertIn(b"Reviewer", authors)
                self.assertIn(b"Confirm the decision owner", comments)

    def test_speaker_notes_fails_closed_without_notes_relationship(self) -> None:
        from pptx import Presentation

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "without-notes.pptx"
            presentation = Presentation()
            presentation.slides.add_slide(presentation.slide_layouts[6])
            presentation.save(source)
            with zipfile.ZipFile(source) as archive:
                slide_id = pptx_structured_editor.presentation_order(archive)[0]["slideId"]
            # python-pptx may create a notes part lazily only when requested; a
            # package without it must not silently fabricate an invalid master graph.
            with zipfile.ZipFile(source) as archive:
                has_notes = any(name.startswith("ppt/notesSlides/notesSlide") for name in archive.namelist())
            if has_notes:
                self.skipTest("fixture writer eagerly created a notes relationship")
            with self.assertRaisesRegex(pptx_structured_editor.PptxEditError, "existing notes"):
                pptx_structured_editor.patch_pptx(source, root / "failed.pptx", [{
                    "op": "set_speaker_notes", "slideId": slide_id, "text": "Notes",
                }])

    def test_clone_preserves_animation_and_copy_on_write_smartart_ole_closure(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source, slide_id, _ = self._source(root)
            injected = root / "advanced-source.pptx"
            rel_ns = "http://schemas.openxmlformats.org/package/2006/relationships"
            p_ns = "http://schemas.openxmlformats.org/presentationml/2006/main"
            ct_ns = "http://schemas.openxmlformats.org/package/2006/content-types"
            additions = {
                "ppt/diagrams/data1.xml": b'<?xml version="1.0"?><dgm:dataModel xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram"/>',
                "ppt/diagrams/layout1.xml": b'<?xml version="1.0"?><dgm:layoutDef xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram"/>',
                "ppt/diagrams/colors1.xml": b'<?xml version="1.0"?><dgm:colorsDef xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram"/>',
                "ppt/diagrams/quickStyle1.xml": b'<?xml version="1.0"?><dgm:styleDef xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram"/>',
                "ppt/embeddings/oleObject1.bin": b"OLE-COPY-ON-WRITE-EVIDENCE",
            }
            content_types = {
                "ppt/diagrams/data1.xml": "application/vnd.openxmlformats-officedocument.drawingml.diagramData+xml",
                "ppt/diagrams/layout1.xml": "application/vnd.openxmlformats-officedocument.drawingml.diagramLayout+xml",
                "ppt/diagrams/colors1.xml": "application/vnd.openxmlformats-officedocument.drawingml.diagramColors+xml",
                "ppt/diagrams/quickStyle1.xml": "application/vnd.openxmlformats-officedocument.drawingml.diagramStyle+xml",
                "ppt/embeddings/oleObject1.bin": "application/vnd.openxmlformats-officedocument.oleObject",
            }
            with zipfile.ZipFile(source) as archive, zipfile.ZipFile(injected, "w") as output:
                for info in archive.infolist():
                    data = archive.read(info.filename)
                    if info.filename == "ppt/slides/slide1.xml":
                        slide = ET.fromstring(data)
                        timing = ET.SubElement(slide, f"{{{p_ns}}}timing")
                        timing.set("advTm", "2500")
                        data = ET.tostring(slide, encoding="utf-8", xml_declaration=True)
                    elif info.filename == "ppt/slides/_rels/slide1.xml.rels":
                        relationships = ET.fromstring(data)
                        ET.SubElement(relationships, f"{{{rel_ns}}}Relationship", {
                            "Id": "rId900", "Type": f"{pptx_structured_editor.R_NS}/diagramData",
                            "Target": "../diagrams/data1.xml",
                        })
                        ET.SubElement(relationships, f"{{{rel_ns}}}Relationship", {
                            "Id": "rId901", "Type": f"{pptx_structured_editor.R_NS}/oleObject",
                            "Target": "../embeddings/oleObject1.bin",
                        })
                        data = ET.tostring(relationships, encoding="utf-8", xml_declaration=True)
                    elif info.filename == "[Content_Types].xml":
                        types = ET.fromstring(data)
                        for part, content_type in content_types.items():
                            ET.SubElement(types, f"{{{ct_ns}}}Override", {
                                "PartName": f"/{part}", "ContentType": content_type,
                            })
                        data = ET.tostring(types, encoding="utf-8", xml_declaration=True)
                    output.writestr(info, data)
                diagram_relationships = ET.Element(f"{{{rel_ns}}}Relationships")
                for index, (leaf, target) in enumerate((
                    ("diagramLayout", "layout1.xml"),
                    ("diagramColors", "colors1.xml"),
                    ("diagramQuickStyle", "quickStyle1.xml"),
                ), start=1):
                    ET.SubElement(diagram_relationships, f"{{{rel_ns}}}Relationship", {
                        "Id": f"rId{index}",
                        "Type": f"{pptx_structured_editor.R_NS}/{leaf}",
                        "Target": target,
                    })
                output.writestr(
                    "ppt/diagrams/_rels/data1.xml.rels",
                    ET.tostring(diagram_relationships, encoding="utf-8", xml_declaration=True),
                )
                for name, data in additions.items():
                    output.writestr(name, data)

            output_path = root / "advanced-clone.pptx"
            result = pptx_structured_editor.patch_pptx(
                injected,
                output_path,
                [{"op": "clone_slide", "slideId": slide_id}],
            )
            mapping = result["operations"][0]["detail"]["clonedParts"]
            for original in additions:
                self.assertIn(original, mapping)
                self.assertNotEqual(original, mapping[original])
            with zipfile.ZipFile(output_path) as archive:
                self.assertIn(b"timing", archive.read("ppt/slides/slide2.xml"))
                self.assertEqual(
                    archive.read("ppt/embeddings/oleObject1.bin"),
                    archive.read(mapping["ppt/embeddings/oleObject1.bin"]),
                )
                cloned_data = mapping["ppt/diagrams/data1.xml"]
                cloned_rels = ET.fromstring(archive.read(pptx_structured_editor._rels_path(cloned_data)))
                targets = {item.attrib["Target"] for item in cloned_rels}
                self.assertNotIn("layout1.xml", targets)
                self.assertTrue(any("layout" in target for target in targets))
                self.assertEqual(
                    additions["ppt/diagrams/data1.xml"],
                    archive.read("ppt/diagrams/data1.xml"),
                )


if __name__ == "__main__":
    unittest.main()
