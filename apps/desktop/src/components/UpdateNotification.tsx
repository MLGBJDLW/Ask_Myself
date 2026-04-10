import { X, Download, RefreshCw } from 'lucide-react';
import { useUpdater } from '../lib/useUpdater';
import { useTranslation } from '../i18n';
import { useState } from 'react';

export function UpdateNotification() {
  const { t } = useTranslation();
  const { status, version, progress, downloadAndInstall, checkForUpdate } = useUpdater();
  const [dismissed, setDismissed] = useState(false);

  if (dismissed || status === 'idle' || status === 'checking' || status === 'up-to-date') {
    return null;
  }

  return (
    <div className="relative flex items-center gap-3 px-4 py-2.5 bg-accent/10 border-b border-accent/20 text-sm">
      {status === 'available' && (
        <>
          <Download className="w-4 h-4 text-accent shrink-0" />
          <span className="text-text-primary">
            {t('update.version').replace('{version}', version ?? '')}
          </span>
          <button
            onClick={downloadAndInstall}
            className="ml-auto px-3 py-1 rounded-md bg-accent text-white text-xs font-medium hover:bg-accent-hover transition-colors"
          >
            {t('update.downloadInstall')}
          </button>
        </>
      )}

      {status === 'downloading' && (
        <>
          <RefreshCw className="w-4 h-4 text-accent shrink-0 animate-spin" />
          <span className="text-text-primary">{t('update.downloading')}</span>
          <div className="flex-1 max-w-48 h-1.5 rounded-full bg-surface-3 overflow-hidden">
            <div
              className="h-full bg-accent rounded-full transition-all duration-300"
              style={{ width: `${progress ?? 0}%` }}
            />
          </div>
          <span className="text-text-tertiary text-xs">{progress ?? 0}%</span>
        </>
      )}

      {status === 'ready' && (
        <>
          <RefreshCw className="w-4 h-4 text-green-500 shrink-0" />
          <span className="text-text-primary">{t('update.ready')}</span>
        </>
      )}

      {status === 'error' && (
        <>
          <span className="text-danger text-xs">{t('update.error')}</span>
          <button
            onClick={checkForUpdate}
            className="ml-auto px-3 py-1 rounded-md bg-surface-3 text-text-primary text-xs hover:bg-surface-4 transition-colors"
          >
            {t('update.checkNow')}
          </button>
        </>
      )}

      <button
        onClick={() => setDismissed(true)}
        className="p-1 rounded hover:bg-surface-2 text-text-tertiary hover:text-text-primary transition-colors"
        aria-label={t('update.dismiss')}
      >
        <X className="w-3.5 h-3.5" />
      </button>
    </div>
  );
}
