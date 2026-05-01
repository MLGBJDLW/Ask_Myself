import { useState } from 'react';
import { motion } from 'framer-motion';
import { toast } from 'sonner';
import {
  ThumbsUp,
  ThumbsDown,
  Pin,
  FileText,
  Film,
  Music,
  Clock,
  Hash,
  FolderOpen,
  ChevronDown,
  ChevronUp,
  ExternalLink,
  BotMessageSquare,
} from 'lucide-react';
import type { EvidenceCard as EvidenceCardType, Highlight } from '../types/evidence';
import { Badge } from './ui/Badge';
import { SaveToPlaybookButton } from './SaveToPlaybookButton';
import { Tooltip } from './ui/Tooltip';
import { useTranslation } from '../i18n';
import { openFileInDefaultApp, showInFileExplorer } from '../lib/api';
import { canPreviewInApp, useFilePreview } from '../lib/filePreviewContext';
import { VideoPreviewModal } from './media/VideoPreviewModal';

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

type FeedbackAction = 'upvote' | 'downvote' | 'pin';

interface FeedbackState {
  upvoted?: boolean;
  downvoted?: boolean;
  pinned?: boolean;
}

interface Props {
  card: EvidenceCardType;
  onFeedback?: (chunkId: string, action: FeedbackAction) => void;
  feedbackState?: FeedbackState;
  onAskAbout?: (context: string) => void;
}

/* ------------------------------------------------------------------ */
/*  Constants & helpers                                                */
/* ------------------------------------------------------------------ */

const TRUNCATE_LENGTH = 200;

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

function fileExtension(path: string): string {
  const m = path.match(/\.(\w+)$/);
  return m ? m[1].toUpperCase() : '';
}

function directoryPath(path: string): string {
  const parts = path.replace(/\\/g, '/').split('/');
  parts.pop();
  return parts.join('/') || '/';
}

function scoreColor(score: number): string {
  if (score >= 0.8) return 'var(--color-success)';
  if (score >= 0.5) return 'var(--color-warning)';
  return 'var(--color-text-tertiary)';
}

/* ------------------------------------------------------------------ */
/*  Highlight renderer                                                 */
/* ------------------------------------------------------------------ */

function renderHighlights(content: string, highlights: Highlight[]) {
  if (highlights.length === 0) return <span>{content}</span>;

  const sorted = [...highlights].sort((a, b) => a.start - b.start);
  const parts: React.ReactNode[] = [];
  let cursor = 0;

  for (let i = 0; i < sorted.length; i++) {
    const h = sorted[i];
    if (h.start > cursor) {
      parts.push(<span key={`t-${i}`}>{content.slice(cursor, h.start)}</span>);
    }
    parts.push(
      <mark
        key={`h-${i}`}
        className="rounded-sm px-0.5 py-px"
        style={{
          backgroundColor: 'var(--color-accent-subtle)',
          color: 'var(--color-accent-hover)',
        }}
      >
        {content.slice(h.start, h.end)}
      </mark>,
    );
    cursor = h.end;
  }

  if (cursor < content.length) {
    parts.push(<span key="tail">{content.slice(cursor)}</span>);
  }

  return <>{parts}</>;
}

/* ------------------------------------------------------------------ */
/*  Animation variants                                                 */
/* ------------------------------------------------------------------ */

