import type pptxgen from 'pptxgenjs';
import type {
  Slide,
  ThemeTokens,
  TitleSlide,
  AgendaSlide,
  BodySlide,
  TwoColumnSlide,
  StatSlide,
  QuoteSlide,
  SectionSlide,
  ImageFullSlide,
  ColumnContent,
} from './types';

export const LAYOUT_W = 13.333;
export const LAYOUT_H = 7.5;
export const MARGIN = 0.5;

function addTitleBar(s: pptxgen.Slide, title: string, theme: ThemeTokens): void {
  s.addText(title, {
    x: MARGIN,
    y: 0.35,
    w: LAYOUT_W - MARGIN * 2,
    h: 0.8,
    fontFace: theme.title_font,
    fontSize: 32,
    bold: true,
    color: theme.title_color,
    align: 'left',
    valign: 'middle',
  });
  s.addShape('rect', {
    x: MARGIN,
    y: 1.15,
    w: 1.2,
    h: 0.06,
    fill: { color: theme.primary_color },
    line: { color: theme.primary_color, width: 0 },
  });
}

function setBackground(s: pptxgen.Slide, theme: ThemeTokens): void {
  s.background = { color: theme.background_color };
}

export function renderTitle(s: pptxgen.Slide, slide: TitleSlide, theme: ThemeTokens): void {
  setBackground(s, theme);
  if (slide.image_url) {
    s.addImage({
      path: slide.image_url,
      x: 0,
      y: 0,
      w: LAYOUT_W,
      h: LAYOUT_H,
      sizing: { type: 'cover', w: LAYOUT_W, h: LAYOUT_H },
    });
    s.addShape('rect', {
      x: 0,
      y: 0,
      w: LAYOUT_W,
      h: LAYOUT_H,
      fill: { color: '000000', transparency: 50 },
      line: { type: 'none' },
    });
  }
  const titleColor = slide.image_url ? 'FFFFFF' : theme.title_color;
  s.addText(slide.title, {
    x: MARGIN,
    y: LAYOUT_H / 2 - 1.2,
    w: LAYOUT_W - MARGIN * 2,
    h: 1.6,
    fontFace: theme.title_font,
    fontSize: 54,
    bold: true,
    color: titleColor,
    align: 'left',
    valign: 'bottom',
  });
  s.addShape('rect', {
    x: MARGIN,
    y: LAYOUT_H / 2 + 0.4,
    w: 1.8,
    h: 0.1,
    fill: { color: theme.primary_color },
    line: { type: 'none' },
  });
  if (slide.subtitle) {
    s.addText(slide.subtitle, {
      x: MARGIN,
      y: LAYOUT_H / 2 + 0.6,
      w: LAYOUT_W - MARGIN * 2,
      h: 0.8,
      fontFace: theme.body_font,
      fontSize: 24,
      color: titleColor,
      align: 'left',
    });
  }
  if (slide.author) {
    s.addText(slide.author, {
      x: MARGIN,
      y: LAYOUT_H - 0.9,
      w: LAYOUT_W - MARGIN * 2,
      h: 0.5,
      fontFace: theme.body_font,
      fontSize: 14,
      color: titleColor,
      align: 'left',
      italic: true,
    });
  }
}

export function renderAgenda(s: pptxgen.Slide, slide: AgendaSlide, theme: ThemeTokens): void {
  setBackground(s, theme);
  addTitleBar(s, slide.title ?? 'Agenda', theme);
  const startY = 1.7;
  const itemHeight = Math.min(0.9, (LAYOUT_H - startY - MARGIN) / Math.max(slide.items.length, 1));
  slide.items.forEach((item, i) => {
    const y = startY + i * itemHeight;
    s.addText(String(i + 1).padStart(2, '0'), {
      x: MARGIN,
      y,
      w: 1,
      h: itemHeight,
      fontFace: theme.title_font,
      fontSize: 36,
      bold: true,
      color: theme.primary_color,
      align: 'left',
      valign: 'middle',
    });
    s.addText(item, {
      x: MARGIN + 1.1,
      y,
      w: LAYOUT_W - MARGIN * 2 - 1.1,
      h: itemHeight,
      fontFace: theme.body_font,
      fontSize: 20,
      color: theme.text_color,
      align: 'left',
      valign: 'middle',
    });
  });
}

