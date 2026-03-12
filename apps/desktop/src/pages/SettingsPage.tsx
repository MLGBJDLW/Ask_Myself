import { useState, useEffect, useCallback, useRef } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { useBlocker } from 'react-router-dom';
import {
  Database,
  Shield,
  RefreshCw,
  Zap,
  Plus,
  Trash2,
  Save,
  Brain,
  CheckCircle,
  XCircle,
  Loader2,
  KeyRound,
  AlertTriangle,
  Bot,
  Star,
  Pencil,
  Settings2,
  X,
  ScanLine,
  Film,
  Blocks,
  Plug,
  ChevronLeft,
  ChevronRight,
  ChevronDown,
  ChevronUp,
  Mic,
  HardDrive,
} from 'lucide-react';
import { toast } from 'sonner';
import { listen } from '@tauri-apps/api/event';
import * as api from '../lib/api';
import type { IndexStats } from '../types/index-stats';
import type { PrivacyConfig, RedactRule } from '../types/privacy';
import type { EmbedderConfig } from '../types/embedder';
import type { AgentConfig, SaveAgentConfigInput, UserMemory } from '../types/conversation';
import type { ScanProgress, FtsProgress, DownloadProgress } from '../types/ingest';
import type { OcrConfig, OcrDownloadProgress } from '../types/ocr';
import type { VideoConfig, VideoDownloadProgress } from '../types/video';
import type { Skill, McpServer, McpToolInfo, SaveSkillInput, SaveMcpServerInput } from '../types/extensions';
import { useTranslation } from '../i18n';
import { ThemeSwitcher } from '../components/ui/ThemeSwitcher';
import { Button } from '../components/ui/Button';
import { Input } from '../components/ui/Input';
import { Badge } from '../components/ui/Badge';
import { ConfirmDialog } from '../components/ui/ConfirmDialog';
import { AgentConfigForm } from '../components/settings/AgentConfigForm';
import { SkillEditor } from '../components/settings/SkillEditor';
import { McpServerForm } from '../components/settings/McpServerForm';
import { PROVIDER_PRESETS, type ProviderPreset } from '../lib/providerPresets';
import { DEFAULT_SUBAGENT_TOOL_NAMES } from '../lib/subagentTools';
import { ModelCard } from '../components/settings/ModelCard';

/* ── Section wrapper ──────────────────────────────────────────────── */
function Section({
  icon,
  title,
  children,
  delay = 0,
}: {
  icon: React.ReactNode;
  title: string;
  children: React.ReactNode;
  delay?: number;
}) {
  return (
    <motion.section
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3, delay, ease: [0.16, 1, 0.3, 1] }}
      className="rounded-xl border border-border bg-surface-1 p-6"
    >
      <div className="mb-5 flex items-center gap-2.5">
        <span className="text-accent">{icon}</span>
        <h2 className="text-base font-semibold text-text-primary">{title}</h2>
      </div>
      {children}
    </motion.section>
  );
}

/* ── Stat card ────────────────────────────────────────────────────── */
function StatCard({ label, value }: { label: string; value: number | string }) {
  return (
    <div className="rounded-lg bg-surface-2 px-4 py-3">
      <p className="text-xs text-text-tertiary">{label}</p>
      <p className="mt-1 text-xl font-bold text-text-primary">{value}</p>
    </div>
  );
}

/* ── Helpers ───────────────────────────────────────────────────────── */
function estimateTokens(text: string): number {
  if (!text) return 0;
  let tokens = 0;
  for (let i = 0; i < text.length; i++) {
    tokens += text.charCodeAt(i) > 0x2fff ? 1.5 : 0.25;
  }
  return Math.ceil(tokens);
}

/* ── Settings page ────────────────────────────────────────────────── */
type SettingsTab = 'appearance' | 'models' | 'embedding' | 'index' | 'privacy' | 'providers' | 'ocr' | 'video' | 'extensions';
const MEMORY_CHAR_LIMIT = 240;
const TAB_STRIP_EDGE_EPSILON = 4;

