import { convertFileSrc } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { Download, Image, Plus, RotateCcw, Save, Trash2, Upload, WandSparkles } from 'lucide-react';
import { useMemo, useState, type CSSProperties } from 'react';
import { toast } from 'sonner';

import * as api from '../../lib/api';
import { useTranslation } from '../../i18n';
import { useTheme } from '../../lib/ThemeProvider';
import {
  contrastRatio,
  customThemeToCssVariables,
  normalizeCustomTheme,
  parseCustomTheme,
  serializeCustomTheme,
  type CustomThemeDefinition,
} from '../../lib/themeProfile';
import { CollapsiblePanel } from './SettingsSection';

type Translate = ReturnType<typeof useTranslation>['t'];

function newTheme(t: Translate): CustomThemeDefinition {
  return {
    version: 1,
    id: `custom-${Date.now().toString(36)}`,
    name: t('themeStudio.defaultName'),
    baseTheme: 'dark',
    mode: 'dark',
    colors: { surface0: '#0a0a0f', surface1: '#12121a', textPrimary: '#f0f0f5', textSecondary: '#a0a0b0', accent: '#14b8a6', danger: '#ef4444' },
    effects: { surfaceOpacity: 0.9, glassBlur: 12, shadowIntensity: 1, radiusScale: 1 },
    background: { kind: 'gradient', value: 'linear-gradient(145deg, #0a0a0f, #172554)', fit: 'cover', position: 'center', opacity: 1, dim: 0.15, blur: 0 },
  };
}

const COLOR_SLOTS: Array<[keyof CustomThemeDefinition['colors'], string]> = [
  ['surface0', 'canvas'], ['surface1', 'panel'], ['surface2', 'raisedPanel'], ['surface3', 'hoverSurface'], ['surface4', 'strongSurface'],
  ['textPrimary', 'primaryText'], ['textSecondary', 'secondaryText'], ['textTertiary', 'tertiaryText'], ['textInverse', 'inverseText'],
  ['accent', 'accent'], ['accentHover', 'accentHover'], ['accentSubtle', 'accentSubtle'],
  ['success', 'success'], ['warning', 'warning'], ['danger', 'danger'], ['info', 'info'],
  ['border', 'border'], ['borderHover', 'borderHover'], ['borderActive', 'borderActive'],
  ['contextPrompts', 'hudPrompts'], ['contextConversation', 'hudConversation'], ['contextToolResults', 'hudToolResults'],
  ['contextTools', 'hudTools'], ['contextMcp', 'hudMcp'], ['contextOverhead', 'hudOverhead'],
];

