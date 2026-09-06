import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('nexa-locale', 'en');
    localStorage.setItem('nexa-mic-device-id', 'configured-microphone');

    const nowIso = new Date().toISOString();
    const conversation = {
      id: 'conv-voice-dock',
      title: 'Voice Dock',
      provider: 'open_ai',
      model: 'gpt-5-mini',
      systemPrompt: '',
      createdAt: nowIso,
      updatedAt: nowIso,
    };
    const otherConversation = {
      ...conversation,
      id: 'conv-voice-other',
      title: 'Other Voice Draft',
    };
    const config = {
      id: 'cfg-default',
      name: 'Default',
      provider: 'open_ai',
      apiKey: 'sk-test',
      baseUrl: 'https://api.openai.com/v1',
      model: 'gpt-5-mini',
      temperature: 0.3,
      maxTokens: 4096,
      contextWindow: 128000,
      isDefault: true,
      reasoningEnabled: null,
      thinkingBudget: null,
      reasoningEffort: null,
      maxIterations: null,
      summarizationModel: null,
      summarizationProvider: null,
      subagentAllowedTools: null,
      createdAt: nowIso,
      updatedAt: nowIso,
    };

    const callbackMap = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handlerId: number }>();
    let callbackSeq = 1;
    let listenerSeq = 1;
    let voiceSessionSequence = 0;
    const voiceCancelCalls: string[] = [];
    const voiceFinishCalls: string[] = [];
    const realtimeCancelCalls: string[] = [];
    const microphoneControl = {
      deferGrant: false,
      grantPending: false,
      deferExactFailure: false,
      exactFailurePending: false,
      deferWorkletModule: false,
      workletModulePending: false,
      deferAppConfig: false,
      appConfigPending: false,
      deferVoiceSpoolStart: false,
      voiceSpoolStartPending: false,
      deferRealtimeStart: false,
      realtimeStartPending: false,
      failRealtimeFinish: false,
      failVoiceSpoolCancel: false,
      voiceSpoolStartCalls: 0,
      realtimeStartCalls: 0,
      defaultRequestCalls: 0,
      stopCalls: 0,
      contextCloseCalls: 0,
      grant: () => {},
      rejectExact: () => {},
      grantWorkletModule: () => {},
      grantAppConfig: () => {},
      grantVoiceSpoolStart: () => {},
      grantRealtimeStart: () => {},
    };

    class FakeAudioNode {
      connect() { return this; }
      disconnect() {}
    }
    class FakeAnalyserNode extends FakeAudioNode {
      fftSize = 1024;
      getFloatTimeDomainData(samples: Float32Array) { samples.fill(0); }
    }
    class FakeTrack {
      label = 'Studio Microphone';
      onended: (() => void) | null = null;
      onmute: (() => void) | null = null;
      onunmute: (() => void) | null = null;
      stop() { microphoneControl.stopCalls += 1; }
    }
    class FakeWorkletNode extends FakeAudioNode {
      onprocessorerror: (() => void) | null = null;
      port = {
        onmessage: null as ((event: { data: unknown }) => void) | null,
        close: () => {},
        postMessage: (message: { type?: string; requestId?: number }) => {
          if (message.type === 'flush') {
            queueMicrotask(() => this.port.onmessage?.({
              data: { type: 'flushed', requestId: message.requestId },
            }));
          }
        },
      };
    }
    class FakeAudioContext {
      state: AudioContextState = 'running';
      sampleRate = 48_000;
      destination = new FakeAudioNode();
      audioWorklet = {
        addModule: async () => {
          if (!microphoneControl.deferWorkletModule) return;
          microphoneControl.workletModulePending = true;
          await new Promise<void>((resolve) => {
            microphoneControl.grantWorkletModule = resolve;
          });
          microphoneControl.workletModulePending = false;
        },
      };
      createMediaStreamSource() { return new FakeAudioNode(); }
      createAnalyser() { return new FakeAnalyserNode(); }
      createGain() {
        const node = new FakeAudioNode() as FakeAudioNode & { gain: { value: number } };
        node.gain = { value: 1 };
        return node;
      }
      async suspend() { this.state = 'suspended'; }
      async resume() { this.state = 'running'; }
      async close() {
        microphoneControl.contextCloseCalls += 1;
        this.state = 'closed';
      }
    }
    const track = new FakeTrack();
    (window as unknown as { __SET_VOICE_TRACK_MUTED__: (muted: boolean) => void })
      .__SET_VOICE_TRACK_MUTED__ = (muted) => {
        if (muted) track.onmute?.();
        else track.onunmute?.();
      };
    const mediaDevices = {
      getUserMedia: async (constraints?: MediaStreamConstraints) => {
        const deviceConstraint = typeof constraints?.audio === 'object'
          ? constraints.audio.deviceId
          : undefined;
        if (
          typeof deviceConstraint === 'object'
          && 'exact' in deviceConstraint
          && deviceConstraint.exact === 'configured-microphone'
        ) {
          if (microphoneControl.deferExactFailure) {
            microphoneControl.exactFailurePending = true;
            await new Promise<void>((resolve) => {
              microphoneControl.rejectExact = resolve;
            });
            microphoneControl.exactFailurePending = false;
          }
          throw new DOMException('Configured microphone is busy', 'NotReadableError');
        }
        microphoneControl.defaultRequestCalls += 1;
        if (microphoneControl.deferGrant) {
          microphoneControl.grantPending = true;
          await new Promise<void>((resolve) => {
            microphoneControl.grant = resolve;
          });
          microphoneControl.grantPending = false;
        }
        return {
          getTracks: () => [track],
          getAudioTracks: () => [track],
        };
      },
      enumerateDevices: async () => ([
        {
          deviceId: 'default',
          groupId: 'group-default',
          kind: 'audioinput',
          label: 'Studio Microphone',
          toJSON: () => ({}),
        },
        {
          deviceId: 'configured-microphone',
          groupId: 'group-configured',
          kind: 'audioinput',
          label: 'Configured Microphone',
          toJSON: () => ({}),
        },
      ]),
      addEventListener: () => {},
      removeEventListener: () => {},
    };
    Object.defineProperty(navigator, 'mediaDevices', { configurable: true, value: mediaDevices });
    Object.defineProperty(window, 'AudioContext', { configurable: true, value: FakeAudioContext });
    Object.defineProperty(window, 'AudioWorkletNode', { configurable: true, value: FakeWorkletNode });

    const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
    const invoke = async (cmd: string, args: Record<string, unknown> = {}) => {
      switch (cmd) {
        case 'get_wizard_state_cmd':
          return { completed: true };
        case 'plugin:event|listen': {
          const listenerId = listenerSeq++;
          listeners.set(listenerId, {
            event: String(args.event ?? ''),
            handlerId: Number(args.handler ?? 0),
          });
          return listenerId;
        }
        case 'plugin:event|unlisten':
          listeners.delete(Number(args.eventId ?? 0));
          return null;
        case 'list_agent_configs_cmd':
          return [clone(config)];
        case 'list_conversations_cmd':
          return [clone(conversation), clone(otherConversation)];
        case 'list_projects_cmd':
        case 'get_conversation_turns_cmd':
        case 'list_sources':
        case 'get_conversation_sources_cmd':
        case 'list_checkpoints_cmd':
        case 'list_voice_audio_spools_cmd':
          return [];
        case 'get_conversation_cmd': {
          const requested = String(args.id ?? '');
          return [clone(requested === otherConversation.id ? otherConversation : conversation), []];
        }
        case 'get_conversation_usage_snapshot_cmd':
          return null;
        case 'agent_chat_cmd':
          setTimeout(() => {
            for (const [id, listener] of listeners) {
              if (listener.event === 'agent://run-event') callbackMap.get(listener.handlerId)?.({
                event: listener.event, id,
                payload: { conversationId: conversation.id, type: 'thinking', content: 'Working on the request.' },
              });
            }
          }, 25);
          return null;
        case 'get_app_config_cmd': {
          if (microphoneControl.deferAppConfig) {
            microphoneControl.appConfigPending = true;
            await new Promise<void>((resolve) => {
              microphoneControl.grantAppConfig = () => {
                microphoneControl.deferAppConfig = false;
                resolve();
              };
            });
            microphoneControl.appConfigPending = false;
          }
          return {
            speechToText: {
              provider: 'open_ai',
              apiStyle: 'openai_realtime_transcription',
              apiKey: 'sk-voice-test',
              baseUrl: 'https://api.openai.com/v1',
              model: 'gpt-live-transcribe',
              language: 'en',
            },
          };
        }
        case 'start_voice_audio_spool_cmd': {
          microphoneControl.voiceSpoolStartCalls += 1;
          if (microphoneControl.deferVoiceSpoolStart) {
            microphoneControl.voiceSpoolStartPending = true;
            await new Promise<void>((resolve) => {
              microphoneControl.grantVoiceSpoolStart = () => {
                microphoneControl.deferVoiceSpoolStart = false;
                resolve();
              };
            });
            microphoneControl.voiceSpoolStartPending = false;
          }
          voiceSessionSequence += 1;
          return {
            sessionId: `voice-${voiceSessionSequence}`,
            sampleRate: Number(args.sampleRate ?? 24_000),
            maxChunkBytes: 262144,
            maxAudioBytes: 2147483648,
          };
        }
        case 'append_voice_audio_spool_cmd':
          return { sequence: 0, audioBytes: 0, durationMs: 0 };
        case 'finish_voice_audio_spool_cmd':
          voiceFinishCalls.push(String(args.sessionId ?? ''));
          return {
            sessionId: String(args.sessionId ?? `voice-${voiceSessionSequence}`),
            audioBytes: 0,
            durationMs: 0,
            sampleRate: 24_000,
            checksumSha256: 'test',
            createdAtMs: 1,
            expiresAtMs: 2,
            target: {
              provider: 'open_ai',
              apiStyle: 'openai_realtime_transcription',
              model: 'gpt-live-transcribe',
              configurationFingerprintSha256: 'test',
            },
          };
        case 'cancel_voice_audio_spool_cmd':
          voiceCancelCalls.push(String(args.sessionId ?? ''));
          if (microphoneControl.failVoiceSpoolCancel) {
            throw new Error('managed voice spool cleanup failed');
          }
          return null;
        case 'start_realtime_transcription_cmd':
          microphoneControl.realtimeStartCalls += 1;
          if (microphoneControl.deferRealtimeStart) {
            microphoneControl.realtimeStartPending = true;
            await new Promise<void>((resolve) => {
              microphoneControl.grantRealtimeStart = () => {
                microphoneControl.deferRealtimeStart = false;
                resolve();
              };
            });
            microphoneControl.realtimeStartPending = false;
          }
          return 'realtime-voice-test';
        case 'append_realtime_transcription_audio_cmd':
          return null;
        case 'cancel_realtime_transcription_cmd':
          realtimeCancelCalls.push(String(args.sessionId ?? ''));
          return null;
        case 'transcribe_voice_audio_spool_cmd':
          return { transcript: 'fallback voice transcript', cleanupPending: false };
        case 'finish_realtime_transcription_cmd':
          if (microphoneControl.failRealtimeFinish) {
            throw new Error('realtime finalization failed');
          }
          await new Promise((resolve) => setTimeout(resolve, 350));
          return 'final voice transcript';
        default:
          return null;
      }
    };

    (window as unknown as { __VOICE_CANCEL_CALLS__: string[] }).__VOICE_CANCEL_CALLS__ = voiceCancelCalls;
    (window as unknown as { __VOICE_FINISH_CALLS__: string[] }).__VOICE_FINISH_CALLS__ = voiceFinishCalls;
    (window as unknown as { __REALTIME_CANCEL_CALLS__: string[] })
      .__REALTIME_CANCEL_CALLS__ = realtimeCancelCalls;
    (window as unknown as { __VOICE_MICROPHONE_CONTROL__: typeof microphoneControl })
      .__VOICE_MICROPHONE_CONTROL__ = microphoneControl;
    (window as unknown as { __EMIT_TAURI_EVENT__: (event: string, payload: unknown) => void })
      .__EMIT_TAURI_EVENT__ = (event, payload) => {
        for (const [id, listener] of listeners) {
          if (listener.event !== event) continue;
          callbackMap.get(listener.handlerId)?.({ event, id, payload });
        }
      };
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
      invoke,
      transformCallback: (callback: (event: unknown) => void) => {
        const id = callbackSeq++;
        callbackMap.set(id, callback);
        return id;
      },
      unregisterCallback: (id: number) => callbackMap.delete(id),
      convertFileSrc: (filePath: string) => filePath,
    };
    (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: (_event: string, eventId: number) => listeners.delete(eventId),
    };
  });
});

