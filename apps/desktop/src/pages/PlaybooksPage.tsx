import { useState, useEffect, useCallback } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { BookOpen, Plus, Trash2, X, Pencil, FileText, Calendar, ChevronUp, ChevronDown, Check, BotMessageSquare } from 'lucide-react';
import { toast } from 'sonner';
import * as api from '../lib/api';
import type { Playbook, PlaybookCitation } from '../types';
import { useTranslation } from '../i18n';
import { Button } from '../components/ui/Button';
import { Input } from '../components/ui/Input';
import { Badge } from '../components/ui/Badge';
import { Modal } from '../components/ui/Modal';
import { Skeleton, CardSkeleton } from '../components/ui/Skeleton';
import { EmptyState } from '../components/ui/EmptyState';
import { ConfirmDialog } from '../components/ui/ConfirmDialog';
import { ChatPanel } from '../components/chat/ChatPanel';

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

function formatDate(iso: string, locale: string): string {
  return new Date(iso).toLocaleDateString(locale, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  });
}

const listItemVariants = {
  hidden: { opacity: 0, x: -12 },
  visible: (i: number) => ({
    opacity: 1,
    x: 0,
    transition: { delay: i * 0.04, duration: 0.25, ease: [0.16, 1, 0.3, 1] as const },
  }),
  exit: { opacity: 0, x: -12, transition: { duration: 0.15 } },
};

