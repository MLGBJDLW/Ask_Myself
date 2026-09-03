import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';

import * as api from '../../lib/api';
import { getModelStatus, invalidate as invalidateModelStatus } from '../../lib/modelStatusCache';
import type { SpeechToTextConfig } from '../../types/conversation';
import { sttRuntimeCapabilities } from '../../lib/sttProviderPresets';
import type { VideoConfig, WhisperModel } from '../../types/video';
import { useMicrophoneDevices } from './useMicrophoneDevices';
import { useVoiceRecorder } from './useVoiceRecorder';
import {
  appendBoundedVoicePartial,
  replaceBoundedVoicePartial,
} from './boundedVoicePartial';
import { BoundedAudioUploadQueue } from './boundedAudioQueue';
import { NativeVoiceSpoolUpload } from './nativeVoiceSpool';

export type VoiceRuntimeErrorCode =
  | 'busy'
  | 'permission_denied'
  | 'recording_failed'
  | 'transcription_failed'
  | 'realtime_backpressure'
  | 'realtime_deferred'
  | 'voice_cleanup_pending'
  | 'speech_provider_not_configured'
  | 'whisper_check_failed'
  | 'whisper_model_missing';

export type VoiceTransportState =
  | 'local'
  | 'online'
  | 'buffering'
  | 'degraded'
  | 'offline'
  | 'processing';

export interface VoiceRecordingContext {
  providerLabel: string;
  language: string | null;
  realtime: boolean;
}

export interface RealtimeTranscriptEvent {
  sessionId: string;
  sequence: number;
  kind: 'interim' | 'final' | 'error' | 'closed';
  update?: 'appendDelta' | 'replaceSnapshot';
  text?: string;
  utteranceId?: string;
}

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

export function projectRealtimeTranscriptText(
  current: string,
  event: RealtimeTranscriptEvent,
): string {
  if (event.kind === 'final') {
    return replaceBoundedVoicePartial(event.text ?? '');
  }
  if (event.kind !== 'interim') return current;
  if (event.update === 'replaceSnapshot' && typeof event.text === 'string') {
    return replaceBoundedVoicePartial(event.text);
  }
  return event.text ? appendBoundedVoicePartial(current, event.text) : current;
}

export function isRealtimeTranscriptionConfig(
  config?: SpeechToTextConfig | null,
): boolean {
  const capabilities = sttRuntimeCapabilities(config);
  return capabilities.audioInput === 'chunkStream'
    && capabilities.transcriptDelivery === 'interimAndFinal';
}