export function renderBody(s: pptxgen.Slide, slide: BodySlide, theme: ThemeTokens): void {
  setBackground(s, theme);
  addTitleBar(s, slide.title, theme);
  const contentY = 1.7;
  const contentH = LAYOUT_H - contentY - MARGIN;
  const hasImage = Boolean(slide.image_url);
  const textW = hasImage ? (LAYOUT_W - MARGIN * 2) * 0.55 : LAYOUT_W - MARGIN * 2;
  const imageX = MARGIN + textW + 0.3;
  const imageW = (LAYOUT_W - MARGIN * 2) * 0.4;

  if (slide.bullets && slide.bullets.length > 0) {
    const items = slide.bullets.map((b) => ({
      text: b,
      options: { bullet: { code: '25A0' }, color: theme.text_color, paraSpaceAfter: 8 },
    }));
    s.addText(items, {
      x: MARGIN,
      y: contentY,
      w: textW,
      h: contentH,
      fontFace: theme.body_font,
      fontSize: 18,
      color: theme.text_color,
      valign: 'top',
    });
  } else if (slide.paragraph) {
    s.addText(slide.paragraph, {
      x: MARGIN,
      y: contentY,
      w: textW,
      h: contentH,
      fontFace: theme.body_font,
      fontSize: 18,
      color: theme.text_color,
      align: 'left',
      valign: 'top',
      paraSpaceAfter: 8,
    });
  }
  if (hasImage && slide.image_url) {
    s.addImage({
      path: slide.image_url,
      x: imageX,
      y: contentY,
      w: imageW,
      h: contentH,
      sizing: { type: 'contain', w: imageW, h: contentH },
    });
    if (slide.image_caption) {
      s.addText(slide.image_caption, {
        x: imageX,
        y: contentY + contentH - 0.4,
        w: imageW,
        h: 0.35,
        fontFace: theme.body_font,
        fontSize: 10,
        italic: true,
        color: theme.text_color,
        align: 'center',
      });
    }
  }
}

export function renderTwoColumn(
  s: pptxgen.Slide,
  slide: TwoColumnSlide,
  theme: ThemeTokens,
): void {
  setBackground(s, theme);
  addTitleBar(s, slide.title, theme);
  const contentY = 1.7;
  const contentH = LAYOUT_H - contentY - MARGIN;
  const colW = (LAYOUT_W - MARGIN * 2 - 0.4) / 2;
  const renderCol = (col: ColumnContent, x: number): void => {
    let y = contentY;
    if (col.heading) {
      s.addText(col.heading, {
        x,
        y,
        w: colW,
        h: 0.5,
        fontFace: theme.title_font,
        fontSize: 18,
        bold: true,
        color: theme.primary_color,
        align: 'left',
      });
      y += 0.55;
    }
    const remaining = contentY + contentH - y;
    if (col.image_url) {
      s.addImage({
        path: col.image_url,
        x,
        y,
        w: colW,
        h: remaining,
        sizing: { type: 'contain', w: colW, h: remaining },
      });
    } else if (col.bullets && col.bullets.length > 0) {
      s.addText(
        col.bullets.map((b) => ({
          text: b,
          options: { bullet: { code: '25A0' }, color: theme.text_color, paraSpaceAfter: 6 },
        })),
        {
          x,
          y,
          w: colW,
          h: remaining,
          fontFace: theme.body_font,
          fontSize: 16,
          color: theme.text_color,
          valign: 'top',
        },
      );
    } else if (col.paragraph) {
      s.addText(col.paragraph, {
        x,
        y,
        w: colW,
        h: remaining,
        fontFace: theme.body_font,
        fontSize: 16,
        color: theme.text_color,
        valign: 'top',
      });
    }
  };
  renderCol(slide.left, MARGIN);
  renderCol(slide.right, MARGIN + colW + 0.4);
}

export function renderStat(s: pptxgen.Slide, slide: StatSlide, theme: ThemeTokens): void {
  setBackground(s, theme);
  if (slide.title) addTitleBar(s, slide.title, theme);
  const topOffset = slide.title ? 1.8 : 0;
  const availH = LAYOUT_H - topOffset - MARGIN;
  const n = Math.min(slide.stats.length, 4);
  const colW = (LAYOUT_W - MARGIN * 2 - (n - 1) * 0.3) / n;
  slide.stats.slice(0, n).forEach((stat, i) => {
    const x = MARGIN + i * (colW + 0.3);
    const y = topOffset + availH * 0.2;
    s.addText(stat.value, {
      x,
      y,
      w: colW,
      h: 1.8,
      fontFace: theme.title_font,
      fontSize: 64,
      bold: true,
      color: theme.primary_color,
      align: 'center',
      valign: 'bottom',
    });
    s.addShape('rect', {
      x: x + colW / 2 - 0.3,
      y: y + 1.85,
      w: 0.6,
      h: 0.05,
      fill: { color: theme.accent_color },
      line: { type: 'none' },
    });
    s.addText(stat.label, {
      x,
      y: y + 2.0,
      w: colW,
      h: 0.6,
      fontFace: theme.body_font,
      fontSize: 18,
      bold: true,
      color: theme.text_color,
      align: 'center',
    });
    if (stat.caption) {
      s.addText(stat.caption, {
        x,
        y: y + 2.6,
        w: colW,
        h: 0.8,
        fontFace: theme.body_font,
        fontSize: 12,
        color: theme.text_color,
        align: 'center',
      });
    }
  });
}

