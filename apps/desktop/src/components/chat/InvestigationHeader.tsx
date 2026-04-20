import {
  BookOpen,
  ChevronDown,
  Compass,
  Database,
  FolderOpen,
  ShieldCheck,
  Sparkles,
} from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from '../../i18n';
import type { Conversation } from '../../types/conversation';

interface SourceSelectionSummary {
  selectedCount: number;
  totalCount: number;
  loading: boolean;
}

type EvidenceLevel = 'high' | 'medium' | 'low' | 'none';

interface InvestigationHeaderProps {
  conversationTitle?: string | null;
  collectionContext?: Conversation['collectionContext'] | null;
  sourceSummary: SourceSelectionSummary;
  isStreaming?: boolean;
  routeKind?: string | null;
  turnStatus?: string | null;
  evidenceLevel: EvidenceLevel;
  evidenceCount: number;
}

function formatRouteKind(routeKind: string): string {
  return routeKind
    .replace(/([a-z])([A-Z])/g, '$1 $2')
    .replace(/^./, (char) => char.toUpperCase());
}

function formatTurnStatus(
  status: string | null | undefined,
  t: ReturnType<typeof useTranslation>['t'],
): string {
  switch (status) {
    case 'success':
      return t('chat.investigationStatusReady');
    case 'cached':
      return t('chat.cached');
    case 'error':
      return t('chat.investigationStatusNeedsAttention');
    case 'running':
      return t('chat.investigationStatusInvestigating');
    case 'max_iterations':
      return t('chat.investigationStatusIncomplete');
    case 'cancelled':
      return t('chat.investigationStatusStopped');
    default:
      return t('chat.investigationStatusIdle');
  }
}

function evidenceTone(level: EvidenceLevel) {
  switch (level) {
    case 'high':
      return 'border-emerald-500/20 bg-emerald-500/10 text-emerald-300';
    case 'medium':
      return 'border-cyan-500/20 bg-cyan-500/10 text-cyan-300';
    case 'low':
      return 'border-amber-500/20 bg-amber-500/10 text-amber-300';
    case 'none':
    default:
      return 'border-border/70 bg-surface-1/70 text-text-secondary';
  }
}

function evidenceLabel(
  level: EvidenceLevel,
  t: ReturnType<typeof useTranslation>['t'],
): string {
  switch (level) {
    case 'high':
      return t('chat.investigationEvidenceHigh');
    case 'medium':
      return t('chat.investigationEvidenceMedium');
    case 'low':
      return t('chat.investigationEvidenceLow');
    case 'none':
    default:
      return t('chat.investigationEvidenceNone');
  }
}

