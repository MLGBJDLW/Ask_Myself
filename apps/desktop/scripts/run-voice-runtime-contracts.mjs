import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import ts from 'typescript';

const root = process.cwd();
const sourcePath = path.join(root, 'src', 'features', 'voice', 'voiceStorage.ts');
const source = fs.readFileSync(sourcePath, 'utf8');
const { outputText } = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.CommonJS,
    target: ts.ScriptTarget.ES2020,
  },
});

class MemoryStorage {
  constructor(seed = {}) {
    this.values = new Map(Object.entries(seed));
  }

  getItem(key) {
    return this.values.get(key) ?? null;
  }

  setItem(key, value) {
    this.values.set(key, String(value));
  }

  removeItem(key) {
    this.values.delete(key);
  }
}

function loadVoiceStorage(windowOverride = undefined) {
  const module = { exports: {} };
  const context = {
    exports: module.exports,
    module,
    window: windowOverride,
    CustomEvent: class CustomEvent {
      constructor(type, init = {}) {
        this.type = type;
        this.detail = init.detail;
      }
    },
  };
  vm.runInNewContext(outputText, context, { filename: sourcePath });
  return module.exports;
}

function loadVoiceInputRuntime() {
  const runtimePath = path.join(root, 'src', 'features', 'voice', 'voiceInputRuntime.ts');
  const runtimeSource = fs.readFileSync(runtimePath, 'utf8');
  const transpiled = ts.transpileModule(runtimeSource, {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2020,
    },
  });
  const module = { exports: {} };
  const context = {
    exports: module.exports,
    module,
    require: (specifier) => {
      if (specifier === 'react') {
        return {
          useCallback: (value) => value,
          useMemo: (value) => value(),
          useState: (value) => [value, () => {}],
        };
      }
      return {};
    },
  };
  vm.runInNewContext(transpiled.outputText, context, { filename: runtimePath });
  return module.exports;
}

function loadWaveform() {
  const waveformPath = path.join(root, 'src', 'features', 'voice', 'waveform.ts');
  const transpiled = ts.transpileModule(fs.readFileSync(waveformPath, 'utf8'), {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2020,
    },
  });
  const module = { exports: {} };
  vm.runInNewContext(transpiled.outputText, { exports: module.exports, module }, {
    filename: waveformPath,
  });
  return module.exports;
}

function loadRealtimePcm() {
  const pcmPath = path.join(root, 'src', 'features', 'voice', 'realtimePcm.ts');
  const transpiled = ts.transpileModule(fs.readFileSync(pcmPath, 'utf8'), {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2020,
    },
  });
  const module = { exports: {} };
  vm.runInNewContext(transpiled.outputText, { exports: module.exports, module }, {
    filename: pcmPath,
  });
  return module.exports;
}

function loadBoundedAudioQueue() {
  const queuePath = path.join(root, 'src', 'features', 'voice', 'boundedAudioQueue.ts');
  const transpiled = ts.transpileModule(fs.readFileSync(queuePath, 'utf8'), {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2020,
    },
  });
  const module = { exports: {} };
  vm.runInNewContext(transpiled.outputText, { exports: module.exports, module, Uint8Array, Error }, {
    filename: queuePath,
  });
  return module.exports;
}

function loadNativeVoiceSpool() {
  const queueModule = loadBoundedAudioQueue();
  const spoolPath = path.join(root, 'src', 'features', 'voice', 'nativeVoiceSpool.ts');
  const transpiled = ts.transpileModule(fs.readFileSync(spoolPath, 'utf8'), {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2020,
    },
  });
  const module = { exports: {} };
  const require = (specifier) => {
    if (specifier === './boundedAudioQueue') return queueModule;
    return {};
  };
  vm.runInNewContext(
    transpiled.outputText,
    { exports: module.exports, module, require, Uint8Array, Error, Promise },
    { filename: spoolPath },
  );
  return module.exports;
}

function loadProviderIcons() {
  const iconsPath = path.join(root, 'src', 'lib', 'providerIcons.tsx');
  const transpiled = ts.transpileModule(fs.readFileSync(iconsPath, 'utf8'), {
    compilerOptions: {
      jsx: ts.JsxEmit.ReactJSX,
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2020,
    },
  });
  const module = { exports: {} };
  const context = {
    exports: module.exports,
    module,
    require: () => ({ jsx: () => null, jsxs: () => null }),
  };
  vm.runInNewContext(transpiled.outputText, context, { filename: iconsPath });
  return module.exports;
}

