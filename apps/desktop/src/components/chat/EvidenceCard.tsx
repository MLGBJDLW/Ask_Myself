import { useEffect, useRef, useCallback, useState } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { FileText, Film, Music, Clock, ExternalLink, Copy, Check, X } from 'lucide-react';
import { useTranslation } from '../../i18n';
import { openFileInDefaultApp, showInFileExplorer, getEvidenceCard } from '../../lib/api';
import { VideoPreviewModal } from '../media/VideoPreviewModal';
import { SaveToPlaybookButton } from '../SaveToPlaybookButton';
import type { CitationCardData } from '../../lib/citationParser';

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

interface EvidenceCardPopupProps {
  card: CitationCardData;
  /** Anchor element to position near */
  anchorRect: DOMRect | null;
  onClose: () => void;
}

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

const VIDEO_EXTS = ['.mp4', '.mkv', '.webm', '.mov', '.avi', '.flv', '.wmv', '.m4v', '.mpeg', '.mpg'];
const AUDIO_EXTS = ['.mp3', '.wav', '.flac', '.ogg', '.aac', '.m4a', '.wma', '.opus'];

function isVideoFile(path: string): boolean {
  return VIDEO_EXTS.some(ext => path.toLowerCase().endsWith(ext));
}

function isAudioFile(path: string): boolean {
  return AUDIO_EXTS.some(ext => path.toLowerCase().endsWith(ext));
}

function extractTimestamp(headingContext: string | undefined): string | null {
  if (!headingContext) return null;
  const match = headingContext.match(/(\d{2}:\d{2}:\d{2})/);
  return match ? match[1] : null;
}

function basename(path: string): string {
  const normalized = path.replace(/[\\/]+$/, '');
  const lastSep = Math.max(normalized.lastIndexOf('/'), normalized.lastIndexOf('\\'));
  return lastSep === -1 ? normalized : normalized.slice(lastSep + 1);
}

function truncate(text: string, max: number): string {
  if (text.length <= max) return text;
  return text.slice(0, max).trimEnd() + '…';
}

