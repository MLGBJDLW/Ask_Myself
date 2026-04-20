import type { ThemeName, ThemeTokens } from './types';

export const THEMES: Record<ThemeName, ThemeTokens> = {
  'nexa-light': {
    primary_color: '14B8A6',
    accent_color: '0D9488',
    background_color: 'FFFFFF',
    text_color: '0F172A',
    title_color: '0F172A',
    title_font: 'Inter',
    body_font: 'Inter',
  },
  'nexa-dark': {
    primary_color: '14B8A6',
    accent_color: '2DD4BF',
    background_color: '0A0A0F',
    text_color: 'F0F0F5',
    title_color: 'FFFFFF',
    title_font: 'Inter',
    body_font: 'Inter',
  },
};

export function resolveTheme(input: string | ThemeTokens | undefined): ThemeTokens {
  if (!input) return THEMES['nexa-light'];
  if (typeof input === 'string') {
    return THEMES[input as ThemeName] ?? THEMES['nexa-light'];
  }
  return { ...THEMES['nexa-light'], ...input };
}
