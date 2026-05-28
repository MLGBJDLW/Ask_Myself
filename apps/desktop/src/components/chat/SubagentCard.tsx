import { useEffect, useMemo, useState } from 'react';
import {
  Bot,
  ChevronDown,
  Loader2,
  CheckCircle2,
  AlertTriangle,
  Wrench,
  BrainCircuit,
  Flag,
} from 'lucide-react';
import type { SubagentRun } from '../../lib/subagentArtifacts';
import { getSubagentToolDescriptor } from '../../lib/subagentTools';
import { useTranslation } from '../../i18n';

interface SubagentCardProps {
  run: SubagentRun;
  compact?: boolean;
  defaultOpen?: boolean;
}

type TranslateFn = ReturnType<typeof useTranslation>['t'];

function statusCopy(status: SubagentRun['status'], t: TranslateFn) {
  switch (status) {
    case 'running':
      return {
        label: t('chat.subagentStatusRunning'),
        icon: Loader2,
        className: 'text-accent',
        chipClassName: 'border-accent/25 bg-accent/10 text-accent',
        spin: true,
      };
    case 'error':
      return {
        label: t('chat.subagentStatusNeedsAttention'),
        icon: AlertTriangle,
        className: 'text-danger',
        chipClassName: 'border-danger/25 bg-danger/10 text-danger',
        spin: false,
      };
    case 'done':
    default:
      return {
        label: t('chat.subagentStatusComplete'),
        icon: CheckCircle2,
        className: 'text-success',
        chipClassName: 'border-success/25 bg-success/10 text-success',
        spin: false,
      };
  }
}

function toolLabel(name: string) {
  return getSubagentToolDescriptor(name)?.label ?? name;
}

function truncate(value: string, max = 220) {
  const text = value.trim();
  if (text.length <= max) return text;
  return `${text.slice(0, max).trimEnd()}...`;
}

