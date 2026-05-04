import { Clock, RotateCcw, Save, Settings2, Star } from 'lucide-react';
import { useTranslation, type Locale } from '../../i18n';
import { useUpdater } from '../../lib/useUpdater';
import type { AppConfig } from '../../types/conversation';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { ThemeSwitcher } from '../ui/ThemeSwitcher';
import { CollapsiblePanel, Section } from './SettingsSection';
import { ToolApprovalControl, type ToolApprovalMode } from './ToolApprovalControl';
import { UpdateSettingsPanel } from './UpdateSettingsPanel';

type UpdaterState = ReturnType<typeof useUpdater>;

interface AppearanceSettingsTabProps {
  locale: Locale;
  setLocale: (locale: Locale) => void;
  availableLocales: { code: Locale; name: string }[];
  appVersion: string;
  updater: UpdaterState;
  appConfig: AppConfig | null;
  appConfigLoading: boolean;
  onAppConfigChange: (config: AppConfig) => void;
  onAppConfigSave: () => void;
  onRerunWizard: () => void;
}

export function AppearanceSettingsTab({
  locale,
  setLocale,
  availableLocales,
  appVersion,
  updater,
  appConfig,
  appConfigLoading,
  onAppConfigChange,
  onAppConfigSave,
  onRerunWizard,
}: AppearanceSettingsTabProps) {
  const { t } = useTranslation();
  const agentTimeoutUnlimited = (appConfig?.agentTimeoutSecs ?? 180) <= 0;

  return (
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

        {/* App update */}
        <UpdateSettingsPanel appVersion={appVersion} updater={updater} />

        {/* Re-run setup wizard */}
        <div className="border-t border-border pt-4 mt-4">
          <p className="mb-2 text-sm font-medium text-text-primary">{t('wizard.rerunLabel')}</p>
          <p className="mb-3 text-xs text-text-tertiary">{t('wizard.rerunDescription')}</p>
          <Button
            variant="secondary"
            size="sm"
            icon={<RotateCcw size={14} />}
            onClick={onRerunWizard}
          >
            {t('wizard.rerunButton')}
          </Button>
        </div>

        {/* Timeout Settings */}
        <CollapsiblePanel
          title={t('settings.timeout')}
          defaultOpen={false}
          summary={<Clock size={14} className="text-text-tertiary" />}
        >
          {appConfig && (
            <div className="space-y-4">
              <div className="grid grid-cols-2 gap-4">
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.toolTimeout')}</label>
                  <Input
                    type="number"
                    value={appConfig.toolTimeoutSecs}
                    onChange={(e) => onAppConfigChange({ ...appConfig, toolTimeoutSecs: parseInt(e.target.value) || 30 })}
                    min={5}
                    max={300}
                    step={5}
                  />
                  <p className="text-xs text-text-tertiary">
                    {t('settings.toolTimeoutDesc')}
                  </p>
                </div>
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.agentTimeout')}</label>
                  <div className="flex gap-2">
                    <Input
                      type="number"
                      value={appConfig.agentTimeoutSecs}
                      onChange={(e) => {
                        const parsed = Number.parseInt(e.target.value, 10);
                        onAppConfigChange({
                          ...appConfig,
                          agentTimeoutSecs: Number.isFinite(parsed) ? Math.max(0, parsed) : 180,
                        });
                      }}
                      min={0}
                      max={3600}
                      step={30}
                      disabled={agentTimeoutUnlimited}
                    />
                    <Button
                      type="button"
                      variant={agentTimeoutUnlimited ? 'primary' : 'secondary'}
                      size="md"
                      className="shrink-0"
                      onClick={() =>
                        onAppConfigChange({
                          ...appConfig,
                          agentTimeoutSecs: agentTimeoutUnlimited ? 180 : 0,
                        })
                      }
                    >
                      {t('settings.agentTimeoutNoLimit')}
                    </Button>
                  </div>
                  <p className="text-xs text-text-tertiary">
                    {t('settings.agentTimeoutDesc')}
                  </p>
                </div>
              </div>
              <div className="grid grid-cols-2 gap-4">
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.llmTimeout')}</label>
                  <Input
                    type="number"
                    value={appConfig.llmTimeoutSecs}
                    onChange={(e) => onAppConfigChange({ ...appConfig, llmTimeoutSecs: parseInt(e.target.value) || 300 })}
                    min={10}
                    max={600}
                    step={10}
                  />
                  <p className="text-xs text-text-tertiary">
                    {t('settings.llmTimeoutDesc')}
                  </p>
                </div>
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.mcpTimeout')}</label>
                  <Input
                    type="number"
                    value={appConfig.mcpCallTimeoutSecs}
                    onChange={(e) => onAppConfigChange({ ...appConfig, mcpCallTimeoutSecs: parseInt(e.target.value) || 60 })}
                    min={5}
                    max={300}
                    step={5}
                  />
                  <p className="text-xs text-text-tertiary">
                    {t('settings.mcpTimeoutDesc')}
                  </p>
                </div>
              </div>
              <div className="flex justify-end">
                <Button
                  variant="primary"
                  size="sm"
                  icon={<Save size={14} />}
                  loading={appConfigLoading}
                  onClick={onAppConfigSave}
                >
                  {t('common.save')}
                </Button>
              </div>
            </div>
          )}
        </CollapsiblePanel>

        {/* Advanced Settings */}
        <CollapsiblePanel
          title={t('settings.advanced')}
          defaultOpen={false}
          summary={<Settings2 size={14} className="text-text-tertiary" />}
        >
          {appConfig && (
            <div className="space-y-4">
              {/* Cache & Search */}
              <div className="grid grid-cols-2 gap-4">
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.cacheTtl')}</label>
                  <Input
                    type="number"
                    value={appConfig.cacheTtlHours}
                    onChange={(e) => onAppConfigChange({ ...appConfig, cacheTtlHours: Math.max(0, Math.min(168, parseInt(e.target.value) || 0)) })}
                    min={0}
                    max={168}
                  />
                  <p className="text-xs text-text-tertiary">{t('settings.cacheTtlDesc')}</p>
                </div>
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.searchLimit')}</label>
                  <Input
                    type="number"
                    value={appConfig.defaultSearchLimit}
                    onChange={(e) => onAppConfigChange({ ...appConfig, defaultSearchLimit: Math.max(1, Math.min(100, parseInt(e.target.value) || 20)) })}
                    min={1}
                    max={100}
                  />
                  <p className="text-xs text-text-tertiary">{t('settings.searchLimitDesc')}</p>
                </div>
              </div>
              <div className="space-y-2">
                <label className="text-sm font-medium text-text-primary">{t('settings.searchSimilarity')}</label>
                <Input
                  type="number"
                  value={appConfig.minSearchSimilarity}
                  onChange={(e) => onAppConfigChange({ ...appConfig, minSearchSimilarity: Math.max(0, Math.min(1, parseFloat(e.target.value) || 0.2)) })}
                  min={0}
                  max={1}
                  step={0.05}
                />
                <p className="text-xs text-text-tertiary">{t('settings.searchSimilarityDesc')}</p>
              </div>

              {/* File Size Limits */}
              <h4 className="text-xs font-medium text-text-secondary mt-2">{t('settings.fileSizeLimits')}</h4>
              <div className="grid grid-cols-3 gap-4">
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.maxTextFileSize')}</label>
                  <Input
                    type="number"
                    value={Math.round(appConfig.maxTextFileSize / (1024 * 1024))}
                    onChange={(e) => onAppConfigChange({ ...appConfig, maxTextFileSize: Math.max(1, parseInt(e.target.value) || 100) * 1024 * 1024 })}
                    min={1}
                    max={1024}
                  />
                  <p className="text-xs text-text-tertiary">{t('settings.maxTextFileSizeDesc')}</p>
                </div>
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.maxVideoFileSize')}</label>
                  <Input
                    type="number"
                    value={Math.round(appConfig.maxVideoFileSize / (1024 * 1024 * 1024))}
                    onChange={(e) => onAppConfigChange({ ...appConfig, maxVideoFileSize: Math.max(1, parseInt(e.target.value) || 2) * 1024 * 1024 * 1024 })}
                    min={1}
                    max={10}
                  />
                </div>
                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.maxAudioFileSize')}</label>
                  <Input
                    type="number"
                    value={Math.round(appConfig.maxAudioFileSize / (1024 * 1024))}
                    onChange={(e) => onAppConfigChange({ ...appConfig, maxAudioFileSize: Math.max(1, parseInt(e.target.value) || 500) * 1024 * 1024 })}
                    min={1}
                    max={2048}
                  />
                </div>
              </div>

              {/* Agent Behavior */}
              <div className="space-y-3 mt-2">
                <label className="flex items-center gap-2 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={appConfig.dynamicToolVisibility ?? true}
                    onChange={(e) => onAppConfigChange({ ...appConfig, dynamicToolVisibility: e.target.checked })}
                    className="rounded border-border"
                  />
                  <span className="text-sm font-medium text-text-primary">{t('settings.dynamicTools')}</span>
                </label>
                <p className="text-xs text-text-tertiary ml-6">{t('settings.dynamicToolsDesc')}</p>

                <label className="flex items-center gap-2 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={appConfig.traceEnabled ?? true}
                    onChange={(e) => onAppConfigChange({ ...appConfig, traceEnabled: e.target.checked })}
                    className="rounded border-border"
                  />
                  <span className="text-sm font-medium text-text-primary">{t('settings.traceEnabled')}</span>
                </label>
                <p className="text-xs text-text-tertiary ml-6">{t('settings.traceEnabledDesc')}</p>

                <label className="flex items-center gap-2 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={appConfig.confirmDestructive ?? false}
                    onChange={(e) => onAppConfigChange({ ...appConfig, confirmDestructive: e.target.checked })}
                    className="rounded border-border"
                  />
                  <span className="text-sm font-medium text-text-primary">{t('settings.confirmDestructive')}</span>
                </label>
                <p className="text-xs text-text-tertiary ml-6">{t('settings.confirmDestructiveDesc')}</p>

                <div className="space-y-2">
                  <label className="text-sm font-medium text-text-primary">{t('settings.shellAccessMode')}</label>
                  <p className="text-xs text-text-tertiary">{t('settings.shellAccessModeDesc')}</p>
                  <div className="grid gap-2 md:grid-cols-3">
                    {[
                      {
                        value: 'restricted',
                        label: t('settings.shellAccessRestricted'),
                        desc: t('settings.shellAccessRestrictedDesc'),
                      },
                      {
                        value: 'confirm_all',
                        label: t('settings.shellAccessConfirmAll'),
                        desc: t('settings.shellAccessConfirmAllDesc'),
                      },
                      {
                        value: 'open',
                        label: t('settings.shellAccessOpen'),
                        desc: t('settings.shellAccessOpenDesc'),
                      },
                    ].map((option) => (
                      <label
                        key={option.value}
                        className={`cursor-pointer rounded-lg border p-3 transition-colors ${
                          (appConfig.shellAccessMode ?? 'restricted') === option.value
                            ? 'border-accent bg-accent/10'
                            : 'border-border bg-surface-2'
                        }`}
                      >
                        <div className="flex items-start gap-3">
                          <input
                            type="radio"
                            name="shell-access-mode"
                            value={option.value}
                            checked={(appConfig.shellAccessMode ?? 'restricted') === option.value}
                            onChange={() => onAppConfigChange({
                              ...appConfig,
                              shellAccessMode: option.value as 'restricted' | 'confirm_all' | 'open',
                            })}
                            className="mt-1"
                          />
                          <div className="space-y-1">
                            <div className="text-sm font-medium text-text-primary">{option.label}</div>
                            <div className="text-xs text-text-tertiary">{option.desc}</div>
                          </div>
                        </div>
                      </label>
                    ))}
                  </div>
                </div>

                <ToolApprovalControl
                  mode={appConfig.toolApprovalMode ?? 'ask'}
                  onChange={(mode: ToolApprovalMode) => onAppConfigChange({ ...appConfig, toolApprovalMode: mode })}
                />
              </div>

              <div className="flex justify-end">
                <Button
                  variant="primary"
                  size="sm"
                  icon={<Save size={14} />}
                  loading={appConfigLoading}
                  onClick={onAppConfigSave}
                >
                  {t('common.save')}
                </Button>
              </div>
            </div>
          )}
        </CollapsiblePanel>
      </div>
    </Section>
  );
}
