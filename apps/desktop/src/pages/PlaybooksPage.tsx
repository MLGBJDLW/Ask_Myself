import { useState, useEffect, useCallback } from 'react';
import { useNavigate } from 'react-router-dom';
import { motion, AnimatePresence } from 'framer-motion';
import { BookOpen, Plus, Trash2, X, Pencil, FileText, Calendar, ChevronUp, ChevronDown, Check, BotMessageSquare, FolderOpen, ExternalLink, Hash } from 'lucide-react';
import { toast } from 'sonner';
import * as api from '../lib/api';
import type { EvidenceCard, Playbook, PlaybookCitation } from '../types';
import { useTranslation } from '../i18n';
import { Button } from '../components/ui/Button';
import { Input } from '../components/ui/Input';
import { Badge } from '../components/ui/Badge';
import { Modal } from '../components/ui/Modal';
import { Skeleton, CardSkeleton } from '../components/ui/Skeleton';
import { EmptyState } from '../components/ui/EmptyState';
import { ConfirmDialog } from '../components/ui/ConfirmDialog';
import { undoableAction } from '../lib/undoToast';
import { canPreviewInApp, useFilePreview } from '../lib/filePreviewContext';

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

function basename(path: string): string {
  const normalized = path.replace(/[\\/]+$/, '');
  const lastSep = Math.max(normalized.lastIndexOf('/'), normalized.lastIndexOf('\\'));
  return lastSep === -1 ? normalized : normalized.slice(lastSep + 1);
}

