import type { TextToSpeechConfig } from '../types/conversation';
import { credentialFingerprint } from './credentialFingerprint';
import { normalizeModelEndpointUrl } from './modelCatalog';

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
  credentialFingerprint?: string;
}

interface StoredTtsVoiceCatalog {
  version: 2;
  snapshot: TtsVoiceCatalogSnapshot;
}

export const TTS_VOICE_CATALOG_TTL_MS = 24 * 60 * 60 * 1000;
const CACHE_PREFIX = 'nexa-tts-voice-catalog-v1:';

function normalizeLabel(value: string | null | undefined): string {
  return (value ?? '').trim().toLowerCase();
}

function normalizeModelId(value: string | null | undefined): string {
  return (value ?? '').trim();
}

type TtsVoiceCatalogIdentity = Pick<
  TextToSpeechConfig,
  'provider' | 'apiStyle' | 'apiKey' | 'baseUrl' | 'model'
>;

export function ttsCredentialFingerprint(apiKey: string): string {
  return credentialFingerprint(apiKey);
}

export function ttsVoiceCatalogCacheKey(config: TtsVoiceCatalogIdentity): string {
  return `${CACHE_PREFIX}${encodeURIComponent([
    normalizeLabel(config.provider),
    normalizeLabel(config.apiStyle),
    normalizeModelEndpointUrl(config.baseUrl),
    normalizeModelId(config.model),
    ttsCredentialFingerprint(config.apiKey),
  ].join('::'))}`;
}

export function bindTtsVoiceCatalogCredential(
  snapshot: TtsVoiceCatalogSnapshot,
  config: TtsVoiceCatalogIdentity,
): TtsVoiceCatalogSnapshot {
  return {
    ...snapshot,
    credentialFingerprint: ttsCredentialFingerprint(config.apiKey),
  };
}

export function ttsVoiceCatalogMatches(
  snapshot: TtsVoiceCatalogSnapshot,
  config: TextToSpeechConfig,
): boolean {
  return normalizeLabel(snapshot.provider) === normalizeLabel(config.provider)
    && normalizeLabel(snapshot.apiStyle) === normalizeLabel(config.apiStyle)
    && normalizeModelEndpointUrl(snapshot.baseUrl) === normalizeModelEndpointUrl(config.baseUrl)
    && normalizeModelId(snapshot.model) === normalizeModelId(config.model)
    && snapshot.credentialFingerprint === ttsCredentialFingerprint(config.apiKey);
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
    const snapshot = stored.version === 2 ? stored.snapshot : null;
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
  config: TtsVoiceCatalogIdentity,
  storage: Pick<Storage, 'setItem'> = localStorage,
): void {
  try {
    const boundSnapshot = bindTtsVoiceCatalogCredential(snapshot, config);
    storage.setItem(
      ttsVoiceCatalogCacheKey(config),
      JSON.stringify({ version: 2, snapshot: boundSnapshot } satisfies StoredTtsVoiceCatalog),
    );
  } catch {
    // Keep the in-memory snapshot usable if browser storage is unavailable.
  }
}
