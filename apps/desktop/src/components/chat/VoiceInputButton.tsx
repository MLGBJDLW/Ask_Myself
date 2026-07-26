import { useCallback, useEffect } from 'react';
import { Mic, Loader2 } from 'lucide-react';
import { toast } from 'sonner';
import { useTranslation } from '../../i18n';
import { useVoiceInputRuntime, type VoiceRuntimeErrorCode } from '../../features/voice';

interface VoiceInputButtonProps {
  onTranscript: (text: string) => void;
  disabled?: boolean;
}

export function VoiceInputButton({ onTranscript, disabled }: VoiceInputButtonProps) {
  const { t } = useTranslation();
  const voiceRuntime = useVoiceInputRuntime();
  const { isRecording, busy, cancelRecording, recordingDuration, toggleRecording, formatDuration } =
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

  const showRuntimeError = useCallback((code: VoiceRuntimeErrorCode, message?: string) => {
    if (code === 'whisper_model_missing') {
      toast.error(t('voice.noModel'));
    } else if (code === 'speech_provider_not_configured') {
      toast.error(t('voice.providerNotConfigured'));
    } else if (code === 'permission_denied') {
      toast.error(t('voice.permissionDenied'));
    } else if (code === 'transcription_failed' && message) {
      toast.error(message);
    } else if (code !== 'busy') {
      toast.error(t('voice.error'));
    }
  }, [t]);

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
          <span className="text-[11px] font-medium tabular-nums">{formatDuration(recordingDuration)}</span>
        </>
      ) : (
        <Mic className="h-3.5 w-3.5" />
      )}
    </button>
  );
}