test('dictation and next-turn Nexus controls remain usable during an active response', async ({ page }) => {
  await page.goto('/chat/conv-voice-dock');
  await page.getByTestId('chat-input-textarea').fill('Start a long task');
  await page.getByTestId('chat-send').click();
  await expect(page.getByTestId('chat-stop')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Start voice input' })).toBeEnabled();
  await page.getByRole('button', { name: 'Start voice input' }).click();
  await expect(page.getByTestId('voice-recording-dock')).toBeVisible();
  await page.getByTestId('chat-nexus-mode').click();
  await page.getByTestId('chat-nexus-confirm').click();
  await expect(page.getByTestId('chat-nexus-mode')).toHaveAttribute('aria-pressed', 'true');
  await expect(page.getByTestId('chat-stop')).toBeVisible();
  await page.evaluate(() => {
    (window as unknown as { __EMIT_TAURI_EVENT__: (event: string, payload: unknown) => void })
      .__EMIT_TAURI_EVENT__('speech-to-text:realtime', {
        sessionId: 'realtime-voice-test', kind: 'interim', update: 'replaceSnapshot', sequence: 1,
        text: 'Here is my next instruction',
      });
  });
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('Here is my next instruction');
});

test('recording dock retracts through intermediate layout heights', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await page.goto('/chat/conv-voice-dock');
  await page.getByRole('button', { name: 'Start voice input' }).click();
  await expect(page.getByTestId('voice-recording-dock')).toBeVisible();
  const heights = await page.evaluate(async () => {
    const dock = document.querySelector('[data-testid="voice-recording-dock"]')!;
    const container = dock.parentElement!;
    const initial = container.getBoundingClientRect().height;
    (dock.querySelector('[aria-label="Cancel and delete recording"]') as HTMLButtonElement).click();
    const samples: number[] = [];
    const start = performance.now();
    while (performance.now() - start < 450) {
      await new Promise(requestAnimationFrame);
      const height = container.isConnected ? container.getBoundingClientRect().height : 0;
      if (height > 0 && height < initial - 1) samples.push(Math.round(height));
    }
    return [...new Set(samples)];
  });
  expect(heights.length).toBeGreaterThan(2);
  await expect(page.getByTestId('voice-recording-dock')).toHaveCount(0);
});

