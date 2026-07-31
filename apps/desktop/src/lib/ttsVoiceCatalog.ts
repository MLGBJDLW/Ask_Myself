import type { TextToSpeechConfig } from '../types/conversation';

export interface TtsVoiceCatalogEntry {
  id: string;
  name: string;
  recommended: boolean;
  source: 'discovered' | 'curated';
  modelIds: string[];
  languages: string[];
  gender: string | null;
  description: string | null;
  previewUrl: string | null;
}

export interface TtsVoiceCatalogSnapshot {
  provider: string;
  apiStyle: string;
  baseUrl: string | null;
  model: string;
  voices: TtsVoiceCatalogEntry[];
  refreshedAt: string;
  liveDiscoverySucceeded: boolean;
}

interface StoredTtsVoiceCatalog {
  version: 1;
  snapshot: TtsVoiceCatalogSnapshot;
}

export const TTS_VOICE_CATALOG_TTL_MS = 24 * 60 * 60 * 1000;
const CACHE_PREFIX = 'nexa-tts-voice-catalog-v1:';

function normalize(value: string | null | undefined): string {
  return (value ?? '').trim().replace(/\/+$/, '').toLowerCase();
}

type TtsVoiceCatalogIdentity = Pick<
  TextToSpeechConfig,
  'provider' | 'apiStyle' | 'baseUrl' | 'model'
>;

export function ttsVoiceCatalogCacheKey(config: TtsVoiceCatalogIdentity): string {
  return `${CACHE_PREFIX}${encodeURIComponent([
    normalize(config.provider),
    normalize(config.apiStyle),
    normalize(config.baseUrl),
    normalize(config.model),
  ].join('::'))}`;
}

export function ttsVoiceCatalogMatches(
  snapshot: TtsVoiceCatalogSnapshot,
  config: TextToSpeechConfig,
): boolean {
  return normalize(snapshot.provider) === normalize(config.provider)
    && normalize(snapshot.apiStyle) === normalize(config.apiStyle)
    && normalize(snapshot.baseUrl) === normalize(config.baseUrl)
    && normalize(snapshot.model) === normalize(config.model);
}

export function isTtsVoiceCatalogStale(
  snapshot: TtsVoiceCatalogSnapshot,
  now = Date.now(),
): boolean {
  const refreshedAt = Date.parse(snapshot.refreshedAt);
  return !Number.isFinite(refreshedAt) || now - refreshedAt > TTS_VOICE_CATALOG_TTL_MS;
}

export function loadTtsVoiceCatalog(
  config: TextToSpeechConfig,
  storage: Pick<Storage, 'getItem'> = localStorage,
): TtsVoiceCatalogSnapshot | null {
  try {
    const raw = storage.getItem(ttsVoiceCatalogCacheKey(config));
    if (!raw) return null;
    const stored = JSON.parse(raw) as Partial<StoredTtsVoiceCatalog>;
    const snapshot = stored.version === 1 ? stored.snapshot : null;
    return snapshot
      && Array.isArray(snapshot.voices)
      && ttsVoiceCatalogMatches(snapshot, config)
      ? snapshot
      : null;
  } catch {
    return null;
  }
}

export function saveTtsVoiceCatalog(
  snapshot: TtsVoiceCatalogSnapshot,
  storage: Pick<Storage, 'setItem'> = localStorage,
): void {
  try {
    const config: TtsVoiceCatalogIdentity = {
      provider: snapshot.provider,
      apiStyle: snapshot.apiStyle,
      baseUrl: snapshot.baseUrl,
      model: snapshot.model,
    };
    storage.setItem(
      ttsVoiceCatalogCacheKey(config),
      JSON.stringify({ version: 1, snapshot } satisfies StoredTtsVoiceCatalog),
    );
  } catch {
    // Keep the in-memory snapshot usable if browser storage is unavailable.
  }
}
