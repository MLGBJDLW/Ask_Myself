import {
  AlertTriangle,
  CheckCircle,
  ChevronDown,
  Film,
  Loader2,
  Mic,
  RefreshCw,
  Save,
  Settings2,
  Trash2,
  XCircle,
} from 'lucide-react';
import { useTranslation } from '../../i18n';
import type { FfmpegDownloadProgress, VideoConfig } from '../../types/video';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { ConfirmDialog } from '../ui/ConfirmDialog';
import { Input } from '../ui/Input';
import { Section } from './SettingsSection';

interface VideoSettingsSectionProps {
  videoConfig: VideoConfig | null;
  ffmpegAvailable: boolean | null;
  ffmpegDownloading: boolean;
  ffmpegProgress: FfmpegDownloadProgress | null;
  whisperModelExists: boolean | null;
  videoSaveLoading: boolean;
  showAdvancedVideo: boolean;
  deleteModelConfirmOpen: boolean;
  micDevices: MediaDeviceInfo[];
  micDeviceId: string | null;
  onConfigChange: (config: VideoConfig) => void;
  onMarkDirty: () => void;
  onFfmpegDownload: () => void;
  onAdvancedToggle: () => void;
  onMicDeviceChange: (deviceId: string | null) => void;
  onRefreshMics: () => void;
  onRequestDeleteModel: () => void;
  onCloseDeleteModel: () => void;
  onConfirmDeleteModel: () => void;
  onSave: () => void;
}