test('recording dock exposes responsive live, pause, details, processing, and cancel states', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto('/chat/conv-voice-dock');

  await page.getByRole('button', { name: 'Start voice input' }).click();
  const dock = page.getByTestId('voice-recording-dock');
  await expect(dock).toBeVisible();
  await expect(dock).toHaveAttribute('data-state', 'online');
  await expect(dock).toContainText('en · gpt-live-transcribe');
  expect((await dock.boundingBox())?.width ?? 0).toBeGreaterThanOrEqual(420);
  await page.screenshot({ path: testInfo.outputPath('voice-dock-desktop.png') });

  const waveform = page.getByTestId('microphone-waveform');
  await expect(waveform).toHaveAttribute('data-animated', 'false');

  await page.evaluate(() => {
    (window as unknown as { __EMIT_TAURI_EVENT__: (event: string, payload: unknown) => void })
      .__EMIT_TAURI_EVENT__('speech-to-text:realtime', {
        sessionId: 'realtime-voice-test',
        kind: 'interim',
        update: 'replaceSnapshot',
        sequence: 1,
        text: 'check the entire configuration',
      });
  });
  await expect(page.getByTestId('voice-partial-transcript')).toContainText('check the entire configuration');
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('check the entire configuration');

  await page.evaluate(() => {
    (window as unknown as { __EMIT_TAURI_EVENT__: (event: string, payload: unknown) => void })
      .__EMIT_TAURI_EVENT__('speech-to-text:realtime', {
        sessionId: 'realtime-voice-test',
        kind: 'interim',
        update: 'replaceSnapshot',
        sequence: 2,
        text: '',
      });
  });
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('');

  await page.evaluate(() => {
    (window as unknown as { __EMIT_TAURI_EVENT__: (event: string, payload: unknown) => void })
      .__EMIT_TAURI_EVENT__('speech-to-text:realtime', {
        sessionId: 'realtime-voice-test',
        kind: 'interim',
        update: 'replaceSnapshot',
        sequence: 3,
        text: 'check the entire configuration',
      });
  });
  await expect(page.getByTestId('voice-partial-transcript')).toContainText('check the entire configuration');
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('check the entire configuration');

  await page.getByTestId('chat-input-textarea').fill('check the whole configuration');
  await page.evaluate(() => {
    (window as unknown as { __EMIT_TAURI_EVENT__: (event: string, payload: unknown) => void })
      .__EMIT_TAURI_EVENT__('speech-to-text:realtime', {
        sessionId: 'realtime-voice-test',
        kind: 'interim',
        update: 'replaceSnapshot',
        sequence: 4,
        text: 'check the complete configuration carefully',
      });
  });
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('check the whole configuration carefully');

  await page.evaluate(() => {
    (window as unknown as { __SET_VOICE_TRACK_MUTED__: (muted: boolean) => void })
      .__SET_VOICE_TRACK_MUTED__(true);
  });
  await expect(dock).toHaveAttribute('data-state', 'buffering');
  await page.evaluate(() => {
    (window as unknown as { __SET_VOICE_TRACK_MUTED__: (muted: boolean) => void })
      .__SET_VOICE_TRACK_MUTED__(false);
  });
  await expect(dock).toHaveAttribute('data-state', 'online');

  await page.getByRole('button', { name: 'Pause recording' }).click();
  await expect(dock).toHaveAttribute('data-state', 'paused');
  await page.getByRole('button', { name: 'Resume recording' }).click();
  await expect(dock).toHaveAttribute('data-state', 'online');

  await page.getByRole('button', { name: 'Audio input details' }).click();
  await expect(dock).toContainText('Studio Microphone');
  await expect(dock).not.toContainText('Configured Microphone');
  await expect(dock).toContainText('Local protected spool');

  await page.setViewportSize({ width: 700, height: 800 });
  await expect.poll(() => dock.evaluate((element) => {
    const dockWidth = element.getBoundingClientRect().width;
    const toolbarWidth = element.closest('[data-testid="chat-input-toolbar"]')?.getBoundingClientRect().width ?? 0;
    return Math.abs(toolbarWidth - dockWidth - 20);
  })).toBeLessThan(8);
  await page.screenshot({ path: testInfo.outputPath('voice-dock-narrow.png') });
  await expect.poll(() => dock.evaluate((element) => {
    const bounds = element.getBoundingClientRect();
    return bounds.top >= 0 && bounds.bottom <= window.innerHeight;
  })).toBe(true);

  const stopTransition = await page.getByRole('button', { name: 'Stop & Transcribe' }).evaluate(
    async (button) => {
      const startedAt = performance.now();
      (button as HTMLButtonElement).click();
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      return {
        elapsedMs: performance.now() - startedAt,
        state: document.querySelector('[data-testid="voice-recording-dock"]')
          ?.getAttribute('data-state'),
      };
    },
  );
  expect(stopTransition.state).toBe('processing');
  expect(stopTransition.elapsedMs).toBeLessThan(300);
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('check the whole configuration carefully');
  await expect(dock).toHaveCount(0);

  await page.getByRole('button', { name: 'Start voice input' }).click();
  await expect(dock).toBeVisible();
  await page.getByRole('button', { name: 'Cancel and delete recording' }).click();
  await expect(dock).toHaveCount(0);
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __VOICE_CANCEL_CALLS__: string[] }).__VOICE_CANCEL_CALLS__,
  )).toContain('voice-2');
});

