import type { TranslationKey } from '../i18n/types';

export interface Shortcut {
  keys: string;
  macKeys: string;
  description: TranslationKey;
  scope: 'global' | 'chat' | 'search';
}

const isMac =
  typeof navigator !== 'undefined' &&
  /Mac|iPod|iPhone|iPad/.test(navigator.platform);

export const SHORTCUTS: Shortcut[] = [
  { keys: 'Ctrl+K', macKeys: '⌘K', description: 'shortcuts.commandPalette', scope: 'global' },
  { keys: 'Ctrl+B', macKeys: '⌘B', description: 'shortcuts.toggleSidebar', scope: 'chat' },
  { keys: 'Ctrl+Shift+A', macKeys: '⌘⇧A', description: 'shortcuts.askAi', scope: 'search' },
];

/** Return the platform-appropriate key display string. */
export function formatKeys(shortcut: Shortcut): string {
  return isMac ? shortcut.macKeys : shortcut.keys;
}
