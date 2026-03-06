import { useState, useEffect, useCallback, useRef } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { Database, FolderOpen, ChevronDown, Check } from 'lucide-react';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import type { Source } from '../../types';

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

interface SourceSelectorProps {
  conversationId: string;
  onUpdate?: () => void;
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function SourceSelector({ conversationId, onUpdate }: SourceSelectorProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [sources, setSources] = useState<Source[]>([]);
  const [linkedIds, setLinkedIds] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(true);
  const ref = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const firstSourceButtonRef = useRef<HTMLButtonElement>(null);

  /* ── Load sources + linked ids ──────────────────────────────────── */
  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [allSources, linked] = await Promise.all([
        api.listSources(),
        api.getConversationSources(conversationId),
      ]);
      setSources(allSources);
      setLinkedIds(new Set(linked));
    } catch {
      // Silently fail - sources list may just be empty
    } finally {
      setLoading(false);
    }
  }, [conversationId]);

  useEffect(() => {
    load();
  }, [load]);

  const closeSelector = useCallback((restoreFocus = true) => {
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
        closeSelector(false);
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [closeSelector, open]);

  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        closeSelector();
      }
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [closeSelector, open]);

  useEffect(() => {
    if (!open || loading) return;
    const frame = requestAnimationFrame(() => {
      firstSourceButtonRef.current?.focus() ?? panelRef.current?.focus();
    });
    return () => cancelAnimationFrame(frame);
  }, [loading, open, sources.length]);

  /* ── Toggle a source ────────────────────────────────────────────── */
  const toggle = useCallback(
    async (sourceId: string) => {
      const next = new Set(linkedIds);
      if (next.has(sourceId)) {
        next.delete(sourceId);
      } else {
        next.add(sourceId);
      }
      setLinkedIds(next);
      try {
        await api.setConversationSources(conversationId, Array.from(next));
        onUpdate?.();
      } catch {
        // Revert on error
        setLinkedIds(linkedIds);
      }
    },
    [conversationId, linkedIds, onUpdate],
  );

  /* ── Derive label ───────────────────────────────────────────────── */
  const selectedCount = linkedIds.size;
  const label =
    selectedCount === 0
      ? t('chat.allSources')
      : `${selectedCount} / ${sources.length}`;

  /* ── Render ─────────────────────────────────────────────────────── */
  return (
    <div ref={ref} className="relative inline-block">
      {/* Trigger button */}
      <button
        ref={triggerRef}
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        aria-haspopup="dialog"
        className="
          inline-flex items-center gap-1.5 px-2.5 py-1 rounded-md text-xs
          text-text-secondary hover:text-text-primary
          bg-surface-2 hover:bg-surface-3 border border-border
          transition-colors duration-fast
        "
        title={t('chat.selectSources')}
      >
        <Database size={13} />
        <span>{t('chat.knowledgeSources')}</span>
        <span className="text-text-tertiary">/</span>
        <span className={selectedCount === 0 ? 'text-text-tertiary' : 'text-accent'}>
          {label}
        </span>
        <ChevronDown
          size={12}
          className={`transition-transform duration-fast ${open ? 'rotate-180' : ''}`}
        />
      </button>

      {/* Dropdown */}
      <AnimatePresence>
        {open && (
          <motion.div
            ref={panelRef}
            initial={{ opacity: 0, y: -4 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -4 }}
            transition={{ duration: 0.15, ease: [0.16, 1, 0.3, 1] }}
            role="dialog"
            aria-modal="false"
            aria-label={t('chat.selectSources')}
            tabIndex={-1}
            className="
              absolute left-0 top-full mt-1 z-50
              w-72 max-h-64 overflow-y-auto
              bg-surface-1 border border-border rounded-lg shadow-lg
            "
          >
            {/* Header */}
            <div className="px-3 py-2 border-b border-border">
              <p className="text-xs text-text-tertiary">
                {t('chat.selectSources')}
              </p>
            </div>

            {/* Source list */}
            {loading ? (
              <div className="px-3 py-4 text-xs text-text-tertiary text-center">
                {t('common.loading')}
              </div>
            ) : sources.length === 0 ? (
              <div className="px-3 py-4 text-xs text-text-tertiary text-center">
                {t('sources.emptyTitle')}
              </div>
            ) : (
              <ul className="py-1">
                {sources.map((source) => {
                  const checked = linkedIds.has(source.id);
                  return (
                    <li key={source.id}>
                      <button
                        ref={source.id === sources[0]?.id ? firstSourceButtonRef : undefined}
                        type="button"
                        onClick={() => toggle(source.id)}
                        aria-pressed={checked}
                        className="
                          w-full flex items-center gap-2 px-3 py-1.5 text-left text-xs
                          hover:bg-surface-2 transition-colors duration-fast
                        "
                      >
                        {/* Checkbox */}
                        <span
                          className={`
                            flex shrink-0 items-center justify-center w-4 h-4 rounded border
                            transition-colors duration-fast
                            ${checked
                              ? 'bg-accent border-accent text-white'
                              : 'border-border bg-surface-2'
                            }
                          `}
                        >
                          {checked && <Check size={10} />}
                        </span>

                        {/* Icon */}
                        <FolderOpen size={14} className="shrink-0 text-text-tertiary" />

                        {/* Path */}
                        <span className="truncate text-text-primary" title={source.rootPath}>
                          {source.rootPath}
                        </span>
                      </button>
                    </li>
                  );
                })}
              </ul>
            )}

            {/* Footer: "all sources" hint */}
            {!loading && sources.length > 0 && (
              <div className="px-3 py-1.5 border-t border-border">
                <p className="text-[10px] text-text-tertiary">
                  {selectedCount === 0
                    ? `✓ ${t('chat.allSources')}`
                    : `${selectedCount} / ${sources.length}`}
                </p>
              </div>
            )}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

