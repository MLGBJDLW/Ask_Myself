import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';

import * as api from '../../lib/api';
import { getModelStatus, invalidate as invalidateModelStatus } from '../../lib/modelStatusCache';
import type { SpeechToTextConfig } from '../../types/conversation';
import type { VideoConfig, WhisperModel } from '../../types/video';
import { useMicrophoneDevices } from './useMicrophoneDevices';
import { useVoiceRecorder } from './useVoiceRecorder';
import { BoundedAudioUploadQueue } from './boundedAudioQueue';

export type VoiceRuntimeErrorCode =
  | 'busy'
  | 'permission_denied'
  | 'recording_failed'
  | 'transcription_failed'
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
  const [partialTranscript, setPartialTranscript] = useState('');
  const realtimeSessionIdRef = useRef<string | null>(null);
  const realtimeUploadQueueRef = useRef<BoundedAudioUploadQueue | null>(null);
  const realtimeUploadErrorRef = useRef<string | null>(null);

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
      }
    }).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    }).catch(() => {
      // The listener is unavailable outside the Tauri runtime (for example in
      // isolated component tests); command failures still surface to callers.
    });
    return () => {
      disposed = true;
      unlisten?.();
      const sessionId = realtimeSessionIdRef.current;
      realtimeUploadQueueRef.current?.cancel();
      if (sessionId) void api.cancelRealtimeTranscription(sessionId);
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

  const transcribeWav = useCallback(async (wav: Uint8Array): Promise<VoiceRuntimeActionResult> => {
    setTranscribing(true);
    try {
      const transcript = normalizeTranscript(await api.transcribeAudioBuffer(wav));
      return transcript ? { status: 'transcribed', text: transcript } : { status: 'empty' };
    } catch (error) {
      return {
        status: 'error',
        code: 'transcription_failed',
        message: String(error),
      };
    } finally {
      setTranscribing(false);
    }
  }, []);

  const queueRealtimeAudio = useCallback((sessionId: string, chunk: Uint8Array) => {
    if (realtimeSessionIdRef.current !== sessionId) return;
    const queue = realtimeUploadQueueRef.current;
    if (!queue || realtimeUploadErrorRef.current) return;
    if (!queue.enqueue(chunk)) {
      const snapshot = queue.snapshot();
      realtimeUploadErrorRef.current = `Realtime audio backpressure limit reached (${snapshot.maxBufferedBytes} bytes buffered)`;
    }
  }, []);

  const finishRealtimeRecording = useCallback(async (): Promise<VoiceRuntimeActionResult> => {
    const sessionId = realtimeSessionIdRef.current;
    await recorder.stopRecording();
    if (!sessionId) return { status: 'empty' };

    setTranscribing(true);
    try {
      await realtimeUploadQueueRef.current?.flush();
      if (realtimeUploadErrorRef.current) {
        throw new Error(realtimeUploadErrorRef.current);
      }
      const transcript = normalizeTranscript(await api.finishRealtimeTranscription(sessionId));
      setPartialTranscript('');
      return transcript ? { status: 'transcribed', text: transcript } : { status: 'empty' };
    } catch (error) {
      void api.cancelRealtimeTranscription(sessionId);
      return {
        status: 'error',
        code: 'transcription_failed',
        message: String(error),
      };
    } finally {
      realtimeSessionIdRef.current = null;
      realtimeUploadQueueRef.current = null;
      realtimeUploadErrorRef.current = null;
      setTranscribing(false);
    }
  }, [recorder]);

  const toggleRecording = useCallback(async (): Promise<VoiceRuntimeActionResult> => {
    if (transcribing || recorder.isProcessing || whisperChecking) {
      return { status: 'error', code: 'busy' };
    }

    if (recorder.isRecording) {
      if (realtimeSessionIdRef.current) {
        return finishRealtimeRecording();
      }
      const wav = await recorder.stopRecording();
      return wav ? transcribeWav(wav) : { status: 'empty' };
    }

    const readinessError = await ensureSpeechProviderReadyForRecording();
    if (readinessError) return readinessError;

    try {
      const appConfig = await api.getAppConfig();
      if (isRealtimeTranscriptionConfig(appConfig.speechToText)) {
        const sessionId = await api.startRealtimeTranscription();
        realtimeSessionIdRef.current = sessionId;
        realtimeUploadQueueRef.current = new BoundedAudioUploadQueue(
          (chunk) => api.appendRealtimeTranscriptionAudio(sessionId, chunk),
        );
        realtimeUploadErrorRef.current = null;
        setPartialTranscript('');
        try {
          await recorder.startRecording({
            captureWav: false,
            targetSampleRate: 24_000,
            onPcmChunk: (chunk) => queueRealtimeAudio(sessionId, chunk),
          });
        } catch (error) {
          realtimeSessionIdRef.current = null;
          realtimeUploadQueueRef.current?.cancel();
          realtimeUploadQueueRef.current = null;
          void api.cancelRealtimeTranscription(sessionId);
          throw error;
        }
      } else {
        await recorder.startRecording();
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
    ensureSpeechProviderReadyForRecording,
    finishRealtimeRecording,
    queueRealtimeAudio,
    transcribeWav,
    transcribing,
    whisperChecking,
  ]);

  const busy = recorder.isProcessing || transcribing || whisperChecking;

  const cancelRecording = useCallback(() => {
    recorder.cancelRecording();
    const sessionId = realtimeSessionIdRef.current;
    realtimeSessionIdRef.current = null;
    realtimeUploadQueueRef.current?.cancel();
    realtimeUploadQueueRef.current = null;
    realtimeUploadErrorRef.current = null;
    setPartialTranscript('');
    if (sessionId) void api.cancelRealtimeTranscription(sessionId);
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
      isProcessing: recorder.isProcessing,
      isTranscribing: transcribing,
      busy,
      recordingDuration: recorder.recordingDuration,
      partialTranscript,
      analyser: recorder.analyser,
      toggleRecording,
      cancelRecording,
      transcribeWav,
      formatDuration: formatRecordingDuration,
    }),
    [
      busy,
      cancelRecording,
      deleteWhisperModel,
      downloadWhisperModel,
      microphones,
      partialTranscript,
      recorder,
      refreshWhisperReadiness,
      resetWhisperReadiness,
      toggleRecording,
      transcribeWav,
      transcribing,
      whisperChecking,
      whisperDownloading,
      whisperModelExists,
    ],
  );
}
