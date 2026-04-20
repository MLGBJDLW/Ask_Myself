export type ThemeName = 'nexa-light' | 'nexa-dark';

export interface ThemeTokens {
  primary_color: string;
  accent_color: string;
  background_color: string;
  text_color: string;
  title_color: string;
  title_font: string;
  body_font: string;
}

export type Theme = ThemeName | ThemeTokens;

export interface TitleSlide {
  layout: 'title';
  title: string;
  subtitle?: string;
  author?: string;
  image_url?: string;
}

export interface AgendaSlide {
  layout: 'agenda';
  title?: string;
  items: string[];
}

export interface BodySlide {
  layout: 'body';
  title: string;
  bullets?: string[];
  paragraph?: string;
  image_url?: string;
  image_caption?: string;
}

export interface ColumnContent {
  heading?: string;
  bullets?: string[];
  paragraph?: string;
  image_url?: string;
}

export interface TwoColumnSlide {
  layout: 'two_column';
  title: string;
  left: ColumnContent;
  right: ColumnContent;
}

export interface StatItem {
  value: string;
  label: string;
  caption?: string;
}

export interface StatSlide {
  layout: 'stat';
  title?: string;
  stats: StatItem[];
}

export interface QuoteSlide {
  layout: 'quote';
  text: string;
  attribution?: string;
}

export interface SectionSlide {
  layout: 'section';
  title: string;
  subtitle?: string;
}

export interface ImageFullSlide {
  layout: 'image_full';
  title?: string;
  image_url: string;
  caption?: string;
}

export type Slide =
  | TitleSlide
  | AgendaSlide
  | BodySlide
  | TwoColumnSlide
  | StatSlide
  | QuoteSlide
  | SectionSlide
  | ImageFullSlide;

export interface DeckSpec {
  title: string;
  subtitle?: string;
  author?: string;
  theme?: Theme;
  slides: Slide[];
  notes_per_slide?: string[];
}

export interface DeckArtifact {
  path: string;
  spec: DeckSpec;
}
