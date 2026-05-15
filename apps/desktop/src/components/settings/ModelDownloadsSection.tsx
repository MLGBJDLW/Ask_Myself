import { AlertTriangle, Brain, HardDrive, Mic, ScanLine } from 'lucide-react';
import { useTranslation } from '../../i18n';
import type { OfficeRuntimeReadiness } from '../../lib/api';
import type { DownloadProgress } from '../../types/ingest';
import type { AppConfig } from '../../types/conversation';
import type { EmbedderConfig, LocalModelId } from '../../types/embedder';
import type { OcrDownloadProgress } from '../../types/ocr';
import type { VideoConfig, VideoDownloadProgress, WhisperModel } from '../../types/video';
import { ConfirmDialog } from '../ui/ConfirmDialog';
import { CollapsiblePanel, Section } from './SettingsSection';
import { ModelCard } from './ModelCard';
import { NetworkMirrorsPanel } from './NetworkMirrorsPanel';
import { OfficeRuntimePanel } from './OfficeRuntimePanel';

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
  onEmbedLocalModelChange: (model: LocalModelId) => void;
  onDownloadModel: () => void;
  onCancelDownload: () => void;
  onRequestDeleteEmbedModel: () => void;
  onCloseDeleteEmbedModel: () => void;
  onConfirmDeleteEmbedModel: () => void;
  onDownloadOcrModels: () => void;
  onWhisperDownload: () => void;
  onWhisperModelChange: (model: WhisperModel) => void;
  onPrepareOfficeRuntime: () => void;
  onRefreshOfficeRuntime: () => void;
  onAskAiPrepareOfficeRuntime: () => void;
  onAppConfigChange: (config: AppConfig) => void;
  onAppConfigSave: () => void;
  onMarkModelsDirty: () => void;
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
  onEmbedLocalModelChange,
  onDownloadModel,
  onCancelDownload,
  onRequestDeleteEmbedModel,
  onCloseDeleteEmbedModel,
  onConfirmDeleteEmbedModel,
  onDownloadOcrModels,
  onWhisperDownload,
  onWhisperModelChange,
  onPrepareOfficeRuntime,
  onRefreshOfficeRuntime,
  onAskAiPrepareOfficeRuntime,
  onAppConfigChange,
  onAppConfigSave,
  onMarkModelsDirty,
}: ModelDownloadsSectionProps) {
  const { t } = useTranslation();

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
          size={embedConfig?.localModel === 'MultilingualE5Base' ? '~470 MB' : '~46 MB'}
          onDownload={onDownloadModel}
          onCancel={onCancelDownload}
          onDelete={onRequestDeleteEmbedModel}
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
              {t('settings.modelsEmbedding')}: {embedConfig?.localModel === 'MultilingualE5Base' ? '~470 MB' : '~46 MB'}
            </span>
            <span className="flex items-center gap-1.5">
              <span className="h-2 w-2 rounded-full bg-success" />
              {t('settings.modelsOcr')}: ~16 MB
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
    </Section>
  );
}