test('live dictation stays pinned to the draft where recording started', async ({ page }) => {
  await page.goto('/chat/conv-voice-dock');
  await page.getByRole('button', { name: 'Start voice input' }).click();
  await expect(page.getByTestId('voice-recording-dock')).toBeVisible();

  await page.evaluate(() => {
    (window as unknown as { __EMIT_TAURI_EVENT__: (event: string, payload: unknown) => void })
      .__EMIT_TAURI_EVENT__('speech-to-text:realtime', {
        sessionId: 'realtime-voice-test',
        kind: 'interim',
        update: 'replaceSnapshot',
        sequence: 1,
        text: 'owned by the first draft',
      });
  });
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('owned by the first draft');

  await page.getByRole('button', { name: /Other Voice Draft/ }).click();
  await expect(page).toHaveURL(/\/chat\/conv-voice-other$/);
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('');

  await page.evaluate(() => {
    (window as unknown as { __EMIT_TAURI_EVENT__: (event: string, payload: unknown) => void })
      .__EMIT_TAURI_EVENT__('speech-to-text:realtime', {
        sessionId: 'realtime-voice-test',
        kind: 'interim',
        update: 'replaceSnapshot',
        sequence: 2,
        text: 'owned by the first draft and continued',
      });
  });
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('');

  await page.getByRole('button', { name: /Voice Dock/ }).click();
  await expect(page.getByTestId('chat-input-textarea'))
    .toHaveValue('owned by the first draft and continued');
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('voice-recording-dock')).toHaveCount(0);
});