export function VideoSettingsSection({
  videoConfig,
  ffmpegAvailable,
  ffmpegDownloading,
  ffmpegProgress,
  whisperModelExists,
  videoSaveLoading,
  showAdvancedVideo,
  deleteModelConfirmOpen,
  micDevices,
  micDeviceId,
  onConfigChange,
  onMarkDirty,
  onFfmpegDownload,
  onAdvancedToggle,
  onMicDeviceChange,
  onRefreshMics,
  onRequestDeleteModel,
  onCloseDeleteModel,
  onConfirmDeleteModel,
  onSave,
}: VideoSettingsSectionProps) {
  const { t } = useTranslation();

  const updateConfig = (patch: Partial<VideoConfig>) => {
    if (!videoConfig) return;
    onConfigChange({ ...videoConfig, ...patch });
    onMarkDirty();
  };

  return (
    <Section
      icon={<Film size={20} />}
      title={t('settings.videoSection')}
      delay={0.06}
      collapsible
      defaultOpen={false}
      summary={videoConfig ? (
        <span className="rounded-full border border-border/60 bg-surface-2 px-2 py-1 text-[11px] text-text-secondary">
          {videoConfig.enabled ? t('settings.skillFilterEnabled') : t('settings.skillFilterDisabled')}
        </span>
      ) : undefined}
    >
      {videoConfig ? (
        <div className="space-y-5">
          {/* Enable toggle */}
          <div className="flex items-center justify-between">
            <div>
              <p className="text-sm font-medium text-text-primary">{t('settings.videoEnabled')}</p>
              <p className="text-xs text-text-tertiary">{t('settings.videoEnabledDesc')}</p>
            </div>
            <button
              onClick={() => updateConfig({ enabled: !videoConfig.enabled })}
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
            ) : ffmpegDownloading ? (
              <Badge variant="default" className="gap-1">
                <Loader2 size={12} className="animate-spin" />
                {t('settings.videoFfmpegDownloading')}
              </Badge>
            ) : (
              <Badge variant="default" className="gap-1">
                <XCircle size={12} className="text-danger" />
                {t('settings.videoFfmpegNotFound')}
              </Badge>
            )}
          </div>
          {ffmpegDownloading && ffmpegProgress && (
            <div className="space-y-1">
              <div className="h-2 w-full overflow-hidden rounded-full bg-surface-3">
                <div
                  className="h-full rounded-full bg-accent transition-all duration-300"
                  style={{ width: `${Math.min(ffmpegProgress.progressPct, 100)}%` }}
                />
              </div>
              <p className="text-xs text-text-tertiary">{ffmpegProgress.status}</p>
            </div>
          )}
          {ffmpegAvailable === false && !ffmpegDownloading && (
            <div className="flex items-start gap-2 rounded-lg border border-warning/30 bg-warning/5 p-2">
              <AlertTriangle size={14} className="mt-0.5 shrink-0 text-warning" />
              <div className="flex flex-col gap-2">
                <p className="text-xs text-warning">{t('settings.videoFfmpegHint')}</p>
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={onFfmpegDownload}
                  disabled={ffmpegDownloading}
                >
                  {t('settings.videoFfmpegDownload')}
                </Button>
              </div>
            </div>
          )}

          {/* Language */}
          <div>
            <p className="text-sm font-medium text-text-primary mb-1">{t('settings.videoLanguage')}</p>
            <p className="text-xs text-text-tertiary mb-2">{t('settings.videoLanguageDesc')}</p>
            <Input
              type="text"
              value={videoConfig.language ?? ''}
              onChange={(e) => updateConfig({ language: e.target.value || null })}
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
              onClick={() => updateConfig({ translateToEnglish: !videoConfig.translateToEnglish })}
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
              onClick={() => updateConfig({ frameExtractionEnabled: !videoConfig.frameExtractionEnabled })}
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
                onChange={(e) => updateConfig({ frameIntervalSecs: parseInt(e.target.value) || 30 })}
                className="w-32"
              />
            </div>
          )}

          {/* Voice Input - Microphone Selection */}
          <div className="space-y-3 border-t border-border pt-4 mt-4">
            <div className="flex items-center gap-2">
              <Mic size={16} className="text-text-secondary" />
              <p className="text-sm font-medium text-text-primary">{t('voice.microphoneSection')}</p>
            </div>
            <div>
              <p className="text-xs text-text-tertiary mb-2">{t('voice.microphoneDeviceDesc')}</p>
              <div className="flex items-center gap-2">
                <select
                  value={micDeviceId ?? ''}
                  onChange={(e) => onMicDeviceChange(e.target.value || null)}
                  className="flex-1 rounded-lg border border-border bg-surface-0 px-3 py-2 text-sm text-text-primary outline-none transition-colors duration-fast hover:border-border-hover focus:border-accent focus:ring-1 focus:ring-accent/30"
                >
                  <option value="">{t('voice.microphoneDefault')}</option>
                  {micDevices.map((device, index) => (
                    <option key={device.deviceId} value={device.deviceId}>
                      {device.label || `${t('voice.microphoneDeviceN')} ${index + 1}`}
                    </option>
                  ))}
                </select>
                <button
                  type="button"
                  onClick={onRefreshMics}
                  className="rounded-lg border border-border bg-surface-0 p-2 text-text-tertiary transition-colors hover:border-border-hover hover:text-text-secondary cursor-pointer"
                  title={t('voice.microphoneRefresh')}
                >
                  <RefreshCw size={14} />
                </button>
              </div>
            </div>
          </div>

          {/* Advanced Settings - collapsible */}
          <div className="space-y-4 border-t border-border pt-4 mt-4">
            <button
              type="button"
              onClick={onAdvancedToggle}
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
                    onClick={() => updateConfig({ useGpu: !videoConfig.useGpu })}
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
                    onClick={() => updateConfig({ preferEmbeddedSubtitles: !videoConfig.preferEmbeddedSubtitles })}
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
                      onChange={(e) => updateConfig({ sceneThreshold: parseInt(e.target.value) / 100 })}
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
                      onChange={(e) => updateConfig({ beamSize: parseInt(e.target.value) })}
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
                onClick={onRequestDeleteModel}
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
              onClick={onSave}
            >
              {t('settings.videoSave')}
            </Button>
          </div>

          {/* Delete model confirmation */}
          <ConfirmDialog
            open={deleteModelConfirmOpen}
            onClose={onCloseDeleteModel}
            onConfirm={onConfirmDeleteModel}
            title={t('settings.videoDeleteModel')}
            message={t('settings.videoDeleteConfirm')}
            confirmText={t('common.delete')}
            variant="danger"
          />
        </div>
      ) : (
        <div className="flex flex-col items-center justify-center py-12 text-text-tertiary">
          <Film size={32} className="mb-3 opacity-50" />
          <p className="text-sm font-medium text-text-primary mb-1">{t('settings.videoUnavailable')}</p>
          <p className="text-xs text-center max-w-md">
            {t('settings.videoUnavailableRequires')}{' '}
            {t('settings.videoUnavailableRebuild')}
          </p>
        </div>
      )}
    </Section>
  );
}
