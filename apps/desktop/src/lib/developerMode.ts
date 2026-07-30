import { useCallback, useEffect, useState } from 'react';

const STORAGE_KEY = 'nexa-developer-mode';
const CHANGE_EVENT = 'nexa-developer-mode-change';

export function getDeveloperMode(): boolean {
  try {
    return localStorage.getItem(STORAGE_KEY) === 'true';
  } catch {
    return false;
  }
}

export function setDeveloperMode(enabled: boolean): void {
  try {
    if (enabled) localStorage.setItem(STORAGE_KEY, 'true');
    else localStorage.removeItem(STORAGE_KEY);
  } catch {
    // Storage can be unavailable in hardened webviews.
  }
  window.dispatchEvent(new CustomEvent<boolean>(CHANGE_EVENT, { detail: enabled }));
}

export function useDeveloperMode(): [boolean, (enabled: boolean) => void] {
  const [enabled, setEnabled] = useState(getDeveloperMode);
  useEffect(() => {
    const onChange = (event: Event) => {
      setEnabled((event as CustomEvent<boolean>).detail ?? getDeveloperMode());
    };
    window.addEventListener(CHANGE_EVENT, onChange);
    return () => window.removeEventListener(CHANGE_EVENT, onChange);
  }, []);
  const update = useCallback((next: boolean) => {
    setDeveloperMode(next);
    setEnabled(next);
  }, []);
  return [enabled, update];
}