test('CJK live dictation preserves a manual correction and appends without spaces', async ({ page }) => {
  await page.goto('/chat/conv-voice-dock');
  await page.getByRole('button', { name: 'Start voice input' }).click();
  await expect(page.getByTestId('voice-recording-dock')).toBeVisible();

  await page.evaluate(() => {
    (window as unknown as { __EMIT_TAURI_EVENT__: (event: string, payload: unknown) => void })
      .__EMIT_TAURI_EVENT__('speech-to-text:realtime', {
        sessionId: 'realtime-voice-test',
        kind: 'interim',
        update: 'replaceSnapshot',
        sequence: 1,
        text: '今天天气',
      });
  });
  const composer = page.getByTestId('chat-input-textarea');
  await expect(composer).toHaveValue('今天天气');

  await composer.fill('明天天气');
  await page.evaluate(() => {
    (window as unknown as { __EMIT_TAURI_EVENT__: (event: string, payload: unknown) => void })
      .__EMIT_TAURI_EVENT__('speech-to-text:realtime', {
        sessionId: 'realtime-voice-test',
        kind: 'interim',
        update: 'replaceSnapshot',
        sequence: 2,
        text: '今天天气很好',
      });
  });
  await expect(composer).toHaveValue('明天天气很好');

  await page.evaluate(() => {
    (window as unknown as { __EMIT_TAURI_EVENT__: (event: string, payload: unknown) => void })
      .__EMIT_TAURI_EVENT__('speech-to-text:realtime', {
        sessionId: 'realtime-voice-test',
        kind: 'interim',
        update: 'replaceSnapshot',
        sequence: 3,
        text: '今天天气很好。',
      });
  });
  await expect(composer).toHaveValue('明天天气很好。');

  await page.keyboard.press('Escape');
  await expect(page.getByTestId('voice-recording-dock')).toHaveCount(0);
  await expect(composer).toHaveValue('明天天气很好。');
});

