import { LogOut, Minimize2, RotateCcw, Save, Settings2, Star } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation, type Locale } from '../../i18n';
import * as api from '../../lib/api';
import { useUpdater } from '../../lib/useUpdater';
import type { Source } from '../../types';
import type { AppConfig } from '../../types/conversation';
import type { Project } from '../../types/project';
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
  developerMode: boolean;
  onAppConfigChange: (config: AppConfig) => void;
  onAppConfigSave: (config?: AppConfig) => void;
  onDeveloperModeChange: (enabled: boolean) => void;
  onRerunWizard: () => void;
}

function toggleId(ids: string[], id: string): string[] {
  return ids.includes(id) ? ids.filter((item) => item !== id) : [...ids, id];
}

function compactPath(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() || path;
}

export function AppearanceSettingsTab({
  locale,
  setLocale,
  availableLocales,
  appVersion,
  updater,
  appConfig,
  appConfigLoading,
  developerMode,
  onAppConfigChange,
  onAppConfigSave,
  onDeveloperModeChange,
  onRerunWizard,
}: AppearanceSettingsTabProps) {
  const { t } = useTranslation();
  const [dreamSources, setDreamSources] = useState<Source[]>([]);
  const [dreamProjects, setDreamProjects] = useState<Project[]>([]);
  const dreamingConfig = appConfig?.dreaming ?? {
    enabled: true,
    idle: false,
    afterScan: false,
    afterSuccessfulTurn: false,
    schedule: false,
    idleIntervalMinutes: 180,
    scheduleIntervalMinutes: 720,
    maxArtifactsPerRun: 24,
    maxRunsPerDay: 12,
    localOnly: true,
    sourceIds: [],
    projectIds: [],
  };

  useEffect(() => {
    let cancelled = false;
    void Promise.all([
      api.listSources().catch(() => [] as Source[]),
      api.listProjects().catch(() => [] as Project[]),
    ]).then(([sources, projects]) => {
      if (cancelled) return;
      setDreamSources(sources);
      setDreamProjects(projects.filter((project) => !project.archived));
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const updateDreamingConfig = (patch: Partial<typeof dreamingConfig>) => {
    if (!appConfig) return;
    onAppConfigChange({
      ...appConfig,
      dreaming: {
        ...dreamingConfig,
        ...patch,
      },
    });
  };
  const toggleDreamSource = (sourceId: string) => {
    updateDreamingConfig({
      sourceIds: toggleId(dreamingConfig.sourceIds ?? [], sourceId),
    });
  };
  const toggleDreamProject = (projectId: string) => {
    updateDreamingConfig({
      projectIds: toggleId(dreamingConfig.projectIds ?? [], projectId),
    });
  };

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
          <div className="grid grid-cols-[repeat(auto-fit,minmax(6.75rem,1fr))] gap-2">
            {availableLocales.map((l) => (
              <button
                key={l.code}
                onClick={() => setLocale(l.code)}
                className={`min-h-10 rounded-lg border px-3 py-2 text-sm font-medium leading-snug transition-all duration-fast cursor-pointer ${
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

        <div className="border-t border-border pt-5">
          <p className="mb-2 text-sm font-medium text-text-primary">{t('settings.windowCloseBehavior')}</p>
          <p className="mb-3 text-xs leading-relaxed text-text-tertiary">
            {t('settings.windowCloseBehaviorDesc')}
          </p>
          <div className="grid gap-2 sm:grid-cols-2">
            {([
              {
                value: 'exit' as const,
                icon: LogOut,
                label: t('settings.windowCloseExit'),
                description: t('settings.windowCloseExitDesc'),
              },
              {
                value: 'minimize_to_tray' as const,
                icon: Minimize2,
                label: t('settings.windowCloseTray'),
                description: t('settings.windowCloseTrayDesc'),
              },
            ]).map((option) => {
              const selected = (appConfig?.windowCloseBehavior ?? 'exit') === option.value;
              const Icon = option.icon;
              return (
                <button
                  key={option.value}
                  type="button"
                  disabled={!appConfig || appConfigLoading}
                  aria-pressed={selected}
                  onClick={() => {
                    if (!appConfig) return;
                    const nextConfig = { ...appConfig, windowCloseBehavior: option.value };
                    onAppConfigChange(nextConfig);
                    onAppConfigSave(nextConfig);
                  }}
                  className={`group flex min-h-20 items-start gap-3 rounded-lg border px-3 py-3 text-left transition-all duration-fast disabled:cursor-not-allowed disabled:opacity-55 ${
                    selected
                      ? 'border-accent/60 bg-accent-subtle text-text-primary ring-1 ring-accent/15'
                      : 'border-border bg-surface-1/70 text-text-secondary hover:border-border-hover hover:bg-surface-2'
                  }`}
                >
                  <span className={`mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-md border ${
                    selected
                      ? 'border-accent/35 bg-accent/10 text-accent'
                      : 'border-border bg-surface-2 text-text-tertiary group-hover:text-text-secondary'
                  }`}>
                    <Icon size={15} />
                  </span>
                  <span className="min-w-0">
                    <span className="block text-sm font-medium">{option.label}</span>
                    <span className="mt-1 block text-xs leading-relaxed text-text-tertiary">
                      {option.description}
                    </span>
                  </span>
                </button>
              );
            })}
          </div>
        </div>

        {/* App update */}
        <UpdateSettingsPanel appVersion={appVersion} updater={updater} />

        <div className="border-t border-border pt-4">
          <label className="flex cursor-pointer items-start gap-3">
            <input
              type="checkbox"
              checked={developerMode}
              onChange={(event) => onDeveloperModeChange(event.target.checked)}
              className="mt-0.5 h-4 w-4 rounded border-border text-accent focus:ring-accent/30"
            />
            <span>
              <span className="block text-sm font-medium text-text-primary">
                {t('settings.developerMode')}
              </span>
              <span className="mt-1 block text-xs leading-relaxed text-text-tertiary">
                {t('settings.developerMode.description')}
              </span>
            </span>
          </label>
        </div>

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
                    checked={appConfig.dynamicToolVisibility ?? false}
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
                    checked={appConfig.autoMemoryExtraction ?? true}
                    onChange={(e) => onAppConfigChange({ ...appConfig, autoMemoryExtraction: e.target.checked })}
                    className="rounded border-border"
                  />
                  <span className="text-sm font-medium text-text-primary">{t('settings.autoMemoryExtraction')}</span>
                </label>
                <p className="text-xs text-text-tertiary ml-6">{t('settings.autoMemoryExtractionDesc')}</p>

                <label className="flex items-center gap-2 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={appConfig.autoSkillLearning ?? true}
                    onChange={(e) => onAppConfigChange({ ...appConfig, autoSkillLearning: e.target.checked })}
                    className="rounded border-border"
                  />
                  <span className="text-sm font-medium text-text-primary">{t('settings.autoSkillLearning')}</span>
                </label>
                <p className="text-xs text-text-tertiary ml-6">{t('settings.autoSkillLearningDesc')}</p>

                <div className="rounded-lg border border-border bg-surface-1 p-3 space-y-3">
                  <label className="flex items-center gap-2 cursor-pointer">
                    <input
                      type="checkbox"
                      checked={dreamingConfig.enabled}
                      onChange={(e) => updateDreamingConfig({ enabled: e.target.checked })}
                      className="rounded border-border"
                    />
                    <span className="text-sm font-medium text-text-primary">{t('settings.dreamingEnabled')}</span>
                  </label>
                  <p className="text-xs text-text-tertiary ml-6">{t('settings.dreamingEnabledDesc')}</p>

                  <div className="grid gap-3 md:grid-cols-2">
                    <div>
                      <label className="flex items-center gap-2 cursor-pointer">
                        <input
                          type="checkbox"
                          checked={dreamingConfig.idle}
                          disabled={!dreamingConfig.enabled}
                          onChange={(e) => updateDreamingConfig({ idle: e.target.checked })}
                          className="rounded border-border"
                        />
                        <span className="text-sm text-text-primary">{t('settings.dreamingIdle')}</span>
                      </label>
                      <p className="mt-1 text-xs text-text-tertiary ml-6">{t('settings.dreamingIdleDesc')}</p>
                    </div>
                    <div>
                      <label className="flex items-center gap-2 cursor-pointer">
                        <input
                          type="checkbox"
                          checked={dreamingConfig.afterScan}
                          disabled={!dreamingConfig.enabled}
                          onChange={(e) => updateDreamingConfig({ afterScan: e.target.checked })}
                          className="rounded border-border"
                        />
                        <span className="text-sm text-text-primary">{t('settings.dreamingAfterScan')}</span>
                      </label>
                      <p className="mt-1 text-xs text-text-tertiary ml-6">{t('settings.dreamingAfterScanDesc')}</p>
                    </div>
                    <div>
                      <label className="flex items-center gap-2 cursor-pointer">
                        <input
                          type="checkbox"
                          checked={dreamingConfig.afterSuccessfulTurn}
                          disabled={!dreamingConfig.enabled}
                          onChange={(e) => updateDreamingConfig({ afterSuccessfulTurn: e.target.checked })}
                          className="rounded border-border"
                        />
                        <span className="text-sm text-text-primary">{t('settings.dreamingAfterTurn')}</span>
                      </label>
                      <p className="mt-1 text-xs text-text-tertiary ml-6">{t('settings.dreamingAfterTurnDesc')}</p>
                    </div>
                    <div>
                      <label className="flex items-center gap-2 cursor-pointer">
                        <input
                          type="checkbox"
                          checked={dreamingConfig.schedule}
                          disabled={!dreamingConfig.enabled}
                          onChange={(e) => updateDreamingConfig({ schedule: e.target.checked })}
                          className="rounded border-border"
                        />
                        <span className="text-sm text-text-primary">{t('settings.dreamingSchedule')}</span>
                      </label>
                      <p className="mt-1 text-xs text-text-tertiary ml-6">{t('settings.dreamingScheduleDesc')}</p>
                    </div>
                  </div>
                  <div className="grid gap-3 md:grid-cols-4">
                    <div>
                      <label className="mb-1 block text-sm font-medium text-text-primary">{t('settings.dreamingIdleInterval')}</label>
                      <Input
                        type="number"
                        value={dreamingConfig.idleIntervalMinutes}
                        onChange={(e) => updateDreamingConfig({
                          idleIntervalMinutes: Math.max(15, Math.min(1440, parseInt(e.target.value) || 180)),
                        })}
                        min={15}
                        max={1440}
                        disabled={!dreamingConfig.enabled || !dreamingConfig.idle}
                      />
                    </div>
                    <div>
                      <label className="mb-1 block text-sm font-medium text-text-primary">{t('settings.dreamingScheduleInterval')}</label>
                      <Input
                        type="number"
                        value={dreamingConfig.scheduleIntervalMinutes}
                        onChange={(e) => updateDreamingConfig({
                          scheduleIntervalMinutes: Math.max(30, Math.min(10080, parseInt(e.target.value) || 720)),
                        })}
                        min={30}
                        max={10080}
                        disabled={!dreamingConfig.enabled || !dreamingConfig.schedule}
                      />
                    </div>
                    <div>
                      <label className="mb-1 block text-sm font-medium text-text-primary">{t('settings.dreamingMaxArtifacts')}</label>
                      <Input
                        type="number"
                        value={dreamingConfig.maxArtifactsPerRun}
                        onChange={(e) => updateDreamingConfig({
                          maxArtifactsPerRun: Math.max(1, Math.min(100, parseInt(e.target.value) || 24)),
                        })}
                        min={1}
                        max={100}
                        disabled={!dreamingConfig.enabled}
                      />
                      <p className="mt-1 text-xs text-text-tertiary">{t('settings.dreamingMaxArtifactsDesc')}</p>
                    </div>
                    <div>
                      <label className="mb-1 block text-sm font-medium text-text-primary">{t('settings.dreamingMaxRuns')}</label>
                      <Input
                        type="number"
                        value={dreamingConfig.maxRunsPerDay}
                        onChange={(e) => updateDreamingConfig({
                          maxRunsPerDay: Math.max(0, Math.min(96, parseInt(e.target.value) || 0)),
                        })}
                        min={0}
                        max={96}
                        disabled={!dreamingConfig.enabled}
                      />
                      <p className="mt-1 text-xs text-text-tertiary">{t('settings.dreamingMaxRunsDesc')}</p>
                    </div>
                  </div>

                  <label className="flex items-center gap-2 cursor-pointer">
                    <input
                      type="checkbox"
                      checked={dreamingConfig.localOnly}
                      disabled={!dreamingConfig.enabled}
                      onChange={(e) => updateDreamingConfig({ localOnly: e.target.checked })}
                      className="rounded border-border"
                    />
                    <span className="text-sm font-medium text-text-primary">{t('settings.dreamingLocalOnly')}</span>
                  </label>
                  <p className="text-xs text-text-tertiary ml-6">{t('settings.dreamingLocalOnlyDesc')}</p>

                  <div className="grid gap-3 lg:grid-cols-2">
                    <div className="rounded-md border border-border bg-surface-0 p-3">
                      <div className="flex items-center justify-between gap-2">
                        <div>
                          <div className="text-sm font-medium text-text-primary">{t('settings.dreamingSources')}</div>
                          <p className="mt-1 text-xs text-text-tertiary">{t('settings.dreamingSourcesDesc')}</p>
                        </div>
                        {(dreamingConfig.sourceIds?.length ?? 0) > 0 && (
                          <Button
                            variant="ghost"
                            size="sm"
                            onClick={() => updateDreamingConfig({ sourceIds: [] })}
                            disabled={!dreamingConfig.enabled}
                          >
                            {t('settings.dreamingAll')}
                          </Button>
                        )}
                      </div>
                      <div className="mt-3 max-h-40 space-y-2 overflow-y-auto">
                        {dreamSources.length === 0 ? (
                          <p className="text-xs text-text-tertiary">{t('settings.dreamingNoSources')}</p>
                        ) : dreamSources.map((source) => (
                          <label key={source.id} className="flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 hover:bg-surface-2">
                            <input
                              type="checkbox"
                              checked={(dreamingConfig.sourceIds ?? []).includes(source.id)}
                              disabled={!dreamingConfig.enabled}
                              onChange={() => toggleDreamSource(source.id)}
                              className="rounded border-border"
                            />
                            <span className="min-w-0 flex-1 truncate text-sm text-text-primary" title={source.rootPath}>
                              {compactPath(source.rootPath)}
                            </span>
                          </label>
                        ))}
                      </div>
                    </div>

                    <div className="rounded-md border border-border bg-surface-0 p-3">
                      <div className="flex items-center justify-between gap-2">
                        <div>
                          <div className="text-sm font-medium text-text-primary">{t('settings.dreamingProjects')}</div>
                          <p className="mt-1 text-xs text-text-tertiary">{t('settings.dreamingProjectsDesc')}</p>
                        </div>
                        {(dreamingConfig.projectIds?.length ?? 0) > 0 && (
                          <Button
                            variant="ghost"
                            size="sm"
                            onClick={() => updateDreamingConfig({ projectIds: [] })}
                            disabled={!dreamingConfig.enabled}
                          >
                            {t('settings.dreamingAll')}
                          </Button>
                        )}
                      </div>
                      <div className="mt-3 max-h-40 space-y-2 overflow-y-auto">
                        {dreamProjects.length === 0 ? (
                          <p className="text-xs text-text-tertiary">{t('settings.dreamingNoProjects')}</p>
                        ) : dreamProjects.map((project) => (
                          <label key={project.id} className="flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 hover:bg-surface-2">
                            <input
                              type="checkbox"
                              checked={(dreamingConfig.projectIds ?? []).includes(project.id)}
                              disabled={!dreamingConfig.enabled}
                              onChange={() => toggleDreamProject(project.id)}
                              className="rounded border-border"
                            />
                            <span className="min-w-0 flex-1 truncate text-sm text-text-primary" title={project.name}>
                              {project.name}
                            </span>
                          </label>
                        ))}
                      </div>
                    </div>
                  </div>
                </div>

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
                  onClick={() => onAppConfigSave()}
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