export function isSpeechToTextConfigured(config?: SpeechToTextConfig | null): boolean {
  if (!config || config.apiStyle === 'local_whisper') return true;
  if (config.apiStyle === 'openai_realtime_transcription'
    || config.apiStyle === 'dashscope_realtime_asr') {
    return isRealtimeTranscriptionConfig(config)
      && Boolean(config.apiKey.trim() && config.baseUrl?.trim() && config.model.trim());
  }
  if (config.apiStyle === 'openai_transcription' || config.apiStyle === 'dashscope_asr') {
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
  const [starting, setStarting] = useState(false);
  const [safeStopping, setSafeStopping] = useState(false);
  const [partialTranscript, setPartialTranscript] = useState('');
  const [runtimeNotice, setRuntimeNotice] = useState<VoiceRuntimeErrorCode | null>(null);
  const [automaticResult, setAutomaticResult] = useState<VoiceRuntimeActionResult | null>(null);
  const [hasPendingVoiceSpool, setHasPendingVoiceSpool] = useState(false);
  const [transportState, setTransportState] = useState<VoiceTransportState>('local');
  const [recordingContext, setRecordingContext] = useState<VoiceRecordingContext | null>(null);
  const [activeMicrophoneLabel, setActiveMicrophoneLabel] = useState<string | null>(null);
  const realtimeSessionIdRef = useRef<string | null>(null);
  const realtimeUploadQueueRef = useRef<BoundedAudioUploadQueue | null>(null);
  const realtimeUploadErrorRef = useRef<string | null>(null);
  const realtimeAcceptingAudioRef = useRef(false);
  const realtimeFinishPromiseRef = useRef<Promise<VoiceRuntimeActionResult> | null>(null);
  const realtimeEventSequenceRef = useRef(0);
  const realtimeSafeStopHandlerRef = useRef<() => void>(() => {});
  const voiceSpoolUploadRef = useRef<NativeVoiceSpoolUpload | null>(null);
  const pendingVoiceSpoolIdsRef = useRef<string[]>([]);
  const pendingVoiceCleanupIdsRef = useRef<string[]>([]);
  const voiceSpoolFinishPromiseRef = useRef<Promise<VoiceRuntimeActionResult> | null>(null);
  const voiceSpoolSafeStopHandlerRef = useRef<() => void>(() => {});
  const startInProgressRef = useRef(false);
  const discardInProgressRef = useRef(false);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<RealtimeTranscriptEvent>('speech-to-text:realtime', (event) => {
      if (event.payload.sessionId !== realtimeSessionIdRef.current) return;
      if (event.payload.sequence <= realtimeEventSequenceRef.current) return;
      realtimeEventSequenceRef.current = event.payload.sequence;
      if (event.payload.kind === 'interim' || event.payload.kind === 'final') {
        setPartialTranscript((current) => projectRealtimeTranscriptText(current, event.payload));
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
      pendingVoiceSpoolIdsRef.current = spools
        .filter((spool) => spool.state === 'ready')
        .map((spool) => spool.sessionId);
      pendingVoiceCleanupIdsRef.current = spools
        .filter((spool) => spool.state === 'deletionPending')
        .map((spool) => spool.sessionId);
      setHasPendingVoiceSpool(
        pendingVoiceSpoolIdsRef.current.length > 0
          || pendingVoiceCleanupIdsRef.current.length > 0,
      );
      if (pendingVoiceCleanupIdsRef.current.length > 0) {
        setRuntimeNotice('voice_cleanup_pending');
      }
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
      if (voiceSpool) void voiceSpool.preserveAcceptedAudio().catch(() => {});
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
      setRecordingContext(null);
      setTransportState('local');
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
      const result = await api.transcribeVoiceAudioSpool(sessionId);
      const transcript = normalizeTranscript(result.transcript);
      pendingVoiceSpoolIdsRef.current = forgetPendingVoiceSpool(
        pendingVoiceSpoolIdsRef.current,
        sessionId,
      );
      if (result.cleanupPending) {
        pendingVoiceCleanupIdsRef.current = queuePendingVoiceSpool(
          pendingVoiceCleanupIdsRef.current,
          sessionId,
        );
        setRuntimeNotice('voice_cleanup_pending');
      }
      setHasPendingVoiceSpool(
        pendingVoiceSpoolIdsRef.current.length > 0
          || pendingVoiceCleanupIdsRef.current.length > 0,
      );
      return transcript ? { status: 'transcribed', text: transcript } : { status: 'empty' };
    } catch (error) {
      const entries = await api.listVoiceAudioSpools().catch(() => null);
      const cleanupPending = entries?.some(
        (entry) => entry.sessionId === sessionId && entry.state === 'deletionPending',
      ) ?? false;
      if (cleanupPending) {
        pendingVoiceSpoolIdsRef.current = forgetPendingVoiceSpool(
          pendingVoiceSpoolIdsRef.current,
          sessionId,
        );
        pendingVoiceCleanupIdsRef.current = queuePendingVoiceSpool(
          pendingVoiceCleanupIdsRef.current,
          sessionId,
        );
        setRuntimeNotice('voice_cleanup_pending');
      } else {
        pendingVoiceSpoolIdsRef.current = queuePendingVoiceSpool(
          pendingVoiceSpoolIdsRef.current,
          sessionId,
        );
      }
      setHasPendingVoiceSpool(true);
      return {
        status: 'error',
        code: cleanupPending ? 'voice_cleanup_pending' : 'transcription_failed',
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
    const detachRealtimeProjection = (reason: string) => {
      uploadQueue?.cancel(reason);
      if (realtimeSessionIdRef.current === sessionId) {
        realtimeSessionIdRef.current = null;
        realtimeEventSequenceRef.current = 0;
        if (realtimeUploadQueueRef.current === uploadQueue) {
          realtimeUploadQueueRef.current = null;
        }
        realtimeUploadErrorRef.current = null;
      }
      setPartialTranscript('');
    };
    const finishPromise = (async (): Promise<VoiceRuntimeActionResult> => {
      try {
        await recorder.stopRecording();
      } catch (error) {
        detachRealtimeProjection('Realtime recording stopped with an error');
        if (sessionId) void api.cancelRealtimeTranscription(sessionId);
        if (voiceSpool) void voiceSpool.cancel();
        return {
          status: 'error',
          code: 'recording_failed',
          message: String(error),
        };
      }
      if (!voiceSpool) {
        detachRealtimeProjection('Realtime recording ended without a managed spool');
        if (sessionId) void api.cancelRealtimeTranscription(sessionId);
        return { status: 'empty' };
      }

      setTranscribing(true);
      let descriptor: api.VoiceAudioSpoolDescriptor;
      try {
        descriptor = await voiceSpool.finish();
      } catch (error) {
        try {
          descriptor = await voiceSpool.finishAcceptedAudio();
        } catch (recoveryError) {
          if (voiceSpoolUploadRef.current === voiceSpool) voiceSpoolUploadRef.current = null;
          detachRealtimeProjection('Realtime recording could not finalize accepted audio');
          if (sessionId) void api.cancelRealtimeTranscription(sessionId);
          setTranscribing(false);
          return {
            status: 'error',
            code: 'recording_failed',
            message: `${String(error)}. Accepted audio remains checkpointed: ${String(recoveryError)}`,
          };
        }
      }
      if (voiceSpoolUploadRef.current === voiceSpool) voiceSpoolUploadRef.current = null;
      pendingVoiceSpoolIdsRef.current = queuePendingVoiceSpool(
        pendingVoiceSpoolIdsRef.current,
        descriptor.sessionId,
      );
      setHasPendingVoiceSpool(true);
      const transcribeRealtimeFallback = () => {
        // The managed spool becomes the sole transcript authority once the
        // realtime terminal path fails. Detach the realtime actor before the
        // slower batch call so queued provider events cannot repopulate its
        // stale interim hypothesis or publish it into another draft later.
        detachRealtimeProjection('Realtime finalization fell back to managed voice spool');
        if (sessionId) void api.cancelRealtimeTranscription(sessionId);
        return transcribeManagedVoiceSpool(descriptor.sessionId);
      };

      if (!sessionId || realtimeUploadErrorRef.current) {
        return transcribeRealtimeFallback();
      }
      let transcript: string;
      try {
        await uploadQueue?.flush();
        if (realtimeUploadErrorRef.current) {
          throw new Error(realtimeUploadErrorRef.current);
        }
        transcript = normalizeTranscript(await api.finishRealtimeTranscription(sessionId));
        // The accepted terminal transcript becomes the sole text authority
        // before private-spool cleanup. Cleanup may be slow or retryable, but
        // it must never keep the final realtime hypothesis publishable.
        detachRealtimeProjection('Realtime transcription finalized');
      } catch {
        return transcribeRealtimeFallback();
      }
      try {
        await api.cancelVoiceAudioSpool(descriptor.sessionId);
        pendingVoiceSpoolIdsRef.current = forgetPendingVoiceSpool(
          pendingVoiceSpoolIdsRef.current,
          descriptor.sessionId,
        );
        setHasPendingVoiceSpool(
          pendingVoiceSpoolIdsRef.current.length > 0
            || pendingVoiceCleanupIdsRef.current.length > 0,
        );
        return transcript ? { status: 'transcribed', text: transcript } : { status: 'empty' };
      } catch {
        pendingVoiceSpoolIdsRef.current = forgetPendingVoiceSpool(
          pendingVoiceSpoolIdsRef.current,
          descriptor.sessionId,
        );
        pendingVoiceCleanupIdsRef.current = queuePendingVoiceSpool(
          pendingVoiceCleanupIdsRef.current,
          descriptor.sessionId,
        );
        setHasPendingVoiceSpool(true);
        setRuntimeNotice('voice_cleanup_pending');
        return transcript ? { status: 'transcribed', text: transcript } : { status: 'empty' };
      } finally {
        if (realtimeSessionIdRef.current === sessionId) {
          realtimeSessionIdRef.current = null;
          realtimeEventSequenceRef.current = 0;
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
    realtimeEventSequenceRef.current = 0;
    realtimeUploadQueueRef.current?.cancel('Realtime provider degraded to native spool');
    realtimeUploadQueueRef.current = null;
    realtimeUploadErrorRef.current = 'Realtime provider degraded to native spool';
    setPartialTranscript('');
    setTransportState('degraded');
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
        try {
          descriptor = await upload.finishAcceptedAudio();
        } catch (recoveryError) {
          if (voiceSpoolUploadRef.current === upload) voiceSpoolUploadRef.current = null;
          setTranscribing(false);
          return {
            status: 'error',
            code: 'recording_failed',
            message: `${String(error)}. Accepted audio remains checkpointed: ${String(recoveryError)}`,
          };
        }
      }

      if (voiceSpoolUploadRef.current === upload) voiceSpoolUploadRef.current = null;
      pendingVoiceSpoolIdsRef.current = queuePendingVoiceSpool(
        pendingVoiceSpoolIdsRef.current,
        descriptor.sessionId,
      );
      setHasPendingVoiceSpool(true);
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

  const finishActiveRecording = useCallback(async (): Promise<VoiceRuntimeActionResult> => {
    setSafeStopping(true);
    setTransportState('processing');
    const finish = realtimeSessionIdRef.current
      ? finishRealtimeRecording
      : finishManagedRecording;
    try {
      return await finish();
    } finally {
      setSafeStopping(false);
      setRecordingContext(null);
      setActiveMicrophoneLabel(null);
    }
  }, [finishManagedRecording, finishRealtimeRecording]);

  const stopVoiceSpoolSafely = useCallback(() => {
    if (!voiceSpoolUploadRef.current) return;
    void finishActiveRecording().then(setAutomaticResult);
  }, [finishActiveRecording]);

  useEffect(() => {
    voiceSpoolSafeStopHandlerRef.current = stopVoiceSpoolSafely;
    return () => {
      voiceSpoolSafeStopHandlerRef.current = () => {};
    };
  }, [stopVoiceSpoolSafely]);

  const clearRuntimeNotice = useCallback(() => setRuntimeNotice(null), []);
  const clearAutomaticResult = useCallback(() => setAutomaticResult(null), []);

  const toggleRecording = useCallback(async (): Promise<VoiceRuntimeActionResult> => {
    if (transcribing || whisperChecking || safeStopping || startInProgressRef.current) {
      return { status: 'error', code: 'busy' };
    }

    if (recorder.isRecording) {
      return finishActiveRecording();
    }

    startInProgressRef.current = true;
    setStarting(true);
    try {
      const pendingVoiceSpoolId = pendingVoiceSpoolIdsRef.current[0];
      if (pendingVoiceSpoolId) {
        return await transcribeManagedVoiceSpool(pendingVoiceSpoolId);
      }

      const readinessError = await ensureSpeechProviderReadyForRecording();
      if (readinessError) return readinessError;

      const appConfig = await api.getAppConfig();
      const speechConfig = appConfig.speechToText;
      const realtime = isRealtimeTranscriptionConfig(speechConfig);
      const sampleRate = sttRuntimeCapabilities(speechConfig).sampleRateHz;
      const handleCaptureStateChange = (state: 'capturing' | 'interrupted') => {
        if (state === 'interrupted') {
          setTransportState('buffering');
        } else {
          setTransportState(realtimeUploadErrorRef.current
            ? 'degraded'
            : realtime ? 'online' : 'local');
        }
      };
      const handleCaptureIssue = (state: 'interrupted' | 'disconnected') => {
        setTransportState(state === 'disconnected' ? 'offline' : 'buffering');
        voiceSpoolSafeStopHandlerRef.current();
      };
      setRecordingContext({
        providerLabel: speechConfig?.model?.trim()
          || speechConfig?.provider?.trim()
          || (realtime ? 'Realtime STT' : 'Local STT'),
        language: speechConfig?.language?.trim() || null,
        realtime,
      });
      setActiveMicrophoneLabel(null);
      setTransportState(realtime ? 'buffering' : 'local');
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
          realtimeEventSequenceRef.current = 0;
          realtimeUploadQueueRef.current = new BoundedAudioUploadQueue(
            (chunk) => api.appendRealtimeTranscriptionAudio(sessionId!, chunk),
            {
              bytesPerSecond: sampleRate * 2,
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
          setTransportState('online');
        } catch (error) {
          realtimeUploadErrorRef.current = String(error);
          setTransportState('degraded');
          setRuntimeNotice('realtime_deferred');
        }
        try {
          await recorder.startRecording({
            targetSampleRate: sampleRate,
            onPcmChunk: (chunk) => {
              const accepted = upload.enqueue(chunk);
              if (sessionId) queueRealtimeAudio(sessionId, chunk);
              return accepted;
            },
            onCaptureIssue: handleCaptureIssue,
            onCaptureStateChange: handleCaptureStateChange,
            onCaptureReady: ({ label }) => setActiveMicrophoneLabel(label),
          });
          void microphones.refresh();
        } catch (error) {
          realtimeSessionIdRef.current = null;
          realtimeEventSequenceRef.current = 0;
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
            targetSampleRate: sampleRate,
            onPcmChunk: (chunk) => upload.enqueue(chunk),
            onCaptureIssue: handleCaptureIssue,
            onCaptureStateChange: handleCaptureStateChange,
            onCaptureReady: ({ label }) => setActiveMicrophoneLabel(label),
          });
          void microphones.refresh();
        } catch (error) {
          voiceSpoolUploadRef.current = null;
          void upload.cancel();
          throw error;
        }
      }
      return { status: 'started' };
    } catch (error) {
      setRecordingContext(null);
      setActiveMicrophoneLabel(null);
      setTransportState('local');
      return {
        status: 'error',
        code: isPermissionDeniedError(error) ? 'permission_denied' : 'recording_failed',
        message: String(error),
      };
    } finally {
      startInProgressRef.current = false;
      setStarting(false);
    }
  }, [
    recorder,
    degradeRealtimeToSpool,
    ensureSpeechProviderReadyForRecording,
    finishActiveRecording,
    microphones,
    queueRealtimeAudio,
    safeStopping,
    transcribeManagedVoiceSpool,
    transcribing,
    whisperChecking,
  ]);

  const busy = transcribing || whisperChecking || safeStopping || starting;

  const discardPendingVoiceSpool = useCallback(async (): Promise<VoiceRuntimeActionResult> => {
    if (discardInProgressRef.current) return { status: 'error', code: 'busy' };
    const sessionId = pendingVoiceCleanupIdsRef.current[0]
      ?? pendingVoiceSpoolIdsRef.current[0];
    if (!sessionId) {
      setHasPendingVoiceSpool(false);
      return { status: 'empty' };
    }

    discardInProgressRef.current = true;
    setTranscribing(true);
    try {
      await api.cancelVoiceAudioSpool(sessionId);
      pendingVoiceSpoolIdsRef.current = forgetPendingVoiceSpool(
        pendingVoiceSpoolIdsRef.current,
        sessionId,
      );
      pendingVoiceCleanupIdsRef.current = forgetPendingVoiceSpool(
        pendingVoiceCleanupIdsRef.current,
        sessionId,
      );
      setHasPendingVoiceSpool(
        pendingVoiceSpoolIdsRef.current.length > 0
          || pendingVoiceCleanupIdsRef.current.length > 0,
      );
      return { status: 'empty' };
    } catch (error) {
      pendingVoiceSpoolIdsRef.current = forgetPendingVoiceSpool(
        pendingVoiceSpoolIdsRef.current,
        sessionId,
      );
      pendingVoiceCleanupIdsRef.current = queuePendingVoiceSpool(
        pendingVoiceCleanupIdsRef.current,
        sessionId,
      );
      setHasPendingVoiceSpool(true);
      setRuntimeNotice('voice_cleanup_pending');
      return {
        status: 'error',
        code: 'voice_cleanup_pending',
        message: String(error),
      };
    } finally {
      discardInProgressRef.current = false;
      setTranscribing(false);
    }
  }, []);

  const cancelRecording = useCallback(() => {
    recorder.cancelRecording();
    const sessionId = realtimeSessionIdRef.current;
    realtimeSessionIdRef.current = null;
    realtimeEventSequenceRef.current = 0;
    realtimeAcceptingAudioRef.current = false;
    realtimeUploadQueueRef.current?.cancel();
    realtimeUploadQueueRef.current = null;
    realtimeUploadErrorRef.current = null;
    setRuntimeNotice(null);
    setAutomaticResult(null);
    setPartialTranscript('');
    setRecordingContext(null);
    setActiveMicrophoneLabel(null);
    setTransportState('local');
    if (sessionId) void api.cancelRealtimeTranscription(sessionId);
    const voiceSpool = voiceSpoolUploadRef.current;
    voiceSpoolUploadRef.current = null;
    if (voiceSpool) {
      void voiceSpool.cancel().catch(() => setRuntimeNotice('voice_cleanup_pending'));
    }
  }, [recorder]);

  const toggleRecordingPause = useCallback(async (): Promise<VoiceRuntimeActionResult> => {
    if (!recorder.isRecording || transcribing || safeStopping) {
      return { status: 'error', code: 'busy' };
    }
    try {
      if (recorder.isPaused) await recorder.resumeRecording();
      else await recorder.pauseRecording();
      return { status: 'started' };
    } catch (error) {
      return {
        status: 'error',
        code: 'recording_failed',
        message: String(error),
      };
    }
  }, [recorder, safeStopping, transcribing]);

  const recordingDockVisible = recordingContext !== null
    && (recorder.isRecording || safeStopping || transcribing);

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
      isPaused: recorder.isPaused,
      captureState: recorder.captureState,
      isTranscribing: transcribing,
      busy,
      recordingDockVisible,
      recordingDuration: recorder.recordingDuration,
      transportState,
      recordingContext,
      activeMicrophoneLabel,
      partialTranscript,
      runtimeNotice,
      automaticResult,
      hasPendingVoiceSpool,
      clearRuntimeNotice,
      clearAutomaticResult,
      analyser: recorder.analyser,
      toggleRecording,
      toggleRecordingPause,
      cancelRecording,
      discardPendingVoiceSpool,
      formatDuration: formatRecordingDuration,
    }),
    [
      busy,
      activeMicrophoneLabel,
      cancelRecording,
      discardPendingVoiceSpool,
      deleteWhisperModel,
      downloadWhisperModel,
      microphones,
      hasPendingVoiceSpool,
      partialTranscript,
      runtimeNotice,
      automaticResult,
      clearAutomaticResult,
      clearRuntimeNotice,
      recorder,
      recordingContext,
      recordingDockVisible,
      refreshWhisperReadiness,
      resetWhisperReadiness,
      toggleRecording,
      toggleRecordingPause,
      transcribing,
      transportState,
      whisperChecking,
      whisperDownloading,
      whisperModelExists,
    ],
  );
}
