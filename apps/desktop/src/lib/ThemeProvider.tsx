import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { type ThemeId, getInitialTheme, applyTheme, isThemeId } from './theme';
import * as api from './api';
import {
  applyCustomTheme,
  clearCustomThemeVariables,
  normalizeThemeResourcePlugin,
  themeResourcePluginToCustomTheme,
  themeToResourcePlugin,
  type CustomThemeDefinition,
  type ThemeContent,
  type ThemeResourcePlugin,
} from './themeProfile';
import { persistStartupAppearance, snapshotStartupAppearance } from './startupAppearance';

interface ThemeContextValue {
  theme: ThemeId;
  activeThemeId: string;
  customThemes: CustomThemeDefinition[];
  themePlugins: ThemeResourcePlugin[];
  content: ThemeContent;
  setTheme: (theme: string) => void;
  installThemePlugin: (plugin: ThemeResourcePlugin) => void;
  uninstallThemePlugin: (id: string, additionallyRetainedAssetIds?: string[]) => void;
  rollbackTheme: () => void;
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
  const registryRevisionRef = useRef(0);
  const activeCustomTheme = customThemes.find((profile) => profile.id === activeThemeId);
  const theme = activeCustomTheme?.baseTheme ?? (isThemeId(activeThemeId) ? activeThemeId : 'dark');
  const content = activeCustomTheme?.content ?? {};

  useEffect(() => {
    let cancelled = false;
    applyTheme(theme);
    if (activeCustomTheme) {
      applyCustomTheme(activeCustomTheme);
      const assetId = activeCustomTheme.background.kind === 'image'
        ? activeCustomTheme.background.assetId
        : undefined;
      if (assetId) {
        void api.resolveThemeBackground(assetId)
          .then((asset) => {
            if (!cancelled) applyCustomTheme(activeCustomTheme, convertFileSrc(asset.path));
          })
          .catch((error: unknown) => {
            console.warn('Unable to resolve managed theme background', error);
          });
      }
    } else clearCustomThemeVariables();
    persistStartupAppearance(snapshotStartupAppearance(
      activeCustomTheme?.mode ?? (theme === 'light' || theme === 'bloom' ? 'light' : 'dark'),
      activeCustomTheme?.content,
    ));
    localStorage.setItem(ACTIVE_THEME_KEY, activeThemeId);
    return () => { cancelled = true; };
  }, [activeCustomTheme, activeThemeId, theme]);

  const applyRegistry = useCallback((registry: api.AppearanceRegistry | null | undefined) => {
    if (!registry || !Array.isArray(registry.plugins)) return;
    const plugins = registry.plugins.flatMap((plugin) => {
      try { return [normalizeThemeResourcePlugin(plugin)]; } catch { return []; }
    });
    registryRevisionRef.current = Math.max(registryRevisionRef.current, registry.revision ?? 0);
    localStorage.setItem(THEME_PLUGINS_KEY, JSON.stringify(plugins));
    localStorage.setItem(ACTIVE_THEME_KEY, registry.activeThemeId);
    setThemePlugins(plugins);
    setActiveThemeId(registry.activeThemeId);
  }, []);

  useEffect(() => {
    let disposed = false;
    const initialPlugins = readThemePlugins();
    const initialActive = localStorage.getItem(ACTIVE_THEME_KEY) ?? getInitialTheme();
    void api.hydrateAppearanceRegistry(initialPlugins, initialActive)
      .then((registry) => { if (!disposed) applyRegistry(registry); })
      .catch(() => undefined);
    const timer = window.setInterval(() => {
      void api.getAppearanceRegistry()
        .then((registry) => {
          if (!disposed && registry.revision > registryRevisionRef.current) applyRegistry(registry);
        })
        .catch(() => undefined);
    }, 1_200);
    return () => { disposed = true; window.clearInterval(timer); };
  }, [applyRegistry]);

  const setTheme = (newTheme: string) => {
    if (isThemeId(newTheme) || customThemes.some((profile) => profile.id === newTheme)) {
      setActiveThemeId(newTheme);
      void api.activateAppearance(newTheme).then(applyRegistry).catch((error) => {
        console.warn('Unable to persist active appearance', error);
      });
    }
  };

  const installThemePlugin = (plugin: ThemeResourcePlugin) => {
    const normalized = normalizeThemeResourcePlugin(plugin);
    const next = [...themePlugins.filter((item) => item.id !== normalized.id), normalized];
    localStorage.setItem(THEME_PLUGINS_KEY, JSON.stringify(next));
    setThemePlugins(next);
    void collectUnusedThemeAssets(next.map(themeResourcePluginToCustomTheme));
    setActiveThemeId(normalized.id);
    void api.applyAppearancePlugin(normalized).then(applyRegistry).catch((error) => {
      console.warn('Unable to persist appearance plugin', error);
    });
  };

  const uninstallThemePlugin = (id: string, additionallyRetainedAssetIds: string[] = []) => {
    const next = themePlugins.filter((plugin) => plugin.id !== id);
    localStorage.setItem(THEME_PLUGINS_KEY, JSON.stringify(next));
    setThemePlugins(next);
    void collectUnusedThemeAssets(next.map(themeResourcePluginToCustomTheme), additionallyRetainedAssetIds);
    if (activeThemeId === id) setActiveThemeId('dark');
    void api.removeAppearance(id).then(applyRegistry).catch((error) => {
      console.warn('Unable to remove appearance plugin from the durable registry', error);
    });
  };

  const rollbackTheme = () => {
    void api.rollbackAppearance().then(applyRegistry).catch((error) => {
      console.warn('Unable to roll back appearance', error);
    });
  };

  return (
    <ThemeContext.Provider value={{ theme, activeThemeId, customThemes, themePlugins, content, setTheme, installThemePlugin, uninstallThemePlugin, rollbackTheme }}>
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
    await api.garbageCollectThemeAssets(retained);
  } catch (error) {
    console.warn('Unable to garbage collect managed theme assets', error);
  }
}

const THEME_PLUGINS_KEY = 'nexa-theme-resource-plugins-v2';
const LEGACY_THEME_PLUGINS_KEY = 'nexa-theme-resource-plugins-v1';
const LEGACY_CUSTOM_THEMES_KEY = 'nexa-custom-themes-v1';
const ACTIVE_THEME_KEY = 'nexa-active-theme-v1';

function readThemePlugins(): ThemeResourcePlugin[] {
  try {
    const storedPlugins = localStorage.getItem(THEME_PLUGINS_KEY)
      ?? localStorage.getItem(LEGACY_THEME_PLUGINS_KEY);
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
    if (plugins.length > 0) localStorage.setItem(THEME_PLUGINS_KEY, JSON.stringify(plugins));
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
