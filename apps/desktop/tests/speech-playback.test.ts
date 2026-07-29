import {
  classifyMediaError,
  playableMediaType,
  speechCacheKeyMaterial,
} from '../src/features/voice/speechPlaybackRuntime';

function equal(actual: unknown, expected: unknown): void {
  if (actual !== expected) throw new Error(`Expected ${String(expected)}, received ${String(actual)}`);
}

equal(playableMediaType('audio/mpeg', 'probably'), true);
equal(playableMediaType('audio/wav', ''), false);
equal(classifyMediaError(2), 'asset_access');
equal(classifyMediaError(3), 'decode');
equal(classifyMediaError(4), 'unsupported_format');
equal(classifyMediaError(undefined), 'playback');

equal(
  speechCacheKeyMaterial({ provider: 'open_ai', model: 'tts-1', voice: 'alloy', speed: 1, outputFormat: 'mp3' }, '  hello\nworld '),
  'open_ai\u0000tts-1\u0000alloy\u00001\u0000mp3\u0000hello world',
);
