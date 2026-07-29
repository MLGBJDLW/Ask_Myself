export type ThemeId = 'dark' | 'light' | 'midnight' | 'aurora' | 'bloom' | 'dream';

export interface ThemeOption {
  id: ThemeId;
  label: string;
  icon: string;
}

export const THEMES: ThemeOption[] = [
  { id: 'dark', label: 'Dark', icon: 'moon' },
  { id: 'light', label: 'Light', icon: 'sun' },
  { id: 'midnight', label: 'Midnight', icon: 'star' },
  { id: 'aurora', label: 'Aurora', icon: 'sparkles' },
  { id: 'bloom', label: 'Bloom', icon: 'palette' },
  { id: 'dream', label: 'Dream', icon: 'heart' },
];

export const THEME_IDS = THEMES.map((theme) => theme.id);
export const LIGHT_THEME_IDS: ThemeId[] = ['light', 'bloom'];

export const STORAGE_KEY = 'nexa-theme';

export function isThemeId(value: string): value is ThemeId {
  return THEME_IDS.includes(value as ThemeId);
}

export function isLightTheme(theme: ThemeId): boolean {
  return LIGHT_THEME_IDS.includes(theme);
}

export function getInitialTheme(): ThemeId {
  const stored = localStorage.getItem(STORAGE_KEY);
  if (stored && isThemeId(stored)) {
    return stored;
  }
  if (window.matchMedia('(prefers-color-scheme: light)').matches) {
    return 'light';
  }
  return 'dark';
}

export function applyTheme(theme: ThemeId): void {
  const root = document.documentElement;
  for (const id of THEME_IDS) {
    if (id !== 'dark') root.classList.remove(`theme-${id}`);
  }
  if (theme !== 'dark') {
    root.classList.add(`theme-${theme}`);
  }
  localStorage.setItem(STORAGE_KEY, theme);
}
