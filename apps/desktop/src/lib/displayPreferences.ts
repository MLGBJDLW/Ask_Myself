import { useSyncExternalStore } from 'react';

import type { StreamingMode } from './streaming/textPresentation';
export type { StreamingMode } from './streaming/textPresentation';
export interface DisplayPreferences {
  uiFontId: string;
  codeFontId: string;
  streamingMode: StreamingMode;
}
const KEY = 'nexa-display-preferences';
const defaults: DisplayPreferences = { uiFontId: 'theme', codeFontId: 'theme', streamingMode: 'balanced' };
function read(): DisplayPreferences {
  try {
    const value = JSON.parse(localStorage.getItem(KEY) ?? '{}') as Partial<DisplayPreferences>;
    return {
      uiFontId: typeof value.uiFontId === 'string' ? value.uiFontId : 'theme',
      codeFontId: typeof value.codeFontId === 'string' ? value.codeFontId : 'theme',
      streamingMode: ['chunked', 'balanced', 'smooth'].includes(value.streamingMode ?? '') ? value.streamingMode! : 'balanced',
    };
  } catch { return defaults; }
}
let preferences = read();
const listeners = new Set<() => void>();
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };
export const getDisplayPreferences = () => preferences;
export function updateDisplayPreferences(patch: Partial<DisplayPreferences>) {
  preferences = { ...preferences, ...patch };
  try { localStorage.setItem(KEY, JSON.stringify(preferences)); } catch { /* Current session still works. */ }
  listeners.forEach(listener => listener());
}
if (typeof window !== 'undefined') window.addEventListener('storage', event => {
  if (event.key !== KEY && event.key !== null) return;
  preferences = read();
  listeners.forEach(listener => listener());
});
export const useDisplayPreferences = () => useSyncExternalStore(subscribe, getDisplayPreferences);
