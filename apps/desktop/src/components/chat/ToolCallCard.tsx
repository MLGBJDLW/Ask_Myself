import { useState, useEffect, useMemo, useRef, useCallback } from 'react';
import type { CSSProperties } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { save as showSaveDialog } from '@tauri-apps/plugin-dialog';
import { motion, AnimatePresence, useReducedMotion } from 'framer-motion';
import { toast } from 'sonner';
import {
  Search,
  BookOpen,
  FileText,
  List,
  ChevronDown,
  ChevronUp,
  Loader2,
  CheckCircle2,
  XCircle,
  Wrench,
  FolderOpen,
  Globe,
  Layers,
  PenLine,
  ClipboardList,
  ShieldCheck,
  Terminal,
  Download,
  ExternalLink,
  Image as ImageIcon,
  Save,
} from 'lucide-react';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import { FileBadge } from '../ui/FileBadge';
import { Button } from '../ui/Button';
import { Tooltip } from '../ui/Tooltip';
import { getSoftCollapseMotion } from '../../lib/uiMotion';
import type { ToolCallEvent } from '../../lib/streaming/protocol';
import {
  isPendingToolCallStatus,
  isUnsuccessfulToolCallStatus,
} from '../../lib/streaming/toolStatus';
import { extractPlanArtifact, extractVerificationArtifact } from '../../lib/taskArtifacts';
import {
  extractSubagentArtifact,
  extractSubagentBatchArtifact,
  extractSubagentJudgementArtifact,
  parseSubagentArguments,
} from '../../lib/subagentArtifacts';
import { PlanPanel, VerificationPanel } from './TaskPanels';
import type { ArtifactPayload, ToolPluginInfo, ToolRenderKind, ToolRunCapabilities } from '../../types/conversation';
import type { VerificationOverallStatus } from '../../lib/taskArtifacts';
import { SubagentCard } from './SubagentCard';
import {
  FileDiffPreview,
  extractDiffStatsArtifact,
  extractFileDiffArtifact,
  type DiffStatsArtifact,
} from './FileDiffPreview';
import { isFileChangeToolRender } from './toolRenderers';

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

interface SearchResultItem {
  score: number;
  source: string;
  path: string;
  title: string;
  preview: string;
}

interface TrustBoundaryArtifact {
  origin?: string;
  authority?: string;
  visibility?: string;
  mutability?: string;
  externality?: string;
  canInstruct?: boolean;
}

interface GeneratedImageArtifact {
  kind: 'generatedImage';
  path?: string;
  previewPath?: string;
  dataUrl?: string;
  mediaType?: string;
  provider?: string;
  model?: string;
  prompt?: string;
  revisedPrompt?: string;
  bytes?: number;
  saved?: boolean;
  transient?: boolean;
  suggestedFilename?: string;
  providerImageUrl?: string;
}

interface ImagePromptArgs {
  prompt?: string;
  size?: string;
  quality?: string;
  outputFormat?: string;
  model?: string;
}

type ToolCallCardStatus = ToolCallEvent['status'];

interface ToolCallCardProps {
  toolName?: string;
  arguments?: string;
  status: ToolCallCardStatus;
  plugin?: ToolPluginInfo;
  renderKind?: ToolRenderKind;
  capabilities?: ToolRunCapabilities;
  durationMs?: number;
  content?: string;
  isError?: boolean;
  artifacts?: ArtifactPayload;
  compact?: boolean;
  inline?: boolean;
  trace?: boolean;
  /** Assembly progress of `arguments` before execution. */
  argsStatus?: ToolCallEvent['argsStatus'];
  /** Total characters of `arguments` received so far. */
  argsBytes?: number;
}

function formatByteCount(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '';
  if (bytes < 1024) return `${bytes} B`;
  const kb = bytes / 1024;
  if (kb < 1024) return `${kb.toFixed(kb < 10 ? 1 : 0)} KB`;
  return `${(kb / 1024).toFixed(1)} MB`;
}

function formatDurationMs(durationMs: number | undefined): string {
  if (typeof durationMs !== 'number' || !Number.isFinite(durationMs) || durationMs < 0) return '';
  if (durationMs < 1000) return `${Math.round(durationMs)} ms`;
  const seconds = durationMs / 1000;
  if (seconds < 60) return `${seconds.toFixed(seconds < 10 ? 1 : 0)} s`;
  const minutes = Math.floor(seconds / 60);
  const remainingSeconds = Math.round(seconds % 60);
  return `${minutes}m ${remainingSeconds}s`;
}

