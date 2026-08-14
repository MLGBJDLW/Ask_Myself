import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { type ThemeId, getInitialTheme, applyTheme, isThemeId } from './theme';
import { garbageCollectThemeAssets, resolveThemeBackground } from './api';
import {
  applyCustomTheme,
  clearCustomThemeVariables,
  normalizeThemeResourcePlugin,
  themeResourcePluginToCustomTheme,
  themeToResourcePlugin,
  type CustomThemeDefinition,
  type ThemeResourcePlugin,
} from './themeProfile';

interface ThemeContextValue {
  theme: ThemeId;
  activeThemeId: string;
  customThemes: CustomThemeDefinition[];
  themePlugins: ThemeResourcePlugin[];
  setTheme: (theme: string) => void;
  installThemePlugin: (plugin: ThemeResourcePlugin) => void;
  uninstallThemePlugin: (id: string, additionallyRetainedAssetIds?: string[]) => void;
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [themePlugins, setThemePlugins] = useState<ThemeResourcePlugin[]>(readThemePlugins);
  const customThemes = useMemo(
    () => themePlugins.map(themeResourcePluginToCustomTheme),
    [themePlugins],
  );
  const [activeThemeId, setActiveThemeId] = useState<string>(() => {
    const stored = localStorage.getItem(ACTIVE_THEME_KEY);
    return stored && (isThemeId(stored) || readThemePlugins().some((plugin) => plugin.id === stored))
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

  const installThemePlugin = (plugin: ThemeResourcePlugin) => {
    const normalized = normalizeThemeResourcePlugin(plugin);
    const next = [...themePlugins.filter((item) => item.id !== normalized.id), normalized];
    localStorage.setItem(THEME_PLUGINS_KEY, JSON.stringify(next));
    setThemePlugins(next);
    void collectUnusedThemeAssets(next.map(themeResourcePluginToCustomTheme));
    setActiveThemeId(normalized.id);
  };

  const uninstallThemePlugin = (id: string, additionallyRetainedAssetIds: string[] = []) => {
    const next = themePlugins.filter((plugin) => plugin.id !== id);
    localStorage.setItem(THEME_PLUGINS_KEY, JSON.stringify(next));
    setThemePlugins(next);
    void collectUnusedThemeAssets(next.map(themeResourcePluginToCustomTheme), additionallyRetainedAssetIds);
    if (activeThemeId === id) setActiveThemeId('dark');
  };

  return (
    <ThemeContext.Provider value={{ theme, activeThemeId, customThemes, themePlugins, setTheme, installThemePlugin, uninstallThemePlugin }}>
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

const THEME_PLUGINS_KEY = 'nexa-theme-resource-plugins-v1';
const LEGACY_CUSTOM_THEMES_KEY = 'nexa-custom-themes-v1';
const ACTIVE_THEME_KEY = 'nexa-active-theme-v1';

function readThemePlugins(): ThemeResourcePlugin[] {
  try {
    const storedPlugins = localStorage.getItem(THEME_PLUGINS_KEY);
    const value = JSON.parse(storedPlugins ?? localStorage.getItem(LEGACY_CUSTOM_THEMES_KEY) ?? '[]') as unknown;
    if (!Array.isArray(value)) return [];
    const plugins = value.flatMap((item) => {
      try {
        return [storedPlugins
          ? normalizeThemeResourcePlugin(item)
          : themeToResourcePlugin(item as CustomThemeDefinition)];
      } catch {
        return [];
      }
    });
    if (!storedPlugins && plugins.length > 0) localStorage.setItem(THEME_PLUGINS_KEY, JSON.stringify(plugins));
    return plugins;
  } catch {
    return [];
  }
}

export function useTheme() {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error('useTheme must be used within ThemeProvider');
  return ctx;
}
