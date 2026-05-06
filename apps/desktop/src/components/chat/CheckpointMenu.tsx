import { useState, useCallback, useRef, useEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { Bookmark, GitBranch, RotateCcw, Trash2 } from 'lucide-react';
import { toast } from 'sonner';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import { appTimeMs, parseAppDate } from '../../lib/dateTime';
import type { Checkpoint, Conversation } from '../../types/conversation';

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

interface CheckpointMenuProps {
  conversationId: string;
  /** Called after a checkpoint is successfully restored */
  onRestore: () => void;
  onBranch?: (conversation: Conversation) => void;
}

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

function formatDate(iso: string): string {
  try {
    const d = parseAppDate(iso);
    if (Number.isNaN(d.getTime())) return iso;
    return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
      + ' ' + d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  } catch {
    return iso;
  }
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function CheckpointMenu({ conversationId, onRestore, onBranch }: CheckpointMenuProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [checkpoints, setCheckpoints] = useState<Checkpoint[]>([]);
  const [loading, setLoading] = useState(false);
  const [confirmingId, setConfirmingId] = useState<string | null>(null);
  const [branchingId, setBranchingId] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const firstRestoreButtonRef = useRef<HTMLButtonElement>(null);

  /* ── Load checkpoints when dropdown opens ───────────────────────── */
  const load = useCallback(async () => {
    setLoading(true);
    try {
      const list = await api.listCheckpoints(conversationId);
      list.sort((a, b) => appTimeMs(b.createdAt) - appTimeMs(a.createdAt));
      setCheckpoints(list);
    } catch {
      setCheckpoints([]);
    } finally {
      setLoading(false);
    }
  }, [conversationId]);

  useEffect(() => {
    if (open) {
      load();
      setConfirmingId(null);
    }
  }, [open, load]);

  const closeMenu = useCallback((restoreFocus = true) => {
    setOpen(false);
    if (restoreFocus) {
      requestAnimationFrame(() => triggerRef.current?.focus());
    }
  }, []);

  /* ── Close on outside click ─────────────────────────────────────── */
  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        closeMenu(false);
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [closeMenu, open]);

  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        closeMenu();
      }
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [closeMenu, open]);

  useEffect(() => {
    if (!open || loading) return;
    const frame = requestAnimationFrame(() => {
      firstRestoreButtonRef.current?.focus() ?? panelRef.current?.focus();
    });
    return () => cancelAnimationFrame(frame);
  }, [checkpoints.length, loading, open]);

  /* ── Restore a checkpoint ───────────────────────────────────────── */
  const handleRestore = useCallback(async (checkpointId: string) => {
    try {
      await api.restoreCheckpoint(checkpointId);
      toast.success(t('chat.checkpointRestored'));
      closeMenu();
      onRestore();
    } catch (e) {
      toast.error(String(e));
    }
  }, [closeMenu, onRestore, t]);

  /* ── Delete a checkpoint ────────────────────────────────────────── */
  const handleDelete = useCallback(async (checkpointId: string) => {
    try {
      await api.deleteCheckpoint(checkpointId);
      setCheckpoints(prev => prev.filter(cp => cp.id !== checkpointId));
    } catch (e) {
      toast.error(String(e));
    }
  }, []);

  const handleBranch = useCallback(async (checkpointId: string) => {
    if (!onBranch) return;
    setBranchingId(checkpointId);
    try {
      const result = await api.branchCheckpoint(checkpointId);
      toast.success(t('chat.checkpointBranched'));
      closeMenu(false);
      onBranch(result.conversation);
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBranchingId(null);
    }
  }, [closeMenu, onBranch, t]);

  /* ── Render ─────────────────────────────────────────────────────── */
  return (
    <div ref={ref} className="relative inline-block">
      {/* Trigger button */}
      <button
        ref={triggerRef}
        type="button"
        data-testid="checkpoint-menu-trigger"
        onClick={() => setOpen(v => !v)}
        aria-expanded={open}
        aria-haspopup="dialog"
        className="
          p-0.5 rounded hover:bg-surface-3 text-muted/60 hover:text-text-secondary
          transition-colors cursor-pointer
        "
        title={t('chat.checkpoints')}
      >
        <Bookmark size={12} />
      </button>

      {/* Dropdown (opens upward since button is near bottom) */}
      <AnimatePresence>
        {open && (
          <motion.div
            ref={panelRef}
            data-testid="checkpoint-menu-panel"
            initial={{ opacity: 0, y: 4 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: 4 }}
            transition={{ duration: 0.15, ease: [0.16, 1, 0.3, 1] }}
            role="dialog"
            aria-modal="false"
            aria-label={t('chat.checkpoints')}
            tabIndex={-1}
            className="
              absolute right-0 bottom-full mb-1 z-50
              w-80 max-h-72 overflow-y-auto
              bg-surface-1 border border-border rounded-lg shadow-lg
            "
          >
            {/* Header */}
            <div className="px-3 py-2 border-b border-border">
              <p className="text-xs font-medium text-text-secondary">
                {t('chat.checkpoints')}
              </p>
            </div>

            {/* Content */}
            {loading ? (
              <div className="px-3 py-4 text-xs text-text-tertiary text-center">
                {t('common.loading')}
              </div>
            ) : checkpoints.length === 0 ? (
              <div className="px-3 py-4 text-xs text-text-tertiary text-center">
                {t('chat.noCheckpoints')}
              </div>
            ) : (
              <ul className="py-1">
                {checkpoints.map(cp => (
                  <li key={cp.id} className="px-3 py-2 hover:bg-surface-2 transition-colors">
                    {/* Checkpoint info */}
                    <div className="flex items-start justify-between gap-2">
                      <div className="min-w-0 flex-1">
                        <p className="text-xs text-text-primary truncate" title={cp.label}>
                          {cp.label}
                        </p>
                        <p className="text-[10px] text-text-tertiary mt-0.5">
                          {t('chat.messagesShort', { count: cp.messageCount })} · {formatDate(cp.createdAt)}
                        </p>
                      </div>
                      <div className="flex items-center gap-1 shrink-0">
                        {onBranch && (
                          <button
                            type="button"
                            onClick={() => handleBranch(cp.id)}
                            disabled={branchingId === cp.id}
                            className="p-1 rounded text-text-tertiary hover:text-accent hover:bg-accent/10 disabled:pointer-events-none disabled:opacity-50 transition-colors cursor-pointer"
                            title={t('chat.branchCheckpoint')}
                            aria-label={t('chat.branchCheckpoint')}
                          >
                            <GitBranch size={12} />
                          </button>
                        )}
                        <button
                          ref={cp.id === checkpoints[0]?.id ? firstRestoreButtonRef : undefined}
                          type="button"
                          onClick={() => setConfirmingId(confirmingId === cp.id ? null : cp.id)}
                          className="p-1 rounded text-text-tertiary hover:text-accent hover:bg-accent/10 transition-colors cursor-pointer"
                          title={t('chat.restoreCheckpoint')}
                        >
                          <RotateCcw size={12} />
                        </button>
                        <button
                          type="button"
                          onClick={() => handleDelete(cp.id)}
                          className="p-1 rounded text-text-tertiary hover:text-danger hover:bg-danger/10 transition-colors cursor-pointer"
                          title={t('chat.deleteCheckpoint')}
                        >
                          <Trash2 size={12} />
                        </button>
                      </div>
                    </div>

                    {/* Confirm restore */}
                    {confirmingId === cp.id && (
                      <div className="mt-1.5 p-2 bg-surface-2 rounded-md">
                        <p className="text-[10px] text-text-secondary mb-1.5">
                          {t('chat.confirmRestore')}
                        </p>
                        <div className="flex gap-1.5">
                          <button
                            type="button"
                            onClick={() => handleRestore(cp.id)}
                            className="px-2 py-0.5 rounded text-[10px] font-medium bg-accent text-white hover:bg-accent-hover transition-colors cursor-pointer"
                          >
                            {t('chat.restoreCheckpoint')}
                          </button>
                          <button
                            type="button"
                            onClick={() => setConfirmingId(null)}
                            className="px-2 py-0.5 rounded text-[10px] font-medium bg-surface-3 text-text-secondary hover:bg-surface-3/80 transition-colors cursor-pointer"
                          >
                            {t('chat.cancel')}
                          </button>
                        </div>
                      </div>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
