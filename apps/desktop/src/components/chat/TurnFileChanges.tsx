import { useEffect, useState } from 'react';
import { ChevronDown, ChevronRight, FileDiff, Loader2 } from 'lucide-react';
import { useTranslation } from '../../i18n';
import { getTurnFileDiff, type TurnFileChange, type TurnFileChangeSummary } from '../../lib/api';
import { FileBadge } from '../ui/FileBadge';
import { FileDiffPreview, type FileDiffArtifact } from './FileDiffPreview';

function ChangedFile({ conversationId, turnId, file }: { conversationId: string; turnId: string; file: TurnFileChange }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [detail, setDetail] = useState<FileDiffArtifact | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!open || file.contentKind !== 'text') return;
    let disposed = false;
    setDetail(null);
    setError(null);
    void getTurnFileDiff(conversationId, turnId, file.absolutePath).then(value => {
      if (!disposed) setDetail(value);
    }).catch(cause => { if (!disposed) setError(String(cause)); });
    return () => { disposed = true; };
  }, [conversationId, turnId, file.absolutePath, file.revision, file.contentKind, open]);
  return <li className="py-1">
    <div className="flex min-w-0 items-center gap-2">
      <button type="button" aria-expanded={open} onClick={() => setOpen(value => !value)} className="flex min-w-0 flex-1 items-center gap-2 rounded px-2 py-2 text-left text-xs text-text-secondary hover:bg-surface-2" title={file.absolutePath}>
        {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        <span className="min-w-0 flex-1 truncate">{file.path}</span>
        {file.additions !== null && <span className="shrink-0 font-mono tabular-nums text-emerald-500">+{file.additions}</span>}
        {file.deletions !== null && <span className="shrink-0 font-mono tabular-nums text-rose-400">−{file.deletions}</span>}
        {file.additions === null && <span className="shrink-0 text-text-tertiary">{t('chat.turnChangesNoStats')}</span>}
      </button>
      {file.operation !== 'delete' && <FileBadge path={file.absolutePath} className="max-w-36 shrink-0" />}
    </div>
    {open && <div className="mb-2 px-2" data-testid="turn-file-change-detail">
      {file.contentKind !== 'text' ? <p className="py-2 text-xs text-text-tertiary">{t('chat.turnChangesContentUnavailable')}</p>
        : error ? <p role="alert" className="py-2 text-xs text-error">{t('chat.turnChangesLoadError')} {error}</p>
        : detail ? <FileDiffPreview diff={detail} defaultOpen compact />
        : <Loader2 size={14} className="my-2 animate-spin text-text-tertiary" aria-label={t('common.loading')} />}
    </div>}
  </li>;
}

export function TurnFileChanges({ conversationId, summary }: { conversationId: string; summary: TurnFileChangeSummary }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [limit, setLimit] = useState(10);
  if (summary.files.length === 0 && !summary.partial) return null;
  return <div className="my-3 min-w-0" data-testid="turn-file-changes" data-turn-id={summary.turnId}>
    <button type="button" aria-expanded={open} onClick={() => setOpen(value => !value)} className="inline-flex max-w-full items-center gap-2 rounded-full border border-border/70 bg-surface-1/70 px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-border-hover hover:bg-surface-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/40">
      <FileDiff size={13} className="shrink-0 text-text-tertiary" />
      <span>{summary.files.length ? t('chat.turnChangesTitle', { count: String(summary.files.length) }) : t('chat.turnChangesPartial')}</span>
      {summary.files.length > 0 && <>
        <span className="font-mono tabular-nums text-emerald-500">+{summary.additions}</span>
        <span className="font-mono tabular-nums text-rose-400">−{summary.deletions}</span>
      </>}
      {(summary.partial || summary.unknownFiles > 0) && <span className="text-text-tertiary" title={t('chat.turnChangesCoverage')}>*</span>}
      {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
    </button>
    {open && <div className="mt-2 overflow-hidden rounded-xl border border-border/70 bg-surface-0/60 px-2 py-1">
      <p className="px-2 py-2 text-[11px] leading-relaxed text-text-tertiary">{t('chat.turnChangesBaseline')}</p>
      {(summary.partial || summary.unknownFiles > 0) && <p className="px-2 pb-2 text-[11px] leading-relaxed text-text-tertiary">{t('chat.turnChangesCoverage')}</p>}
      <ul className="max-h-[440px] divide-y divide-border/40 overflow-y-auto">{summary.files.slice(0, limit).map(file => <ChangedFile key={file.absolutePath} conversationId={conversationId} turnId={summary.turnId} file={file} />)}</ul>
      {limit < summary.files.length && <button type="button" className="my-1 rounded px-2 py-2 text-xs text-accent hover:bg-surface-2" onClick={() => setLimit(value => value + 20)}>{t('chat.turnChangesShowMore', { count: String(summary.files.length - limit) })}</button>}
    </div>}
  </div>;
}
