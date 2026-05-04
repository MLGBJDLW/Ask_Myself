import { useState, useCallback, useEffect } from 'react';
import { Mic, Loader2 } from 'lucide-react';
import { toast } from 'sonner';
import { useTranslation } from '../../i18n';
import { useVoiceRecorder } from '../../lib/useVoiceRecorder';
import * as api from '../../lib/api';

interface VoiceInputButtonProps {
  onTranscript: (text: string) => void;
  disabled?: boolean;
}

export function VoiceInputButton({ onTranscript, disabled }: VoiceInputButtonProps) {
  const { t } = useTranslation();
  const savedDeviceId = typeof window !== 'undefined'
    ? localStorage.getItem('nexa-mic-device-id')
    : null;
  const { isRecording, isProcessing, startRecording, stopRecording, cancelRecording, recordingDuration } =
    useVoiceRecorder(savedDeviceId);
  const [transcribing, setTranscribing] = useState(false);

  const busy = isProcessing || transcribing;

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

  const handleClick = useCallback(async () => {
    if (busy) return;

    if (isRecording) {
      // Stop and transcribe
      const wav = await stopRecording();
      if (!wav) return;

      setTranscribing(true);
      try {
        const text = await api.transcribeAudioBuffer(Array.from(wav));
        if (text.trim()) {
          onTranscript(text.trim());
        }
      } catch (e) {
        toast.error(String(e));
      } finally {
        setTranscribing(false);
      }
      return;
    }

    // Start: check model first
    try {
      const config = await api.getVideoConfig();
      // TODO: migrate to modelStatusCache
      const exists = await api.checkWhisperModel(config);
      if (!exists) {
        toast.error(t('voice.noModel'));
        return;
      }
    } catch {
      toast.error(t('voice.error'));
      return;
    }

    try {
      await startRecording();
    } catch (err) {
      if (err instanceof DOMException && err.name === 'NotAllowedError') {
        toast.error(t('voice.permissionDenied'));
      } else {
        toast.error(t('voice.error'));
      }
    }
  }, [isRecording, busy, stopRecording, startRecording, onTranscript, t]);

  const formatDuration = (secs: number) => {
    const m = Math.floor(secs / 60);
    const s = secs % 60;
    return `${m}:${s.toString().padStart(2, '0')}`;
  };

  const label = busy
    ? t('voice.processing')
    : isRecording
      ? t('voice.stopRecording')
      : t('voice.startRecording');

  return (
    <button
      onClick={handleClick}
      disabled={disabled || busy}
      className={`relative flex h-10 shrink-0 items-center justify-center rounded-lg transition-colors duration-fast ease-out cursor-pointer disabled:pointer-events-none disabled:opacity-40 ${
        isRecording
          ? 'gap-1.5 bg-danger/10 px-3 text-danger voice-btn-recording'
          : 'w-10 text-text-tertiary hover:bg-surface-2 hover:text-text-secondary'
      }`}
      aria-label={label}
      title={label}
    >
      {busy ? (
        <Loader2 className="h-4 w-4 animate-spin" />
      ) : isRecording ? (
        <>
          <span className="recording-indicator" />
          <span className="text-xs font-medium tabular-nums">{formatDuration(recordingDuration)}</span>
        </>
      ) : (
        <Mic className="h-4 w-4" />
      )}
    </button>
  );
}
