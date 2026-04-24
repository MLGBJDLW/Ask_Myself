import { X, Download, RefreshCw } from 'lucide-react';
import { useTranslation } from '../i18n';
import { useState } from 'react';
import type { useUpdater } from '../lib/useUpdater';

interface UpdateNotificationProps {
  updater: ReturnType<typeof useUpdater>;
}

export function UpdateNotification({ updater }: UpdateNotificationProps) {
  const { t } = useTranslation();
  const { status, version, progress, downloadAndInstall, checkForUpdate, error, errorCode, errorDetail, errorStage } = updater;
  const [dismissed, setDismissed] = useState(false);
  const [detailsOpen, setDetailsOpen] = useState(false);

  if (dismissed || status === 'idle' || status === 'checking' || status === 'up-to-date') {
    return null;
  }

  const errText = error ?? '';
  const isNotFound = /404|not found/i.test(errText);
  const isSignatureErr = /signature|\bsig\b/i.test(errText);
  const isNetworkErr = /error sending request|timed?\s*out|timeout|failed to connect|network|dns|connection|githubusercontent\.com/i.test(errText);
  const hint = isNetworkErr
    ? '可能原因：GitHub Release CDN 无法访问或响应过慢，可稍后重试，或切换到可访问 GitHub Release 资产的网络/代理。'
    : isNotFound
      ? '可能原因：当前版本的 release 资产缺失，或 updater manifest 中的下载 URL 与实际资产名不匹配。'
      : isSignatureErr
        ? '签名验证失败，可能是 GitHub Release 资产不完整。'
        : errorStage === 'download' || errorStage === 'install'
          ? '可能原因：更新包下载或安装过程被中断。'
          : null;

  const errorLabel =
    errorStage === 'download'
      ? t('update.downloadFailed')
      : errorStage === 'install'
        ? t('update.installFailed')
        : t('update.error');

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
          <div className="flex flex-col gap-1 flex-1 min-w-0">
            <div className="flex items-center gap-2">
              <span className="text-danger text-xs font-medium">{errorLabel}</span>
              {error && <span className="text-text-secondary text-xs truncate">{error}</span>}
            </div>
            {hint && <span className="text-text-tertiary text-xs">{hint}</span>}
            {(errorCode != null || errorDetail?.stack) && (
              <div>
                <button
                  type="button"
                  onClick={() => setDetailsOpen(v => !v)}
                  className="text-text-tertiary text-xs hover:text-text-primary transition-colors"
                >
                  {detailsOpen ? '▼' : '▶'} 详细信息
                </button>
                {detailsOpen && (
                  <pre className="mt-1 p-2 rounded bg-surface-2 text-text-tertiary text-xs whitespace-pre-wrap break-all max-h-48 overflow-auto">
                    {errorCode != null && `code: ${errorCode}\n`}
                    {errorDetail?.stack ?? ''}
                  </pre>
                )}
              </div>
            )}
          </div>
          <button
            onClick={checkForUpdate}
            className="ml-auto px-3 py-1 rounded-md bg-surface-3 text-text-primary text-xs hover:bg-surface-4 transition-colors shrink-0"
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
