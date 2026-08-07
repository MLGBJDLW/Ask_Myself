import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';

import * as api from '../../lib/api';
import { getModelStatus, invalidate as invalidateModelStatus } from '../../lib/modelStatusCache';
import type { SpeechToTextConfig } from '../../types/conversation';
import type { VideoConfig, WhisperModel } from '../../types/video';
import { useMicrophoneDevices } from './useMicrophoneDevices';
import { useVoiceRecorder } from './useVoiceRecorder';
import { BoundedAudioUploadQueue } from './boundedAudioQueue';
import { NativeVoiceSpoolUpload } from './nativeVoiceSpool';

export type VoiceRuntimeErrorCode =
  | 'busy'
  | 'permission_denied'
  | 'recording_failed'
  | 'transcription_failed'
  | 'realtime_backpressure'
  | 'realtime_deferred'
  | 'speech_provider_not_configured'
  | 'whisper_check_failed'
  | 'whisper_model_missing';

export type VoiceRuntimeActionResult =
  | { status: 'started' }
  | { status: 'empty' }
  | { status: 'transcribed'; text: string }
  | { status: 'error'; code: VoiceRuntimeErrorCode; message?: string };

export interface UseVoiceInputRuntimeOptions {
  videoConfig?: VideoConfig | null;
}

function whisperStatusKey(config: VideoConfig): string {
  return JSON.stringify(config);
}

export function getWhisperReadiness(config: VideoConfig): Promise<boolean> {
  return getModelStatus('whisper', whisperStatusKey(config), () => api.checkWhisperModel(config));
}

export function invalidateWhisperReadiness(): void {
  invalidateModelStatus('whisper');
}

export function normalizeTranscript(text: string): string {
  return text.trim();
}

export function isRealtimeTranscriptionConfig(
  config?: SpeechToTextConfig | null,
): boolean {
  return config?.apiStyle === 'openai_realtime_transcription';
}

export function isSpeechToTextConfigured(config?: SpeechToTextConfig | null): boolean {
  if (!config || config.apiStyle === 'local_whisper') return true;
  if (
    config.apiStyle === 'openai_transcription'
    || config.apiStyle === 'openai_realtime_transcription'
    || config.apiStyle === 'dashscope_asr'
  ) {
    return Boolean(config.apiKey.trim() && config.baseUrl?.trim() && config.model.trim());
  }
  if (config.apiStyle === 'sherpa_onnx') {
    const common = Boolean(config.executablePath?.trim() && config.tokensPath?.trim());
    return config.sherpaModelFamily === 'zipformer'
      ? common && Boolean(config.encoderPath?.trim() && config.decoderPath?.trim() && config.joinerPath?.trim())
      : common && Boolean(config.modelPath?.trim());
  }
  return false;
}

export function formatRecordingDuration(seconds: number): string {
  const minutes = Math.floor(seconds / 60);
  const remainingSeconds = seconds % 60;
  return `${minutes}:${remainingSeconds.toString().padStart(2, '0')}`;
}

export function queuePendingVoiceSpool(ids: string[], sessionId: string): string[] {
  return ids.includes(sessionId) ? ids : [...ids, sessionId];
}

export function forgetPendingVoiceSpool(ids: string[], sessionId: string): string[] {
  return ids.filter((id) => id !== sessionId);
}

async function startManagedVoiceSpool(
  sampleRate: number,
  onFailure: () => void,
): Promise<NativeVoiceSpoolUpload> {
  const started = await api.startVoiceAudioSpool(sampleRate);
  return new NativeVoiceSpoolUpload(
    started,
    {
      append: api.appendVoiceAudioSpool,
      finish: api.finishVoiceAudioSpool,
      cancel: api.cancelVoiceAudioSpool,
    },
    {
      onBackpressure: onFailure,
      onError: onFailure,
    },
  );
}

function isPermissionDeniedError(error: unknown): boolean {
  return typeof DOMException !== 'undefined'
    && error instanceof DOMException
    && error.name === 'NotAllowedError';
}

export function withWhisperModel(config: VideoConfig, whisperModel: WhisperModel): VideoConfig {
  return { ...config, whisperModel };
}

