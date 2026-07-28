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

function test(name, fn) {
  try {
    fn();
    console.log(`ok - ${name}`);
  } catch (error) {
    console.error(`not ok - ${name}`);
    throw error;
  }
}

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
