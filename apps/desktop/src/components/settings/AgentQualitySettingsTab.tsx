import {
  AlertTriangle,
  CheckCircle2,
  Clipboard,
  ClipboardCheck,
  Loader2,
  Play,
  RefreshCw,
  XCircle,
} from 'lucide-react';
import { toast } from 'sonner';
import { useTranslation } from '../../i18n';
import type {
  QualityEvalCaseResult,
  QualityEvalCheckResult,
  QualityEvalReport,
  QualityEvalSuiteReport,
} from '../../types/qualityEval';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { Section } from './SettingsSection';

interface AgentQualitySettingsTabProps {
  report: QualityEvalReport | null;
  loading: boolean;
  lastRunAt: string | null;
  onRun: () => void;
}

function statusBadgeVariant(status: string): 'success' | 'danger' | 'default' {
  if (status === 'passed') return 'success';
  if (status === 'failed') return 'danger';
  return 'default';
}

function severityBadgeVariant(severity: string): 'danger' | 'warning' | 'info' | 'default' {
  if (severity === 'critical') return 'danger';
  if (severity === 'high') return 'warning';
  if (severity === 'medium') return 'info';
  return 'default';
}

function formatLastRun(value: string | null): string {
  if (!value) return '';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString();
}

async function copyText(value: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(value);
    return;
  }

  const textarea = document.createElement('textarea');
  textarea.value = value;
  textarea.setAttribute('readonly', 'true');
  textarea.style.position = 'fixed';
  textarea.style.left = '-9999px';
  textarea.style.top = '0';
  document.body.appendChild(textarea);
  textarea.select();

  try {
    if (!document.execCommand('copy')) {
      throw new Error('copy command was rejected');
    }
  } finally {
    document.body.removeChild(textarea);
  }
}

function ScoreTile({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0 rounded-lg border border-border/70 bg-surface-2 px-4 py-3">
      <p className="truncate text-[11px] font-medium uppercase text-text-tertiary">{label}</p>
      <p className="mt-1 text-lg font-semibold tabular-nums text-text-primary">{value}</p>
    </div>
  );
}

function CheckRow({ check }: { check: QualityEvalCheckResult }) {
  return (
    <div className="flex min-w-0 items-start gap-2 rounded-md bg-surface-2/70 px-3 py-2">
      {check.passed ? (
        <CheckCircle2 className="mt-0.5 shrink-0 text-success" size={14} />
      ) : (
        <XCircle className="mt-0.5 shrink-0 text-danger" size={14} />
      )}
      <div className="min-w-0">
        <div className="font-mono text-[11px] text-text-secondary">{check.id}</div>
        <div className="mt-0.5 break-words text-xs leading-relaxed text-text-tertiary">
          {check.detail}
        </div>
      </div>
    </div>
  );
}

function CaseRow({ item }: { item: QualityEvalCaseResult }) {
  const { t } = useTranslation();
  const failedChecks = item.checks.filter((check) => !check.passed).length;

  return (
    <div
      data-testid={`agent-quality-case-${item.id}`}
      className="rounded-lg border border-border/70 bg-surface-1"
    >
      <div className="flex flex-wrap items-start justify-between gap-3 px-4 py-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            {item.passed ? (
              <CheckCircle2 className="shrink-0 text-success" size={15} />
            ) : (
              <AlertTriangle className="shrink-0 text-danger" size={15} />
            )}
            <h4 className="text-sm font-medium text-text-primary">{item.label}</h4>
          </div>
          <div className="mt-2 flex flex-wrap items-center gap-2">
            <Badge variant={severityBadgeVariant(item.severity)}>{item.severity}</Badge>
            <span className="text-[11px] tabular-nums text-text-tertiary">
              {item.checks.length} {t('settings.agentQualityChecks')}
            </span>
            {failedChecks > 0 && (
              <span className="text-[11px] tabular-nums text-danger">
                {failedChecks} {t('settings.agentQualityFailedChecks')}
              </span>
            )}
          </div>
        </div>
        <Badge variant={item.passed ? 'success' : 'danger'}>
          {item.passed ? t('settings.agentQualityPassed') : t('settings.agentQualityFailed')}
        </Badge>
      </div>

      <div className="space-y-2 border-t border-border/70 px-4 py-3">
        {item.checks.map((check) => (
          <CheckRow key={check.id} check={check} />
        ))}
      </div>
    </div>
  );
}

