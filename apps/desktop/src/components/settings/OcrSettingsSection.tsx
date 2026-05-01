import { Save, ScanLine } from 'lucide-react';
import { useTranslation } from '../../i18n';
import type { OcrConfig } from '../../types/ocr';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Section } from './SettingsSection';

interface OcrSettingsSectionProps {
  ocrConfig: OcrConfig | null;
  ocrSaveLoading: boolean;
  onConfigChange: (config: OcrConfig) => void;
  onMarkDirty: () => void;
  onSave: () => void;
}

export function OcrSettingsSection({
  ocrConfig,
  ocrSaveLoading,
  onConfigChange,
  onMarkDirty,
  onSave,
}: OcrSettingsSectionProps) {
  const { t } = useTranslation();

  const updateConfig = (patch: Partial<OcrConfig>) => {
    if (!ocrConfig) return;
    onConfigChange({ ...ocrConfig, ...patch });
    onMarkDirty();
  };

  return (
    <Section
      icon={<ScanLine size={20} />}
      title={t('settings.ocrSection')}
      delay={0.03}
      collapsible
      defaultOpen={false}
      summary={ocrConfig ? (
        <span className="rounded-full border border-border/60 bg-surface-2 px-2 py-1 text-[11px] text-text-secondary">
          {ocrConfig.enabled ? t('settings.skillFilterEnabled') : t('settings.skillFilterDisabled')}
        </span>
      ) : undefined}
    >
      {ocrConfig && (
        <div className="space-y-5">
          {/* Enable toggle */}
          <div className="flex items-center justify-between">
            <div>
              <p className="text-sm font-medium text-text-primary">{t('settings.ocrEnabled')}</p>
              <p className="text-xs text-text-tertiary">{t('settings.ocrEnabledDesc')}</p>
            </div>
            <button
              onClick={() => updateConfig({ enabled: !ocrConfig.enabled })}
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
              onChange={(e) => updateConfig({ confidenceThreshold: parseFloat(e.target.value) })}
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
              onClick={() => updateConfig({ llmFallbackEnabled: !ocrConfig.llmFallbackEnabled })}
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
              onChange={(e) => updateConfig({ detLimitSideLen: parseInt(e.target.value) || 960 })}
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
              onClick={() => updateConfig({ useCls: !ocrConfig.useCls })}
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
              onClick={onSave}
            >
              {t('settings.saveConfig')}
            </Button>
          </div>
        </div>
      )}
    </Section>
  );
}