function AnimatedCount({
  value,
  prefix = '',
  className,
}: {
  value: number;
  prefix?: string;
  className?: string;
}) {
  const shouldReduceMotion = useReducedMotion();
  const displayRef = useRef(0);
  const [display, setDisplay] = useState(0);

  useEffect(() => {
    const target = Number.isFinite(value) ? value : 0;
    if (shouldReduceMotion) {
      displayRef.current = target;
      setDisplay(target);
      return;
    }

    const start = displayRef.current;
    const delta = target - start;
    if (Math.abs(delta) < 0.001) return;

    let frame = 0;
    const startedAt = performance.now();
    const duration = Math.min(900, 320 + Math.abs(delta) * 18);
    const easeOutCubic = (t: number) => 1 - Math.pow(1 - t, 3);

    const tick = (now: number) => {
      const progress = Math.min(1, (now - startedAt) / duration);
      const next = start + delta * easeOutCubic(progress);
      displayRef.current = next;
      setDisplay(next);
      if (progress < 1) {
        frame = requestAnimationFrame(tick);
      } else {
        displayRef.current = target;
        setDisplay(target);
      }
    };

    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [shouldReduceMotion, value]);

  return (
    <span className={`inline-block min-w-[1.4ch] text-right tabular-nums ${className ?? ''}`}>
      {prefix}{Math.round(display)}
    </span>
  );
}

function DiffStatsTicker({ stats, compact = false }: { stats: DiffStatsArtifact; compact?: boolean }) {
  const { t } = useTranslation();
  const pillBase = compact
    ? 'h-5 px-1.5 text-[10px]'
    : 'h-6 px-2 text-[11px]';
  const neutralPill = `${pillBase} inline-flex items-center gap-1 rounded-md border border-border/60 bg-surface-0/70 text-text-tertiary`;

  return (
    <div className="inline-flex shrink-0 items-center gap-1 font-mono tabular-nums">
      <span className={`${pillBase} inline-flex items-center gap-0.5 rounded-md border border-success/20 bg-success/10 text-success`}>
        <AnimatedCount value={stats.additions} prefix="+" />
      </span>
      <span className={`${pillBase} inline-flex items-center gap-0.5 rounded-md border border-danger/20 bg-danger/10 text-danger`}>
        <AnimatedCount value={stats.deletions} prefix="-" />
      </span>
      {stats.filesChanged > 1 && (
        <span className={neutralPill}>
          <AnimatedCount value={stats.filesChanged} />
          <span className="font-sans">{t('chat.diffFiles')}</span>
        </span>
      )}
      {typeof stats.replacements === 'number' && stats.replacements > 0 && (
        <span className={`${neutralPill} hidden sm:inline-flex`}>
          <AnimatedCount value={stats.replacements} />
          <span className="font-sans">{t('chat.diffReplacements')}</span>
        </span>
      )}
    </div>
  );
}

function PendingDiffTicker({ compact = false }: { compact?: boolean }) {
  const { t } = useTranslation();
  return (
    <span
      className={`inline-flex shrink-0 items-center gap-1 rounded-md border border-accent/20 bg-accent/10 font-mono tabular-nums text-accent ${
        compact ? 'h-5 px-1.5 text-[10px]' : 'h-6 px-2 text-[11px]'
      }`}
    >
      <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-accent" />
      {t('chat.diffPending')}
    </span>
  );
}

function DiffStatsSummaryPanel({ stats }: { stats: DiffStatsArtifact }) {
  const { t } = useTranslation();
  const path = stats.paths[0];
  return (
    <div className="flex flex-wrap items-center gap-2 rounded-lg border border-border/60 bg-surface-0/65 px-3 py-2">
      <DiffStatsTicker stats={stats} />
      {path && <FileBadge path={path} className="min-w-0 max-w-full" />}
      {stats.hunks > 0 && (
        <span className="rounded-md border border-border/60 bg-surface-1 px-2 py-1 text-[11px] text-text-tertiary">
          {t('chat.diffHunks', { count: String(stats.hunks) })}
        </span>
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

const TOOL_ICONS: Record<string, typeof Search> = {
  search_playbooks: BookOpen,
  search_sessions: BookOpen,
  search_by_date: Search,
  search: Search,
  code_intelligence: Search,
  grep_files: Search,
  glob_files: FolderOpen,
  read_files: FileText,
  read_file: FileText,
  get_document_info: FileText,
  compare_documents: Layers,
  summarize_document: List,
  retrieve_evidence: BookOpen,
  query_knowledge_graph: Layers,
  get_related_concepts: Layers,
  list_documents: List,
  list_sources: BookOpen,
  compile_document: ClipboardList,
  desktop_automation: Globe,
  project_tool: Wrench,
  playbook: BookOpen,
  multi_edit: PenLine,
  edit_file: PenLine,
  file: FileText,
  summarize: List,
  list_dir: FolderOpen,
  web_search: Globe,
  fetch_url: Globe,
  download_asset: Download,
  chunk_context: Layers,
  write_note: PenLine,
  update_plan: ClipboardList,
  record_verification: ShieldCheck,
  run_shell: Terminal,
  generate_image: ImageIcon,
};

function getToolIcon(name?: string) {
  const lower = (name || '').toLowerCase();
  for (const [key, Icon] of Object.entries(TOOL_ICONS)) {
    if (lower.includes(key)) return Icon;
  }
  return Wrench;
}

function parseSearchResults(content: string): SearchResultItem[] | null {
  const blocks = content.split(/---\s*Result\s+\d+\s*\(score:\s*([\d.]+)\)\s*---/);
  // blocks[0] is preamble (e.g. "Found N results:"), then pairs of [score, body]
  if (blocks.length < 3) return null;

  const items: SearchResultItem[] = [];
  for (let i = 1; i < blocks.length; i += 2) {
    const score = parseFloat(blocks[i]);
    const body = (blocks[i + 1] || '').trim();

    const get = (key: string): string => {
      const m = body.match(new RegExp(`^${key}:\\s*(.+)`, 'm'));
      return m ? m[1].trim() : '';
    };

    const contentMatch = body.match(/^Content:\s*\n([\s\S]*)/m);
    const preview = contentMatch ? contentMatch[1].trim().slice(0, 200) : '';

    items.push({ score, source: get('Source'), path: get('Path'), title: get('Title'), preview });
  }
  return items.length > 0 ? items : null;
}

function formatArgs(raw?: string): string {
  if (!raw) return '';
  try {
    const parsed = JSON.parse(raw);
    return Object.entries(parsed)
      .map(([k, v]) => `${k}: ${JSON.stringify(v)}`)
      .join(', ');
  } catch {
    return raw;
  }
}

function getToolBriefLabel(name: string, args?: string): string {
  if (!args) return name;
  try {
    const parsed = JSON.parse(args);
    const key = parsed.path || parsed.file || parsed.filename || parsed.query || parsed.program;
    if (key && typeof key === 'string') {
      const short = key.length > 25 ? '\u2026' + key.slice(-22) : key;
      return `${name}(${short})`;
    }
  } catch { /* ignore */ }
  return name;
}

function getToolBriefResult(
  status: ToolCallCardStatus,
  t: ReturnType<typeof useTranslation>['t'],
  content?: string,
  toolName?: string,
): string {
  if (isPendingToolCallStatus(status)) return '\u2026';
  if (status === 'error' || status === 'timedOut') return t('chat.toolBriefError');
  if (status === 'declined') return t('chat.toolBriefDeclined');
  if (status === 'cancelled') return t('chat.toolBriefCancelled');
  const lower = (toolName || '').toLowerCase();
  if (lower.includes('search') && content) {
    const match = content.match(/Found (\d+) result/i);
    if (match) return t('search.results', { count: match[1] });
  }
  if (content) {
    const lines = content.split('\n').length;
    if (lines > 3) return t('chat.toolBriefLines', { count: String(lines) });
  }
  return t('chat.toolBriefDone');
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value));
}

function extractTrustBoundary(
  artifacts: ArtifactPayload | undefined,
): TrustBoundaryArtifact | null {
  if (!isRecord(artifacts)) return null;
  const boundary = artifacts.trustBoundary;
  if (!isRecord(boundary)) return null;
  return boundary as TrustBoundaryArtifact;
}

function extractGeneratedImageArtifact(
  artifacts: ArtifactPayload | undefined,
): GeneratedImageArtifact | null {
  if (!isRecord(artifacts)) return null;
  if (artifacts.kind !== 'generatedImage') return null;
  return artifacts as unknown as GeneratedImageArtifact;
}

function parseImagePromptArgs(raw?: string): ImagePromptArgs {
  if (!raw) return {};
  try {
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    return {
      prompt: typeof parsed.prompt === 'string' ? parsed.prompt : undefined,
      size: typeof parsed.size === 'string' ? parsed.size : undefined,
      quality: typeof parsed.quality === 'string' ? parsed.quality : undefined,
      outputFormat: typeof parsed.output_format === 'string'
        ? parsed.output_format
        : typeof parsed.outputFormat === 'string'
          ? parsed.outputFormat
          : undefined,
      model: typeof parsed.model === 'string' ? parsed.model : undefined,
    };
  } catch {
    return {};
  }
}

function imageAspectStyle(size?: string): CSSProperties {
  const raw = (size ?? '').trim();
  const pixelMatch = raw.match(/(\d{2,5})\s*[x*]\s*(\d{2,5})/i);
  if (pixelMatch) {
    const width = Number(pixelMatch[1]);
    const height = Number(pixelMatch[2]);
    if (width > 0 && height > 0) return { aspectRatio: `${width} / ${height}` };
  }
  const ratioMatch = raw.match(/(\d{1,3})\s*:\s*(\d{1,3})/);
  if (ratioMatch) {
    const width = Number(ratioMatch[1]);
    const height = Number(ratioMatch[2]);
    if (width > 0 && height > 0) return { aspectRatio: `${width} / ${height}` };
  }
  return { aspectRatio: '1 / 1' };
}

function generatedImagePreviewPath(image: GeneratedImageArtifact): string {
  return image.previewPath || image.path || '';
}

function extensionForMediaType(mediaType?: string): string {
  const lower = (mediaType ?? '').toLowerCase();
  if (lower.includes('jpeg') || lower.includes('jpg')) return 'jpg';
  if (lower.includes('webp')) return 'webp';
  if (lower.includes('gif')) return 'gif';
  return 'png';
}

function generatedImageSuggestedFilename(image: GeneratedImageArtifact): string {
  const raw = (image.suggestedFilename ?? '').trim();
  if (raw) return raw;
  return `generated-image.${extensionForMediaType(image.mediaType)}`;
}

function GeneratedImagePreview({
  image,
  compact = false,
}: {
  image: GeneratedImageArtifact;
  compact?: boolean;
}) {
  const { t } = useTranslation();
  const previewPath = generatedImagePreviewPath(image);
  const [previewSrc, setPreviewSrc] = useState(image.dataUrl ?? '');
  const [imageError, setImageError] = useState('');
  const [isSaving, setIsSaving] = useState(false);
  const [savedPath, setSavedPath] = useState('');
  const prompt = image.revisedPrompt || image.prompt;
  const maxHeight = compact ? 'max-h-32' : 'max-h-[28rem]';
  const suggestedFilename = generatedImageSuggestedFilename(image);
  const outputExtension = extensionForMediaType(image.mediaType);

  useEffect(() => {
    let cancelled = false;
    setImageError('');
    if (image.dataUrl) {
      setPreviewSrc(image.dataUrl);
      return () => {
        cancelled = true;
      };
    }
    if (!previewPath) {
      setPreviewSrc('');
      setImageError(t('chat.generatedImageLoadFailed'));
      return () => {
        cancelled = true;
      };
    }

    setPreviewSrc('');
    api.readGeneratedImageDataUrl(previewPath, image.mediaType)
      .then((dataUrl) => {
        if (!cancelled) setPreviewSrc(dataUrl);
      })
      .catch(() => {
        if (!cancelled) setPreviewSrc(convertFileSrc(previewPath));
      });

    return () => {
      cancelled = true;
    };
  }, [image.dataUrl, image.mediaType, previewPath, t]);

  const handleSave = useCallback(async () => {
    if (isSaving) return;
    const dataUrlForSave = image.dataUrl || (previewSrc.startsWith('data:image/') ? previewSrc : '');
    if (!previewPath && !dataUrlForSave) {
      toast.error(t('chat.generatedImageLoadFailed'));
      return;
    }

    setIsSaving(true);
    try {
      const outputPath = await showSaveDialog({
        defaultPath: suggestedFilename,
        filters: [
          {
            name: t('chat.generatedImageAlt'),
            extensions: [outputExtension],
          },
        ],
      });
      if (!outputPath) return;

      const result = await api.saveGeneratedImage({
        outputPath,
        sourcePath: previewPath || null,
        dataUrl: previewPath ? null : dataUrlForSave,
        mediaType: image.mediaType ?? null,
      });
      setSavedPath(result.path);
      toast.success(t('chat.generatedImageSaveSuccess'));
    } catch (error) {
      toast.error(`${t('chat.generatedImageSaveFailed')}: ${String(error)}`);
    } finally {
      setIsSaving(false);
    }
  }, [
    image.dataUrl,
    image.mediaType,
    isSaving,
    outputExtension,
    previewPath,
    previewSrc,
    suggestedFilename,
    t,
  ]);

  const handleOpenSaved = useCallback(() => {
    if (!savedPath) return;
    api.openFileInDefaultApp(savedPath).catch((error) => {
      toast.error(String(error));
    });
  }, [savedPath]);

  const handleRevealSaved = useCallback(() => {
    if (!savedPath) return;
    api.showInFileExplorer(savedPath).catch((error) => {
      toast.error(String(error));
    });
  }, [savedPath]);

  return (
    <div className={compact ? 'space-y-1.5' : 'space-y-2.5'} data-testid="generated-image-preview">
      <div className="overflow-hidden rounded-md border border-border/60 bg-surface-0">
        {previewSrc && !imageError ? (
          <img
            src={previewSrc}
            alt={image.prompt || t('chat.generatedImageAlt')}
            className={`${maxHeight} w-full object-contain`}
            data-testid="generated-image-img"
            onError={() => setImageError(t('chat.generatedImageLoadFailed'))}
          />
        ) : (
          <div className={`${compact ? 'min-h-28' : 'min-h-64'} flex items-center justify-center px-4 text-center text-xs text-text-tertiary`}>
            {imageError || t('chat.generatedImageLoading')}
          </div>
        )}
      </div>
      <div className="grid gap-1.5 text-[11px] text-text-tertiary">
        <div className="flex flex-wrap gap-1.5">
          {!compact && (
            <span className="rounded-md border border-accent/25 bg-accent/10 px-1.5 py-0.5 text-accent">
              {savedPath ? t('chat.generatedImageSaved') : t('chat.generatedImageUnsaved')}
            </span>
          )}
          {image.provider && (
            <span className="rounded-md border border-border/50 bg-surface-0/50 px-1.5 py-0.5">
              {image.provider}
            </span>
          )}
          {image.model && (
            <span className="rounded-md border border-border/50 bg-surface-0/50 px-1.5 py-0.5">
              {image.model}
            </span>
          )}
          {image.mediaType && (
            <span className="rounded-md border border-border/50 bg-surface-0/50 px-1.5 py-0.5">
              {image.mediaType.replace('image/', '').toUpperCase()}
            </span>
          )}
          {typeof image.bytes === 'number' && (
            <span className="rounded-md border border-border/50 bg-surface-0/50 px-1.5 py-0.5">
              {formatByteCount(image.bytes)}
            </span>
          )}
        </div>
        {!compact && prompt && (
          <div className="line-clamp-2 text-text-secondary">{prompt}</div>
        )}
        {!compact && (
          <div className="flex flex-wrap items-center gap-1.5">
            <Button
              type="button"
              size="sm"
              variant="secondary"
              icon={<Save className="h-3.5 w-3.5" />}
              loading={isSaving}
              onClick={handleSave}
              disabled={!previewPath && !previewSrc}
              aria-label={t('chat.generatedImageSaveAs')}
            >
              {isSaving ? t('chat.generatedImageSaving') : t('chat.generatedImageSaveAs')}
            </Button>
            {savedPath && (
              <>
                <Tooltip content={savedPath} side="bottom">
                  <span className="min-w-0 max-w-[22rem] truncate text-text-secondary">
                    {savedPath}
                  </span>
                </Tooltip>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  icon={<ExternalLink className="h-3.5 w-3.5" />}
                  onClick={handleOpenSaved}
                >
                  {t('chat.generatedImageOpen')}
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  icon={<FolderOpen className="h-3.5 w-3.5" />}
                  onClick={handleRevealSaved}
                >
                  {t('chat.generatedImageReveal')}
                </Button>
              </>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

function ImageGenerationPendingPreview({
  prompt,
  size,
  compact = false,
}: {
  prompt?: string;
  size?: string;
  compact?: boolean;
}) {
  const shouldReduceMotion = useReducedMotion();
  const aspectStyle = imageAspectStyle(size);

  return (
    <div
      className={`chat-image-loading relative overflow-hidden rounded-md border border-accent/25 bg-surface-0/80 ${
        compact ? 'min-h-28' : 'min-h-64'
      }`}
      data-reduce-motion={shouldReduceMotion ? 'true' : 'false'}
      style={aspectStyle}
    >
      <div className="absolute inset-0 opacity-80">
        <div className="absolute left-[9%] top-[11%] h-[16%] w-[28%] rounded-md border border-border/55 bg-surface-1/60" />
        <div className="absolute right-[9%] top-[12%] h-[9%] w-[18%] rounded-full border border-border/50 bg-surface-1/55" />
        <div className="absolute bottom-[18%] left-[8%] h-[35%] w-[84%] rounded-md border border-border/50 bg-surface-1/45" />
        <div className="absolute bottom-[24%] left-[13%] h-[20%] w-[24%] rounded-md border border-border/45 bg-surface-2/45" />
        <div className="absolute bottom-[25%] right-[14%] h-[24%] w-[36%] rounded-md border border-border/45 bg-surface-2/35" />
      </div>
      <div className="chat-image-loading-grid absolute inset-0" />
      <div className="chat-image-loading-scan absolute inset-y-0 w-1/3" />
      <div className="absolute inset-0 flex items-center justify-center">
        <motion.div
          animate={shouldReduceMotion ? undefined : { opacity: [0.64, 1, 0.64], scale: [0.96, 1.04, 0.96] }}
          transition={{ duration: 2.4, repeat: Infinity, ease: 'easeInOut' }}
          className="flex h-14 w-14 items-center justify-center rounded-md border border-accent/30 bg-surface-0/80 shadow-glow"
        >
          <img src="/logo-small.svg" alt="" className="h-8 w-8 opacity-90" />
        </motion.div>
      </div>
      {!compact && prompt && (
        <div className="absolute inset-x-0 bottom-0 border-t border-border/40 bg-surface-0/75 px-3 py-2">
          <div className="line-clamp-2 text-xs leading-5 text-text-secondary">{prompt}</div>
        </div>
      )}
    </div>
  );
}

function verificationStatusLabel(
  status: VerificationOverallStatus,
  t: ReturnType<typeof useTranslation>['t'],
) {
  switch (status) {
    case 'passed':
      return t('chat.verificationPassed');
    case 'failed':
      return t('chat.verificationFailed');
    case 'partial':
      return t('chat.verificationPartial');
    case 'pending':
    default:
      return t('chat.verificationPending');
  }
}

function trustAuthorityLabel(authority: string | undefined, t: ReturnType<typeof useTranslation>['t']) {
  switch (authority) {
    case 'evidence':
      return t('chat.contextManifestRoleEvidence');
    case 'observation':
      return t('chat.trustAuthorityObservation');
    default:
      return authority || t('chat.contextManifestRoleEvidence');
  }
}

function trustVisibilityLabel(visibility: string | undefined, t: ReturnType<typeof useTranslation>['t']) {
  switch (visibility) {
    case 'source_scope':
      return t('chat.trustVisibilitySourceScope');
    case 'workspace':
      return t('chat.trustVisibilityWorkspace');
    case 'current_chat':
      return t('chat.trustVisibilityCurrentChat');
    default:
      return visibility || '';
  }
}

function buildSubagentRun(
  toolName: string,
  args: string | undefined,
  status: ToolCallCardStatus,
  content: string | undefined,
  isError: boolean | undefined,
  artifacts: ArtifactPayload | undefined,
) {
  if (toolName !== 'spawn_subagent') return null;
  const artifact = extractSubagentArtifact(artifacts);
  const parsedArgs = parseSubagentArguments(args);
  const task = artifact?.task ?? parsedArgs?.task;
  if (!task) return null;
  const runStatus: 'running' | 'done' | 'error' =
    isPendingToolCallStatus(status)
      ? 'running'
      : status === 'done'
        ? 'done'
        : 'error';
  return {
    id: `${toolName}-${task}`,
    status: runStatus,
    task,
    roleId: artifact?.roleId ?? parsedArgs?.roleId ?? null,
    roleName: artifact?.roleName ?? null,
    role: artifact?.role ?? parsedArgs?.role ?? null,
    expectedOutput: artifact?.expectedOutput ?? parsedArgs?.expectedOutput ?? null,
    acceptanceCriteria: artifact?.acceptanceCriteria ?? parsedArgs?.acceptanceCriteria ?? null,
    evidenceChunkIds: artifact?.evidenceChunkIds ?? parsedArgs?.evidenceChunkIds ?? null,
    evidenceHandoff: artifact?.evidenceHandoff ?? null,
    requestedSourceScope: artifact?.requestedSourceScope ?? parsedArgs?.sourceIds ?? null,
    effectiveSourceScope: artifact?.effectiveSourceScope ?? null,
    requestedAllowedTools: artifact?.requestedAllowedTools ?? parsedArgs?.allowedTools ?? null,
    allowedSkills: artifact?.allowedSkills ?? null,
    parallelGroup: artifact?.parallelGroup ?? parsedArgs?.parallelGroup ?? null,
    deliverableStyle: artifact?.deliverableStyle ?? parsedArgs?.deliverableStyle ?? null,
    returnSections: artifact?.returnSections ?? parsedArgs?.returnSections ?? null,
    result: artifact?.result ?? undefined,
    finishReason: artifact?.finishReason ?? null,
    usageTotal: artifact?.usageTotal ?? null,
    toolEvents: artifact?.toolEvents ?? [],
    thinking: artifact?.thinking ?? null,
    sourceScopeApplied: artifact?.sourceScopeApplied ?? false,
    allowedTools: artifact?.allowedTools ?? null,
    argumentsText: args,
    isError,
    content,
  };
}

/* ------------------------------------------------------------------ */
/*  Sub-components                                                     */
/* ------------------------------------------------------------------ */

function SearchResultCards({ items }: { items: SearchResultItem[] }) {
  return (
    <div className="space-y-2">
      {items.map((item, i) => (
        <div
          key={i}
          className="flex items-start gap-2 p-2 rounded-md bg-surface-0/50 border border-border/50"
        >
          {/* Score indicator */}
          <div
            className={`shrink-0 w-1 h-8 rounded-full ${
              item.score >= 0.8
                ? 'bg-success'
                : item.score >= 0.5
                  ? 'bg-warning'
                  : 'bg-text-tertiary'
            }`}
          />
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2 mb-0.5">
              <FileBadge path={item.path} />
              <span className="text-[11px] text-text-tertiary">
                {(item.score * 100).toFixed(0)}%
              </span>
            </div>
            {item.title && (
              <div className="text-xs font-medium text-text-primary truncate">
                {item.title}
              </div>
            )}
            {item.preview && (
              <div className="text-[11px] text-text-secondary line-clamp-2 mt-0.5">
                {item.preview}
              </div>
            )}
          </div>
        </div>
      ))}
    </div>
  );
}

function TrustBoundaryPills({ boundary }: { boundary: TrustBoundaryArtifact }) {
  const { t } = useTranslation();
  const visibilityLabel = trustVisibilityLabel(boundary.visibility, t);
  return (
    <div className="mb-2 flex flex-wrap gap-1.5 text-[10px] text-text-tertiary">
      <span className="rounded-md border border-border/50 bg-surface-0/50 px-1.5 py-0.5">
        {trustAuthorityLabel(boundary.authority, t)}
      </span>
      <span className="rounded-md border border-border/50 bg-surface-0/50 px-1.5 py-0.5">
        {t('chat.trustCanInstruct')}: {boundary.canInstruct ? t('chat.trustCanInstructYes') : t('chat.trustCanInstructNo')}
      </span>
      {visibilityLabel && (
        <span className="rounded-md border border-border/50 bg-surface-0/50 px-1.5 py-0.5">
          {visibilityLabel}
        </span>
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function ToolCallCard({
  toolName,
  arguments: args,
  status,
  plugin,
  renderKind,
  capabilities,
  durationMs,
  content,
  isError,
  artifacts,
  compact,
  inline,
  trace,
  argsStatus,
  argsBytes,
}: ToolCallCardProps) {
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();
  const safeToolName =
    typeof toolName === 'string' && toolName.trim().length > 0
      ? toolName
      : 'unknown_tool';
  const Icon = getToolIcon(safeToolName);
  const formattedArgs = formatArgs(args);
  const briefLabel = getToolBriefLabel(safeToolName, args);
  const briefResult = getToolBriefResult(status, t, content, safeToolName);
  const isPending = isPendingToolCallStatus(status);
  const argsByteLabel = formatByteCount(
    typeof argsBytes === 'number' ? argsBytes : (args ? args.length : 0),
  );
  const durationLabel = formatDurationMs(durationMs);
  const resourceKeyCount = Array.isArray(capabilities?.resourceKeys)
    ? capabilities.resourceKeys.length
    : 0;
  const capabilitySummary = capabilities
    ? [
        plugin?.name ?? null,
        capabilities.readOnly ? t('chat.capabilityReadOnly') : t('chat.capabilityWrites'),
        capabilities.concurrencySafe ? t('chat.capabilityParallel') : t('chat.capabilitySerial'),
        capabilities.interruptBehavior === 'cancel' ? t('chat.capabilityCancellable') : t('chat.capabilityBlocking'),
        resourceKeyCount > 0 ? t('chat.capabilityResources', { count: String(resourceKeyCount) }) : null,
      ].filter(Boolean).join(' · ')
    : null;
  const streamingArgsPreview =
    isPending && (argsStatus === 'streaming' || status === 'starting' || status === 'approvalPending') && args
      ? args.length > 500 ? args.slice(0, 500) + '\u2026' : args
      : null;
  const subagentRun = useMemo(
    () => buildSubagentRun(safeToolName, args, status, content, isError, artifacts),
    [safeToolName, args, status, content, isError, artifacts],
  );
  const subagentBatch = useMemo(() => extractSubagentBatchArtifact(artifacts), [artifacts]);
  const subagentJudgement = useMemo(() => extractSubagentJudgementArtifact(artifacts), [artifacts]);
  const planArtifact = useMemo(() => extractPlanArtifact(artifacts), [artifacts]);
  const verificationArtifact = useMemo(() => extractVerificationArtifact(artifacts), [artifacts]);
  const fileDiff = useMemo(() => extractFileDiffArtifact(artifacts), [artifacts]);
  const diffStats = useMemo(() => extractDiffStatsArtifact(artifacts), [artifacts]);
  const trustBoundary = useMemo(() => extractTrustBoundary(artifacts), [artifacts]);
  const generatedImage = useMemo(() => extractGeneratedImageArtifact(artifacts), [artifacts]);
  const imageArgs = useMemo(() => parseImagePromptArgs(args), [args]);
  const isImageRender = renderKind === 'image' || safeToolName.toLowerCase() === 'generate_image';
  const showImagePendingPreview = isImageRender && isPending && !generatedImage;
  const showPendingDiffStats = isPending && !diffStats && isFileChangeToolRender(safeToolName, renderKind);
  const isSearchDone =
    safeToolName.toLowerCase().includes('search') && status === 'done' && !!content;
  const searchItems = useMemo(
    () => (isSearchDone ? parseSearchResults(content!) : null),
    [isSearchDone, content],
  );

  const [expanded, setExpanded] = useState(false);

  // Auto-collapse file mutation details when execution finishes; users can manually re-open.
  useEffect(() => {
    if (!isPending) {
      setExpanded(false);
    }
  }, [isPending]);

  if (inline) {
    return (
      <span className="inline-flex items-center gap-1">
        <Icon className="h-2.5 w-2.5 shrink-0" />
        <span className="font-medium text-text-secondary">{briefLabel}</span>
        <span className="text-text-tertiary/40">→</span>
        <span>{briefResult}</span>
      </span>
    );
  }

  const statusConfig = {
    preparing: { icon: Loader2, text: t('chat.toolRunning'), color: 'text-accent', spin: true },
    starting: { icon: Loader2, text: t('chat.toolRunning'), color: 'text-accent', spin: true },
    approvalPending: { icon: Loader2, text: t('chat.toolRunning'), color: 'text-accent', spin: true },
    running: { icon: Loader2, text: t('chat.toolRunning'), color: 'text-accent', spin: true },
    done: { icon: CheckCircle2, text: t('chat.toolDone'), color: 'text-success', spin: false },
    error: { icon: XCircle, text: t('chat.toolError'), color: 'text-danger', spin: false },
    declined: { icon: XCircle, text: t('chat.toolError'), color: 'text-danger', spin: false },
    cancelled: { icon: XCircle, text: t('chat.toolError'), color: 'text-danger', spin: false },
    timedOut: { icon: XCircle, text: t('chat.toolError'), color: 'text-danger', spin: false },
  }[status];
  const baseHeaderSummary = planArtifact
    ? t('chat.planStepsCompleted', {
      completed: String(planArtifact.steps.filter(step => step.status === 'completed').length),
      total: String(planArtifact.steps.length),
    })
      : verificationArtifact
        ? t('chat.verificationStatus', {
          status: verificationStatusLabel(verificationArtifact.overallStatus ?? 'pending', t),
        })
      : searchItems
        ? t('search.results', { count: String(searchItems.length) })
        : generatedImage
          ? t('chat.generatedImageReady')
        : showImagePendingPreview
          ? t('chat.generatedImageLoading')
        : diffStats
          ? `${diffStats.operation === 'create' ? t('chat.fileDiffCreated') : t('chat.fileDiffModified')}`
        : showPendingDiffStats
          ? t('chat.fileDiffModified')
        : status === 'done' && content
          ? briefResult
          : statusConfig.text;
  const headerSummary =
    isPending && argsByteLabel
      ? `${baseHeaderSummary} · ${argsByteLabel}`
      : !isPending && durationLabel
        ? `${baseHeaderSummary} · ${durationLabel}`
      : baseHeaderSummary;

  const StatusIcon = statusConfig.icon;
  const traceActive = isPending && !shouldReduceMotion;
  const traceSoft = status !== 'error';
  const expandableDetails = Boolean(
    formattedArgs ||
    content ||
    searchItems ||
    subagentRun ||
    subagentBatch ||
    subagentJudgement ||
    planArtifact ||
    verificationArtifact ||
    fileDiff ||
    diffStats ||
    generatedImage ||
    showImagePendingPreview ||
    streamingArgsPreview,
  );
  const failedStatus = isUnsuccessfulToolCallStatus(status);
  const traceToneClass = failedStatus
    ? 'border-danger/25 bg-danger/10 hover:bg-danger/15'
    : isPending
      ? 'border-accent/25 bg-accent/10 hover:bg-accent/15'
      : 'border-border/45 bg-surface-0/35 hover:border-border/70 hover:bg-surface-0/55';
  const traceDetailBorderClass = failedStatus
    ? 'border-danger/25'
    : isPending
      ? 'border-accent/25'
      : 'border-border/35';
  const tracePreviewText = isPending ? '' : headerSummary;

  if (trace) {
    return (
      <div className="my-1 max-w-full">
        <button
          type="button"
          onClick={() => expandableDetails && setExpanded((prev) => !prev)}
          aria-expanded={expandableDetails ? expanded : undefined}
          className={`group inline-flex min-h-7 max-w-full items-center gap-1.5 rounded-full border px-2.5 py-1 text-left transition-colors disabled:cursor-default ${expandableDetails ? 'cursor-pointer' : 'cursor-default'} ${traceToneClass}`}
          disabled={!expandableDetails}
          title={capabilitySummary ?? undefined}
        >
          <Icon className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
          <span className="min-w-0 max-w-[13rem] truncate text-[11px] font-medium text-text-secondary group-hover:text-text-primary sm:max-w-[17rem]">
            {briefLabel}
          </span>
          {tracePreviewText && (
            <span className="hidden min-w-0 max-w-[18rem] truncate text-[11px] text-text-tertiary sm:inline">
              {tracePreviewText}
            </span>
          )}
          <span className={`inline-flex shrink-0 items-center gap-1 text-[11px] ${statusConfig.color}`}>
            <StatusIcon className={`h-3.5 w-3.5 shrink-0 ${statusConfig.spin ? 'animate-spin' : ''}`} />
          </span>
          {diffStats ? (
            <DiffStatsTicker stats={diffStats} compact />
          ) : showPendingDiffStats ? (
            <PendingDiffTicker compact />
          ) : null}
          {expandableDetails && (
            expanded
              ? <ChevronUp className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
              : <ChevronDown className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
          )}
        </button>

        <AnimatePresence initial={false}>
          {expanded && expandableDetails && (
            <motion.div
              {...getSoftCollapseMotion(!!shouldReduceMotion)}
              className="overflow-hidden"
            >
              <div className={`ml-4 mt-1 space-y-2 border-l py-1.5 pl-3 pr-1 ${traceDetailBorderClass}`}>
                {streamingArgsPreview ? (
                  <pre className="whitespace-pre-wrap break-words rounded-md bg-surface-0/35 px-2 py-1 text-[11px] leading-relaxed text-text-tertiary">
                    {streamingArgsPreview}
                  </pre>
                ) : formattedArgs ? (
                  <div className="break-words rounded-md bg-surface-0/35 px-2 py-1 text-[11px] leading-relaxed text-text-tertiary">
                    {formattedArgs}
                  </div>
                ) : null}
                {generatedImage ? (
                  <GeneratedImagePreview image={generatedImage} compact />
                ) : showImagePendingPreview ? (
                  <ImageGenerationPendingPreview
                    prompt={imageArgs.prompt}
                    size={imageArgs.size}
                    compact
                  />
                ) : subagentRun ? (
                  <SubagentCard run={subagentRun} compact defaultOpen />
                ) : subagentBatch ? (
                  <div className="space-y-2">
                    {subagentBatch.runs.map((run) => (
                      <SubagentCard key={run.id} run={run} compact defaultOpen />
                    ))}
                  </div>
                ) : subagentJudgement ? (
                  <div className="border-l border-border/50 pl-3">
                    <div className="mb-1 flex flex-wrap items-center gap-1.5 text-xs text-text-secondary">
                      <span className="font-medium text-text-primary">
                        {subagentJudgement.task || t('chat.subagentJudgementFallback')}
                      </span>
                      <span className="rounded-full border border-border/60 bg-surface-0/55 px-2 py-0.5">
                        {subagentJudgement.decisionMode}
                      </span>
                      {subagentJudgement.confidence && (
                        <span className="rounded-full border border-border/60 bg-surface-0/55 px-2 py-0.5">
                          {t('chat.subagentConfidence', { value: subagentJudgement.confidence })}
                        </span>
                      )}
                    </div>
                    <div className="text-sm text-text-primary">{subagentJudgement.summary}</div>
                    {subagentJudgement.rationale && (
                      <div className="mt-2 text-xs text-text-secondary">{subagentJudgement.rationale}</div>
                    )}
                  </div>
                ) : searchItems ? (
                  <>
                    {trustBoundary && <TrustBoundaryPills boundary={trustBoundary} />}
                    <SearchResultCards items={searchItems} />
                  </>
                ) : planArtifact ? (
                  <PlanPanel plan={planArtifact} />
                ) : verificationArtifact ? (
                  <VerificationPanel verification={verificationArtifact} />
                ) : fileDiff ? (
                  <FileDiffPreview diff={fileDiff} compact />
                ) : diffStats ? (
                  <>
                    <DiffStatsSummaryPanel stats={diffStats} />
                    {content && (
                      <pre className={`whitespace-pre-wrap break-words text-[11px] leading-relaxed ${isError ? 'text-danger' : 'text-text-secondary'}`}>
                        {content}
                      </pre>
                    )}
                  </>
                ) : content ? (
                  <pre className={`whitespace-pre-wrap break-words text-[11px] leading-relaxed ${isError ? 'text-danger' : 'text-text-secondary'}`}>
                    {content}
                  </pre>
                ) : null}
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    );
  }

  if (compact) {
    return (
      <div className="my-1 max-w-full">
        <button
          type="button"
          onClick={() => expandableDetails && setExpanded((p) => !p)}
          aria-expanded={expandableDetails ? expanded : undefined}
          disabled={!expandableDetails}
          className={`inline-flex min-h-6 max-w-full items-center gap-1.5 rounded-full border px-2 py-0.5 text-left transition-colors disabled:cursor-default ${expandableDetails ? 'cursor-pointer' : 'cursor-default'} ${traceToneClass}`}
        >
          <Icon className="h-3 w-3 shrink-0 text-text-tertiary" />
          <span className="max-w-[12rem] truncate text-[11px] font-medium text-text-secondary">{briefLabel}</span>
          <span className="hidden max-w-[16rem] truncate text-[10px] text-text-tertiary sm:inline">{headerSummary}</span>
          {diffStats ? (
            <DiffStatsTicker stats={diffStats} compact />
          ) : showPendingDiffStats ? (
            <PendingDiffTicker compact />
          ) : null}
          <StatusIcon
            className={`h-3 w-3 shrink-0 ${statusConfig.color} ${statusConfig.spin ? 'animate-spin' : ''}`}
          />
          {expandableDetails && (
            expanded
              ? <ChevronUp className="h-3 w-3 shrink-0 text-text-tertiary" />
              : <ChevronDown className="h-3 w-3 shrink-0 text-text-tertiary" />
          )}
        </button>
        <AnimatePresence initial={false}>
          {expanded && expandableDetails && (
            <motion.div
              {...getSoftCollapseMotion(!!shouldReduceMotion)}
              className="overflow-hidden"
            >
              <div className={`ml-4 mt-1 space-y-1.5 border-l py-1 pl-2.5 pr-1 ${traceDetailBorderClass}`}>
                {streamingArgsPreview ? (
                  <pre className="whitespace-pre-wrap break-words rounded bg-surface-0/40 px-1.5 py-0.5 text-[10px] text-text-tertiary">
                    {streamingArgsPreview}
                  </pre>
                ) : formattedArgs ? (
                  <div className="break-words rounded bg-surface-0/40 px-1.5 py-0.5 text-[10px] text-text-tertiary">
                    {formattedArgs}
                  </div>
                ) : null}
                {generatedImage ? (
                  <GeneratedImagePreview image={generatedImage} compact />
                ) : showImagePendingPreview ? (
                  <ImageGenerationPendingPreview
                    prompt={imageArgs.prompt}
                    size={imageArgs.size}
                    compact
                  />
                ) : subagentRun ? (
                  <SubagentCard run={subagentRun} compact defaultOpen />
                ) : subagentBatch ? (
                  <div className="space-y-1.5">
                    {subagentBatch.runs.map((run) => (
                      <SubagentCard key={run.id} run={run} compact defaultOpen />
                    ))}
                  </div>
                ) : subagentJudgement ? (
                  <div className="border-l border-border/50 pl-2.5">
                    <div className="mb-1 flex flex-wrap items-center gap-1.5 text-[11px] text-text-secondary">
                      <span className="font-medium text-text-primary">
                        {subagentJudgement.task || t('chat.subagentJudgementFallback')}
                      </span>
                      <span className="rounded-full border border-border/60 bg-surface-0/55 px-1.5 py-0.5">
                        {subagentJudgement.decisionMode}
                      </span>
                    </div>
                    <div className="text-xs text-text-primary">{subagentJudgement.summary}</div>
                  </div>
                ) : searchItems ? (
                  <>
                    {trustBoundary && <TrustBoundaryPills boundary={trustBoundary} />}
                    <SearchResultCards items={searchItems} />
                  </>
                ) : planArtifact ? (
                  <PlanPanel plan={planArtifact} />
                ) : verificationArtifact ? (
                  <VerificationPanel verification={verificationArtifact} />
                ) : fileDiff ? (
                  <FileDiffPreview diff={fileDiff} compact />
                ) : diffStats ? (
                  <div className="space-y-1.5">
                    <DiffStatsSummaryPanel stats={diffStats} />
                    {content && (
                      <pre className={`text-[11px] whitespace-pre-wrap break-words max-h-32 overflow-y-auto ${isError ? 'text-danger' : 'text-text-tertiary'}`}>
                        {content}
                      </pre>
                    )}
                  </div>
                ) : content ? (
                  <pre className={`text-[11px] whitespace-pre-wrap break-words max-h-32 overflow-y-auto ${isError ? 'text-danger' : 'text-text-tertiary'}`}>
                    {content}
                  </pre>
                ) : null}
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    );
  }

  if (isImageRender && (generatedImage || showImagePendingPreview)) {
    const imageMeta = [
      generatedImage?.provider,
      generatedImage?.model ?? imageArgs.model,
      imageArgs.size,
      imageArgs.quality,
      imageArgs.outputFormat,
    ].filter(Boolean);

    return (
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
        className="chat-image-panel my-2 overflow-hidden rounded-md border border-border/55 bg-surface-0/55"
        data-state={generatedImage ? 'ready' : 'loading'}
      >
        <div className="flex min-h-11 items-center gap-2 border-b border-border/40 px-3 py-2">
          <Icon className="h-4 w-4 shrink-0 text-accent" />
          <div className="min-w-0 flex-1">
            <div className="truncate text-xs font-medium text-text-primary">{briefLabel}</div>
            <div className="mt-0.5 flex min-w-0 flex-wrap gap-1.5 text-[11px] text-text-tertiary">
              {imageMeta.slice(0, 4).map((item) => (
                <span
                  key={String(item)}
                  className="rounded-md border border-border/45 bg-surface-0/45 px-1.5 py-0.5"
                >
                  {String(item)}
                </span>
              ))}
            </div>
          </div>
          <span className={`inline-flex shrink-0 items-center gap-1 text-[11px] ${statusConfig.color}`}>
            <StatusIcon className={`h-3.5 w-3.5 ${statusConfig.spin ? 'animate-spin' : ''}`} />
            <span>{headerSummary}</span>
          </span>
        </div>
        <div className="px-3 py-3">
          {generatedImage ? (
            <GeneratedImagePreview image={generatedImage} />
          ) : (
            <ImageGenerationPendingPreview
              prompt={imageArgs.prompt}
              size={imageArgs.size}
            />
          )}
        </div>
      </motion.div>
    );
  }

  if (subagentRun) {
    return (
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
        className="my-2"
      >
        <SubagentCard run={subagentRun} />
      </motion.div>
    );
  }

  if (subagentBatch) {
    return (
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
        className="my-2 rounded-lg border border-border/60 bg-surface-0/55 p-3"
      >
        <div className="mb-3 flex flex-wrap items-center gap-1.5 text-xs text-text-secondary">
          <span className="font-medium text-text-primary">
            {subagentBatch.batchGoal || t('chat.subagentParallelRun')}
          </span>
          {typeof subagentBatch.effectiveMaxParallel === 'number' && (
            <span className="rounded-full border border-border/55 bg-surface-1/70 px-2 py-0.5">
              {t('chat.subagentParallelCount', { count: String(subagentBatch.effectiveMaxParallel) })}
            </span>
          )}
          {subagentBatch.workflowTemplateLabel && (
            <span
              className="rounded-full border border-border/55 bg-surface-1/70 px-2 py-0.5"
              title={subagentBatch.workflowTemplateDescription ?? undefined}
            >
              {subagentBatch.workflowTemplateLabel}
            </span>
          )}
          {typeof subagentBatch.completedRuns === 'number' && (
            <span className="rounded-full border border-border/55 bg-surface-1/70 px-2 py-0.5">
              {t('chat.subagentCompletedCount', { count: String(subagentBatch.completedRuns) })}
            </span>
          )}
          {typeof subagentBatch.failedRuns === 'number' && subagentBatch.failedRuns > 0 && (
            <span className="rounded-full border border-danger/25 bg-danger/10 px-2 py-0.5 text-danger">
              {t('chat.subagentFailedCount', { count: String(subagentBatch.failedRuns) })}
            </span>
          )}
        </div>
        <div className="space-y-2">
          {subagentBatch.runs.map(run => (
            <SubagentCard key={run.id} run={run} compact />
          ))}
        </div>
      </motion.div>
    );
  }

  if (subagentJudgement) {
    return (
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
        className="my-2 rounded-lg border border-border/60 bg-surface-0/55 p-3"
      >
        <div className="mb-2 flex flex-wrap items-center gap-1.5 text-xs text-text-secondary">
          <span className="font-medium text-text-primary">
            {subagentJudgement.task || t('chat.subagentJudgementFallback')}
          </span>
          <span className="rounded-full border border-border/55 bg-surface-1/70 px-2 py-0.5">
            {subagentJudgement.decisionMode}
          </span>
          {subagentJudgement.confidence && (
            <span className="rounded-full border border-border/55 bg-surface-1/70 px-2 py-0.5">
              {t('chat.subagentConfidence', { value: subagentJudgement.confidence })}
            </span>
          )}
          {subagentJudgement.winnerIds.length > 0 && (
            <span className="rounded-full border border-accent/25 bg-accent/10 px-2 py-0.5 text-accent">
              {t('chat.subagentWinners', { value: subagentJudgement.winnerIds.join(', ') })}
            </span>
          )}
        </div>
        <div className="border-l border-border/50 pl-3 text-sm text-text-primary">
          {subagentJudgement.summary}
        </div>
        {subagentJudgement.rationale && (
          <div className="mt-2 border-l border-border/35 pl-3 text-xs text-text-secondary">
            {subagentJudgement.rationale}
          </div>
        )}
        {subagentJudgement.rubric && subagentJudgement.rubric.length > 0 && (
          <div className="mt-3">
            <div className="mb-1 text-[11px] uppercase tracking-[0.14em] text-text-tertiary">
              {t('chat.subagentRubric')}
            </div>
            <div className="flex flex-wrap gap-1.5">
              {subagentJudgement.rubric.map((item, index) => (
                <span
                  key={`judge-rubric-${index}`}
                  className="inline-flex items-center rounded-md border border-border/60 bg-surface-0 px-2 py-1 text-[11px] text-text-secondary"
                >
                  {item}
                </span>
              ))}
            </div>
          </div>
        )}
      </motion.div>
    );
  }

  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
      className="chat-trace-panel my-1 overflow-hidden rounded-md border border-border/50 bg-surface-0/45"
      data-trace-soft={traceSoft ? 'true' : 'false'}
      data-trace-active={traceActive ? 'true' : 'false'}
    >
      {/* Header */}
      <button
        onClick={() => expandableDetails && setExpanded((p) => !p)}
        aria-expanded={expandableDetails ? expanded : undefined}
        aria-label={expandableDetails ? (expanded ? t('common.collapse') : t('common.expand')) : briefLabel}
        disabled={!expandableDetails}
        className="flex items-center gap-2 w-full px-3 py-2 text-left hover:bg-surface-2
          transition-colors duration-fast ease-out cursor-pointer disabled:cursor-default disabled:hover:bg-transparent"
      >
        <Icon className="h-4 w-4 shrink-0 text-text-tertiary" />
        <span className="text-xs font-medium text-text-primary truncate">{briefLabel}</span>
        <span className="text-[11px] text-text-tertiary truncate flex-1">
          {headerSummary}
        </span>
        {diffStats ? (
          <DiffStatsTicker stats={diffStats} />
        ) : showPendingDiffStats ? (
          <PendingDiffTicker />
        ) : null}
        <StatusIcon
          className={`h-3.5 w-3.5 shrink-0 ${statusConfig.color} ${statusConfig.spin ? 'animate-spin' : ''}`}
        />
        {expandableDetails ? (
          expanded ? (
            <ChevronUp className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
          ) : (
            <ChevronDown className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
          )
        ) : null}
      </button>

      {/* Expandable result */}
      <AnimatePresence>
        {expanded && expandableDetails && (
          <motion.div
            {...getSoftCollapseMotion(!!shouldReduceMotion)}
            className="overflow-hidden"
          >
            <div className="border-t border-border px-3 py-2">
              {streamingArgsPreview && (
                <pre
                  className="mb-2 whitespace-pre-wrap break-words rounded-md bg-surface-0/60 px-2 py-1 text-[11px] text-text-tertiary max-h-48 overflow-y-auto"
                >
                  {streamingArgsPreview}
                </pre>
              )}
              {formattedArgs && !streamingArgsPreview && (
                <div className="mb-2 rounded-md bg-surface-0/60 px-2 py-1 text-[11px] text-text-tertiary break-words">
                  {formattedArgs}
                </div>
              )}
              {generatedImage ? (
                <GeneratedImagePreview image={generatedImage} />
              ) : showImagePendingPreview ? (
                <ImageGenerationPendingPreview
                  prompt={imageArgs.prompt}
                  size={imageArgs.size}
                />
              ) : planArtifact ? (
                <PlanPanel plan={planArtifact} />
              ) : verificationArtifact ? (
                <VerificationPanel verification={verificationArtifact} />
              ) : fileDiff ? (
                <FileDiffPreview diff={fileDiff} />
              ) : diffStats ? (
                <div className="space-y-2">
                  <DiffStatsSummaryPanel stats={diffStats} />
                  {content && (
                    <pre
                      className={`text-xs whitespace-pre-wrap break-words max-h-48 overflow-y-auto
                        ${isError ? 'text-danger' : 'text-text-secondary'}`}
                    >
                      {content}
                    </pre>
                  )}
                </div>
              ) : searchItems ? (
                <>
                  {trustBoundary && <TrustBoundaryPills boundary={trustBoundary} />}
                  <SearchResultCards items={searchItems} />
                </>
              ) : content ? (
                <pre
                  className={`text-xs whitespace-pre-wrap break-words max-h-48 overflow-y-auto
                    ${isError ? 'text-danger' : 'text-text-secondary'}`}
                >
                  {content}
                </pre>
              ) : null}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </motion.div>
  );
}
