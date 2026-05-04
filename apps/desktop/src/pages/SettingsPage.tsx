import { useState, useEffect, useCallback, useMemo, useRef } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { useBlocker, useNavigate } from 'react-router-dom';
import { getVersion } from '@tauri-apps/api/app';
import {
  Database,
  Brain,
  Bot,
  Star,
  Film,
  Blocks,
  ChevronLeft,
  ChevronRight,
} from 'lucide-react';
import { toast } from 'sonner';
import * as api from '../lib/api';
import type { PersonaProfile, SavePersonaInput } from '../lib/api';
import { useProgress, progressStore } from '../lib/progressStore';
import { getModelStatus, invalidate as invalidateModelStatus } from '../lib/modelStatusCache';
import type { IndexStats } from '../types/index-stats';
import type { PrivacyConfig, RedactRule } from '../types/privacy';
import type { EmbedderConfig } from '../types/embedder';
import type { AgentConfig, AppConfig, SaveAgentConfigInput, UserMemory } from '../types/conversation';
import type { OcrConfig } from '../types/ocr';
import type { VideoConfig } from '../types/video';
import type { Skill, McpServer, McpToolInfo, SaveSkillInput, SaveMcpServerInput } from '../types/extensions';
import type { TraceSummary, AgentTrace } from '../types/trace';
import { useTranslation } from '../i18n';
import { ConfirmDialog } from '../components/ui/ConfirmDialog';
import { AppearanceSettingsTab } from '../components/settings/AppearanceSettingsTab';
import { DataPrivacySettingsTab } from '../components/settings/DataPrivacySettingsTab';
import { EmbeddingConfigSection } from '../components/settings/EmbeddingConfigSection';
import { ExtensionsSettingsTab, type SkillFilter } from '../components/settings/ExtensionsSettingsTab';
import { ModelDownloadsSection } from '../components/settings/ModelDownloadsSection';
import { OcrSettingsSection } from '../components/settings/OcrSettingsSection';
import { ProvidersSettingsTab, type ProviderView } from '../components/settings/ProvidersSettingsTab';
import { VideoSettingsSection } from '../components/settings/VideoSettingsSection';
import type { ProviderPreset } from '../lib/providerPresets';
import { useMicrophoneDevices } from '../lib/useMicrophoneDevices';
import { useUpdater } from '../lib/useUpdater';

/* ── Settings page ────────────────────────────────────────────────── */
type SettingsTab = 'appearance' | 'models_embedding' | 'providers' | 'media' | 'data_privacy' | 'extensions';
const MEMORY_CHAR_LIMIT = 240;
const TAB_STRIP_EDGE_EPSILON = 4;