function loadTtsVoiceCatalog() {
  const fingerprintPath = path.join(root, 'src', 'lib', 'credentialFingerprint.ts');
  const fingerprintTranspiled = ts.transpileModule(fs.readFileSync(fingerprintPath, 'utf8'), {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2020,
    },
  });
  const fingerprintModule = { exports: {} };
  vm.runInNewContext(
    fingerprintTranspiled.outputText,
    { exports: fingerprintModule.exports, module: fingerprintModule },
    { filename: fingerprintPath },
  );

  const modelCatalogPath = path.join(root, 'src', 'lib', 'modelCatalog.ts');
  const modelCatalogTranspiled = ts.transpileModule(fs.readFileSync(modelCatalogPath, 'utf8'), {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2020,
    },
  });
  const modelCatalogModule = { exports: {} };
  vm.runInNewContext(
    modelCatalogTranspiled.outputText,
    { exports: modelCatalogModule.exports, module: modelCatalogModule, URL },
    { filename: modelCatalogPath },
  );

  const catalogPath = path.join(root, 'src', 'lib', 'ttsVoiceCatalog.ts');
  const transpiled = ts.transpileModule(fs.readFileSync(catalogPath, 'utf8'), {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2020,
    },
  });
  const module = { exports: {} };
  const require = (specifier) => {
    if (specifier === './credentialFingerprint') return fingerprintModule.exports;
    if (specifier === './modelCatalog') return modelCatalogModule.exports;
    throw new Error(`Unexpected TTS catalog dependency: ${specifier}`);
  };
  vm.runInNewContext(transpiled.outputText, { exports: module.exports, module, require }, {
    filename: catalogPath,
  });
  return module.exports;
}

function test(name, fn) {
  try {
    fn();
    console.log(`ok - ${name}`);
  } catch (error) {
    console.error(`not ok - ${name}`);
    throw error;
  }
}

async function testAsync(name, fn) {
  try {
    await fn();
    console.log(`ok - ${name}`);
  } catch (error) {
    console.error(`not ok - ${name}`);
    throw error;
  }
}

test('OpenAI live transcription is a distinct realtime provider', () => {
  const sttCatalog = JSON.parse(fs.readFileSync(path.join(root, '..', '..', 'shared', 'stt-provider-presets.json'), 'utf8'));
  const openaiLive = sttCatalog.find((preset) => preset.id === 'openai-live');

  assert.ok(openaiLive, 'OpenAI Live preset should exist');
  assert.equal(openaiLive.apiStyle, 'openai_realtime_transcription');
  assert.equal(openaiLive.baseUrl, 'https://api.openai.com/v1');
  assert.equal(openaiLive.models[0].id, 'gpt-live-transcribe');
  assert.equal(openaiLive.models[0].recommended, true);
});

test('QwenCloud and Token Plan keep distinct Qwen model catalogs', () => {
  const catalog = JSON.parse(fs.readFileSync(path.join(root, '..', '..', 'shared', 'provider-presets.json'), 'utf8'));
  const qwenCloud = catalog.find((preset) => preset.id === 'qwen-cloud-intl');
  const tokenPlan = catalog.find((preset) => preset.id === 'qwen-token-plan-cn');

  assert.ok(qwenCloud, 'QwenCloud international preset should exist');
  assert.equal(qwenCloud.baseUrl, 'https://dashscope-intl.aliyuncs.com/compatible-mode/v1');
  assert.ok(qwenCloud.models.some((model) => model.id === 'qwen3.7-flash' && model.recommended));
  assert.deepEqual(tokenPlan.models.map((model) => model.id), [
    'qwen3.8-max',
    'qwen3.8-max-preview',
  ]);
});

test('QwenCloud international preset uses the Qwen provider identity', () => {
  const { resolveProviderIconMeta } = loadProviderIcons();
  const icon = resolveProviderIconMeta({
    provider: 'alibaba_model_studio',
    providerId: 'qwen-cloud-intl',
    baseUrl: 'https://dashscope-intl.aliyuncs.com/compatible-mode/v1',
  });

  assert.equal(icon.label, 'Qwen');
});

