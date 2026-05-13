import { useState, useEffect, useMemo, useRef } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { motion, AnimatePresence, useReducedMotion } from 'framer-motion';
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
} from 'lucide-react';
import { useTranslation } from '../../i18n';
import { FileBadge } from '../ui/FileBadge';
import { getSoftCollapseMotion } from '../../lib/uiMotion';
import { extractPlanArtifact, extractVerificationArtifact } from '../../lib/taskArtifacts';
import {
  extractSubagentArtifact,
  extractSubagentBatchArtifact,
  extractSubagentJudgementArtifact,
  parseSubagentArguments,
} from '../../lib/subagentArtifacts';
import { PlanPanel, VerificationPanel } from './TaskPanels';
import type { ArtifactPayload, ToolRenderKind, ToolRunCapabilities } from '../../types/conversation';
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
  dataUrl?: string;
  mediaType?: string;
  provider?: string;
  model?: string;
  prompt?: string;
  bytes?: number;
}

interface WorkPlanTarget {
  kind?: string;
  value?: string;
}

interface WorkPlanStep {
  id?: string;
  stage?: string;
  status?: string;
}

interface WorkPlanArtifact {
  version?: number;
  toolName?: string;
  risk?: string;
  requiresReview?: boolean;
  targets?: WorkPlanTarget[];
  steps?: WorkPlanStep[];
}

interface ContextManifestItem {
  id?: string;
  role?: string;
  source?: string;
  trustLevel?: string;
  tokenEstimate?: number;
}

interface ContextManifestArtifact {
  version?: number;
  tokenBudget?: number | null;
  totalTokenEstimate?: number;
  items?: ContextManifestItem[];
}

type ToolCallCardStatus =
  | 'preparing'
  | 'starting'
  | 'approvalPending'
  | 'running'
  | 'done'
  | 'error'
  | 'declined'
  | 'cancelled'
  | 'timedOut';

