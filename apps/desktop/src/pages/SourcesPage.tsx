import { useState, useEffect, useCallback, useMemo } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { motion, AnimatePresence, useReducedMotion } from 'framer-motion';
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
  BotMessageSquare,
  AlertTriangle,
  ChevronDown,
} from 'lucide-react';
import { toast } from 'sonner';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import * as api from '../lib/api';
import type { Source, IngestResult, EmbedResult } from '../types';
import { useProgress, progressStore } from '../lib/progressStore';
import { useTranslation } from '../i18n';
import type { TranslationKeys } from '../i18n/types';
import { Button } from '../components/ui/Button';
import { ConfirmDialog } from '../components/ui/ConfirmDialog';
import { Input } from '../components/ui/Input';
import { TagInput, parseTags } from '../components/ui/TagInput';
import { Badge } from '../components/ui/Badge';
import { Modal } from '../components/ui/Modal';
import { CardSkeleton } from '../components/ui/Skeleton';
import { EmptyState } from '../components/ui/EmptyState';
import { VideoProcessingProgress } from '../components/media/VideoProcessingProgress';
import { SourceFileTree } from '../components/sources/SourceFileTree';
import { undoableAction } from '../lib/undoToast';

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

const KIND_OPTIONS = [
  { value: 'local_folder', labelKey: 'sources.addModal.kindFolder' as const, icon: FolderOpen },
] as const;

type TFunc = (key: keyof TranslationKeys, params?: Record<string, string | number>) => string;

type BatchAction = 'scanAll' | 'rebuildEmbeddings';
type SourceDetailTab = 'overview' | 'files';

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
/*  Shared preset definitions                                          */
/* ------------------------------------------------------------------ */

const INCLUDE_PRESET_KEYS = [
  { labelKey: 'sources.presetMarkdown', value: '**/*.md' },
  { labelKey: 'sources.presetText', value: '**/*.txt' },
  { labelKey: 'sources.presetHtml', value: '**/*.html' },
  { labelKey: 'sources.presetWord', value: '**/*.docx' },
  { labelKey: 'sources.presetExcel', value: '**/*.{xlsx,xls}' },
  { labelKey: 'sources.presetPowerpoint', value: '**/*.pptx' },
  { labelKey: 'sources.presetPdf', value: '**/*.pdf' },
  { labelKey: 'sources.presetImage', value: '**/*.{jpg,jpeg,png,gif,webp}' },
  { labelKey: 'sources.presetVideo', value: '**/*.{mp4,mkv,webm,avi,mov,flv,mpeg,mpg,wmv,m4v}' },
  { labelKey: 'sources.presetAudio', value: '**/*.{mp3,wav,flac,ogg,aac,m4a,wma,opus}' },
  { labelKey: 'sources.presetJson', value: '**/*.json' },
  { labelKey: 'sources.presetYaml', value: '**/*.{yml,yaml}' },
  { labelKey: 'sources.presetCode', value: '**/*.{ts,js,py,rs}' },
  { labelKey: 'sources.presetLog', value: '**/*.log' },
] as const;

