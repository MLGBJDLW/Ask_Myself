export const MIC_DEVICE_STORAGE_KEY = 'nexa-mic-device-id';
export const MIC_DEVICE_CHANGED_EVENT = 'nexa:mic-device-changed';
const LEGACY_MIC_DEVICE_STORAGE_KEY = 'ask-myself-mic-device-id';

function browserStorage(): Storage | null {
  if (typeof window === 'undefined') return null;
  try {
    return window.localStorage ?? null;
  } catch {
    return null;
  }
}

export function migrateLegacyMicDeviceId(storage = browserStorage()): void {
  if (!storage || storage.getItem(MIC_DEVICE_STORAGE_KEY)) return;
  const legacy = storage.getItem(LEGACY_MIC_DEVICE_STORAGE_KEY);
  if (!legacy) return;

  storage.setItem(MIC_DEVICE_STORAGE_KEY, legacy);
  storage.removeItem(LEGACY_MIC_DEVICE_STORAGE_KEY);
}

export function readSelectedMicDeviceId(storage = browserStorage()): string | null {
  migrateLegacyMicDeviceId(storage);
  return storage?.getItem(MIC_DEVICE_STORAGE_KEY) ?? null;
}

export function writeSelectedMicDeviceId(id: string | null, storage = browserStorage()): void {
  if (!storage) return;
  if (id) {
    storage.setItem(MIC_DEVICE_STORAGE_KEY, id);
  } else {
    storage.removeItem(MIC_DEVICE_STORAGE_KEY);
  }
  if (typeof window !== 'undefined') {
    window.dispatchEvent(new CustomEvent(MIC_DEVICE_CHANGED_EVENT, { detail: { deviceId: id } }));
  }
}
