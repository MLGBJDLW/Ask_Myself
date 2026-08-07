import { useState } from 'react';
import {
  ChevronDown,
  Loader2,
  Mic2,
  Pause,
  Play,
  Square,
  Trash2,
  Wifi,
  WifiOff,
} from 'lucide-react';

import { useTranslation } from '../../i18n';
import type {
  VoiceCaptureState,
  VoiceRecordingContext,
  VoiceTransportState,
} from '../../features/voice';
import { MicrophoneWaveform } from './MicrophoneWaveform';

interface VoiceRecordingDockProps {
  analyser: AnalyserNode | null;
  captureState: VoiceCaptureState;
  context: VoiceRecordingContext | null;
  duration: string;
  isPaused: boolean;
  isProcessing: boolean;
  microphoneLabel: string | null;
  partialTranscript: string;
  transportState: VoiceTransportState;
  onCancel: () => void;
  onPauseResume: () => void;
  onStop: () => void;
}

function statusTone(
  state: VoiceTransportState,
  captureState: VoiceCaptureState,
  isPaused: boolean,
): string {
  if (captureState === 'disconnected' || state === 'offline') return 'bg-danger';
  if (state === 'degraded' || state === 'buffering' || captureState === 'interrupted') {
    return 'bg-warning';
  }
  if (isPaused) return 'bg-text-tertiary';
  if (state === 'processing') return 'bg-accent';
  return 'bg-success';
}

export function VoiceRecordingDock({
  analyser,
  captureState,
  context,
  duration,
  isPaused,
  isProcessing,
  microphoneLabel,
  partialTranscript,
  transportState,
  onCancel,
  onPauseResume,
  onStop,
}: VoiceRecordingDockProps) {
  const { t } = useTranslation();
  const [detailsOpen, setDetailsOpen] = useState(false);
  const stateKey = isProcessing
    ? 'processing'
    : isPaused
      ? 'paused'
      : captureState === 'disconnected' || transportState === 'offline'
        ? 'offline'
        : transportState;
  const language = context?.language || t('voice.languageAuto');
  const provider = context?.providerLabel || t('voice.localSpool');
  const statusLabel = stateKey === 'online'
    ? t('voice.statusOnline')
    : stateKey === 'buffering'
      ? t('voice.statusBuffering')
      : stateKey === 'degraded'
        ? t('voice.statusDegraded')
        : stateKey === 'offline'
          ? t('voice.statusOffline')
          : stateKey === 'processing'
            ? t('voice.statusProcessing')
            : stateKey === 'paused'
              ? t('voice.statusPaused')
              : t('voice.statusLocal');

  return (
    <section
      data-testid="voice-recording-dock"
      data-state={stateKey}
      className="min-w-0 flex-1 overflow-hidden rounded-lg border border-danger/25 bg-surface-1/92 shadow-[0_8px_24px_rgba(0,0,0,0.14)]"
      aria-label={t('voice.recordingDock')}
    >
      <div className="flex min-h-14 min-w-0 flex-col gap-2 px-2.5 py-2 sm:flex-row sm:items-center">
        <div className="flex min-w-0 flex-1 items-center gap-2.5">
          <span
            className={`h-2 w-2 shrink-0 rounded-full ${statusTone(transportState, captureState, isPaused)} ${!isPaused && !isProcessing ? 'animate-pulse motion-reduce:animate-none' : ''}`}
            aria-hidden="true"
          />
          <span className="w-10 shrink-0 text-xs font-semibold tabular-nums text-text-primary">
            {duration}
          </span>
          <MicrophoneWaveform
            analyser={isPaused || isProcessing ? null : analyser}
            barCount={24}
            className="min-w-24 flex-1 text-danger"
            label={t('voice.waveformLabel')}
          />
          <div className="min-w-0 max-w-44 text-right">
            <div className="truncate text-[11px] font-medium text-text-primary">
              {language} · {provider}
            </div>
            <div
              className="flex items-center justify-end gap-1 text-[10px] text-text-tertiary"
              role="status"
              aria-atomic="true"
            >
              {stateKey === 'offline' ? <WifiOff className="h-3 w-3" /> : <Wifi className="h-3 w-3" />}
              <span>{statusLabel}</span>
            </div>
          </div>
        </div>

        <div className="flex shrink-0 items-center justify-end gap-1.5">
          <button
            type="button"
            onClick={() => setDetailsOpen((open) => !open)}
            className="flex h-8 items-center gap-1 rounded-md px-2 text-[11px] text-text-tertiary transition-colors hover:bg-surface-2 hover:text-text-primary"
            aria-expanded={detailsOpen}
            aria-label={t('voice.audioDetails')}
          >
            <Mic2 className="h-3.5 w-3.5" />
            <ChevronDown className={`h-3 w-3 transition-transform motion-reduce:transition-none ${detailsOpen ? 'rotate-180' : ''}`} />
          </button>
          {!isProcessing && (
            <>
              <button
                type="button"
                onClick={onCancel}
                className="flex h-8 items-center gap-1 rounded-md px-2 text-[11px] text-text-tertiary transition-colors hover:bg-danger/10 hover:text-danger"
                aria-label={t('voice.cancelRecording')}
              >
                <Trash2 className="h-3.5 w-3.5" />
                <span className="hidden xl:inline">{t('voice.cancel')}</span>
              </button>
              <button
                type="button"
                onClick={onPauseResume}
                className="flex h-8 items-center gap-1 rounded-md px-2 text-[11px] text-text-secondary transition-colors hover:bg-surface-2 hover:text-text-primary"
                aria-label={isPaused ? t('voice.resumeRecording') : t('voice.pauseRecording')}
              >
                {isPaused ? <Play className="h-3.5 w-3.5" /> : <Pause className="h-3.5 w-3.5" />}
                <span className="hidden xl:inline">
                  {isPaused ? t('voice.resume') : t('voice.pause')}
                </span>
              </button>
              <button
                type="button"
                onClick={onStop}
                className="flex h-8 items-center gap-1.5 rounded-md bg-danger px-2.5 text-[11px] font-medium text-white transition-colors hover:bg-danger/90"
                aria-label={t('voice.stopAndTranscribe')}
              >
                <Square className="h-3 w-3 fill-current" />
                <span>{t('voice.stopAndTranscribe')}</span>
              </button>
            </>
          )}
          {isProcessing && (
            <div className="flex h-8 items-center gap-1.5 px-2 text-[11px] font-medium text-accent">
              <Loader2 className="h-3.5 w-3.5 animate-spin motion-reduce:animate-none" />
              {t('voice.processing')}
            </div>
          )}
        </div>
      </div>

      {(partialTranscript || detailsOpen) && (
        <div className="border-t border-border/45 px-3 py-1.5 text-[11px]">
          {partialTranscript && (
            <p
              data-testid="voice-partial-transcript"
              className="truncate text-text-secondary"
              aria-live="polite"
              aria-atomic="true"
            >
              “{partialTranscript}”
            </p>
          )}
          {detailsOpen && (
            <div className="mt-1 flex flex-wrap gap-x-4 gap-y-1 text-[10px] text-text-tertiary">
              <span>{t('voice.microphoneDevice')}: {microphoneLabel || t('voice.microphoneDefault')}</span>
              <span>{t('voice.provider')}: {provider}</span>
              <span>{t('voice.language')}: {language}</span>
              <span>{t('voice.storage')}: {t('voice.localSpool')}</span>
            </div>
          )}
        </div>
      )}
    </section>
  );
}
