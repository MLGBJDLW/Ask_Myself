import type { CustomThemeDefinition } from './themeProfile';

export const STARTUP_APPEARANCE_STORAGE_KEY = 'nexa-startup-appearance-v1';

export interface StartupAppearanceSnapshot {
  version: 1;
  mode: 'dark' | 'light';
  canvas: string;
  panel: string;
  text: string;
  accent: string;
  muted: string;
  tagline?: string;
}

export function snapshotStartupAppearance(
  mode: 'dark' | 'light',
  content?: CustomThemeDefinition['content'],
): StartupAppearanceSnapshot | null {
  const style = getComputedStyle(document.documentElement);
  const snapshot: StartupAppearanceSnapshot = {
    version: 1,
    mode,
    canvas: style.getPropertyValue('--color-surface-0').trim(),
    panel: style.getPropertyValue('--color-surface-1').trim(),
    text: style.getPropertyValue('--color-text-primary').trim(),
    accent: style.getPropertyValue('--color-accent').trim(),
    muted: style.getPropertyValue('--color-text-tertiary').trim(),
    ...(content?.tagline ? { tagline: content.tagline } : {}),
  };
  return [snapshot.canvas, snapshot.panel, snapshot.text, snapshot.accent, snapshot.muted]
    .every(Boolean) ? snapshot : null;
}

export function persistStartupAppearance(snapshot: StartupAppearanceSnapshot | null): void {
  if (!snapshot) return;
  try {
    localStorage.setItem(STARTUP_APPEARANCE_STORAGE_KEY, JSON.stringify(snapshot));
  } catch {
    // A valid in-memory theme remains usable when storage is unavailable.
  }
}