function formatScore(score: number): string {
  if (score <= 0) return '';
  return `${(score * 100).toFixed(0)}%`;
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function EvidenceCardPopup({ card, anchorRect, onClose }: EvidenceCardPopupProps) {
  const { t } = useTranslation();
  const popupRef = useRef<HTMLDivElement>(null);
  const [copied, setCopied] = useState(false);
  const [videoPreviewPath, setVideoPreviewPath] = useState<string | null>(null);

  // Close on click outside
  useEffect(() => {
    function handleClickOutside(e: MouseEvent) {
      if (popupRef.current && !popupRef.current.contains(e.target as Node)) {
        onClose();
      }
    }
    document.addEventListener('mousedown', handleClickOutside);
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, [onClose]);

  // Close on Escape
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') onClose();
    }
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [onClose]);

  const handleOpenFile = useCallback(() => {
    if (card.documentPath) openFileInDefaultApp(card.documentPath);
  }, [card.documentPath]);

  const handleShowInExplorer = useCallback(() => {
    if (card.documentPath) showInFileExplorer(card.documentPath);
  }, [card.documentPath]);

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(card.content);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // silently fail
    }
  }, [card.content]);

  // Position: below the anchor, clamped to viewport
  const style: React.CSSProperties = {};
  if (anchorRect) {
    style.position = 'fixed';
    style.top = Math.min(anchorRect.bottom + 6, window.innerHeight - 320);
    style.left = Math.max(8, Math.min(anchorRect.left, window.innerWidth - 360));
    style.zIndex = 100;
  }

  const title = card.documentTitle || basename(card.documentPath) || t('citation.evidence');
  const scoreLabel = formatScore(card.score);
  const isVideo = card.documentPath ? isVideoFile(card.documentPath) : false;
  const isAudio = card.documentPath ? isAudioFile(card.documentPath) : false;
  const headingCtx = card.headingPath.length > 0 ? card.headingPath.join(' › ') : undefined;
  const timestamp = extractTimestamp(headingCtx);

  const FileIcon = isVideo ? Film : isAudio ? Music : FileText;
  const iconColor = isVideo ? 'text-violet-500' : isAudio ? 'text-amber-500' : 'text-accent';

  return (
    <>
    <AnimatePresence>
      <motion.div
        ref={popupRef}
        initial={{ opacity: 0, y: -4, scale: 0.97 }}
        animate={{ opacity: 1, y: 0, scale: 1 }}
        exit={{ opacity: 0, y: -4, scale: 0.97 }}
        transition={{ duration: 0.15 }}
        style={style}
        className="w-[340px] max-h-[300px] rounded-lg border border-border bg-surface-1 shadow-xl overflow-hidden"
        role="dialog"
        aria-label={t('citation.evidence')}
      >
        {/* Header */}
        <div className="flex items-center gap-2 px-3 py-2 border-b border-border bg-surface-2">
          <FileIcon className={`h-3.5 w-3.5 ${iconColor} shrink-0`} />
          <span className="text-xs font-medium text-text-primary truncate flex-1" title={title}>
            {title}
          </span>
          {scoreLabel && (
            <span className="text-[10px] font-medium text-accent bg-accent/10 px-1.5 py-0.5 rounded-full shrink-0">
              {scoreLabel}
            </span>
          )}
          {timestamp && (isVideo || isAudio) && (
            <span className="inline-flex items-center px-1.5 py-0.5 rounded-full text-[10px] font-medium bg-violet-100 text-violet-700 dark:bg-violet-900/30 dark:text-violet-300 shrink-0">
              <Clock className="h-2.5 w-2.5 mr-0.5" />
              {timestamp}
            </span>
          )}
          <button
            type="button"
            onClick={onClose}
            className="p-0.5 text-text-tertiary hover:text-text-primary transition-colors cursor-pointer"
            aria-label={t('common.close')}
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>

        {/* Source path */}
        {card.documentPath && (
          <div className="px-3 py-1.5 border-b border-border/50">
            <span className="text-[10px] text-text-tertiary break-all">
              {card.sourceName ? `${card.sourceName} · ` : ''}
              {card.documentPath}
            </span>
          </div>
        )}

        {/* Heading path breadcrumb */}
        {card.headingPath.length > 0 && (
          <div className="px-3 py-1 border-b border-border/50">
            <span className="text-[10px] text-text-tertiary">
              {card.headingPath.join(' › ')}
            </span>
          </div>
        )}

        {/* Content preview */}
        <div className="px-3 py-2 max-h-[150px] overflow-y-auto">
          <p className="text-xs text-text-secondary leading-relaxed whitespace-pre-wrap">
            {truncate(card.snippet || card.content, 200)}
          </p>
        </div>

        {/* Actions */}
        <div className="flex items-center gap-1 px-3 py-2 border-t border-border bg-surface-2">
          {card.documentPath && (
            <>
              <button
                type="button"
                onClick={isVideo ? () => setVideoPreviewPath(card.documentPath) : handleOpenFile}
                className="inline-flex items-center gap-1 px-2 py-1 text-[10px] font-medium rounded-md
                  bg-accent/10 text-accent hover:bg-accent/20 transition-colors cursor-pointer"
              >
                <ExternalLink className="h-3 w-3" />
                {isVideo ? t('media.videoDetails') : t('citation.openFile')}
              </button>
              <button
                type="button"
                onClick={handleShowInExplorer}
                className="inline-flex items-center gap-1 px-2 py-1 text-[10px] font-medium rounded-md
                  bg-surface-3 text-text-tertiary hover:text-text-primary transition-colors cursor-pointer"
              >
                {t('citation.showInFolder')}
              </button>
            </>
          )}
          <div className="flex-1" />
          <SaveToPlaybookButton chunkId={card.chunkId} size="sm" />
          <button
            type="button"
            onClick={handleCopy}
            className="inline-flex items-center gap-1 px-2 py-1 text-[10px] font-medium rounded-md
              bg-surface-3 text-text-tertiary hover:text-text-primary transition-colors cursor-pointer"
          >
            {copied ? (
              <>
                <Check className="h-3 w-3 text-green-500" />
                <span className="text-green-500">{t('chat.copied')}</span>
              </>
            ) : (
              <>
                <Copy className="h-3 w-3" />
                {t('citation.copy')}
              </>
            )}
          </button>
        </div>
      </motion.div>
    </AnimatePresence>

    {videoPreviewPath && (
      <VideoPreviewModal
        open
        onClose={() => setVideoPreviewPath(null)}
        filePath={videoPreviewPath}
      />
    )}
    </>
  );
}

