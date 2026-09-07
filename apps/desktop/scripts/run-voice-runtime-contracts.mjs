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
      if (specifier === '../../lib/sttProviderPresets') {
        const catalog = JSON.parse(fs.readFileSync(
          path.join(root, '..', '..', 'shared', 'stt-provider-presets.json'),
          'utf8',
        ));
        const finalOnly = {
          audioInput: 'completeFile',
          transcriptDelivery: 'finalOnly',
          sampleRateHz: 16_000,
        };
        return {
          sttRuntimeCapabilities: (config) => {
            const preset = catalog.find((candidate) => (
              candidate.provider === config?.provider
              && candidate.apiStyle === config?.apiStyle
              && (candidate.sherpaModelFamily ?? null) === (
              config?.apiStyle === 'sherpa_onnx'
                ? config?.sherpaModelFamily ?? 'sense_voice'
                : null
              )
            ));
            if (!preset) return finalOnly;
            if (preset.transcription.transcriptDelivery === 'interimAndFinal'
              && !preset.models.some((model) => model.id === config?.model?.trim())) {
              return finalOnly;
            }
            return preset.transcription;
          },
        };
      }
      if (specifier === './boundedVoicePartial') {
        return loadBoundedVoicePartial();
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

function loadVoicePcmProcessor() {
  const processorPath = path.join(root, 'src', 'features', 'voice', 'voicePcmProcessor.js');
  const processorSource = fs.readFileSync(processorPath, 'utf8');
  let registeredName = null;
  let RegisteredProcessor = null;

  class TestAudioWorkletProcessor {
    constructor() {
      this.port = {
        messages: [],
        onmessage: null,
        postMessage(message, transfer = []) {
          this.messages.push({ message, transfer });
        },
      };
    }
  }

  vm.runInNewContext(processorSource, {
    AudioWorkletProcessor: TestAudioWorkletProcessor,
    Int16Array,
    Math,
    Number,
    sampleRate: 48_000,
    registerProcessor: (name, processor) => {
      registeredName = name;
      RegisteredProcessor = processor;
    },
  }, { filename: processorPath });

  assert.equal(registeredName, 'nexa-voice-pcm-processor');
  return RegisteredProcessor;
}

function loadBoundedVoicePartial() {
  const partialPath = path.join(root, 'src', 'features', 'voice', 'boundedVoicePartial.ts');
  const transpiled = ts.transpileModule(fs.readFileSync(partialPath, 'utf8'), {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2020,
    },
  });
  const module = { exports: {} };
  vm.runInNewContext(transpiled.outputText, { exports: module.exports, module }, {
    filename: partialPath,
  });
  return module.exports;
}

function loadVoiceDraftProjection() {
  const projectionPath = path.join(root, 'src', 'features', 'voice', 'voiceDraftProjection.ts');
  const transpiled = ts.transpileModule(fs.readFileSync(projectionPath, 'utf8'), {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2020,
    },
  });
  const module = { exports: {} };
  vm.runInNewContext(transpiled.outputText, { exports: module.exports, module }, {
    filename: projectionPath,
  });
  return module.exports;
}

function loadTerminalPcmDelivery() {
  const deliveryPath = path.join(root, 'src', 'features', 'voice', 'terminalPcmDelivery.ts');
  const transpiled = ts.transpileModule(fs.readFileSync(deliveryPath, 'utf8'), {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2020,
    },
  });
  const module = { exports: {} };
  vm.runInNewContext(
    transpiled.outputText,
    { exports: module.exports, module, Uint8Array },
    { filename: deliveryPath },
  );
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
  assert.equal(openaiLive.transcription.audioInput, 'chunkStream');
  assert.equal(openaiLive.transcription.transcriptDelivery, 'interimAndFinal');
  assert.equal(openaiLive.transcription.interimSemantics, 'appendDelta');
  assert.equal(openaiLive.transcription.sampleRateHz, 24_000);
});

