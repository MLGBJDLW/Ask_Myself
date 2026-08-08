import { open } from '@tauri-apps/plugin-dialog';
import { Cat, Download, RefreshCw, Trash2 } from 'lucide-react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { toast } from 'sonner';

import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import type { AppConfig, CompanionSettings } from '../../types/conversation';
import { Button } from '../../components/ui/Button';
import { NexaSelect } from '../../components/ui/overlay/NexaSelect';
import { DEFAULT_COMPANION_SETTINGS } from './defaults';

interface CompanionSettingsCardProps {
  appConfig: AppConfig | null;
  loading: boolean;
  onChange: (config: AppConfig) => void;
  onSave: (config: AppConfig) => void;
}

function Toggle({
  checked,
  disabled,
  label,
  onChange,
}: {
  checked: boolean;
  disabled?: boolean;
  label: string;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label className="flex cursor-pointer items-center gap-2 text-sm text-text-primary">
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
        className="rounded border-border"
      />
      <span>{label}</span>
    </label>
  );
}

function CompanionPackPreview({ pack }: { pack: api.NormalizedCompanionPack | null }) {
  const { t } = useTranslation();
  const [asset, setAsset] = useState<string | null>(null);

  useEffect(() => {
    let current = true;
    setAsset(null);
    if (!pack) return () => { current = false; };
    void api.readCompanionAsset(pack.id, pack.contentHash)
      .then((result) => { if (current) setAsset(result.dataUrl); })
      .catch(() => { if (current) setAsset(null); });
    return () => { current = false; };
  }, [pack]);

  if (!pack || !asset) {
    return (
      <div className="flex h-28 w-28 items-center justify-center rounded-2xl border border-border bg-surface-1 text-text-tertiary">
        {pack ? <span className="px-2 text-center text-[10px]">{t('companion.previewUnavailable')}</span> : <Cat size={28} />}
      </div>
    );
  }

  return (
    <div
      className="h-28 w-28 rounded-2xl border border-border bg-center bg-no-repeat shadow-sm [image-rendering:pixelated]"
      style={{
        backgroundImage: `url(${asset})`,
        backgroundSize: `${pack.frame.columns * 100}% ${pack.frame.rows * 100}%`,
        backgroundPosition: '0 0',
      }}
      aria-label={pack.displayName}
    />
  );
}

