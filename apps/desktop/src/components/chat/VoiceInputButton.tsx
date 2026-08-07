import { useCallback, useEffect } from 'react';
import { Mic, Loader2, Trash2 } from 'lucide-react';
import { toast } from 'sonner';
import { useTranslation } from '../../i18n';
import { useVoiceInputRuntime, type VoiceRuntimeErrorCode } from '../../features/voice';
import { VoiceRecordingDock } from '../voice/VoiceRecordingDock';

interface VoiceInputButtonProps {
  onTranscript: (text: string) => void;
  disabled?: boolean;
}

export function VoiceInputButton({ onTranscript, disabled }: VoiceInputButtonProps) {
  const { t } = useTranslation();
  const voiceRuntime = useVoiceInputRuntime();
  const {
    isRecording,
    isPaused,
    captureState,
    busy,
    cancelRecording,
    recordingDockVisible,
    transportState,
    recordingContext,
    activeMicrophoneLabel,
    partialTranscript,
    runtimeNotice,
    automaticResult,
    hasPendingVoiceSpool,
    clearRuntimeNotice,
    clearAutomaticResult,
    recordingDuration,
    toggleRecording,
    toggleRecordingPause,
    discardPendingVoiceSpool,
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

  const showRuntimeError = useCallback((code: VoiceRuntimeErrorCode, message?: string) => {
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
    } else if (code === 'voice_cleanup_pending') {
      toast.warning(t('voice.cleanupPending'), message ? { description: message } : undefined);
    } else if (code === 'transcription_failed') {
      toast.error(t('voice.transcriptionFailed'), message ? { description: message } : undefined);
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

  const handleDiscard = useCallback(async () => {
    if (busy) return;
    const result = await discardPendingVoiceSpool();
    if (result.status === 'error') showRuntimeError(result.code, result.message);
  }, [busy, discardPendingVoiceSpool, showRuntimeError]);

  const handlePauseResume = useCallback(async () => {
    const result = await toggleRecordingPause();
    if (result.status === 'error') showRuntimeError(result.code, result.message);
  }, [showRuntimeError, toggleRecordingPause]);

  const label = busy
    ? t('voice.processing')
    : isRecording
      ? t('voice.stopRecording')
      : t('voice.startRecording');

  return (
    <div className={recordingDockVisible
      ? 'order-first flex w-full min-w-0 shrink-0 basis-full lg:order-none lg:w-auto lg:min-w-[420px] lg:flex-1 lg:basis-auto'
      : 'flex shrink-0 items-center gap-0.5'}>
      {recordingDockVisible && (
        <VoiceRecordingDock
          analyser={analyser}
          captureState={captureState}
          context={recordingContext}
          duration={formatDuration(recordingDuration)}
          isPaused={isPaused}
          isProcessing={transportState === 'processing'}
          microphoneLabel={activeMicrophoneLabel}
          partialTranscript={partialTranscript}
          transportState={transportState}
          onCancel={cancelRecording}
          onPauseResume={() => { void handlePauseResume(); }}
          onStop={() => { void handleClick(); }}
        />
      )}
      {hasPendingVoiceSpool && !isRecording && !recordingDockVisible && (
        <button
          type="button"
          onClick={handleDiscard}
          disabled={disabled || busy}
          className="flex h-8 w-7 items-center justify-center rounded-md text-text-tertiary transition-colors duration-fast ease-out hover:bg-danger/10 hover:text-danger disabled:pointer-events-none disabled:opacity-40"
          aria-label={t('voice.discardPending')}
          title={t('voice.discardPending')}
        >
          <Trash2 className="h-3.5 w-3.5" />
        </button>
      )}
      <button
        type="button"
        onClick={handleClick}
        disabled={disabled || busy}
        className={`relative h-8 shrink-0 items-center justify-center rounded-md transition-colors duration-fast ease-out cursor-pointer disabled:pointer-events-none disabled:opacity-40 ${recordingDockVisible ? 'hidden' : 'flex'} ${
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
          <span className="recording-indicator" />
        ) : (
          <Mic className="h-3.5 w-3.5" />
        )}
      </button>
    </div>
  );
}