test('sending an interim transcript terminates dictation without repopulating the composer', async ({ page }) => {
  await page.goto('/chat/conv-voice-dock');
  await page.getByRole('button', { name: 'Start voice input' }).click();
  await expect(page.getByTestId('voice-recording-dock')).toBeVisible();

  await page.evaluate(() => {
    (window as unknown as { __EMIT_TAURI_EVENT__: (event: string, payload: unknown) => void })
      .__EMIT_TAURI_EVENT__('speech-to-text:realtime', {
        sessionId: 'realtime-voice-test',
        kind: 'interim',
        update: 'replaceSnapshot',
        sequence: 1,
        text: 'send this interim transcript',
      });
  });
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('send this interim transcript');

  await page.getByTestId('chat-send').click();
  await expect(page.getByTestId('voice-recording-dock')).toHaveCount(0);
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('');

  await page.evaluate(() => {
    (window as unknown as { __EMIT_TAURI_EVENT__: (event: string, payload: unknown) => void })
      .__EMIT_TAURI_EVENT__('speech-to-text:realtime', {
        sessionId: 'realtime-voice-test',
        kind: 'interim',
        update: 'replaceSnapshot',
        sequence: 2,
        text: 'send this interim transcript with a late suffix',
      });
  });
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('');
});

test('sending during transcription rejects the late finalized transcript', async ({ page }) => {
  await page.goto('/chat/conv-voice-dock');
  await page.getByRole('button', { name: 'Start voice input' }).click();
  await page.evaluate(() => {
    (window as unknown as { __EMIT_TAURI_EVENT__: (event: string, payload: unknown) => void })
      .__EMIT_TAURI_EVENT__('speech-to-text:realtime', {
        sessionId: 'realtime-voice-test',
        kind: 'interim',
        update: 'replaceSnapshot',
        sequence: 1,
        text: 'send while finalizing',
      });
  });
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('send while finalizing');

  await page.getByRole('button', { name: 'Stop & Transcribe' }).click();
  await expect(page.getByTestId('voice-recording-dock')).toHaveAttribute('data-state', 'processing');
  await page.getByTestId('chat-send').click();
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('');
  await expect(page.getByTestId('voice-recording-dock')).toHaveCount(0);
  await page.waitForTimeout(500);
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('');
});

test('batch fallback clears the stale realtime hypothesis before a draft switch', async ({ page }) => {
  await page.goto('/chat/conv-voice-dock');
  await page.evaluate(() => {
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { failRealtimeFinish: boolean };
    }).__VOICE_MICROPHONE_CONTROL__.failRealtimeFinish = true;
  });
  await page.getByRole('button', { name: 'Start voice input' }).click();
  await page.evaluate(() => {
    (window as unknown as { __EMIT_TAURI_EVENT__: (event: string, payload: unknown) => void })
      .__EMIT_TAURI_EVENT__('speech-to-text:realtime', {
        sessionId: 'realtime-voice-test',
        kind: 'interim',
        update: 'replaceSnapshot',
        sequence: 1,
        text: 'stale realtime hypothesis',
      });
  });
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('stale realtime hypothesis');

  await page.getByRole('button', { name: 'Stop & Transcribe' }).click();
  await expect(page.getByTestId('voice-recording-dock')).toHaveCount(0);
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('fallback voice transcript');

  await page.evaluate(() => {
    (window as unknown as { __EMIT_TAURI_EVENT__: (event: string, payload: unknown) => void })
      .__EMIT_TAURI_EVENT__('speech-to-text:realtime', {
        sessionId: 'realtime-voice-test',
        kind: 'interim',
        update: 'replaceSnapshot',
        sequence: 2,
        text: 'late queued realtime hypothesis',
      });
  });
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('fallback voice transcript');

  await page.getByRole('button', { name: /Other Voice Draft/ }).click();
  await expect(page).toHaveURL(/\/chat\/conv-voice-other$/);
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('');
  await page.getByRole('button', { name: /Voice Dock/ }).click();
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('fallback voice transcript');
});

test('realtime success clears its hypothesis even when private spool cleanup is deferred', async ({ page }) => {
  await page.goto('/chat/conv-voice-dock');
  await page.evaluate(() => {
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { failVoiceSpoolCancel: boolean };
    }).__VOICE_MICROPHONE_CONTROL__.failVoiceSpoolCancel = true;
  });
  await page.getByRole('button', { name: 'Start voice input' }).click();
  await page.evaluate(() => {
    (window as unknown as { __EMIT_TAURI_EVENT__: (event: string, payload: unknown) => void })
      .__EMIT_TAURI_EVENT__('speech-to-text:realtime', {
        sessionId: 'realtime-voice-test',
        kind: 'interim',
        update: 'replaceSnapshot',
        sequence: 1,
        text: 'stale successful hypothesis',
      });
  });
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('stale successful hypothesis');

  await page.getByRole('button', { name: 'Stop & Transcribe' }).click();
  await expect(page.getByTestId('voice-recording-dock')).toHaveCount(0);
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('final voice transcript');

  await page.getByRole('button', { name: /Other Voice Draft/ }).click();
  await expect(page).toHaveURL(/\/chat\/conv-voice-other$/);
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('');
  await page.getByRole('button', { name: /Voice Dock/ }).click();
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('final voice transcript');
});