export function SettingsPage() {
  const { t, locale, setLocale, availableLocales } = useTranslation();
  const navigate = useNavigate();
  const updater = useUpdater(false);
  const [appVersion, setAppVersion] = useState('');
  const { devices: micDevices, selectedDeviceId: micDeviceId, setSelectedDeviceId: setMicDeviceId, refresh: refreshMics } = useMicrophoneDevices();
  const tabStripRef = useRef<HTMLDivElement | null>(null);
  const [activeTab, setActiveTab] = useState<SettingsTab>('models_embedding');
  const [dirtyTabs, setDirtyTabs] = useState<Set<string>>(new Set());
  const [pendingTab, setPendingTab] = useState<SettingsTab | null>(null);
  const [discardingTabChanges, setDiscardingTabChanges] = useState(false);
  const [showLeftTabIndicator, setShowLeftTabIndicator] = useState(false);
  const [showRightTabIndicator, setShowRightTabIndicator] = useState(false);
  const [providerFormDirty, setProviderFormDirty] = useState(false);
  const [skillEditorDirty, setSkillEditorDirty] = useState(false);
  const [mcpFormDirty, setMcpFormDirty] = useState(false);
  const [personaEditorDirty, setPersonaEditorDirty] = useState(false);
  const hasDirtyTabs = dirtyTabs.size > 0;

  useEffect(() => {
    getVersion().then(setAppVersion).catch(() => setAppVersion(''));
  }, []);

  const isTabDirty = useCallback((tabId: SettingsTab) => {
    if (tabId === 'media') return dirtyTabs.has('ocr') || dirtyTabs.has('video');
    return dirtyTabs.has(tabId);
  }, [dirtyTabs]);

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
  const progress = useProgress();
  const ftsProgress = progress.ftsProgress;
  const embedRebuildProgress = progress.embedRebuildProgress;

  const loadStats = useCallback(() => {
    api.getIndexStats().then(setStats).catch(() => {
      toast.error(t('settings.loadStatsError'));
    });
  }, []);

  useEffect(() => {
    loadStats();
  }, [loadStats]);

  /* ── FTS & rebuild progress (from global store) ─────────────────── */

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

  /* ── Analytics state ────────────────────────────────────────────── */
  const [traceSummary, setTraceSummary] = useState<TraceSummary | null>(null);
  const [recentTraces, setRecentTraces] = useState<AgentTrace[]>([]);
  const [analyticsLoading, setAnalyticsLoading] = useState(false);

  /* ── Embedding state ─────────────────────────────────────────────── */
  const [embedConfig, setEmbedConfig] = useState<EmbedderConfig | null>(null);
  const [localModelReady, setLocalModelReady] = useState<boolean | null>(null);
  const [downloadLoading, setDownloadLoading] = useState(false);
  const downloadProgress = progress.modelDownload;
  const [testLoading, setTestLoading] = useState(false);
  const [embedSaveLoading, setEmbedSaveLoading] = useState(false);
  const [rebuildEmbedLoading, setRebuildEmbedLoading] = useState(false);

  /* ── App Config state ─────────────────────────────────────────────── */
  const [appConfig, setAppConfig] = useState<AppConfig | null>(null);
  const [appConfigLoading, setAppConfigLoading] = useState(false);
  const [officeRuntime, setOfficeRuntime] = useState<api.OfficeRuntimeReadiness | null>(null);
  const [officePreparing, setOfficePreparing] = useState(false);

  /* ── OCR state ────────────────────────────────────────────────────── */
  const [ocrConfig, setOcrConfig] = useState<OcrConfig | null>(null);
  const [ocrModelsExist, setOcrModelsExist] = useState<boolean | null>(null);
  const [ocrDownloading, setOcrDownloading] = useState(false);
  const ocrProgress = progress.ocrDownload;
  const [ocrSaveLoading, setOcrSaveLoading] = useState(false);

  /* ── Video state ──────────────────────────────────────────────────── */
  const [videoConfig, setVideoConfig] = useState<VideoConfig | null>(null);
  const [whisperModelExists, setWhisperModelExists] = useState<boolean | null>(null);
  const [ffmpegAvailable, setFfmpegAvailable] = useState<boolean | null>(null);
  const [videoDownloading, setVideoDownloading] = useState(false);
  const videoProgress = progress.videoDownload;
  const [videoSaveLoading, setVideoSaveLoading] = useState(false);
  const [showAdvancedVideo, setShowAdvancedVideo] = useState(false);
  const [deleteModelConfirmOpen, setDeleteModelConfirmOpen] = useState(false);
  const [ffmpegDownloading, setFfmpegDownloading] = useState(false);
  const ffmpegProgress = progress.ffmpegDownload;

  useEffect(() => {
    if (!rebuildEmbedLoading) {
      progressStore.update('embedRebuildProgress', null);
    }
  }, [rebuildEmbedLoading]);

  useEffect(() => {
    if (!downloadLoading) {
      progressStore.update('modelDownload', null);
    }
  }, [downloadLoading]);

  const loadEmbedConfig = useCallback(async () => {
    try {
      const cfg = await api.getEmbedderConfig();
      setEmbedConfig(cfg);
      if (cfg.provider === 'local') {
        setLocalModelReady(null);
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
      const key = embedConfig.localModel ?? '';
      getModelStatus('embed', key, () => api.checkLocalModel(embedConfig.localModel))
        .then(setLocalModelReady)
        .catch(() => setLocalModelReady(false));
    }
  }, [embedConfig?.provider, embedConfig?.localModel]);

  const handleDownloadModel = async () => {
    if (!embedConfig) return;
    if (downloadLoading) return;
    setDownloadLoading(true);
    try {
      await api.downloadLocalModel(embedConfig.localModel);
      setLocalModelReady(true);
      invalidateModelStatus('embed');
      toast.success(t('settings.embeddingDownloaded'));
    } catch (e) {
      toast.error(t('settings.embeddingDownloadFail') + ': ' + String(e));
    } finally {
      setDownloadLoading(false);
    }
  };

  const handleCancelDownload = async () => {
    try {
      await api.cancelModelDownload();
      setDownloadLoading(false);
      toast.success(t('settings.downloadCancelled'));
    } catch (e) {
      toast.error(String(e));
    }
  };

  const [deleteEmbedModelConfirmOpen, setDeleteEmbedModelConfirmOpen] = useState(false);

  const handleDeleteModel = async () => {
    if (!embedConfig) return;
    try {
      await api.deleteLocalModel(embedConfig.localModel);
      setLocalModelReady(false);
      invalidateModelStatus('embed');
      toast.success(t('settings.modelDeleted'));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setDeleteEmbedModelConfirmOpen(false);
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
      markClean('models_embedding');
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

  /* ── App Config effects & handlers ─────────────────────────────── */
  const loadAppConfig = useCallback(async () => {
    try {
      const cfg = await api.getAppConfig();
      setAppConfig(cfg);
    } catch {
      setAppConfig({
        toolTimeoutSecs: 30,
        agentTimeoutSecs: 180,
        cacheTtlHours: 24,
        defaultSearchLimit: 20,
        minSearchSimilarity: 0.2,
        maxTextFileSize: 104857600,
        maxVideoFileSize: 2147483648,
        maxAudioFileSize: 536870912,
        llmTimeoutSecs: 300,
        mcpCallTimeoutSecs: 60,
        confirmDestructive: false,
        shellAccessMode: 'restricted',
        toolApprovalMode: 'ask',
        hfMirrorBaseUrl: 'https://hf-mirror.com',
        ghproxyBaseUrl: 'https://mirror.ghproxy.com',
      });
    }
  }, []);

  useEffect(() => {
    void loadAppConfig();
  }, [loadAppConfig]);

  const handleAppConfigSave = async () => {
    if (!appConfig) return;
    setAppConfigLoading(true);
    try {
      await api.saveAppConfig(appConfig);
      toast.success(t('common.success'));
    } catch {
      toast.error(t('common.error'));
    } finally {
      setAppConfigLoading(false);
    }
  };

  const loadOfficeRuntime = useCallback(async () => {
    try {
      const readiness = await getModelStatus('office', 'runtime', () => api.checkOfficeRuntime());
      setOfficeRuntime(readiness);
      return true;
    } catch {
      setOfficeRuntime(null);
      return false;
    }
  }, []);

  useEffect(() => {
    void loadOfficeRuntime();
  }, [loadOfficeRuntime]);

  const handlePrepareOfficeRuntime = async () => {
    if (officePreparing) return;
    setOfficePreparing(true);
    try {
      const result = await api.prepareOfficeRuntime();
      setOfficeRuntime(result.readiness);
      invalidateModelStatus('office');
      if (result.success) {
        toast.success(t('settings.documentToolsInstallSuccess'));
      } else {
        toast.error(result.readiness.summary || t('settings.documentToolsInstallFail'));
      }
    } catch (e) {
      toast.error(t('settings.documentToolsInstallFail') + ': ' + String(e));
    } finally {
      setOfficePreparing(false);
    }
  };

  const handleAskAiPrepareOfficeRuntime = useCallback(() => {
    const readinessSummary = officeRuntime
      ? `Current document tools status: ${officeRuntime.status}. ${officeRuntime.summary}`
      : 'Current document tools status has not been checked yet.';
    navigate('/chat', {
      state: {
        initialMessage:
          `请帮我准备本机文档工具，用于 DOCX、XLSX、PPTX 和 PDF 的创建、编辑、转换与渲染。\n\n` +
          `${readinessSummary}\n\n` +
          `请先调用 prepare_document_tools 检查当前状态。若必需依赖缺失，请帮我准备必需的 Python 环境和包。` +
          `在安装或下载可选依赖（LibreOffice、Poppler）之前，请先询问我是否需要这些能力；` +
          `如果准备过程中发生权限、网络或路径问题，请继续诊断并给出可执行的下一步。`,
      },
    });
  }, [navigate, officeRuntime]);

  /**
   * Reset the "wizard_completed" flag and navigate to `/wizard`.
   * Does NOT clear any other settings (providers, sources) so the user can
   * re-pick where they left off.
   */
  const handleRerunWizard = async () => {
    try {
      await api.resetWizard();
      toast.success(t('wizard.rerunSuccess'));
      navigate('/wizard');
    } catch {
      toast.error(t('wizard.rerunError'));
    }
  };

  /* ── OCR effects & handlers ──────────────────────────────────────── */
  const loadOcrConfig = useCallback(async () => {
    try {
      const cfg = await api.getOcrConfig();
      setOcrConfig(cfg);
      getModelStatus('ocr', JSON.stringify(cfg), () => api.checkOcrModels(cfg))
        .then(setOcrModelsExist)
        .catch(() => setOcrModelsExist(false));
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
      progressStore.update('ocrDownload', null);
    }
  }, [ocrDownloading]);

  const handleDownloadOcrModels = async () => {
    if (!ocrConfig) return;
    if (ocrDownloading) return;
    setOcrDownloading(true);
    try {
      await api.downloadOcrModels(ocrConfig);
      setOcrModelsExist(true);
      invalidateModelStatus('ocr');
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
      const cfgKey = JSON.stringify(cfg);
      getModelStatus('whisper', cfgKey, () => api.checkWhisperModel(cfg))
        .then(setWhisperModelExists)
        .catch(() => setWhisperModelExists(false));
      getModelStatus('ffmpeg', cfgKey, () => api.checkFfmpeg(cfg))
        .then(setFfmpegAvailable)
        .catch(() => setFfmpegAvailable(false));
      return true;
    } catch {
      return false;
    }
  }, []);

  useEffect(() => {
    void loadVideoConfig();
  }, [loadVideoConfig]);

  useEffect(() => {
    if (!videoDownloading) { progressStore.update('videoDownload', null); }
  }, [videoDownloading]);

  const handleWhisperDownload = async () => {
    if (!videoConfig) return;
    if (videoDownloading) return;
    setVideoDownloading(true);
    try {
      await api.downloadWhisperModel(videoConfig);
      setWhisperModelExists(true);
      invalidateModelStatus('whisper');
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
      invalidateModelStatus('whisper');
      toast.success(t('settings.videoDeleteSuccess'));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setDeleteModelConfirmOpen(false);
    }
  };

  // FFmpeg download
  useEffect(() => {
    if (!ffmpegDownloading) { progressStore.update('ffmpegDownload', null); }
  }, [ffmpegDownloading]);

  const handleFfmpegDownload = async () => {
    if (ffmpegDownloading) return;
    setFfmpegDownloading(true);
    try {
      const path = await api.downloadFfmpeg();
      setFfmpegAvailable(true);
      invalidateModelStatus('ffmpeg');
      toast.success(t('settings.videoFfmpegDownloadComplete'));
      // Refresh config to pick up the saved ffmpeg path
      await loadVideoConfig();
      void path; // path is auto-saved by backend
    } catch (e) {
      toast.error(t('settings.videoFfmpegDownloadFailed') + ': ' + String(e));
    } finally {
      setFfmpegDownloading(false);
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
    if (activeTab === 'data_privacy' && !privacyConfig) {
      void loadPrivacyConfig();
    }
  }, [activeTab, loadPrivacyConfig, privacyConfig]);

  const loadAnalytics = useCallback(async () => {
    setAnalyticsLoading(true);
    try {
      const [summary, traces] = await Promise.all([
        api.getTraceSummary(),
        api.getRecentTraces(20),
      ]);
      setTraceSummary(summary);
      setRecentTraces(traces);
    } catch {
      // Silently fail — analytics are non-critical
    } finally {
      setAnalyticsLoading(false);
    }
  }, []);

  useEffect(() => {
    if (activeTab === 'data_privacy') {
      void loadAnalytics();
    }
  }, [activeTab, loadAnalytics]);

  const discardActiveTabChanges = useCallback(async () => {
    switch (activeTab) {
      case 'models_embedding': {
        const reloaded = await loadEmbedConfig();
        if (!reloaded) return false;
        break;
      }
      case 'data_privacy': {
        const reloaded = await loadPrivacyConfig();
        if (!reloaded) return false;
        break;
      }
      case 'media': {
        const [ocrReloaded, videoReloaded] = await Promise.all([loadOcrConfig(), loadVideoConfig()]);
        if (!ocrReloaded || !videoReloaded) return false;
        markClean('ocr');
        markClean('video');
        return true;
      }
      default:
        break;
    }

    markClean(activeTab);
    return true;
  }, [activeTab, loadEmbedConfig, loadOcrConfig, loadVideoConfig, loadPrivacyConfig, markClean]);

  const handleTabChange = useCallback((nextTab: SettingsTab) => {
    if (nextTab === activeTab) return;
    if (isTabDirty(activeTab)) {
      setPendingTab(nextTab);
      return;
    }
    setActiveTab(nextTab);
  }, [activeTab, isTabDirty]);

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
    if (pendingTab && !isTabDirty(activeTab)) {
      setActiveTab(pendingTab);
      setPendingTab(null);
    }
  }, [activeTab, dirtyTabs, isTabDirty, pendingTab]);

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
    if (personaEditorDirty || skillEditorDirty || mcpFormDirty) {
      markDirty('extensions');
      return;
    }

    markClean('extensions');
  }, [markClean, markDirty, mcpFormDirty, personaEditorDirty, skillEditorDirty]);

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
    markDirty('data_privacy');
    setNewPattern('');
  };

  const removePattern = (idx: number) => {
    if (!privacyConfig) return;
    setPrivacyConfig({
      ...privacyConfig,
      excludePatterns: privacyConfig.excludePatterns.filter((_, i) => i !== idx),
    });
    markDirty('data_privacy');
  };

  const addRule = () => {
    if (!newRule.name.trim() || !newRule.pattern.trim() || !privacyConfig) return;
    setPrivacyConfig({
      ...privacyConfig,
      redactPatterns: [...privacyConfig.redactPatterns, { ...newRule }],
    });
    markDirty('data_privacy');
    setNewRule({ name: '', pattern: '', replacement: '' });
  };

  const removeRule = (idx: number) => {
    if (!privacyConfig) return;
    setPrivacyConfig({
      ...privacyConfig,
      redactPatterns: privacyConfig.redactPatterns.filter((_, i) => i !== idx),
    });
    markDirty('data_privacy');
  };

  const handleSavePrivacy = async () => {
    if (!privacyConfig) return;
    setSaveLoading(true);
    try {
      await api.savePrivacyConfig(privacyConfig);
      markClean('data_privacy');
      toast.success(t('settings.privacySaved'));
    } catch {
      toast.error(t('settings.privacySaveError'));
    } finally {
      setSaveLoading(false);
    }
  };

  /* ── Extensions state ────────────────────────────────────────────── */
  const [personas, setPersonas] = useState<PersonaProfile[]>([]);
  const [skills, setSkills] = useState<Skill[]>([]);
  const [mcpServers, setMcpServers] = useState<McpServer[]>([]);
  const [editingPersona, setEditingPersona] = useState<PersonaProfile | null>(null);
  const [editingSkill, setEditingSkill] = useState<Skill | null>(null);
  const [editingMcpServer, setEditingMcpServer] = useState<McpServer | null>(null);
  const [showPersonaForm, setShowPersonaForm] = useState(false);
  const [showSkillForm, setShowSkillForm] = useState(false);
  const [showMcpForm, setShowMcpForm] = useState(false);
  const [deletePersonaTarget, setDeletePersonaTarget] = useState<PersonaProfile | null>(null);
  const [deleteSkillTarget, setDeleteSkillTarget] = useState<Skill | null>(null);
  const [deleteMcpTarget, setDeleteMcpTarget] = useState<McpServer | null>(null);
  const [mcpTestLoading, setMcpTestLoading] = useState<string | null>(null);
  const [mcpToolCounts, setMcpToolCounts] = useState<Record<string, { tools: McpToolInfo[]; loading: boolean; error?: string }>>({});
  const [mcpToolsExpanded, setMcpToolsExpanded] = useState<Record<string, boolean>>({});
  const [skillSearch, setSkillSearch] = useState('');
  const [skillFilter, setSkillFilter] = useState<SkillFilter>('all');
  const [viewSkill, setViewSkill] = useState<Skill | null>(null);

  const loadPersonas = useCallback(() => {
    api.listPersonas()
      .then(setPersonas)
      .catch(() => {
        toast.error(t('common.error'));
      });
  }, []);

  const loadSkills = useCallback(() => {
    api.listAllSkills()
      .then(setSkills)
      .catch(() => {
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
      loadPersonas();
      loadSkills();
      loadMcpServers();
    }
  }, [activeTab, loadPersonas, loadSkills, loadMcpServers]);

  const handleSavePersona = async (input: SavePersonaInput) => {
    try {
      await api.savePersona(input);
      toast.success(t('common.success'));
      setPersonaEditorDirty(false);
      setShowPersonaForm(false);
      setEditingPersona(null);
      loadPersonas();
    } catch {
      toast.error(t('common.error'));
    }
  };

  const handleDeletePersona = async () => {
    if (!deletePersonaTarget) return;
    try {
      await api.deletePersona(deletePersonaTarget.id);
      toast.success(t('common.success'));
      setDeletePersonaTarget(null);
      loadPersonas();
    } catch {
      toast.error(t('common.error'));
    }
  };

  const handleTogglePersona = async (id: string, enabled: boolean) => {
    try {
      await api.togglePersona(id, enabled);
      setPersonas((prev) => prev.map((p) => p.id === id ? { ...p, enabled } : p));
    } catch {
      toast.error(t('common.error'));
    }
  };

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

  const filteredSkills = useMemo(() => {
    const needle = skillSearch.trim().toLowerCase();
    return skills.filter((s) => {
      // Filter chip.
      if (skillFilter === 'builtin' && !s.builtin) return false;
      if (skillFilter === 'user' && s.builtin) return false;
      if (skillFilter === 'enabled' && !s.enabled) return false;
      if (skillFilter === 'disabled' && s.enabled) return false;
      // Fuzzy search on name / description / content substring.
      if (needle) {
        const hay = `${s.name}\n${s.description}\n${s.content}`.toLowerCase();
        if (!hay.includes(needle)) return false;
      }
      return true;
    });
  }, [skills, skillSearch, skillFilter]);

  const handleExportAllSkills = useCallback(async () => {
    if (skills.length === 0) return;
    try {
      const chunks: string[] = [];
      for (const s of skills) {
        const md = await api.exportSkillToMd(s.id);
        // Separator makes the bundle easily splittable by hand.
        chunks.push(`${md.trimEnd()}\n\n<!-- ===== END OF SKILL: ${s.name} ===== -->\n`);
      }
      const blob = new Blob([chunks.join('\n')], { type: 'text/markdown' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = `skills-export-${new Date().toISOString().slice(0, 10)}.md`;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
      toast.success(
        t('settings.skillExportAllSuccess', { count: String(skills.length) }),
      );
    } catch {
      toast.error(t('common.error'));
    }
  }, [skills, t]);

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
    if (activeTab === 'providers') {
      loadAgentConfigs();
    }
  }, [activeTab, loadAgentConfigs]);

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

  const tabs: { id: SettingsTab; label: string; icon: React.ReactNode }[] = [
    { id: 'appearance', label: t('settings.appearance'), icon: <Star size={16} /> },
    { id: 'models_embedding', label: t('settings.tabModelsEmbedding'), icon: <Brain size={16} /> },
    { id: 'providers', label: t('settings.aiProviders'), icon: <Bot size={16} /> },
    { id: 'media', label: t('settings.tabMedia'), icon: <Film size={16} /> },
    { id: 'data_privacy', label: t('settings.tabDataPrivacy'), icon: <Database size={16} /> },
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
              {isTabDirty(tab.id) && (
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
        <AppearanceSettingsTab
          locale={locale}
          setLocale={setLocale}
          availableLocales={availableLocales}
          appVersion={appVersion}
          updater={updater}
          appConfig={appConfig}
          appConfigLoading={appConfigLoading}
          onAppConfigChange={setAppConfig}
          onAppConfigSave={() => { void handleAppConfigSave(); }}
          onRerunWizard={() => { void handleRerunWizard(); }}
        />
      )}

      {/* ── Tab: Models & Embedding ──────────────────────────────── */}
      {activeTab === 'models_embedding' && (
        <>
        {/* Models section */}
        <ModelDownloadsSection
          embedConfig={embedConfig}
          localModelReady={localModelReady}
          downloadLoading={downloadLoading}
          downloadProgress={downloadProgress}
          ocrDownloading={ocrDownloading}
          ocrModelsExist={ocrModelsExist}
          ocrProgress={ocrProgress}
          videoConfig={videoConfig}
          videoDownloading={videoDownloading}
          videoProgress={videoProgress}
          whisperModelExists={whisperModelExists}
          officeRuntime={officeRuntime}
          officePreparing={officePreparing}
          appConfig={appConfig}
          appConfigLoading={appConfigLoading}
          deleteEmbedModelConfirmOpen={deleteEmbedModelConfirmOpen}
          onEmbedLocalModelChange={(localModel) => {
            if (!embedConfig) return;
            setEmbedConfig({ ...embedConfig, localModel });
            setLocalModelReady(null);
            markDirty('models_embedding');
          }}
          onDownloadModel={handleDownloadModel}
          onCancelDownload={handleCancelDownload}
          onRequestDeleteEmbedModel={() => setDeleteEmbedModelConfirmOpen(true)}
          onCloseDeleteEmbedModel={() => setDeleteEmbedModelConfirmOpen(false)}
          onConfirmDeleteEmbedModel={() => { void handleDeleteModel(); }}
          onDownloadOcrModels={handleDownloadOcrModels}
          onWhisperDownload={handleWhisperDownload}
          onWhisperModelChange={(whisperModel) => {
            if (!videoConfig) return;
            const updated = { ...videoConfig, whisperModel };
            setVideoConfig(updated);
            setWhisperModelExists(null);
            markDirty('video');
            api.checkWhisperModel(updated)
              .then(setWhisperModelExists)
              .catch(() => setWhisperModelExists(false));
          }}
          onPrepareOfficeRuntime={handlePrepareOfficeRuntime}
          onRefreshOfficeRuntime={() => { void loadOfficeRuntime(); }}
          onAskAiPrepareOfficeRuntime={handleAskAiPrepareOfficeRuntime}
          onAppConfigChange={setAppConfig}
          onAppConfigSave={() => { void handleAppConfigSave(); }}
          onMarkModelsDirty={() => markDirty('models_embedding')}
        />

        <EmbeddingConfigSection
          embedConfig={embedConfig}
          localModelReady={localModelReady}
          testLoading={testLoading}
          embedSaveLoading={embedSaveLoading}
          rebuildEmbedLoading={rebuildEmbedLoading}
          embedRebuildProgress={embedRebuildProgress}
          onConfigChange={setEmbedConfig}
          onMarkDirty={() => markDirty('models_embedding')}
          onTestConnection={handleTestConnection}
          onSave={handleSaveEmbedConfig}
          onRebuild={handleRebuildEmbeddings}
        />
      </>
      )}

      {/* ── Tab: AI Providers ──────────────────────────────────────── */}
      {activeTab === 'providers' && (
        <ProvidersSettingsTab
          providerView={providerView}
          agentConfigs={agentConfigs}
          editingConfig={editingConfig}
          selectedPreset={selectedPreset}
          agentSaveLoading={agentSaveLoading}
          onSaveAgent={handleSaveAgent}
          onProviderViewChange={setProviderView}
          onProviderFormDirtyChange={setProviderFormDirty}
          onEditingConfigChange={setEditingConfig}
          onSelectedPresetChange={setSelectedPreset}
          onSetDefault={handleSetDefault}
          onDeleteTargetChange={setDeleteTarget}
        />
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

      {/* ── Tab: Data & Privacy ─────────────────────────────────── */}
      {activeTab === 'data_privacy' && (
        <DataPrivacySettingsTab
          analyticsLoading={analyticsLoading}
          traceSummary={traceSummary}
          recentTraces={recentTraces}
          stats={stats}
          rebuildLoading={rebuildLoading}
          optimizeLoading={optimizeLoading}
          clearCacheLoading={clearCacheLoading}
          ftsProgress={ftsProgress}
          privacyConfig={privacyConfig}
          newPattern={newPattern}
          newRule={newRule}
          userMemories={userMemories}
          editingMemoryId={editingMemoryId}
          editingMemoryDraft={editingMemoryDraft}
          memoryLoading={memoryLoading}
          newMemory={newMemory}
          memoryCharLimit={MEMORY_CHAR_LIMIT}
          saveLoading={saveLoading}
          onRebuild={handleRebuild}
          onOptimize={handleOptimize}
          onClearCache={handleClearCache}
          onNewPatternChange={setNewPattern}
          onAddPattern={addPattern}
          onRemovePattern={removePattern}
          onNewRuleChange={setNewRule}
          onAddRule={addRule}
          onRemoveRule={removeRule}
          onEditingMemoryDraftChange={setEditingMemoryDraft}
          onStartEditMemory={handleStartEditUserMemory}
          onCancelEditMemory={handleCancelEditUserMemory}
          onUpdateMemory={handleUpdateUserMemory}
          onDeleteMemory={handleDeleteUserMemory}
          onNewMemoryChange={setNewMemory}
          onAddMemory={handleAddUserMemory}
          onSavePrivacy={handleSavePrivacy}
        />
      )}

      {/* ── Tab: Media Processing ─────────────────────────────────── */}
      {activeTab === 'media' && (
      <>
      {/* OCR section */}
      <OcrSettingsSection
        ocrConfig={ocrConfig}
        ocrSaveLoading={ocrSaveLoading}
        onConfigChange={setOcrConfig}
        onMarkDirty={() => markDirty('ocr')}
        onSave={handleSaveOcrConfig}
      />

      {/* Video section */}
      <VideoSettingsSection
        videoConfig={videoConfig}
        ffmpegAvailable={ffmpegAvailable}
        ffmpegDownloading={ffmpegDownloading}
        ffmpegProgress={ffmpegProgress}
        whisperModelExists={whisperModelExists}
        videoSaveLoading={videoSaveLoading}
        showAdvancedVideo={showAdvancedVideo}
        deleteModelConfirmOpen={deleteModelConfirmOpen}
        micDevices={micDevices}
        micDeviceId={micDeviceId}
        onConfigChange={setVideoConfig}
        onMarkDirty={() => markDirty('video')}
        onFfmpegDownload={handleFfmpegDownload}
        onAdvancedToggle={() => setShowAdvancedVideo((value) => !value)}
        onMicDeviceChange={setMicDeviceId}
        onRefreshMics={refreshMics}
        onRequestDeleteModel={() => setDeleteModelConfirmOpen(true)}
        onCloseDeleteModel={() => setDeleteModelConfirmOpen(false)}
        onConfirmDeleteModel={handleWhisperDelete}
        onSave={handleVideoSave}
      />
      </>
      )}

      {/* ── Tab: Extensions ────────────────────────────────────────── */}
      {activeTab === 'extensions' && (
        <ExtensionsSettingsTab
          personas={personas}
          skills={skills}
          filteredSkills={filteredSkills}
          showPersonaForm={showPersonaForm}
          editingPersona={editingPersona}
          deletePersonaTarget={deletePersonaTarget}
          skillSearch={skillSearch}
          skillFilter={skillFilter}
          showSkillForm={showSkillForm}
          editingSkill={editingSkill}
          deleteSkillTarget={deleteSkillTarget}
          viewSkill={viewSkill}
          mcpServers={mcpServers}
          showMcpForm={showMcpForm}
          editingMcpServer={editingMcpServer}
          deleteMcpTarget={deleteMcpTarget}
          mcpTestLoading={mcpTestLoading}
          mcpToolCounts={mcpToolCounts}
          mcpToolsExpanded={mcpToolsExpanded}
          onAddPersona={() => { setEditingPersona(null); setShowPersonaForm(true); }}
          onSavePersona={handleSavePersona}
          onCancelPersonaForm={() => {
            setPersonaEditorDirty(false);
            setShowPersonaForm(false);
            setEditingPersona(null);
          }}
          onPersonaEditorDirtyChange={setPersonaEditorDirty}
          onTogglePersona={handleTogglePersona}
          onEditPersona={(persona) => { setEditingPersona(persona); setShowPersonaForm(true); }}
          onDeletePersonaTargetChange={setDeletePersonaTarget}
          onConfirmDeletePersona={handleDeletePersona}
          onSkillSearchChange={setSkillSearch}
          onSkillFilterChange={setSkillFilter}
          onExportAllSkills={handleExportAllSkills}
          onAddSkill={() => { setEditingSkill(null); setShowSkillForm(true); }}
          onSaveSkill={handleSaveSkill}
          onCancelSkillForm={() => {
            setSkillEditorDirty(false);
            setShowSkillForm(false);
            setEditingSkill(null);
          }}
          onSkillEditorDirtyChange={setSkillEditorDirty}
          onViewSkillChange={setViewSkill}
          onToggleSkill={handleToggleSkill}
          onEditSkill={(skill) => { setEditingSkill(skill); setShowSkillForm(true); }}
          onDeleteSkillTargetChange={setDeleteSkillTarget}
          onConfirmDeleteSkill={handleDeleteSkill}
          onAddMcpServer={() => { setEditingMcpServer(null); setShowMcpForm(true); }}
          onSaveMcpServer={handleSaveMcpServer}
          onCancelMcpForm={() => {
            setMcpFormDirty(false);
            setShowMcpForm(false);
            setEditingMcpServer(null);
          }}
          onMcpFormDirtyChange={setMcpFormDirty}
          onToggleMcpServer={handleToggleMcpServer}
          onTestMcpServer={handleTestMcpServer}
          onEditMcpServer={(server) => { setEditingMcpServer(server); setShowMcpForm(true); }}
          onDeleteMcpTargetChange={setDeleteMcpTarget}
          onToggleMcpToolsExpanded={(serverId) => setMcpToolsExpanded((prev) => ({ ...prev, [serverId]: !prev[serverId] }))}
          onConfirmDeleteMcpServer={handleDeleteMcpServer}
        />
      )}

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
