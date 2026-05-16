import { useCallback, useMemo, useState } from 'react';

import * as api from '../../lib/api';
import { getModelStatus, invalidate as invalidateModelStatus } from '../../lib/modelStatusCache';
import type { VideoConfig, WhisperModel } from '../../types/video';
import { useMicrophoneDevices } from './useMicrophoneDevices';
import { useVoiceRecorder } from './useVoiceRecorder';

export type VoiceRuntimeErrorCode =
  | 'busy'
  | 'permission_denied'
  | 'recording_failed'
  | 'transcription_failed'
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
      const transcript = normalizeTranscript(await api.transcribeAudioBuffer(Array.from(wav)));
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

  const toggleRecording = useCallback(async (): Promise<VoiceRuntimeActionResult> => {
    if (transcribing || recorder.isProcessing || whisperChecking) {
      return { status: 'error', code: 'busy' };
    }

    if (recorder.isRecording) {
      const wav = await recorder.stopRecording();
      return wav ? transcribeWav(wav) : { status: 'empty' };
    }

    const readinessError = await ensureWhisperReadyForRecording();
    if (readinessError) return readinessError;

    try {
      await recorder.startRecording();
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
    ensureWhisperReadyForRecording,
    transcribeWav,
    transcribing,
    whisperChecking,
  ]);

  const busy = recorder.isProcessing || transcribing || whisperChecking;

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
      toggleRecording,
      cancelRecording: recorder.cancelRecording,
      transcribeWav,
      formatDuration: formatRecordingDuration,
    }),
    [
      busy,
      deleteWhisperModel,
      downloadWhisperModel,
      microphones,
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