interface ToolCallCardProps {
  toolName?: string;
  arguments?: string;
  status: ToolCallCardStatus;
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
  argsStatus?: 'pending' | 'streaming' | 'ready' | 'done' | 'error';
  /** Total characters of `arguments` received so far. */
  argsBytes?: number;
  /** Accumulated heartbeat notes during tool execution. */
  progressNotes?: string[];
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
  search: Search,
  grep_files: Search,
  glob_files: FolderOpen,
  playbook: BookOpen,
  multi_edit: PenLine,
  edit_file: PenLine,
  file: FileText,
  summarize: List,
  list_dir: FolderOpen,
  fetch_url: Globe,
  chunk_context: Layers,
  write_note: PenLine,
  update_plan: ClipboardList,
  record_verification: ShieldCheck,
  run_shell: Terminal,
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
  status: string,
  t: ReturnType<typeof useTranslation>['t'],
  content?: string,
  toolName?: string,
): string {
  if (
    status === 'running'
    || status === 'starting'
    || status === 'preparing'
    || status === 'approvalPending'
  ) return '\u2026';
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

function extractWorkPlanArtifact(
  artifacts: ArtifactPayload | undefined,
): WorkPlanArtifact | null {
  if (!isRecord(artifacts)) return null;
  const workPlan = artifacts.workPlan;
  if (!isRecord(workPlan)) return null;

  const targets = Array.isArray(workPlan.targets)
    ? workPlan.targets.filter(isRecord).map((target) => ({
      kind: typeof target.kind === 'string' ? target.kind : undefined,
      value: typeof target.value === 'string' ? target.value : undefined,
    }))
    : undefined;
  const steps = Array.isArray(workPlan.steps)
    ? workPlan.steps.filter(isRecord).map((step) => ({
      id: typeof step.id === 'string' ? step.id : undefined,
      stage: typeof step.stage === 'string' ? step.stage : undefined,
      status: typeof step.status === 'string' ? step.status : undefined,
    }))
    : undefined;

  return {
    version: typeof workPlan.version === 'number' ? workPlan.version : undefined,
    toolName: typeof workPlan.toolName === 'string' ? workPlan.toolName : undefined,
    risk: typeof workPlan.risk === 'string' ? workPlan.risk : undefined,
    requiresReview: typeof workPlan.requiresReview === 'boolean' ? workPlan.requiresReview : undefined,
    targets,
    steps,
  };
}

function extractContextManifestArtifact(
  artifacts: ArtifactPayload | undefined,
): ContextManifestArtifact | null {
  if (!isRecord(artifacts)) return null;
  const contextManifest = artifacts.contextManifest;
  if (!isRecord(contextManifest)) return null;

  const items = Array.isArray(contextManifest.items)
    ? contextManifest.items.filter(isRecord).map((item) => ({
      id: typeof item.id === 'string' ? item.id : undefined,
      role: typeof item.role === 'string' ? item.role : undefined,
      source: typeof item.source === 'string' ? item.source : undefined,
      trustLevel: typeof item.trustLevel === 'string' ? item.trustLevel : undefined,
      tokenEstimate: typeof item.tokenEstimate === 'number' ? item.tokenEstimate : undefined,
    }))
    : undefined;

  return {
    version: typeof contextManifest.version === 'number' ? contextManifest.version : undefined,
    tokenBudget:
      typeof contextManifest.tokenBudget === 'number' || contextManifest.tokenBudget === null
        ? contextManifest.tokenBudget
        : undefined,
    totalTokenEstimate:
      typeof contextManifest.totalTokenEstimate === 'number'
        ? contextManifest.totalTokenEstimate
        : undefined,
    items,
  };
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

function workPlanStageLabel(stage: string | undefined, t: ReturnType<typeof useTranslation>['t']) {
  switch (stage) {
    case 'planner':
      return t('chat.workPlanStagePlanner');
    case 'executor':
      return t('chat.workPlanStageExecutor');
    case 'reviewer':
      return t('chat.workPlanStageReviewer');
    default:
      return stage || t('chat.workPlanStagePlanner');
  }
}

function workPlanRiskLabel(risk: string | undefined, t: ReturnType<typeof useTranslation>['t']) {
  switch (risk) {
    case 'low':
      return t('chat.workPlanRiskLow');
    case 'medium':
      return t('chat.workPlanRiskMedium');
    case 'high':
      return t('chat.workPlanRiskHigh');
    default:
      return risk || t('chat.workPlanRiskLow');
  }
}

function workPlanStepStatusLabel(status: string | undefined, t: ReturnType<typeof useTranslation>['t']) {
  switch (status) {
    case 'done':
      return t('chat.taskRunCompleted');
    case 'running':
      return t('chat.taskRunRunning');
    case 'failed':
      return t('chat.taskRunFailed');
    case 'skipped':
      return t('chat.taskRunUnknown');
    default:
      return t('chat.taskRunQueued');
  }
}

function contextRoleLabel(role: string | undefined, t: ReturnType<typeof useTranslation>['t']) {
  switch (role) {
    case 'instruction':
      return t('chat.contextManifestRoleInstruction');
    case 'evidence':
      return t('chat.contextManifestRoleEvidence');
    case 'tool_guidance':
      return t('chat.contextManifestRoleToolGuidance');
    case 'memory':
      return t('chat.contextManifestRoleMemory');
    case 'conversation':
      return t('chat.contextManifestRoleConversation');
    case 'source_scope':
      return t('chat.contextManifestRoleSourceScope');
    default:
      return role || t('chat.contextManifestRoleEvidence');
  }
}

function contextTrustLabel(trustLevel: string | undefined, t: ReturnType<typeof useTranslation>['t']) {
  switch (trustLevel) {
    case 'system':
      return t('chat.contextManifestTrustSystem');
    case 'user_selected':
      return t('chat.contextManifestTrustUserSelected');
    case 'retrieved_evidence':
      return t('chat.contextManifestTrustRetrievedEvidence');
    case 'agent_memory':
      return t('chat.contextManifestTrustAgentMemory');
    case 'external':
      return t('chat.contextManifestTrustExternal');
    default:
      return trustLevel || t('chat.contextManifestTrustRetrievedEvidence');
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
    status === 'starting'
    || status === 'preparing'
    || status === 'approvalPending'
    || status === 'running'
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

function WorkPlanPanel({ plan, compact = false }: { plan: WorkPlanArtifact; compact?: boolean }) {
  const { t } = useTranslation();
  const targets = plan.targets ?? [];
  const steps = plan.steps ?? [];
  const visibleTargets = targets.slice(0, compact ? 2 : 4);
  const visibleSteps = steps.slice(0, compact ? 2 : 3);

  return (
    <div className="rounded-lg border border-border/60 bg-surface-0/65 px-3 py-2">
      <div className="flex flex-wrap items-center gap-2 text-xs">
        <span className="font-medium text-text-primary">{t('chat.workPlanLabel')}</span>
        <span className="rounded-full border border-border/60 bg-surface-1 px-2 py-0.5 text-[11px] text-text-secondary">
          {workPlanRiskLabel(plan.risk, t)}
        </span>
        <span className="rounded-full border border-border/60 bg-surface-1 px-2 py-0.5 text-[11px] text-text-secondary">
          {plan.requiresReview ? t('chat.workPlanReviewRequired') : t('chat.workPlanReviewOptional')}
        </span>
        {targets.length > 0 && (
          <span className="text-[11px] text-text-tertiary">
            {t('chat.workPlanTargetCount', { count: String(targets.length) })}
          </span>
        )}
      </div>

      {visibleTargets.length > 0 && (
        <div className="mt-2 flex flex-wrap gap-1.5">
          {visibleTargets.map((target, index) => (
            <span
              key={`${target.kind ?? 'target'}-${target.value ?? index}`}
              className="inline-flex max-w-full items-center gap-1 rounded-md border border-border/60 bg-surface-1 px-2 py-1 text-[11px] text-text-secondary"
            >
              {target.kind && <span className="text-text-tertiary">{target.kind}</span>}
              {target.value && <span className="truncate">{target.value}</span>}
            </span>
          ))}
        </div>
      )}

      {visibleSteps.length > 0 && (
        <div className="mt-2 grid gap-1.5">
          {visibleSteps.map((step, index) => (
            <div
              key={step.id ?? `${step.stage ?? 'step'}-${index}`}
              className="flex items-center justify-between gap-2 rounded-md border border-border/50 bg-surface-1/70 px-2 py-1"
            >
              <span className="text-[11px] font-medium text-text-secondary">
                {workPlanStageLabel(step.stage, t)}
              </span>
              <span className="text-[11px] text-text-tertiary">
                {workPlanStepStatusLabel(step.status, t)}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function ContextManifestPanel({ manifest, compact = false }: { manifest: ContextManifestArtifact; compact?: boolean }) {
  const { t } = useTranslation();
  const items = manifest.items ?? [];
  const visibleItems = items.slice(0, compact ? 2 : 4);
  const tokenEstimate = manifest.totalTokenEstimate ?? items.reduce((sum, item) => sum + (item.tokenEstimate ?? 0), 0);

  return (
    <div className="rounded-lg border border-border/60 bg-surface-0/65 px-3 py-2">
      <div className="flex flex-wrap items-center gap-2 text-xs">
        <span className="font-medium text-text-primary">{t('chat.contextManifestLabel')}</span>
        <span className="text-[11px] text-text-tertiary">
          {t('chat.contextManifestSummary', {
            count: String(items.length),
            tokens: String(tokenEstimate),
          })}
        </span>
      </div>

      {visibleItems.length > 0 && (
        <div className="mt-2 grid gap-1.5">
          {visibleItems.map((item, index) => (
            <div
              key={item.id ?? `${item.source ?? 'context'}-${index}`}
              className="flex flex-wrap items-center gap-1.5 rounded-md border border-border/50 bg-surface-1/70 px-2 py-1 text-[11px]"
            >
              <span className="font-medium text-text-secondary">{contextRoleLabel(item.role, t)}</span>
              <span className="text-text-tertiary">{contextTrustLabel(item.trustLevel, t)}</span>
              {item.source && <span className="min-w-0 truncate text-text-tertiary">{item.source}</span>}
            </div>
          ))}
        </div>
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
  progressNotes,
}: ToolCallCardProps) {
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();
  const safeToolName =
    typeof toolName === 'string' && toolName.trim().length > 0
      ? toolName
      : 'unknown_tool';
  const Icon = getToolIcon(safeToolName);
  const formattedArgs = formatArgs(args);
  const isPending =
    status === 'running'
    || status === 'starting'
    || status === 'preparing'
    || status === 'approvalPending';
  const argsByteLabel = formatByteCount(
    typeof argsBytes === 'number' ? argsBytes : (args ? args.length : 0),
  );
  const durationLabel = formatDurationMs(durationMs);
  const latestProgressNote =
    progressNotes && progressNotes.length > 0 ? progressNotes[progressNotes.length - 1] : null;
  const resourceKeyCount = Array.isArray(capabilities?.resourceKeys)
    ? capabilities.resourceKeys.length
    : 0;
  const capabilitySummary = capabilities
    ? [
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
  const workPlanArtifact = useMemo(() => extractWorkPlanArtifact(artifacts), [artifacts]);
  const contextManifest = useMemo(() => extractContextManifestArtifact(artifacts), [artifacts]);
  const showPendingDiffStats = isPending && !diffStats && isFileChangeToolRender(safeToolName, renderKind);
  const isStructuredTaskCard = Boolean(
    planArtifact ||
    verificationArtifact ||
    fileDiff ||
    diffStats ||
    generatedImage ||
    workPlanArtifact ||
    contextManifest,
  );
  const shouldAutoOpenStructuredTaskCard = Boolean(planArtifact || verificationArtifact || generatedImage || workPlanArtifact);

  const isSearchDone =
    safeToolName.toLowerCase().includes('search') && status === 'done' && !!content;
  const searchItems = useMemo(
    () => (isSearchDone ? parseSearchResults(content!) : null),
    [isSearchDone, content],
  );

  const [expanded, setExpanded] = useState(shouldAutoOpenStructuredTaskCard);

  // Auto-collapse file mutation details when execution finishes; users can manually re-open.
  useEffect(() => {
    if (!isPending && !shouldAutoOpenStructuredTaskCard) {
      setExpanded(false);
    }
  }, [isPending, shouldAutoOpenStructuredTaskCard]);

  useEffect(() => {
    if (shouldAutoOpenStructuredTaskCard) {
      setExpanded(true);
    }
  }, [shouldAutoOpenStructuredTaskCard]);

  if (inline) {
    const briefLabel = getToolBriefLabel(safeToolName, args);
    const briefResult = getToolBriefResult(status, t, content, safeToolName);
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
      : workPlanArtifact
        ? t('chat.workPlanLabel')
      : searchItems
        ? t('search.results', { count: String(searchItems.length) })
        : contextManifest
          ? t('chat.contextManifestLabel')
        : diffStats
          ? `${diffStats.operation === 'create' ? t('chat.fileDiffCreated') : t('chat.fileDiffModified')}`
        : showPendingDiffStats
          ? t('chat.fileDiffModified')
        : status === 'done' && content
          ? t('chat.traceOutputReady')
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

  if (trace) {
    const canExpand = Boolean(
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
      workPlanArtifact ||
      contextManifest ||
      streamingArgsPreview,
    );
    return (
      <div className="rounded-lg border border-border/45 bg-surface-0/35">
        <button
          type="button"
          onClick={() => canExpand && setExpanded((prev) => !prev)}
          className="flex w-full items-center gap-2 px-3 py-2 text-left transition-colors hover:bg-surface-0/45 cursor-pointer disabled:cursor-default"
          disabled={!canExpand}
          title={capabilitySummary ?? undefined}
        >
          <Icon className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
          <span className="min-w-0 flex-1">
            <span className="block truncate text-[12px] font-medium text-text-primary">{safeToolName}</span>
            {isPending && latestProgressNote ? (
              <span className="block truncate text-[11px] text-text-tertiary/80 italic">{latestProgressNote}</span>
            ) : formattedArgs ? (
              <span className="block truncate text-[11px] text-text-tertiary">{formattedArgs}</span>
            ) : null}
          </span>
          <span className={`inline-flex items-center gap-1 text-[11px] ${statusConfig.color}`}>
            <StatusIcon className={`h-3.5 w-3.5 shrink-0 ${statusConfig.spin ? 'animate-spin' : ''}`} />
            <span>{headerSummary}</span>
          </span>
          {diffStats ? (
            <DiffStatsTicker stats={diffStats} compact />
          ) : showPendingDiffStats ? (
            <PendingDiffTicker compact />
          ) : null}
          {canExpand && (
            expanded
              ? <ChevronUp className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
              : <ChevronDown className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
          )}
        </button>

        {expanded && canExpand && (
          <div className="border-t border-border/35 px-3 py-2 space-y-2">
            {streamingArgsPreview && (
              <pre className="whitespace-pre-wrap break-words text-[11px] leading-relaxed text-text-tertiary bg-surface-0/40 rounded-md px-2 py-1">
                {streamingArgsPreview}
              </pre>
            )}
            {workPlanArtifact && <WorkPlanPanel plan={workPlanArtifact} compact />}
            {contextManifest && <ContextManifestPanel manifest={contextManifest} compact />}
            {generatedImage ? (
              <div className="space-y-2">
                <div className="overflow-hidden rounded-md border border-border/60 bg-surface-0">
                  <img
                    src={generatedImage.dataUrl || (generatedImage.path ? convertFileSrc(generatedImage.path) : '')}
                    alt={generatedImage.prompt || t('chat.generatedImageAlt')}
                    className="max-h-64 w-full object-contain"
                  />
                </div>
                {generatedImage.path && (
                  <div className="break-all text-[11px] text-text-secondary">{generatedImage.path}</div>
                )}
              </div>
            ) : subagentRun ? (
              <SubagentCard run={subagentRun} defaultOpen />
            ) : subagentBatch ? (
              <div className="space-y-2">
                {subagentBatch.runs.map((run) => (
                  <SubagentCard key={run.id} run={run} compact defaultOpen />
                ))}
              </div>
            ) : subagentJudgement ? (
              <div className="rounded-lg border border-border/60 bg-surface-0/70 px-3 py-2">
                <div className="mb-1 flex flex-wrap items-center gap-2 text-xs text-text-secondary">
                  <span className="font-medium text-text-primary">
                    {subagentJudgement.task || t('chat.subagentJudgementFallback')}
                  </span>
                  <span className="rounded-full border border-border/60 bg-surface-1 px-2 py-0.5">
                    {subagentJudgement.decisionMode}
                  </span>
                  {subagentJudgement.confidence && (
                    <span className="rounded-full border border-border/60 bg-surface-1 px-2 py-0.5">
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
        )}
      </div>
    );
  }

  if (compact) {
    return (
      <div className="rounded border border-border/40 overflow-hidden">
        <button
          onClick={() => setExpanded((p) => !p)}
          className="flex items-center gap-1.5 w-full px-2 py-1 text-left hover:bg-surface-2/50 transition-colors cursor-pointer"
        >
          <Icon className="h-3 w-3 shrink-0 text-text-tertiary" />
          <span className="text-[11px] font-medium text-text-secondary truncate">{safeToolName}</span>
          <span className="text-[10px] text-text-tertiary truncate flex-1">{headerSummary}</span>
          {diffStats ? (
            <DiffStatsTicker stats={diffStats} compact />
          ) : showPendingDiffStats ? (
            <PendingDiffTicker compact />
          ) : null}
          <StatusIcon
            className={`h-3 w-3 shrink-0 ${statusConfig.color} ${statusConfig.spin ? 'animate-spin' : ''}`}
          />
        </button>
        {expanded && (content || fileDiff || diffStats || generatedImage || workPlanArtifact || contextManifest) && (
          <div className="border-t border-border/30 px-2 py-1.5">
            {formattedArgs && (
              <div className="mb-1 rounded bg-surface-0/60 px-1.5 py-0.5 text-[10px] text-text-tertiary break-words">
                {formattedArgs}
              </div>
            )}
            {workPlanArtifact && <WorkPlanPanel plan={workPlanArtifact} compact />}
            {contextManifest && <ContextManifestPanel manifest={contextManifest} compact />}
            {generatedImage ? (
              <div className="overflow-hidden rounded-md border border-border/60 bg-surface-0">
                <img
                  src={generatedImage.dataUrl || (generatedImage.path ? convertFileSrc(generatedImage.path) : '')}
                  alt={generatedImage.prompt || t('chat.generatedImageAlt')}
                  className="max-h-32 w-full object-contain"
                />
              </div>
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
        )}
      </div>
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
        className="my-2 rounded-xl border border-border/70 bg-surface-1/80 p-3"
      >
        <div className="mb-3 flex flex-wrap items-center gap-2 text-xs text-text-secondary">
          <span className="font-medium text-text-primary">
            {subagentBatch.batchGoal || t('chat.subagentParallelRun')}
          </span>
          {typeof subagentBatch.effectiveMaxParallel === 'number' && (
            <span className="rounded-full border border-border/60 bg-surface-0 px-2 py-1">
              {t('chat.subagentParallelCount', { count: String(subagentBatch.effectiveMaxParallel) })}
            </span>
          )}
          {subagentBatch.workflowTemplateLabel && (
            <span
              className="rounded-full border border-border/60 bg-surface-0 px-2 py-1"
              title={subagentBatch.workflowTemplateDescription ?? undefined}
            >
              {subagentBatch.workflowTemplateLabel}
            </span>
          )}
          {typeof subagentBatch.completedRuns === 'number' && (
            <span className="rounded-full border border-border/60 bg-surface-0 px-2 py-1">
              {t('chat.subagentCompletedCount', { count: String(subagentBatch.completedRuns) })}
            </span>
          )}
          {typeof subagentBatch.failedRuns === 'number' && subagentBatch.failedRuns > 0 && (
            <span className="rounded-full border border-danger/25 bg-danger/10 px-2 py-1 text-danger">
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
        className="my-2 rounded-xl border border-border/70 bg-surface-1/80 p-3"
      >
        <div className="mb-2 flex flex-wrap items-center gap-2 text-xs text-text-secondary">
          <span className="font-medium text-text-primary">
            {subagentJudgement.task || t('chat.subagentJudgementFallback')}
          </span>
          <span className="rounded-full border border-border/60 bg-surface-0 px-2 py-1">
            {subagentJudgement.decisionMode}
          </span>
          {subagentJudgement.confidence && (
            <span className="rounded-full border border-border/60 bg-surface-0 px-2 py-1">
              {t('chat.subagentConfidence', { value: subagentJudgement.confidence })}
            </span>
          )}
          {subagentJudgement.winnerIds.length > 0 && (
            <span className="rounded-full border border-accent/25 bg-accent/10 px-2 py-1 text-accent">
              {t('chat.subagentWinners', { value: subagentJudgement.winnerIds.join(', ') })}
            </span>
          )}
        </div>
        <div className="rounded-lg border border-border/60 bg-surface-0/70 px-3 py-2 text-sm text-text-primary">
          {subagentJudgement.summary}
        </div>
        {subagentJudgement.rationale && (
          <div className="mt-2 rounded-lg border border-border/60 bg-surface-0/55 px-3 py-2 text-xs text-text-secondary">
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
      className="chat-trace-panel bg-surface-1 border border-border rounded-lg overflow-hidden my-2"
      data-trace-soft={traceSoft ? 'true' : 'false'}
      data-trace-active={traceActive ? 'true' : 'false'}
    >
      {/* Header */}
      <button
        onClick={() => setExpanded((p) => !p)}
        aria-expanded={expanded}
        aria-label={expanded ? t('common.collapse') : t('common.expand')}
        className="flex items-center gap-2 w-full px-3 py-2 text-left hover:bg-surface-2
          transition-colors duration-fast ease-out cursor-pointer"
      >
        <Icon className="h-4 w-4 shrink-0 text-text-tertiary" />
        <span className="text-xs font-medium text-text-primary truncate">{safeToolName}</span>
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
        {(content || streamingArgsPreview || fileDiff || diffStats || generatedImage || workPlanArtifact || contextManifest) ? (
          expanded ? (
            <ChevronUp className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
          ) : (
            <ChevronDown className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
          )
        ) : null}
      </button>

      {isPending && latestProgressNote && (
        <div className="border-t border-border/30 bg-surface-0/30 px-3 py-1 text-[11px] italic text-text-tertiary truncate">
          {latestProgressNote}
        </div>
      )}

      {/* Expandable result */}
      <AnimatePresence>
        {expanded && (content || streamingArgsPreview || fileDiff || diffStats || generatedImage || workPlanArtifact || contextManifest) && (
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
              {workPlanArtifact && (
                <div className="mb-2">
                  <WorkPlanPanel plan={workPlanArtifact} />
                </div>
              )}
              {contextManifest && (
                <div className="mb-2">
                  <ContextManifestPanel manifest={contextManifest} />
                </div>
              )}
              {generatedImage ? (
                <div className="space-y-2">
                  <div className="overflow-hidden rounded-md border border-border/60 bg-surface-0">
                    <img
                      src={generatedImage.dataUrl || (generatedImage.path ? convertFileSrc(generatedImage.path) : '')}
                      alt={generatedImage.prompt || t('chat.generatedImageAlt')}
                      className="max-h-80 w-full object-contain"
                    />
                  </div>
                  <div className="grid gap-1 text-[11px] text-text-tertiary">
                    <div className="flex flex-wrap gap-1.5">
                      {generatedImage.provider && (
                        <span className="rounded-md border border-border/50 bg-surface-0/50 px-1.5 py-0.5">
                          {generatedImage.provider}
                        </span>
                      )}
                      {generatedImage.model && (
                        <span className="rounded-md border border-border/50 bg-surface-0/50 px-1.5 py-0.5">
                          {generatedImage.model}
                        </span>
                      )}
                      {typeof generatedImage.bytes === 'number' && (
                        <span className="rounded-md border border-border/50 bg-surface-0/50 px-1.5 py-0.5">
                          {formatByteCount(generatedImage.bytes)}
                        </span>
                      )}
                    </div>
                    {generatedImage.path && (
                      <div className="break-all text-text-secondary">{generatedImage.path}</div>
                    )}
                  </div>
                </div>
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
              {artifacts && !isStructuredTaskCard && !fileDiff && (
                <div className="mt-2 text-[11px] text-text-tertiary">
                  {JSON.stringify(artifacts, null, 2).slice(0, 500)}
                </div>
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </motion.div>
  );
}
