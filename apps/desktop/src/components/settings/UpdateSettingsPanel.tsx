import { useState } from 'react';
import ReactMarkdown from 'react-markdown';
import {
  AlertTriangle,
  CheckCircle,
  Download,
  Loader2,
  RefreshCw,
  XCircle,
} from 'lucide-react';
import { useTranslation } from '../../i18n';
import { useUpdater } from '../../lib/useUpdater';
import {
  markdownComponents,
  markdownRemarkPlugins,
  rehypePlugins,
} from '../chat/markdownComponents';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';

type UpdaterState = ReturnType<typeof useUpdater>;

function formatUpdateTimestamp(value: string | undefined, locale: string): string {
  if (!value) return '';
  const time = new Date(value);
  if (Number.isNaN(time.getTime())) return '';
  return time.toLocaleString(locale);
}

interface UpdateSettingsPanelProps {
  appVersion: string;
  updater: UpdaterState;
}

export function UpdateSettingsPanel({ appVersion, updater }: UpdateSettingsPanelProps) {
  const { t, locale } = useTranslation();
  const [detailsOpen, setDetailsOpen] = useState(false);
  const {
    status,
    version,
    notes,
    progress,
    error,
    errorCode,
    errorDetail,
    errorStage,
    lastCheckedAt,
    checkForUpdate,
    downloadAndInstall,
    restart,
  } = updater;

  const statusMeta = (() => {
    switch (status) {
      case 'checking':
        return { label: t('knowledge.checking'), variant: 'info' as const, icon: <Loader2 size={14} className="animate-spin" /> };
      case 'available':
        return { label: t('update.available'), variant: 'warning' as const, icon: <Download size={14} /> };
      case 'downloading':
        return { label: t('update.downloading'), variant: 'info' as const, icon: <Loader2 size={14} className="animate-spin" /> };
      case 'ready':
        return { label: t('update.ready'), variant: 'success' as const, icon: <CheckCircle size={14} /> };
      case 'error':
        return { label: t('update.error'), variant: 'danger' as const, icon: <XCircle size={14} /> };
      case 'up-to-date':
        return { label: t('update.upToDate'), variant: 'success' as const, icon: <CheckCircle size={14} /> };
      default:
        return { label: t('update.notChecked'), variant: 'default' as const, icon: <RefreshCw size={14} /> };
    }
  })();

  const checkedAt = formatUpdateTimestamp(lastCheckedAt, locale);
  const errorLabel =
    errorStage === 'download'
      ? t('update.downloadFailed')
      : errorStage === 'install'
        ? t('update.installFailed')
        : t('update.error');

  return (
    <div className="space-y-4 border-t border-border pt-4">
      <div className="flex flex-col gap-3 md:flex-row md:items-start md:justify-between">
        <div className="space-y-1">
          <h3 className="flex items-center gap-2 text-sm font-semibold text-text-primary">
            <RefreshCw size={16} className="text-accent" />
            {t('update.appUpdate')}
          </h3>
          <p className="max-w-2xl text-xs text-text-tertiary">{t('update.appUpdateDescription')}</p>
        </div>

        <div className="flex shrink-0 flex-wrap items-center gap-2">
          {status === 'available' && (
            <Button
              variant="primary"
              size="sm"
              icon={<Download size={14} />}
              onClick={downloadAndInstall}
            >
              {t('update.downloadInstall')}
            </Button>
          )}
          {status === 'ready' && (
            <Button
              variant="primary"
              size="sm"
              icon={<RefreshCw size={14} />}
              onClick={restart}
            >
              {t('update.restart')}
            </Button>
          )}
          {status !== 'available' && status !== 'ready' && (
            <Button
              variant="secondary"
              size="sm"
              icon={status === 'checking' ? <Loader2 size={14} className="animate-spin" /> : <RefreshCw size={14} />}
              loading={status === 'checking'}
              disabled={status === 'downloading'}
              onClick={checkForUpdate}
            >
              {t('update.checkNow')}
            </Button>
          )}
        </div>
      </div>

      <div className="grid gap-3 md:grid-cols-[1fr_1fr_1.25fr]">
        <div className="rounded-lg bg-surface-2 px-4 py-3">
          <p className="text-[11px] font-medium uppercase text-text-tertiary">{t('update.currentVersion')}</p>
          <p className="mt-1 text-lg font-semibold tabular-nums text-text-primary">v{appVersion || '...'}</p>
        </div>
        <div className="rounded-lg bg-surface-2 px-4 py-3">
          <p className="text-[11px] font-medium uppercase text-text-tertiary">{t('update.latestVersion')}</p>
          <p className="mt-1 text-lg font-semibold tabular-nums text-text-primary">
            {version ? `v${version}` : '-'}
          </p>
        </div>
        <div className="rounded-lg bg-surface-2 px-4 py-3">
          <p className="text-[11px] font-medium uppercase text-text-tertiary">{t('update.status')}</p>
          <div className="mt-1 flex flex-wrap items-center gap-2">
            <Badge variant={statusMeta.variant} className="gap-1.5">
              {statusMeta.icon}
              {statusMeta.label}
            </Badge>
            <span className="text-xs text-text-tertiary">
              {checkedAt ? t('update.lastChecked', { time: checkedAt }) : t('update.notChecked')}
            </span>
          </div>
        </div>
      </div>

      {status === 'downloading' && (
        <div className="flex items-center gap-3">
          <div className="h-2 flex-1 overflow-hidden rounded-full bg-surface-3">
            <div
              className="h-full rounded-full bg-accent transition-all duration-300"
              style={{ width: `${progress ?? 0}%` }}
            />
          </div>
          <span className="w-10 text-right text-xs tabular-nums text-text-tertiary">{progress ?? 0}%</span>
        </div>
      )}

      {status === 'error' && (
        <div className="rounded-lg border border-danger/20 bg-danger/5 px-4 py-3">
          <div className="flex items-start gap-2">
            <AlertTriangle size={16} className="mt-0.5 shrink-0 text-danger" />
            <div className="min-w-0 flex-1">
              <p className="text-sm font-medium text-danger">{errorLabel}</p>
              {error && <p className="mt-1 wrap-break-word text-xs text-text-secondary">{error}</p>}
              {(errorCode != null || errorDetail?.stack) && (
                <div className="mt-2">
                  <button
                    type="button"
                    onClick={() => setDetailsOpen(v => !v)}
                    className="text-xs text-text-tertiary transition-colors hover:text-text-primary"
                  >
                    {detailsOpen ? '▼' : '▶'} {t('update.details')}
                  </button>
                  {detailsOpen && (
                    <pre className="mt-2 max-h-40 overflow-auto rounded-md bg-surface-1 p-2 text-xs text-text-tertiary whitespace-pre-wrap break-all">
                      {errorCode != null && `code: ${errorCode}\n`}
                      {errorDetail?.stack ?? ''}
                    </pre>
                  )}
                </div>
              )}
            </div>
          </div>
        </div>
      )}

      {notes && (
        <details className="rounded-lg border border-border bg-surface-2 px-4 py-3">
          <summary className="cursor-pointer text-sm font-medium text-text-primary">
            {t('update.releaseNotes')}
          </summary>
          <div className="mt-2 max-h-72 overflow-auto rounded-md border border-border/60 bg-surface-1/70 px-3 py-2 text-xs leading-relaxed text-text-secondary">
            <ReactMarkdown
              remarkPlugins={markdownRemarkPlugins}
              rehypePlugins={rehypePlugins}
              components={markdownComponents}
            >
              {notes}
            </ReactMarkdown>
          </div>
        </details>
      )}
    </div>
  );
}