const cardVariants = {
  hidden: { opacity: 0, y: 12 },
  visible: { opacity: 1, y: 0 },
};

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function EvidenceCardComponent({
  card,
  onFeedback,
  feedbackState = {},
  onAskAbout,
}: Props) {
  const [expanded, setExpanded] = useState(false);
  const [videoPreviewPath, setVideoPreviewPath] = useState<string | null>(null);
  const { t } = useTranslation();
  const { openFilePreview } = useFilePreview();

  const previewText = card.snippet || card.content;
  const needsTruncation = previewText.length > TRUNCATE_LENGTH;
  const displayContent =
    needsTruncation && !expanded
      ? previewText.slice(0, TRUNCATE_LENGTH) + '…'
      : previewText;

  const visibleHighlights =
    expanded || !needsTruncation
      ? card.highlights
      : card.highlights.filter((h) => h.end <= TRUNCATE_LENGTH);

  const ext = fileExtension(card.documentPath);
  const dir = directoryPath(card.documentPath);
  const pct = Math.min(Math.max(card.score, 0), 1) * 100;
  const isVideo = isVideoFile(card.documentPath);
  const isAudio = isAudioFile(card.documentPath);
  const headingCtx = card.headingPath.length > 0 ? card.headingPath.join(' › ') : undefined;
  const timestamp = extractTimestamp(headingCtx);

  const FileIcon = isVideo ? Film : isAudio ? Music : FileText;
  const iconColor = isVideo ? 'text-violet-500' : isAudio ? 'text-amber-500' : 'text-text-tertiary';

  return (
    <>
    <motion.div
      variants={cardVariants}
      initial="hidden"
      animate="visible"
      transition={{ duration: 0.3, ease: [0.16, 1, 0.3, 1] }}
      whileHover={{
        boxShadow:
          '0 4px 16px rgba(0,0,0,0.4), 0 0 20px var(--color-accent-subtle)',
        borderColor: 'var(--color-border-hover)',
      }}
      className="rounded-lg border border-border bg-surface-2 p-4 transition-colors"
    >
      {/* ── Header: filename + score ── */}
      <div className="mb-3 flex items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-2">
          <FileIcon size={14} className={`shrink-0 ${iconColor}`} />
          <button
            onClick={() => {
              if (isVideo) {
                setVideoPreviewPath(card.documentPath);
              } else if (canPreviewInApp(card.documentPath)) {
                openFilePreview(card.documentPath);
              } else {
                openFileInDefaultApp(card.documentPath).catch(() =>
                  toast.error(t('card.fileNotFound')),
                );
              }
            }}
            className="cursor-pointer truncate text-sm font-medium text-text-primary transition-colors hover:text-accent hover:underline"
            title={card.documentPath}
          >
            {card.documentTitle ||
              card.documentPath.split(/[/\\]/).pop()}
          </button>
          {timestamp && (isVideo || isAudio) && (
            <span className="inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-violet-100 text-violet-700 dark:bg-violet-900/30 dark:text-violet-300">
              <Clock className="h-3 w-3 mr-1" />
              {timestamp}
            </span>
          )}
        </div>

        <div className="flex shrink-0 items-center gap-2">
          <span className="text-[11px] text-text-tertiary">{t('card.relevance')}</span>
          <div className="flex items-center gap-1.5">
            <div className="h-1.5 w-16 overflow-hidden rounded-full bg-surface-4">
              <motion.div
                className="h-full rounded-full"
                style={{ backgroundColor: scoreColor(card.score) }}
                initial={{ width: 0 }}
                animate={{ width: `${pct}%` }}
                transition={{
                  duration: 0.6,
                  ease: [0.16, 1, 0.3, 1],
                  delay: 0.2,
                }}
              />
            </div>
            <span
              className="font-mono text-xs font-semibold"
              style={{ color: scoreColor(card.score) }}
            >
              {card.score.toFixed(2)}
            </span>
          </div>
        </div>
      </div>

      {/* ── Heading breadcrumb ── */}
      {card.headingPath.length > 0 && (
        <div className="mb-2 flex items-center gap-1 text-[11px] text-text-tertiary">
          <Hash size={10} className="shrink-0" />
          <span className="truncate">
            {card.headingPath.join(' › ')}
          </span>
        </div>
      )}

      <div className="mb-3 border-t border-border" />

      {/* ── Content snippet ── */}
      <div className="whitespace-pre-wrap text-sm leading-relaxed text-text-secondary">
        {renderHighlights(displayContent, visibleHighlights)}
      </div>

      {/* ── Expand / Collapse ── */}
      {needsTruncation && (
        <button
          onClick={() => setExpanded((v) => !v)}
          className="mt-2 inline-flex cursor-pointer items-center gap-1 text-xs font-medium text-accent transition-colors hover:text-accent-hover"
        >
          {expanded ? (
            <>
              {t('card.collapse')}
              <ChevronUp size={12} />
            </>
          ) : (
            <>
              {t('card.expand')}
              <ChevronDown size={12} />
            </>
          )}
        </button>
      )}

      <div className="mt-3 mb-2 border-t border-border" />

      {/* ── Footer: metadata + feedback ── */}
      <div className="flex items-center justify-between gap-2">
        {/* Metadata */}
        <div className="flex min-w-0 items-center gap-2 text-text-tertiary">
          <button
            onClick={() => {
              showInFileExplorer(card.documentPath).catch(() =>
                toast.error(t('card.fileNotFound')),
              );
            }}
            className="flex cursor-pointer items-center gap-1 text-[11px] transition-colors hover:text-accent"
            title={dir}
          >
            <FolderOpen size={11} className="shrink-0" />
            <span className="max-w-[140px] truncate">{dir}</span>
          </button>
          <span className="text-border">┊</span>
          <span className="max-w-[100px] truncate text-[11px]">
            {card.sourceName}
          </span>
          {ext && (
            <>
              <span className="text-border">┊</span>
              <Badge>{ext}</Badge>
            </>
          )}
        </div>

        {/* File actions */}
        <div className="flex shrink-0 items-center gap-0.5">
          <Tooltip content={t('card.openFile')}>
            <button
              onClick={() => {
                if (canPreviewInApp(card.documentPath)) {
                  openFilePreview(card.documentPath);
                } else {
                  openFileInDefaultApp(card.documentPath).catch(() =>
                    toast.error(t('card.fileNotFound')),
                  );
                }
              }}
              className="cursor-pointer rounded-md p-1.5 text-text-tertiary transition-colors hover:bg-surface-3 hover:text-text-secondary"
            >
              <ExternalLink size={14} />
            </button>
          </Tooltip>

          <Tooltip content={t('card.showInFolder')}>
            <button
              onClick={() => {
                showInFileExplorer(card.documentPath).catch(() =>
                  toast.error(t('card.fileNotFound')),
                );
              }}
              className="cursor-pointer rounded-md p-1.5 text-text-tertiary transition-colors hover:bg-surface-3 hover:text-text-secondary"
            >
              <FolderOpen size={14} />
            </button>
          </Tooltip>

          <SaveToPlaybookButton chunkId={card.chunkId} size="sm" />

          <span className="mx-0.5 h-4 w-px bg-border" />

          {onAskAbout && (
            <Tooltip content={t('chat.askAboutThis')}>
              <button
                onClick={() => {
                  const title = card.documentTitle || card.documentPath.split(/[/\\]/).pop() || '';
                  const heading = card.headingPath?.length ? card.headingPath.join(' > ') : '';
                  const meta = [
                    card.sourceName && `Source: ${card.sourceName}`,
                    card.documentPath && `Path: ${card.documentPath}`,
                    heading && `Section: ${heading}`,
                  ].filter(Boolean).join('\n');
                  const content = card.content.length > 1500
                    ? card.content.slice(0, 1500) + '…'
                    : card.content;
                  onAskAbout(
                    t('chat.askAboutPrompt', { title }) +
                    (meta ? `\n\n${meta}` : '') +
                    `\n\n> ${content}`
                  );
                }}
                className="cursor-pointer rounded-md p-1.5 text-text-tertiary transition-colors hover:bg-accent/10 hover:text-accent"
              >
                <BotMessageSquare size={14} />
              </button>
            </Tooltip>
          )}
        </div>

        {/* Feedback */}
        {onFeedback && (
          <div className="flex shrink-0 items-center gap-0.5">
            <Tooltip content={t('card.upvote')}>
              <button
                onClick={() => onFeedback(card.chunkId, 'upvote')}
                className={`cursor-pointer rounded-md p-1.5 transition-colors ${
                  feedbackState.upvoted
                    ? 'bg-success/15 text-success'
                    : 'text-text-tertiary hover:bg-surface-3 hover:text-text-secondary'
                }`}
              >
                <ThumbsUp
                  size={14}
                  fill={feedbackState.upvoted ? 'currentColor' : 'none'}
                />
              </button>
            </Tooltip>

            <Tooltip content={t('card.downvote')}>
              <button
                onClick={() => onFeedback(card.chunkId, 'downvote')}
                className={`cursor-pointer rounded-md p-1.5 transition-colors ${
                  feedbackState.downvoted
                    ? 'bg-danger/15 text-danger'
                    : 'text-text-tertiary hover:bg-surface-3 hover:text-text-secondary'
                }`}
              >
                <ThumbsDown
                  size={14}
                  fill={feedbackState.downvoted ? 'currentColor' : 'none'}
                />
              </button>
            </Tooltip>

            <Tooltip content={t('card.pin')}>
              <button
                onClick={() => onFeedback(card.chunkId, 'pin')}
                className={`cursor-pointer rounded-md p-1.5 transition-colors ${
                  feedbackState.pinned
                    ? 'bg-accent/15 text-accent'
                    : 'text-text-tertiary hover:bg-surface-3 hover:text-text-secondary'
                }`}
              >
                <Pin
                  size={14}
                  fill={feedbackState.pinned ? 'currentColor' : 'none'}
                />
              </button>
            </Tooltip>
          </div>
        )}
      </div>
    </motion.div>

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
