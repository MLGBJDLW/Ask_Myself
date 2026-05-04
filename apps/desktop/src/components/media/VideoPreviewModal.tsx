import { useState, useEffect, useRef } from 'react';
import { X } from 'lucide-react';
import { motion, AnimatePresence, useReducedMotion } from 'framer-motion';
import { useTranslation } from '../../i18n';
import { getVideoTranscript, getVideoMetadata } from '../../lib/api';
import { getSoftDropdownMotion, INSTANT_TRANSITION } from '../../lib/uiMotion';
import type { TranscriptChunk, VideoMetadata } from '../../types/video';
import { VideoPlayer } from './VideoPlayer';
import type { VideoPlayerHandle } from './VideoPlayer';
import { TranscriptViewer } from './TranscriptViewer';

interface VideoPreviewModalProps {
  open: boolean;
  onClose: () => void;
  filePath: string;
}

function formatDuration(secs: number): string {
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = Math.floor(secs % 60);
  if (h > 0) return `${h}:${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`;
  return `${m}:${s.toString().padStart(2, '0')}`;
}

export function VideoPreviewModal({ open, onClose, filePath }: VideoPreviewModalProps) {
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();
  const videoRef = useRef<VideoPlayerHandle>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const [transcript, setTranscript] = useState<TranscriptChunk[]>([]);
  const [metadata, setMetadata] = useState<VideoMetadata | null>(null);
  const [currentTimeMs, setCurrentTimeMs] = useState(0);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    if (!open) {
      setTranscript([]);
      setMetadata(null);
      setCurrentTimeMs(0);
      return;
    }
  }, [open]);

  useEffect(() => {
    if (open && filePath) {
      setLoading(true);
      Promise.all([
        getVideoTranscript(filePath),
        getVideoMetadata(filePath),
      ]).then(([t, m]) => {
        setTranscript(t);
        setMetadata(m);
        setLoading(false);
      }).catch(() => setLoading(false));
    }
  }, [open, filePath]);

  useEffect(() => {
    if (!open) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [open, onClose]);

  const fileName = filePath.split(/[\\/]/).pop() || filePath;

  return (
    <AnimatePresence>
      {open && (
        <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
          <motion.div
            initial={shouldReduceMotion ? false : { opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={shouldReduceMotion ? INSTANT_TRANSITION : { duration: 0.15 }}
            className="absolute inset-0 bg-black/60 backdrop-blur-sm"
            onClick={onClose}
            aria-hidden="true"
          />
          <motion.div
            ref={contentRef}
            {...getSoftDropdownMotion(!!shouldReduceMotion, 8)}
            role="dialog"
            aria-modal="true"
            aria-label={fileName}
            tabIndex={-1}
            className="relative z-10 w-full max-w-4xl max-h-[90vh] bg-surface-2 border border-border rounded-lg shadow-lg overflow-hidden flex flex-col"
          >
            {/* Header */}
            <div className="flex items-center justify-between px-5 py-4 border-b border-border shrink-0">
              <h2 className="text-sm font-semibold text-text-primary truncate">{fileName}</h2>
              <button
                onClick={onClose}
                className="p-1 rounded-md text-text-tertiary hover:text-text-primary hover:bg-surface-3 transition-colors"
                aria-label={t('common.close')}
              >
                <X size={16} />
              </button>
            </div>

            {/* Body */}
            <div className="px-5 py-4 overflow-y-auto">
              <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
                {/* Video Player */}
                <div>
                  <VideoPlayer
                    ref={videoRef}
                    src={filePath}
                    thumbnailSrc={metadata?.thumbnailPath || undefined}
                    onTimeUpdate={setCurrentTimeMs}
                  />
                  {metadata && (
                    <div className="mt-2 flex flex-wrap gap-3 text-xs text-text-secondary">
                      {metadata.width != null && metadata.height != null && (
                        <span>{metadata.width}×{metadata.height}</span>
                      )}
                      {metadata.codec && <span>{metadata.codec}</span>}
                      {metadata.framerate != null && (
                        <span>{metadata.framerate.toFixed(1)} fps</span>
                      )}
                      {metadata.durationSecs != null && (
                        <span>{formatDuration(metadata.durationSecs)}</span>
                      )}
                    </div>
                  )}
                </div>

                {/* Transcript */}
                <div>
                  <h3 className="text-sm font-medium mb-2">{t('media.transcript')}</h3>
                  {loading ? (
                    <p className="text-sm text-text-secondary">{t('common.loading')}</p>
                  ) : transcript.length > 0 ? (
                    <TranscriptViewer
                      segments={transcript.map(c => ({
                        startMs: c.startMs || 0,
                        endMs: c.endMs || 0,
                        text: c.text,
                      }))}
                      currentTimeMs={currentTimeMs}
                      onSegmentClick={(ms) => videoRef.current?.seekTo(ms / 1000)}
                      className="max-h-[50vh]"
                    />
                  ) : (
                    <div className="flex flex-col items-center justify-center h-full text-muted-foreground text-sm gap-2 p-4">
                      <p>{t('media.noTranscript')}</p>
                      <p className="text-xs">{t('media.transcriptHint')}</p>
                    </div>
                  )}
                </div>
              </div>
            </div>
          </motion.div>
        </div>
      )}
    </AnimatePresence>
  );
}
