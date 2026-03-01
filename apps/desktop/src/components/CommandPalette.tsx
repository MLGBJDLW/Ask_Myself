import { useEffect, useState } from 'react';
import { Command } from 'cmdk';
import { useNavigate } from 'react-router-dom';
import { motion, AnimatePresence } from 'framer-motion';
import { Search, FolderOpen, BookOpen, MessageCircle, Settings, ScanSearch, Database, Clock } from 'lucide-react';
import { toast } from 'sonner';
import * as api from '../lib/api';
import type { QueryLog } from '../types';
import { useTranslation } from '../i18n';

export function CommandPalette() {
  const [open, setOpen] = useState(false);
  const [recentQueries, setRecentQueries] = useState<QueryLog[]>([]);
  const navigate = useNavigate();
  const { t } = useTranslation();

  /* ── Ctrl/Cmd+K toggle ───────────────────────────────────────────── */
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'k' && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        setOpen((prev) => !prev);
      }
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, []);

  /* ── Escape to close ─────────────────────────────────────────────── */
  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        setOpen(false);
      }
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [open]);

  /* ── Load recent queries on open ─────────────────────────────────── */
  useEffect(() => {
    if (!open) return;
    // Recent queries are a non-critical UX hint; log but don't disrupt the palette
    api.getRecentQueries(5).then(setRecentQueries).catch((e) => {
      console.error('Failed to load recent queries:', e);
    });
  }, [open]);

  /* ── Helpers ─────────────────────────────────────────────────────── */
  const select = (fn: () => void) => {
    setOpen(false);
    fn();
  };

  const handleScanAll = () => {
    select(() => {
      toast.promise(api.scanAllSources(), {
        loading: t('cmd.scanningAll'),
        success: t('cmd.scanComplete'),
        error: t('cmd.scanError'),
      });
    });
  };

  const handleRebuildEmbeddings = () => {
    select(() => {
      toast.promise(api.rebuildEmbeddings(), {
        loading: t('cmd.rebuildingEmbeddings'),
        success: t('cmd.rebuildComplete'),
        error: t('cmd.rebuildError'),
      });
    });
  };

  /* ── Render ──────────────────────────────────────────────────────── */
  return (
    <AnimatePresence>
      {open && (
        <div className="fixed inset-0 z-50">
          {/* Backdrop */}
          <motion.div
            className="absolute inset-0 bg-black/60 backdrop-blur-sm"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.15 }}
            onClick={() => setOpen(false)}
          />

          {/* Dialog */}
          <motion.div
            className="absolute left-1/2 top-[20%] w-full max-w-lg -translate-x-1/2 px-4"
            initial={{ opacity: 0, scale: 0.96, y: -8 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.96, y: -8 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
          >
            <Command
              className="overflow-hidden rounded-xl border border-border bg-surface-1 shadow-lg"
              loop
            >
              <Command.Input
                placeholder={t('cmd.placeholder')}
                className="w-full border-b border-border bg-transparent px-4 py-3 text-sm
                  text-text-primary placeholder:text-text-tertiary outline-none"
                autoFocus
              />

              <Command.List className="max-h-72 overflow-y-auto p-2">
                <Command.Empty className="px-4 py-8 text-center text-sm text-text-tertiary">
                  {t('cmd.noResults')}
                </Command.Empty>

                {/* Navigation */}
                <Command.Group heading={t('cmd.navigation')}>
                  <Command.Item onSelect={() => select(() => navigate('/'))}>
                    <Search className="h-4 w-4 shrink-0 text-text-tertiary" />
                    {t('nav.search')}
                  </Command.Item>
                  <Command.Item onSelect={() => select(() => navigate('/sources'))}>
                    <FolderOpen className="h-4 w-4 shrink-0 text-text-tertiary" />
                    {t('nav.sources')}
                  </Command.Item>
                  <Command.Item onSelect={() => select(() => navigate('/playbooks'))}>
                    <BookOpen className="h-4 w-4 shrink-0 text-text-tertiary" />
                    {t('nav.playbooks')}
                  </Command.Item>
                  <Command.Item onSelect={() => select(() => navigate('/chat'))}>
                    <MessageCircle className="h-4 w-4 shrink-0 text-text-tertiary" />
                    {t('nav.chat')}
                  </Command.Item>
                  <Command.Item onSelect={() => select(() => navigate('/settings'))}>
                    <Settings className="h-4 w-4 shrink-0 text-text-tertiary" />
                    {t('nav.settings')}
                  </Command.Item>
                </Command.Group>

                <Command.Separator className="mx-2 my-1 h-px bg-border" />

                {/* Actions */}
                <Command.Group heading={t('cmd.actions')}>
                  <Command.Item onSelect={handleScanAll}>
                    <ScanSearch className="h-4 w-4 shrink-0 text-text-tertiary" />
                    {t('cmd.scanAll')}
                  </Command.Item>
                  <Command.Item onSelect={handleRebuildEmbeddings}>
                    <Database className="h-4 w-4 shrink-0 text-text-tertiary" />
                    {t('cmd.rebuildEmbeddings')}
                  </Command.Item>
                </Command.Group>

                {/* Recent queries */}
                {recentQueries.length > 0 && (
                  <>
                    <Command.Separator className="mx-2 my-1 h-px bg-border" />
                    <Command.Group heading={t('cmd.recentQueries')}>
                      {recentQueries.map((q) => (
                        <Command.Item
                          key={q.id}
                          value={q.queryText}
                          onSelect={() => select(() => navigate('/'))}
                        >
                          <Clock className="h-4 w-4 shrink-0 text-text-tertiary" />
                          <span className="truncate">{q.queryText}</span>
                        </Command.Item>
                      ))}
                    </Command.Group>
                  </>
                )}
              </Command.List>
            </Command>
          </motion.div>
        </div>
      )}
    </AnimatePresence>
  );
}
