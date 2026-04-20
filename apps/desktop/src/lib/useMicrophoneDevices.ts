import { useState, useEffect, useCallback } from 'react';

const STORAGE_KEY = 'nexa-mic-device-id';
const LEGACY_STORAGE_KEY = 'ask-myself-mic-device-id';

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

export interface UseMicrophoneDevicesReturn {
  devices: MediaDeviceInfo[];
  selectedDeviceId: string | null;
  setSelectedDeviceId: (id: string | null) => void;
  refresh: () => Promise<void>;
}

export function useMicrophoneDevices(): UseMicrophoneDevicesReturn {
  const [devices, setDevices] = useState<MediaDeviceInfo[]>([]);
  const [selectedDeviceId, setSelectedDeviceIdState] = useState<string | null>(
    () => localStorage.getItem(STORAGE_KEY),
  );

  const refresh = useCallback(async () => {
    try {
      const allDevices = await navigator.mediaDevices.enumerateDevices();
      const audioInputs = allDevices.filter((d) => d.kind === 'audioinput');
      setDevices(audioInputs);

      // If saved device is no longer present, reset to default
      const savedId = localStorage.getItem(STORAGE_KEY);
      if (savedId && !audioInputs.some((d) => d.deviceId === savedId)) {
        localStorage.removeItem(STORAGE_KEY);
        setSelectedDeviceIdState(null);
      }
    } catch {
      // enumerateDevices not available or permission issue
      setDevices([]);
    }
  }, []);

  const setSelectedDeviceId = useCallback((id: string | null) => {
    if (id) {
      localStorage.setItem(STORAGE_KEY, id);
    } else {
      localStorage.removeItem(STORAGE_KEY);
    }
    setSelectedDeviceIdState(id);
  }, []);

  useEffect(() => {
    refresh();

    // Re-enumerate when devices change (plug/unplug)
    const handler = () => { refresh(); };
    navigator.mediaDevices?.addEventListener('devicechange', handler);
    return () => {
      navigator.mediaDevices?.removeEventListener('devicechange', handler);
    };
  }, [refresh]);

  return { devices, selectedDeviceId, setSelectedDeviceId, refresh };
}
