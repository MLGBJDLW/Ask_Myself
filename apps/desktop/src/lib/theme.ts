export type ThemeId = 'dark' | 'light' | 'midnight';

export interface ThemeOption {
  id: ThemeId;
  label: string;
  icon: string;
}

export const THEMES: ThemeOption[] = [
  { id: 'dark', label: 'Dark', icon: 'moon' },
  { id: 'light', label: 'Light', icon: 'sun' },
  { id: 'midnight', label: 'Midnight', icon: 'star' },
];

export const STORAGE_KEY = 'nexa-theme';
const LEGACY_STORAGE_KEY = 'ask-myself-theme';

// One-shot migration from pre-Nexa storage key (v0.x rebrand)
if (typeof window !== 'undefined' && window.localStorage) {
  if (!localStorage.getItem(STORAGE_KEY)) {
    const legacy = localStorage.getItem(LEGACY_STORAGE_KEY);
    if (legacy) {
      localStorage.setItem(STORAGE_KEY, legacy);
      localStorage.removeItem(LEGACY_STORAGE_KEY);
    }
  }
}

export function getInitialTheme(): ThemeId {
  const stored = localStorage.getItem(STORAGE_KEY);
  if (stored && ['dark', 'light', 'midnight'].includes(stored)) {
    return stored as ThemeId;
  }
  if (window.matchMedia('(prefers-color-scheme: light)').matches) {
    return 'light';
  }
  return 'dark';
}

export function applyTheme(theme: ThemeId): void {
  const root = document.documentElement;
  root.classList.remove('theme-light', 'theme-midnight');
  if (theme !== 'dark') {
    root.classList.add(`theme-${theme}`);
  }
  localStorage.setItem(STORAGE_KEY, theme);
}