export function useVoiceInputRuntime(options: UseVoiceInputRuntimeOptions = {}) {
  const microphones = useMicrophoneDevices();
  const recorder = useVoiceRecorder(microphones.selectedDeviceId);
  const [whisperModelExists, setWhisperModelExists] = useState<boolean | null>(null);
  const [whisperChecking, setWhisperChecking] = useState(false);
  const [whisperDownloading, setWhisperDownloading] = useState(false);
  const [transcribing, setTranscribing] = useState(false);
  const [safeStopping, setSafeStopping] = useState(false);
  const [partialTranscript, setPartialTranscript] = useState('');
  const [runtimeNotice, setRuntimeNotice] = useState<VoiceRuntimeErrorCode | null>(null);
  const [automaticResult, setAutomaticResult] = useState<VoiceRuntimeActionResult | null>(null);
  const realtimeSessionIdRef = useRef<string | null>(null);
  const realtimeUploadQueueRef = useRef<BoundedAudioUploadQueue | null>(null);
  const realtimeUploadErrorRef = useRef<string | null>(null);
  const realtimeAcceptingAudioRef = useRef(false);
  const realtimeFinishPromiseRef = useRef<Promise<VoiceRuntimeActionResult> | null>(null);
  const realtimeSafeStopHandlerRef = useRef<() => void>(() => {});
  const voiceSpoolUploadRef = useRef<NativeVoiceSpoolUpload | null>(null);
  const pendingVoiceSpoolIdsRef = useRef<string[]>([]);
  const voiceSpoolFinishPromiseRef = useRef<Promise<VoiceRuntimeActionResult> | null>(null);
  const voiceSpoolSafeStopHandlerRef = useRef<() => void>(() => {});

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<{
      sessionId: string;
      kind: 'delta' | 'completed' | 'error' | 'closed';
      text?: string;
      itemId?: string;
    }>('speech-to-text:realtime', (event) => {
      if (event.payload.sessionId !== realtimeSessionIdRef.current) return;
      if (event.payload.kind === 'delta' && event.payload.text) {
        setPartialTranscript((current) => current + event.payload.text);
      } else if (event.payload.kind === 'completed') {
        setPartialTranscript(event.payload.text ?? '');
      } else if (event.payload.kind === 'error') {
        realtimeUploadErrorRef.current = event.payload.text ?? 'Realtime transcription failed';
        realtimeSafeStopHandlerRef.current();
      }
    }).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    }).catch(() => {
      // The listener is unavailable outside the Tauri runtime (for example in
      // isolated component tests); command failures still surface to callers.
    });
    void api.listVoiceAudioSpools().then((spools) => {
      if (disposed) return;
      pendingVoiceSpoolIdsRef.current = spools.map((spool) => spool.sessionId);
    }).catch(() => {
      // Recovery discovery is best-effort outside Tauri and during shutdown.
    });
    return () => {
      disposed = true;
      unlisten?.();
      const sessionId = realtimeSessionIdRef.current;
      realtimeUploadQueueRef.current?.cancel();
      if (sessionId) void api.cancelRealtimeTranscription(sessionId);
      const voiceSpool = voiceSpoolUploadRef.current;
      voiceSpoolUploadRef.current = null;
      if (voiceSpool) void voiceSpool.cancel();
    };
  }, []);

  const resolveVideoConfig = useCallback(
    async (configOverride?: VideoConfig | null): Promise<VideoConfig> =>
      configOverride ?? options.videoConfig ?? api.getVideoConfig(),
    [options.videoConfig],
  );

  const refreshWhisperReadiness = useCallback(
    async (configOverride?: VideoConfig | null): Promise<boolean> => {
      setWhisperChecking(true);
      try {
        const config = await resolveVideoConfig(configOverride);
        const exists = await getWhisperReadiness(config);
        setWhisperModelExists(exists);
        return exists;
      } catch {
        setWhisperModelExists(false);
        return false;
      } finally {
        setWhisperChecking(false);
      }
    },
    [resolveVideoConfig],
  );

  const resetWhisperReadiness = useCallback(() => {
    setWhisperModelExists(null);
  }, []);

  const ensureWhisperReadyForRecording = useCallback(async (): Promise<VoiceRuntimeActionResult | null> => {
    setWhisperChecking(true);
    try {
      const config = await resolveVideoConfig();
      const exists = await getWhisperReadiness(config);
      setWhisperModelExists(exists);
      return exists ? null : { status: 'error', code: 'whisper_model_missing' };
    } catch (error) {
      setWhisperModelExists(false);
      return {
        status: 'error',
        code: 'whisper_check_failed',
        message: String(error),
      };
    } finally {
      setWhisperChecking(false);
    }
  }, [resolveVideoConfig]);

  const ensureSpeechProviderReadyForRecording = useCallback(async (): Promise<VoiceRuntimeActionResult | null> => {
    try {
      const appConfig = await api.getAppConfig();
      const speechConfig = appConfig.speechToText;
      if (!isSpeechToTextConfigured(speechConfig)) {
        return { status: 'error', code: 'speech_provider_not_configured' };
      }
      if (speechConfig?.apiStyle !== 'local_whisper') return null;
      return ensureWhisperReadyForRecording();
    } catch (error) {
      return {
        status: 'error',
        code: 'whisper_check_failed',
        message: String(error),
      };
    }
  }, [ensureWhisperReadyForRecording]);

  const downloadWhisperModel = useCallback(
    async (configOverride?: VideoConfig | null): Promise<void> => {
      const config = await resolveVideoConfig(configOverride);
      setWhisperDownloading(true);
      try {
        await api.downloadWhisperModel(config);
        invalidateWhisperReadiness();
        setWhisperModelExists(true);
      } finally {
        setWhisperDownloading(false);
      }
    },
    [resolveVideoConfig],
  );

  const deleteWhisperModel = useCallback(async (): Promise<void> => {
    await api.deleteWhisperModel();
    invalidateWhisperReadiness();
    setWhisperModelExists(false);
  }, []);

  const transcribeManagedVoiceSpool = useCallback(async (
    sessionId: string,
  ): Promise<VoiceRuntimeActionResult> => {
    setTranscribing(true);
    try {
      const transcript = normalizeTranscript(await api.transcribeVoiceAudioSpool(sessionId));
      pendingVoiceSpoolIdsRef.current = forgetPendingVoiceSpool(
        pendingVoiceSpoolIdsRef.current,
        sessionId,
      );
      return transcript ? { status: 'transcribed', text: transcript } : { status: 'empty' };
    } catch (error) {
      pendingVoiceSpoolIdsRef.current = queuePendingVoiceSpool(
        pendingVoiceSpoolIdsRef.current,
        sessionId,
      );
      return {
        status: 'error',
        code: 'transcription_failed',
        message: String(error),
      };
    } finally {
      setTranscribing(false);
    }
  }, []);

  const finishRealtimeRecording = useCallback(async (): Promise<VoiceRuntimeActionResult> => {
    if (realtimeFinishPromiseRef.current) return realtimeFinishPromiseRef.current;

    const sessionId = realtimeSessionIdRef.current;
    const uploadQueue = realtimeUploadQueueRef.current;
    const voiceSpool = voiceSpoolUploadRef.current;
    realtimeAcceptingAudioRef.current = false;
    const finishPromise = (async (): Promise<VoiceRuntimeActionResult> => {
      try {
        await recorder.stopRecording();
      } catch (error) {
        if (sessionId) void api.cancelRealtimeTranscription(sessionId);
        if (voiceSpool) void voiceSpool.cancel();
        if (realtimeSessionIdRef.current === sessionId) {
          realtimeSessionIdRef.current = null;
          realtimeUploadQueueRef.current?.cancel();
          realtimeUploadQueueRef.current = null;
          realtimeUploadErrorRef.current = null;
        }
        return {
          status: 'error',
          code: 'recording_failed',
          message: String(error),
        };
      }
      if (!voiceSpool) return { status: 'empty' };

      setTranscribing(true);
      let descriptor: api.VoiceAudioSpoolDescriptor;
      try {
        descriptor = await voiceSpool.finish();
      } catch (error) {
        await voiceSpool.cancel().catch(() => {});
        if (sessionId) void api.cancelRealtimeTranscription(sessionId);
        setTranscribing(false);
        return {
          status: 'error',
          code: 'recording_failed',
          message: String(error),
        };
      }
      if (voiceSpoolUploadRef.current === voiceSpool) voiceSpoolUploadRef.current = null;
      pendingVoiceSpoolIdsRef.current = queuePendingVoiceSpool(
        pendingVoiceSpoolIdsRef.current,
        descriptor.sessionId,
      );

      if (!sessionId || realtimeUploadErrorRef.current) {
        return transcribeManagedVoiceSpool(descriptor.sessionId);
      }
      let transcript: string;
      try {
        await uploadQueue?.flush();
        if (realtimeUploadErrorRef.current) {
          throw new Error(realtimeUploadErrorRef.current);
        }
        transcript = normalizeTranscript(await api.finishRealtimeTranscription(sessionId));
      } catch {
        void api.cancelRealtimeTranscription(sessionId);
        return transcribeManagedVoiceSpool(descriptor.sessionId);
      }
      try {
        await api.cancelVoiceAudioSpool(descriptor.sessionId);
        pendingVoiceSpoolIdsRef.current = forgetPendingVoiceSpool(
          pendingVoiceSpoolIdsRef.current,
          descriptor.sessionId,
        );
        setPartialTranscript('');
        return transcript ? { status: 'transcribed', text: transcript } : { status: 'empty' };
      } catch (error) {
        const ready = await api.listVoiceAudioSpools().catch(() => null);
        if (ready && !ready.some((spool) => spool.sessionId === descriptor.sessionId)) {
          pendingVoiceSpoolIdsRef.current = forgetPendingVoiceSpool(
            pendingVoiceSpoolIdsRef.current,
            descriptor.sessionId,
          );
        }
        return {
          status: 'error',
          code: 'transcription_failed',
          message: `Realtime transcription succeeded but managed audio cleanup is pending: ${String(error)}`,
        };
      } finally {
        if (realtimeSessionIdRef.current === sessionId) {
          realtimeSessionIdRef.current = null;
          realtimeUploadQueueRef.current = null;
          realtimeUploadErrorRef.current = null;
        }
        setTranscribing(false);
      }
    })();
    realtimeFinishPromiseRef.current = finishPromise;
    try {
      return await finishPromise;
    } finally {
      if (realtimeFinishPromiseRef.current === finishPromise) {
        realtimeFinishPromiseRef.current = null;
      }
    }
  }, [recorder, transcribeManagedVoiceSpool]);

  const degradeRealtimeToSpool = useCallback((showNotice: boolean) => {
    const sessionId = realtimeSessionIdRef.current;
    if (!sessionId && !realtimeAcceptingAudioRef.current) return;
    realtimeAcceptingAudioRef.current = false;
    realtimeSessionIdRef.current = null;
    realtimeUploadQueueRef.current?.cancel('Realtime provider degraded to native spool');
    realtimeUploadQueueRef.current = null;
    realtimeUploadErrorRef.current = 'Realtime provider degraded to native spool';
    setPartialTranscript('');
    if (showNotice) setRuntimeNotice('realtime_deferred');
    if (sessionId) void api.cancelRealtimeTranscription(sessionId);
  }, []);

  useEffect(() => {
    realtimeSafeStopHandlerRef.current = () => degradeRealtimeToSpool(true);
    return () => {
      realtimeSafeStopHandlerRef.current = () => {};
    };
  }, [degradeRealtimeToSpool]);

  const queueRealtimeAudio = useCallback((sessionId: string, chunk: Uint8Array) => {
    if (!realtimeAcceptingAudioRef.current || realtimeSessionIdRef.current !== sessionId) return;
    realtimeUploadQueueRef.current?.enqueue(chunk);
  }, []);

  const finishManagedRecording = useCallback(async (): Promise<VoiceRuntimeActionResult> => {
    if (voiceSpoolFinishPromiseRef.current) return voiceSpoolFinishPromiseRef.current;
    const upload = voiceSpoolUploadRef.current;
    const finishPromise = (async (): Promise<VoiceRuntimeActionResult> => {
      try {
        await recorder.stopRecording();
      } catch (error) {
        if (upload) void upload.cancel();
        if (voiceSpoolUploadRef.current === upload) voiceSpoolUploadRef.current = null;
        return {
          status: 'error',
          code: 'recording_failed',
          message: String(error),
        };
      }
      if (!upload) return { status: 'empty' };

      setTranscribing(true);
      let descriptor: api.VoiceAudioSpoolDescriptor;
      try {
        descriptor = await upload.finish();
      } catch (error) {
        await upload.cancel().catch(() => {});
        if (voiceSpoolUploadRef.current === upload) voiceSpoolUploadRef.current = null;
        setTranscribing(false);
        return {
          status: 'error',
          code: 'recording_failed',
          message: String(error),
        };
      }

      if (voiceSpoolUploadRef.current === upload) voiceSpoolUploadRef.current = null;
      pendingVoiceSpoolIdsRef.current = queuePendingVoiceSpool(
        pendingVoiceSpoolIdsRef.current,
        descriptor.sessionId,
      );
      return transcribeManagedVoiceSpool(descriptor.sessionId);
    })();
    voiceSpoolFinishPromiseRef.current = finishPromise;
    try {
      return await finishPromise;
    } finally {
      if (voiceSpoolFinishPromiseRef.current === finishPromise) {
        voiceSpoolFinishPromiseRef.current = null;
      }
    }
  }, [recorder, transcribeManagedVoiceSpool]);

  const stopVoiceSpoolSafely = useCallback(() => {
    if (!voiceSpoolUploadRef.current) return;
    setSafeStopping(true);
    const finish = realtimeSessionIdRef.current
      ? finishRealtimeRecording
      : finishManagedRecording;
    void finish()
      .then(setAutomaticResult)
      .finally(() => setSafeStopping(false));
  }, [finishManagedRecording, finishRealtimeRecording]);

  useEffect(() => {
    voiceSpoolSafeStopHandlerRef.current = stopVoiceSpoolSafely;
    return () => {
      voiceSpoolSafeStopHandlerRef.current = () => {};
    };
  }, [stopVoiceSpoolSafely]);

  const clearRuntimeNotice = useCallback(() => setRuntimeNotice(null), []);
  const clearAutomaticResult = useCallback(() => setAutomaticResult(null), []);

  const toggleRecording = useCallback(async (): Promise<VoiceRuntimeActionResult> => {
    if (transcribing || whisperChecking || safeStopping) {
      return { status: 'error', code: 'busy' };
    }

    if (recorder.isRecording) {
      if (realtimeSessionIdRef.current) {
        return finishRealtimeRecording();
      }
      return finishManagedRecording();
    }

    const pendingVoiceSpoolId = pendingVoiceSpoolIdsRef.current[0];
    if (pendingVoiceSpoolId) {
      return transcribeManagedVoiceSpool(pendingVoiceSpoolId);
    }

    const readinessError = await ensureSpeechProviderReadyForRecording();
    if (readinessError) return readinessError;

    try {
      const appConfig = await api.getAppConfig();
      const realtime = isRealtimeTranscriptionConfig(appConfig.speechToText);
      const sampleRate = realtime ? 24_000 : 16_000;
      const upload = await startManagedVoiceSpool(
        sampleRate,
        () => voiceSpoolSafeStopHandlerRef.current(),
      );
      voiceSpoolUploadRef.current = upload;
      setRuntimeNotice(null);
      setAutomaticResult(null);
      setPartialTranscript('');

      if (realtime) {
        let sessionId: string | null = null;
        try {
          sessionId = await api.startRealtimeTranscription();
          realtimeSessionIdRef.current = sessionId;
          realtimeUploadQueueRef.current = new BoundedAudioUploadQueue(
            (chunk) => api.appendRealtimeTranscriptionAudio(sessionId!, chunk),
            {
              bytesPerSecond: 24_000 * 2,
              maxBufferedDurationMs: 2_000,
              onRejected: () => degradeRealtimeToSpool(true),
              onError: (error) => {
                realtimeUploadErrorRef.current = error.message;
                degradeRealtimeToSpool(true);
              },
            },
          );
          realtimeUploadErrorRef.current = null;
          realtimeAcceptingAudioRef.current = true;
        } catch (error) {
          realtimeUploadErrorRef.current = String(error);
          setRuntimeNotice('realtime_deferred');
        }
        try {
          await recorder.startRecording({
            targetSampleRate: 24_000,
            onPcmChunk: (chunk) => {
              upload.enqueue(chunk);
              if (sessionId) queueRealtimeAudio(sessionId, chunk);
            },
          });
        } catch (error) {
          realtimeSessionIdRef.current = null;
          realtimeAcceptingAudioRef.current = false;
          realtimeUploadQueueRef.current?.cancel();
          realtimeUploadQueueRef.current = null;
          voiceSpoolUploadRef.current = null;
          void upload.cancel();
          if (sessionId) void api.cancelRealtimeTranscription(sessionId);
          throw error;
        }
      } else {
        try {
          await recorder.startRecording({
            targetSampleRate: 16_000,
            onPcmChunk: (chunk) => upload.enqueue(chunk),
          });
        } catch (error) {
          voiceSpoolUploadRef.current = null;
          void upload.cancel();
          throw error;
        }
      }
      return { status: 'started' };
    } catch (error) {
      return {
        status: 'error',
        code: isPermissionDeniedError(error) ? 'permission_denied' : 'recording_failed',
        message: String(error),
      };
    }
  }, [
    recorder,
    degradeRealtimeToSpool,
    ensureSpeechProviderReadyForRecording,
    finishManagedRecording,
    finishRealtimeRecording,
    queueRealtimeAudio,
    safeStopping,
    transcribeManagedVoiceSpool,
    transcribing,
    whisperChecking,
  ]);

  const busy = transcribing || whisperChecking || safeStopping;

  const cancelRecording = useCallback(() => {
    recorder.cancelRecording();
    const sessionId = realtimeSessionIdRef.current;
    realtimeSessionIdRef.current = null;
    realtimeAcceptingAudioRef.current = false;
    realtimeUploadQueueRef.current?.cancel();
    realtimeUploadQueueRef.current = null;
    realtimeUploadErrorRef.current = null;
    setRuntimeNotice(null);
    setAutomaticResult(null);
    setPartialTranscript('');
    if (sessionId) void api.cancelRealtimeTranscription(sessionId);
    const voiceSpool = voiceSpoolUploadRef.current;
    voiceSpoolUploadRef.current = null;
    if (voiceSpool) void voiceSpool.cancel();
  }, [recorder]);

  return useMemo(
    () => ({
      microphones,
      recorder,
      whisper: {
        modelExists: whisperModelExists,
        checking: whisperChecking,
        downloading: whisperDownloading,
        refresh: refreshWhisperReadiness,
        reset: resetWhisperReadiness,
        download: downloadWhisperModel,
        deleteModel: deleteWhisperModel,
      },
      isRecording: recorder.isRecording,
      isTranscribing: transcribing,
      busy,
      recordingDuration: recorder.recordingDuration,
      partialTranscript,
      runtimeNotice,
      automaticResult,
      clearRuntimeNotice,
      clearAutomaticResult,
      analyser: recorder.analyser,
      toggleRecording,
      cancelRecording,
      formatDuration: formatRecordingDuration,
    }),
    [
      busy,
      cancelRecording,
      deleteWhisperModel,
      downloadWhisperModel,
      microphones,
      partialTranscript,
      runtimeNotice,
      automaticResult,
      clearAutomaticResult,
      clearRuntimeNotice,
      recorder,
      refreshWhisperReadiness,
      resetWhisperReadiness,
      toggleRecording,
      transcribing,
      whisperChecking,
      whisperDownloading,
      whisperModelExists,
    ],
  );
}