export function ThemeStudio() {
  const { t } = useTranslation();
  const { customThemes, activeThemeId, setTheme, saveCustomTheme, deleteCustomTheme } = useTheme();
  const [draft, setDraft] = useState<CustomThemeDefinition>(() => newTheme(t));
  const [importValue, setImportValue] = useState('');
  const [backgroundPreviewUrl, setBackgroundPreviewUrl] = useState<string | null>(null);
  const variables = useMemo(
    () => customThemeToCssVariables(draft, backgroundPreviewUrl ?? undefined),
    [backgroundPreviewUrl, draft],
  );
  const contrast = contrastRatio(draft.colors.textPrimary ?? '#ffffff', draft.colors.surface0 ?? '#000000');

  const updateColor = (key: keyof CustomThemeDefinition['colors'], value: string) => {
    setDraft((current) => ({ ...current, colors: { ...current.colors, [key]: value } }));
  };
  const importBackground = async () => {
    const selected = await open({ multiple: false, filters: [{ name: t('themeStudio.images'), extensions: ['png', 'jpg', 'jpeg', 'webp', 'gif'] }] });
    if (!selected) return;
    try {
      const asset = await api.importThemeBackground(selected);
      setBackgroundPreviewUrl(convertFileSrc(asset.path));
      setDraft((current) => ({
        ...current,
        background: { ...current.background, kind: 'image', assetId: asset.assetId, value: undefined, fit: 'cover' },
      }));
    } catch (error) {
      console.error('[theme-studio] background import failed', error);
      toast.error(t('themeStudio.backgroundImportFailed'));
    }
  };
  const updateBackgroundKind = (kind: CustomThemeDefinition['background']['kind']) => {
    if (kind === 'image') {
      void importBackground();
      return;
    }
    setBackgroundPreviewUrl(null);
    setDraft((current) => ({
      ...current,
      background: {
        ...current.background,
        kind,
        value: kind === 'gradient'
          ? 'linear-gradient(145deg, #0a0a0f, #172554)'
          : kind === 'color' ? '#0a0a0f' : undefined,
        assetId: undefined,
      },
    }));
  };
  const selectProfile = async (profile: CustomThemeDefinition) => {
    setTheme(profile.id);
    setDraft(profile);
    setBackgroundPreviewUrl(null);
    if (profile.background.kind === 'image' && profile.background.assetId) {
      try {
        const asset = await api.resolveThemeBackground(profile.background.assetId);
        setBackgroundPreviewUrl(convertFileSrc(asset.path));
      } catch (error) {
        console.error('[theme-studio] managed background unavailable', error);
        toast.error(t('themeStudio.backgroundUnavailable'));
      }
    }
  };
  const save = () => {
    try {
      saveCustomTheme(normalizeCustomTheme(draft));
      toast.success(t('themeStudio.saved'));
    } catch (error) {
      console.error('[theme-studio] save failed', error);
      toast.error(t('themeStudio.saveFailed'));
    }
  };
  const importJson = () => {
    try {
      const profile = parseCustomTheme(importValue);
      setDraft(profile);
      setBackgroundPreviewUrl(null);
      saveCustomTheme(profile);
      setImportValue('');
      toast.success(t('themeStudio.imported'));
    } catch (error) {
      console.error('[theme-studio] invalid theme', error);
      toast.error(t('themeStudio.invalidTheme'));
    }
  };

  return (
    <div className="mt-5 space-y-4 rounded-xl border border-border bg-surface-1 p-4" data-testid="theme-studio">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <div className="flex items-center gap-2 text-sm font-semibold text-text-primary"><WandSparkles size={16} /> {t('themeStudio.title')}</div>
          <p className="mt-1 text-xs text-text-tertiary">{t('themeStudio.description')}</p>
        </div>
        <button type="button" onClick={() => { setDraft(newTheme(t)); setBackgroundPreviewUrl(null); }} className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary hover:bg-surface-2"><Plus size={13} /> {t('themeStudio.new')}</button>
      </div>

      {customThemes.length > 0 && (
        <div className="flex flex-wrap gap-2">
          {customThemes.map((profile) => (
            <div key={profile.id} className={`flex items-center gap-1 rounded-md border px-2 py-1 ${activeThemeId === profile.id ? 'border-accent bg-accent/10' : 'border-border'}`}>
              <button type="button" onClick={() => void selectProfile(profile)} className="text-xs text-text-primary">{profile.name}</button>
              <button type="button" onClick={() => deleteCustomTheme(profile.id, draft.background.assetId ? [draft.background.assetId] : [])} aria-label={t('themeStudio.deleteTheme', { name: profile.name })} className="text-text-tertiary hover:text-danger"><Trash2 size={12} /></button>
            </div>
          ))}
        </div>
      )}

      <div className="grid gap-4 lg:grid-cols-[minmax(0,1.1fr)_minmax(260px,0.9fr)]">
        <div className="space-y-4">
          <div className="grid gap-3 sm:grid-cols-3">
            <label className="text-xs text-text-secondary">{t('themeStudio.name')}<input value={draft.name} onChange={(event) => setDraft({ ...draft, name: event.target.value })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary" /></label>
            <label className="text-xs text-text-secondary">{t('themeStudio.baseTheme')}<select value={draft.baseTheme} onChange={(event) => setDraft({ ...draft, baseTheme: event.target.value as CustomThemeDefinition['baseTheme'] })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary">{['dark', 'light', 'midnight', 'aurora', 'bloom', 'dream'].map((id) => <option key={id} value={id}>{t((`themeStudio.theme.${id}`) as Parameters<Translate>[0])}</option>)}</select></label>
            <label className="text-xs text-text-secondary">{t('themeStudio.mode')}<select value={draft.mode} onChange={(event) => setDraft({ ...draft, mode: event.target.value as 'dark' | 'light' })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary"><option value="dark">{t('themeStudio.dark')}</option><option value="light">{t('themeStudio.light')}</option></select></label>
          </div>
          <CollapsiblePanel title={t('themeStudio.advancedColors')}>
            <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
              {COLOR_SLOTS.map(([key, label]) => (
                <label key={key} className="flex items-center justify-between gap-2 rounded-md border border-border bg-surface-0 px-2 py-1.5 text-xs text-text-secondary">
                  {t((`themeStudio.color.${label}`) as Parameters<Translate>[0])}<input type="color" value={draft.colors[key] ?? '#64748b'} onChange={(event) => updateColor(key, event.target.value)} className="h-7 w-10 cursor-pointer border-0 bg-transparent" />
                </label>
              ))}
            </div>
          </CollapsiblePanel>
          <CollapsiblePanel title={t('themeStudio.effectsBackground')}>
            <div className="space-y-4">
              <div className="grid gap-3 sm:grid-cols-2">
                <Range label={t('themeStudio.surfaceOpacity')} value={draft.effects.surfaceOpacity ?? 1} min={0.35} max={1} step={0.05} onChange={(value) => setDraft({ ...draft, effects: { ...draft.effects, surfaceOpacity: value } })} />
                <Range label={t('themeStudio.glassBlur')} value={draft.effects.glassBlur ?? 0} min={0} max={48} onChange={(value) => setDraft({ ...draft, effects: { ...draft.effects, glassBlur: value } })} />
                <Range label={t('themeStudio.shadowIntensity')} value={draft.effects.shadowIntensity ?? 1} min={0} max={2} step={0.1} onChange={(value) => setDraft({ ...draft, effects: { ...draft.effects, shadowIntensity: value } })} />
                <Range label={t('themeStudio.radiusScale')} value={draft.effects.radiusScale ?? 1} min={0.5} max={2} step={0.1} onChange={(value) => setDraft({ ...draft, effects: { ...draft.effects, radiusScale: value } })} />
                <Range label={t('themeStudio.backgroundOpacity')} value={draft.background.opacity ?? 1} min={0} max={1} step={0.05} onChange={(value) => setDraft({ ...draft, background: { ...draft.background, opacity: value } })} />
                <Range label={t('themeStudio.backgroundDim')} value={draft.background.dim ?? 0} min={0} max={1} step={0.05} onChange={(value) => setDraft({ ...draft, background: { ...draft.background, dim: value } })} />
                <Range label={t('themeStudio.backgroundBlur')} value={draft.background.blur ?? 0} min={0} max={32} onChange={(value) => setDraft({ ...draft, background: { ...draft.background, blur: value } })} />
              </div>
              <div className="grid gap-3 sm:grid-cols-2">
                <label className="text-xs text-text-secondary">{t('themeStudio.backgroundKind')}<select value={draft.background.kind} onChange={(event) => updateBackgroundKind(event.target.value as CustomThemeDefinition['background']['kind'])} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary"><option value="none">{t('themeStudio.none')}</option><option value="color">{t('themeStudio.colorKind')}</option><option value="gradient">{t('themeStudio.gradient')}</option><option value="image">{t('themeStudio.imageChoose')}</option></select></label>
                <label className="text-xs text-text-secondary">{t('themeStudio.fit')}<select value={draft.background.fit ?? 'cover'} onChange={(event) => setDraft({ ...draft, background: { ...draft.background, fit: event.target.value as 'cover' | 'contain' | 'tile' } })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary"><option value="cover">{t('themeStudio.cover')}</option><option value="contain">{t('themeStudio.contain')}</option><option value="tile">{t('themeStudio.tile')}</option></select></label>
                {(draft.background.kind === 'color' || draft.background.kind === 'gradient') && <label className="text-xs text-text-secondary sm:col-span-2">{t('themeStudio.backgroundValue')}<input value={draft.background.value ?? ''} onChange={(event) => setDraft({ ...draft, background: { ...draft.background, value: event.target.value } })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary" /></label>}
                <label className="text-xs text-text-secondary">{t('themeStudio.position')}<input value={draft.background.position ?? 'center'} onChange={(event) => setDraft({ ...draft, background: { ...draft.background, position: event.target.value } })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary" /></label>
                <label className="text-xs text-text-secondary">{t('themeStudio.overlayColor')}<input type="color" value={draft.background.overlayColor ?? '#000000'} onChange={(event) => setDraft({ ...draft, background: { ...draft.background, overlayColor: event.target.value } })} className="mt-1 block h-8 w-full cursor-pointer rounded-md border border-border bg-surface-0" /></label>
              </div>
              <button type="button" onClick={() => void importBackground()} className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary hover:bg-surface-2"><Image size={13} /> {t('themeStudio.importBackground')}</button>
            </div>
          </CollapsiblePanel>
          <div className="flex flex-wrap gap-2">
            <button type="button" onClick={() => { const replacement = newTheme(t); setDraft({ ...replacement, id: draft.id, name: draft.name, baseTheme: draft.baseTheme, mode: draft.mode }); setBackgroundPreviewUrl(null); }} className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary hover:bg-surface-2"><RotateCcw size={13} /> {t('themeStudio.resetDraft')}</button>
            <button type="button" onClick={save} className="inline-flex items-center gap-1.5 rounded-md bg-accent px-2.5 py-1.5 text-xs font-medium text-text-inverse"><Save size={13} /> {t('themeStudio.saveApply')}</button>
          </div>
        </div>

        <div className="space-y-3">
          <div className="relative min-h-64 overflow-hidden rounded-xl border border-border p-4" style={variables as CSSProperties}>
            <div className="absolute inset-0" style={{ backgroundColor: variables['--theme-background-color'], backgroundImage: variables['--theme-background-image'], backgroundPosition: variables['--theme-background-position'], backgroundSize: variables['--theme-background-fit'], backgroundRepeat: variables['--theme-background-repeat'], opacity: Number(variables['--theme-background-opacity'] ?? 1), filter: `blur(${variables['--theme-background-blur'] ?? '0px'})` }} />
            <div className="relative rounded-lg border p-4 shadow-lg" style={{ color: variables['--color-text-primary'], background: variables['--color-surface-0'] }}>
              <div className="text-sm font-semibold">{t('themeStudio.livePreview')}</div>
              <p className="mt-2 text-xs" style={{ color: variables['--color-text-secondary'] }}>{t('themeStudio.previewDescription')}</p>
              <button type="button" className="mt-4 rounded-md px-3 py-1.5 text-xs" style={{ background: variables['--color-accent'], color: variables['--color-text-inverse'] ?? '#fff' }}>{t('themeStudio.accentAction')}</button>
            </div>
          </div>
          {contrast !== null && contrast < 4.5 && <div className="rounded-md border border-warning/40 bg-warning/10 p-2 text-xs text-warning">{t('themeStudio.contrastWarning', { ratio: contrast.toFixed(2) })}</div>}
          <CollapsiblePanel title={t('themeStudio.importExport')}>
            <div className="space-y-3">
              <button type="button" onClick={() => void navigator.clipboard.writeText(serializeCustomTheme(draft))} className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary"><Download size={13} /> {t('themeStudio.copyJson')}</button>
              <textarea value={importValue} onChange={(event) => setImportValue(event.target.value)} placeholder={t('themeStudio.importPlaceholder')} className="min-h-20 w-full rounded-md border border-border bg-surface-0 p-2 text-xs text-text-primary" />
              <button type="button" disabled={!importValue.trim()} onClick={importJson} className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary disabled:opacity-50"><Upload size={13} /> {t('themeStudio.importJson')}</button>
            </div>
          </CollapsiblePanel>
        </div>
      </div>
    </div>
  );
}

function Range({ label, value, min, max, step = 1, onChange }: { label: string; value: number; min: number; max: number; step?: number; onChange(value: number): void }) {
  return <label className="text-xs text-text-secondary">{label}: <span className="tabular-nums text-text-primary">{value}</span><input type="range" value={value} min={min} max={max} step={step} onChange={(event) => onChange(Number(event.target.value))} className="mt-1 block w-full accent-accent" /></label>;
}
