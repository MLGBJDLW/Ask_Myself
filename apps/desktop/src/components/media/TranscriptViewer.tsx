import { useRef, useEffect } from 'react';
import { useTranslation } from '../../i18n';

export interface TranscriptSegment {
  startMs: number;
  endMs: number;
  text: string;
}

interface TranscriptViewerProps {
  segments: TranscriptSegment[];
  currentTimeMs?: number;
  onSegmentClick?: (startMs: number) => void;
  className?: string;
}

export function TranscriptViewer({
  segments,
  currentTimeMs = 0,
  onSegmentClick,
  className = '',
}: TranscriptViewerProps) {
  const { t } = useTranslation();
  const activeRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const id = requestAnimationFrame(() => {
      activeRef.current?.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
    });
    return () => cancelAnimationFrame(id);
  }, [currentTimeMs]);

  if (segments.length === 0) {
    return (
      <div className={`text-sm text-text-secondary text-center py-8 ${className}`}>
        {t('media.noTranscript')}
      </div>
    );
  }

  return (
    <div className={`overflow-y-auto space-y-1 text-sm ${className}`}>
      {segments.map((seg, i) => {
        const isActive = currentTimeMs >= seg.startMs && currentTimeMs < seg.endMs;
        return (
          <div
            key={i}
            ref={isActive ? activeRef : undefined}
            onClick={() => onSegmentClick?.(seg.startMs)}
            role="button"
            tabIndex={0}
            onKeyDown={(e) => {
              if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                onSegmentClick?.(seg.startMs);
              }
            }}
            title={t('media.clickToSeek')}
            className={`flex gap-3 p-2 rounded cursor-pointer transition-colors ${
              isActive
                ? 'bg-primary/10 text-primary'
                : 'hover:bg-surface-3'
            }`}
          >
            <span className="text-xs text-text-secondary font-mono whitespace-nowrap mt-0.5">
              {formatTime(seg.startMs)}
            </span>
            <span>{seg.text}</span>
          </div>
        );
      })}
    </div>
  );
}

function formatTime(ms: number): string {
  const totalSecs = Math.floor(ms / 1000);
  const h = Math.floor(totalSecs / 3600);
  const m = Math.floor((totalSecs % 3600) / 60);
  const s = totalSecs % 60;
  if (h > 0) return `${h}:${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`;
  return `${m}:${s.toString().padStart(2, '0')}`;
}
