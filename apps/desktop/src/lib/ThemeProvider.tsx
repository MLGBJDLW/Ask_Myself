import { createContext, useContext, useEffect, useState, type ReactNode } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { type ThemeId, getInitialTheme, applyTheme, isThemeId } from './theme';
import { garbageCollectThemeAssets, resolveThemeBackground } from './api';
import { applyCustomTheme, clearCustomThemeVariables, normalizeCustomTheme, type CustomThemeDefinition } from './themeProfile';

interface ThemeContextValue {
  theme: ThemeId;
  activeThemeId: string;
  customThemes: CustomThemeDefinition[];
  setTheme: (theme: string) => void;
  saveCustomTheme: (theme: CustomThemeDefinition) => void;
  deleteCustomTheme: (id: string, additionallyRetainedAssetIds?: string[]) => void;
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [customThemes, setCustomThemes] = useState<CustomThemeDefinition[]>(readCustomThemes);
  const [activeThemeId, setActiveThemeId] = useState<string>(() => {
    const stored = localStorage.getItem(ACTIVE_THEME_KEY);
    return stored && (isThemeId(stored) || readCustomThemes().some((theme) => theme.id === stored))
      ? stored
      : getInitialTheme();
  });
  const activeCustomTheme = customThemes.find((profile) => profile.id === activeThemeId);
  const theme = activeCustomTheme?.baseTheme ?? (isThemeId(activeThemeId) ? activeThemeId : 'dark');

  useEffect(() => {
    let cancelled = false;
    applyTheme(theme);
    if (activeCustomTheme) {
      applyCustomTheme(activeCustomTheme);
      const assetId = activeCustomTheme.background.kind === 'image'
        ? activeCustomTheme.background.assetId
        : undefined;
      if (assetId) {
        void resolveThemeBackground(assetId)
          .then((asset) => {
            if (!cancelled) applyCustomTheme(activeCustomTheme, convertFileSrc(asset.path));
          })
          .catch((error: unknown) => {
            console.warn('Unable to resolve managed theme background', error);
          });
      }
    } else clearCustomThemeVariables();
    localStorage.setItem(ACTIVE_THEME_KEY, activeThemeId);
    return () => { cancelled = true; };
  }, [activeCustomTheme, activeThemeId, theme]);

  const setTheme = (newTheme: string) => {
    if (isThemeId(newTheme) || customThemes.some((profile) => profile.id === newTheme)) setActiveThemeId(newTheme);
  };

  const saveCustomTheme = (profile: CustomThemeDefinition) => {
    const normalized = normalizeCustomTheme(profile);
    setCustomThemes((current) => {
      const next = [...current.filter((item) => item.id !== normalized.id), normalized];
      localStorage.setItem(CUSTOM_THEMES_KEY, JSON.stringify(next));
      void collectUnusedThemeAssets(next);
      return next;
    });
    setActiveThemeId(normalized.id);
  };

  const deleteCustomTheme = (id: string, additionallyRetainedAssetIds: string[] = []) => {
    setCustomThemes((current) => {
      const next = current.filter((profile) => profile.id !== id);
      localStorage.setItem(CUSTOM_THEMES_KEY, JSON.stringify(next));
      void collectUnusedThemeAssets(next, additionallyRetainedAssetIds);
      return next;
    });
    if (activeThemeId === id) setActiveThemeId('dark');
  };

  return (
    <ThemeContext.Provider value={{ theme, activeThemeId, customThemes, setTheme, saveCustomTheme, deleteCustomTheme }}>
      {children}
    </ThemeContext.Provider>
  );
}

async function collectUnusedThemeAssets(
  themes: CustomThemeDefinition[],
  additionallyRetainedAssetIds: string[] = [],
): Promise<void> {
  const retained = [
    ...themes.flatMap((theme) => theme.background.assetId ? [theme.background.assetId] : []),
    ...additionallyRetainedAssetIds,
  ];
  try {
    await garbageCollectThemeAssets(retained);
  } catch (error) {
    console.warn('Unable to garbage collect managed theme assets', error);
  }
}

const CUSTOM_THEMES_KEY = 'nexa-custom-themes-v1';
const ACTIVE_THEME_KEY = 'nexa-active-theme-v1';

function readCustomThemes(): CustomThemeDefinition[] {
  try {
    const value = JSON.parse(localStorage.getItem(CUSTOM_THEMES_KEY) ?? '[]') as unknown;
    if (!Array.isArray(value)) return [];
    return value.flatMap((item) => {
      try { return [normalizeCustomTheme(item)]; } catch { return []; }
    });
  } catch {
    return [];
  }
}

export function useTheme() {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error('useTheme must be used within ThemeProvider');
  return ctx;
}