/* ------------------------------------------------------------------ */
/*  Inline Citation Chip                                               */
/* ------------------------------------------------------------------ */

interface CitationChipProps {
  chunkId: string;
  displayText: string;
  card: CitationCardData | undefined;
}

export function CitationChip({ chunkId, displayText, card }: CitationChipProps) {
  const [popupOpen, setPopupOpen] = useState(false);
  const [anchorRect, setAnchorRect] = useState<DOMRect | null>(null);
  const [fetchedCard, setFetchedCard] = useState<CitationCardData | null>(null);
  const [fetching, setFetching] = useState(false);
  const chipRef = useRef<HTMLButtonElement>(null);

  const resolvedCard = card ?? fetchedCard;

  const handleClick = useCallback(async (e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (chipRef.current) {
      setAnchorRect(chipRef.current.getBoundingClientRect());
    }

    // If no card data available, fetch from backend
    if (!card && !fetchedCard && !fetching) {
      setFetching(true);
      try {
        const ec = await getEvidenceCard(chunkId);
        setFetchedCard({
          chunkId: ec.chunkId,
          documentPath: ec.documentPath,
          documentTitle: ec.documentTitle,
          sourceName: ec.sourceName,
          content: ec.content,
          score: ec.score,
          headingPath: ec.headingPath,
          snippet: ec.snippet,
        });
      } catch {
        // Silently fail — popup won't show detailed info
      } finally {
        setFetching(false);
      }
    }

    setPopupOpen((prev) => !prev);
  }, [card, fetchedCard, fetching, chunkId]);

  const handleClose = useCallback(() => {
    setPopupOpen(false);
  }, []);

  const title = resolvedCard?.documentTitle || resolvedCard?.documentPath || chunkId.slice(0, 8);
  const tooltipText = resolvedCard ? `${resolvedCard.documentTitle || basename(resolvedCard.documentPath)}` : chunkId.slice(0, 8);

  return (
    <>
      <button
        ref={chipRef}
        type="button"
        onClick={handleClick}
        title={tooltipText}
        className="inline-flex items-center gap-0.5 px-1.5 py-0 text-[11px] font-medium
          rounded-full border cursor-pointer transition-all duration-150
          bg-accent/10 text-accent border-accent/20
          hover:bg-accent/20 hover:border-accent/30
          active:scale-95 align-baseline leading-[1.4]
          mx-0.5"
      >
        {isVideoFile(resolvedCard?.documentPath ?? '') ? <Film className="h-2.5 w-2.5 shrink-0" /> : isAudioFile(resolvedCard?.documentPath ?? '') ? <Music className="h-2.5 w-2.5 shrink-0" /> : <FileText className="h-2.5 w-2.5 shrink-0" />}
        <span className="truncate max-w-[120px]">{displayText || title}</span>
      </button>
      {popupOpen && resolvedCard && (
        <EvidenceCardPopup card={resolvedCard} anchorRect={anchorRect} onClose={handleClose} />
      )}
    </>
  );
}
