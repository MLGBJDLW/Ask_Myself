import { useEffect, useId, useRef, useState } from 'react';
import { ChevronRight, FileDiff, Loader2 } from 'lucide-react';
import { AnimatePresence, motion, useReducedMotion } from 'framer-motion';
import { useTranslation } from '../../i18n';
import { getTurnFileDiff, type TurnFileChange, type TurnFileChangeSummary } from '../../lib/api';
import { FileBadge } from '../ui/FileBadge';
import { FileDiffPreview, type FileDiffArtifact } from './FileDiffPreview';

function ChangedFile({ conversationId, turnId, file }: { conversationId: string; turnId: string; file: TurnFileChange }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [detail, setDetail] = useState<FileDiffArtifact | null>(null);
  const [error, setError] = useState<string | null>(null);
  const reduceMotion = useReducedMotion();
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
        <ChevronRight size={13} className={`shrink-0 transition-transform duration-200 motion-reduce:transition-none ${open ? 'rotate-90' : ''}`} />
        <span className="min-w-0 flex-1 truncate">{file.path}</span>
        {file.additions !== null && <span className="shrink-0 font-mono tabular-nums text-emerald-500">+{file.additions}</span>}
        {file.deletions !== null && <span className="shrink-0 font-mono tabular-nums text-rose-400">−{file.deletions}</span>}
        {file.additions === null && <span className="shrink-0 text-text-tertiary">{t('chat.turnChangesNoStats')}</span>}
      </button>
      {file.operation !== 'delete' && <FileBadge path={file.absolutePath} className="max-w-36 shrink-0" />}
    </div>
    <AnimatePresence initial={false}>{open && <motion.div initial={{ height: 0, opacity: 0 }} animate={{ height: 'auto', opacity: 1 }} exit={{ height: 0, opacity: 0 }} transition={{ duration: reduceMotion ? 0 : 0.2 }} className="overflow-hidden px-2" data-testid="turn-file-change-detail">
      {file.contentKind !== 'text' ? <p className="py-2 text-xs text-text-tertiary">{t('chat.turnChangesContentUnavailable')}</p>
        : error ? <p role="alert" className="py-2 text-xs text-error">{t('chat.turnChangesLoadError')} {error}</p>
        : detail ? <FileDiffPreview diff={detail} defaultOpen compact />
        : <Loader2 size={14} className="my-2 animate-spin text-text-tertiary" aria-label={t('common.loading')} />}
    </motion.div>}</AnimatePresence>
  </li>;
}