test('TTS voice catalogs are partitioned by a non-secret credential fingerprint', () => {
  const voiceCatalog = loadTtsVoiceCatalog();
  const storage = new MemoryStorage();
  const firstConfig = {
    provider: 'elevenlabs',
    apiStyle: 'elevenlabs_tts',
    apiKey: 'account-one-secret',
    baseUrl: 'https://api.elevenlabs.io/v1',
    model: 'eleven_multilingual_v2',
  };
  const secondConfig = { ...firstConfig, apiKey: 'account-two-secret' };
  const snapshot = {
    provider: firstConfig.provider,
    apiStyle: firstConfig.apiStyle,
    baseUrl: firstConfig.baseUrl,
    model: firstConfig.model,
    voices: [{ id: 'private-voice', name: 'Private voice' }],
    refreshedAt: new Date().toISOString(),
    liveDiscoverySucceeded: true,
  };

  voiceCatalog.saveTtsVoiceCatalog(snapshot, firstConfig, storage);
  const firstKey = voiceCatalog.ttsVoiceCatalogCacheKey(firstConfig);
  const secondKey = voiceCatalog.ttsVoiceCatalogCacheKey(secondConfig);
  assert.notEqual(firstKey, secondKey);
  assert.equal(firstKey.includes(firstConfig.apiKey), false);
  assert.equal(voiceCatalog.loadTtsVoiceCatalog(firstConfig, storage).voices[0].id, 'private-voice');
  assert.equal(voiceCatalog.loadTtsVoiceCatalog(secondConfig, storage), null);
});

test('migrates the legacy microphone device key once', () => {
  const storage = new MemoryStorage({ 'ask-myself-mic-device-id': 'legacy-mic' });
  const voiceStorage = loadVoiceStorage();

  voiceStorage.migrateLegacyMicDeviceId(storage);

  assert.equal(storage.getItem('nexa-mic-device-id'), 'legacy-mic');
  assert.equal(storage.getItem('ask-myself-mic-device-id'), null);
});

test('readSelectedMicDeviceId performs legacy migration', () => {
  const storage = new MemoryStorage({ 'ask-myself-mic-device-id': 'legacy-mic' });
  const voiceStorage = loadVoiceStorage();

  assert.equal(voiceStorage.readSelectedMicDeviceId(storage), 'legacy-mic');
  assert.equal(storage.getItem('ask-myself-mic-device-id'), null);
});

test('writeSelectedMicDeviceId stores, clears, and emits runtime changes', () => {
  const storage = new MemoryStorage();
  const events = [];
  const voiceStorage = loadVoiceStorage({
    localStorage: storage,
    dispatchEvent: (event) => events.push(event),
  });

  voiceStorage.writeSelectedMicDeviceId('mic-1', storage);
  assert.equal(storage.getItem('nexa-mic-device-id'), 'mic-1');

  voiceStorage.writeSelectedMicDeviceId(null, storage);
  assert.equal(storage.getItem('nexa-mic-device-id'), null);
  assert.deepEqual(events.map((event) => event.detail.deviceId), ['mic-1', null]);
});

test('accepts configured DashScope ASR providers for voice input', () => {
  const { isSpeechToTextConfigured } = loadVoiceInputRuntime();
  const configured = {
    provider: 'qwen',
    apiStyle: 'dashscope_asr',
    apiKey: 'sk-demo',
    baseUrl: 'https://dashscope.aliyuncs.com/api/v1',
    model: 'qwen3-asr-flash',
    language: null,
  };

  assert.equal(isSpeechToTextConfigured(configured), true);
  assert.equal(isSpeechToTextConfigured({ ...configured, apiKey: '' }), false);
  assert.equal(isSpeechToTextConfigured({ ...configured, model: '' }), false);
});

test('accepts configured OpenAI realtime transcription for voice input', () => {
  const { isRealtimeTranscriptionConfig, isSpeechToTextConfigured } = loadVoiceInputRuntime();
  const configured = {
    provider: 'open_ai',
    apiStyle: 'openai_realtime_transcription',
    apiKey: 'sk-demo',
    baseUrl: 'https://api.openai.com/v1',
    model: 'gpt-live-transcribe',
    language: 'zh-cn',
  };

  assert.equal(isRealtimeTranscriptionConfig(configured), true);
  assert.equal(isRealtimeTranscriptionConfig({ ...configured, apiStyle: 'openai_transcription' }), false);
  assert.equal(isSpeechToTextConfigured(configured), true);
  assert.equal(isSpeechToTextConfigured({ ...configured, apiKey: '' }), false);
  assert.equal(isSpeechToTextConfigured({ ...configured, model: '' }), false);
});

