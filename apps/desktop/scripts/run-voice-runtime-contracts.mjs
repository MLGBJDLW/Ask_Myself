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