export function SettingsPage() {
  const { t, locale, setLocale, availableLocales } = useTranslation();
  const tabStripRef = useRef<HTMLDivElement | null>(null);
  const extensionCopy = {
    toolCount: (count: number) => t('settings.extensions.toolCount', { count }),
    connectionFailed: t('settings.extensions.connectionFailed'),
    availableTools: t('settings.extensions.availableTools'),
    toggleTools: t('settings.extensions.toggleTools'),
  };
  const [activeTab, setActiveTab] = useState<SettingsTab>('embedding');
  const [dirtyTabs, setDirtyTabs] = useState<Set<string>>(new Set());
  const [pendingTab, setPendingTab] = useState<SettingsTab | null>(null);
  const [discardingTabChanges, setDiscardingTabChanges] = useState(false);
  const [showLeftTabIndicator, setShowLeftTabIndicator] = useState(false);
  const [showRightTabIndicator, setShowRightTabIndicator] = useState(false);
  const [providerFormDirty, setProviderFormDirty] = useState(false);
  const [skillEditorDirty, setSkillEditorDirty] = useState(false);
  const [mcpFormDirty, setMcpFormDirty] = useState(false);
  const hasDirtyTabs = dirtyTabs.size > 0;

  const updateTabStripIndicators = useCallback(() => {
    const element = tabStripRef.current;
    if (!element) return;

    const hasOverflow = element.scrollWidth - element.clientWidth > TAB_STRIP_EDGE_EPSILON;
    if (!hasOverflow) {
      setShowLeftTabIndicator(false);
      setShowRightTabIndicator(false);
      return;
    }

    setShowLeftTabIndicator(element.scrollLeft > TAB_STRIP_EDGE_EPSILON);
    setShowRightTabIndicator(
      element.scrollLeft + element.clientWidth < element.scrollWidth - TAB_STRIP_EDGE_EPSILON
    );
  }, []);

  const markDirty = useCallback((tab: string) => {
    setDirtyTabs((prev) => {
      if (prev.has(tab)) return prev;
      const next = new Set(prev);
      next.add(tab);
      return next;
    });
  }, []);

  const markClean = useCallback((tab: string) => {
    setDirtyTabs((prev) => {
      if (!prev.has(tab)) return prev;
      const next = new Set(prev);
      next.delete(tab);
      return next;
    });
  }, []);

  const settingsNavigationBlocker = useBlocker(
    useCallback(({
      currentLocation,
      nextLocation,
    }: {
      currentLocation: { pathname: string };
      nextLocation: { pathname: string };
    }) => {
      return (
        dirtyTabs.size > 0
        && currentLocation.pathname.startsWith('/settings')
        && nextLocation.pathname !== currentLocation.pathname
      );
    }, [dirtyTabs])
  );

  /* ── Index state ─────────────────────────────────────────────────── */
  const [stats, setStats] = useState<IndexStats | null>(null);
  const [rebuildLoading, setRebuildLoading] = useState(false);
  const [optimizeLoading, setOptimizeLoading] = useState(false);
  const [clearCacheLoading, setClearCacheLoading] = useState(false);
  const [ftsProgress, setFtsProgress] = useState<FtsProgress | null>(null);
  const [embedRebuildProgress, setEmbedRebuildProgress] = useState<ScanProgress | null>(null);

  const loadStats = useCallback(() => {
    api.getIndexStats().then(setStats).catch(() => {
      toast.error(t('settings.loadStatsError'));
    });
  }, []);

  useEffect(() => {
    loadStats();
  }, [loadStats]);

  /* ── FTS & rebuild progress listeners ───────────────────────────── */

  useEffect(() => {
    let cancelled = false;
    let unlistenFts: (() => void) | undefined;
    let unlistenRebuild: (() => void) | undefined;

    listen<FtsProgress>('batch:fts-progress', (event) => {
      if (cancelled) return;
      const p = event.payload;
      if (p.phase === 'complete') {
        setFtsProgress(null);
      } else {
        setFtsProgress(p);
      }
    }).then((fn) => {
      if (cancelled) { fn(); } else { unlistenFts = fn; }
    });

    listen<ScanProgress>('batch:rebuild-progress', (event) => {
      if (cancelled) return;
      setEmbedRebuildProgress(event.payload);
    }).then((fn) => {
      if (cancelled) { fn(); } else { unlistenRebuild = fn; }
    });

    return () => {
      cancelled = true;
      unlistenFts?.();
      unlistenRebuild?.();
    };
  }, []);

  const handleRebuild = async () => {
    setRebuildLoading(true);
    try {
      await api.rebuildIndex();
      toast.success(t('settings.indexRebuilt'));
      loadStats();
    } catch {
      toast.error(t('settings.indexRebuildError'));
    } finally {
      setRebuildLoading(false);
    }
  };

  const handleOptimize = async () => {
    setOptimizeLoading(true);
    try {
      await api.optimizeFtsIndex();
      toast.success(t('settings.ftsOptimized'));
    } catch {
      toast.error(t('settings.ftsOptimizeError'));
    } finally {
      setOptimizeLoading(false);
    }
  };

  const handleClearCache = async () => {
    setClearCacheLoading(true);
    try {
      const deleted = await api.clearAnswerCache();
      toast.success(t('settings.cacheClearedCount', { count: deleted }));
    } catch {
      toast.error(t('settings.clearCacheError'));
    } finally {
      setClearCacheLoading(false);
    }
  };

  /* ── Privacy state ───────────────────────────────────────────────── */
  const [privacyConfig, setPrivacyConfig] = useState<PrivacyConfig | null>(null);
  const [newPattern, setNewPattern] = useState('');
  const [newRule, setNewRule] = useState<RedactRule>({ name: '', pattern: '', replacement: '' });
  const [saveLoading, setSaveLoading] = useState(false);
  const [userMemories, setUserMemories] = useState<UserMemory[]>([]);
  const [newMemory, setNewMemory] = useState('');
  const [editingMemoryId, setEditingMemoryId] = useState<string | null>(null);
  const [editingMemoryDraft, setEditingMemoryDraft] = useState('');
  const [memoryLoading, setMemoryLoading] = useState(false);

  /* ── Embedding state ─────────────────────────────────────────────── */
  const [embedConfig, setEmbedConfig] = useState<EmbedderConfig | null>(null);
  const [localModelReady, setLocalModelReady] = useState<boolean | null>(null);
  const [downloadLoading, setDownloadLoading] = useState(false);
  const [downloadProgress, setDownloadProgress] = useState<DownloadProgress | null>(null);
  const [testLoading, setTestLoading] = useState(false);
  const [embedSaveLoading, setEmbedSaveLoading] = useState(false);
  const [rebuildEmbedLoading, setRebuildEmbedLoading] = useState(false);

  /* ── OCR state ────────────────────────────────────────────────────── */
  const [ocrConfig, setOcrConfig] = useState<OcrConfig | null>(null);
  const [ocrModelsExist, setOcrModelsExist] = useState<boolean | null>(null);
  const [ocrDownloading, setOcrDownloading] = useState(false);
  const [ocrProgress, setOcrProgress] = useState<OcrDownloadProgress | null>(null);
  const [ocrSaveLoading, setOcrSaveLoading] = useState(false);

  /* ── Video state ──────────────────────────────────────────────────── */
  const [videoConfig, setVideoConfig] = useState<VideoConfig | null>(null);
  const [whisperModelExists, setWhisperModelExists] = useState<boolean | null>(null);
  const [ffmpegAvailable, setFfmpegAvailable] = useState<boolean | null>(null);
  const [videoDownloading, setVideoDownloading] = useState(false);
  const [videoProgress, setVideoProgress] = useState<VideoDownloadProgress | null>(null);
  const [videoSaveLoading, setVideoSaveLoading] = useState(false);
  const [showAdvancedVideo, setShowAdvancedVideo] = useState(false);
  const [deleteModelConfirmOpen, setDeleteModelConfirmOpen] = useState(false);

  useEffect(() => {
    if (!rebuildEmbedLoading) {
      setEmbedRebuildProgress(null);
    }
  }, [rebuildEmbedLoading]);

  useEffect(() => {
    if (!downloadLoading) {
      setDownloadProgress(null);
      return;
    }
    const unlisten = listen<DownloadProgress>('model:download-progress', (event) => {
      setDownloadProgress(event.payload);
    });
    return () => { unlisten.then(fn => fn()); };
  }, [downloadLoading]);

  const loadEmbedConfig = useCallback(async () => {
    try {
      const cfg = await api.getEmbedderConfig();
      setEmbedConfig(cfg);
      if (cfg.provider === 'local') {
        api.checkLocalModel(cfg.localModel).then(setLocalModelReady).catch(() => setLocalModelReady(false));
      } else {
        setLocalModelReady(null);
      }
      return true;
    } catch (e) {
      console.error('Failed to load embedder config:', e);
      toast.error(t('settings.loadStatsError'));
      return false;
    }
  }, []);

  useEffect(() => {
    void loadEmbedConfig();
  }, [loadEmbedConfig]);

  useEffect(() => {
    if (embedConfig?.provider === 'local') {
      api.checkLocalModel(embedConfig.localModel).then(setLocalModelReady).catch(() => setLocalModelReady(false));
    }
  }, [embedConfig?.provider, embedConfig?.localModel]);

  const handleDownloadModel = async () => {
    if (!embedConfig) return;
    setDownloadLoading(true);
    try {
      await api.downloadLocalModel(embedConfig.localModel);
      setLocalModelReady(true);
      toast.success(t('settings.embeddingDownloaded'));
    } catch (e) {
      toast.error(t('settings.embeddingDownloadFail') + ': ' + String(e));
    } finally {
      setDownloadLoading(false);
    }
  };

  const handleTestConnection = async () => {
    if (!embedConfig) return;
    setTestLoading(true);
    try {
      const ok = await api.testApiConnection(embedConfig.apiKey, embedConfig.apiBaseUrl);
      if (ok) {
        toast.success(t('settings.embeddingTestSuccess'));
      } else {
        toast.error(t('settings.embeddingTestFail'));
      }
    } catch {
      toast.error(t('settings.embeddingTestFail'));
    } finally {
      setTestLoading(false);
    }
  };

  const handleSaveEmbedConfig = async () => {
    if (!embedConfig) return;
    setEmbedSaveLoading(true);
    try {
      await api.saveEmbedderConfig(embedConfig);
      markClean('embedding');
      toast.success(t('settings.privacySaved'));
    } catch {
      toast.error(t('settings.privacySaveError'));
    } finally {
      setEmbedSaveLoading(false);
    }
  };

  const handleRebuildEmbeddings = async () => {
    setRebuildEmbedLoading(true);
    try {
      await api.rebuildEmbeddings();
      toast.success(t('cmd.rebuildComplete'));
    } catch {
      toast.error(t('cmd.rebuildError'));
    } finally {
      setRebuildEmbedLoading(false);
    }
  };

  /* ── OCR effects & handlers ──────────────────────────────────────── */
  const loadOcrConfig = useCallback(async () => {
    try {
      const cfg = await api.getOcrConfig();
      setOcrConfig(cfg);
      api.checkOcrModels(cfg).then(setOcrModelsExist).catch(() => setOcrModelsExist(false));
      return true;
    } catch {
      toast.error(t('settings.ocrLoadError'));
      return false;
    }
  }, []);

  useEffect(() => {
    void loadOcrConfig();
  }, [loadOcrConfig]);

  useEffect(() => {
    if (!ocrDownloading) {
      setOcrProgress(null);
      return;
    }
    const unlisten = listen<OcrDownloadProgress>('ocr:download-progress', (event) => {
      setOcrProgress(event.payload);
    });
    return () => { unlisten.then(fn => fn()); };
  }, [ocrDownloading]);

  const handleDownloadOcrModels = async () => {
    if (!ocrConfig) return;
    setOcrDownloading(true);
    try {
      await api.downloadOcrModels(ocrConfig);
      setOcrModelsExist(true);
      toast.success(t('settings.ocrModelsDownloaded'));
    } catch (e) {
      toast.error(t('settings.ocrDownloadFail') + ': ' + String(e));
    } finally {
      setOcrDownloading(false);
    }
  };

  const handleSaveOcrConfig = async () => {
    if (!ocrConfig) return;
    setOcrSaveLoading(true);
    try {
      await api.saveOcrConfig(ocrConfig);
      markClean('ocr');
      toast.success(t('settings.ocrSaved'));
    } catch {
      toast.error(t('settings.ocrSaveError'));
    } finally {
      setOcrSaveLoading(false);
    }
  };

  /* ── Video effects & handlers ────────────────────────────────────── */
  const loadVideoConfig = useCallback(async () => {
    try {
      const cfg = await api.getVideoConfig();
      setVideoConfig(cfg);
      api.checkWhisperModel(cfg).then(setWhisperModelExists).catch(() => setWhisperModelExists(false));
      api.checkFfmpeg(cfg).then(setFfmpegAvailable).catch(() => setFfmpegAvailable(false));
      return true;
    } catch {
      return false;
    }
  }, []);

  useEffect(() => {
    void loadVideoConfig();
  }, [loadVideoConfig]);

  useEffect(() => {
    if (!videoDownloading) { setVideoProgress(null); return; }
    let cancelled = false;
    let unlistenFn: (() => void) | undefined;
    const setupListener = async () => {
      const fn = await listen<VideoDownloadProgress>('video:download-progress', (event) => {
        if (!cancelled) setVideoProgress(event.payload);
      });
      if (cancelled) {
        fn();
      } else {
        unlistenFn = fn;
      }
    };
    setupListener();
    return () => {
      cancelled = true;
      unlistenFn?.();
    };
  }, [videoDownloading]);

  const handleWhisperDownload = async () => {
    if (!videoConfig) return;
    setVideoDownloading(true);
    try {
      await api.downloadWhisperModel(videoConfig);
      setWhisperModelExists(true);
    } catch (e) {
      toast.error(t('settings.videoDownloadFail') + ': ' + String(e));
    } finally {
      setVideoDownloading(false);
    }
  };

  const handleWhisperDelete = async () => {
    try {
      await api.deleteWhisperModel();
      setWhisperModelExists(false);
      toast.success(t('settings.videoDeleteSuccess'));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setDeleteModelConfirmOpen(false);
    }
  };

  const handleVideoSave = async () => {
    if (!videoConfig) return;
    setVideoSaveLoading(true);
    try {
      await api.saveVideoConfig(videoConfig);
      markClean('video');
      const exists = await api.checkWhisperModel(videoConfig);
      setWhisperModelExists(exists);
      toast.success(t('settings.ocrSaved'));
    } catch {
      toast.error(t('settings.ocrSaveError'));
    } finally {
      setVideoSaveLoading(false);
    }
  };

  const loadPrivacyConfig = useCallback(async () => {
    try {
      const config = await api.getPrivacyConfig();
      setPrivacyConfig(config);
      return true;
    } catch {
      toast.error(t('settings.loadPrivacyError'));
      return false;
    }
  }, []);

  useEffect(() => {
    void loadPrivacyConfig();
  }, [loadPrivacyConfig]);

  const discardActiveTabChanges = useCallback(async () => {
    switch (activeTab) {
      case 'embedding': {
        const reloaded = await loadEmbedConfig();
        if (!reloaded) return false;
        break;
      }
      case 'privacy': {
        const reloaded = await loadPrivacyConfig();
        if (!reloaded) return false;
        break;
      }
      case 'ocr': {
        const reloaded = await loadOcrConfig();
        if (!reloaded) return false;
        break;
      }
      case 'video': {
        const reloaded = await loadVideoConfig();
        if (!reloaded) return false;
        break;
      }
      default:
        break;
    }

    markClean(activeTab);
    return true;
  }, [activeTab, loadEmbedConfig, loadOcrConfig, loadVideoConfig, loadPrivacyConfig, markClean]);

  const handleTabChange = useCallback((nextTab: SettingsTab) => {
    if (nextTab === activeTab) return;
    if (dirtyTabs.has(activeTab)) {
      setPendingTab(nextTab);
      return;
    }
    setActiveTab(nextTab);
  }, [activeTab, dirtyTabs]);

  const handleCancelPendingTabChange = useCallback(() => {
    if (discardingTabChanges) return;
    setPendingTab(null);
  }, [discardingTabChanges]);

  const handleConfirmPendingTabChange = useCallback(async () => {
    if (!pendingTab) return;

    setDiscardingTabChanges(true);
    const nextTab = pendingTab;
    const discarded = await discardActiveTabChanges();
    setDiscardingTabChanges(false);

    if (!discarded) return;

    setPendingTab(null);
    setActiveTab(nextTab);
  }, [discardActiveTabChanges, pendingTab]);

  const handleCancelBlockedNavigation = useCallback(() => {
    if (settingsNavigationBlocker.state === 'blocked') {
      settingsNavigationBlocker.reset();
    }
  }, [settingsNavigationBlocker]);

  const handleConfirmBlockedNavigation = useCallback(() => {
    if (settingsNavigationBlocker.state === 'blocked') {
      settingsNavigationBlocker.proceed();
    }
  }, [settingsNavigationBlocker]);

  useEffect(() => {
    if (pendingTab && !dirtyTabs.has(activeTab)) {
      setActiveTab(pendingTab);
      setPendingTab(null);
    }
  }, [activeTab, dirtyTabs, pendingTab]);

  useEffect(() => {
    if (settingsNavigationBlocker.state === 'blocked' && !hasDirtyTabs) {
      settingsNavigationBlocker.proceed();
    }
  }, [hasDirtyTabs, settingsNavigationBlocker]);

  useEffect(() => {
    if (!hasDirtyTabs) return;

    const handleBeforeUnload = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = '';
      return '';
    };

    window.addEventListener('beforeunload', handleBeforeUnload);
    return () => window.removeEventListener('beforeunload', handleBeforeUnload);
  }, [hasDirtyTabs]);

  useEffect(() => {
    const element = tabStripRef.current;
    if (!element) return;

    updateTabStripIndicators();
    element.addEventListener('scroll', updateTabStripIndicators, { passive: true });

    const resizeObserver = typeof ResizeObserver !== 'undefined'
      ? new ResizeObserver(() => updateTabStripIndicators())
      : null;

    resizeObserver?.observe(element);
    window.addEventListener('resize', updateTabStripIndicators);

    return () => {
      element.removeEventListener('scroll', updateTabStripIndicators);
      resizeObserver?.disconnect();
      window.removeEventListener('resize', updateTabStripIndicators);
    };
  }, [dirtyTabs, locale, updateTabStripIndicators]);

  useEffect(() => {
    if (providerFormDirty) {
      markDirty('providers');
      return;
    }

    markClean('providers');
  }, [markClean, markDirty, providerFormDirty]);

  useEffect(() => {
    if (skillEditorDirty || mcpFormDirty) {
      markDirty('extensions');
      return;
    }

    markClean('extensions');
  }, [markClean, markDirty, mcpFormDirty, skillEditorDirty]);

  const loadUserMemories = useCallback(async () => {
    try {
      const list = await api.listUserMemories();
      setUserMemories(list);
    } catch (e) {
      console.error('Failed to load user memories:', e);
    }
  }, []);

  useEffect(() => {
    loadUserMemories();
  }, [loadUserMemories]);

  const handleAddUserMemory = async () => {
    const trimmed = newMemory.trim();
    if (!trimmed) return;
    if (trimmed.length > MEMORY_CHAR_LIMIT) {
      toast.error(t('settings.memoryTooLong', { limit: String(MEMORY_CHAR_LIMIT) }));
      return;
    }
    setMemoryLoading(true);
    try {
      const created = await api.createUserMemory(trimmed);
      setUserMemories((prev) => [created, ...prev]);
      setNewMemory('');
      toast.success(t('settings.memorySaved'));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setMemoryLoading(false);
    }
  };

  const handleDeleteUserMemory = async (id: string) => {
    setMemoryLoading(true);
    try {
      await api.deleteUserMemory(id);
      setUserMemories((prev) => prev.filter((m) => m.id !== id));
      if (editingMemoryId === id) {
        setEditingMemoryId(null);
        setEditingMemoryDraft('');
      }
    } catch (e) {
      toast.error(String(e));
    } finally {
      setMemoryLoading(false);
    }
  };

  const handleStartEditUserMemory = (memory: UserMemory) => {
    setEditingMemoryId(memory.id);
    setEditingMemoryDraft(memory.content);
  };

  const handleCancelEditUserMemory = () => {
    setEditingMemoryId(null);
    setEditingMemoryDraft('');
  };

  const handleUpdateUserMemory = async () => {
    const id = editingMemoryId;
    const trimmed = editingMemoryDraft.trim();
    if (!id || !trimmed) return;
    if (trimmed.length > MEMORY_CHAR_LIMIT) {
      toast.error(t('settings.memoryTooLong', { limit: String(MEMORY_CHAR_LIMIT) }));
      return;
    }

    setMemoryLoading(true);
    try {
      const updated = await api.updateUserMemory(id, trimmed);
      setUserMemories((prev) => prev.map((m) => (m.id === updated.id ? updated : m)));
      setEditingMemoryId(null);
      setEditingMemoryDraft('');
      toast.success(t('settings.memoryUpdated'));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setMemoryLoading(false);
    }
  };

  const addPattern = () => {
    const trimmed = newPattern.trim();
    if (!trimmed || !privacyConfig) return;
    if (privacyConfig.excludePatterns.includes(trimmed)) {
      toast.error(t('settings.patternExists'));
      return;
    }
    setPrivacyConfig({
      ...privacyConfig,
      excludePatterns: [...privacyConfig.excludePatterns, trimmed],
    });
    markDirty('privacy');
    setNewPattern('');
  };

  const removePattern = (idx: number) => {
    if (!privacyConfig) return;
    setPrivacyConfig({
      ...privacyConfig,
      excludePatterns: privacyConfig.excludePatterns.filter((_, i) => i !== idx),
    });
    markDirty('privacy');
  };

  const addRule = () => {
    if (!newRule.name.trim() || !newRule.pattern.trim() || !privacyConfig) return;
    setPrivacyConfig({
      ...privacyConfig,
      redactPatterns: [...privacyConfig.redactPatterns, { ...newRule }],
    });
    markDirty('privacy');
    setNewRule({ name: '', pattern: '', replacement: '' });
  };

  const removeRule = (idx: number) => {
    if (!privacyConfig) return;
    setPrivacyConfig({
      ...privacyConfig,
      redactPatterns: privacyConfig.redactPatterns.filter((_, i) => i !== idx),
    });
    markDirty('privacy');
  };

  const handleSavePrivacy = async () => {
    if (!privacyConfig) return;
    setSaveLoading(true);
    try {
      await api.savePrivacyConfig(privacyConfig);
      markClean('privacy');
      toast.success(t('settings.privacySaved'));
    } catch {
      toast.error(t('settings.privacySaveError'));
    } finally {
      setSaveLoading(false);
    }
  };

  /* ── Extensions state ────────────────────────────────────────────── */
  const [skills, setSkills] = useState<Skill[]>([]);
  const [mcpServers, setMcpServers] = useState<McpServer[]>([]);
  const [editingSkill, setEditingSkill] = useState<Skill | null>(null);
  const [editingMcpServer, setEditingMcpServer] = useState<McpServer | null>(null);
  const [showSkillForm, setShowSkillForm] = useState(false);
  const [showMcpForm, setShowMcpForm] = useState(false);
  const [deleteSkillTarget, setDeleteSkillTarget] = useState<Skill | null>(null);
  const [deleteMcpTarget, setDeleteMcpTarget] = useState<McpServer | null>(null);
  const [mcpTestLoading, setMcpTestLoading] = useState<string | null>(null);
  const [mcpToolCounts, setMcpToolCounts] = useState<Record<string, { tools: McpToolInfo[]; loading: boolean; error?: string }>>({});
  const [mcpToolsExpanded, setMcpToolsExpanded] = useState<Record<string, boolean>>({});

  const loadSkills = useCallback(() => {
    api.listSkills().then(setSkills).catch(() => {
      toast.error(t('common.error'));
    });
  }, []);

  const loadMcpServers = useCallback(() => {
    api.listMcpServers().then(setMcpServers).catch(() => {
      toast.error(t('common.error'));
    });
  }, []);

  useEffect(() => {
    if (activeTab === 'extensions') {
      loadSkills();
      loadMcpServers();
    }
  }, [activeTab, loadSkills, loadMcpServers]);

  const handleSaveSkill = async (input: SaveSkillInput) => {
    try {
      await api.saveSkill(input);
      toast.success(t('common.success'));
      setSkillEditorDirty(false);
      setShowSkillForm(false);
      setEditingSkill(null);
      loadSkills();
    } catch {
      toast.error(t('common.error'));
    }
  };

  const handleDeleteSkill = async () => {
    if (!deleteSkillTarget) return;
    try {
      await api.deleteSkill(deleteSkillTarget.id);
      toast.success(t('common.success'));
      setDeleteSkillTarget(null);
      loadSkills();
    } catch {
      toast.error(t('common.error'));
    }
  };

  const handleToggleSkill = async (id: string, enabled: boolean) => {
    try {
      await api.toggleSkill(id, enabled);
      setSkills((prev) => prev.map((s) => s.id === id ? { ...s, enabled } : s));
    } catch {
      toast.error(t('common.error'));
    }
  };

  const handleSaveMcpServer = async (input: SaveMcpServerInput) => {
    try {
      const saved = await api.saveMcpServer(input);
      toast.success(t('common.success'));
      setMcpFormDirty(false);
      setShowMcpForm(false);
      setEditingMcpServer(null);
      setMcpToolCounts((prev) => {
        const next = { ...prev };
        delete next[saved.id];
        return next;
      });
      loadMcpServers();
      if (saved.enabled) {
        void fetchMcpTools(saved.id);
      }
    } catch {
      toast.error(t('common.error'));
    }
  };

  const handleDeleteMcpServer = async () => {
    if (!deleteMcpTarget) return;
    try {
      await api.deleteMcpServer(deleteMcpTarget.id);
      toast.success(t('common.success'));
      setMcpToolCounts((prev) => {
        const next = { ...prev };
        delete next[deleteMcpTarget.id];
        return next;
      });
      setMcpToolsExpanded((prev) => {
        const next = { ...prev };
        delete next[deleteMcpTarget.id];
        return next;
      });
      setDeleteMcpTarget(null);
      loadMcpServers();
    } catch {
      toast.error(t('common.error'));
    }
  };

  const handleToggleMcpServer = async (id: string, enabled: boolean) => {
    try {
      await api.toggleMcpServer(id, enabled);
      setMcpServers((prev) => prev.map((s) => s.id === id ? { ...s, enabled } : s));
      if (!enabled) {
        setMcpToolCounts((prev) => {
          const next = { ...prev };
          delete next[id];
          return next;
        });
        setMcpToolsExpanded((prev) => ({ ...prev, [id]: false }));
      } else {
        void fetchMcpTools(id);
      }
    } catch {
      toast.error(t('common.error'));
    }
  };

  const handleTestMcpServer = async (id: string) => {
    setMcpTestLoading(id);
    try {
      const tools = await api.testMcpServer(id);
      toast.success(t('settings.mcpTestSuccess', { count: String(tools.length) }));
      setMcpToolCounts((prev) => ({ ...prev, [id]: { tools, loading: false } }));
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setMcpToolCounts((prev) => ({ ...prev, [id]: { tools: [], loading: false, error: msg } }));
      toast.error(`${t('settings.mcpTestFailed')}: ${msg}`, { duration: 8000 });
    } finally {
      setMcpTestLoading(null);
    }
  };

  const fetchMcpTools = useCallback(async (id: string) => {
    setMcpToolCounts((prev) => ({ ...prev, [id]: { tools: prev[id]?.tools ?? [], loading: true } }));
    try {
      const tools = await api.listMcpTools(id);
      setMcpToolCounts((prev) => ({ ...prev, [id]: { tools, loading: false } }));
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setMcpToolCounts((prev) => ({ ...prev, [id]: { tools: [], loading: false, error: msg } }));
    }
  }, []);

  // Auto-fetch tools for enabled servers when tab is opened
  useEffect(() => {
    if (activeTab !== 'extensions') return;
    mcpServers.filter((s) => s.enabled).forEach((s) => {
      if (!mcpToolCounts[s.id]) fetchMcpTools(s.id);
    });
  }, [mcpServers, activeTab, fetchMcpTools, mcpToolCounts]);

  /* ── AI Providers state ──────────────────────────────────────────── */
  const [agentConfigs, setAgentConfigs] = useState<AgentConfig[]>([]);
  type ProviderView = 'list' | 'selector' | 'form';
  const [providerView, setProviderView] = useState<ProviderView>('list');
  const [selectedPreset, setSelectedPreset] = useState<ProviderPreset | null>(null);
  const [editingConfig, setEditingConfig] = useState<AgentConfig | undefined>(undefined);
  const [agentSaveLoading, setAgentSaveLoading] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<AgentConfig | null>(null);
  const [deleteLoading, setDeleteLoading] = useState(false);

  const loadAgentConfigs = useCallback(() => {
    api.listAgentConfigs().then(setAgentConfigs).catch((e) => {
      console.error('Failed to load AI provider configs:', e);
      toast.error(t('settings.loadStatsError'));
    });
  }, []);

  useEffect(() => {
    loadAgentConfigs();
  }, [loadAgentConfigs]);

  const handleSaveAgent = async (input: SaveAgentConfigInput) => {
    setAgentSaveLoading(true);
    try {
      await api.saveAgentConfig(input);
      toast.success(t('settings.providerSaved'));
      setProviderFormDirty(false);
      setProviderView('list');
      setEditingConfig(undefined);
      setSelectedPreset(null);
      loadAgentConfigs();
    } catch {
      toast.error(t('common.error'));
    } finally {
      setAgentSaveLoading(false);
    }
  };

  const handleDeleteAgent = async () => {
    if (!deleteTarget) return;
    setDeleteLoading(true);
    try {
      await api.deleteAgentConfig(deleteTarget.id);
      toast.success(t('settings.providerDeleted'));
      setDeleteTarget(null);
      loadAgentConfigs();
    } catch {
      toast.error(t('common.error'));
    } finally {
      setDeleteLoading(false);
    }
  };

  const handleSetDefault = async (id: string) => {
    try {
      await api.setDefaultAgentConfig(id);
      toast.success(t('settings.defaultSet'));
      loadAgentConfigs();
    } catch {
      toast.error(t('common.error'));
    }
  };

  const PROVIDER_LABELS: Record<string, string> = {
    open_ai: t('settings.providerOpenAI'),
    anthropic: t('settings.providerAnthropic'),
    google: t('settings.providerGoogle'),
    deep_seek: t('settings.providerDeepSeek'),
    ollama: t('settings.providerOllama'),
    lm_studio: t('settings.providerLMStudio'),
    azure_open_ai: t('settings.providerAzure'),
    custom: t('settings.providerCustom'),
  };

  const tabs: { id: SettingsTab; label: string; icon: React.ReactNode }[] = [
    { id: 'appearance', label: t('settings.appearance'), icon: <Star size={16} /> },
    { id: 'models', label: t('settings.models'), icon: <HardDrive size={16} /> },
    { id: 'embedding', label: t('settings.embeddingSection'), icon: <Brain size={16} /> },
    { id: 'providers', label: t('settings.aiProviders'), icon: <Bot size={16} /> },
    { id: 'ocr', label: t('settings.ocrTab'), icon: <ScanLine size={16} /> },
    { id: 'video', label: t('settings.videoTab'), icon: <Film size={16} /> },
    { id: 'index', label: t('settings.indexSection'), icon: <Database size={16} /> },
    { id: 'privacy', label: t('settings.privacySection'), icon: <Shield size={16} /> },
    { id: 'extensions', label: t('settings.extensionsTab'), icon: <Blocks size={16} /> },
  ];

  /* ── Render ──────────────────────────────────────────────────────── */
  return (
    <div className="mx-auto max-w-3xl space-y-6 p-6">
      {/* Header */}
      <motion.div
        initial={{ opacity: 0, y: -8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.25 }}
      >
        <h1 className="text-xl font-bold text-text-primary">{t('settings.title')}</h1>
        <p className="mt-1 text-sm text-text-secondary">{t('settings.subtitle')}</p>
      </motion.div>

      {/* Tab Navigation */}
      <div className="relative">
        <div
          ref={tabStripRef}
          className="flex gap-1 rounded-lg border border-border bg-surface-1 p-1 overflow-x-auto"
        >
          {tabs.map((tab) => (
            <button
              key={tab.id}
              onClick={() => handleTabChange(tab.id)}
              className={`flex items-center gap-1.5 rounded-md px-3 py-2 text-xs font-medium transition-all duration-fast cursor-pointer whitespace-nowrap ${
                activeTab === tab.id
                  ? 'bg-accent text-white shadow-sm'
                  : 'text-text-tertiary hover:text-text-secondary hover:bg-surface-2'
              }`}
            >
              {tab.icon}
              {tab.label}
              {dirtyTabs.has(tab.id) && (
                <span className="w-1.5 h-1.5 rounded-full bg-warning" />
              )}
            </button>
          ))}
        </div>

        <AnimatePresence initial={false}>
          {showLeftTabIndicator && (
            <motion.div
              key="settings-tab-strip-left-indicator"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={{ duration: 0.18 }}
              className="pointer-events-none absolute inset-y-1 left-px flex w-12 items-center justify-start rounded-l-lg pl-2"
              style={{ background: 'linear-gradient(90deg, var(--color-surface-1) 45%, transparent 100%)' }}
              aria-hidden="true"
            >
              <ChevronLeft size={14} className="text-text-secondary/80" />
            </motion.div>
          )}
        </AnimatePresence>

        <AnimatePresence initial={false}>
          {showRightTabIndicator && (
            <motion.div
              key="settings-tab-strip-right-indicator"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={{ duration: 0.18 }}
              className="pointer-events-none absolute inset-y-1 right-px flex w-12 items-center justify-end rounded-r-lg pr-2"
              style={{ background: 'linear-gradient(270deg, var(--color-surface-1) 45%, transparent 100%)' }}
              aria-hidden="true"
            >
              <ChevronRight size={14} className="text-text-secondary/80" />
            </motion.div>
          )}
        </AnimatePresence>
      </div>

      {/* ── Tab: Appearance ─────────────────────────────────────── */}
      {activeTab === 'appearance' && (
        <Section icon={<Star size={20} />} title={t('settings.appearance')} delay={0.03}>
          <div className="space-y-6">
            {/* Theme section */}
            <div>
              <p className="mb-2 text-sm font-medium text-text-primary">{t('settings.appearance.theme')}</p>
              <p className="mb-3 text-xs text-text-tertiary">{t('settings.appearance.theme.description')}</p>
              <ThemeSwitcher />
            </div>

            {/* Separator */}
            <div className="border-t border-border" />

            {/* Language section */}
            <div>
              <p className="mb-2 text-sm font-medium text-text-primary">{t('settings.appearance.language')}</p>
              <p className="mb-3 text-xs text-text-tertiary">{t('settings.appearance.language.description')}</p>
              <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-5 gap-2">
                {availableLocales.map((l) => (
                  <button
                    key={l.code}
                    onClick={() => setLocale(l.code)}
                    className={`rounded-lg border px-3 py-2.5 text-sm font-medium transition-all duration-fast cursor-pointer ${
                      locale === l.code
                        ? 'border-accent bg-accent-subtle text-accent ring-1 ring-accent/20'
                        : 'border-border bg-surface-2 text-text-secondary hover:border-border-hover hover:bg-surface-3'
                    }`}
                  >
                    {l.name}
                  </button>
                ))}
              </div>
            </div>
          </div>
        </Section>
      )}

      {/* ── Tab: Models ─────────────────────────────────────────── */}
      {activeTab === 'models' && (
        <Section icon={<HardDrive size={20} />} title={t('settings.models')} delay={0.03}>
          <p className="mb-5 text-xs text-text-tertiary">{t('settings.modelsDesc')}</p>
          <div className="space-y-4">
            {/* Embedding Model */}
            <ModelCard
              title={t('settings.modelsEmbedding')}
              icon={<Brain size={18} />}
              description={t('settings.modelsEmbeddingDesc')}
              status={
                downloadLoading ? 'downloading'
                : localModelReady === null ? 'checking'
                : localModelReady ? 'downloaded'
                : embedConfig?.provider !== 'local' ? 'downloaded'
                : 'not-downloaded'
              }
              size={embedConfig?.localModel === 'MultilingualE5Base' ? '~470 MB' : '~46 MB'}
              onDownload={handleDownloadModel}
              downloadProgress={downloadProgress}
            >
              {embedConfig?.provider === 'local' && (
                <div className="space-y-3">
                  <p className="text-sm font-medium text-text-primary">{t('settings.embeddingLocalModelSelect')}</p>
                  <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                    {([
                      {
                        id: 'MultilingualMiniLM' as const,
                        label: t('settings.embeddingModelLight'),
                        desc: t('settings.embeddingModelLightDesc'),
                      },
                      {
                        id: 'MultilingualE5Base' as const,
                        label: t('settings.embeddingModelQuality'),
                        desc: t('settings.embeddingModelQualityDesc'),
                      },
                    ]).map((opt) => (
                      <button
                        key={opt.id}
                        onClick={() => {
                          if (embedConfig) {
                            setEmbedConfig({ ...embedConfig, localModel: opt.id });
                            setLocalModelReady(null);
                            markDirty('embedding');
                          }
                        }}
                        className={`rounded-lg border p-3 text-left transition-all duration-fast cursor-pointer ${
                          embedConfig?.localModel === opt.id
                            ? 'border-accent bg-accent-subtle ring-1 ring-accent/20'
                            : 'border-border bg-surface-1 hover:border-border-hover hover:bg-surface-3/50'
                        }`}
                      >
                        <div className="text-sm font-medium text-text-primary">{opt.label}</div>
                        <div className="mt-1 text-xs text-text-tertiary">{opt.desc}</div>
                      </button>
                    ))}
                  </div>
                  <div className="flex items-start gap-2 rounded-lg border border-info/30 bg-info/5 p-2">
                    <AlertTriangle size={14} className="mt-0.5 shrink-0 text-info" />
                    <p className="text-xs text-info">{t('settings.embeddingModelChangeWarning')}</p>
                  </div>
                </div>
              )}
            </ModelCard>

            {/* OCR Model */}
            <ModelCard
              title={t('settings.modelsOcr')}
              icon={<ScanLine size={18} />}
              description={t('settings.modelsOcrDesc')}
              status={
                ocrDownloading ? 'downloading'
                : ocrModelsExist === null ? 'checking'
                : ocrModelsExist ? 'downloaded'
                : 'not-downloaded'
              }
              size={t('settings.ocrModelSize')}
              onDownload={handleDownloadOcrModels}
              downloadProgress={ocrProgress ? {
                filename: ocrProgress.filename,
                bytesDownloaded: ocrProgress.bytesDownloaded,
                totalBytes: ocrProgress.totalBytes ?? null,
                fileIndex: ocrProgress.fileIndex,
                totalFiles: ocrProgress.totalFiles,
              } : null}
            />

            {/* Whisper Model */}
            <ModelCard
              title={t('settings.modelsWhisper')}
              icon={<Mic size={18} />}
              description={t('settings.modelsWhisperDesc')}
              status={
                videoDownloading ? 'downloading'
                : whisperModelExists === null ? 'checking'
                : whisperModelExists ? 'downloaded'
                : 'not-downloaded'
              }
              size={
                videoConfig?.whisperModel === 'tiny' ? '~39 MB'
                : videoConfig?.whisperModel === 'base' ? '~142 MB'
                : videoConfig?.whisperModel === 'small' ? '~466 MB'
                : videoConfig?.whisperModel === 'medium' ? '~1.5 GB'
                : videoConfig?.whisperModel === 'large' ? '~3.1 GB'
                : videoConfig?.whisperModel === 'large_turbo' ? '~1.6 GB'
                : undefined
              }
              onDownload={handleWhisperDownload}
              downloadProgress={videoProgress ? {
                filename: videoProgress.filename,
                bytesDownloaded: videoProgress.bytesDownloaded,
                totalBytes: videoProgress.totalBytes ?? null,
                fileIndex: 0,
                totalFiles: 1,
              } : null}
            >
              {videoConfig && (
                <div className="space-y-3">
                  <p className="text-sm font-medium text-text-primary">{t('settings.videoWhisperModel')}</p>
                  <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                    {([
                      { id: 'tiny' as const, label: t('settings.videoModelTiny'), desc: t('settings.videoModelTinyDesc') },
                      { id: 'base' as const, label: t('settings.videoModelBase'), desc: t('settings.videoModelBaseDesc') },
                      { id: 'small' as const, label: t('settings.videoModelSmall'), desc: t('settings.videoModelSmallDesc') },
                      { id: 'medium' as const, label: t('settings.videoModelMedium'), desc: t('settings.videoModelMediumDesc') },
                      { id: 'large' as const, label: t('settings.videoModelLarge'), desc: t('settings.videoModelLargeDesc') },
                      { id: 'large_turbo' as const, label: t('settings.videoModelLargeTurbo'), desc: t('settings.videoModelLargeTurboDesc') },
                    ]).map((opt) => (
                      <button
                        key={opt.id}
                        onClick={() => {
                          setVideoConfig({ ...videoConfig, whisperModel: opt.id });
                          setWhisperModelExists(null);
                          markDirty('video');
                        }}
                        className={`rounded-lg border p-3 text-left transition-all duration-fast cursor-pointer ${
                          videoConfig.whisperModel === opt.id
                            ? 'border-accent bg-accent-subtle ring-1 ring-accent/20'
                            : 'border-border bg-surface-1 hover:border-border-hover hover:bg-surface-3/50'
                        }`}
                      >
                        <div className="text-sm font-medium text-text-primary">{opt.label}</div>
                        <div className="mt-1 text-xs text-text-tertiary">{opt.desc}</div>
                      </button>
                    ))}
                  </div>
                  <div className="flex items-start gap-2 rounded-lg border border-info/30 bg-info/5 p-2">
                    <AlertTriangle size={14} className="mt-0.5 shrink-0 text-info" />
                    <p className="text-xs text-info">{t('settings.videoModelChangeWarning')}</p>
                  </div>
                </div>
              )}
            </ModelCard>

            {/* Disk Usage Summary */}
            <div className="rounded-lg border border-border p-4 bg-surface-1">
              <h4 className="text-sm font-medium text-text-primary mb-2">{t('settings.modelDiskUsage')}</h4>
              <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-text-tertiary">
                <span className="flex items-center gap-1.5">
                  <span className="h-2 w-2 rounded-full bg-accent" />
                  {t('settings.modelsEmbedding')}: {embedConfig?.localModel === 'MultilingualE5Base' ? '~470 MB' : '~46 MB'}
                </span>
                <span className="flex items-center gap-1.5">
                  <span className="h-2 w-2 rounded-full bg-success" />
                  {t('settings.modelsOcr')}: ~16 MB
                </span>
                <span className="flex items-center gap-1.5">
                  <span className="h-2 w-2 rounded-full bg-warning" />
                  {t('settings.modelsWhisper')}: {
                    videoConfig?.whisperModel === 'tiny' ? '~39 MB'
                    : videoConfig?.whisperModel === 'base' ? '~142 MB'
                    : videoConfig?.whisperModel === 'small' ? '~466 MB'
                    : videoConfig?.whisperModel === 'medium' ? '~1.5 GB'
                    : videoConfig?.whisperModel === 'large' ? '~3.1 GB'
                    : videoConfig?.whisperModel === 'large_turbo' ? '~1.6 GB'
                    : '—'
                  }
                </span>
              </div>
            </div>
          </div>
        </Section>
      )}

      {/* ── Tab: Embedding ──────────────────────────────────────── */}
      {activeTab === 'embedding' && (
      <Section icon={<Brain size={20} />} title={t('settings.embeddingSection')} delay={0.03}>
        {embedConfig && (
          <div className="space-y-5">
            {/* Provider pills */}
            <div>
              <p className="mb-2 text-sm font-medium text-text-primary">{t('settings.embeddingProvider')}</p>
              <div className="inline-flex rounded-full border border-border bg-surface-1 p-0.5">
                {(['local', 'api', 'tfidf'] as const).map((p) => (
                  <button
                    key={p}
                    onClick={() => { setEmbedConfig({ ...embedConfig, provider: p }); markDirty('embedding'); }}
                    className={`rounded-full px-4 py-1.5 text-xs font-medium transition-all duration-fast cursor-pointer ${
                      embedConfig.provider === p
                        ? 'bg-accent text-white shadow-sm'
                        : 'text-text-tertiary hover:text-text-secondary'
                    }`}
                  >
                    {p === 'local' ? t('settings.embeddingLocal') : p === 'api' ? t('settings.embeddingApi') : t('settings.embeddingTfidf')}
                  </button>
                ))}
              </div>
            </div>

            {/* Local model status */}
            {embedConfig.provider === 'local' && (
              <div className="rounded-lg border border-border bg-surface-2 p-4 space-y-3">
                <div className="flex items-center justify-between">
                  <p className="text-sm font-medium text-text-primary">{t('settings.embeddingLocalModel')}</p>
                  <div className="flex items-center gap-2 text-sm">
                    {localModelReady === null ? (
                      <Loader2 size={14} className="animate-spin text-text-tertiary" />
                    ) : localModelReady ? (
                      <Badge variant="default" className="gap-1">
                        <CheckCircle size={12} className="text-success" />
                        {t('settings.embeddingDownloaded')}
                      </Badge>
                    ) : (
                      <Badge variant="default" className="gap-1">
                        <XCircle size={12} className="text-danger" />
                        {t('settings.embeddingNotDownloaded')}
                      </Badge>
                    )}
                  </div>
                </div>
                <p className="text-xs text-text-tertiary">
                  {embedConfig.localModel === 'MultilingualE5Base' ? t('settings.embeddingModelQuality') : t('settings.embeddingModelLight')}
                </p>
              </div>
            )}

            {/* API panel */}
            {embedConfig.provider === 'api' && (
              <div className="rounded-lg border border-border bg-surface-2 p-4 space-y-3">
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.embeddingApiKey')}</label>
                  <div className="relative">
                    <KeyRound size={14} className="absolute left-3 top-1/2 -translate-y-1/2 text-text-tertiary" />
                    <Input
                      type="password"
                      value={embedConfig.apiKey}
                      onChange={(e) => { setEmbedConfig({ ...embedConfig, apiKey: e.target.value }); markDirty('embedding'); }}
                      className="pl-9"
                      placeholder="sk-..."
                    />
                  </div>
                </div>
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.embeddingBaseUrl')}</label>
                  <Input
                    value={embedConfig.apiBaseUrl}
                    onChange={(e) => { setEmbedConfig({ ...embedConfig, apiBaseUrl: e.target.value }); markDirty('embedding'); }}
                    placeholder="https://api.openai.com/v1"
                  />
                </div>
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.embeddingModel')}</label>
                  <Input
                    value={embedConfig.apiModel}
                    onChange={(e) => { setEmbedConfig({ ...embedConfig, apiModel: e.target.value }); markDirty('embedding'); }}
                    placeholder="text-embedding-3-small"
                  />
                </div>
                <Button
                  variant="secondary"
                  size="sm"
                  icon={testLoading ? <Loader2 size={14} className="animate-spin" /> : <Zap size={14} />}
                  loading={testLoading}
                  onClick={handleTestConnection}
                  disabled={!embedConfig.apiKey.trim() || !embedConfig.apiBaseUrl.trim()}
                >
                  {t('settings.embeddingTestConnection')}
                </Button>
              </div>
            )}

            {/* TF-IDF warning */}
            {embedConfig.provider === 'tfidf' && (
              <div className="flex items-start gap-2 rounded-lg border border-warning/30 bg-warning/5 p-3">
                <AlertTriangle size={16} className="mt-0.5 shrink-0 text-warning" />
                <p className="text-sm text-warning">{t('settings.embeddingTfidfWarning')}</p>
              </div>
            )}

            {/* Provider change warning + actions */}
            <div className="space-y-3 border-t border-border pt-4">
              <div className="flex items-start gap-2 rounded-lg border border-warning/30 bg-warning/5 p-3">
                <AlertTriangle size={16} className="mt-0.5 shrink-0 text-warning" />
                <p className="text-sm text-warning">{t('settings.embeddingProviderChangeWarning')}</p>
              </div>
              <div className="flex items-center gap-3">
                <Button
                  variant="primary"
                  size="md"
                  icon={<Save size={16} />}
                  loading={embedSaveLoading}
                  onClick={handleSaveEmbedConfig}
                >
                  {t('settings.embeddingSave')}
                </Button>
                <Button
                  variant="secondary"
                  size="md"
                  icon={rebuildEmbedLoading ? <Loader2 size={16} className="animate-spin" /> : <RefreshCw size={16} />}
                  loading={rebuildEmbedLoading}
                  onClick={handleRebuildEmbeddings}
                >
                  {rebuildEmbedLoading ? t('settings.embeddingRebuilding') : t('settings.embeddingRebuild')}
                </Button>
              </div>
              {embedRebuildProgress && (
                <div className="mt-2">
                  <div className="flex items-center gap-2 text-xs text-muted">
                    <RefreshCw size={12} className="animate-spin" />
                    <span>{embedRebuildProgress.current}/{embedRebuildProgress.total}</span>
                  </div>
                  {embedRebuildProgress.total > 0 && (
                    <div className="w-full bg-surface-3 rounded h-1 mt-1">
                      <div
                        className="bg-accent h-1 rounded transition-all duration-300"
                        style={{ width: `${Math.min(100, (embedRebuildProgress.current / embedRebuildProgress.total) * 100)}%` }}
                      />
                    </div>
                  )}
                </div>
              )}
            </div>
          </div>
        )}
      </Section>
      )}

      {/* ── Tab: AI Providers ──────────────────────────────────────── */}
      {activeTab === 'providers' && (
      <Section icon={<Bot size={20} />} title={t('settings.aiProviders')} delay={0.03}>
        {providerView === 'form' ? (
          <AgentConfigForm
            config={editingConfig}
            preset={editingConfig ? undefined : selectedPreset}
            onSave={handleSaveAgent}
            onCancel={() => {
              setProviderFormDirty(false);
              setProviderView('list');
              setEditingConfig(undefined);
              setSelectedPreset(null);
            }}
            isSaving={agentSaveLoading}
            onDirtyChange={setProviderFormDirty}
          />
        ) : providerView === 'selector' ? (
          <div className="space-y-4">
            <div className="flex items-center justify-between">
              <h3 className="text-lg font-medium text-text-primary">{t('settings.selectProvider')}</h3>
              <button
                onClick={() => setProviderView('list')}
                className="flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-sm text-text-tertiary hover:text-text-secondary hover:bg-surface-3/50 transition-colors cursor-pointer"
              >
                <X size={16} /> {t('common.cancel')}
              </button>
            </div>
            <div className="grid grid-cols-2 gap-3">
              {PROVIDER_PRESETS.map(preset => (
                <button
                  key={preset.id}
                  onClick={() => { setSelectedPreset(preset); setProviderView('form'); }}
                  className="flex items-start gap-3 rounded-lg border border-border bg-surface-2 p-4 text-left transition-colors duration-fast hover:border-accent hover:bg-surface-3/50 cursor-pointer"
                >
                  <span className="text-2xl">{preset.icon}</span>
                  <div>
                    <div className="font-medium text-text-primary">{preset.name}</div>
                    <div className="text-sm text-text-tertiary">{preset.description}</div>
                  </div>
                </button>
              ))}
              {/* Custom / Manual option */}
              <button
                onClick={() => { setSelectedPreset(null); setProviderView('form'); }}
                className="flex items-start gap-3 rounded-lg border border-dashed border-border bg-surface-2 p-4 text-left transition-colors duration-fast hover:border-accent hover:bg-surface-3/50 cursor-pointer"
              >
                <Settings2 className="mt-0.5 text-text-tertiary" size={24} />
                <div>
                  <div className="font-medium text-text-primary">{t('settings.customProvider')}</div>
                  <div className="text-sm text-text-tertiary">{t('settings.customProviderDesc')}</div>
                </div>
              </button>
            </div>
          </div>
        ) : (
          <div className="space-y-4">
            {/* Add button */}
            <div className="flex justify-end">
              <Button
                variant="primary"
                size="sm"
                icon={<Plus size={14} />}
                onClick={() => { setEditingConfig(undefined); setSelectedPreset(null); setProviderView('selector'); }}
              >
                {t('settings.addProvider')}
              </Button>
            </div>

            {/* Config list */}
            {agentConfigs.length === 0 ? (
              <div className="py-8 text-center">
                <Bot size={32} className="mx-auto mb-3 text-text-tertiary" />
                <p className="text-sm font-medium text-text-secondary">{t('settings.noProviders')}</p>
                <p className="mt-1 text-xs text-text-tertiary">{t('settings.noProvidersDesc')}</p>
              </div>
            ) : (
              <div className="space-y-3">
                {agentConfigs.map((cfg) => (
                  <div
                    key={cfg.id}
                    className="flex items-center justify-between rounded-lg border border-border bg-surface-2 p-4 transition-colors hover:bg-surface-3/50"
                  >
                    <div className="flex items-center gap-3 min-w-0">
                      {cfg.isDefault && (
                        <Star size={14} className="shrink-0 fill-warning text-warning" />
                      )}
                      <div className="min-w-0">
                        <div className="flex items-center gap-2">
                          <p className="text-sm font-medium text-text-primary truncate">{cfg.name}</p>
                          <Badge variant="default" className="text-[10px] shrink-0">
                            {PROVIDER_LABELS[cfg.provider] ?? cfg.provider}
                          </Badge>
                          <Badge variant="default" className="text-[10px] shrink-0 bg-accent/10 text-accent border-accent/20">
                            {`subagents ${(cfg.subagentAllowedTools ?? DEFAULT_SUBAGENT_TOOL_NAMES).length}`}
                          </Badge>
                        </div>
                        <p className="mt-0.5 text-xs text-text-tertiary truncate">
                          {cfg.model}
                          {cfg.baseUrl ? ` · ${cfg.baseUrl}` : ''}
                        </p>
                      </div>
                    </div>

                    <div className="flex items-center gap-1 shrink-0 ml-3">
                      {!cfg.isDefault && (
                        <button
                          onClick={() => handleSetDefault(cfg.id)}
                          className="rounded p-1.5 text-text-tertiary hover:text-warning hover:bg-warning/10 transition-colors cursor-pointer"
                          aria-label={t('settings.setDefault')}
                          title={t('settings.setDefault')}
                        >
                          <Star size={14} />
                        </button>
                      )}
                      <button
                        onClick={() => { setEditingConfig(cfg); setProviderView('form'); }}
                        className="rounded p-1.5 text-text-tertiary hover:text-accent hover:bg-accent/10 transition-colors cursor-pointer"
                        aria-label={t('common.edit')}
                        title={t('common.edit')}
                      >
                        <Pencil size={14} />
                      </button>
                      <button
                        onClick={() => setDeleteTarget(cfg)}
                        className="rounded p-1.5 text-text-tertiary hover:text-danger hover:bg-danger/10 transition-colors cursor-pointer"
                        aria-label={t('common.delete')}
                        title={t('common.delete')}
                      >
                        <Trash2 size={14} />
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
      </Section>
      )}

      {/* Delete confirm dialog */}
      <ConfirmDialog
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        onConfirm={handleDeleteAgent}
        title={t('settings.deleteProvider')}
        message={t('settings.deleteProviderConfirm')}
        confirmText={t('common.delete')}
        variant="danger"
        loading={deleteLoading}
      />

      {/* ── Tab: Index ─────────────────────────────────────────────── */}
      {activeTab === 'index' && (
      <Section icon={<Database size={20} />} title={t('settings.indexSection')} delay={0.05}>
        {/* Stats grid */}
        <div className="mb-5 grid grid-cols-3 gap-3">
          <StatCard label={t('settings.totalDocs')} value={stats?.totalDocuments ?? '—'} />
          <StatCard label={t('settings.totalChunks')} value={stats?.totalChunks ?? '—'} />
          <StatCard label={t('settings.ftsEntries')} value={stats?.ftsRows ?? '—'} />
        </div>

        {/* Actions */}
        <div className="flex items-center gap-3">
          <Button
            variant="secondary"
            size="sm"
            icon={<RefreshCw size={14} />}
            loading={rebuildLoading}
            onClick={handleRebuild}
          >
            {t('settings.rebuildIndex')}
          </Button>
          <Button
            variant="secondary"
            size="sm"
            icon={<Zap size={14} />}
            loading={optimizeLoading}
            onClick={handleOptimize}
          >
            {t('settings.optimizeIndex')}
          </Button>
          <Button
            variant="secondary"
            size="sm"
            icon={<Trash2 size={14} />}
            loading={clearCacheLoading}
            onClick={handleClearCache}
          >
            {t('settings.clearCache')}
          </Button>
        </div>
        {ftsProgress && (
          <div className="mt-2">
            <div className="flex items-center gap-2 text-xs text-muted">
              <RefreshCw size={12} className="animate-spin" />
              <span>{ftsProgress.operation === 'rebuild-fts' ? t('settings.rebuildingIndex') : t('settings.optimizingIndex')}</span>
            </div>
            <div className="w-full bg-surface-3 rounded h-1 mt-1 overflow-hidden">
              <div className="bg-accent h-1 rounded animate-pulse w-full" />
            </div>
          </div>
        )}
      </Section>
      )}

      {/* ── Tab: Privacy ───────────────────────────────────────────── */}
      {activeTab === 'privacy' && (
      <Section icon={<Shield size={20} />} title={t('settings.privacySection')} delay={0.1}>
        {privacyConfig && (
          <div className="space-y-6">
            {/* Exclude patterns */}
            <div>
              <h3 className="mb-2 text-sm font-medium text-text-primary">{t('settings.excludePatterns')}</h3>
              <p className="mb-3 text-xs text-text-tertiary">{t('settings.excludePatternsDesc')}</p>

              {/* Pattern chips */}
              {privacyConfig.excludePatterns.length > 0 && (
                <div className="mb-3 flex flex-wrap gap-2">
                  {privacyConfig.excludePatterns.map((pat, i) => (
                    <Badge key={i} variant="default" className="gap-1.5 pl-2.5 pr-1.5 py-1">
                      <span className="font-mono text-[11px]">{pat}</span>
                      <button
                        onClick={() => removePattern(i)}
                        className="ml-0.5 rounded hover:bg-surface-4 p-0.5 text-text-tertiary hover:text-danger transition-colors cursor-pointer"
                        aria-label={`${t('common.remove')} ${pat}`}
                      >
                        <Trash2 size={12} />
                      </button>
                    </Badge>
                  ))}
                </div>
              )}

              {/* Add pattern */}
              <div className="flex gap-2">
                <Input
                  placeholder="*.log, .git/**, node_modules/**"
                  value={newPattern}
                  onChange={(e) => setNewPattern(e.target.value)}
                  onKeyDown={(e) => { if (e.key === 'Enter') addPattern(); }}
                  className="flex-1"
                />
                <Button
                  variant="ghost"
                  size="md"
                  icon={<Plus size={16} />}
                  onClick={addPattern}
                  disabled={!newPattern.trim()}
                >
                  {t('settings.addPattern')}
                </Button>
              </div>
            </div>

            {/* Redaction rules */}
            <div>
              <h3 className="mb-2 text-sm font-medium text-text-primary">{t('settings.redactRules')}</h3>
              <p className="mb-3 text-xs text-text-tertiary">{t('settings.redactRulesDesc')}</p>

              {/* Rules table */}
              {privacyConfig.redactPatterns.length > 0 && (
                <div className="mb-3 overflow-hidden rounded-lg border border-border">
                  <table className="w-full text-sm">
                    <thead>
                      <tr className="border-b border-border bg-surface-2">
                        <th className="px-3 py-2 text-left text-xs font-medium text-text-tertiary">{t('settings.ruleName')}</th>
                        <th className="px-3 py-2 text-left text-xs font-medium text-text-tertiary">{t('settings.rulePattern')}</th>
                        <th className="px-3 py-2 text-left text-xs font-medium text-text-tertiary">{t('settings.ruleReplacement')}</th>
                        <th className="w-10 px-3 py-2" />
                      </tr>
                    </thead>
                    <tbody>
                      {privacyConfig.redactPatterns.map((rule, i) => (
                        <tr
                          key={i}
                          className="border-b border-border last:border-0 hover:bg-surface-2/50 transition-colors"
                        >
                          <td className="px-3 py-2 text-text-primary">{rule.name}</td>
                          <td className="px-3 py-2 font-mono text-xs text-text-secondary">{rule.pattern}</td>
                          <td className="px-3 py-2 font-mono text-xs text-text-secondary">{rule.replacement}</td>
                          <td className="px-3 py-2 text-right">
                            <button
                              onClick={() => removeRule(i)}
                              className="rounded p-1 text-text-tertiary hover:text-danger hover:bg-danger/10 transition-colors cursor-pointer"
                              aria-label={`${t('common.delete')} ${rule.name}`}
                            >
                              <Trash2 size={14} />
                            </button>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}

              {/* Add rule form */}
              <div className="flex gap-2">
                <Input
                  placeholder={t('settings.ruleName')}
                  value={newRule.name}
                  onChange={(e) => setNewRule({ ...newRule, name: e.target.value })}
                  className="flex-1"
                />
                <Input
                  placeholder={t('settings.rulePattern')}
                  value={newRule.pattern}
                  onChange={(e) => setNewRule({ ...newRule, pattern: e.target.value })}
                  className="flex-1"
                />
                <Input
                  placeholder={t('settings.ruleReplacement')}
                  value={newRule.replacement}
                  onChange={(e) => setNewRule({ ...newRule, replacement: e.target.value })}
                  className="flex-1"
                />
                <Button
                  variant="ghost"
                  size="md"
                  icon={<Plus size={16} />}
                  onClick={addRule}
                  disabled={!newRule.name.trim() || !newRule.pattern.trim()}
                >
                  {t('settings.addRule')}
                </Button>
              </div>
            </div>

            {/* Local user memory */}
            <div>
              <h3 className="mb-2 text-sm font-medium text-text-primary">{t('settings.memorySection')}</h3>
              <p className="mb-3 text-xs text-text-tertiary">
                {t('settings.memoryDescription')}
              </p>

              <div className="space-y-2 mb-3">
                {userMemories.length === 0 && (
                  <div className="rounded-md border border-dashed border-border px-3 py-2 text-xs text-text-tertiary">
                    {t('settings.memoryEmpty')}
                  </div>
                )}
                {userMemories.map((memory) => (
                  <div key={memory.id} className="flex items-start gap-2 rounded-md border border-border bg-surface-2 px-3 py-2">
                    {editingMemoryId === memory.id ? (
                      <div className="flex-1 space-y-2">
                        <Input
                          value={editingMemoryDraft}
                          onChange={(e) => setEditingMemoryDraft(e.target.value)}
                          maxLength={MEMORY_CHAR_LIMIT}
                          disabled={memoryLoading}
                          className="w-full"
                        />
                        <div className="flex items-center justify-between gap-2">
                          <p className="text-xs text-text-tertiary">
                            {editingMemoryDraft.length}/{MEMORY_CHAR_LIMIT}
                          </p>
                          <div className="flex items-center gap-1">
                            <button
                              type="button"
                              onClick={handleUpdateUserMemory}
                              disabled={!editingMemoryDraft.trim() || memoryLoading}
                              className="rounded p-1 text-text-tertiary hover:text-accent hover:bg-accent/10 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                              aria-label={t('common.save')}
                            >
                              <Save size={14} />
                            </button>
                            <button
                              type="button"
                              onClick={handleCancelEditUserMemory}
                              disabled={memoryLoading}
                              className="rounded p-1 text-text-tertiary hover:text-text-primary hover:bg-surface-3 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                              aria-label={t('common.cancel')}
                            >
                              <X size={14} />
                            </button>
                          </div>
                        </div>
                      </div>
                    ) : (
                      <>
                        <p className="flex-1 text-sm text-text-primary whitespace-pre-wrap break-words">
                          {memory.content}
                        </p>
                        <div className="flex items-center gap-1">
                          <button
                            type="button"
                            onClick={() => handleStartEditUserMemory(memory)}
                            disabled={memoryLoading}
                            className="mt-0.5 rounded p-1 text-text-tertiary hover:text-accent hover:bg-accent/10 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                            aria-label={t('common.edit')}
                          >
                            <Pencil size={14} />
                          </button>
                          <button
                            type="button"
                            onClick={() => handleDeleteUserMemory(memory.id)}
                            disabled={memoryLoading}
                            className="mt-0.5 rounded p-1 text-text-tertiary hover:text-danger hover:bg-danger/10 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                            aria-label={t('common.delete')}
                          >
                            <Trash2 size={14} />
                          </button>
                        </div>
                      </>
                    )}
                  </div>
                ))}
              </div>

              <div className="flex gap-2">
                <Input
                  placeholder={t('settings.memoryPlaceholder')}
                  value={newMemory}
                  onChange={(e) => setNewMemory(e.target.value)}
                  maxLength={MEMORY_CHAR_LIMIT}
                  onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); void handleAddUserMemory(); } }}
                  className="flex-1"
                />
                <Button
                  variant="ghost"
                  size="md"
                  icon={<Plus size={16} />}
                  onClick={handleAddUserMemory}
                  loading={memoryLoading}
                  disabled={!newMemory.trim()}
                >
                  {t('settings.addMemory')}
                </Button>
              </div>
              <p className="mt-2 text-xs text-text-tertiary">
                {t('settings.memoryCharHelper', { length: String(newMemory.length), limit: String(MEMORY_CHAR_LIMIT) })}
              </p>
            </div>

            {/* Save button */}
            <div className="flex justify-end border-t border-border pt-4">
              <Button
                variant="primary"
                size="md"
                icon={<Save size={16} />}
                loading={saveLoading}
                onClick={handleSavePrivacy}
              >
                {t('settings.saveConfig')}
              </Button>
            </div>
          </div>
        )}
      </Section>
      )}

      {/* ── Tab: OCR ──────────────────────────────────────────────── */}
      {activeTab === 'ocr' && (
      <Section icon={<ScanLine size={20} />} title={t('settings.ocrSection')} delay={0.03}>
        {ocrConfig && (
          <div className="space-y-5">
            {/* Enable toggle */}
            <div className="flex items-center justify-between">
              <div>
                <p className="text-sm font-medium text-text-primary">{t('settings.ocrEnabled')}</p>
                <p className="text-xs text-text-tertiary">{t('settings.ocrEnabledDesc')}</p>
              </div>
              <button
                onClick={() => { setOcrConfig({ ...ocrConfig, enabled: !ocrConfig.enabled }); markDirty('ocr'); }}
                className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors duration-fast cursor-pointer ${
                  ocrConfig.enabled ? 'bg-accent' : 'bg-surface-3'
                }`}
              >
                <span
                  className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform duration-fast ${
                    ocrConfig.enabled ? 'translate-x-6' : 'translate-x-1'
                  }`}
                />
              </button>
            </div>

            {/* Confidence threshold */}
            <div>
              <div className="flex items-center justify-between mb-1">
                <p className="text-sm font-medium text-text-primary">{t('settings.ocrConfidence')}</p>
                <span className="text-xs font-mono text-text-tertiary">{ocrConfig.confidenceThreshold.toFixed(2)}</span>
              </div>
              <p className="text-xs text-text-tertiary mb-2">{t('settings.ocrConfidenceDesc')}</p>
              <input
                type="range"
                min="0"
                max="1"
                step="0.05"
                value={ocrConfig.confidenceThreshold}
                onChange={(e) => { setOcrConfig({ ...ocrConfig, confidenceThreshold: parseFloat(e.target.value) }); markDirty('ocr'); }}
                className="w-full accent-accent"
              />
            </div>

            {/* LLM fallback toggle */}
            <div className="flex items-center justify-between">
              <div>
                <p className="text-sm font-medium text-text-primary">{t('settings.ocrLlmFallback')}</p>
                <p className="text-xs text-text-tertiary">{t('settings.ocrLlmFallbackDesc')}</p>
              </div>
              <button
                onClick={() => { setOcrConfig({ ...ocrConfig, llmFallbackEnabled: !ocrConfig.llmFallbackEnabled }); markDirty('ocr'); }}
                className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors duration-fast cursor-pointer ${
                  ocrConfig.llmFallbackEnabled ? 'bg-accent' : 'bg-surface-3'
                }`}
              >
                <span
                  className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform duration-fast ${
                    ocrConfig.llmFallbackEnabled ? 'translate-x-6' : 'translate-x-1'
                  }`}
                />
              </button>
            </div>

            {/* Detection size limit */}
            <div>
              <p className="text-sm font-medium text-text-primary mb-1">{t('settings.ocrDetLimit')}</p>
              <p className="text-xs text-text-tertiary mb-2">{t('settings.ocrDetLimitDesc')}</p>
              <Input
                type="number"
                value={ocrConfig.detLimitSideLen}
                onChange={(e) => { setOcrConfig({ ...ocrConfig, detLimitSideLen: parseInt(e.target.value) || 960 }); markDirty('ocr'); }}
                className="w-32"
              />
            </div>

            {/* Orientation detection toggle */}
            <div className="flex items-center justify-between">
              <div>
                <p className="text-sm font-medium text-text-primary">{t('settings.ocrUseCls')}</p>
                <p className="text-xs text-text-tertiary">{t('settings.ocrUseClsDesc')}</p>
              </div>
              <button
                onClick={() => { setOcrConfig({ ...ocrConfig, useCls: !ocrConfig.useCls }); markDirty('ocr'); }}
                className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors duration-fast cursor-pointer ${
                  ocrConfig.useCls ? 'bg-accent' : 'bg-surface-3'
                }`}
              >
                <span
                  className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform duration-fast ${
                    ocrConfig.useCls ? 'translate-x-6' : 'translate-x-1'
                  }`}
                />
              </button>
            </div>

            {/* Save button */}
            <div className="flex justify-end pt-2">
              <Button
                variant="primary"
                size="md"
                icon={<Save size={16} />}
                loading={ocrSaveLoading}
                onClick={handleSaveOcrConfig}
              >
                {t('settings.saveConfig')}
              </Button>
            </div>
          </div>
        )}
      </Section>
      )}

      {/* ── Tab: Video ─────────────────────────────────────────── */}
      {activeTab === 'video' && (
      <Section icon={<Film size={20} />} title={t('settings.videoSection')} delay={0.03}>
        {videoConfig && (
          <div className="space-y-5">
            {/* Enable toggle */}
            <div className="flex items-center justify-between">
              <div>
                <p className="text-sm font-medium text-text-primary">{t('settings.videoEnabled')}</p>
                <p className="text-xs text-text-tertiary">{t('settings.videoEnabledDesc')}</p>
              </div>
              <button
                onClick={() => { setVideoConfig({ ...videoConfig, enabled: !videoConfig.enabled }); markDirty('video'); }}
                className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors duration-fast cursor-pointer ${
                  videoConfig.enabled ? 'bg-accent' : 'bg-surface-3'
                }`}
              >
                <span
                  className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform duration-fast ${
                    videoConfig.enabled ? 'translate-x-6' : 'translate-x-1'
                  }`}
                />
              </button>
            </div>

            {/* FFmpeg Status */}
            <div className="flex items-center gap-2 text-sm">
              <span className="text-text-secondary">{t('settings.videoFfmpegStatus')}:</span>
              {ffmpegAvailable === null ? (
                <Loader2 size={14} className="animate-spin text-text-tertiary" />
              ) : ffmpegAvailable ? (
                <Badge variant="default" className="gap-1">
                  <CheckCircle size={12} className="text-success" />
                  {t('settings.videoFfmpegAvailable')}
                </Badge>
              ) : (
                <Badge variant="default" className="gap-1">
                  <XCircle size={12} className="text-danger" />
                  {t('settings.videoFfmpegNotFound')}
                </Badge>
              )}
            </div>
            {ffmpegAvailable === false && (
              <div className="flex items-start gap-2 rounded-lg border border-warning/30 bg-warning/5 p-2">
                <AlertTriangle size={14} className="mt-0.5 shrink-0 text-warning" />
                <p className="text-xs text-warning">{t('settings.videoFfmpegHint')}</p>
              </div>
            )}

            {/* Language */}
            <div>
              <p className="text-sm font-medium text-text-primary mb-1">{t('settings.videoLanguage')}</p>
              <p className="text-xs text-text-tertiary mb-2">{t('settings.videoLanguageDesc')}</p>
              <Input
                type="text"
                value={videoConfig.language ?? ''}
                onChange={(e) => { setVideoConfig({ ...videoConfig, language: e.target.value || null }); markDirty('video'); }}
                placeholder="en, zh, ja..."
                className="w-40"
              />
            </div>

            {/* Translate to English */}
            <div className="flex items-center justify-between">
              <div>
                <p className="text-sm font-medium text-text-primary">{t('settings.videoTranslate')}</p>
                <p className="text-xs text-text-tertiary">{t('settings.videoTranslateDesc')}</p>
              </div>
              <button
                onClick={() => { setVideoConfig({ ...videoConfig, translateToEnglish: !videoConfig.translateToEnglish }); markDirty('video'); }}
                className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors duration-fast cursor-pointer ${
                  videoConfig.translateToEnglish ? 'bg-accent' : 'bg-surface-3'
                }`}
              >
                <span
                  className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform duration-fast ${
                    videoConfig.translateToEnglish ? 'translate-x-6' : 'translate-x-1'
                  }`}
                />
              </button>
            </div>

            {/* Frame Extraction */}
            <div className="flex items-center justify-between">
              <div>
                <p className="text-sm font-medium text-text-primary">{t('settings.videoFrameExtraction')}</p>
                <p className="text-xs text-text-tertiary">{t('settings.videoFrameExtractionDesc')}</p>
              </div>
              <button
                onClick={() => { setVideoConfig({ ...videoConfig, frameExtractionEnabled: !videoConfig.frameExtractionEnabled }); markDirty('video'); }}
                className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors duration-fast cursor-pointer ${
                  videoConfig.frameExtractionEnabled ? 'bg-accent' : 'bg-surface-3'
                }`}
              >
                <span
                  className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform duration-fast ${
                    videoConfig.frameExtractionEnabled ? 'translate-x-6' : 'translate-x-1'
                  }`}
                />
              </button>
            </div>

            {/* Frame Interval */}
            {videoConfig.frameExtractionEnabled && (
              <div>
                <p className="text-sm font-medium text-text-primary mb-1">{t('settings.videoFrameInterval')}</p>
                <p className="text-xs text-text-tertiary mb-2">{t('settings.videoFrameIntervalDesc')}</p>
                <Input
                  type="number"
                  value={videoConfig.frameIntervalSecs}
                  onChange={(e) => { setVideoConfig({ ...videoConfig, frameIntervalSecs: parseInt(e.target.value) || 30 }); markDirty('video'); }}
                  className="w-32"
                />
              </div>
            )}

            {/* Advanced Settings - collapsible */}
            <div className="space-y-4 border-t border-border pt-4 mt-4">
              <button
                type="button"
                onClick={() => setShowAdvancedVideo(!showAdvancedVideo)}
                className="flex items-center gap-2 text-sm font-medium text-text-primary cursor-pointer w-full"
              >
                <Settings2 size={16} />
                {t('settings.videoAdvanced')}
                <ChevronDown
                  size={16}
                  className={`transition-transform duration-fast ${showAdvancedVideo ? 'rotate-180' : ''}`}
                />
              </button>

              {showAdvancedVideo && (
                <div className="space-y-4 pl-4">
                  {/* GPU Acceleration */}
                  <div className="flex items-center justify-between">
                    <div>
                      <p className="text-sm font-medium text-text-primary">{t('settings.videoGpu')}</p>
                      <p className="text-xs text-text-tertiary">{t('settings.videoGpuDesc')}</p>
                    </div>
                    <button
                      onClick={() => { setVideoConfig({ ...videoConfig, useGpu: !videoConfig.useGpu }); markDirty('video'); }}
                      className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors duration-fast cursor-pointer ${
                        videoConfig.useGpu ? 'bg-accent' : 'bg-surface-3'
                      }`}
                    >
                      <span
                        className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform duration-fast ${
                          videoConfig.useGpu ? 'translate-x-6' : 'translate-x-1'
                        }`}
                      />
                    </button>
                  </div>

                  {/* Prefer Embedded Subtitles */}
                  <div className="flex items-center justify-between">
                    <div>
                      <p className="text-sm font-medium text-text-primary">{t('settings.videoPreferSubtitles')}</p>
                      <p className="text-xs text-text-tertiary">{t('settings.videoPreferSubtitlesDesc')}</p>
                    </div>
                    <button
                      onClick={() => { setVideoConfig({ ...videoConfig, preferEmbeddedSubtitles: !videoConfig.preferEmbeddedSubtitles }); markDirty('video'); }}
                      className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors duration-fast cursor-pointer ${
                        videoConfig.preferEmbeddedSubtitles ? 'bg-accent' : 'bg-surface-3'
                      }`}
                    >
                      <span
                        className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform duration-fast ${
                          videoConfig.preferEmbeddedSubtitles ? 'translate-x-6' : 'translate-x-1'
                        }`}
                      />
                    </button>
                  </div>

                  {/* Scene Detection Threshold */}
                  <div>
                    <p className="text-sm font-medium text-text-primary mb-1">{t('settings.videoSceneThreshold')}</p>
                    <p className="text-xs text-text-tertiary mb-2">{t('settings.videoSceneThresholdDesc')}</p>
                    <div className="flex items-center gap-3">
                      <input
                        type="range"
                        min={10}
                        max={90}
                        step={5}
                        value={Math.round(videoConfig.sceneThreshold * 100)}
                        onChange={(e) => { setVideoConfig({ ...videoConfig, sceneThreshold: parseInt(e.target.value) / 100 }); markDirty('video'); }}
                        className="flex-1 accent-accent"
                      />
                      <span className="text-xs text-text-secondary w-10 text-right">{videoConfig.sceneThreshold.toFixed(2)}</span>
                    </div>
                  </div>

                  {/* Beam Size */}
                  <div>
                    <p className="text-sm font-medium text-text-primary mb-1">{t('settings.videoBeamSize')}</p>
                    <p className="text-xs text-text-tertiary mb-2">{t('settings.videoBeamSizeDesc')}</p>
                    <div className="flex items-center gap-3">
                      <input
                        type="range"
                        min={1}
                        max={10}
                        step={1}
                        value={videoConfig.beamSize}
                        onChange={(e) => { setVideoConfig({ ...videoConfig, beamSize: parseInt(e.target.value) }); markDirty('video'); }}
                        className="flex-1 accent-accent"
                      />
                      <span className="text-xs text-text-secondary w-6 text-right">{videoConfig.beamSize}</span>
                    </div>
                  </div>
                </div>
              )}
            </div>

            {/* Save button */}
            <div className="flex items-center justify-between pt-2">
              {whisperModelExists && (
                <Button
                  variant="ghost"
                  size="sm"
                  icon={<Trash2 size={14} />}
                  onClick={() => setDeleteModelConfirmOpen(true)}
                  className="text-danger hover:bg-danger/10"
                >
                  {t('settings.videoDeleteModel')}
                </Button>
              )}
              <div className="flex-1" />
              <Button
                variant="primary"
                size="md"
                icon={<Save size={16} />}
                loading={videoSaveLoading}
                onClick={handleVideoSave}
              >
                {t('settings.videoSave')}
              </Button>
            </div>

            {/* Delete model confirmation */}
            <ConfirmDialog
              open={deleteModelConfirmOpen}
              onClose={() => setDeleteModelConfirmOpen(false)}
              onConfirm={handleWhisperDelete}
              title={t('settings.videoDeleteModel')}
              message={t('settings.videoDeleteConfirm')}
              confirmText={t('common.delete')}
              variant="danger"
            />
          </div>
        )}
      </Section>
      )}

      {/* ── Tab: Extensions ────────────────────────────────────────── */}
      {activeTab === 'extensions' && (
        <>
          {/* Skills */}
          <Section icon={<Blocks size={20} />} title={t('settings.skills')} delay={0.03}>
            <p className="mb-4 text-xs text-text-tertiary">{t('settings.skillsDescription')}</p>
            {showSkillForm ? (
              <SkillEditor
                skill={editingSkill ?? undefined}
                onSave={handleSaveSkill}
                onCancel={() => {
                  setSkillEditorDirty(false);
                  setShowSkillForm(false);
                  setEditingSkill(null);
                }}
                onDirtyChange={setSkillEditorDirty}
              />
            ) : (
              <div className="space-y-4">
                <div className="flex justify-end">
                  <Button variant="primary" size="sm" icon={<Plus size={14} />} onClick={() => { setEditingSkill(null); setShowSkillForm(true); }}>
                    {t('settings.addSkill')}
                  </Button>
                </div>
                {skills.length === 0 ? (
                  <div className="py-8 text-center">
                    <Blocks size={32} className="mx-auto mb-3 text-text-tertiary" />
                    <p className="text-sm text-text-secondary">{t('settings.noSkills')}</p>
                  </div>
                ) : (
                  <div className="space-y-3">
                    {skills.map((skill) => (
                      <motion.div
                        key={skill.id}
                        initial={{ opacity: 0, y: 20 }}
                        animate={{ opacity: 1, y: 0 }}
                        className="flex items-center justify-between rounded-lg border border-border bg-surface-2 p-4 transition-colors hover:bg-surface-3/50"
                      >
                        <div className="min-w-0 flex-1">
                          <div className="flex items-center gap-2">
                            <p className="text-sm font-medium text-text-primary truncate">{skill.name}</p>
                            <Badge variant="default" className="text-[10px] shrink-0">
                              ~{estimateTokens(skill.content)} tok
                            </Badge>
                          </div>
                          <p className="mt-0.5 text-xs text-text-tertiary truncate">
                            {skill.content.slice(0, 80)}{skill.content.length > 80 ? '\u2026' : ''}
                          </p>
                        </div>
                        <div className="flex items-center gap-1 shrink-0 ml-3">
                          <button
                            onClick={() => handleToggleSkill(skill.id, !skill.enabled)}
                            className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors duration-fast cursor-pointer ${
                              skill.enabled ? 'bg-accent' : 'bg-surface-3'
                            }`}
                          >
                            <span className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform duration-fast ${
                              skill.enabled ? 'translate-x-6' : 'translate-x-1'
                            }`} />
                          </button>
                          <button
                            onClick={() => { setEditingSkill(skill); setShowSkillForm(true); }}
                            className="rounded p-1.5 text-text-tertiary hover:text-accent hover:bg-accent/10 transition-colors cursor-pointer"
                            aria-label={t('common.edit')}
                          >
                            <Pencil size={14} />
                          </button>
                          <button
                            onClick={() => setDeleteSkillTarget(skill)}
                            className="rounded p-1.5 text-text-tertiary hover:text-danger hover:bg-danger/10 transition-colors cursor-pointer"
                            aria-label={t('common.delete')}
                          >
                            <Trash2 size={14} />
                          </button>
                        </div>
                      </motion.div>
                    ))}
                  </div>
                )}
              </div>
            )}
          </Section>

          {/* MCP Servers */}
          <Section icon={<Plug size={20} />} title={t('settings.mcpServers')} delay={0.06}>
            <p className="mb-4 text-xs text-text-tertiary">{t('settings.mcpServersDescription')}</p>
            {showMcpForm ? (
              <McpServerForm
                server={editingMcpServer ?? undefined}
                onSave={handleSaveMcpServer}
                onCancel={() => {
                  setMcpFormDirty(false);
                  setShowMcpForm(false);
                  setEditingMcpServer(null);
                }}
                onDirtyChange={setMcpFormDirty}
              />
            ) : (
              <div className="space-y-4">
                <div className="flex justify-end">
                  <Button variant="primary" size="sm" icon={<Plus size={14} />} onClick={() => { setEditingMcpServer(null); setShowMcpForm(true); }}>
                    {t('settings.addMcpServer')}
                  </Button>
                </div>
                {mcpServers.length === 0 ? (
                  <div className="py-8 text-center">
                    <Plug size={32} className="mx-auto mb-3 text-text-tertiary" />
                    <p className="text-sm text-text-secondary">{t('settings.noMcpServers')}</p>
                  </div>
                ) : (
                  <div className="space-y-3">
                    {mcpServers.map((server) => (
                      <motion.div
                        key={server.id}
                        initial={{ opacity: 0, y: 20 }}
                        animate={{ opacity: 1, y: 0 }}
                        className="rounded-lg border border-border bg-surface-2 transition-colors hover:bg-surface-3/50"
                      >
                        <div className="flex items-center justify-between p-4">
                          <div className="min-w-0 flex-1">
                            <div className="flex items-center gap-2">
                              <p className="text-sm font-medium text-text-primary truncate">{server.name}</p>
                              <Badge variant="default" className="text-[10px] shrink-0">{server.transport}</Badge>
                              {server.enabled && mcpToolCounts[server.id] && !mcpToolCounts[server.id].loading && !mcpToolCounts[server.id].error && (
                                <Badge variant="default" className="text-[10px] shrink-0 bg-accent/10 text-accent border-accent/20">
                                  {extensionCopy.toolCount(mcpToolCounts[server.id].tools.length)}
                                </Badge>
                              )}
                              {server.enabled && mcpToolCounts[server.id]?.error && !mcpToolCounts[server.id].loading && (
                                <Badge
                                  variant="default"
                                  className="text-[10px] shrink-0 bg-danger/10 text-danger border-danger/20 cursor-help max-w-[180px] truncate"
                                  title={mcpToolCounts[server.id].error}
                                >
                                  <AlertTriangle size={10} className="inline mr-0.5 -mt-px" />
                                  {extensionCopy.connectionFailed}
                                </Badge>
                              )}
                              {server.enabled && mcpToolCounts[server.id]?.loading && (
                                <Loader2 size={12} className="animate-spin text-text-tertiary" />
                              )}
                            </div>
                            <p className="mt-0.5 text-xs text-text-tertiary truncate">
                              {server.transport === 'stdio' ? server.command : server.url}
                            </p>
                          </div>
                          <div className="flex items-center gap-1 shrink-0 ml-3">
                            {server.enabled && mcpToolCounts[server.id]?.tools.length > 0 && (
                              <button
                                onClick={() => setMcpToolsExpanded((prev) => ({ ...prev, [server.id]: !prev[server.id] }))}
                                className="rounded p-1.5 text-text-tertiary hover:text-accent hover:bg-accent/10 transition-colors cursor-pointer"
                                aria-label={extensionCopy.toggleTools}
                              >
                                {mcpToolsExpanded[server.id] ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
                              </button>
                            )}
                            <button
                              onClick={() => handleToggleMcpServer(server.id, !server.enabled)}
                              className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors duration-fast cursor-pointer ${
                                server.enabled ? 'bg-accent' : 'bg-surface-3'
                              }`}
                            >
                              <span className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform duration-fast ${
                                server.enabled ? 'translate-x-6' : 'translate-x-1'
                              }`} />
                            </button>
                            <button
                              onClick={() => handleTestMcpServer(server.id)}
                              disabled={mcpTestLoading === server.id}
                              className="rounded p-1.5 text-text-tertiary hover:text-accent hover:bg-accent/10 transition-colors cursor-pointer disabled:opacity-50"
                              aria-label={t('settings.mcpTestConnection')}
                            >
                              {mcpTestLoading === server.id ? <Loader2 size={14} className="animate-spin" /> : <Zap size={14} />}
                            </button>
                            <button
                              onClick={() => { setEditingMcpServer(server); setShowMcpForm(true); }}
                              className="rounded p-1.5 text-text-tertiary hover:text-accent hover:bg-accent/10 transition-colors cursor-pointer"
                              aria-label={t('common.edit')}
                            >
                              <Pencil size={14} />
                            </button>
                            <button
                              onClick={() => setDeleteMcpTarget(server)}
                              className="rounded p-1.5 text-text-tertiary hover:text-danger hover:bg-danger/10 transition-colors cursor-pointer"
                              aria-label={t('common.delete')}
                            >
                              <Trash2 size={14} />
                            </button>
                          </div>
                        </div>
                        {/* Expandable tool list */}
                        <AnimatePresence initial={false}>
                          {mcpToolsExpanded[server.id] && mcpToolCounts[server.id]?.tools.length > 0 && (
                            <motion.div
                              initial={{ height: 0, opacity: 0 }}
                              animate={{ height: 'auto', opacity: 1 }}
                              exit={{ height: 0, opacity: 0 }}
                              transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
                              className="overflow-hidden"
                            >
                              <div className="px-4 pb-3 border-t border-border/50">
                                <p className="text-[10px] text-text-tertiary uppercase tracking-wider mt-2 mb-1.5">{extensionCopy.availableTools}</p>
                                <div className="flex flex-wrap gap-1.5">
                                  {mcpToolCounts[server.id].tools.map((tool) => (
                                    <span
                                      key={tool.name}
                                      title={tool.description ?? tool.name}
                                      className="inline-flex items-center px-2 py-0.5 rounded text-[11px] font-mono
                                        bg-surface-3 text-text-secondary border border-border/50"
                                    >
                                      {tool.name}
                                    </span>
                                  ))}
                                </div>
                              </div>
                            </motion.div>
                          )}
                        </AnimatePresence>
                      </motion.div>
                    ))}
                  </div>
                )}
              </div>
            )}
          </Section>
        </>
      )}

      {/* Delete skill confirm */}
      <ConfirmDialog
        open={!!deleteSkillTarget}
        onClose={() => setDeleteSkillTarget(null)}
        onConfirm={handleDeleteSkill}
        title={t('common.delete')}
        message={t('settings.deleteSkillConfirm')}
        confirmText={t('common.delete')}
        variant="danger"
      />

      {/* Delete MCP server confirm */}
      <ConfirmDialog
        open={!!deleteMcpTarget}
        onClose={() => setDeleteMcpTarget(null)}
        onConfirm={handleDeleteMcpServer}
        title={t('common.delete')}
        message={t('settings.deleteMcpServerConfirm')}
        confirmText={t('common.delete')}
        variant="danger"
      />

      <ConfirmDialog
        open={pendingTab !== null}
        onClose={handleCancelPendingTabChange}
        onConfirm={() => { void handleConfirmPendingTabChange(); }}
        title={t('settings.unsavedChangesTitle')}
        message={t('settings.discardTabChangesMessage')}
        confirmText={t('settings.discardChanges')}
        variant="warning"
        loading={discardingTabChanges}
      />

      <ConfirmDialog
        open={settingsNavigationBlocker.state === 'blocked'}
        onClose={handleCancelBlockedNavigation}
        onConfirm={handleConfirmBlockedNavigation}
        title={t('settings.unsavedChangesTitle')}
        message={t('settings.discardPageChangesMessage')}
        confirmText={t('settings.discardChanges')}
        variant="warning"
      />
    </div>
  );
}
