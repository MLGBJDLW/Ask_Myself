import { AlertTriangle, Brain, FolderOpen, HardDrive, Mic, RotateCcw, ScanLine } from 'lucide-react';
import { open } from '@tauri-apps/plugin-dialog';
import { useEffect, useState } from 'react';
import { useTranslation } from '../../i18n';
import type { ManagedModelPaths, OfficeRuntimeReadiness } from '../../lib/api';
import type { DownloadProgress } from '../../types/ingest';
import type { AppConfig } from '../../types/conversation';
import type { EmbedderConfig, LocalModelId } from '../../types/embedder';
import type { OcrDownloadProgress } from '../../types/ocr';
import type { VideoConfig, VideoDownloadProgress, WhisperModel } from '../../types/video';
import { ConfirmDialog } from '../ui/ConfirmDialog';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { CollapsiblePanel, Section } from './SettingsSection';
import { ModelCard } from './ModelCard';
import { NetworkMirrorsPanel } from './NetworkMirrorsPanel';
import { OfficeRuntimePanel } from './OfficeRuntimePanel';
import { SpeechToTextSettingsPanel } from './SpeechToTextSettingsPanel';
import { TextToSpeechSettingsPanel } from './TextToSpeechSettingsPanel';

interface ModelDownloadsSectionProps {
  embedConfig: EmbedderConfig | null;
  localModelReady: boolean | null;
  downloadLoading: boolean;
  downloadProgress: DownloadProgress | null;
  ocrDownloading: boolean;
  ocrModelsExist: boolean | null;
  ocrProgress: OcrDownloadProgress | null;
  videoConfig: VideoConfig | null;
  videoDownloading: boolean;
  videoProgress: VideoDownloadProgress | null;
  whisperModelExists: boolean | null;
  officeRuntime: OfficeRuntimeReadiness | null;
  officePreparing: boolean;
  appConfig: AppConfig | null;
  appConfigLoading: boolean;
  deleteEmbedModelConfirmOpen: boolean;
  managedModelPaths: ManagedModelPaths | null;
  modelStorageSaving: boolean;
  onEmbedLocalModelChange: (model: LocalModelId) => void;
  onDownloadModel: () => void;
  onCancelDownload: () => void;
  onRequestDeleteEmbedModel: () => void;
  onCloseDeleteEmbedModel: () => void;
  onConfirmDeleteEmbedModel: () => void;
  onDownloadOcrModels: () => void;
  onDeleteOcrModels: () => void | Promise<void>;
  onWhisperDownload: () => void;
  onDeleteWhisperModel: () => void | Promise<void>;
  onWhisperModelChange: (model: WhisperModel) => void;
  onPrepareOfficeRuntime: () => void;
  onRefreshOfficeRuntime: () => void;
  onAskAiPrepareOfficeRuntime: () => void;
  onAppConfigChange: (config: AppConfig) => void;
  onAppConfigSave: () => void;
  onMarkModelsDirty: () => void;
  onApplyManagedModelRoot: (root: string) => void | Promise<void>;
  onResetManagedModelRoot: () => void | Promise<void>;
}

function whisperModelSize(model: VideoConfig['whisperModel'] | undefined): string | undefined {
  switch (model) {
    case 'tiny':
      return '~39 MB';
    case 'base':
      return '~142 MB';
    case 'small':
      return '~466 MB';
    case 'medium':
      return '~1.5 GB';
    case 'large':
      return '~3.1 GB';
    case 'large_turbo':
      return '~1.6 GB';
    default:
      return undefined;
  }
}

function embeddingModelSize(model: LocalModelId | undefined): string {
  switch (model) {
    case 'Qwen3Embedding06B':
      return '~614 MB';
    case 'MultilingualE5Base':
      return '~470 MB';
    default:
      return '~46 MB';
  }
}