export function InvestigationHeader({
  conversationTitle,
  collectionContext,
  sourceSummary,
  isStreaming = false,
  routeKind,
  turnStatus,
  evidenceLevel,
  evidenceCount,
}: InvestigationHeaderProps) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);

  const scopeLabel = sourceSummary.loading
    ? t('common.loading')
    : sourceSummary.totalCount === 0 || sourceSummary.selectedCount === 0
      ? t('chat.allSources')
      : `${sourceSummary.selectedCount} / ${sourceSummary.totalCount}`;
  const scopeHint = sourceSummary.loading
    ? t('common.loading')
    : sourceSummary.totalCount === 0 || sourceSummary.selectedCount === 0
      ? t('chat.investigationScopeAllHint')
      : t('chat.investigationScopeSelectedHint');

  const statusLabel = isStreaming
    ? t('chat.investigationStatusInvestigating')
    : formatTurnStatus(turnStatus, t);
  const routeLabel = routeKind
    ? formatRouteKind(routeKind)
    : (isStreaming ? t('chat.investigationRouteLiveDefault') : t('chat.investigationRouteIdle'));
  const title = collectionContext?.title || conversationTitle || t('chat.investigationNew');
  const supportingCount = evidenceCount > 0
    ? t('chat.investigationSupportingCount', { count: evidenceCount })
    : t('chat.investigationSupportingNone');
  const contextSummary = collectionContext?.description?.trim()
    || collectionContext?.queryText?.trim()
    || t('chat.investigationDefaultSummary');

  return (
    <div className="shrink-0 border-b border-border/60 bg-surface-1/80 px-3 py-3 backdrop-blur">
      <div className="rounded-2xl border border-border/70 bg-surface-0/80">
        <button
          type="button"
          onClick={() => setExpanded((prev) => !prev)}
          className="flex w-full flex-wrap items-start gap-2 px-4 py-3 text-left"
          aria-expanded={expanded}
        >
          <div className="min-w-0 flex-1">
            <div className="text-[11px] uppercase tracking-[0.16em] text-text-tertiary">
              {t('chat.investigationLabel')}
            </div>
            <div className="mt-1 text-sm font-semibold text-text-primary truncate">
              {title}
            </div>
            <div className="mt-1 flex flex-wrap items-center gap-1.5 text-[11px]">
              <span className="inline-flex items-center gap-1.5 rounded-full border border-border/70 bg-surface-1/70 px-2 py-1 text-text-secondary">
                <Sparkles className="h-3.5 w-3.5 text-accent" />
                {statusLabel}
              </span>
              <span className={`inline-flex items-center gap-1.5 rounded-full border px-2 py-1 ${evidenceTone(evidenceLevel)}`}>
                <ShieldCheck className="h-3.5 w-3.5" />
                {evidenceLabel(evidenceLevel, t)}
              </span>
              <span className="inline-flex items-center gap-1.5 rounded-full border border-border/70 bg-surface-1/70 px-2 py-1 text-text-secondary">
                <FolderOpen className="h-3.5 w-3.5" />
                {scopeLabel}
              </span>
              {collectionContext && (
                <span className="inline-flex items-center gap-1.5 rounded-full border border-accent/25 bg-accent/10 px-2 py-1 text-accent">
                  <BookOpen className="h-3.5 w-3.5" />
                  {t('chat.investigationWorkingSet')}
                </span>
              )}
            </div>
          </div>
          <span className="inline-flex items-center gap-1 text-[11px] text-text-tertiary">
            {t('common.expand')}
            <ChevronDown className={`h-3.5 w-3.5 transition-transform ${expanded ? 'rotate-180' : ''}`} />
          </span>
        </button>

        {expanded && (
          <div className="border-t border-border/60 px-4 pb-4 pt-3">
            <p className="max-w-3xl text-sm text-text-secondary">
              {contextSummary}
            </p>

            <div className="mt-4 grid gap-2 lg:grid-cols-4">
              <div className="rounded-xl border border-border/60 bg-surface-1/70 px-3 py-2.5">
                <div className="flex items-center gap-1.5 text-[11px] font-medium text-text-tertiary">
                  <BookOpen className="h-3.5 w-3.5" />
                  {t('chat.investigationWorkingSet')}
                </div>
                <div className="mt-1 text-sm font-medium text-text-primary">
                  {collectionContext?.title || t('chat.investigationConversationContext')}
                </div>
                <div className="mt-1 text-[11px] text-text-secondary">
                  {collectionContext
                    ? t('chat.investigationCollectionContext')
                    : t('chat.investigationGeneralContext')}
                </div>
              </div>

              <div className="rounded-xl border border-border/60 bg-surface-1/70 px-3 py-2.5">
                <div className="flex items-center gap-1.5 text-[11px] font-medium text-text-tertiary">
                  <FolderOpen className="h-3.5 w-3.5" />
                  {t('chat.contextScopeLabel')}
                </div>
                <div className="mt-1 text-sm font-medium text-text-primary">
                  {scopeLabel}
                </div>
                <div className="mt-1 text-[11px] text-text-secondary">
                  {scopeHint}
                </div>
              </div>

              <div className="rounded-xl border border-border/60 bg-surface-1/70 px-3 py-2.5">
                <div className="flex items-center gap-1.5 text-[11px] font-medium text-text-tertiary">
                  <Compass className="h-3.5 w-3.5" />
                  {t('chat.investigationRouteLabel')}
                </div>
                <div className="mt-1 text-sm font-medium text-text-primary">
                  {routeLabel}
                </div>
                <div className="mt-1 text-[11px] text-text-secondary">
                  {isStreaming
                    ? t('chat.investigationRouteLiveHint')
                    : t('chat.investigationRouteLastTurnHint')}
                </div>
              </div>

              <div className="rounded-xl border border-border/60 bg-surface-1/70 px-3 py-2.5">
                <div className="flex items-center gap-1.5 text-[11px] font-medium text-text-tertiary">
                  <Database className="h-3.5 w-3.5" />
                  {t('chat.answerEvidence')}
                </div>
                <div className="mt-1 text-sm font-medium text-text-primary">
                  {evidenceLabel(evidenceLevel, t)}
                </div>
                <div className="mt-1 text-[11px] text-text-secondary">
                  {supportingCount}
                </div>
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
