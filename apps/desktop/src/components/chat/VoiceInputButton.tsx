import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef } from 'react';
import { AnimatePresence, motion, useReducedMotion } from 'framer-motion';
import { Mic, Loader2, Trash2 } from 'lucide-react';
import { toast } from 'sonner';
import { useTranslation } from '../../i18n';
import { useVoiceInputRuntime, type VoiceRuntimeErrorCode } from '../../features/voice';
import type { VoiceDictationEvent } from '../../features/voice/voiceDraftProjection';
import { VoiceRecordingDock } from '../voice/VoiceRecordingDock';

interface VoiceInputButtonProps {
  onDictationEvent: (event: VoiceDictationEvent) => void;
  disabled?: boolean;
}

export interface VoiceInputButtonHandle {
  cancelCapture: () => void;
}

export const VoiceInputButton = forwardRef<VoiceInputButtonHandle, VoiceInputButtonProps>(
function VoiceInputButton({ onDictationEvent, disabled }, ref) {
  const { t } = useTranslation();
  const reducedMotion = useReducedMotion();
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
  const lastPublishedPartialRef = useRef('');
  const captureGenerationRef = useRef(0);
  const discardAutomaticResultRef = useRef(false);

  useEffect(() => {
    if (partialTranscript === lastPublishedPartialRef.current) return;
    lastPublishedPartialRef.current = partialTranscript;
    onDictationEvent({ kind: 'interim', text: partialTranscript });
  }, [onDictationEvent, partialTranscript]);

  const cancelCapture = useCallback(() => {
    captureGenerationRef.current += 1;
    discardAutomaticResultRef.current = true;
    cancelRecording();
    lastPublishedPartialRef.current = '';
  }, [cancelRecording]);

  const handleCancel = useCallback(() => {
    cancelCapture();
    onDictationEvent({ kind: 'cancel' });
  }, [cancelCapture, onDictationEvent]);

  useImperativeHandle(ref, () => ({ cancelCapture }), [cancelCapture]);

  // Cancel on Escape
  useEffect(() => {
    if (!isRecording) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        handleCancel();
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [handleCancel, isRecording]);

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
    if (discardAutomaticResultRef.current) {
      clearAutomaticResult();
      return;
    }
    if (automaticResult.status === 'transcribed') {
      onDictationEvent({ kind: 'final', text: automaticResult.text });
    } else if (automaticResult.status === 'error') {
      onDictationEvent({ kind: 'end' });
      showRuntimeError(automaticResult.code, automaticResult.message);
    } else if (automaticResult.status === 'empty') {
      onDictationEvent({ kind: 'cancel' });
    }
    lastPublishedPartialRef.current = '';
    clearAutomaticResult();
  }, [automaticResult, clearAutomaticResult, onDictationEvent, showRuntimeError]);

  const handleClick = useCallback(async () => {
    if (busy) return;

    const wasRecording = isRecording;
    if (!wasRecording) {
      // Establish composer ownership before microphone startup can race a fast
      // provider interim event back to React.
      lastPublishedPartialRef.current = '';
      discardAutomaticResultRef.current = false;
      onDictationEvent({ kind: 'start' });
    }
    const captureGeneration = captureGenerationRef.current;
    const result = await toggleRecording();
    if (captureGeneration !== captureGenerationRef.current) return;
    if (result.status === 'transcribed') {
      lastPublishedPartialRef.current = '';
      onDictationEvent({ kind: 'final', text: result.text });
    } else if (result.status === 'empty') {
      lastPublishedPartialRef.current = '';
      onDictationEvent({ kind: 'cancel' });
    } else if (result.status === 'error') {
      if (wasRecording) onDictationEvent({ kind: 'end' });
      else onDictationEvent({ kind: 'cancel' });
      showRuntimeError(result.code, result.message);
    }
  }, [busy, isRecording, onDictationEvent, showRuntimeError, toggleRecording]);

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
    <div className="contents">
      <AnimatePresence initial={false}>
      {recordingDockVisible && (
        <motion.div
          key="recording-dock"
          className="order-first w-full min-w-0 shrink-0 basis-full overflow-hidden"
          initial={{ height: 0, opacity: 0 }}
          animate={{ height: 'auto', opacity: 1 }}
          exit={{ height: 0, opacity: 0 }}
          transition={{ duration: reducedMotion ? 0 : 0.28, ease: [0.22, 1, 0.36, 1] }}
        >
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
          onCancel={handleCancel}
          onPauseResume={() => { void handlePauseResume(); }}
          onStop={() => { void handleClick(); }}
        />
        </motion.div>
      )}
      </AnimatePresence>
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
          <span className="recording-indicator" />
        ) : (
          <Mic className="h-3.5 w-3.5" />
        )}
      </button>
    </div>
  );
});
