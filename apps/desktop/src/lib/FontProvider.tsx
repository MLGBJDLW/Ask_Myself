import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import * as api from './api';
import { FONT_PRESETS } from './fontCatalog';
import { useDisplayPreferences } from './displayPreferences';

const FontContext = createContext<{ assets: api.FontAsset[]; reload: () => Promise<void>; error: string | null }>({ assets: [], reload: async () => {}, error: null });
export const useFonts = () => useContext(FontContext);

export function FontProvider({ children }: { children: ReactNode }) {
  const [assets, setAssets] = useState<api.FontAsset[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [applyErrors, setApplyErrors] = useState<Record<string, string | null>>({});
  const faces = useRef(new Map<string, FontFace>());
  const sequence = useRef(0);
  const preferences = useDisplayPreferences();
  const reload = useCallback(async () => {
    const revision = ++sequence.current;
    try {
      const next = await api.listFontAssets();
      if (revision !== sequence.current) return;
      setAssets(Array.isArray(next) ? next : []);
      setLoadError(null);
    } catch (error) {
      if (revision === sequence.current) setLoadError(String(error));
    }
  }, []);
  useEffect(() => { void reload(); return () => { sequence.current++; }; }, [reload]);
  useEffect(() => {
    let cancelled = false;
    const root = document.documentElement.style;
    for (const [slot, id] of [['sans', preferences.uiFontId], ['mono', preferences.codeFontId]]) {
      root.removeProperty(`--user-font-${slot}`);
      setApplyErrors(errors => ({ ...errors, [slot]: null }));
      if (id === 'theme') continue;
      const preset = FONT_PRESETS.find(font => font.id === id);
      const asset = assets.find(font => font.id === id);
      if (!preset && !asset) continue;
      void (async () => {
        let family: string;
        if (preset) {
          await preset.load();
          family = preset.family;
          await document.fonts.load(`16px "${family}"`, 'Nexa 中文');
        } else {
          family = asset!.family;
          let face = faces.current.get(asset!.id);
          if (!face) {
            face = new FontFace(family, `url(${JSON.stringify(convertFileSrc(asset!.path))})`);
            faces.current.set(asset!.id, face);
          }
          await face.load();
          if (!cancelled) document.fonts.add(face);
        }
        if (!cancelled) root.setProperty(`--user-font-${slot}`, `"${family}", var(--theme-font-${slot}, var(--font-${slot}))`);
      })().catch(error => {
        if (!cancelled) setApplyErrors(errors => ({ ...errors, [slot]: String(error) }));
      });
    }
    return () => { cancelled = true; };
  }, [assets, preferences.uiFontId, preferences.codeFontId]);
  useEffect(() => {
    const ids = new Set(assets.map(asset => asset.id));
    for (const [id, face] of faces.current) if (!ids.has(id)) {
      document.fonts.delete(face);
      faces.current.delete(id);
    }
  }, [assets]);
  return <FontContext.Provider value={{ assets, reload, error: loadError ?? Object.values(applyErrors).find(Boolean) ?? null }}>{children}</FontContext.Provider>;
}