const EXCLUDE_PRESETS = [
  { label: 'node_modules', value: '**/node_modules/**' },
  { label: '.git', value: '**/.git/**' },
  { label: '.obsidian', value: '**/.obsidian/**' },
  { label: 'dist', value: '**/dist/**' },
  { label: 'build', value: '**/build/**' },
  { label: 'target', value: '**/target/**' },
];

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function SourcesPage() {
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();
  const navigate = useNavigate();
  const location = useLocation();

  const includePresets = useMemo(
    () => INCLUDE_PRESET_KEYS.map(p => ({ label: t(p.labelKey as any), value: p.value })),
    [t],
  );
  const [sources, setSources] = useState<Source[]>([]);
  const [loading, setLoading] = useState(true);
  const [scanningId, setScanningId] = useState<string | null>(null);
  const [scanningAll, setScanningAll] = useState(false);
  const [embeddingId, setEmbeddingId] = useState<string | null>(null);
  const [rebuildingEmbeddings, setRebuildingEmbeddings] = useState(false);
  const [indexingIds, setIndexingIds] = useState<Set<string>>(new Set());
  // Progressive disclosure: remember which cards the user has manually expanded.
  // Actively-scanning cards auto-expand in the render path regardless of this set.
  const [expandedSourceIds, setExpandedSourceIds] = useState<Set<string>>(new Set());
  const [sourceDetailTabs, setSourceDetailTabs] = useState<Record<string, SourceDetailTab>>({});

  // Add source modal
  const [showAddModal, setShowAddModal] = useState(false);
  const [formPath, setFormPath] = useState('');
  const [formInclude, setFormInclude] = useState('**/*.md');
  const [formExclude, setFormExclude] = useState('');
  const [formWatch, setFormWatch] = useState(true);
  const [formIndexNow, setFormIndexNow] = useState(true);
  const [adding, setAdding] = useState(false);

  // Edit source modal
  const [editTarget, setEditTarget] = useState<Source | null>(null);
  const [editRootPath, setEditRootPath] = useState('');
  const [editInclude, setEditInclude] = useState('');
  const [editExclude, setEditExclude] = useState('');
  const [editWatch, setEditWatch] = useState(false);
  const [editing, setEditing] = useState(false);

  // Watcher
  const [togglingWatch, setTogglingWatch] = useState<string | null>(null);

  // Scan/embed progress (from global store)
  const progress = useProgress();
  const scanProgress = progress.scanProgress;
  const videoProcessing = progress.videoProcessing;
  // Merge batch:scan-progress and batch:rebuild-progress into one batchProgress
  const batchProgress = useMemo(() => {
    if (progress.batchProgress) return progress.batchProgress;
    if (progress.embedRebuildProgress) {
      const p = progress.embedRebuildProgress;
      return {
        operation: 'rebuild-embeddings',
        sourceIndex: 0,
        sourceCount: 0,
        sourceId: p.sourceId,
        phase: p.phase,
        current: p.current,
        total: p.total,
        currentFile: p.currentFile,
      };
    }
    return null;
  }, [progress.batchProgress, progress.embedRebuildProgress]);
  const [pendingBatchAction, setPendingBatchAction] = useState<BatchAction | null>(null);

  // Whisper model check
  const [whisperModelMissing, setWhisperModelMissing] = useState(false);

  /* 鈹€鈹€ Load 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */

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

  useEffect(() => {
    // TODO: migrate to modelStatusCache
    api.getVideoConfig()
      .then(config => config && api.checkWhisperModel(config))
      .then(exists => setWhisperModelMissing(exists === false))
      .catch(() => {}); // Video feature may not be compiled
  }, []);

  /* 鈹€鈹€ File-change event listener 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */

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

  /* ── Progress clear on idle (from global store) ──────────────────── */

  useEffect(() => {
    if (!scanningAll && !rebuildingEmbeddings) {
      progressStore.update('batchProgress', null);
      progressStore.update('embedRebuildProgress', null);
    }
  }, [scanningAll, rebuildingEmbeddings]);

  useEffect(() => {
    const incomingBatchAction = (location.state as { pendingBatchAction?: BatchAction } | null)?.pendingBatchAction;
    if (!incomingBatchAction) {
      return;
    }

    setPendingBatchAction(incomingBatchAction);
    navigate(location.pathname, { replace: true, state: null });
  }, [location.pathname, location.state, navigate]);

  /* 鈹€鈹€ Watch toggle 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */

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

  /* 鈹€鈹€ Add 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */

  const resetForm = () => {
    setFormPath('');
    setFormInclude('**/*.md');
    setFormExclude('');
    setFormWatch(true);
    setFormIndexNow(true);
  };

  const handleAdd = async () => {
    if (!formPath.trim()) return;
    setAdding(true);
    try {
      const includeGlobs = parseTags(formInclude);
      const excludeGlobs = parseTags(formExclude);
      const newSource = await api.addSource(formPath.trim(), includeGlobs, excludeGlobs);
      toast.success(formIndexNow ? t('sources.autoIndexing') : t('sources.added'));
      resetForm();
      setShowAddModal(false);
      await loadSources();

      if (formWatch) {
        try {
          await api.startWatching(newSource.id);
          toast.success(t('sources.watcherStart'));
        } catch (e) {
          toast.error(String(e));
        }
      }

      if (!formIndexNow) {
        return;
      }

      // Explicit initial index: scan + embed in background when the user opts in.
      const sourceId = newSource.id;
      setIndexingIds((prev) => new Set(prev).add(sourceId));
      try {
        const scanResult = await api.scanSource(sourceId);
        toast.info(formatScanResult(scanResult, t));
        await loadSources();
        const embedResult = await api.embedSource(sourceId);
        toast.success(`${t('sources.indexingComplete')} ${formatEmbedResult(embedResult, t)}`);
      } catch (e) {
        toast.error(`${t('sources.scanError')}: ${String(e)}`);
      } finally {
        setIndexingIds((prev) => {
          const next = new Set(prev);
          next.delete(sourceId);
          return next;
        });
        await loadSources();
      }
    } catch (e) {
      toast.error(`${t('sources.addError')}: ${String(e)}`);
    } finally {
      setAdding(false);
    }
  };

  /* 鈹€鈹€ Delete 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */

  const handleDelete = (source: Source) => {
    setSources((prev) => prev.filter((s) => s.id !== source.id));
    undoableAction({
      message: t('sources.deleted'),
      undoLabel: t('common.undo'),
      onConfirm: async () => {
        try {
          await api.deleteSource(source.id);
        } catch (e) {
          toast.error(`${t('sources.deleteError')}: ${String(e)}`);
          await loadSources();
        }
      },
    });
  };

  /* 鈹€鈹€ Edit 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */

  const openEditModal = (source: Source) => {
    setEditTarget(source);
    setEditRootPath(source.rootPath);
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
      await api.updateSource(editTarget.id, includeGlobs, excludeGlobs, editWatch, editRootPath.trim());
      toast.success(t('sources.updated'));
      setEditTarget(null);
      await loadSources();
    } catch (e) {
      toast.error(`${t('sources.updateError')}: ${String(e)}`);
    } finally {
      setEditing(false);
    }
  };

  /* 鈹€鈹€ Scan 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */

  const handleScan = async (sourceId: string) => {
    setScanningId(sourceId);
    progressStore.update('scanProgress', null);
    try {
      const result = await api.scanSource(sourceId);
      toast.success(formatScanResult(result, t));
    } catch (e) {
      toast.error(`${t('sources.scanError')}: ${String(e)}`);
    } finally {
      setScanningId(null);
      progressStore.update('scanProgress', null);
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
      await loadSources();
    } catch (e) {
      toast.error(`${t('sources.scanAllError')}: ${String(e)}`);
    } finally {
      setScanningAll(false);
      progressStore.update('batchProgress', null);
      progressStore.update('embedRebuildProgress', null);
    }
  };

  /* 鈹€鈹€ Embed 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */

  const handleEmbed = async (sourceId: string) => {
    setEmbeddingId(sourceId);
    progressStore.update('scanProgress', null);
    try {
      const result = await api.embedSource(sourceId);
      toast.success(formatEmbedResult(result, t));
    } catch (e) {
      toast.error(`${t('sources.embedError')}: ${String(e)}`);
    } finally {
      setEmbeddingId(null);
      progressStore.update('scanProgress', null);
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
      progressStore.update('batchProgress', null);
      progressStore.update('embedRebuildProgress', null);
    }
  };

  const handleConfirmBatchAction = async () => {
    if (pendingBatchAction === 'scanAll') {
      await handleScanAll();
    } else if (pendingBatchAction === 'rebuildEmbeddings') {
      await handleRebuildEmbeddings();
    }
    setPendingBatchAction(null);
  };

  const pendingBatchActionKey = pendingBatchAction ?? 'scanAll';
  const pendingBatchActionLoading = pendingBatchAction === 'scanAll' ? scanningAll : rebuildingEmbeddings;

  /* 鈹€鈹€ Ask AI handler 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */

  const handleAskAI = (context: string, sourceIds?: string[]) => {
    const trimmed = context.trim();
    navigate('/chat', {
      state: trimmed
        ? {
            initialMessage: trimmed,
            sourceIds: sourceIds && sourceIds.length > 0 ? sourceIds : undefined,
          }
        : null,
    });
  };

  /* 鈹€鈹€ Loading skeleton 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */

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

  /* 鈹€鈹€ Render 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */

  return (
    <div className="flex h-full">
    <div className="mx-auto max-w-3xl p-6 flex-1 min-w-0 overflow-y-auto">
      {/* Header */}
      <div className="mb-6 flex items-center justify-between">
        <h2 className="text-lg font-semibold text-text-primary">{t('sources.title')}</h2>
        <div className="flex gap-2">
          <Button
            variant="secondary"
            size="sm"
            icon={<ScanSearch size={14} />}
            onClick={() => setPendingBatchAction('scanAll')}
            loading={scanningAll}
            disabled={sources.length === 0}
          >
            {t('sources.scanAll')}
          </Button>
          <Button
            variant="secondary"
            size="sm"
            icon={<RefreshCw size={14} />}
            onClick={() => setPendingBatchAction('rebuildEmbeddings')}
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
      {/* Batch progress bar */}
      {(scanningAll || rebuildingEmbeddings) && batchProgress && (
        <div className="mb-4 p-3 bg-surface-2 rounded-lg border border-border">
          <div className="flex items-center gap-2 text-sm text-muted mb-1">
            <RefreshCw size={14} className="animate-spin" />
            <span className="font-medium">
              {batchProgress.operation === 'scan-all'
                ? t('sources.scanningAll')
                : t('sources.rebuildingEmbeddings_progress')
              }
            </span>
            {batchProgress.sourceCount > 0 && (
              <span className="text-xs">
                ({t('sources.sourceProgress', { current: batchProgress.sourceIndex, total: batchProgress.sourceCount })})
              </span>
            )}
          </div>
          <div className="flex items-center gap-2 text-[11px] text-muted/70 mb-1">
            <span className="capitalize">{batchProgress.phase}</span>
            {batchProgress.total > 0 && (
              <span>{batchProgress.current}/{batchProgress.total}</span>
            )}
          </div>
          {batchProgress.currentFile && (
            <div className="text-[10px] text-muted/50 truncate mb-1">{batchProgress.currentFile}</div>
          )}
          {batchProgress.total > 0 && (
            <div className="w-full bg-surface-3 rounded h-1.5">
              <div
                className="bg-accent h-1.5 rounded transition-all duration-300"
                style={{ width: `${Math.min(100, (batchProgress.current / batchProgress.total) * 100)}%` }}
              />
            </div>
          )}
        </div>
      )}

      {whisperModelMissing && (
        <div className="flex items-center gap-3 p-3 rounded-lg bg-amber-500/10 border border-amber-500/20 text-amber-600 dark:text-amber-400 text-sm mb-4">
          <AlertTriangle className="h-4 w-4 shrink-0" />
          <span>{t('sources.whisperModelMissing')}</span>
          <Button
            variant="secondary"
            size="sm"
            className="ml-auto shrink-0"
            onClick={() => navigate('/settings?tab=models')}
          >
            {t('sources.goToSettings')}
          </Button>
        </div>
      )}

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
            {sources.map((source) => {
              const isActivelyScanning =
                scanningId === source.id ||
                embeddingId === source.id ||
                indexingIds.has(source.id);
              const isManuallyExpanded = expandedSourceIds.has(source.id);
              const expanded = isManuallyExpanded || isActivelyScanning;
              const activeDetailTab = sourceDetailTabs[source.id] ?? (isActivelyScanning ? 'overview' : 'files');
              const toggleExpanded = () => {
                setExpandedSourceIds((prev) => {
                  const next = new Set(prev);
                  if (next.has(source.id)) next.delete(source.id);
                  else next.add(source.id);
                  return next;
                });
              };
              return (
              <motion.div
                key={source.id}
                variants={listItem}
                layout="position"
                exit={{ opacity: 0, x: -20, transition: { duration: 0.2 } }}
                className="rounded-lg border border-border bg-surface-2 p-4 hover:border-border-hover transition-colors duration-fast"
              >
                {/* Summary row (always visible) */}
                <div className="flex items-start justify-between gap-3">
                  {/* Left: icon + info */}
                  <div className="flex items-start gap-3 min-w-0 flex-1">
                    <button
                      type="button"
                      onClick={toggleExpanded}
                      aria-expanded={expanded}
                      aria-label={expanded ? t('common.collapse') : t('common.expand')}
                      className="shrink-0 p-2 rounded-lg bg-accent-subtle text-accent mt-0.5 hover:bg-accent/20 transition-colors cursor-pointer"
                    >
                      <ChevronDown
                        size={14}
                        className={`transition-transform ${expanded ? 'rotate-180' : '-rotate-90'}`}
                      />
                    </button>
                    <div className="shrink-0 p-2 rounded-lg bg-accent-subtle text-accent mt-0.5">
                      {kindIcon(source.kind)}
                    </div>
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-2 mb-1 flex-wrap">
                        <p className="truncate text-sm font-medium text-text-primary font-mono">
                          {source.rootPath}
                        </p>
                        <Badge variant="default">{kindLabel(source.kind, t)}</Badge>
                        {indexingIds.has(source.id) && (
                          <Badge variant="info">
                            <RefreshCw size={10} className="animate-spin mr-1" />
                            {t('sources.indexingInProgress')}
                          </Badge>
                        )}
                        {!expanded && (
                          <span className={`text-[11px] ${source.watchEnabled ? 'text-green-500 font-medium' : 'text-text-tertiary'}`}>
                            {source.watchEnabled ? t('sources.watcherActive') : t('sources.watchOff')}
                          </span>
                        )}
                      </div>

                    </div>
                  </div>

                  {/* Right: actions (always visible so scan/re-index/delete remain reachable) */}
                  <div className="flex shrink-0 flex-wrap justify-end gap-1.5">
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
                      onClick={() => handleDelete(source)}
                    >
                      {t('common.delete')}
                    </Button>
                    <button
                      onClick={() => handleAskAI(`Tell me about the source at "${source.rootPath}". Include globs: ${source.includeGlobs.join(', ')}. Exclude globs: ${source.excludeGlobs.join(', ')}.`, [source.id])}
                      className="rounded-md p-1.5 text-accent hover:bg-accent/10 transition-colors cursor-pointer"
                      title={t('chat.askAboutThis')}
                    >
                      <BotMessageSquare size={14} />
                    </button>
                  </div>
                </div>

                <AnimatePresence initial={false}>
                  {expanded && (
                    <motion.div
                      key="source-detail"
                      className="overflow-hidden"
                      initial={shouldReduceMotion ? false : { opacity: 0, height: 0, y: -4 }}
                      animate={shouldReduceMotion ? { opacity: 1 } : { opacity: 1, height: 'auto', y: 0 }}
                      exit={shouldReduceMotion ? { opacity: 0 } : { opacity: 0, height: 0, y: -4 }}
                      transition={shouldReduceMotion ? { duration: 0 } : {
                        height: { duration: 0.2, ease: [0.16, 1, 0.3, 1] },
                        opacity: { duration: 0.14, ease: 'easeOut' },
                        y: { duration: 0.16, ease: 'easeOut' },
                      }}
                    >
                      <div className="mt-4 overflow-hidden rounded-lg border border-border bg-surface-1/60">
                        <div className="flex flex-col gap-2 border-b border-border bg-surface-2/80 px-3 py-2.5 lg:flex-row lg:items-center lg:justify-between">
                          <div className="flex min-w-0 items-center gap-2">
                            <FolderSearch size={15} className="shrink-0 text-accent" />
                            <div className="min-w-0">
                              <div className="truncate text-xs font-medium text-text-primary">数据源详情</div>
                              <div className="truncate text-[11px] text-text-tertiary">{source.rootPath}</div>
                            </div>
                          </div>

                          <div className="inline-flex w-fit rounded-md border border-border bg-surface-0 p-0.5">
                            {([
                              ['files', '文件', FolderSearch],
                              ['overview', '概览', Info],
                            ] as const).map(([tab, label, Icon]) => {
                              const selected = activeDetailTab === tab;
                              return (
                                <button
                                  key={tab}
                                  type="button"
                                  onClick={() => setSourceDetailTabs((prev) => ({ ...prev, [source.id]: tab }))}
                                  className={`inline-flex h-8 items-center gap-1.5 rounded px-3 text-xs transition-colors ${
                                    selected
                                      ? 'bg-surface-3 text-text-primary shadow-sm'
                                      : 'text-text-tertiary hover:bg-surface-2 hover:text-text-secondary'
                                  }`}
                                >
                                  <Icon size={13} />
                                  {label}
                                </button>
                              );
                            })}
                          </div>
                        </div>

                        {activeDetailTab === 'files' ? (
                          <div className="p-3">
                            <SourceFileTree sourceId={source.id} />
                          </div>
                        ) : (
                          <div className="grid gap-3 p-3 lg:grid-cols-[1.2fr_0.8fr]">
                            <div className="space-y-3">
                              {scanProgress && scanProgress.sourceId === source.id && isActivelyScanning && scanProgress.total > 0 && (
                                <div className="rounded-lg border border-border bg-surface-0 p-3">
                                  <div className="mb-2 flex items-center justify-between text-xs text-text-secondary">
                                    <span className="flex items-center gap-1.5">
                                      <RefreshCw size={12} className="animate-spin text-accent" />
                                      <span className="font-medium capitalize">{scanProgress.phase}</span>
                                      <span className="text-text-tertiary">{scanProgress.current}/{scanProgress.total}</span>
                                    </span>
                                    <span className="text-[11px] font-medium text-accent">
                                      {Math.round((scanProgress.current / scanProgress.total) * 100)}%
                                    </span>
                                  </div>
                                  {scanProgress.currentFile && (
                                    <div className="mb-2 truncate text-[11px] text-text-tertiary">
                                      {scanProgress.currentFile}
                                    </div>
                                  )}
                                  <div className="h-2 w-full rounded-full bg-surface-3">
                                    <div
                                      className="h-2 rounded-full bg-accent transition-all duration-300 ease-out"
                                      style={{ width: `${Math.min(100, (scanProgress.current / scanProgress.total) * 100)}%` }}
                                    />
                                  </div>
                                </div>
                              )}

                              {videoProcessing && (
                                <VideoProcessingProgress
                                  phase={videoProcessing.phase}
                                  progress={videoProcessing.progress}
                                  fileName={videoProcessing.fileName}
                                />
                              )}

                              <div className="rounded-lg border border-border bg-surface-0 p-3">
                                <div className="mb-2 text-xs font-medium text-text-primary">索引规则</div>
                                <div className="flex flex-wrap gap-1.5">
                                  {source.includeGlobs.map((g, i) => (
                                    <Badge key={i} variant="success">{g}</Badge>
                                  ))}
                                  {source.excludeGlobs.map((g, i) => (
                                    <Badge key={`e-${i}`} variant="danger">x {g}</Badge>
                                  ))}
                                </div>
                              </div>
                            </div>

                            <div className="rounded-lg border border-border bg-surface-0 p-3">
                              <div className="mb-2 text-xs font-medium text-text-primary">状态</div>
                              <div className="space-y-2 text-xs text-text-secondary">
                                <div className="flex items-center justify-between gap-3">
                                  <span>{t('sources.watch')}</span>
                                  <span className={source.watchEnabled ? 'font-medium text-green-500' : 'text-text-tertiary'}>
                                    {source.watchEnabled ? t('sources.watcherActive') : t('sources.watchOff')}
                                  </span>
                                </div>
                                <div className="flex items-center justify-between gap-3">
                                  <span>{t('sources.addedAt')}</span>
                                  <span className="text-text-tertiary">{new Date(source.createdAt).toLocaleDateString()}</span>
                                </div>
                                <div className="flex items-center justify-between gap-3">
                                  <span>类型</span>
                                  <span className="text-text-tertiary">{kindLabel(source.kind, t)}</span>
                                </div>
                              </div>
                            </div>
                          </div>
                        )}
                      </div>
                    </motion.div>
                  )}
                </AnimatePresence>
              </motion.div>
              );
            })}
          </AnimatePresence>
        </motion.div>
      )}

      {/* 鈹€鈹€ Add Source Modal 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */}
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
                  placeholder={t('sources.pathPlaceholder')}
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
                {t('sources.browse')}
              </Button>
            </div>
          </div>

          {/* Include globs */}
          <div>
            <label className="block text-xs font-medium text-text-secondary mb-1.5">{t('sources.addModal.includeGlobs')}</label>
            <TagInput
              value={formInclude}
              onChange={setFormInclude}
              presets={includePresets}
              showSelectAll
              placeholder={t('sources.addIncludePattern')}
            />
          </div>

          {/* Exclude globs */}
          <div>
            <label className="block text-xs font-medium text-text-secondary mb-1.5">{t('sources.addModal.excludeGlobs')}</label>
            <TagInput
              value={formExclude}
              onChange={setFormExclude}
              presets={EXCLUDE_PRESETS}
              showSelectAll
              placeholder={t('sources.addExcludePattern')}
            />
          </div>

          {/* Watch toggle */}
          <div className="flex items-center gap-2">
            <input
              type="checkbox"
              id="add-watch"
              checked={formWatch}
              onChange={(e) => setFormWatch(e.target.checked)}
              className="h-4 w-4 rounded border-border text-accent focus:ring-accent/30"
            />
            <label htmlFor="add-watch" className="text-xs font-medium text-text-secondary">
              {t('sources.editModal.watch')}
            </label>
          </div>

          <div className="flex items-center gap-2">
            <input
              type="checkbox"
              id="add-index-now"
              checked={formIndexNow}
              onChange={(e) => setFormIndexNow(e.target.checked)}
              className="h-4 w-4 rounded border-border text-accent focus:ring-accent/30"
            />
            <label htmlFor="add-index-now" className="text-xs font-medium text-text-secondary">
              添加后立即扫描并嵌入
            </label>
          </div>
        </div>
      </Modal>

      {/* 鈹€鈹€ Edit Source Modal 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ */}
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
              disabled={!editRootPath.trim()}
            >
              {t('common.save')}
            </Button>
          </>
        }
      >
        <div className="space-y-4">
          {/* Root path */}
          <div>
            <label className="block text-xs font-medium text-text-secondary mb-1.5">{t('sources.addModal.rootPath')}</label>
            <div className="flex gap-2">
              <div className="flex-1">
                <Input
                  value={editRootPath}
                  onChange={(e) => setEditRootPath(e.target.value)}
                  placeholder={t('sources.pathPlaceholder')}
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
                    setEditRootPath(typeof selected === 'string' ? selected : selected[0]);
                  }
                }}
              >
                <FolderSearch size={15} className="mr-1" />
                {t('sources.browse')}
              </Button>
            </div>
          </div>

          {/* Include globs */}
          <div>
            <label className="block text-xs font-medium text-text-secondary mb-1.5">{t('sources.addModal.includeGlobs')}</label>
            <TagInput
              value={editInclude}
              onChange={setEditInclude}
              presets={includePresets}
              showSelectAll
              placeholder={t('sources.addIncludePattern')}
            />
          </div>

          {/* Exclude globs */}
          <div>
            <label className="block text-xs font-medium text-text-secondary mb-1.5">{t('sources.addModal.excludeGlobs')}</label>
            <TagInput
              value={editExclude}
              onChange={setEditExclude}
              presets={EXCLUDE_PRESETS}
              showSelectAll
              placeholder={t('sources.addExcludePattern')}
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

      <ConfirmDialog
        open={pendingBatchAction !== null}
        onClose={() => {
          if (!pendingBatchActionLoading) {
            setPendingBatchAction(null);
          }
        }}
        onConfirm={handleConfirmBatchAction}
        title={t(`sources.batchConfirm.${pendingBatchActionKey}.title`)}
        message={t(`sources.batchConfirm.${pendingBatchActionKey}.message`)}
        confirmText={t(`sources.batchConfirm.${pendingBatchActionKey}.confirm`)}
        variant="warning"
        loading={pendingBatchActionLoading}
      />


    </div>

    </div>
  );
}