test('STT catalog distinguishes native interim delivery from batch and engine potential', () => {
  const sttCatalog = JSON.parse(fs.readFileSync(path.join(root, '..', '..', 'shared', 'stt-provider-presets.json'), 'utf8'));
  const qwenBatch = sttCatalog.find((preset) => preset.id === 'alibaba-qwen-asr');
  const qwenRealtime = sttCatalog.find((preset) => preset.id === 'alibaba-qwen-realtime');
  const sherpaZipformer = sttCatalog.find((preset) => preset.id === 'sherpa-zipformer');
  const custom = sttCatalog.find((preset) => preset.id === 'custom-openai');

  assert.equal(qwenBatch.transcription.transcriptDelivery, 'finalOnly');
  assert.equal(qwenRealtime.apiStyle, 'dashscope_realtime_asr');
  assert.equal(qwenRealtime.transcription.interimSemantics, 'replaceSnapshot');
  assert.equal(qwenRealtime.transcription.sampleRateHz, 16_000);
  assert.equal(sherpaZipformer.transcription.engineStreamingCapable, true);
  assert.equal(sherpaZipformer.transcription.transcriptDelivery, 'finalOnly');
  assert.equal(custom.transcription.transcriptDelivery, 'finalOnly');
});

test('QwenCloud and Token Plan keep distinct Qwen model catalogs', () => {
  const catalog = JSON.parse(fs.readFileSync(path.join(root, '..', '..', 'shared', 'provider-presets.json'), 'utf8'));
  const qwenCloud = catalog.find((preset) => preset.id === 'qwen-cloud-intl');
  const tokenPlan = catalog.find((preset) => preset.id === 'qwen-token-plan-cn');

  assert.ok(qwenCloud, 'QwenCloud international preset should exist');
  assert.equal(qwenCloud.baseUrl, 'https://dashscope-intl.aliyuncs.com/compatible-mode/v1');
  assert.ok(qwenCloud.models.some((model) => model.id === 'qwen3.8-flash' && model.recommended));
  assert.deepEqual(tokenPlan.models.map((model) => model.id), [
    'qwen3.8-max',
    'qwen3.8-flash',
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

test('accepts Qwen realtime only through its explicit WebSocket dialect', () => {
  const { isRealtimeTranscriptionConfig, isSpeechToTextConfigured } = loadVoiceInputRuntime();
  const configured = {
    provider: 'alibaba_model_studio',
    apiStyle: 'dashscope_realtime_asr',
    apiKey: 'sk-demo',
    baseUrl: 'https://dashscope.aliyuncs.com/api-ws/v1',
    model: 'qwen3-asr-flash-realtime',
    language: 'zh',
  };

  assert.equal(isRealtimeTranscriptionConfig(configured), true);
  assert.equal(isSpeechToTextConfigured(configured), true);
  assert.equal(isRealtimeTranscriptionConfig({
    ...configured,
    apiStyle: 'dashscope_asr',
    model: 'qwen3-asr-flash',
  }), false);
  assert.equal(isRealtimeTranscriptionConfig({
    ...configured,
    model: 'qwen3-asr-flash',
  }), false);
  assert.equal(isSpeechToTextConfigured({
    ...configured,
    model: 'qwen3-asr-flash',
  }), false);
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
  assert.match(apiSource, /VoiceAudioSpoolTranscriptionResult/);
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
  assert.match(runtimeSource, /startupGenerationRef\.current/);
  assert.match(runtimeSource, /startupAbortModeRef\.current/);
  assert.match(runtimeSource, /releaseStaleUpload/);
  assert.ok(
    (runtimeSource.match(/if \(!startupIsCurrent\(\)\)/g) ?? []).length >= 7,
    'every async startup resource boundary must reject a stale generation',
  );
  assert.match(runtimeSource, /pendingVoiceCleanupIdsRef/);
  assert.match(runtimeSource, /discardPendingVoiceSpool/);
  assert.match(runtimeSource, /voiceSpool\.preserveAcceptedAudio\(\)/);
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
  assert.doesNotMatch(recorderSource, /createScriptProcessor|ScriptProcessorNode|StreamingPcm16Encoder/);
  assert.match(recorderSource, /new AudioWorkletNode/);
  assert.match(recorderSource, /new URL\('\.\/voicePcmProcessor\.js', import\.meta\.url\)/);
  assert.match(recorderSource, /WORKLET_MAX_CREDITS = 4/);
  assert.match(recorderSource, /WORKLET_MAX_PENDING_CHUNKS = 8/);
  assert.match(recorderSource, /new Uint8Array\(message\.buffer\)/);
  assert.match(recorderSource, /worklet\.onprocessorerror/);
  assert.match(recorderSource, /worklet\.port\.close\(\)/);
  assert.match(recorderSource, /TerminalPcmDelivery/);

  const realtimeNativeSource = fs.readFileSync(
    path.join(root, 'src-tauri', 'src', 'commands', 'realtime_transcription.rs'),
    'utf8',
  );
  assert.match(realtimeNativeSource, /REPLAY_CHUNK_MILLIS: u64 = 100/);
  assert.match(realtimeNativeSource, /wait_for_session_ready\(&mut socket\)/);
  assert.match(realtimeNativeSource, /transcribe_realtime_spool/);
  assert.doesNotMatch(realtimeNativeSource, /std::fs::read\(wav_path\)/);
});

test('terminal PCM delivery never resumes after the first rejected chunk', () => {
  const { TerminalPcmDelivery } = loadTerminalPcmDelivery();
  const delivered = [];
  let accepting = true;
  const delivery = new TerminalPcmDelivery((chunk) => {
    delivered.push(chunk[0]);
    return accepting;
  });

  assert.equal(delivery.deliver(new Uint8Array([1])), 'accepted');
  accepting = false;
  assert.equal(delivery.deliver(new Uint8Array([2])), 'rejected');
  accepting = true;
  assert.equal(delivery.deliver(new Uint8Array([3])), 'discarded');
  assert.deepEqual(delivered, [1, 2]);
  assert.equal(delivery.isTerminal, true);
  assert.equal(delivery.terminate(), false);
});

test('terminal PCM delivery contains callback failures and explicit cancellation', () => {
  let attempts = 0;
  const { TerminalPcmDelivery } = loadTerminalPcmDelivery();
  const failingDelivery = new TerminalPcmDelivery(() => {
    attempts += 1;
    throw new Error('native append failed');
  });

  assert.equal(failingDelivery.deliver(new Uint8Array([1])), 'rejected');
  assert.equal(failingDelivery.deliver(new Uint8Array([2])), 'discarded');
  assert.equal(attempts, 1);

  const cancelledDelivery = new TerminalPcmDelivery(() => true);
  assert.equal(cancelledDelivery.terminate(), true);
  assert.equal(cancelledDelivery.deliver(new Uint8Array([3])), 'discarded');
});

test('audio worklet transfers fixed PCM16 chunks behind explicit credits', () => {
  const VoicePcmProcessor = loadVoicePcmProcessor();
  const processor = new VoicePcmProcessor({
    processorOptions: {
      targetSampleRate: 48_000,
      chunkFrames: 4,
      maxCredits: 1,
      maxPendingChunks: 2,
    },
  });
  processor.process([[Float32Array.from([
    -1, -0.5, 0, 0.5, 1, 0.25, 0, -0.25, 0.75, 0.5, 0.25, 0, -0.5,
  ])]]);

  let pcmMessages = processor.port.messages.filter(({ message }) => message.type === 'pcm');
  assert.equal(pcmMessages.length, 1);
  const firstChunk = pcmMessages[0].message.buffer;
  assert.equal(new Int16Array(firstChunk).length, 4);
  const firstChunkView = new DataView(firstChunk);
  assert.deepEqual(
    Array.from({ length: 4 }, (_, index) => firstChunkView.getInt16(index * 2, true)),
    [-32768, -16384, 0, 16384],
  );
  assert.equal(pcmMessages[0].transfer.length, 1);
  assert.equal(pcmMessages[0].transfer[0], pcmMessages[0].message.buffer);
  assert.equal(processor.pendingChunks.length, 2);

  processor.port.onmessage({ data: { type: 'ack' } });
  processor.port.onmessage({ data: { type: 'ack' } });
  pcmMessages = processor.port.messages.filter(({ message }) => message.type === 'pcm');
  assert.equal(pcmMessages.length, 3);
  assert.equal(processor.pendingChunks.length, 0);
});

test('audio worklet overflow is bounded and becomes terminal', () => {
  const VoicePcmProcessor = loadVoicePcmProcessor();
  const processor = new VoicePcmProcessor({
    processorOptions: {
      targetSampleRate: 48_000,
      chunkFrames: 4,
      maxCredits: 1,
      maxPendingChunks: 1,
    },
  });
  const samples = Float32Array.from({ length: 17 }, (_, index) => (index % 4) / 4);
  processor.process([[samples]]);
  assert.equal(
    processor.port.messages.filter(({ message }) => message.type === 'overflow').length,
    1,
  );
  const messageCount = processor.port.messages.length;
  processor.process([[samples]]);
  assert.equal(processor.port.messages.length, messageCount);
});

test('audio worklet pause flushes its partial chunk before suspending capture', () => {
  const VoicePcmProcessor = loadVoicePcmProcessor();
  const processor = new VoicePcmProcessor({
    processorOptions: {
      targetSampleRate: 48_000,
      chunkFrames: 4,
      maxCredits: 2,
      maxPendingChunks: 2,
    },
  });
  processor.process([[Float32Array.from([0, 0.25, 0.5])]]);
  assert.equal(processor.port.messages.length, 0);

  processor.port.onmessage({ data: { type: 'flush', requestId: 7, pauseAfter: true } });
  const pcmMessage = processor.port.messages.find(({ message }) => message.type === 'pcm');
  assert.ok(pcmMessage);
  assert.equal(new Int16Array(pcmMessage.message.buffer).length, 2);
  assert.equal(
    processor.port.messages.some(({ message }) => message.type === 'flushed'),
    false,
  );

  processor.process([[Float32Array.from([0.75, 1, 0.5, 0])]]);
  assert.equal(
    processor.port.messages.filter(({ message }) => message.type === 'pcm').length,
    1,
  );
  processor.port.onmessage({ data: { type: 'ack' } });
  assert.equal(
    processor.port.messages.some(({ message }) => message.type === 'flushed' && message.requestId === 7),
    true,
  );
});

test('audio worklet resampling preserves phase across render quanta', () => {
  const VoicePcmProcessor = loadVoicePcmProcessor();
  const createProcessor = () => new VoicePcmProcessor({
    processorOptions: {
      targetSampleRate: 24_000,
      chunkFrames: 256,
      maxCredits: 4,
      maxPendingChunks: 4,
    },
  });
  const samples = Float32Array.from(
    { length: 257 },
    (_, index) => Math.sin(index / 13) * 0.75,
  );
  const onePass = createProcessor();
  const split = createProcessor();
  onePass.process([[samples]]);
  split.process([[samples.slice(0, 128)]]);
  split.process([[samples.slice(128)]]);
  onePass.port.onmessage({ data: { type: 'flush', requestId: 1 } });
  split.port.onmessage({ data: { type: 'flush', requestId: 2 } });

  const collectSamples = (processor) => processor.port.messages
    .filter(({ message }) => message.type === 'pcm')
    .flatMap(({ message }) => Array.from(new Int16Array(message.buffer)));
  assert.deepEqual(collectSamples(split), collectSamples(onePass));
});

test('audio worklet keeps live ownership bounded through 30 and 60 logical minutes', () => {
  const VoicePcmProcessor = loadVoicePcmProcessor();
  const processor = new VoicePcmProcessor({
    processorOptions: {
      targetSampleRate: 24_000,
      chunkFrames: 480,
      maxCredits: 4,
      maxPendingChunks: 8,
    },
  });
  const sourceFramesPerBatch = 3_840;
  const source = new Float32Array(sourceFramesPerBatch);
  const batchesPerMinute = 60 * 48_000 / sourceFramesPerBatch;
  const totalBatches = 60 * batchesPerMinute;
  const checkpoints = new Set([30 * batchesPerMinute, totalBatches]);
  let emittedChunks = 0;
  let maxInFlightChunks = 0;
  let maxPendingChunks = 0;

  for (let batch = 1; batch <= totalBatches; batch += 1) {
    processor.process([[source]]);
    const messages = processor.port.messages.splice(0);
    const pcmMessages = messages.filter(({ message }) => message.type === 'pcm');
    assert.equal(messages.some(({ message }) => message.type === 'overflow'), false);
    maxInFlightChunks = Math.max(maxInFlightChunks, pcmMessages.length);
    maxPendingChunks = Math.max(maxPendingChunks, processor.pendingChunks.length);
    emittedChunks += pcmMessages.length;
    for (const { message } of pcmMessages) {
      assert.equal(message.buffer.byteLength, 480 * 2);
      processor.port.onmessage({ data: { type: 'ack' } });
    }

    if (checkpoints.has(batch)) {
      assert.equal(processor.pendingChunks.length, 0);
      assert.equal(processor.credits, processor.maxCredits);
      assert.equal(processor.overflowed, false);
      assert.equal(processor.port.messages.length, 0);
    }
  }

  assert.equal(emittedChunks, 60 * 60 * 50);
  assert.equal(maxInFlightChunks, 4);
  assert.equal(maxPendingChunks, 0);
});

test('realtime partial transcript retains only the newest bounded text', () => {
  const {
    MAX_VOICE_PARTIAL_CHARS,
    appendBoundedVoicePartial,
    replaceBoundedVoicePartial,
  } = loadBoundedVoicePartial();
  const current = 'a'.repeat(MAX_VOICE_PARTIAL_CHARS);
  const appended = appendBoundedVoicePartial(current, 'latest');
  assert.equal(appended.length, MAX_VOICE_PARTIAL_CHARS);
  assert.equal(appended.endsWith('latest'), true);

  const oversized = `old-${'b'.repeat(MAX_VOICE_PARTIAL_CHARS)}-new`;
  const replaced = replaceBoundedVoicePartial(oversized);
  assert.equal(replaced.length, MAX_VOICE_PARTIAL_CHARS);
  assert.equal(replaced.endsWith('-new'), true);
});

test('realtime transcript projection honors append deltas and empty replacement snapshots', () => {
  const { projectRealtimeTranscriptText } = loadVoiceInputRuntime();
  const baseEvent = {
    sessionId: 'session-1',
    sequence: 1,
    kind: 'interim',
  };
  assert.equal(projectRealtimeTranscriptText('hello', {
    ...baseEvent,
    update: 'appendDelta',
    text: ' world',
  }), 'hello world');
  assert.equal(projectRealtimeTranscriptText('stale hypothesis', {
    ...baseEvent,
    update: 'replaceSnapshot',
    text: '',
  }), '');
  assert.equal(projectRealtimeTranscriptText('interim', {
    ...baseEvent,
    kind: 'final',
    text: 'final transcript',
  }), 'final transcript');
});

test('untouched realtime dictation owns one replaceable composer span', () => {
  const { applyVoiceDictationEvent } = loadVoiceDraftProjection();
  let projection = applyVoiceDictationEvent('inspect', null, { kind: 'start' });
  projection = applyVoiceDictationEvent(projection.draft, projection.session, {
    kind: 'interim',
    text: 'the config',
  });
  assert.equal(projection.draft, 'inspect the config');

  projection = applyVoiceDictationEvent(projection.draft, projection.session, {
    kind: 'interim',
    text: 'the configuration',
  });
  assert.equal(projection.draft, 'inspect the configuration');

  projection = applyVoiceDictationEvent(projection.draft, projection.session, {
    kind: 'final',
    text: 'the full configuration',
  });
  assert.equal(projection.draft, 'inspect the full configuration');
  assert.equal(projection.session, null);
});

test('manual composer correction wins over later realtime revisions', () => {
  const { applyVoiceDictationEvent } = loadVoiceDraftProjection();
  let projection = applyVoiceDictationEvent('', null, { kind: 'start' });
  projection = applyVoiceDictationEvent(projection.draft, projection.session, {
    kind: 'interim',
    text: 'check the entire configuration',
  });
  const corrected = projection.draft.replace('entire', 'whole');

  projection = applyVoiceDictationEvent(corrected, projection.session, {
    kind: 'interim',
    text: 'check the entire configuration carefully',
  });
  assert.equal(projection.draft, 'check the whole configuration carefully');

  projection = applyVoiceDictationEvent(projection.draft, projection.session, {
    kind: 'final',
    text: 'check the complete configuration carefully',
  });
  assert.equal(projection.draft, 'check the whole configuration carefully');
  assert.equal(projection.session, null);
});

test('manual correction still accepts a proven tail after provider prefix revision', () => {
  const { applyVoiceDictationEvent } = loadVoiceDraftProjection();
  let projection = applyVoiceDictationEvent('', null, { kind: 'start' });
  projection = applyVoiceDictationEvent(projection.draft, projection.session, {
    kind: 'interim',
    text: 'check the entire configuration',
  });
  const corrected = projection.draft.replace('entire', 'whole');

  projection = applyVoiceDictationEvent(corrected, projection.session, {
    kind: 'interim',
    text: 'check the complete configuration carefully',
  });

  assert.equal(projection.draft, 'check the whole configuration carefully');
});

test('manual CJK correction retains proven no-space transcript extensions', () => {
  const { applyVoiceDictationEvent } = loadVoiceDraftProjection();
  let projection = applyVoiceDictationEvent('', null, { kind: 'start' });
  projection = applyVoiceDictationEvent(projection.draft, projection.session, {
    kind: 'interim',
    text: '今天天气',
  });

  projection = applyVoiceDictationEvent('明天天气', projection.session, {
    kind: 'interim',
    text: '今天天气很好',
  });
  assert.equal(projection.draft, '明天天气很好');

  projection = applyVoiceDictationEvent(projection.draft, projection.session, {
    kind: 'final',
    text: '今天天气很好啊',
  });
  assert.equal(projection.draft, '明天天气很好啊');
  assert.equal(projection.session, null);
});

test('manual Japanese correction retains proven no-space transcript extensions', () => {
  const { applyVoiceDictationEvent } = loadVoiceDraftProjection();
  let projection = applyVoiceDictationEvent('', null, { kind: 'start' });
  projection = applyVoiceDictationEvent(projection.draft, projection.session, {
    kind: 'interim',
    text: '今日は晴れ',
  });

  projection = applyVoiceDictationEvent('明日は晴れ', projection.session, {
    kind: 'final',
    text: '今日は晴れです',
  });
  assert.equal(projection.draft, '明日は晴れです');
  assert.equal(projection.session, null);
});

test('manual CJK rewrite rejects a provider suffix without a user-owned anchor', () => {
  const { applyVoiceDictationEvent } = loadVoiceDraftProjection();
  let projection = applyVoiceDictationEvent('', null, { kind: 'start' });
  projection = applyVoiceDictationEvent(projection.draft, projection.session, {
    kind: 'interim',
    text: '今天天气',
  });

  projection = applyVoiceDictationEvent('彻底改写', projection.session, {
    kind: 'final',
    text: '今天天气很好',
  });
  assert.equal(projection.draft, '彻底改写');
  assert.equal(projection.session, null);
});

test('manual correction attaches punctuation-only snapshot extensions directly', () => {
  const { applyVoiceDictationEvent } = loadVoiceDraftProjection();
  for (const punctuation of ['.', ',', '?', '！']) {
    let projection = applyVoiceDictationEvent('', null, { kind: 'start' });
    projection = applyVoiceDictationEvent(projection.draft, projection.session, {
      kind: 'interim',
      text: 'hello',
    });
    projection = applyVoiceDictationEvent('Hello', projection.session, {
      kind: 'final',
      text: `hello${punctuation}`,
    });
    assert.equal(projection.draft, `Hello${punctuation}`);
    assert.equal(projection.session, null);
  }
});

test('manual correction at the transcript tail does not duplicate a provider suffix', () => {
  const { applyVoiceDictationEvent } = loadVoiceDraftProjection();
  let projection = applyVoiceDictationEvent('', null, { kind: 'start' });
  projection = applyVoiceDictationEvent(projection.draft, projection.session, {
    kind: 'interim',
    text: 'turn on the light',
  });

  projection = applyVoiceDictationEvent('turn on the lights', projection.session, {
    kind: 'interim',
    text: 'turn on the lights in kitchen',
  });

  assert.equal(projection.draft, 'turn on the lights in kitchen');
});

test('manual word replacement rejects an unproven provider word fragment', () => {
  const { applyVoiceDictationEvent } = loadVoiceDraftProjection();
  let projection = applyVoiceDictationEvent('', null, { kind: 'start' });
  projection = applyVoiceDictationEvent(projection.draft, projection.session, {
    kind: 'interim',
    text: 'turn on the light',
  });

  projection = applyVoiceDictationEvent('turn on the lamp', projection.session, {
    kind: 'interim',
    text: 'turn on the lights in kitchen',
  });

  assert.equal(projection.draft, 'turn on the lamp');
});

test('voice cancel removes only an untouched provider-owned span', () => {
  const { applyVoiceDictationEvent } = loadVoiceDraftProjection();
  let untouched = applyVoiceDictationEvent('prefix', null, { kind: 'start' });
  untouched = applyVoiceDictationEvent(untouched.draft, untouched.session, {
    kind: 'interim',
    text: 'draft words',
  });
  untouched = applyVoiceDictationEvent(untouched.draft, untouched.session, { kind: 'cancel' });
  assert.equal(untouched.draft, 'prefix');

  let edited = applyVoiceDictationEvent('prefix', null, { kind: 'start' });
  edited = applyVoiceDictationEvent(edited.draft, edited.session, {
    kind: 'interim',
    text: 'draft words',
  });
  edited = applyVoiceDictationEvent(
    edited.draft.replace('draft', 'corrected'),
    edited.session,
    { kind: 'cancel' },
  );
  assert.equal(edited.draft, 'prefix corrected words');
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

await testAsync('native spool unmount preserves accepted audio instead of privacy-deleting it', async () => {
  const { NativeVoiceSpoolUpload } = loadNativeVoiceSpool();
  let cancelled = 0;
  const upload = new NativeVoiceSpoolUpload(
    { sessionId: 'unmount-id', sampleRate: 16_000, maxChunkBytes: 4, maxAudioBytes: 1024 },
    {
      append: async () => {},
      finish: async (sessionId) => ({
        sessionId,
        audioBytes: 2,
        durationMs: 1,
        sampleRate: 16_000,
        checksumSha256: 'abc',
        createdAtMs: 1,
        expiresAtMs: 2,
      }),
      cancel: async () => { cancelled += 1; },
    },
  );
  upload.enqueue(new Uint8Array([1, 0]));

  const descriptor = await upload.preserveAcceptedAudio();

  assert.equal(descriptor.sessionId, 'unmount-id');
  assert.equal(cancelled, 0);
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
