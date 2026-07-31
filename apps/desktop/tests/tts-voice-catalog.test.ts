import {
  bindTtsVoiceCatalogCredential,
  isTtsVoiceCatalogStale,
  loadTtsVoiceCatalog,
  saveTtsVoiceCatalog,
  ttsVoiceCatalogCacheKey,
  ttsVoiceCatalogMatches,
  type TtsVoiceCatalogSnapshot,
} from '../src/lib/ttsVoiceCatalog';
import type { TextToSpeechConfig } from '../src/types/conversation';

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

const config: TextToSpeechConfig = {
  provider: 'elevenlabs',
  apiStyle: 'elevenlabs_speech',
  apiKey: 'secret-not-in-key',
  baseUrl: 'https://api.elevenlabs.io/v1/',
  model: 'eleven_flash_v2_5',
  voice: 'voice-1',
  outputFormat: 'mp3',
  speed: 1,
};

const snapshot: TtsVoiceCatalogSnapshot = {
  provider: config.provider,
  apiStyle: config.apiStyle,
  baseUrl: config.baseUrl,
  model: config.model,
  refreshedAt: '2026-07-31T09:00:00Z',
  liveDiscoverySucceeded: true,
  voices: [{
    id: 'voice-1',
    name: 'Account Voice',
    recommended: false,
    source: 'discovered',
    modelIds: [],
    languages: ['en-US'],
    gender: null,
    description: null,
    previewUrl: null,
  }],
};

const values = new Map<string, string>();
const storage = {
  getItem: (key: string) => values.get(key) ?? null,
  setItem: (key: string, value: string) => { values.set(key, value); },
};

const boundSnapshot = bindTtsVoiceCatalogCredential(snapshot, config);

saveTtsVoiceCatalog(boundSnapshot, config, storage);
assert(
  ttsVoiceCatalogCacheKey(config) === ttsVoiceCatalogCacheKey({ ...config, baseUrl: config.baseUrl?.replace(/\/$/, '') ?? null }),
  'voice catalog key should normalize endpoint trailing slashes',
);
assert(!ttsVoiceCatalogCacheKey(config).includes(config.apiKey), 'voice catalog key must not contain credentials');
assert(loadTtsVoiceCatalog(config, storage)?.voices[0]?.id === 'voice-1', 'cached account voice should be restored');
assert(loadTtsVoiceCatalog({ ...config, apiKey: 'another-account' }, storage) === null, 'credentials should use isolated caches');
assert(ttsVoiceCatalogMatches(boundSnapshot, config), 'snapshot should match provider, endpoint, model, and credential');
assert(!ttsVoiceCatalogMatches(boundSnapshot, { ...config, model: 'eleven_v3' }), 'models should use isolated caches');
assert(!isTtsVoiceCatalogStale(snapshot, Date.parse('2026-08-01T08:59:59Z')), 'catalog should remain fresh for 24 hours');
assert(isTtsVoiceCatalogStale(snapshot, Date.parse('2026-08-01T09:00:01Z')), 'catalog should expire after 24 hours');
