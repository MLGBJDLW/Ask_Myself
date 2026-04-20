import { useState, useEffect, useMemo } from 'react';
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
import { extractPlanArtifact, extractVerificationArtifact } from '../../lib/taskArtifacts';
import {
  extractSubagentArtifact,
  extractSubagentBatchArtifact,
  extractSubagentJudgementArtifact,
  parseSubagentArguments,
} from '../../lib/subagentArtifacts';
import { PlanPanel, VerificationPanel } from './TaskPanels';
import type { ArtifactPayload } from '../../types/conversation';
import type { VerificationOverallStatus } from '../../lib/taskArtifacts';
import { SubagentCard } from './SubagentCard';
import { PptDeckCard } from './PptDeckCard';
import type { DeckSpec } from '../../lib/ppt';

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

interface ToolCallCardProps {
  toolName?: string;
  arguments?: string;
  status: 'running' | 'done' | 'error';
  content?: string;
  isError?: boolean;
  artifacts?: ArtifactPayload;
  compact?: boolean;
  inline?: boolean;
  trace?: boolean;
}

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

const TOOL_ICONS: Record<string, typeof Search> = {
  search: Search,
  playbook: BookOpen,
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

function getToolBriefResult(status: string, content?: string, toolName?: string): string {
  if (status === 'running') return '\u2026';
  if (status === 'error') return 'error';
  const lower = (toolName || '').toLowerCase();
  if (lower.includes('search') && content) {
    const match = content.match(/Found (\d+) result/i);
    if (match) return `${match[1]} results`;
  }
  if (content) {
    const lines = content.split('\n').length;
    if (lines > 3) return `${lines} lines`;
  }
  return 'done';
}

function extractPptDeckArtifact(
  artifacts: ArtifactPayload | undefined,
): { path: string; spec: DeckSpec } | null {
  if (!artifacts || Array.isArray(artifacts)) return null;
  const raw = (artifacts as Record<string, unknown>).ppt_deck;
  if (!raw || typeof raw !== 'object') return null;
  const obj = raw as { path?: unknown; spec?: unknown };
  if (typeof obj.path !== 'string' || !obj.spec || typeof obj.spec !== 'object') return null;
  const spec = obj.spec as DeckSpec;
  if (!Array.isArray(spec.slides)) return null;
  return { path: obj.path, spec };
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

function buildSubagentRun(
  toolName: string,
  args: string | undefined,
  status: 'running' | 'done' | 'error',
  content: string | undefined,
  isError: boolean | undefined,
  artifacts: ArtifactPayload | undefined,
) {
  if (toolName !== 'spawn_subagent') return null;
  const artifact = extractSubagentArtifact(artifacts);
  const parsedArgs = parseSubagentArguments(args);
  const task = artifact?.task ?? parsedArgs?.task;
  if (!task) return null;
  return {
    id: `${toolName}-${task}`,
    status,
    task,
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

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function ToolCallCard({
  toolName,
  arguments: args,
  status,
  content,
  isError,
  artifacts,
  compact,
  inline,
  trace,
}: ToolCallCardProps) {
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();
  const safeToolName =
    typeof toolName === 'string' && toolName.trim().length > 0
      ? toolName
      : 'unknown_tool';
  const Icon = getToolIcon(safeToolName);
  const formattedArgs = formatArgs(args);
  const subagentRun = useMemo(
    () => buildSubagentRun(safeToolName, args, status, content, isError, artifacts),
    [safeToolName, args, status, content, isError, artifacts],
  );
  const subagentBatch = useMemo(() => extractSubagentBatchArtifact(artifacts), [artifacts]);
  const subagentJudgement = useMemo(() => extractSubagentJudgementArtifact(artifacts), [artifacts]);
  const planArtifact = useMemo(() => extractPlanArtifact(artifacts), [artifacts]);
  const verificationArtifact = useMemo(() => extractVerificationArtifact(artifacts), [artifacts]);
  const pptDeckArtifact = useMemo(() => extractPptDeckArtifact(artifacts), [artifacts]);
  const isStructuredTaskCard = Boolean(planArtifact || verificationArtifact);

  const isSearchDone =
    safeToolName.toLowerCase().includes('search') && status === 'done' && !!content;
  const searchItems = useMemo(
    () => (isSearchDone ? parseSearchResults(content!) : null),
    [isSearchDone, content],
  );

  const [expanded, setExpanded] = useState(isStructuredTaskCard);

  // Auto-collapse when execution finishes; users can manually re-open if needed.
  useEffect(() => {
    if (status !== 'running' && !isStructuredTaskCard) {
      setExpanded(false);
    }
  }, [status, isStructuredTaskCard]);

  useEffect(() => {
    if (isStructuredTaskCard) {
      setExpanded(true);
    }
  }, [isStructuredTaskCard]);

  if (inline) {
    const briefLabel = getToolBriefLabel(safeToolName, args);
    const briefResult = getToolBriefResult(status, content, safeToolName);
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
    running: { icon: Loader2, text: t('chat.toolRunning'), color: 'text-accent', spin: true },
    done: { icon: CheckCircle2, text: t('chat.toolDone'), color: 'text-success', spin: false },
    error: { icon: XCircle, text: t('chat.toolError'), color: 'text-danger', spin: false },
  }[status];
  const headerSummary = planArtifact
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
        : status === 'done' && content
          ? t('chat.traceOutputReady')
          : statusConfig.text;

  const StatusIcon = statusConfig.icon;
  const traceActive = status === 'running' && !shouldReduceMotion;
  const traceSoft = status !== 'error';

  if (trace) {
    const canExpand = Boolean(formattedArgs || content || searchItems || planArtifact || verificationArtifact);
    return (
      <div className="rounded-lg border border-border/45 bg-surface-0/35">
        <button
          type="button"
          onClick={() => canExpand && setExpanded((prev) => !prev)}
          className="flex w-full items-center gap-2 px-3 py-2 text-left transition-colors hover:bg-surface-0/45 cursor-pointer disabled:cursor-default"
          disabled={!canExpand}
        >
          <Icon className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
          <span className="min-w-0 flex-1">
            <span className="block truncate text-[12px] font-medium text-text-primary">{safeToolName}</span>
            {formattedArgs && (
              <span className="block truncate text-[11px] text-text-tertiary">{formattedArgs}</span>
            )}
          </span>
          <span className={`inline-flex items-center gap-1 text-[11px] ${statusConfig.color}`}>
            <StatusIcon className={`h-3.5 w-3.5 shrink-0 ${statusConfig.spin ? 'animate-spin' : ''}`} />
            <span>{headerSummary}</span>
          </span>
          {canExpand && (
            expanded
              ? <ChevronUp className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
              : <ChevronDown className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
          )}
        </button>

        {expanded && canExpand && (
          <div className="border-t border-border/35 px-3 py-2">
            {searchItems ? (
              <SearchResultCards items={searchItems} />
            ) : planArtifact ? (
              <PlanPanel plan={planArtifact} />
            ) : verificationArtifact ? (
              <VerificationPanel verification={verificationArtifact} />
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
          <StatusIcon
            className={`h-3 w-3 shrink-0 ${statusConfig.color} ${statusConfig.spin ? 'animate-spin' : ''}`}
          />
        </button>
        {expanded && content && (
          <div className="border-t border-border/30 px-2 py-1.5">
            {formattedArgs && (
              <div className="mb-1 rounded bg-surface-0/60 px-1.5 py-0.5 text-[10px] text-text-tertiary break-words">
                {formattedArgs}
              </div>
            )}
            <pre className={`text-[11px] whitespace-pre-wrap break-words max-h-32 overflow-y-auto ${isError ? 'text-danger' : 'text-text-tertiary'}`}>
              {content}
            </pre>
          </div>
        )}
      </div>
    );
  }

  if (pptDeckArtifact && !inline && !compact && !trace) {
    const artifactKey = `${pptDeckArtifact.path}::${pptDeckArtifact.spec.slides.length}::${pptDeckArtifact.spec.title}`;
    return (
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
        className="my-2"
      >
        <PptDeckCard
          artifactKey={artifactKey}
          path={pptDeckArtifact.path}
          spec={pptDeckArtifact.spec}
        />
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
        className="my-2 rounded-xl border border-border/70 bg-surface-1/80 p-3"
      >
        <div className="mb-3 flex flex-wrap items-center gap-2 text-xs text-text-secondary">
          <span className="font-medium text-text-primary">
            {subagentBatch.batchGoal || 'Parallel delegated run'}
          </span>
          {typeof subagentBatch.effectiveMaxParallel === 'number' && (
            <span className="rounded-full border border-border/60 bg-surface-0 px-2 py-1">
              parallel {subagentBatch.effectiveMaxParallel}
            </span>
          )}
          {typeof subagentBatch.completedRuns === 'number' && (
            <span className="rounded-full border border-border/60 bg-surface-0 px-2 py-1">
              complete {subagentBatch.completedRuns}
            </span>
          )}
          {typeof subagentBatch.failedRuns === 'number' && subagentBatch.failedRuns > 0 && (
            <span className="rounded-full border border-danger/25 bg-danger/10 px-2 py-1 text-danger">
              failed {subagentBatch.failedRuns}
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
            {subagentJudgement.task || 'Delegated result adjudication'}
          </span>
          <span className="rounded-full border border-border/60 bg-surface-0 px-2 py-1">
            {subagentJudgement.decisionMode}
          </span>
          {subagentJudgement.confidence && (
            <span className="rounded-full border border-border/60 bg-surface-0 px-2 py-1">
              confidence {subagentJudgement.confidence}
            </span>
          )}
          {subagentJudgement.winnerIds.length > 0 && (
            <span className="rounded-full border border-accent/25 bg-accent/10 px-2 py-1 text-accent">
              winners {subagentJudgement.winnerIds.join(', ')}
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
              Rubric
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
        <StatusIcon
          className={`h-3.5 w-3.5 shrink-0 ${statusConfig.color} ${statusConfig.spin ? 'animate-spin' : ''}`}
        />
        {content ? (
          expanded ? (
            <ChevronUp className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
          ) : (
            <ChevronDown className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
          )
        ) : null}
      </button>

      {/* Expandable result */}
      <AnimatePresence>
        {expanded && content && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: 'auto', opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
            className="overflow-hidden"
          >
            <div className="border-t border-border px-3 py-2">
              {formattedArgs && (
                <div className="mb-2 rounded-md bg-surface-0/60 px-2 py-1 text-[11px] text-text-tertiary break-words">
                  {formattedArgs}
                </div>
              )}
              {planArtifact ? (
                <PlanPanel plan={planArtifact} />
              ) : verificationArtifact ? (
                <VerificationPanel verification={verificationArtifact} />
              ) : searchItems ? (
                <SearchResultCards items={searchItems} />
              ) : (
                <pre
                  className={`text-xs whitespace-pre-wrap break-words max-h-48 overflow-y-auto
                    ${isError ? 'text-danger' : 'text-text-secondary'}`}
                >
                  {content}
                </pre>
              )}
              {artifacts && !isStructuredTaskCard && (
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