export function ModelDownloadsSection({
  embedConfig,
  localModelReady,
  downloadLoading,
  downloadProgress,
  ocrDownloading,
  ocrModelsExist,
  ocrProgress,
  videoConfig,
  videoDownloading,
  videoProgress,
  whisperModelExists,
  officeRuntime,
  officePreparing,
  appConfig,
  appConfigLoading,
  deleteEmbedModelConfirmOpen,
  managedModelPaths,
  modelStorageSaving,
  onEmbedLocalModelChange,
  onDownloadModel,
  onCancelDownload,
  onRequestDeleteEmbedModel,
  onCloseDeleteEmbedModel,
  onConfirmDeleteEmbedModel,
  onDownloadOcrModels,
  onDeleteOcrModels,
  onWhisperDownload,
  onDeleteWhisperModel,
  onWhisperModelChange,
  onPrepareOfficeRuntime,
  onRefreshOfficeRuntime,
  onAskAiPrepareOfficeRuntime,
  onAppConfigChange,
  onAppConfigSave,
  onMarkModelsDirty,
  onApplyManagedModelRoot,
  onResetManagedModelRoot,
}: ModelDownloadsSectionProps) {
  const { t } = useTranslation();
  const [modelRootDraft, setModelRootDraft] = useState('');
  const [deleteManagedModel, setDeleteManagedModel] = useState<'ocr' | 'whisper' | null>(null);

  useEffect(() => {
    if (managedModelPaths?.root) setModelRootDraft(managedModelPaths.root);
  }, [managedModelPaths?.root]);

  const chooseModelRoot = async () => {
    const selected = await open({
      directory: true,
      multiple: false,
      title: t('settings.localModelStorageChoose'),
      defaultPath: modelRootDraft || undefined,
    });
    if (typeof selected === 'string') setModelRootDraft(selected);
  };

  return (
    <Section
      icon={<HardDrive size={20} />}
      title={t('settings.models')}
      delay={0.03}
      description={t('settings.modelsDesc')}
      collapsible
      defaultOpen={false}
    >
      <div className="space-y-4">
        <div className="rounded-xl border border-border bg-surface-1/70 p-4">
          <div className="flex items-start gap-3">
            <span className="mt-0.5 flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-accent/10 text-accent">
              <HardDrive size={18} />
            </span>
            <div className="min-w-0 flex-1">
              <p className="text-sm font-semibold text-text-primary">{t('settings.localModelStorage')}</p>
              <p className="mt-1 text-xs leading-relaxed text-text-tertiary">{t('settings.localModelStorageDesc')}</p>
              <div className="mt-3 flex gap-2">
                <Input
                  value={modelRootDraft}
                  onChange={(event) => setModelRootDraft(event.target.value)}
                  placeholder={managedModelPaths?.root ?? ''}
                  aria-label={t('settings.localModelStorage')}
                  className="min-w-0 flex-1 font-mono text-xs"
                />
                <Button
                  variant="secondary"
                  size="sm"
                  icon={<FolderOpen size={14} />}
                  onClick={() => { void chooseModelRoot(); }}
                  disabled={modelStorageSaving}
                >
                  {t('settings.localModelStorageBrowse')}
                </Button>
              </div>
              <div className="mt-3 flex flex-wrap gap-2">
                <Button
                  variant="primary"
                  size="sm"
                  onClick={() => { void onApplyManagedModelRoot(modelRootDraft.trim()); }}
                  loading={modelStorageSaving}
                  disabled={!modelRootDraft.trim()}
                >
                  {t('settings.localModelStorageApply')}
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  icon={<RotateCcw size={14} />}
                  onClick={() => { void onResetManagedModelRoot(); }}
                  disabled={modelStorageSaving}
                >
                  {t('settings.localModelStorageDefault')}
                </Button>
              </div>
              {managedModelPaths && (
                <div className="mt-3 grid gap-1 rounded-lg border border-border/70 bg-surface-2/60 p-3 font-mono text-[11px] leading-relaxed text-text-tertiary">
                  <span>{t('settings.modelsEmbedding')}: {managedModelPaths.embedding}</span>
                  <span>{t('settings.modelsOcr')}: {managedModelPaths.ocr}</span>
                  <span>{t('settings.modelsWhisper')}: {managedModelPaths.whisper}</span>
                </div>
              )}
              <p className="mt-2 text-[11px] leading-relaxed text-warning">{t('settings.localModelStorageWarning')}</p>
            </div>
          </div>
        </div>

        {/* Embedding Model */}
        <ModelCard
          title={t('settings.modelsEmbedding')}
          icon={<Brain size={18} />}
          description={t('settings.modelsEmbeddingDesc')}
          status={
            downloadLoading ? 'downloading'
            : !embedConfig ? 'checking'
            : embedConfig?.provider !== 'local' ? 'downloaded'
            : localModelReady === null ? 'checking'
            : localModelReady ? 'downloaded'
            : 'not-downloaded'
          }
          size={embeddingModelSize(embedConfig?.localModel)}
          onDownload={onDownloadModel}
          onCancel={onCancelDownload}
          onDelete={onRequestDeleteEmbedModel}
          downloadProgress={downloadProgress}
        >
          {embedConfig?.provider === 'local' && (
            <div className="space-y-3">
              <p className="text-sm font-medium text-text-primary">{t('settings.embeddingLocalModelSelect')}</p>
              <div className="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-3 gap-3">
                {([
                  {
                    id: 'Qwen3Embedding06B' as const,
                    label: t('settings.embeddingModelBest'),
                    desc: t('settings.embeddingModelBestDesc'),
                  },
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
                    onClick={() => onEmbedLocalModelChange(opt.id)}
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
          onDownload={onDownloadOcrModels}
          onDelete={() => setDeleteManagedModel('ocr')}
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
          size={whisperModelSize(videoConfig?.whisperModel)}
          onDownload={onWhisperDownload}
          onDelete={() => setDeleteManagedModel('whisper')}
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
                    onClick={() => onWhisperModelChange(opt.id)}
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

        {appConfig && (
          <div className="space-y-3 rounded-xl border border-border bg-surface-1/40 p-4">
            <div>
              <p className="text-sm font-semibold text-text-primary">{t('settings.localSpeechModels')}</p>
              <p className="mt-1 text-xs leading-relaxed text-text-tertiary">{t('settings.localSpeechModelsDesc')}</p>
            </div>
            <TextToSpeechSettingsPanel
              providerScope="local"
              appConfig={appConfig}
              loading={appConfigLoading}
              onChange={onAppConfigChange}
              onMarkDirty={onMarkModelsDirty}
              onSave={onAppConfigSave}
            />
            <SpeechToTextSettingsPanel
              providerScope="local"
              appConfig={appConfig}
              loading={appConfigLoading}
              onChange={onAppConfigChange}
              onMarkDirty={onMarkModelsDirty}
              onSave={onAppConfigSave}
            />
          </div>
        )}

        <OfficeRuntimePanel
          readiness={officeRuntime}
          preparing={officePreparing}
          onPrepare={onPrepareOfficeRuntime}
          onRefresh={onRefreshOfficeRuntime}
          onAskAiPrepare={onAskAiPrepareOfficeRuntime}
        />

        {/* Disk Usage Summary */}
        <CollapsiblePanel title={t('settings.modelDiskUsage')}>
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-text-tertiary">
            <span className="flex items-center gap-1.5">
              <span className="h-2 w-2 rounded-full bg-accent" />
              {t('settings.modelsEmbedding')}: {embeddingModelSize(embedConfig?.localModel)}
            </span>
            <span className="flex items-center gap-1.5">
              <span className="h-2 w-2 rounded-full bg-success" />
              {t('settings.modelsOcr')}: {t('settings.ocrModelSize')}
            </span>
            <span className="flex items-center gap-1.5">
              <span className="h-2 w-2 rounded-full bg-warning" />
              {t('settings.modelsWhisper')}: {whisperModelSize(videoConfig?.whisperModel) ?? '—'}
            </span>
          </div>
        </CollapsiblePanel>

        {/* Network mirrors (advanced) */}
        {appConfig && (
          <NetworkMirrorsPanel
            appConfig={appConfig}
            loading={appConfigLoading}
            onChange={onAppConfigChange}
            onMarkDirty={onMarkModelsDirty}
            onSave={onAppConfigSave}
          />
        )}
      </div>

      {/* Delete embedding model confirmation */}
      <ConfirmDialog
        open={deleteEmbedModelConfirmOpen}
        onClose={onCloseDeleteEmbedModel}
        onConfirm={onConfirmDeleteEmbedModel}
        title={t('settings.deleteModel')}
        message={t('settings.deleteModelConfirm')}
        confirmText={t('common.delete')}
        variant="danger"
      />
      <ConfirmDialog
        open={deleteManagedModel !== null}
        onClose={() => setDeleteManagedModel(null)}
        onConfirm={() => {
          const action = deleteManagedModel === 'ocr' ? onDeleteOcrModels : onDeleteWhisperModel;
          void Promise.resolve(action()).finally(() => setDeleteManagedModel(null));
        }}
        title={t('settings.deleteModel')}
        message={t('settings.deleteModelConfirm')}
        confirmText={t('common.delete')}
        variant="danger"
      />
    </Section>
  );
}