test('cancelling during provider readiness never continues into voice resource startup', async ({ page }) => {
  await page.goto('/chat/conv-voice-dock');
  await page.evaluate(() => {
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { deferAppConfig: boolean };
    }).__VOICE_MICROPHONE_CONTROL__.deferAppConfig = true;
  });
  await page.getByRole('button', { name: 'Start voice input' }).click();
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { appConfigPending: boolean };
    }).__VOICE_MICROPHONE_CONTROL__.appConfigPending,
  )).toBe(true);

  await page.getByTestId('chat-input-textarea').fill('send while provider readiness is pending');
  await page.getByTestId('chat-send').click();
  await page.evaluate(() => {
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { grantAppConfig: () => void };
    }).__VOICE_MICROPHONE_CONTROL__.grantAppConfig();
  });

  await expect.poll(() => page.evaluate(() => {
    const control = (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: {
        appConfigPending: boolean;
        voiceSpoolStartCalls: number;
        defaultRequestCalls: number;
      };
    }).__VOICE_MICROPHONE_CONTROL__;
    return !control.appConfigPending
      && control.voiceSpoolStartCalls === 0
      && control.defaultRequestCalls === 0;
  })).toBe(true);
  await expect(page.getByTestId('voice-recording-dock')).toHaveCount(0);
  await expect(page.getByTestId('chat-input-textarea')).toHaveValue('');
});

test('cancelling a pending spool creation deletes the late spool before microphone startup', async ({ page }) => {
  await page.goto('/chat/conv-voice-dock');
  await page.evaluate(() => {
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { deferVoiceSpoolStart: boolean };
    }).__VOICE_MICROPHONE_CONTROL__.deferVoiceSpoolStart = true;
  });
  await page.getByRole('button', { name: 'Start voice input' }).click();
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { voiceSpoolStartPending: boolean };
    }).__VOICE_MICROPHONE_CONTROL__.voiceSpoolStartPending,
  )).toBe(true);

  await page.getByTestId('chat-input-textarea').fill('send while spool creation is pending');
  await page.getByTestId('chat-send').click();
  await page.evaluate(() => {
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { grantVoiceSpoolStart: () => void };
    }).__VOICE_MICROPHONE_CONTROL__.grantVoiceSpoolStart();
  });

  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __VOICE_CANCEL_CALLS__: string[] }).__VOICE_CANCEL_CALLS__,
  )).toContain('voice-1');
  const resourceStarts = await page.evaluate(() => {
    const control = (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: {
        realtimeStartCalls: number;
        defaultRequestCalls: number;
      };
    }).__VOICE_MICROPHONE_CONTROL__;
    return [control.realtimeStartCalls, control.defaultRequestCalls];
  });
  expect(resourceStarts).toEqual([0, 0]);
  await expect(page.getByTestId('voice-recording-dock')).toHaveCount(0);
});

test('unmounting during spool creation preserves the late spool without starting capture', async ({ page }) => {
  await page.goto('/chat/conv-voice-dock');
  await page.evaluate(() => {
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { deferVoiceSpoolStart: boolean };
    }).__VOICE_MICROPHONE_CONTROL__.deferVoiceSpoolStart = true;
  });
  await page.getByRole('button', { name: 'Start voice input' }).click();
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { voiceSpoolStartPending: boolean };
    }).__VOICE_MICROPHONE_CONTROL__.voiceSpoolStartPending,
  )).toBe(true);

  await page.getByRole('link', { name: 'Settings' }).click();
  await expect(page).toHaveURL(/\/settings$/);
  await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible();
  await page.evaluate(() => {
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { grantVoiceSpoolStart: () => void };
    }).__VOICE_MICROPHONE_CONTROL__.grantVoiceSpoolStart();
  });

  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __VOICE_FINISH_CALLS__: string[] }).__VOICE_FINISH_CALLS__,
  )).toContain('voice-1');
  expect(await page.evaluate(() =>
    (window as unknown as { __VOICE_CANCEL_CALLS__: string[] }).__VOICE_CANCEL_CALLS__,
  )).not.toContain('voice-1');
  expect(await page.evaluate(() =>
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { defaultRequestCalls: number };
    }).__VOICE_MICROPHONE_CONTROL__.defaultRequestCalls,
  )).toBe(0);
  await expect(page.getByTestId('voice-recording-dock')).toHaveCount(0);
});

