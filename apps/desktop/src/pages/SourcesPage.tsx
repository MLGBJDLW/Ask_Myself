import { useState, useEffect, useCallback } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import {
  FolderOpen,
  FolderPlus,
  FolderSearch,
  ScanSearch,
  Cpu,
  Trash2,
  Plus,
  RefreshCw,
  Pencil,
  Eye,
  EyeOff,
  Info,
} from 'lucide-react';
import { toast } from 'sonner';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import * as api from '../lib/api';
import type { Source, IngestResult, EmbedResult } from '../types';
import { useTranslation } from '../i18n';
import type { TranslationKeys } from '../i18n/types';
import { Button } from '../components/ui/Button';
import { Input } from '../components/ui/Input';
import { TagInput, parseTags } from '../components/ui/TagInput';
import { Badge } from '../components/ui/Badge';
import { Modal } from '../components/ui/Modal';
import { CardSkeleton } from '../components/ui/Skeleton';
import { EmptyState } from '../components/ui/EmptyState';
import { ConfirmDialog } from '../components/ui/ConfirmDialog';

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

const KIND_OPTIONS = [
  { value: 'local_folder', labelKey: 'sources.addModal.kindFolder' as const, icon: FolderOpen },
] as const;

type TFunc = (key: keyof TranslationKeys, params?: Record<string, string | number>) => string;

function kindIcon(_kind: string) {
  return <FolderOpen size={18} />;
}

function kindLabel(kind: string, t: TFunc) {
  const opt = KIND_OPTIONS.find((k) => k.value === kind);
  return opt ? t(opt.labelKey) : kind;
}

function formatScanResult(r: IngestResult, t: TFunc): string {
  const parts: string[] = [];
  parts.push(t('sources.scanResult', { scanned: r.filesScanned }));
  if (r.filesAdded > 0) parts.push(t('sources.scanAdded', { count: r.filesAdded }));
  if (r.filesUpdated > 0) parts.push(t('sources.scanUpdated', { count: r.filesUpdated }));
  if (r.filesSkipped > 0) parts.push(t('sources.scanSkipped', { count: r.filesSkipped }));
  if (r.filesFailed > 0) parts.push(t('sources.scanFailed', { count: r.filesFailed }));
  return parts.join(', ');
}

function formatEmbedResult(r: EmbedResult, t: TFunc): string {
  return t('sources.embedResult', { embedded: r.chunksEmbedded, skipped: r.chunksSkipped, model: r.model });
}

/* ------------------------------------------------------------------ */
/*  Stagger animation                                                  */
/* ------------------------------------------------------------------ */

const listContainer = {
  hidden: {},
  show: { transition: { staggerChildren: 0.06 } },
};

