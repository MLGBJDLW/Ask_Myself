import { Volume2, Subtitles, Mic, Film, ScanText, CheckCircle } from 'lucide-react';
import { motion } from 'framer-motion';
import { useTranslation } from '../../i18n';

type ProcessingPhase =
  | 'extracting_audio'
  | 'extracting_subtitles'
  | 'transcribing'
  | 'extracting_frames'
  | 'ocr'
  | 'complete';

interface VideoProcessingProgressProps {
  phase: ProcessingPhase;
  progress: number; // 0-100
  fileName?: string;
  className?: string;
}

const PHASES: { key: ProcessingPhase; icon: typeof Volume2; labelKey: string }[] = [
  { key: 'extracting_audio', icon: Volume2, labelKey: 'media.extractingAudio' },
  { key: 'extracting_subtitles', icon: Subtitles, labelKey: 'media.extractingSubtitles' },
  { key: 'transcribing', icon: Mic, labelKey: 'media.transcribing' },
  { key: 'extracting_frames', icon: Film, labelKey: 'media.extractingFrames' },
  { key: 'ocr', icon: ScanText, labelKey: 'media.runningOcr' },
  { key: 'complete', icon: CheckCircle, labelKey: 'media.processingComplete' },
];

export function VideoProcessingProgress({
  phase,
  progress,
  fileName,
  className = '',
}: VideoProcessingProgressProps) {
  const { t } = useTranslation();
  const currentIndex = PHASES.findIndex((p) => p.key === phase);

  return (
    <div className={`rounded-lg border border-border bg-surface-2 p-4 space-y-3 ${className}`}>
      {fileName && (
        <p className="text-xs text-text-secondary truncate">{fileName}</p>
      )}

      {/* Phase steps */}
      <div className="flex items-center gap-1">
        {PHASES.map((p, i) => {
          const Icon = p.icon;
          const isDone = i < currentIndex;
          const isCurrent = i === currentIndex;

          return (
            <div key={p.key} className="flex items-center gap-1">
              {i > 0 && (
                <div
                  className={`h-px w-4 ${isDone ? 'bg-success' : 'bg-border'}`}
                />
              )}
              <div
                className={`flex items-center justify-center w-6 h-6 rounded-full transition-colors ${
                  isDone
                    ? 'bg-success/15 text-success'
                    : isCurrent
                      ? 'bg-primary/15 text-primary'
                      : 'bg-surface-3 text-text-secondary'
                }`}
              >
                {isDone ? (
                  <CheckCircle className="w-3.5 h-3.5" />
                ) : (
                  <Icon className="w-3.5 h-3.5" />
                )}
              </div>
            </div>
          );
        })}
      </div>

      {/* Current phase label */}
      <p className="text-sm font-medium">
        {t(PHASES[currentIndex >= 0 ? currentIndex : 0].labelKey as any)}
      </p>

      {/* Progress bar */}
      {phase !== 'complete' && (
        <div className="h-1.5 rounded-full bg-surface-3 overflow-hidden">
          <motion.div
            className="h-full rounded-full bg-primary"
            initial={{ width: 0 }}
            animate={{ width: `${Math.min(100, Math.max(0, progress))}%` }}
            transition={{ duration: 0.3, ease: 'easeOut' }}
          />
        </div>
      )}
    </div>
  );
}

export type { ProcessingPhase, VideoProcessingProgressProps };