export function CompanionSettingsCard({
  appConfig,
  loading,
  onChange,
  onSave,
}: CompanionSettingsCardProps) {
  const { t } = useTranslation();
  const settings = appConfig?.companion ?? DEFAULT_COMPANION_SETTINGS;
  const [expanded, setExpanded] = useState(false);
  const [catalog, setCatalog] = useState<api.CompanionPackCatalog>({ packs: [], errors: [] });
  const [catalogLoading, setCatalogLoading] = useState(false);

  const refresh = useCallback(async () => {
    setCatalogLoading(true);
    try {
      const next = await api.scanCompanionPacks();
      setCatalog({
        packs: Array.isArray(next?.packs) ? next.packs : [],
        errors: Array.isArray(next?.errors) ? next.errors : [],
      });
    } catch {
      toast.error(t('companion.loadFailed'));
    } finally {
      setCatalogLoading(false);
    }
  }, [t]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const selectedPack = useMemo(
    () => catalog.packs.find((pack) => pack.id === settings.selectedPetId) ?? catalog.packs[0] ?? null,
    [catalog.packs, settings.selectedPetId],
  );

  const update = (patch: Partial<CompanionSettings>, save = true) => {
    if (!appConfig) return;
    const next = {
      ...appConfig,
      companion: { ...settings, ...patch },
    };
    onChange(next);
    if (save) onSave(next);
  };

  const importPack = async () => {
    const selected = await open({
      multiple: false,
      filters: [{ name: t('companion.packs'), extensions: ['json'] }],
    });
    if (typeof selected !== 'string') return;
    try {
      const imported = await api.importCompanionPack(selected);
      toast.success(t('companion.imported'));
      update({ selectedPetId: imported.id });
      await refresh();
    } catch (error) {
      console.error('[companion] pack import failed', error);
      toast.error(t('companion.importFailed'));
    }
  };

  const deleteSelected = async () => {
    if (!selectedPack?.managed) return;
    if (!window.confirm(t('companion.deleteConfirm', { name: selectedPack.displayName }))) return;
    await api.deleteManagedCompanionPack(selectedPack.id);
    if (settings.selectedPetId === selectedPack.id) update({ selectedPetId: null });
    toast.success(t('companion.deleted'));
    await refresh();
  };

  return (
    <div data-testid="companion-settings-card" className="rounded-xl border border-border bg-surface-2 p-3">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center">
        <CompanionPackPreview pack={selectedPack} />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="text-sm font-semibold text-text-primary">{t('companion.title')}</h3>
            <span className={`rounded-full px-2 py-0.5 text-[10px] font-medium ${settings.enabled ? 'bg-success/15 text-success' : 'bg-surface-3 text-text-tertiary'}`}>
              {settings.enabled ? t('companion.enabled') : t('companion.disabledSummary')}
            </span>
          </div>
          <p className="mt-1 text-xs text-text-tertiary">{t('companion.description')}</p>
          <p className="mt-2 truncate text-sm font-medium text-text-secondary">
            {selectedPack?.displayName ?? t('companion.noPets')}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Toggle
            checked={settings.enabled}
            disabled={!appConfig || loading}
            label={t('companion.enabled')}
            onChange={(enabled) => update({
              enabled,
              selectedPetId: enabled && !settings.selectedPetId ? selectedPack?.id ?? null : settings.selectedPetId,
            }, true)}
          />
          <Button type="button" size="sm" variant="secondary" onClick={() => setExpanded((value) => !value)}>
            {t('companion.configure')}
          </Button>
        </div>
      </div>

      {expanded && (
        <div className="mt-4 space-y-5 border-t border-border pt-4">
          <section className="space-y-3">
            <h4 className="text-xs font-semibold uppercase tracking-wide text-text-tertiary">{t('companion.general')}</h4>
            <div className="grid gap-3 md:grid-cols-3">
              <label className="text-xs text-text-secondary">
                {t('companion.selectedPet')}
                <NexaSelect className="mt-1 w-full" value={selectedPack?.id ?? ''} onChange={(event) => update({ selectedPetId: event.target.value || null })}>
                  <option value="">{t('companion.noPets')}</option>
                  {catalog.packs.map((pack) => <option key={`${pack.id}:${pack.contentHash}`} value={pack.id}>{pack.displayName}</option>)}
                </NexaSelect>
              </label>
              <label className="text-xs text-text-secondary">
                {t('companion.displayMode')}
                <NexaSelect className="mt-1 w-full" value={settings.displayMode} onChange={(event) => update({ displayMode: event.target.value as CompanionSettings['displayMode'] })}>
                  <option value="always">{t('companion.displayAlways')}</option>
                  <option value="during_tasks">{t('companion.displayTasks')}</option>
                  <option value="manual">{t('companion.displayManual')}</option>
                </NexaSelect>
              </label>
              <label className="text-xs text-text-secondary">
                {t('companion.interactionMode')}
                <NexaSelect className="mt-1 w-full" value={settings.interactionMode} onChange={(event) => update({ interactionMode: event.target.value as CompanionSettings['interactionMode'] })}>
                  <option value="smart">{t('companion.interactionSmart')}</option>
                  <option value="locked">{t('companion.interactionLocked')}</option>
                  <option value="click_through">{t('companion.interactionClickThrough')}</option>
                </NexaSelect>
              </label>
            </div>
            <div className="grid gap-2 md:grid-cols-3">
              <Toggle checked={settings.showInChat} label={t('companion.showInChat')} onChange={(showInChat) => update({ showInChat })} />
              <Toggle checked={settings.autoShowOnStart} label={t('companion.autoShow')} onChange={(autoShowOnStart) => update({ autoShowOnStart })} />
              <Toggle checked={settings.continueWhenMainHidden} label={t('companion.continueHidden')} onChange={(continueWhenMainHidden) => update({ continueWhenMainHidden })} />
            </div>
          </section>

          <section className="space-y-3">
            <h4 className="text-xs font-semibold uppercase tracking-wide text-text-tertiary">{t('companion.appearance')}</h4>
            <div className="grid gap-3 md:grid-cols-3">
              <label className="text-xs text-text-secondary">
                {t('companion.scale')} ({Math.round(settings.scale * 100)}%)
                <input
                  type="range"
                  min="0.5"
                  max="2"
                  step="0.05"
                  value={settings.scale}
                  onChange={(event) => update({ scale: Number(event.target.value) }, false)}
                  onPointerUp={(event) => update({ scale: Number(event.currentTarget.value) }, true)}
                  onKeyUp={(event) => update({ scale: Number(event.currentTarget.value) }, true)}
                  className="mt-2 w-full"
                />
              </label>
              <label className="text-xs text-text-secondary">{t('companion.fps')}<NexaSelect className="mt-1 w-full" value={settings.animationFpsCap} onChange={(event) => update({ animationFpsCap: Number(event.target.value) as 24 | 30 | 60 })}><option value="24">24</option><option value="30">30</option><option value="60">60</option></NexaSelect></label>
              <div className="space-y-2"><Toggle checked={settings.reducedMotion} label={t('companion.reducedMotion')} onChange={(reducedMotion) => update({ reducedMotion })} /><Toggle checked={settings.showBubbles} label={t('companion.showBubbles')} onChange={(showBubbles) => update({ showBubbles })} /><Toggle checked={settings.privacyMode} label={t('companion.privacy')} onChange={(privacyMode) => update({ privacyMode })} /></div>
            </div>
          </section>

          <section className="space-y-3">
            <h4 className="text-xs font-semibold uppercase tracking-wide text-text-tertiary">{t('companion.behavior')}</h4>
            <div className="grid gap-3 md:grid-cols-3">
              <Toggle checked={settings.alwaysOnTop} label={t('companion.alwaysOnTop')} onChange={(alwaysOnTop) => update({ alwaysOnTop })} />
              <Toggle checked={settings.lockPosition} label={t('companion.lockPosition')} onChange={(lockPosition) => update({ lockPosition })} />
              <label className="text-xs text-text-secondary">{t('companion.anchor')}<NexaSelect className="mt-1 w-full" value={settings.anchor} onChange={(event) => update({ anchor: event.target.value as CompanionSettings['anchor'] })}><option value="bottom_left">{t('companion.bottomLeft')}</option><option value="bottom_right">{t('companion.bottomRight')}</option><option value="free">{t('companion.free')}</option></NexaSelect></label>
            </div>
          </section>

          <section className="space-y-3">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <h4 className="text-xs font-semibold uppercase tracking-wide text-text-tertiary">{t('companion.packs')}</h4>
              <div className="flex gap-2">
                <Button size="sm" variant="secondary" icon={<RefreshCw size={13} />} loading={catalogLoading} onClick={() => void refresh()}>{t('companion.refresh')}</Button>
                <Button size="sm" variant="secondary" icon={<Download size={13} />} onClick={() => void importPack()}>{t('companion.import')}</Button>
                {selectedPack?.managed && <Button size="sm" variant="ghost" icon={<Trash2 size={13} />} onClick={() => void deleteSelected()}>{t('companion.delete')}</Button>}
              </div>
            </div>
            <label className="block text-xs text-text-secondary">
              {t('companion.codexPath')}
              <input
                className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-sm text-text-primary"
                value={settings.codexImportPath ?? ''}
                onChange={(event) => update({ codexImportPath: event.target.value || null })}
                onBlur={(event) => update({ codexImportPath: event.target.value || null }, true)}
                placeholder="~/.codex"
              />
              <span className="mt-1 block text-[11px] text-text-tertiary">{t('companion.codexPathHint')}</span>
            </label>
            {selectedPack && (
              <div className="flex flex-wrap gap-2 text-[11px] text-text-tertiary">
                <span>{selectedPack.managed ? t('companion.managed') : t('companion.codex')}</span>
                <span aria-hidden="true">&middot;</span>
                <span>{selectedPack.frame.columns}&times;{selectedPack.frame.rows}</span>
                {selectedPack.compatibility === 'experimental' && (
                  <>
                    <span aria-hidden="true">&middot;</span>
                    <span className="text-warning">{t('companion.experimental')}</span>
                  </>
                )}
              </div>
            )}
            {catalog.errors.length > 0 && <details className="rounded-md border border-warning/30 bg-warning/5 p-2 text-xs text-warning"><summary>{t('companion.validationErrors', { count: catalog.errors.length })}</summary><ul className="mt-2 space-y-1 text-[11px]">{catalog.errors.map((error) => <li key={`${error.manifestPath}:${error.message}`} title={error.manifestPath}>{error.message}</li>)}</ul></details>}
          </section>
        </div>
      )}
    </div>
  );
}