const detailVariants = {
  hidden: { opacity: 0, x: 20 },
  visible: { opacity: 1, x: 0, transition: { duration: 0.3, ease: [0.16, 1, 0.3, 1] as const } },
  exit: { opacity: 0, x: 20, transition: { duration: 0.15 } },
};

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function PlaybooksPage() {
  const { t, locale } = useTranslation();
  /* ── Data state ─────────────────────────────────────────────────── */
  const [playbooks, setPlaybooks] = useState<Playbook[]>([]);
  const [loading, setLoading] = useState(true);

  /* ── Create modal ───────────────────────────────────────────────── */
  const [showCreateModal, setShowCreateModal] = useState(false);
  const [formTitle, setFormTitle] = useState('');
  const [formDesc, setFormDesc] = useState('');
  const [formQuery, setFormQuery] = useState('');
  const [creating, setCreating] = useState(false);

  /* ── Detail view ────────────────────────────────────────────────── */
  const [selectedPlaybook, setSelectedPlaybook] = useState<Playbook | null>(null);
  const [citations, setCitations] = useState<PlaybookCitation[]>([]);
  const [loadingCitations, setLoadingCitations] = useState(false);

  /* ── Delete confirmation ────────────────────────────────────────── */
  const [deleteTarget, setDeleteTarget] = useState<Playbook | null>(null);
  const [deleting, setDeleting] = useState(false);

  /* ── Inline edit ────────────────────────────────────────────────── */
  const [editMode, setEditMode] = useState(false);
  const [editTitle, setEditTitle] = useState('');
  const [editDesc, setEditDesc] = useState('');
  const [saving, setSaving] = useState(false);

  /* ── Chat panel ─────────────────────────────────────────────────── */
  const [chatOpen, setChatOpen] = useState(false);
  const [chatMessage, setChatMessage] = useState('');

  /* ── Remove citation confirm ────────────────────────────────────── */
  const [removeCitTarget, setRemoveCitTarget] = useState<string | null>(null);

  /* ── Citation note editing ──────────────────────────────────────── */
  const [editingCitId, setEditingCitId] = useState<string | null>(null);
  const [editNoteText, setEditNoteText] = useState('');
  const [savingNote, setSavingNote] = useState(false);

  /* ================================================================ */
  /*  Data loading                                                     */
  /* ================================================================ */

  const loadPlaybooks = useCallback(async () => {
    try {
      const list = await api.listPlaybooks();
      setPlaybooks(list);
    } catch (e) {
      toast.error(`${t('playbooks.loadError')}: ${String(e)}`);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadPlaybooks();
  }, [loadPlaybooks]);

  /* ================================================================ */
  /*  Handlers                                                         */
  /* ================================================================ */

  const handleCreate = async () => {
    if (!formTitle.trim()) return;
    setCreating(true);
    try {
      await api.createPlaybook(formTitle.trim(), formDesc.trim(), formQuery.trim());
      setFormTitle('');
      setFormDesc('');
      setFormQuery('');
      setShowCreateModal(false);
      toast.success(t('playbooks.created'));
      await loadPlaybooks();
    } catch (e) {
      toast.error(`${t('playbooks.createError')}: ${String(e)}`);
    } finally {
      setCreating(false);
    }
  };

  const handleDelete = async () => {
    if (!deleteTarget) return;
    setDeleting(true);
    try {
      await api.deletePlaybook(deleteTarget.id);
      if (selectedPlaybook?.id === deleteTarget.id) {
        setSelectedPlaybook(null);
        setCitations([]);
      }
      toast.success(t('playbooks.deleted'));
      await loadPlaybooks();
    } catch (e) {
      toast.error(`${t('playbooks.deleteError')}: ${String(e)}`);
    } finally {
      setDeleting(false);
      setDeleteTarget(null);
    }
  };

  const handleSelect = async (playbook: Playbook) => {
    setSelectedPlaybook(playbook);
    setEditMode(false);
    setLoadingCitations(true);
    try {
      const cits = await api.listCitations(playbook.id);
      setCitations(cits);
    } catch (e) {
      toast.error(`${t('playbooks.loadCitationsError')}: ${String(e)}`);
    } finally {
      setLoadingCitations(false);
    }
  };

  const handleRemoveCitation = async () => {
    if (!removeCitTarget) return;
    try {
      await api.removeCitation(removeCitTarget);
      setCitations((prev) => prev.filter((c) => c.id !== removeCitTarget));
      toast.success(t('playbooks.citationRemoved'));
    } catch (e) {
      toast.error(`${t('playbooks.removeCitationError')}: ${String(e)}`);
    } finally {
      setRemoveCitTarget(null);
    }
  };

  const startEdit = () => {
    if (!selectedPlaybook) return;
    setEditTitle(selectedPlaybook.title);
    setEditDesc(selectedPlaybook.description);
    setEditMode(true);
  };

  const handleSaveEdit = async () => {
    if (!selectedPlaybook || !editTitle.trim()) return;
    setSaving(true);
    try {
      const updated = await api.updatePlaybook(
        selectedPlaybook.id,
        editTitle.trim(),
        editDesc.trim(),
      );
      setSelectedPlaybook(updated);
      setEditMode(false);
      toast.success(t('playbooks.updated'));
      await loadPlaybooks();
    } catch (e) {
      toast.error(`${t('playbooks.updateError')}: ${String(e)}`);
    } finally {
      setSaving(false);
    }
  };

  /* ── Citation note editing ──────────────────────────────────────── */

  const startEditNote = (cit: PlaybookCitation) => {
    setEditingCitId(cit.id);
    setEditNoteText(cit.annotation ?? '');
  };

  const cancelEditNote = () => {
    setEditingCitId(null);
    setEditNoteText('');
  };

  const handleSaveNote = async () => {
    if (!editingCitId) return;
    setSavingNote(true);
    try {
      await api.updateCitationNote(editingCitId, editNoteText.trim());
      setCitations((prev) =>
        prev.map((c) => (c.id === editingCitId ? { ...c, annotation: editNoteText.trim() } : c)),
      );
      toast.success(t('playbooks.noteUpdated'));
      setEditingCitId(null);
      setEditNoteText('');
    } catch (e) {
      toast.error(`${t('playbooks.noteUpdateError')}: ${String(e)}`);
    } finally {
      setSavingNote(false);
    }
  };

  /* ── Ask AI handler ──────────────────────────────────────────────── */

  const handleAskAI = (context: string) => {
    setChatMessage(context);
    setChatOpen(true);
  };

  /* ── Citation reordering ────────────────────────────────────────── */

  const handleMoveCitation = async (index: number, direction: 'up' | 'down') => {
    if (!selectedPlaybook) return;
    const swapIndex = direction === 'up' ? index - 1 : index + 1;
    if (swapIndex < 0 || swapIndex >= citations.length) return;
    const reordered = [...citations];
    [reordered[index], reordered[swapIndex]] = [reordered[swapIndex], reordered[index]];
    setCitations(reordered);
    try {
      await api.reorderCitations(selectedPlaybook.id, reordered.map((c) => c.id));
    } catch (e) {
      setCitations(citations); // revert on error
      toast.error(`${t('playbooks.reorderError')}: ${String(e)}`);
    }
  };

  /* ================================================================ */
  /*  Loading skeleton                                                 */
  /* ================================================================ */

  if (loading) {
    return (
      <div className="mx-auto max-w-6xl p-6">
        <div className="flex items-center justify-between mb-6">
          <Skeleton className="h-7 w-24" />
          <Skeleton className="h-9 w-28 rounded-md" />
        </div>
        <div className="flex gap-6">
          <div className="w-[clamp(260px,28vw,340px)] shrink-0 space-y-2">
            {Array.from({ length: 4 }).map((_, i) => (
              <CardSkeleton key={i} />
            ))}
          </div>
          <div className="flex-1">
            <CardSkeleton />
          </div>
        </div>
      </div>
    );
  }

  /* ================================================================ */
  /*  Render                                                           */
  /* ================================================================ */

  return (
    <div className="flex h-full">
    <div className="mx-auto max-w-6xl p-6 flex-1 min-w-0 overflow-y-auto">
      {/* ── Header ──────────────────────────────────────────────── */}
      <div className="flex items-center justify-between mb-6">
        <h1 className="text-lg font-semibold text-text-primary">{t('playbooks.title')}</h1>
        <Button
          variant="primary"
          size="sm"
          icon={<Plus size={15} />}
          onClick={() => setShowCreateModal(true)}
        >
          {t('playbooks.create')}
        </Button>
      </div>

      {/* ── Split panel ─────────────────────────────────────────── */}
      <div className="flex gap-6 items-start">
        {/* ── Left: list ──────────────────────────────────────── */}
        <div className="w-[clamp(260px,28vw,340px)] shrink-0 space-y-1.5 overflow-y-auto max-h-[calc(100vh-160px)] pr-1">
          {playbooks.length === 0 ? (
            <EmptyState
              icon={<BookOpen size={32} />}
              title={t('playbooks.emptyTitle')}
              description={t('playbooks.emptyDesc')}
              action={{ label: t('playbooks.create'), onClick: () => setShowCreateModal(true) }}
            />
          ) : (
            <AnimatePresence mode="popLayout">
              {playbooks.map((pb, i) => {
                const isActive = selectedPlaybook?.id === pb.id;
                return (
                  <motion.button
                    key={pb.id}
                    custom={i}
                    variants={listItemVariants}
                    initial="hidden"
                    animate="visible"
                    exit="exit"
                    layout
                    onClick={() => handleSelect(pb)}
                    className={`
                      w-full rounded-lg border p-3 text-left transition-colors cursor-pointer
                      ${isActive
                        ? 'border-accent bg-accent/8 ring-1 ring-accent/20'
                        : 'border-border bg-surface-1 hover:bg-surface-2 hover:border-border-hover'
                      }
                    `}
                  >
                    <div className="flex items-start gap-2.5">
                      <div className={`shrink-0 mt-0.5 ${isActive ? 'text-accent' : 'text-text-tertiary'}`}>
                        <BookOpen size={16} />
                      </div>
                      <div className="min-w-0 flex-1">
                        <p className="text-sm font-medium text-text-primary truncate">
                          {pb.title}
                        </p>
                        {pb.description && (
                          <p className="mt-0.5 text-xs text-text-tertiary truncate leading-relaxed">
                            {pb.description}
                          </p>
                        )}
                        <div className="mt-2 flex items-center gap-2">
                          <Badge variant="info">
                            {t('playbooks.citationCount', { count: pb.citations?.length ?? 0 })}
                          </Badge>
                          <span className="text-[11px] text-text-tertiary flex items-center gap-1">
                            <Calendar size={11} />
                            {formatDate(pb.createdAt, locale)}
                          </span>
                        </div>
                      </div>
                    </div>
                  </motion.button>
                );
              })}
            </AnimatePresence>
          )}
        </div>

        {/* ── Right: detail ───────────────────────────────────── */}
        <div className="flex-1 min-w-0 min-h-[400px]">
          <AnimatePresence mode="wait">
            {selectedPlaybook ? (
              <motion.div
                key={selectedPlaybook.id}
                variants={detailVariants}
                initial="hidden"
                animate="visible"
                exit="exit"
                className="rounded-lg border border-border bg-surface-1 overflow-hidden"
              >
                {/* Detail header */}
                <div className="px-5 py-4 border-b border-border">
                  {editMode ? (
                    <div className="space-y-3">
                      <Input
                        value={editTitle}
                        onChange={(e) => setEditTitle(e.target.value)}
                        placeholder={t('playbooks.namePlaceholder')}
                      />
                      <Input
                        value={editDesc}
                        onChange={(e) => setEditDesc(e.target.value)}
                        placeholder={t('playbooks.descPlaceholder')}
                      />
                      <div className="flex items-center gap-2">
                        <Button
                          variant="primary"
                          size="sm"
                          onClick={handleSaveEdit}
                          loading={saving}
                          disabled={!editTitle.trim()}
                        >
                          {t('common.save')}
                        </Button>
                        <Button variant="ghost" size="sm" onClick={() => setEditMode(false)}>
                          {t('common.cancel')}
                        </Button>
                      </div>
                    </div>
                  ) : (
                    <div className="flex items-start justify-between gap-3">
                      <div className="min-w-0 flex-1">
                        <h2 className="text-base font-semibold text-text-primary truncate">
                          {selectedPlaybook.title}
                        </h2>
                        {selectedPlaybook.description && (
                          <p className="mt-1 text-sm text-text-secondary leading-relaxed">
                            {selectedPlaybook.description}
                          </p>
                        )}
                        <p className="mt-2 text-xs text-text-tertiary flex items-center gap-1">
                          <Calendar size={12} />
                          {t('playbooks.createdAt', { date: formatDate(selectedPlaybook.createdAt, locale) })}
                        </p>
                      </div>
                      <div className="flex items-center gap-1 shrink-0">
                        <button
                          onClick={() => {
                            const citationContext = citations.length > 0
                              ? '\n\nSaved citations:\n' + citations
                                  .sort((a, b) => a.order - b.order)
                                  .map((c, i) => `${i + 1}. ${c.annotation || '(no note)'}`)
                                  .join('\n')
                              : '';
                            handleAskAI(
                              `Tell me about the playbook "${selectedPlaybook.title}": ${selectedPlaybook.description || ''}${citationContext}`
                            );
                          }}
                          className="rounded-md px-3 py-1.5 text-xs font-medium text-accent hover:bg-accent/10 transition-colors cursor-pointer flex items-center gap-1.5"
                          title={t('chat.askAboutThis')}
                        >
                          <BotMessageSquare size={14} />
                          <span>{t('chat.askAi')}</span>
                        </button>
                        <Button variant="ghost" size="sm" icon={<Pencil size={14} />} onClick={startEdit}>
                          {t('common.edit')}
                        </Button>
                        <Button
                          variant="danger"
                          size="sm"
                          icon={<Trash2 size={14} />}
                          onClick={() => setDeleteTarget(selectedPlaybook)}
                        >
                          {t('playbooks.delete')}
                        </Button>
                      </div>
                    </div>
                  )}
                </div>

                {/* Citations section */}
                <div className="px-5 py-4">
                  <div className="flex items-center gap-2 mb-3">
                    <h3 className="text-xs font-medium uppercase tracking-wider text-text-tertiary">
                      {t('playbooks.citations')}
                    </h3>
                    <Badge variant="default">{citations.length}</Badge>
                  </div>

                  {loadingCitations ? (
                    <div className="space-y-2">
                      {Array.from({ length: 3 }).map((_, i) => (
                        <CardSkeleton key={i} />
                      ))}
                    </div>
                  ) : citations.length === 0 ? (
                    <div className="flex flex-col items-center justify-center py-12 text-center">
                      <div className="p-3 rounded-xl bg-surface-2 text-text-tertiary mb-3">
                        <FileText size={24} />
                      </div>
                      <p className="text-sm font-medium text-text-secondary mb-1">{t('playbooks.noCitations')}</p>
                      <p className="text-xs text-text-tertiary max-w-xs leading-relaxed">
                        {t('playbooks.noCitationsDesc')}
                      </p>
                    </div>
                  ) : (
                    <AnimatePresence mode="popLayout">
                      <div className="space-y-2">
                        {citations.map((cit, i) => (
                          <motion.div
                            key={cit.id}
                            custom={i}
                            variants={listItemVariants}
                            initial="hidden"
                            animate="visible"
                            exit="exit"
                            layout
                            className="group rounded-md border border-border bg-surface-2 p-3 transition-colors hover:border-border-hover"
                          >
                            <div className="flex items-start justify-between gap-2">
                              <div className="min-w-0 flex-1">
                                <p className="text-xs font-mono text-text-tertiary">
                                  {t('playbooks.chunkId')}: {cit.chunkId.slice(0, 12)}…
                                </p>
                                {editingCitId === cit.id ? (
                                  <div className="mt-1.5 space-y-2">
                                    <textarea
                                      value={editNoteText}
                                      onChange={(e) => setEditNoteText(e.target.value)}
                                      className="w-full rounded-md border border-border bg-surface-1 px-2.5 py-1.5 text-sm text-text-primary placeholder:text-text-tertiary focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent/30 resize-y min-h-[60px]"
                                      placeholder={t('playbooks.editNote')}
                                      autoFocus
                                    />
                                    <div className="flex items-center gap-1.5">
                                      <Button
                                        variant="primary"
                                        size="sm"
                                        icon={<Check size={13} />}
                                        onClick={handleSaveNote}
                                        loading={savingNote}
                                      >
                                        {t('playbooks.saveNote')}
                                      </Button>
                                      <Button
                                        variant="ghost"
                                        size="sm"
                                        onClick={cancelEditNote}
                                        disabled={savingNote}
                                      >
                                        {t('playbooks.cancelEdit')}
                                      </Button>
                                    </div>
                                  </div>
                                ) : (
                                  cit.annotation && (
                                    <p className="mt-1.5 text-sm text-text-secondary leading-relaxed">
                                      {cit.annotation}
                                    </p>
                                  )
                                )}
                              </div>
                              <div className="flex items-center gap-0.5 shrink-0">
                                {editingCitId !== cit.id && (
                                  <Button
                                    variant="ghost"
                                    size="sm"
                                    icon={<Pencil size={13} />}
                                    className="opacity-0 group-hover:opacity-100 transition-opacity"
                                    onClick={() => startEditNote(cit)}
                                    title={t('playbooks.editNote')}
                                  />
                                )}
                                <Button
                                  variant="ghost"
                                  size="sm"
                                  icon={<ChevronUp size={14} />}
                                  className="opacity-0 group-hover:opacity-100 transition-opacity"
                                  onClick={() => handleMoveCitation(i, 'up')}
                                  disabled={i === 0}
                                  title={t('playbooks.moveUp')}
                                />
                                <Button
                                  variant="ghost"
                                  size="sm"
                                  icon={<ChevronDown size={14} />}
                                  className="opacity-0 group-hover:opacity-100 transition-opacity"
                                  onClick={() => handleMoveCitation(i, 'down')}
                                  disabled={i === citations.length - 1}
                                  title={t('playbooks.moveDown')}
                                />
                                <Button
                                  variant="ghost"
                                  size="sm"
                                  icon={<X size={14} />}
                                  className="opacity-0 group-hover:opacity-100 transition-opacity"
                                  onClick={() => setRemoveCitTarget(cit.id)}
                                />
                              </div>
                            </div>
                          </motion.div>
                        ))}
                      </div>
                    </AnimatePresence>
                  )}
                </div>
              </motion.div>
            ) : (
              <motion.div
                key="empty-detail"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                className="flex flex-col items-center justify-center h-full min-h-[400px] text-center"
              >
                <div className="p-4 rounded-2xl bg-surface-2 text-text-tertiary mb-4">
                  <BookOpen size={28} />
                </div>
                <p className="text-sm text-text-tertiary">{t('playbooks.selectHintDesc')}</p>
              </motion.div>
            )}
          </AnimatePresence>
        </div>
      </div>

      {/* ── Create playbook modal ────────────────────────────────── */}
      <Modal
        open={showCreateModal}
        onClose={() => setShowCreateModal(false)}
        title={t('playbooks.createModal.title')}
        footer={
          <>
            <Button variant="ghost" size="sm" onClick={() => setShowCreateModal(false)}>
              {t('common.cancel')}
            </Button>
            <Button
              variant="primary"
              size="sm"
              onClick={handleCreate}
              loading={creating}
              disabled={!formTitle.trim()}
            >
              {t('common.create')}
            </Button>
          </>
        }
      >
        <div className="space-y-4">
          <div>
            <label className="block text-xs font-medium text-text-secondary mb-1.5">
              {t('playbooks.createModal.name')} <span className="text-danger">*</span>
            </label>
            <Input
              value={formTitle}
              onChange={(e) => setFormTitle(e.target.value)}
              placeholder={t('playbooks.namePlaceholder')}
              autoFocus
            />
          </div>
          <div>
            <label className="block text-xs font-medium text-text-secondary mb-1.5">
              {t('playbooks.createModal.desc')}
            </label>
            <Input
              value={formDesc}
              onChange={(e) => setFormDesc(e.target.value)}
              placeholder={t('playbooks.descPlaceholder')}
            />
          </div>
          <div>
            <label className="block text-xs font-medium text-text-secondary mb-1.5">
              {t('playbooks.createModal.query')}
            </label>
            <Input
              value={formQuery}
              onChange={(e) => setFormQuery(e.target.value)}
              placeholder={t('playbooks.queryPlaceholder')}
            />
          </div>
        </div>
      </Modal>

      {/* ── Delete confirm dialog ────────────────────────────────── */}
      <ConfirmDialog
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        onConfirm={handleDelete}
        title={t('playbooks.deleteConfirm')}
        message={t('playbooks.deleteConfirmMsg', { name: deleteTarget?.title ?? '' })}
        confirmText={t('playbooks.delete')}
        variant="danger"
        loading={deleting}
      />

      {/* ── Remove citation confirm dialog ───────────────────────── */}
      <ConfirmDialog
        open={!!removeCitTarget}
        onClose={() => setRemoveCitTarget(null)}
        onConfirm={handleRemoveCitation}
        title={t('playbooks.removeCitation')}
        message={t('playbooks.removeCitationConfirm')}
        confirmText={t('common.remove')}
        variant="danger"
      />
    </div>

    {/* ── Chat side panel ────────────────────────────────────── */}
    <AnimatePresence>
      {chatOpen && (
        <motion.div
          initial={{ width: 0, opacity: 0 }}
          animate={{ width: 400, opacity: 1 }}
          exit={{ width: 0, opacity: 0 }}
          transition={{ duration: 0.3, ease: [0.16, 1, 0.3, 1] }}
          className="shrink-0 border-l border-border h-full overflow-hidden"
        >
          <ChatPanel
            initialMessage={chatMessage}
            onClose={() => setChatOpen(false)}
            className="h-full"
          />
        </motion.div>
      )}
    </AnimatePresence>
    </div>
  );
}