function SuiteBlock({ suite }: { suite: QualityEvalSuiteReport }) {
  const { t } = useTranslation();

  return (
    <div
      data-testid={`agent-quality-suite-${suite.id}`}
      className="rounded-lg border border-border bg-surface-2/40"
    >
      <div className="flex flex-wrap items-start justify-between gap-3 px-4 py-3">
        <div className="min-w-0">
          <h3 className="text-sm font-semibold text-text-primary">{suite.label}</h3>
          <p className="mt-1 text-xs tabular-nums text-text-tertiary">
            {suite.passed} / {suite.total}
          </p>
        </div>
        <Badge variant={suite.failed === 0 ? 'success' : 'danger'}>
          {suite.failed === 0 ? t('settings.agentQualityPassed') : t('settings.agentQualityFailed')}
        </Badge>
      </div>
      <div className="space-y-3 border-t border-border px-4 py-4">
        {suite.cases.map((item) => (
          <CaseRow key={item.id} item={item} />
        ))}
      </div>
    </div>
  );
}

export function AgentQualitySettingsTab({
  report,
  loading,
  lastRunAt,
  onRun,
}: AgentQualitySettingsTabProps) {
  const { t } = useTranslation();
  const statusLabel = report
    ? report.status === 'passed'
      ? t('settings.agentQualityPassed')
      : t('settings.agentQualityFailed')
    : t('settings.agentQualityNotRun');
  const handleCopyReport = async () => {
    if (!report) return;

    try {
      await copyText(JSON.stringify(report, null, 2));
      toast.success(t('settings.agentQualityCopied'));
    } catch {
      toast.error(t('settings.agentQualityCopyError'));
    }
  };

  return (
    <Section
      icon={<ClipboardCheck size={20} />}
      title={t('settings.agentQualityTitle')}
      description={t('settings.agentQualityDescription')}
      delay={0.02}
      summary={
        <Badge variant={report ? statusBadgeVariant(report.status) : 'default'}>
          {statusLabel}
        </Badge>
      }
    >
      <div data-testid="agent-quality-panel" className="space-y-5">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <Badge variant={report ? statusBadgeVariant(report.status) : 'default'}>
              {statusLabel}
            </Badge>
            {lastRunAt && (
              <span className="text-xs text-text-tertiary">
                {t('settings.agentQualityLastRun')}: {formatLastRun(lastRunAt)}
              </span>
            )}
          </div>
          <div className="flex shrink-0 items-center gap-2">
            {report && (
              <Button
                variant="ghost"
                size="sm"
                icon={<Clipboard size={14} />}
                onClick={() => { void handleCopyReport(); }}
              >
                {t('settings.agentQualityCopyJson')}
              </Button>
            )}
            <Button
              variant="secondary"
              size="sm"
              icon={report ? <RefreshCw size={14} /> : <Play size={14} />}
              loading={loading}
              onClick={onRun}
            >
              {loading
                ? t('settings.agentQualityRunning')
                : report
                  ? t('settings.agentQualityRerun')
                  : t('settings.agentQualityRun')}
            </Button>
          </div>
        </div>

        {loading && !report ? (
          <div className="flex items-center gap-2 rounded-lg border border-border/70 bg-surface-2 px-4 py-5 text-sm text-text-tertiary">
            <Loader2 size={15} className="animate-spin" />
            <span>{t('settings.agentQualityRunning')}</span>
          </div>
        ) : report ? (
          <div className="space-y-5">
            <div className="grid gap-3 sm:grid-cols-3">
              <ScoreTile
                label={t('settings.agentQualityCases')}
                value={`${report.passed} / ${report.total}`}
              />
              <ScoreTile
                label={t('settings.agentQualitySuites')}
                value={String(report.suites.length)}
              />
              <ScoreTile
                label={t('settings.agentQualityFailedCases')}
                value={String(report.failed)}
              />
            </div>

            {report.failed === 0 && (
              <div className="flex items-start gap-2 rounded-lg border border-success/25 bg-success/10 px-4 py-3 text-sm text-success">
                <CheckCircle2 className="mt-0.5 shrink-0" size={15} />
                <span>{t('settings.agentQualityNoFailures')}</span>
              </div>
            )}

            <div className="space-y-4">
              {report.suites.map((suite) => (
                <SuiteBlock key={suite.id} suite={suite} />
              ))}
            </div>
          </div>
        ) : (
          <div className="rounded-lg border border-dashed border-border bg-surface-2/60 px-4 py-5">
            <p className="text-sm font-medium text-text-primary">
              {t('settings.agentQualityNotRun')}
            </p>
            <p className="mt-1 text-sm leading-relaxed text-text-tertiary">
              {t('settings.agentQualityUnavailable')}
            </p>
          </div>
        )}
      </div>
    </Section>
  );
}