function truncate(text: string, max = 220): string {
  if (text.length <= max) return text;
  return `${text.slice(0, max).trimEnd()}...`;
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
  const navigate = useNavigate();
  const { openFilePreview } = useFilePreview();
  /* 鈹€鈹€ Data state 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */
  const [playbooks, setPlaybooks] = useState<Playbook[]>([]);
  const [loading, setLoading] = useState(true);

  /* 鈹€鈹€ Create modal 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */
  const [showCreateModal, setShowCreateModal] = useState(false);
  const [formTitle, setFormTitle] = useState('');
  const [formDesc, setFormDesc] = useState('');
  const [formQuery, setFormQuery] = useState('');
  const [creating, setCreating] = useState(false);

  /* 鈹€鈹€ Detail view 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */
  const [selectedPlaybook, setSelectedPlaybook] = useState<Playbook | null>(null);
  const [citations, setCitations] = useState<PlaybookCitation[]>([]);
  const [citationEvidence, setCitationEvidence] = useState<Record<string, EvidenceCard | null>>({});
  const [loadingCitations, setLoadingCitations] = useState(false);

  /* 鈹€鈹€ Delete confirmation 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */

  /* 鈹€鈹€ Inline edit 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */
  const [editMode, setEditMode] = useState(false);
  const [editTitle, setEditTitle] = useState('');
  const [editDesc, setEditDesc] = useState('');
  const [saving, setSaving] = useState(false);

  /* 鈹€鈹€ Chat panel 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */
  /* 鈹€鈹€ Remove citation confirm 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */
  const [removeCitTarget, setRemoveCitTarget] = useState<string | null>(null);

  /* 鈹€鈹€ Citation note editing 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */
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
      setSelectedPlaybook((prev) => {
        if (!prev) return prev;
        const refreshed = list.find((pb) => pb.id === prev.id) ?? null;
        if (!refreshed) {
          setCitations([]);
          setCitationEvidence({});
          return null;
        }
        return { ...prev, ...refreshed };
      });
    } catch (e) {
      toast.error(`${t('playbooks.loadError')}: ${String(e)}`);
    } finally {
      setLoading(false);
    }
  }, [t]);

  const loadCitationEvidence = useCallback(async (items: PlaybookCitation[]) => {
    if (items.length === 0) {
      setCitationEvidence({});
      return;
    }

    const next: Record<string, EvidenceCard | null> = {};

    try {
      const evidenceCards = await api.getEvidenceCards(items.map((citation) => citation.chunkId));
      const byChunkId = new Map(evidenceCards.map((card) => [card.chunkId, card] as const));
      for (const citation of items) {
        next[citation.id] = byChunkId.get(citation.chunkId) ?? null;
      }
    } catch {
      const results = await Promise.allSettled(
        items.map(async (citation) => [citation.id, await api.getEvidenceCard(citation.chunkId)] as const),
      );

      for (const result of results) {
        if (result.status === 'fulfilled') {
          const [citationId, evidence] = result.value;
          next[citationId] = evidence;
        }
      }
    }

    for (const citation of items) {
      if (!(citation.id in next)) {
        next[citation.id] = null;
      }
    }

    setCitationEvidence(next);
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
      const created = await api.createPlaybook(formTitle.trim(), formDesc.trim(), formQuery.trim());
      setFormTitle('');
      setFormDesc('');
      setFormQuery('');
      setShowCreateModal(false);
      toast.success(t('playbooks.created'));
      await loadPlaybooks();
      await handleSelect(created);
    } catch (e) {
      toast.error(`${t('playbooks.createError')}: ${String(e)}`);
    } finally {
      setCreating(false);
    }
  };

  const handleDelete = (target: Playbook) => {
    setPlaybooks((prev) => prev.filter((p) => p.id !== target.id));
    if (selectedPlaybook?.id === target.id) {
      setSelectedPlaybook(null);
      setCitations([]);
    }
    undoableAction({
      message: t('playbooks.deleted'),
      undoLabel: t('common.undo'),
      onConfirm: async () => {
        try {
          await api.deletePlaybook(target.id);
        } catch (e) {
          toast.error(`${t('playbooks.deleteError')}: ${String(e)}`);
          await loadPlaybooks();
        }
      },
    });
  };

  const handleSelect = async (playbook: Playbook) => {
    setEditMode(false);
    setLoadingCitations(true);
    try {
      const fullPlaybook = await api.getPlaybook(playbook.id);
      const orderedCitations = [...fullPlaybook.citations].sort((a, b) => a.order - b.order);
      setSelectedPlaybook(fullPlaybook);
      setCitations(orderedCitations);
      await loadCitationEvidence(orderedCitations);
    } catch (e) {
      toast.error(`${t('playbooks.loadCitationsError')}: ${String(e)}`);
      setCitationEvidence({});
    } finally {
      setLoadingCitations(false);
    }
  };

  const handleRemoveCitation = async () => {
    if (!removeCitTarget) return;
    try {
      await api.removeCitation(removeCitTarget);
      setCitations((prev) => prev.filter((c) => c.id !== removeCitTarget));
      setCitationEvidence((prev) => {
        const next = { ...prev };
        delete next[removeCitTarget];
        return next;
      });
      setSelectedPlaybook((prev) => prev ? {
        ...prev,
        citations: prev.citations.filter((c) => c.id !== removeCitTarget),
      } : prev);
      setPlaybooks((prev) => prev.map((pb) => pb.id === selectedPlaybook?.id
        ? { ...pb, citations: pb.citations.filter((c) => c.id !== removeCitTarget) }
        : pb));
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

  /* 鈹€鈹€ Citation note editing 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */

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

  /* 鈹€鈹€ Ask AI handler 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */

  const handleAskAI = (
    context: string,
    collectionContext?: { title: string; description?: string; queryText?: string; sourceIds: string[] },
    sourceIds?: string[],
  ) => {
    const trimmed = context.trim();
    navigate('/chat', {
      state: trimmed ? {
        initialMessage: trimmed,
        collectionContext: collectionContext ?? undefined,
        sourceIds: sourceIds && sourceIds.length > 0 ? sourceIds : undefined,
      } : null,
    });
  };

  const buildPlaybookSourceIds = useCallback(() => {
    const ids = citations
      .map((citation) => citationEvidence[citation.id]?.sourceId)
      .filter((value): value is string => Boolean(value));
    return Array.from(new Set(ids));
  }, [citationEvidence, citations]);

  const buildPlaybookCollectionContext = useCallback(() => {
    if (!selectedPlaybook) return null;
    const sourceIds = buildPlaybookSourceIds();
    return {
      title: selectedPlaybook.title,
      description: selectedPlaybook.description || undefined,
      queryText: selectedPlaybook.queryText || undefined,
      sourceIds,
    };
  }, [buildPlaybookSourceIds, selectedPlaybook]);

  const buildCollectionWorkspacePrompt = useCallback((mode: 'investigate' | 'brief' | 'report' | 'slides') => {
    if (!selectedPlaybook) return '';

    const evidenceLines = citations
      .sort((a, b) => a.order - b.order)
      .map((citation, index) => {
        const evidence = citationEvidence[citation.id];
        const title = evidence?.documentTitle || (evidence?.documentPath ? basename(evidence.documentPath) : citation.chunkId.slice(0, 12));
        const snippet = evidence?.snippet || evidence?.content || '';
        const note = citation.annotation ? `Note: ${citation.annotation}` : '';
        return `${index + 1}. ${title}${note ? `\n${note}` : ''}${snippet ? `\nExcerpt: ${truncate(snippet, 260)}` : ''}`;
      })
      .join('\n');

    const intro =
      mode === 'brief'
        ? 'Write a short executive brief using this collection.'
        : mode === 'report'
          ? 'Draft a polished report using this collection as the working evidence set.'
          : mode === 'slides'
            ? 'Create a presentation outline using this collection as the working evidence set.'
            : 'Continue investigating this collection and help me reason over the saved evidence.';

    return `${intro}
Collection: ${selectedPlaybook.title}
${selectedPlaybook.description ? `Description: ${selectedPlaybook.description}\n` : ''}${selectedPlaybook.queryText ? `Base query: ${selectedPlaybook.queryText}\n` : ''}Saved evidence:
${evidenceLines}`;
  }, [citationEvidence, citations, selectedPlaybook]);

  /* 鈹€鈹€ Citation reordering 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */

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
      {/* 鈹€鈹€ Header 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */}
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

      {/* 鈹€鈹€ Split panel 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */}
      <div className="flex gap-6 items-start">
        {/* 鈹€鈹€ Left: list 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */}
        <div className="w-[clamp(260px,28vw,340px)] shrink-0 space-y-1.5 overflow-y-auto max-h-[calc(100vh-160px)] pr-1">
          {playbooks.length === 0 ? (
            <EmptyState
              icon={<BookOpen size={32} />}
              title={t('playbooks.emptyTitle')}
              description={t('playbooks.emptyDesc')}
              action={{ label: t('playbooks.create'), onClick: () => setShowCreateModal(true) }}
            />
          ) : (
            <AnimatePresence initial={false}>
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
                    layout="position"
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

        {/* 鈹€鈹€ Right: detail 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */}
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
                        {selectedPlaybook.queryText && (
                          <div className="mt-2 inline-flex max-w-full items-center gap-1.5 rounded-full border border-border bg-surface-2 px-2.5 py-1 text-[11px] text-text-tertiary">
                            <Hash size={11} className="shrink-0" />
                            <span className="truncate">{selectedPlaybook.queryText}</span>
                          </div>
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
                                  .map((c, i) => {
                                    const evidence = citationEvidence[c.id];
                                    const title = evidence?.documentTitle || (evidence?.documentPath ? basename(evidence.documentPath) : c.chunkId.slice(0, 12));
                                    const snippet = evidence?.snippet || evidence?.content || '';
                                    const note = c.annotation ? `Note: ${c.annotation}` : 'Note: (none)';
                                    return `${i + 1}. ${title}\n${note}${snippet ? `\nExcerpt: ${truncate(snippet, 320)}` : ''}`;
                                  })
                                  .join('\n')
                              : '';
                            handleAskAI(
                              `Tell me about the collection "${selectedPlaybook.title}".`
                              + `${selectedPlaybook.description ? `\nDescription: ${selectedPlaybook.description}` : ''}`
                              + `${selectedPlaybook.queryText ? `\nBase query: ${selectedPlaybook.queryText}` : ''}`
                              + `${citationContext}`,
                              buildPlaybookCollectionContext() ?? undefined,
                              buildPlaybookSourceIds(),
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
                          onClick={() => handleDelete(selectedPlaybook)}
                        >
                          {t('playbooks.delete')}
                        </Button>
                      </div>
                    </div>
                  )}
                </div>

                {/* Citations section */}
                <div className="px-5 py-4">
                  <div className="mb-4 rounded-xl border border-border/70 bg-surface-1/70 p-3">
                    <div className="flex flex-wrap items-start justify-between gap-3">
                      <div>
                        <h3 className="text-sm font-semibold text-text-primary">
                          {t('playbooks.workspaceTitle')}
                        </h3>
                        <p className="mt-1 text-xs text-text-tertiary max-w-2xl">
                          {t('playbooks.workspaceDesc')}
                        </p>
                      </div>
                      <Badge variant="info">{citations.length}</Badge>
                    </div>
                    <div className="mt-3 flex flex-wrap gap-2">
                      <Button
                        variant="secondary"
                        size="sm"
                        icon={<BotMessageSquare size={13} />}
                        onClick={() => handleAskAI(
                          buildCollectionWorkspacePrompt('investigate'),
                          buildPlaybookCollectionContext() ?? undefined,
                          buildPlaybookSourceIds(),
                        )}
                      >
                        {t('playbooks.actionInvestigate')}
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        icon={<FileText size={13} />}
                        onClick={() => handleAskAI(
                          buildCollectionWorkspacePrompt('brief'),
                          buildPlaybookCollectionContext() ?? undefined,
                          buildPlaybookSourceIds(),
                        )}
                      >
                        {t('playbooks.actionBrief')}
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        icon={<FileText size={13} />}
                        onClick={() => handleAskAI(
                          buildCollectionWorkspacePrompt('report'),
                          buildPlaybookCollectionContext() ?? undefined,
                          buildPlaybookSourceIds(),
                        )}
                      >
                        {t('playbooks.actionReport')}
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        icon={<FileText size={13} />}
                        onClick={() => handleAskAI(
                          buildCollectionWorkspacePrompt('slides'),
                          buildPlaybookCollectionContext() ?? undefined,
                          buildPlaybookSourceIds(),
                        )}
                      >
                        {t('playbooks.actionSlides')}
                      </Button>
                    </div>
                  </div>

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
                    <AnimatePresence initial={false}>
                      <div className="space-y-2">
                        {citations.map((cit, i) => {
                          const evidence = citationEvidence[cit.id];
                          const title = evidence?.documentTitle || (evidence?.documentPath ? basename(evidence.documentPath) : `${t('playbooks.chunkId')}: ${cit.chunkId.slice(0, 12)}...`);
                          const snippet = evidence?.snippet || evidence?.content || '';
                          const heading = evidence?.headingPath?.length ? evidence.headingPath.join(' > ') : '';

                          return (
                            <motion.div
                              key={cit.id}
                              custom={i}
                              variants={listItemVariants}
                              initial="hidden"
                              animate="visible"
                              exit="exit"
                              layout="position"
                              className="group rounded-md border border-border bg-surface-2 p-3 transition-colors hover:border-border-hover"
                            >
                              <div className="flex items-start justify-between gap-3">
                                <div className="min-w-0 flex-1">
                                  <div className="flex flex-wrap items-center gap-2">
                                    <p className="text-sm font-medium text-text-primary truncate">
                                      {title}
                                    </p>
                                    {evidence?.sourceName && (
                                      <Badge variant="info">{evidence.sourceName}</Badge>
                                    )}
                                  </div>

                                  <p className="mt-1 text-[11px] font-mono text-text-tertiary">
                                    {t('playbooks.chunkId')}: {cit.chunkId}
                                  </p>

                                  {heading && (
                                    <p className="mt-1 text-xs text-text-tertiary truncate">
                                      {heading}
                                    </p>
                                  )}

                                  {snippet && (
                                    <p className="mt-2 text-sm text-text-secondary leading-relaxed">
                                      {truncate(snippet)}
                                    </p>
                                  )}

                                  {editingCitId === cit.id ? (
                                    <div className="mt-2 space-y-2">
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
                                    <div className="mt-2 flex flex-wrap items-start gap-2">
                                      {cit.annotation ? (
                                        <div className="rounded-md border border-border bg-surface-1 px-2.5 py-2 text-sm text-text-secondary leading-relaxed">
                                          {cit.annotation}
                                        </div>
                                      ) : (
                                        <div className="rounded-md border border-dashed border-border px-2.5 py-2 text-xs text-text-tertiary">
                                          {t('playbooks.editNote')}
                                        </div>
                                      )}
                                    </div>
                                  )}

                                  {evidence?.documentPath && (
                                    <div className="mt-2 flex flex-wrap items-center gap-2 text-[11px] text-text-tertiary">
                                      <button
                                        type="button"
                                        onClick={() => {
                                          if (canPreviewInApp(evidence.documentPath)) {
                                            openFilePreview(evidence.documentPath);
                                          } else {
                                            void api.openFileInDefaultApp(evidence.documentPath);
                                          }
                                        }}
                                        className="inline-flex items-center gap-1 hover:text-accent transition-colors cursor-pointer"
                                      >
                                        <ExternalLink size={12} />
                                        <span>{basename(evidence.documentPath)}</span>
                                      </button>
                                      <button
                                        type="button"
                                        onClick={() => api.showInFileExplorer(evidence.documentPath)}
                                        className="inline-flex items-center gap-1 hover:text-accent transition-colors cursor-pointer"
                                      >
                                        <FolderOpen size={12} />
                                        <span>{evidence.documentPath}</span>
                                      </button>
                                    </div>
                                  )}
                                </div>

                                <div className="flex items-center gap-0.5 shrink-0">
                                  {evidence && editingCitId !== cit.id && (
                                    <Button
                                      variant="ghost"
                                      size="sm"
                                      icon={<BotMessageSquare size={13} />}
                                      className="opacity-0 group-hover:opacity-100 transition-opacity"
                                      onClick={() => handleAskAI(
                                        `Tell me about this saved citation.\n`
                                        + `Title: ${title}\n`
                                        + `${heading ? `Section: ${heading}\n` : ''}`
                                        + `${cit.annotation ? `Note: ${cit.annotation}\n` : ''}`
                                        + `Excerpt: ${truncate(evidence.content, 1200)}`,
                                        buildPlaybookCollectionContext() ?? undefined,
                                        evidence.sourceId ? [evidence.sourceId] : buildPlaybookSourceIds(),
                                      )}
                                      title={t('chat.askAboutThis')}
                                    />
                                  )}
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
                          );
                        })}
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

      {/* 鈹€鈹€ Create playbook modal 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */}
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

      {/* 鈹€鈹€ Remove citation confirm dialog 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */}
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

    </div>
  );
}


