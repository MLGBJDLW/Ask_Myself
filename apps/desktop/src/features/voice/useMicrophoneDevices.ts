import { useState, useEffect, useCallback } from 'react';

import {
  MIC_DEVICE_CHANGED_EVENT,
  MIC_DEVICE_STORAGE_KEY,
  readSelectedMicDeviceId,
  writeSelectedMicDeviceId,
} from './voiceStorage';

export interface UseMicrophoneDevicesReturn {
  devices: MediaDeviceInfo[];
  selectedDeviceId: string | null;
  setSelectedDeviceId: (id: string | null) => void;
  refresh: () => Promise<void>;
}

export function useMicrophoneDevices(): UseMicrophoneDevicesReturn {
  const [devices, setDevices] = useState<MediaDeviceInfo[]>([]);
  const [selectedDeviceId, setSelectedDeviceIdState] = useState<string | null>(
    () => readSelectedMicDeviceId(),
  );

  const refresh = useCallback(async () => {
    if (typeof navigator === 'undefined' || !navigator.mediaDevices?.enumerateDevices) {
      setDevices([]);
      return;
    }

    try {
      const allDevices = await navigator.mediaDevices.enumerateDevices();
      const audioInputs = allDevices.filter((d) => d.kind === 'audioinput');
      setDevices(audioInputs);

      // If saved device is no longer present, reset to default
      const savedId = readSelectedMicDeviceId();
      if (savedId && !audioInputs.some((d) => d.deviceId === savedId)) {
        writeSelectedMicDeviceId(null);
        setSelectedDeviceIdState(null);
      }
    } catch {
      // enumerateDevices not available or permission issue
      setDevices([]);
    }
  }, []);

  const setSelectedDeviceId = useCallback((id: string | null) => {
    writeSelectedMicDeviceId(id);
    setSelectedDeviceIdState(id);
  }, []);

  useEffect(() => {
    refresh();

    // Re-enumerate when devices change (plug/unplug)
    const handler = () => {
      void refresh();
    };
    const selectedDeviceHandler = () => {
      setSelectedDeviceIdState(readSelectedMicDeviceId());
      void refresh();
    };
    const storageHandler = (event: StorageEvent) => {
      if (event.key !== MIC_DEVICE_STORAGE_KEY) return;
      selectedDeviceHandler();
    };
    navigator.mediaDevices?.addEventListener('devicechange', handler);
    window.addEventListener(MIC_DEVICE_CHANGED_EVENT, selectedDeviceHandler);
    window.addEventListener('storage', storageHandler);
    return () => {
      navigator.mediaDevices?.removeEventListener('devicechange', handler);
      window.removeEventListener(MIC_DEVICE_CHANGED_EVENT, selectedDeviceHandler);
      window.removeEventListener('storage', storageHandler);
    };
  }, [refresh]);

  return { devices, selectedDeviceId, setSelectedDeviceId, refresh };
}