export function renderQuote(s: pptxgen.Slide, slide: QuoteSlide, theme: ThemeTokens): void {
  setBackground(s, theme);
  s.addText('\u201C', {
    x: MARGIN,
    y: 0.6,
    w: 2,
    h: 2,
    fontFace: theme.title_font,
    fontSize: 180,
    bold: true,
    color: theme.primary_color,
    align: 'left',
  });
  s.addText(slide.text, {
    x: MARGIN + 0.2,
    y: LAYOUT_H / 2 - 1.2,
    w: LAYOUT_W - MARGIN * 2 - 0.4,
    h: 2.4,
    fontFace: theme.title_font,
    fontSize: 32,
    italic: true,
    color: theme.text_color,
    align: 'left',
    valign: 'middle',
  });
  if (slide.attribution) {
    s.addText(`\u2014 ${slide.attribution}`, {
      x: MARGIN + 0.2,
      y: LAYOUT_H - 1.5,
      w: LAYOUT_W - MARGIN * 2 - 0.4,
      h: 0.6,
      fontFace: theme.body_font,
      fontSize: 18,
      color: theme.accent_color,
      align: 'left',
    });
  }
}

export function renderSection(s: pptxgen.Slide, slide: SectionSlide, theme: ThemeTokens): void {
  s.background = { color: theme.primary_color };
  s.addText(slide.title, {
    x: MARGIN,
    y: LAYOUT_H / 2 - 1,
    w: LAYOUT_W - MARGIN * 2,
    h: 1.6,
    fontFace: theme.title_font,
    fontSize: 60,
    bold: true,
    color: 'FFFFFF',
    align: 'center',
    valign: 'bottom',
  });
  s.addShape('rect', {
    x: LAYOUT_W / 2 - 1,
    y: LAYOUT_H / 2 + 0.7,
    w: 2,
    h: 0.08,
    fill: { color: 'FFFFFF' },
    line: { type: 'none' },
  });
  if (slide.subtitle) {
    s.addText(slide.subtitle, {
      x: MARGIN,
      y: LAYOUT_H / 2 + 1,
      w: LAYOUT_W - MARGIN * 2,
      h: 0.8,
      fontFace: theme.body_font,
      fontSize: 22,
      color: 'FFFFFF',
      align: 'center',
    });
  }
}

export function renderImageFull(
  s: pptxgen.Slide,
  slide: ImageFullSlide,
  theme: ThemeTokens,
): void {
  setBackground(s, theme);
  s.addImage({
    path: slide.image_url,
    x: 0,
    y: 0,
    w: LAYOUT_W,
    h: LAYOUT_H,
    sizing: { type: 'cover', w: LAYOUT_W, h: LAYOUT_H },
  });
  if (slide.title || slide.caption) {
    s.addShape('rect', {
      x: 0,
      y: LAYOUT_H - 1.6,
      w: LAYOUT_W,
      h: 1.6,
      fill: { color: '000000', transparency: 40 },
      line: { type: 'none' },
    });
  }
  if (slide.title) {
    s.addText(slide.title, {
      x: MARGIN,
      y: LAYOUT_H - 1.4,
      w: LAYOUT_W - MARGIN * 2,
      h: 0.7,
      fontFace: theme.title_font,
      fontSize: 28,
      bold: true,
      color: 'FFFFFF',
      align: 'left',
    });
  }
  if (slide.caption) {
    s.addText(slide.caption, {
      x: MARGIN,
      y: LAYOUT_H - 0.7,
      w: LAYOUT_W - MARGIN * 2,
      h: 0.5,
      fontFace: theme.body_font,
      fontSize: 14,
      italic: true,
      color: 'FFFFFF',
      align: 'left',
    });
  }
}

export function renderSlide(s: pptxgen.Slide, slide: Slide, theme: ThemeTokens): void {
  switch (slide.layout) {
    case 'title':
      return renderTitle(s, slide, theme);
    case 'agenda':
      return renderAgenda(s, slide, theme);
    case 'body':
      return renderBody(s, slide, theme);
    case 'two_column':
      return renderTwoColumn(s, slide, theme);
    case 'stat':
      return renderStat(s, slide, theme);
    case 'quote':
      return renderQuote(s, slide, theme);
    case 'section':
      return renderSection(s, slide, theme);
    case 'image_full':
      return renderImageFull(s, slide, theme);
  }
}