test('recovered native spools remain ordered and individually retryable', () => {
  const { queuePendingVoiceSpool, forgetPendingVoiceSpool } = loadVoiceInputRuntime();
  let pending = ['oldest', 'newer'];

  pending = queuePendingVoiceSpool(pending, 'newest');
  pending = queuePendingVoiceSpool(pending, 'newer');
  assert.deepEqual(Array.from(pending), ['oldest', 'newer', 'newest']);

  pending = forgetPendingVoiceSpool(pending, 'oldest');
  assert.deepEqual(Array.from(pending), ['newer', 'newest']);
});

test('desktop API exposes raw realtime and native spool lifecycles', () => {
  const apiSource = fs.readFileSync(path.join(root, 'src', 'lib', 'api.ts'), 'utf8');
  assert.match(apiSource, /start_realtime_transcription_cmd/);
  assert.match(apiSource, /append_realtime_transcription_audio_cmd/);
  assert.match(apiSource, /finish_realtime_transcription_cmd/);
  assert.match(apiSource, /cancel_realtime_transcription_cmd/);
  assert.match(apiSource, /start_voice_audio_spool_cmd/);
  assert.match(apiSource, /append_voice_audio_spool_cmd/);
  assert.match(apiSource, /finish_voice_audio_spool_cmd/);
  assert.match(apiSource, /transcribe_voice_audio_spool_cmd/);
  assert.match(apiSource, /cancel_voice_audio_spool_cmd/);
  assert.doesNotMatch(apiSource, /transcribe_audio_buffer_cmd/);
  assert.match(
    apiSource,
    /invoke<void>\('append_realtime_transcription_audio_cmd', audioData, \{/,
    'realtime PCM should use a raw Uint8Array invoke body',
  );
  assert.match(
    apiSource,
    /invoke<VoiceAudioSpoolProgress>\('append_voice_audio_spool_cmd', audioData, \{/,
    'native spool PCM should use a raw Uint8Array invoke body',
  );
  assert.match(apiSource, /'x-nexa-session-id': sessionId/);
  assert.match(apiSource, /'x-nexa-voice-session-id': sessionId/);
  assert.match(apiSource, /'x-nexa-voice-sequence': String\(sequence\)/);
  assert.doesNotMatch(apiSource, /Array\.from\(audioData\)/);
});

test('realtime renderer upload queue has a hard chunk and byte bound', () => {
  const { BoundedAudioUploadQueue } = loadBoundedAudioQueue();
  const neverCompletes = () => new Promise(() => {});
  const rejectionSnapshots = [];
  const queue = new BoundedAudioUploadQueue(neverCompletes, {
    maxChunks: 3,
    maxBytes: 12,
    maxChunkBytes: 4,
    onRejected: (telemetry) => rejectionSnapshots.push(telemetry),
  });

  assert.equal(queue.enqueue(new Uint8Array(4)), true);
  assert.equal(queue.enqueue(new Uint8Array(4)), true);
  assert.equal(queue.enqueue(new Uint8Array(4)), true);
  assert.equal(queue.enqueue(new Uint8Array(4)), false);
  assert.equal(queue.enqueue(new Uint8Array(6)), false);
  const telemetry = queue.snapshot();
  assert.equal(telemetry.maxQueueDepth, 3);
  assert.equal(telemetry.maxBufferedBytes, 12);
  assert.equal(telemetry.acceptedChunks, 3);
  assert.equal(telemetry.rejectedChunks, 2);
  assert.equal(rejectionSnapshots.length, 2);
  assert.equal(rejectionSnapshots[0].rejectedChunks, 1);
  assert.equal(rejectionSnapshots[1].rejectedChunks, 2);
  queue.cancel();
});

test('audio queue enforces an explicit buffered-duration bound', () => {
  const { BoundedAudioUploadQueue } = loadBoundedAudioQueue();
  const queue = new BoundedAudioUploadQueue(() => new Promise(() => {}), {
    maxChunks: 100,
    maxBytes: 1024,
    maxChunkBytes: 1024,
    bytesPerSecond: 4,
    maxBufferedDurationMs: 1_000,
  });

  assert.equal(queue.enqueue(new Uint8Array(4)), true);
  assert.equal(queue.enqueue(new Uint8Array(2)), false);
  assert.equal(queue.snapshot().maxBufferedDurationMs, 1_000);
  queue.cancel();
});

test('voice runtime no longer builds an unbounded Promise chain or JSON byte arrays', () => {
  const runtimeSource = fs.readFileSync(
    path.join(root, 'src', 'features', 'voice', 'voiceInputRuntime.ts'),
    'utf8',
  );
  assert.doesNotMatch(runtimeSource, /realtimeUploadChainRef/);
  assert.doesNotMatch(runtimeSource, /Array\.from\((wav|chunk)\)/);
  assert.match(runtimeSource, /BoundedAudioUploadQueue/);
  assert.match(runtimeSource, /NativeVoiceSpoolUpload/);
  assert.match(runtimeSource, /startInProgressRef\.current/);
  assert.match(runtimeSource, /onRejected: \(\) => degradeRealtimeToSpool\(true\)/);
  assert.match(runtimeSource, /startManagedVoiceSpool/);
  assert.match(runtimeSource, /upload\.enqueue\(chunk\)/);
  assert.match(runtimeSource, /queueRealtimeAudio\(sessionId, chunk\)/);
  assert.doesNotMatch(runtimeSource, /backpressure limit reached/);

  const recorderSource = fs.readFileSync(
    path.join(root, 'src', 'features', 'voice', 'useVoiceRecorder.ts'),
    'utf8',
  );
  assert.doesNotMatch(recorderSource, /buffersRef|OfflineAudioContext|encodeWav|captureWav/);
  assert.match(recorderSource, /fixed-size and never retains full PCM/);

  const realtimeNativeSource = fs.readFileSync(
    path.join(root, 'src-tauri', 'src', 'commands', 'realtime_transcription.rs'),
    'utf8',
  );
  assert.match(realtimeNativeSource, /REPLAY_PCM_CHUNK_BYTES: usize = 64 \* 1024/);
  assert.match(realtimeNativeSource, /transcribe_realtime_spool/);
  assert.doesNotMatch(realtimeNativeSource, /std::fs::read\(wav_path\)/);
});

await testAsync('native spool upload assigns ordered sequences and finalizes by opaque handle', async () => {
  const { NativeVoiceSpoolUpload } = loadNativeVoiceSpool();
  const sent = [];
  const transport = {
    append: async (sessionId, sequence, chunk) => {
      sent.push({ sessionId, sequence, bytes: Array.from(chunk) });
    },
    finish: async (sessionId) => ({
      sessionId,
      audioBytes: 8,
      durationMs: 1,
      sampleRate: 16_000,
      checksumSha256: 'abc',
      createdAtMs: 1,
      expiresAtMs: 2,
    }),
    cancel: async () => {},
  };
  const upload = new NativeVoiceSpoolUpload(
    { sessionId: 'opaque-id', sampleRate: 16_000, maxChunkBytes: 4, maxAudioBytes: 1024 },
    transport,
  );

  assert.equal(upload.enqueue(new Uint8Array([1, 0, 2, 0])), true);
  assert.equal(upload.enqueue(new Uint8Array([3, 0, 4, 0])), true);
  const descriptor = await upload.finish();

  assert.equal(descriptor.sessionId, 'opaque-id');
  assert.deepEqual(sent.map(({ sessionId, sequence }) => ({ sessionId, sequence })), [
    { sessionId: 'opaque-id', sequence: 0 },
    { sessionId: 'opaque-id', sequence: 1 },
  ]);
  assert.equal(upload.enqueue(new Uint8Array([5, 0])), false);
});

await testAsync('native spool can finalize acknowledged chunks after a later write failure', async () => {
  const { NativeVoiceSpoolUpload } = loadNativeVoiceSpool();
  let appendCount = 0;
  let finalized = 0;
  const upload = new NativeVoiceSpoolUpload(
    { sessionId: 'recoverable-id', sampleRate: 16_000, maxChunkBytes: 4, maxAudioBytes: 1024 },
    {
      append: async () => {
        appendCount += 1;
        if (appendCount === 2) throw new Error('disk full');
      },
      finish: async (sessionId) => {
        finalized += 1;
        return {
          sessionId,
          audioBytes: 4,
          durationMs: 1,
          sampleRate: 16_000,
          checksumSha256: 'abc',
          createdAtMs: 1,
          expiresAtMs: 2,
        };
      },
      cancel: async () => {},
    },
  );

  upload.enqueue(new Uint8Array([1, 0, 2, 0]));
  await upload.finish();
  assert.equal(finalized, 1);

  const failingUpload = new NativeVoiceSpoolUpload(
    { sessionId: 'recoverable-id-2', sampleRate: 16_000, maxChunkBytes: 4, maxAudioBytes: 1024 },
    {
      append: async () => { throw new Error('disk full'); },
      finish: async (sessionId) => {
        finalized += 1;
        return {
          sessionId,
          audioBytes: 2,
          durationMs: 1,
          sampleRate: 16_000,
          checksumSha256: 'def',
          createdAtMs: 1,
          expiresAtMs: 2,
        };
      },
      cancel: async () => {},
    },
  );
  failingUpload.enqueue(new Uint8Array([1, 0]));
  await assert.rejects(failingUpload.finish(), /disk full/);
  const recovered = await failingUpload.finishAcceptedAudio();

  assert.equal(recovered.sessionId, 'recoverable-id-2');
  assert.equal(finalized, 2);
});

await testAsync('bounded audio queue surfaces native write failures without Promise growth', async () => {
  const { BoundedAudioUploadQueue } = loadBoundedAudioQueue();
  const failures = [];
  const queue = new BoundedAudioUploadQueue(
    async () => { throw new Error('disk full'); },
    { onError: (error, telemetry) => failures.push({ message: error.message, telemetry }) },
  );

  assert.equal(queue.enqueue(new Uint8Array([0, 0])), true);
  await assert.rejects(queue.flush(), /disk full/);
  assert.equal(failures.length, 1);
  assert.equal(failures[0].message, 'disk full');
  assert.equal(failures[0].telemetry.inFlightChunks, 0);
});

test('realtime PCM encoder clips float samples into little-endian PCM16', () => {
  const { float32ToPcm16 } = loadRealtimePcm();
  const encoded = float32ToPcm16(Float32Array.from([-2, -1, 0, 0.5, 1, 2]));
  const view = new DataView(encoded.buffer, encoded.byteOffset, encoded.byteLength);
  assert.deepEqual(
    Array.from({ length: 6 }, (_, index) => view.getInt16(index * 2, true)),
    [-32768, -32768, 0, 16384, 32767, 32767],
  );
});

test('streaming PCM resampling preserves phase across microphone chunks', () => {
  const { StreamingPcm16Encoder } = loadRealtimePcm();
  const onePass = new StreamingPcm16Encoder(48_000, 24_000);
  const split = new StreamingPcm16Encoder(48_000, 24_000);
  const samples = Float32Array.from([0, 0.25, 0.5, 0.75, 1, 0.75, 0.5, 0.25]);

  const expected = onePass.encode(samples);
  const first = split.encode(samples.slice(0, 3));
  const second = split.encode(samples.slice(3));
  const actual = new Uint8Array(first.length + second.length);
  actual.set(first);
  actual.set(second, first.length);

  assert.deepEqual(Array.from(actual), Array.from(expected));
  assert.equal(expected.byteLength, 8);
});

test('waveform bars stay flat for silence and rise with loudness', () => {
  const { computeWaveformBars } = loadWaveform();
  const silence = new Float32Array(512);
  assert.deepEqual(Array.from(computeWaveformBars(silence, 8)), new Array(8).fill(0));

  const quiet = Float32Array.from({ length: 512 }, (_, i) => 0.05 * Math.sin(i));
  const loud = Float32Array.from({ length: 512 }, (_, i) => 0.9 * Math.sin(i));
  const quietBars = computeWaveformBars(quiet, 8);
  const loudBars = computeWaveformBars(loud, 8);
  for (let index = 0; index < 8; index += 1) {
    assert.ok(quietBars[index] > 0, `quiet bar ${index} should be audible`);
    assert.ok(loudBars[index] > quietBars[index], `loud bar ${index} should exceed quiet`);
    assert.ok(loudBars[index] <= 1, `bar ${index} must stay normalized`);
  }
});

test('waveform bars localize loudness to the matching bucket', () => {
  const { computeWaveformBars } = loadWaveform();
  const samples = new Float32Array(400);
  samples.fill(0.8, 300, 400);

  const bars = Array.from(computeWaveformBars(samples, 4));
  assert.deepEqual(bars.slice(0, 3), [0, 0, 0]);
  assert.ok(bars[3] > 0.5);
});

test('waveform smoothing rises faster than it falls and clamps to a visible floor', () => {
  const { smoothWaveformBars, toBarHeights } = loadWaveform();
  const rising = smoothWaveformBars([0, 0], [1, 1]);
  const falling = smoothWaveformBars([1, 1], [0, 0]);
  assert.ok(rising[0] > 1 - falling[0], 'attack should outpace release');

  assert.deepEqual(smoothWaveformBars([0], [0.5, 0.5]), [0.5, 0.5]);
  assert.deepEqual(toBarHeights([0, 0.5, 2]), [0.06, 0.5, 1]);
});
