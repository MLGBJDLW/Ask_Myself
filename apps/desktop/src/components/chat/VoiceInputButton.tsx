import { useCallback, useEffect } from 'react';
import { Mic, Loader2 } from 'lucide-react';
import { toast } from 'sonner';
import { useTranslation } from '../../i18n';
import { useVoiceInputRuntime, type VoiceRuntimeErrorCode } from '../../features/voice';
import { MicrophoneWaveform } from '../voice/MicrophoneWaveform';

interface VoiceInputButtonProps {
  onTranscript: (text: string) => void;
  disabled?: boolean;
}

export function VoiceInputButton({ onTranscript, disabled }: VoiceInputButtonProps) {
  const { t } = useTranslation();
  const voiceRuntime = useVoiceInputRuntime();
  const {
    isRecording,
    busy,
    cancelRecording,
    partialTranscript,
    runtimeNotice,
    automaticResult,
    clearRuntimeNotice,
    clearAutomaticResult,
    recordingDuration,
    toggleRecording,
    formatDuration,
    analyser,
  } =
    voiceRuntime;

  // Cancel on Escape
  useEffect(() => {
    if (!isRecording) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        cancelRecording();
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [isRecording, cancelRecording]);

  const showRuntimeError = useCallback((code: VoiceRuntimeErrorCode, _message?: string) => {
    if (code === 'whisper_model_missing') {
      toast.error(t('voice.noModel'));
    } else if (code === 'speech_provider_not_configured') {
      toast.error(t('voice.providerNotConfigured'));
    } else if (code === 'permission_denied') {
      toast.error(t('voice.permissionDenied'));
    } else if (code === 'realtime_backpressure') {
      toast.error(t('voice.realtimeBackpressure'));
    } else if (code === 'realtime_deferred') {
      toast.info(t('voice.realtimeDeferred'));
    } else if (code === 'transcription_failed') {
      toast.error(t('voice.transcriptionFailed'));
    } else if (code !== 'busy') {
      toast.error(t('voice.error'));
    }
  }, [t]);

  useEffect(() => {
    if (!runtimeNotice) return;
    showRuntimeError(runtimeNotice);
    clearRuntimeNotice();
  }, [clearRuntimeNotice, runtimeNotice, showRuntimeError]);

  useEffect(() => {
    if (!automaticResult) return;
    if (automaticResult.status === 'transcribed') {
      onTranscript(automaticResult.text);
    } else if (automaticResult.status === 'error') {
      showRuntimeError(automaticResult.code, automaticResult.message);
    }
    clearAutomaticResult();
  }, [automaticResult, clearAutomaticResult, onTranscript, showRuntimeError]);

  const handleClick = useCallback(async () => {
    if (busy) return;

    const result = await toggleRecording();
    if (result.status === 'transcribed') {
      onTranscript(result.text);
    } else if (result.status === 'error') {
      showRuntimeError(result.code, result.message);
    }
  }, [busy, onTranscript, showRuntimeError, toggleRecording]);

  const label = busy
    ? t('voice.processing')
    : isRecording
      ? t('voice.stopRecording')
      : t('voice.startRecording');

  return (
    <button
      onClick={handleClick}
      disabled={disabled || busy}
      className={`relative flex h-8 shrink-0 items-center justify-center rounded-md transition-colors duration-fast ease-out cursor-pointer disabled:pointer-events-none disabled:opacity-40 ${
        isRecording
          ? 'gap-1.5 bg-danger/10 px-2.5 text-danger voice-btn-recording'
          : 'w-8 text-text-tertiary hover:bg-surface-2 hover:text-text-secondary'
      }`}
      aria-label={label}
      title={label}
    >
      {busy ? (
        <Loader2 className="h-3.5 w-3.5 animate-spin" />
      ) : isRecording ? (
        <>
          <span className="recording-indicator" />
          <MicrophoneWaveform
            analyser={analyser}
            barCount={12}
            className="h-4"
            label={t('voice.waveformLabel')}
          />
          {partialTranscript && (
            <span className="max-w-36 truncate text-[11px] text-text-secondary">
              {partialTranscript}
            </span>
          )}
          <span className="text-[11px] font-medium tabular-nums">{formatDuration(recordingDuration)}</span>
        </>
      ) : (
        <Mic className="h-3.5 w-3.5" />
      )}
    </button>
  );
}