export function TurnFileChanges({ conversationId, summary, docked = false }: { conversationId: string; summary: TurnFileChangeSummary; docked?: boolean }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [limit, setLimit] = useState(10);
  const [preview, setPreview] = useState(false);
  const reduceMotion = useReducedMotion();
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelId = useId();
  useEffect(() => {
    if (!open && !preview) return;
    const dismiss = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || (!open && !preview)) return;
      event.stopPropagation();
      setOpen(false);
      setPreview(false);
      triggerRef.current?.focus();
    };
    const outside = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) { setOpen(false); setPreview(false); }
    };
    document.addEventListener('keydown', dismiss);
    document.addEventListener('pointerdown', outside);
    return () => { document.removeEventListener('keydown', dismiss); document.removeEventListener('pointerdown', outside); };
  }, [open, preview]);
  if (summary.files.length === 0 && !summary.partial && !summary.pending) return null;
  const transition = { duration: reduceMotion ? 0 : 0.22, ease: [0.22, 1, 0.36, 1] as const };
  return <div ref={rootRef} className={docked ? 'relative min-w-0 max-w-full' : 'relative my-3 min-w-0'} data-testid="turn-file-changes" data-turn-id={summary.turnId}
    onMouseEnter={() => setPreview(true)} onMouseLeave={() => setPreview(false)}
    onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget as Node)) setPreview(false); }}>
    <motion.button ref={triggerRef} type="button" aria-expanded={open} aria-controls={panelId} onFocus={() => setPreview(true)} onClick={() => { setOpen(value => !value); setPreview(false); }}
      whileTap={reduceMotion ? undefined : { scale: 0.97 }}
      className="inline-flex max-w-full items-center gap-2 rounded-full border border-border/70 bg-surface-1 px-3 py-1.5 text-xs text-text-secondary shadow-sm transition-colors hover:border-border-hover hover:bg-surface-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/40">
      {summary.pending ? <Loader2 size={13} className="shrink-0 animate-spin text-text-tertiary" /> : <FileDiff size={13} className="shrink-0 text-text-tertiary" />}
      <span>{summary.files.length ? t('chat.turnChangesTitle', { count: String(summary.files.length) }) : summary.pending ? t('common.loading') : t('chat.turnChangesPartial')}</span>
      {summary.files.length > 0 && <>
        <span className="font-mono tabular-nums text-emerald-500">+{summary.additions}</span>
        <span className="font-mono tabular-nums text-rose-400">−{summary.deletions}</span>
      </>}
      {(summary.partial || summary.unknownFiles > 0) && <span className="text-text-tertiary" title={t('chat.turnChangesCoverage')}>*</span>}
      <ChevronRight size={12} className={`transition-transform duration-200 motion-reduce:transition-none ${open ? (docked ? '-rotate-90' : 'rotate-90') : ''}`} />
    </motion.button>
    <AnimatePresence initial={false}>
    {preview && !open && summary.files.length > 0 && <motion.div key="preview" data-testid="file-changes-hover-preview"
      initial={{ opacity: 0, y: 5, scale: 0.98 }} animate={{ opacity: 1, y: 0, scale: 1 }} exit={{ opacity: 0, y: 3, scale: 0.98 }} transition={transition}
      className={`absolute bottom-full z-30 mb-2 w-80 max-w-[calc(100vw-3rem)] rounded-xl border border-border bg-surface-1 p-2 shadow-lg ${docked ? 'left-1/2 -translate-x-1/2' : 'left-0'}`}>
      {summary.files.slice(0, 6).map(file => <div key={file.absolutePath} className="flex items-center gap-2 px-1 py-1 text-[11px]">
        <FileDiff size={12} className="shrink-0 text-text-tertiary" /><span className="min-w-0 flex-1 truncate text-text-secondary">{file.path}</span>
        {file.additions !== null && <span className="font-mono tabular-nums text-emerald-500">+{file.additions}</span>}
        {file.deletions !== null && <span className="font-mono tabular-nums text-rose-400">−{file.deletions}</span>}
        {file.additions === null && <span className="text-text-tertiary">—</span>}
      </div>)}
      {summary.files.length > 6 && <div className="px-1 pt-1 text-[11px] text-text-tertiary">+{summary.files.length - 6}</div>}
    </motion.div>}
    {open && <motion.div key="panel" id={panelId} data-testid="file-changes-panel"
      initial={{ height: 0, opacity: 0 }} animate={{ height: 'auto', opacity: 1 }} exit={{ height: 0, opacity: 0 }} transition={transition}
      className={`overflow-hidden rounded-xl border border-border bg-surface-1 shadow-lg ${docked ? 'absolute bottom-full left-1/2 z-30 mb-2 w-[min(40rem,calc(100vw-3rem))] -translate-x-1/2' : 'mt-2'}`}>
      <div className="max-h-[min(50vh,440px)] overflow-y-auto overscroll-contain px-2 py-1">
      <ul className="divide-y divide-border/40">{summary.files.slice(0, limit).map(file => <ChangedFile key={file.absolutePath} conversationId={conversationId} turnId={summary.turnId} file={file} />)}</ul>
      {limit < summary.files.length && <button type="button" className="my-1 rounded px-2 py-2 text-xs text-accent hover:bg-surface-2" onClick={() => setLimit(value => value + 20)}>{t('chat.turnChangesShowMore', { count: String(summary.files.length - limit) })}</button>}
      </div>
    </motion.div>}
    </AnimatePresence>
  </div>;
}
