import { useState, useEffect, useMemo } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
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
} from 'lucide-react';
import { useTranslation } from '../../i18n';
import { FileBadge } from '../ui/FileBadge';
import { extractPlanArtifact, extractVerificationArtifact } from '../../lib/taskArtifacts';
import { PlanPanel, VerificationPanel } from './TaskPanels';
import type { ArtifactPayload } from '../../types/conversation';
import type { VerificationOverallStatus } from '../../lib/taskArtifacts';

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
}: ToolCallCardProps) {
  const { t } = useTranslation();
  const safeToolName =
    typeof toolName === 'string' && toolName.trim().length > 0
      ? toolName
      : 'unknown_tool';
  const Icon = getToolIcon(safeToolName);
  const formattedArgs = formatArgs(args);
  const planArtifact = useMemo(() => extractPlanArtifact(artifacts), [artifacts]);
  const verificationArtifact = useMemo(() => extractVerificationArtifact(artifacts), [artifacts]);
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

  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
      className="bg-surface-1 border border-border rounded-lg overflow-hidden my-2"
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
