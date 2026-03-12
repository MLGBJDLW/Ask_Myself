import { useState } from 'react';
import {
  Download,
  CheckCircle,
  XCircle,
  Loader2,
  ChevronDown,
  ChevronUp,
} from 'lucide-react';
import { useTranslation } from '../../i18n';
import { Button } from '../ui/Button';
import { Badge } from '../ui/Badge';

type ModelStatus = 'downloaded' | 'downloading' | 'not-downloaded' | 'checking';

interface ModelCardProps {
  title: string;
  icon: React.ReactNode;
  description?: string;
  status: ModelStatus;
  size?: string;
  onDownload: () => void;
  downloadProgress?: {
    filename: string;
    bytesDownloaded: number;
    totalBytes: number | null;
    fileIndex: number;
    totalFiles: number;
  } | null;
  children?: React.ReactNode;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

export function ModelCard({
  title,
  icon,
  description,
  status,
  size,
  onDownload,
  downloadProgress,
  children,
}: ModelCardProps) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const hasChildren = !!children;

  return (
    <div className="rounded-xl border border-border bg-surface-2 transition-colors">
      {/* Header */}
      <div className="flex items-center gap-3 p-4">
        <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-accent/10 text-accent">
          {icon}
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <h3 className="text-sm font-semibold text-text-primary truncate">{title}</h3>
            {status === 'checking' ? (
              <Loader2 size={14} className="animate-spin text-text-tertiary shrink-0" />
            ) : status === 'downloaded' ? (
              <Badge variant="default" className="gap-1 shrink-0">
                <CheckCircle size={10} className="text-success" />
                {t('settings.modelReady')}
              </Badge>
            ) : status === 'downloading' ? (
              <Badge variant="default" className="gap-1 shrink-0 bg-accent/10 text-accent border-accent/20">
                <Loader2 size={10} className="animate-spin" />
                {t('settings.modelDownloading')}
              </Badge>
            ) : (
              <Badge variant="default" className="gap-1 shrink-0">
                <XCircle size={10} className="text-text-tertiary" />
                {t('settings.modelNotDownloaded')}
              </Badge>
            )}
          </div>
          <div className="flex items-center gap-2 mt-0.5">
            {description && (
              <p className="text-xs text-text-tertiary truncate">{description}</p>
            )}
            {size && (
              <span className="text-[10px] text-text-tertiary/70 shrink-0">{size}</span>
            )}
          </div>
        </div>

        <div className="flex items-center gap-1.5 shrink-0">
          {status === 'not-downloaded' && (
            <Button
              variant="secondary"
              size="sm"
              icon={<Download size={14} />}
              onClick={onDownload}
            >
              {t('settings.embeddingDownload')}
            </Button>
          )}
          {hasChildren && (
            <button
              onClick={() => setExpanded(!expanded)}
              className="rounded-md p-1.5 text-text-tertiary hover:text-text-secondary hover:bg-surface-3 transition-colors cursor-pointer"
              aria-label={expanded ? 'Collapse' : 'Expand'}
            >
              {expanded ? <ChevronUp size={16} /> : <ChevronDown size={16} />}
            </button>
          )}
        </div>
      </div>

      {/* Download progress */}
      {status === 'downloading' && downloadProgress && (
        <div className="px-4 pb-3">
          <div className="flex items-center gap-2 text-xs text-text-tertiary mb-1">
            <Loader2 size={12} className="animate-spin" />
            <span>
              {t('settings.downloadingFile', {
                filename: downloadProgress.filename,
                current: String(downloadProgress.fileIndex + 1),
                total: String(downloadProgress.totalFiles),
              })}
            </span>
          </div>
          {downloadProgress.totalBytes ? (
            <>
              <div className="flex justify-between text-[10px] text-text-tertiary/70 mb-0.5">
                <span>{formatBytes(downloadProgress.bytesDownloaded)} / {formatBytes(downloadProgress.totalBytes)}</span>
                <span>{Math.round((downloadProgress.bytesDownloaded / downloadProgress.totalBytes) * 100)}%</span>
              </div>
              <div className="w-full bg-surface-3 rounded h-1.5">
                <div
                  className="bg-accent h-1.5 rounded transition-all duration-300"
                  style={{ width: `${Math.min(100, (downloadProgress.bytesDownloaded / downloadProgress.totalBytes) * 100)}%` }}
                />
              </div>
            </>
          ) : (
            <div className="w-full bg-surface-3 rounded h-1.5 overflow-hidden">
              <div className="bg-accent h-1.5 rounded animate-pulse w-full" />
            </div>
          )}
        </div>
      )}

      {/* Expandable children */}
      {hasChildren && expanded && (
        <div className="border-t border-border px-4 py-4">
          {children}
        </div>
      )}
    </div>
  );
}