const listItem = {
  hidden: { opacity: 0, y: 12 },
  show: { opacity: 1, y: 0, transition: { duration: 0.25, ease: [0.16, 1, 0.3, 1] as const } },
};

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function SourcesPage() {
  const { t } = useTranslation();
  const [sources, setSources] = useState<Source[]>([]);
  const [loading, setLoading] = useState(true);
  const [scanningId, setScanningId] = useState<string | null>(null);
  const [scanningAll, setScanningAll] = useState(false);
  const [embeddingId, setEmbeddingId] = useState<string | null>(null);
  const [rebuildingEmbeddings, setRebuildingEmbeddings] = useState(false);

  // Add source modal
  const [showAddModal, setShowAddModal] = useState(false);
  const [formPath, setFormPath] = useState('');
  const [formInclude, setFormInclude] = useState('**/*.md');
  const [formExclude, setFormExclude] = useState('');
  const [adding, setAdding] = useState(false);

  // Delete confirmation
  const [deleteTarget, setDeleteTarget] = useState<Source | null>(null);
  const [deleting, setDeleting] = useState(false);

  // Edit source modal
  const [editTarget, setEditTarget] = useState<Source | null>(null);
  const [editInclude, setEditInclude] = useState('');
  const [editExclude, setEditExclude] = useState('');
  const [editWatch, setEditWatch] = useState(false);
  const [editing, setEditing] = useState(false);

  // Watcher
  const [togglingWatch, setTogglingWatch] = useState<string | null>(null);

  /* ── Load ─────────────────────────────────────────────────────────── */

  const loadSources = useCallback(async () => {
    try {
      const list = await api.listSources();
      setSources(list);
    } catch (e) {
      toast.error(`${t('sources.loadError')}: ${String(e)}`);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadSources();
  }, [loadSources]);

  /* ── File-change event listener ───────────────────────────────────── */

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    listen<{ sourceId: string; filesAdded: number; filesUpdated: number }>(
      'file-changed',
      (event) => {
        if (cancelled) return;
        const { sourceId } = event.payload;
        const source = sources.find((s) => s.id === sourceId);
        const name = source?.rootPath ?? sourceId;
        toast.info(t('sources.watcherFileChanged', { path: name }));
        loadSources();
      },
    ).then((fn) => {
      if (cancelled) {
        fn();
      } else {
        unlisten = fn;
      }
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [sources, loadSources, t]);

  /* ── Watch toggle ────────────────────────────────────────────────── */

  const handleToggleWatch = async (source: Source) => {
    setTogglingWatch(source.id);
    try {
      if (source.watchEnabled) {
        await api.stopWatching(source.id);
        toast.success(t('sources.watcherStop'));
      } else {
        await api.startWatching(source.id);
        toast.success(t('sources.watcherStart'));
      }
      await loadSources();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setTogglingWatch(null);
    }
  };

  /* ── Add ──────────────────────────────────────────────────────────── */

  const resetForm = () => {
    setFormPath('');
    setFormInclude('**/*.md');
    setFormExclude('');
  };

  const handleAdd = async () => {
    if (!formPath.trim()) return;
    setAdding(true);
    try {
      const includeGlobs = parseTags(formInclude);
      const excludeGlobs = parseTags(formExclude);
      await api.addSource(formPath.trim(), includeGlobs, excludeGlobs);
      toast.success(t('sources.added'));
      resetForm();
      setShowAddModal(false);
      await loadSources();
    } catch (e) {
      toast.error(`${t('sources.addError')}: ${String(e)}`);
    } finally {
      setAdding(false);
    }
  };

  /* ── Delete ───────────────────────────────────────────────────────── */

  const handleDelete = async () => {
    if (!deleteTarget) return;
    setDeleting(true);
    try {
      await api.deleteSource(deleteTarget.id);
      toast.success(t('sources.deleted'));
      setDeleteTarget(null);
      await loadSources();
    } catch (e) {
      toast.error(`${t('sources.deleteError')}: ${String(e)}`);
    } finally {
      setDeleting(false);
    }
  };

  /* ── Edit ──────────────────────────────────────────────────────────── */

  const openEditModal = (source: Source) => {
    setEditTarget(source);
    setEditInclude(source.includeGlobs.join(', '));
    setEditExclude(source.excludeGlobs.join(', '));
    setEditWatch(source.watchEnabled);
  };

  const handleEdit = async () => {
    if (!editTarget) return;
    setEditing(true);
    try {
      const includeGlobs = parseTags(editInclude);
      const excludeGlobs = parseTags(editExclude);
      await api.updateSource(editTarget.id, includeGlobs, excludeGlobs, editWatch);
      toast.success(t('sources.updated'));
      setEditTarget(null);
      await loadSources();
    } catch (e) {
      toast.error(`${t('sources.updateError')}: ${String(e)}`);
    } finally {
      setEditing(false);
    }
  };

  /* ── Scan ──────────────────────────────────────────────────────────── */

  const handleScan = async (sourceId: string) => {
    setScanningId(sourceId);
    try {
      const result = await api.scanSource(sourceId);
      toast.success(formatScanResult(result, t));
    } catch (e) {
      toast.error(`${t('sources.scanError')}: ${String(e)}`);
    } finally {
      setScanningId(null);
    }
  };

  const handleScanAll = async () => {
    setScanningAll(true);
    try {
      const results = await api.scanAllSources();
      const total = results.reduce((sum, r) => sum + r.filesScanned, 0);
      const added = results.reduce((sum, r) => sum + r.filesAdded, 0);
      const updated = results.reduce((sum, r) => sum + r.filesUpdated, 0);
      toast.success(t('sources.scanAllComplete', { total, added, updated }));
    } catch (e) {
      toast.error(`${t('sources.scanAllError')}: ${String(e)}`);
    } finally {
      setScanningAll(false);
    }
  };

  /* ── Embed ─────────────────────────────────────────────────────────── */

  const handleEmbed = async (sourceId: string) => {
    setEmbeddingId(sourceId);
    try {
      const result = await api.embedSource(sourceId);
      toast.success(formatEmbedResult(result, t));
    } catch (e) {
      toast.error(`${t('sources.embedError')}: ${String(e)}`);
    } finally {
      setEmbeddingId(null);
    }
  };

  const handleRebuildEmbeddings = async () => {
    setRebuildingEmbeddings(true);
    try {
      const result = await api.rebuildEmbeddings();
      toast.success(formatEmbedResult(result, t));
    } catch (e) {
      toast.error(`${t('sources.rebuildEmbedError')}: ${String(e)}`);
    } finally {
      setRebuildingEmbeddings(false);
    }
  };

  /* ── Loading skeleton ──────────────────────────────────────────────── */

  if (loading) {
    return (
      <div className="mx-auto max-w-3xl p-6 space-y-3">
        <div className="flex items-center justify-between mb-6">
          <div className="h-7 w-32 bg-surface-3 rounded-md animate-pulse" />
          <div className="h-9 w-28 bg-surface-3 rounded-md animate-pulse" />
        </div>
        {Array.from({ length: 3 }).map((_, i) => (
          <CardSkeleton key={i} />
        ))}
      </div>
    );
  }

  /* ── Render ─────────────────────────────────────────────────────────── */

  return (
    <div className="mx-auto max-w-3xl p-6">
      {/* Header */}
      <div className="mb-6 flex items-center justify-between">
        <h2 className="text-lg font-semibold text-text-primary">{t('sources.title')}</h2>
        <div className="flex gap-2">
          <Button
            variant="secondary"
            size="sm"
            icon={<ScanSearch size={14} />}
            onClick={handleScanAll}
            loading={scanningAll}
            disabled={sources.length === 0}
          >
            {t('sources.scanAll')}
          </Button>
          <Button
            variant="secondary"
            size="sm"
            icon={<RefreshCw size={14} />}
            onClick={handleRebuildEmbeddings}
            loading={rebuildingEmbeddings}
            disabled={sources.length === 0}
          >
            {t('sources.rebuildEmbeddings')}
          </Button>
          <Button
            variant="primary"
            size="sm"
            icon={<Plus size={14} />}
            onClick={() => setShowAddModal(true)}
          >
            {t('sources.add')}
          </Button>
        </div>
      </div>

      {/* Source list or empty state */}
      {sources.length === 0 ? (
        <EmptyState
          icon={<FolderPlus size={32} />}
          title={t('sources.emptyTitle')}
          description={t('sources.emptyDesc')}
          action={{ label: t('sources.add'), onClick: () => setShowAddModal(true) }}
        />
      ) : (
        <motion.div
          className="space-y-3"
          variants={listContainer}
          initial="hidden"
          animate="show"
        >
          <AnimatePresence mode="popLayout">
            {sources.map((source) => (
              <motion.div
                key={source.id}
                variants={listItem}
                layout
                exit={{ opacity: 0, x: -20, transition: { duration: 0.2 } }}
                className="rounded-lg border border-border bg-surface-2 p-4 hover:border-border-hover transition-colors duration-fast"
              >
                <div className="flex items-start justify-between gap-3">
                  {/* Left: icon + info */}
                  <div className="flex items-start gap-3 min-w-0 flex-1">
                    <div className="shrink-0 p-2 rounded-lg bg-accent-subtle text-accent mt-0.5">
                      {kindIcon(source.kind)}
                    </div>
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-2 mb-1">
                        <p className="truncate text-sm font-medium text-text-primary font-mono">
                          {source.rootPath}
                        </p>
                        <Badge variant="default">{kindLabel(source.kind, t)}</Badge>
                      </div>

                      {/* Globs */}
                      <div className="flex flex-wrap gap-1 mb-1.5">
                        {source.includeGlobs.map((g, i) => (
                          <Badge key={i} variant="success">{g}</Badge>
                        ))}
                        {source.excludeGlobs.map((g, i) => (
                          <Badge key={`e-${i}`} variant="danger">✕ {g}</Badge>
                        ))}
                      </div>

                      {/* Meta row */}
                      <div className="flex items-center gap-3 text-[11px] text-text-tertiary">
                        <span className={source.watchEnabled ? 'text-green-500 font-medium' : ''}>
                          {t('sources.watch')}: {source.watchEnabled ? t('sources.watcherActive') : t('sources.watchOff')}
                        </span>
                        <span>{t('sources.addedAt')}: {new Date(source.createdAt).toLocaleDateString()}</span>
                      </div>
                    </div>
                  </div>

                  {/* Right: actions */}
                  <div className="flex shrink-0 gap-1.5">
                    <Button
                      variant="ghost"
                      size="sm"
                      icon={source.watchEnabled ? <EyeOff size={14} /> : <Eye size={14} />}
                      onClick={() => handleToggleWatch(source)}
                      loading={togglingWatch === source.id}
                    >
                      {source.watchEnabled ? t('sources.watcherStop') : t('sources.watcherStart')}
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      icon={<ScanSearch size={14} />}
                      onClick={() => handleScan(source.id)}
                      loading={scanningId === source.id}
                    >
                      {t('sources.scan')}
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      icon={<Cpu size={14} />}
                      onClick={() => handleEmbed(source.id)}
                      loading={embeddingId === source.id}
                    >
                      {t('sources.embed')}
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      icon={<Pencil size={14} />}
                      onClick={() => openEditModal(source)}
                    >
                      {t('common.edit')}
                    </Button>
                    <Button
                      variant="danger"
                      size="sm"
                      icon={<Trash2 size={14} />}
                      onClick={() => setDeleteTarget(source)}
                    >
                      {t('common.delete')}
                    </Button>
                  </div>
                </div>
              </motion.div>
            ))}
          </AnimatePresence>
        </motion.div>
      )}

      {/* ── Add Source Modal ──────────────────────────────────────────── */}
      <Modal
        open={showAddModal}
        onClose={() => { setShowAddModal(false); resetForm(); }}
        title={t('sources.addModal.title')}
        footer={
          <>
            <Button variant="ghost" size="sm" onClick={() => { setShowAddModal(false); resetForm(); }}>
              {t('common.cancel')}
            </Button>
            <Button
              variant="primary"
              size="sm"
              onClick={handleAdd}
              loading={adding}
              disabled={!formPath.trim()}
            >
              {t('common.add')}
            </Button>
          </>
        }
      >
        <div className="space-y-4">
          {/* Source type info */}
          <div className="flex items-center gap-2 px-3 py-2 rounded-md bg-surface-1 border border-border">
            <FolderOpen size={14} className="text-accent shrink-0" />
            <span className="text-xs font-medium text-text-primary">{t('sources.addModal.kindFolder')}</span>
            <span className="ml-auto flex items-center gap-1 text-[11px] text-text-tertiary">
              <Info size={12} />
              {t('sources.moreTypesSoon')}
            </span>
          </div>

          {/* Root path */}
          <div>
            <label className="block text-xs font-medium text-text-secondary mb-1.5">{t('sources.addModal.rootPath')}</label>
            <div className="flex gap-2">
              <div className="flex-1">
                <Input
                  value={formPath}
                  onChange={(e) => setFormPath(e.target.value)}
                  placeholder="C:\\Users\\you\\notes  /  /home/user/notes"
                  icon={<FolderOpen size={15} />}
                />
              </div>
              <Button
                variant="ghost"
                size="sm"
                className="shrink-0 h-10"
                onClick={async () => {
                  const selected = await open({
                    directory: true,
                    multiple: false,
                    title: t('sources.addModal.rootPath'),
                  });
                  if (selected) {
                    setFormPath(typeof selected === 'string' ? selected : selected[0]);
                  }
                }}
              >
                <FolderSearch size={15} className="mr-1" />
                Browse
              </Button>
            </div>
          </div>

          {/* Include globs */}
          <div>
            <label className="block text-xs font-medium text-text-secondary mb-1.5">{t('sources.addModal.includeGlobs')}</label>
            <TagInput
              value={formInclude}
              onChange={setFormInclude}
              presets={[
                { label: 'Markdown', value: '**/*.md' },
                { label: 'Text', value: '**/*.txt' },
                { label: 'HTML', value: '**/*.html' },
                { label: 'Word', value: '**/*.docx' },
                { label: 'Excel', value: '**/*.{xlsx,xls}' },
                { label: 'PowerPoint', value: '**/*.pptx' },
                { label: 'PDF', value: '**/*.pdf' },
                { label: 'JSON', value: '**/*.json' },
                { label: 'YAML', value: '**/*.{yml,yaml}' },
                { label: 'Code', value: '**/*.{ts,js,py,rs}' },
              ]}
              placeholder="Add glob pattern..."
            />
          </div>

          {/* Exclude globs */}
          <div>
            <label className="block text-xs font-medium text-text-secondary mb-1.5">{t('sources.addModal.excludeGlobs')}</label>
            <TagInput
              value={formExclude}
              onChange={setFormExclude}
              presets={[
                { label: 'node_modules', value: '**/node_modules/**' },
                { label: '.git', value: '**/.git/**' },
                { label: '.obsidian', value: '**/.obsidian/**' },
                { label: 'dist', value: '**/dist/**' },
                { label: 'build', value: '**/build/**' },
                { label: 'target', value: '**/target/**' },
              ]}
              placeholder="Add exclude pattern..."
            />
          </div>
        </div>
      </Modal>

      {/* ── Edit Source Modal ────────────────────────────────────────── */}
      <Modal
        open={!!editTarget}
        onClose={() => setEditTarget(null)}
        title={t('sources.editModal.title')}
        footer={
          <>
            <Button variant="ghost" size="sm" onClick={() => setEditTarget(null)}>
              {t('common.cancel')}
            </Button>
            <Button
              variant="primary"
              size="sm"
              onClick={handleEdit}
              loading={editing}
            >
              {t('common.save')}
            </Button>
          </>
        }
      >
        <div className="space-y-4">
          {/* Root path (read-only) */}
          <div>
            <label className="block text-xs font-medium text-text-secondary mb-1.5">{t('sources.addModal.rootPath')}</label>
            <p className="text-sm text-text-primary font-mono truncate">{editTarget?.rootPath}</p>
          </div>

          {/* Include globs */}
          <div>
            <label className="block text-xs font-medium text-text-secondary mb-1.5">{t('sources.addModal.includeGlobs')}</label>
            <TagInput
              value={editInclude}
              onChange={setEditInclude}
              presets={[
                { label: 'Markdown', value: '**/*.md' },
                { label: 'Text', value: '**/*.txt' },
                { label: 'HTML', value: '**/*.html' },
                { label: 'Word', value: '**/*.docx' },
                { label: 'Excel', value: '**/*.{xlsx,xls}' },
                { label: 'PowerPoint', value: '**/*.pptx' },
                { label: 'PDF', value: '**/*.pdf' },
                { label: 'JSON', value: '**/*.json' },
                { label: 'YAML', value: '**/*.{yml,yaml}' },
                { label: 'Code', value: '**/*.{ts,js,py,rs}' },
              ]}
              placeholder="Add glob pattern..."
            />
          </div>

          {/* Exclude globs */}
          <div>
            <label className="block text-xs font-medium text-text-secondary mb-1.5">{t('sources.addModal.excludeGlobs')}</label>
            <TagInput
              value={editExclude}
              onChange={setEditExclude}
              presets={[
                { label: 'node_modules', value: '**/node_modules/**' },
                { label: '.git', value: '**/.git/**' },
                { label: '.obsidian', value: '**/.obsidian/**' },
                { label: 'dist', value: '**/dist/**' },
                { label: 'build', value: '**/build/**' },
                { label: 'target', value: '**/target/**' },
              ]}
              placeholder="Add exclude pattern..."
            />
          </div>

          {/* Watch toggle */}
          <div className="flex items-center gap-2">
            <input
              type="checkbox"
              id="edit-watch"
              checked={editWatch}
              onChange={(e) => setEditWatch(e.target.checked)}
              className="h-4 w-4 rounded border-border text-accent focus:ring-accent/30"
            />
            <label htmlFor="edit-watch" className="text-xs font-medium text-text-secondary">
              {t('sources.editModal.watch')}
            </label>
          </div>
        </div>
      </Modal>

      {/* ── Delete Confirm Dialog ────────────────────────────────────── */}
      <ConfirmDialog
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        onConfirm={handleDelete}
        title={t('sources.deleteConfirm')}
        message={t('sources.deleteConfirmMsg', { name: deleteTarget?.rootPath ?? '' })}
        confirmText={t('common.delete')}
        variant="danger"
        loading={deleting}
      />
    </div>
  );
}
