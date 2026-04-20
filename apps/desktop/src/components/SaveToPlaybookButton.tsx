import { useState, useRef, useEffect, useCallback } from 'react';
import { BookmarkPlus, Plus, Check, Loader2 } from 'lucide-react';
import { toast } from 'sonner';
import * as api from '../lib/api';
import type { Playbook } from '../types';
import { useTranslation } from '../i18n';

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

interface SaveToPlaybookButtonProps {
  chunkId: string;
  /** Visual size variant */
  size?: 'sm' | 'md';
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function SaveToPlaybookButton({ chunkId, size = 'sm' }: SaveToPlaybookButtonProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [playbooks, setPlaybooks] = useState<Playbook[]>([]);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [showCreate, setShowCreate] = useState(false);
  const [newTitle, setNewTitle] = useState('');
  const [creating, setCreating] = useState(false);
  const popoverRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);

  // Fetch playbooks when popover opens
  const handleOpen = useCallback(async () => {
    if (open) {
      setOpen(false);
      return;
    }
    setOpen(true);
    setLoading(true);
    setSaved(false);
    setShowCreate(false);
    setNewTitle('');
    try {
      const pbs = await api.listPlaybooks();
      setPlaybooks(pbs);
    } catch {
      setPlaybooks([]);
    } finally {
      setLoading(false);
    }
  }, [open]);

  // Close on click outside
  useEffect(() => {
    if (!open) return;
    function handleClickOutside(e: MouseEvent) {
      if (
        popoverRef.current &&
        !popoverRef.current.contains(e.target as Node) &&
        buttonRef.current &&
        !buttonRef.current.contains(e.target as Node)
      ) {
        setOpen(false);
      }
    }
    document.addEventListener('mousedown', handleClickOutside);
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, [open]);

  // Close on Escape
  useEffect(() => {
    if (!open) return;
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') setOpen(false);
    }
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [open]);

  const handleSave = useCallback(async (playbookId: string) => {
    setSaving(playbookId);
    try {
      const existingCitations = await api.listCitations(playbookId).catch(() => []);
      await api.addCitation(playbookId, chunkId, '', existingCitations.length);
      setSaved(true);
      toast.success(t('search.savedToPlaybook'));
      setTimeout(() => setOpen(false), 600);
    } catch (e) {
      toast.error(`${t('search.saveError')}: ${String(e)}`);
    } finally {
      setSaving(null);
    }
  }, [chunkId, t]);

  const handleCreate = useCallback(async () => {
    const title = newTitle.trim();
    if (!title) return;
    setCreating(true);
    try {
      const pb = await api.createPlaybook(title, '', '');
      await api.addCitation(pb.id, chunkId, '', 0);
      setSaved(true);
      toast.success(t('search.savedToPlaybook'));
      setTimeout(() => setOpen(false), 600);
    } catch (e) {
      toast.error(`${t('search.saveError')}: ${String(e)}`);
    } finally {
      setCreating(false);
    }
  }, [chunkId, newTitle, t]);

  const iconSize = size === 'sm' ? 12 : 14;
  const btnClass = size === 'sm'
    ? 'inline-flex items-center gap-1 px-2 py-1 text-[10px] font-medium rounded-md bg-surface-3 text-text-tertiary hover:text-text-primary hover:bg-surface-4 transition-colors cursor-pointer'
    : 'inline-flex items-center gap-1 rounded-md px-2 py-1 text-[11px] text-text-tertiary transition-colors hover:bg-surface-2 hover:text-text-secondary cursor-pointer';

  return (
    <div className="relative inline-block">
      <button
        ref={buttonRef}
        type="button"
        onClick={handleOpen}
        className={saved ? btnClass.replace('text-text-tertiary', 'text-green-500') : btnClass}
        title={t('search.saveToPlaybook')}
      >
        {saved ? <Check size={iconSize} /> : <BookmarkPlus size={iconSize} />}
        {size === 'md' && (
          <span>{saved ? t('search.savedToPlaybook') : t('search.saveToPlaybook')}</span>
        )}
      </button>

      {open && (
        <div
          ref={popoverRef}
          className="absolute right-0 bottom-full mb-1 z-50 w-56 rounded-lg border border-border bg-surface-1 shadow-xl overflow-hidden"
          role="listbox"
        >
          {/* Header */}
          <div className="px-3 py-2 border-b border-border bg-surface-2">
            <span className="text-[11px] font-medium text-text-primary">
              {t('search.selectPlaybook')}
            </span>
          </div>

          {/* Content */}
          {loading ? (
            <div className="flex items-center justify-center py-4">
              <Loader2 size={16} className="animate-spin text-text-tertiary" />
            </div>
          ) : (
            <div className="max-h-[200px] overflow-y-auto">
              {playbooks.length > 0 ? (
                playbooks.map((pb) => (
                  <button
                    key={pb.id}
                    type="button"
                    onClick={() => handleSave(pb.id)}
                    disabled={saving !== null}
                    className="flex w-full items-center gap-2 px-3 py-2 text-left text-xs text-text-secondary hover:bg-surface-2 transition-colors disabled:opacity-50 cursor-pointer"
                  >
                    {saving === pb.id ? (
                      <Loader2 size={12} className="shrink-0 animate-spin" />
                    ) : (
                      <BookmarkPlus size={12} className="shrink-0 text-text-tertiary" />
                    )}
                    <span className="truncate">{pb.title}</span>
                  </button>
                ))
              ) : !showCreate ? (
                <div className="px-3 py-3 text-[11px] text-text-tertiary text-center">
                  {t('search.noPlaybooks')}
                </div>
              ) : null}

              {/* Create new */}
              {!showCreate ? (
                <button
                  type="button"
                  onClick={() => setShowCreate(true)}
                  className="flex w-full items-center gap-2 px-3 py-2 text-left text-xs text-accent hover:bg-accent/10 transition-colors border-t border-border cursor-pointer"
                >
                  <Plus size={12} className="shrink-0" />
                  <span>{t('search.createNewPlaybook')}</span>
                </button>
              ) : (
                <div className="px-3 py-2 border-t border-border space-y-2">
                  <input
                    type="text"
                    value={newTitle}
                    onChange={(e) => setNewTitle(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') handleCreate();
                    }}
                    placeholder={t('search.newPlaybookName')}
                    autoFocus
                    className="w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-xs text-text-primary placeholder:text-text-tertiary outline-none focus:border-accent"
                  />
                  <button
                    type="button"
                    onClick={handleCreate}
                    disabled={!newTitle.trim() || creating}
                    className="flex w-full items-center justify-center gap-1 rounded-md bg-accent px-2 py-1.5 text-xs font-medium text-white hover:bg-accent-hover transition-colors disabled:opacity-50 cursor-pointer"
                  >
                    {creating ? (
                      <Loader2 size={12} className="animate-spin" />
                    ) : (
                      <Plus size={12} />
                    )}
                    <span>{t('common.save')}</span>
                  </button>
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
