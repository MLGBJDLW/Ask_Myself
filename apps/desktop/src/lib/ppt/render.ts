import PptxGenJS from 'pptxgenjs';
import { resolveTheme } from './themes';
import { renderSlide } from './layouts';
import type { DeckSpec } from './types';

export async function renderDeck(spec: DeckSpec): Promise<Uint8Array> {
  const pres = new PptxGenJS();
  pres.layout = 'LAYOUT_WIDE';
  pres.title = spec.title;
  if (spec.author) pres.author = spec.author;
  pres.company = 'Nexa';

  const theme = resolveTheme(spec.theme);
  spec.slides.forEach((slide, idx) => {
    const s = pres.addSlide();
    try {
      renderSlide(s, slide, theme);
    } catch (e) {
      s.background = { color: 'FEF2F2' };
      s.addText(`Slide ${idx + 1} render error: ${(e as Error).message}`, {
        x: 0.5,
        y: 3,
        w: 12.3,
        h: 1.5,
        fontSize: 18,
        color: '991B1B',
        bold: true,
      });
    }
    const note = spec.notes_per_slide?.[idx];
    if (note) s.addNotes(note);
  });

  const out = await pres.write({ outputType: 'uint8array' });
  if (out instanceof Uint8Array) return out;
  if (out instanceof ArrayBuffer) return new Uint8Array(out);
  throw new Error('pptxgenjs returned unexpected output type');
}