export function SubagentCard({
  run,
  compact = false,
  defaultOpen,
}: SubagentCardProps) {
  const { t } = useTranslation();
  const autoOpen = defaultOpen ?? run.status === 'running';
  const [expanded, setExpanded] = useState(autoOpen);
  const status = statusCopy(run.status, t);
  const StatusIcon = status.icon;

  useEffect(() => {
    if (run.status === 'running') {
      setExpanded(true);
    }
  }, [run.status]);

  const startedTools = useMemo(
    () => run.toolEvents.filter(event => event.phase === 'start'),
    [run.toolEvents],
  );
  const failedTools = useMemo(
    () => run.toolEvents.filter(event => event.phase === 'result' && event.isError),
    [run.toolEvents],
  );
  const summaryText = run.result
    ? truncate(run.result, compact ? 120 : 180)
    : run.content
      ? truncate(run.content, compact ? 120 : 180)
      : t('chat.subagentInProgress');
  const displayRole = run.roleName?.trim() || run.role?.trim() || t('chat.helperDefaultLabel');

  return (
    <div className={`rounded-lg border bg-surface-0/55 ${compact ? 'border-border/45' : 'border-border/60'}`}>
      <button
        type="button"
        onClick={() => setExpanded(prev => !prev)}
        className={`flex w-full items-start gap-2.5 text-left transition-colors hover:bg-surface-0/45 ${compact ? 'px-2.5 py-2' : 'px-3 py-2.5'}`}
        aria-expanded={expanded}
      >
        <span className={`mt-0.5 inline-flex shrink-0 items-center justify-center rounded-lg border border-border/55 bg-surface-1/70 text-accent ${compact ? 'h-6 w-6' : 'h-7 w-7'}`}>
          <Bot className={compact ? 'h-3.5 w-3.5' : 'h-4 w-4'} />
        </span>

        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-1.5">
            <span className={`${compact ? 'text-xs' : 'text-[13px]'} font-semibold text-text-primary`}>
              {displayRole}
            </span>
            {run.roleId && (
              <span className="inline-flex items-center rounded-full border border-border/60 bg-surface-1 px-2 py-0.5 text-[11px] text-text-tertiary">
                {run.roleId}
              </span>
            )}
            <span className={`inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] ${status.chipClassName}`}>
              <StatusIcon className={`h-3 w-3 ${status.spin ? 'animate-spin' : ''}`} />
              {status.label}
            </span>
            {startedTools.length > 0 && (
              <span className="inline-flex items-center gap-1 rounded-full border border-border/60 bg-surface-1 px-2 py-0.5 text-[11px] text-text-secondary">
                <Wrench className="h-3 w-3" />
                {t('chat.subagentToolCount', { count: String(startedTools.length) })}
              </span>
            )}
            {failedTools.length > 0 && (
              <span className="inline-flex items-center gap-1 rounded-full border border-danger/25 bg-danger/10 px-2 py-0.5 text-[11px] text-danger">
                <AlertTriangle className="h-3 w-3" />
                {t('chat.subagentIssueCount', { count: String(failedTools.length) })}
              </span>
            )}
          </div>

          <div className={`mt-1 text-text-primary ${compact ? 'text-xs' : 'text-sm'}`}>{run.task}</div>
          <div className={`mt-0.5 text-text-tertiary ${compact ? 'text-[11px]' : 'text-[12px]'}`}>{summaryText}</div>
        </div>

        <ChevronDown
          className={`mt-1 h-4 w-4 shrink-0 text-text-tertiary transition-transform ${expanded ? 'rotate-180' : ''}`}
        />
      </button>

      {expanded && (
        <div className={`border-t border-border/40 ${compact ? 'px-2.5 py-2' : 'px-3 py-2.5'}`}>
          <div className="flex flex-wrap gap-1.5">
            {run.expectedOutput && (
              <span className="inline-flex items-center gap-1 rounded-full border border-border/55 bg-surface-1/70 px-2 py-0.5 text-[11px] text-text-secondary">
                <Flag className="h-3 w-3" />
                {run.expectedOutput}
              </span>
            )}
            {run.parallelGroup && (
              <span className="inline-flex items-center gap-1 rounded-full border border-border/55 bg-surface-1/70 px-2 py-0.5 text-[11px] text-text-secondary">
                {t('chat.subagentParallel', { value: run.parallelGroup })}
              </span>
            )}
            {run.deliverableStyle && (
              <span className="inline-flex items-center gap-1 rounded-full border border-border/55 bg-surface-1/70 px-2 py-0.5 text-[11px] text-text-secondary">
                {t('chat.subagentStyle', { value: run.deliverableStyle })}
              </span>
            )}
            {run.finishReason && (
              <span className="inline-flex items-center gap-1 rounded-full border border-border/55 bg-surface-1/70 px-2 py-0.5 text-[11px] text-text-secondary">
                {t('chat.subagentFinish', { value: run.finishReason })}
              </span>
            )}
            {typeof run.usageTotal?.totalTokens === 'number' && run.usageTotal.totalTokens > 0 && (
              <span className="inline-flex items-center gap-1 rounded-full border border-border/55 bg-surface-1/70 px-2 py-0.5 text-[11px] text-text-secondary">
                {run.usageTotal.totalTokens.toLocaleString()} tokens
              </span>
            )}
            {run.sourceScopeApplied && (
              <span className="inline-flex items-center gap-1 rounded-full border border-border/55 bg-surface-1/70 px-2 py-0.5 text-[11px] text-text-secondary">
                {t('chat.subagentSourceScopeInherited')}
              </span>
            )}
            {run.evidenceChunkIds && run.evidenceChunkIds.length > 0 && (
              <span className="inline-flex items-center gap-1 rounded-full border border-border/55 bg-surface-1/70 px-2 py-0.5 text-[11px] text-text-secondary">
                {t('chat.subagentEvidenceCount', { count: String(run.evidenceChunkIds.length) })}
              </span>
            )}
          </div>

          {run.acceptanceCriteria && run.acceptanceCriteria.length > 0 && (
            <div className="mt-3">
              <div className="mb-1 text-[11px] uppercase tracking-[0.14em] text-text-tertiary">
                {t('chat.subagentAcceptanceCriteria')}
              </div>
              <ul className="space-y-1 text-xs text-text-secondary">
                {run.acceptanceCriteria.map((criterion, index) => (
                  <li key={`${run.id}-criterion-${index}`} className="rounded-md border border-border/60 bg-surface-1 px-2.5 py-1.5">
                    {criterion}
                  </li>
                ))}
              </ul>
            </div>
          )}

          {(run.requestedSourceScope || run.effectiveSourceScope) && (
            <div className="mt-3 grid gap-2 md:grid-cols-2">
              {run.requestedSourceScope && run.requestedSourceScope.length > 0 && (
                <div>
                  <div className="mb-1 text-[11px] uppercase tracking-[0.14em] text-text-tertiary">
                    {t('chat.subagentRequestedSourceScope')}
                  </div>
                  <div className="flex flex-wrap gap-1.5">
                    {run.requestedSourceScope.map(sourceId => (
                      <span
                        key={`${run.id}-requested-source-${sourceId}`}
                        className="inline-flex items-center rounded-md border border-border/60 bg-surface-1 px-2 py-1 text-[11px] text-text-secondary"
                      >
                        {sourceId}
                      </span>
                    ))}
                  </div>
                </div>
              )}
              {run.effectiveSourceScope && run.effectiveSourceScope.length > 0 && (
                <div>
                  <div className="mb-1 text-[11px] uppercase tracking-[0.14em] text-text-tertiary">
                    {t('chat.subagentEffectiveSourceScope')}
                  </div>
                  <div className="flex flex-wrap gap-1.5">
                    {run.effectiveSourceScope.map(sourceId => (
                      <span
                        key={`${run.id}-effective-source-${sourceId}`}
                        className="inline-flex items-center rounded-md border border-border/60 bg-surface-1 px-2 py-1 text-[11px] text-text-secondary"
                      >
                        {sourceId}
                      </span>
                    ))}
                  </div>
                </div>
              )}
            </div>
          )}

          {run.allowedTools && (
            <div className="mt-3">
              <div className="mb-1 text-[11px] uppercase tracking-[0.14em] text-text-tertiary">
                {t('chat.subagentAllowedTools')}
              </div>
              <div className="flex flex-wrap gap-1.5">
                {run.allowedTools.length > 0 ? run.allowedTools.map(toolName => (
                  <span
                    key={toolName}
                    className="inline-flex items-center rounded-md border border-border/60 bg-surface-1 px-2 py-1 text-[11px] text-text-secondary"
                    title={toolName}
                  >
                    {toolLabel(toolName)}
                  </span>
                )) : (
                  <span className="text-xs text-text-tertiary">{t('chat.subagentNoToolsDelegated')}</span>
                )}
              </div>
            </div>
          )}

          {run.allowedSkills && (
            <div className="mt-3">
              <div className="mb-1 text-[11px] uppercase tracking-[0.14em] text-text-tertiary">
                {t('chat.subagentAllowedSkills')}
              </div>
              <div className="flex flex-wrap gap-1.5">
                {run.allowedSkills.length > 0 ? run.allowedSkills.map(skill => (
                  <span
                    key={`${run.id}-skill-${skill.id}`}
                    className="inline-flex items-center rounded-md border border-border/60 bg-surface-1 px-2 py-1 text-[11px] text-text-secondary"
                    title={skill.id}
                  >
                    {skill.name}
                  </span>
                )) : (
                  <span className="text-xs text-text-tertiary">{t('chat.subagentNoSkillsDelegated')}</span>
                )}
              </div>
            </div>
          )}

          {run.requestedAllowedTools && run.requestedAllowedTools.length > 0 && (
            <div className="mt-3">
              <div className="mb-1 text-[11px] uppercase tracking-[0.14em] text-text-tertiary">
                {t('chat.subagentRequestedToolScope')}
              </div>
              <div className="flex flex-wrap gap-1.5">
                {run.requestedAllowedTools.map(toolName => (
                  <span
                    key={`${run.id}-requested-tool-${toolName}`}
                    className="inline-flex items-center rounded-md border border-border/60 bg-surface-1 px-2 py-1 text-[11px] text-text-secondary"
                    title={toolName}
                  >
                    {toolLabel(toolName)}
                  </span>
                ))}
              </div>
            </div>
          )}

          {run.returnSections && run.returnSections.length > 0 && (
            <div className="mt-3">
              <div className="mb-1 text-[11px] uppercase tracking-[0.14em] text-text-tertiary">
                {t('chat.subagentRequestedSections')}
              </div>
              <div className="flex flex-wrap gap-1.5">
                {run.returnSections.map((section, index) => (
                  <span
                    key={`${run.id}-section-${index}`}
                    className="inline-flex items-center rounded-md border border-border/60 bg-surface-1 px-2 py-1 text-[11px] text-text-secondary"
                  >
                    {section}
                  </span>
                ))}
              </div>
            </div>
          )}

          {run.evidenceHandoff && run.evidenceHandoff.length > 0 && (
            <div className="mt-3">
              <div className="mb-2 text-[11px] uppercase tracking-[0.14em] text-text-tertiary">
                {t('chat.subagentEvidenceHandoff')}
              </div>
              <div className="space-y-2">
                {run.evidenceHandoff.map((evidence) => (
                  <div
                    key={`${run.id}-evidence-${evidence.chunkId}`}
                    className="rounded-lg border border-border/60 bg-surface-1 px-3 py-2"
                  >
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="text-xs font-medium text-text-primary">
                        {evidence.title || evidence.path}
                      </span>
                      <span className="rounded-full border border-border/60 bg-surface-0 px-1.5 py-0.5 text-[10px] uppercase tracking-[0.14em] text-text-tertiary">
                        {evidence.chunkId}
                      </span>
                    </div>
                    <div className="mt-1 text-[11px] text-text-tertiary">{evidence.path}</div>
                    <pre className="mt-2 max-h-28 overflow-y-auto whitespace-pre-wrap rounded-md bg-surface-0 px-2 py-1 text-[11px] text-text-tertiary">
                      {evidence.excerpt}
                    </pre>
                  </div>
                ))}
              </div>
            </div>
          )}

          {run.result && (
            <div className="mt-3">
              <div className="mb-1 text-[11px] uppercase tracking-[0.14em] text-text-tertiary">
                {t('chat.subagentResult')}
              </div>
              <pre className="max-h-64 overflow-y-auto whitespace-pre-wrap rounded-lg border border-border/60 bg-surface-1 p-3 text-xs text-text-secondary">
                {run.result}
              </pre>
            </div>
          )}

          {run.toolEvents.length > 0 && (
            <div className="mt-3">
              <div className="mb-2 text-[11px] uppercase tracking-[0.14em] text-text-tertiary">
                {t('chat.subagentInnerTrace')}
              </div>
              <div className="space-y-1.5">
                {run.toolEvents.map((event, index) => (
                  <div
                    key={`${run.id}-${event.callId}-${event.phase}-${index}`}
                    className={`border-l py-1 pl-2.5 pr-1 ${event.isError ? 'border-danger/35' : 'border-border/35'}`}
                  >
                    <div className="flex min-w-0 flex-wrap items-center gap-1.5">
                      <span className="inline-flex max-w-full items-center gap-1 rounded-full border border-border/55 bg-surface-0/45 px-2 py-0.5 text-[11px] font-medium text-text-secondary">
                        <Wrench className="h-3 w-3 shrink-0 text-text-tertiary" />
                        <span className="truncate">{toolLabel(event.toolName)}</span>
                      </span>
                      <span className="rounded-full border border-border/55 bg-surface-0/45 px-1.5 py-0.5 text-[10px] uppercase tracking-[0.12em] text-text-tertiary">
                        {event.phase}
                      </span>
                      {event.isError && (
                        <span className="rounded-full border border-danger/25 bg-danger/10 px-1.5 py-0.5 text-[10px] uppercase tracking-[0.12em] text-danger">
                          {t('chat.subagentError')}
                        </span>
                      )}
                    </div>
                    {(event.content || event.arguments) && (
                      <pre className="mt-1 max-h-32 overflow-y-auto whitespace-pre-wrap rounded-md bg-surface-0/35 px-2 py-1 text-[11px] text-text-tertiary">
                        {event.content || event.arguments}
                      </pre>
                    )}
                  </div>
                ))}
              </div>
            </div>
          )}

          {run.thinking && run.thinking.length > 0 && (
            <details className="group mt-3 rounded-lg border border-border/60 bg-surface-1 px-3 py-2">
              <summary className="flex cursor-pointer list-none items-center gap-2 text-xs font-medium text-text-secondary [&::-webkit-details-marker]:hidden">
                <BrainCircuit className="h-3.5 w-3.5 text-accent" />
                {t('chat.subagentSupervisorNotes')}
                <ChevronDown className="ml-auto h-3.5 w-3.5 text-text-tertiary transition-transform group-open:rotate-180" />
              </summary>
              <div className="mt-2 space-y-2">
                {run.thinking.map((entry, index) => (
                  <pre
                    key={`${run.id}-thinking-${index}`}
                    className="max-h-24 overflow-y-auto whitespace-pre-wrap rounded-md bg-surface-0 px-2 py-1 text-[11px] text-text-tertiary"
                  >
                    {entry}
                  </pre>
                ))}
              </div>
            </details>
          )}
        </div>
      )}
    </div>
  );
}