test('cancelling a pending realtime session closes late resources before microphone startup', async ({ page }) => {
  await page.goto('/chat/conv-voice-dock');
  await page.evaluate(() => {
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { deferRealtimeStart: boolean };
    }).__VOICE_MICROPHONE_CONTROL__.deferRealtimeStart = true;
  });
  await page.getByRole('button', { name: 'Start voice input' }).click();
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { realtimeStartPending: boolean };
    }).__VOICE_MICROPHONE_CONTROL__.realtimeStartPending,
  )).toBe(true);

  await page.getByTestId('chat-input-textarea').fill('send while realtime startup is pending');
  await page.getByTestId('chat-send').click();
  await page.evaluate(() => {
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { grantRealtimeStart: () => void };
    }).__VOICE_MICROPHONE_CONTROL__.grantRealtimeStart();
  });

  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __REALTIME_CANCEL_CALLS__: string[] }).__REALTIME_CANCEL_CALLS__,
  )).toContain('realtime-voice-test');
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __VOICE_CANCEL_CALLS__: string[] }).__VOICE_CANCEL_CALLS__,
  )).toContain('voice-1');
  expect(await page.evaluate(() =>
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { defaultRequestCalls: number };
    }).__VOICE_MICROPHONE_CONTROL__.defaultRequestCalls,
  )).toBe(0);
  await expect(page.getByTestId('voice-recording-dock')).toHaveCount(0);
});

test('a delayed microphone grant is released after the chat recorder unmounts', async ({ page }) => {
  await page.goto('/chat/conv-voice-dock');
  await page.evaluate(() => {
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { deferGrant: boolean };
    }).__VOICE_MICROPHONE_CONTROL__.deferGrant = true;
  });

  await page.getByRole('button', { name: 'Start voice input' }).click();
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { grantPending: boolean };
    }).__VOICE_MICROPHONE_CONTROL__.grantPending,
  )).toBe(true);

  await page.getByRole('link', { name: 'Settings' }).click();
  await expect(page).toHaveURL(/\/settings$/);
  await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible();
  await page.evaluate(() => {
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { grant: () => void };
    }).__VOICE_MICROPHONE_CONTROL__.grant();
  });

  await expect.poll(() => page.evaluate(() =>
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { stopCalls: number };
    }).__VOICE_MICROPHONE_CONTROL__.stopCalls,
  )).toBeGreaterThanOrEqual(1);
  await expect(page.getByTestId('voice-recording-dock')).toHaveCount(0);
});

test('capture resources close while a stale worklet module is still loading', async ({ page }) => {
  await page.goto('/chat/conv-voice-dock');
  await page.evaluate(() => {
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { deferWorkletModule: boolean };
    }).__VOICE_MICROPHONE_CONTROL__.deferWorkletModule = true;
  });

  await page.getByRole('button', { name: 'Start voice input' }).click();
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { workletModulePending: boolean };
    }).__VOICE_MICROPHONE_CONTROL__.workletModulePending,
  )).toBe(true);

  await page.getByRole('link', { name: 'Settings' }).click();
  await expect(page).toHaveURL(/\/settings$/);
  await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible();
  await expect.poll(() => page.evaluate(() => {
    const control = (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { stopCalls: number; contextCloseCalls: number };
    }).__VOICE_MICROPHONE_CONTROL__;
    return control.stopCalls >= 1 && control.contextCloseCalls >= 1;
  })).toBe(true);

  await page.evaluate(() => {
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { grantWorkletModule: () => void };
    }).__VOICE_MICROPHONE_CONTROL__.grantWorkletModule();
  });
  await expect(page.getByTestId('voice-recording-dock')).toHaveCount(0);
});

test('a stale selected-device failure never opens the default microphone', async ({ page }) => {
  await page.goto('/chat/conv-voice-dock');
  await page.evaluate(() => {
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { deferExactFailure: boolean };
    }).__VOICE_MICROPHONE_CONTROL__.deferExactFailure = true;
  });

  await page.getByRole('button', { name: 'Start voice input' }).click();
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { exactFailurePending: boolean };
    }).__VOICE_MICROPHONE_CONTROL__.exactFailurePending,
  )).toBe(true);

  await page.getByRole('link', { name: 'Settings' }).click();
  await expect(page).toHaveURL(/\/settings$/);
  await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible();
  await page.evaluate(() => {
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { rejectExact: () => void };
    }).__VOICE_MICROPHONE_CONTROL__.rejectExact();
  });

  await expect.poll(() => page.evaluate(() =>
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { exactFailurePending: boolean };
    }).__VOICE_MICROPHONE_CONTROL__.exactFailurePending,
  )).toBe(false);
  await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())));
  expect(await page.evaluate(() =>
    (window as unknown as {
      __VOICE_MICROPHONE_CONTROL__: { defaultRequestCalls: number };
    }).__VOICE_MICROPHONE_CONTROL__.defaultRequestCalls,
  )).toBe(0);
  await expect(page.getByTestId('voice-recording-dock')).toHaveCount(0);
});
