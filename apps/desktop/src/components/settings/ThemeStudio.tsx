import { convertFileSrc } from '@tauri-apps/api/core';
import { NexaSelect } from '../ui/overlay';
import { open } from '@tauri-apps/plugin-dialog';
import { Download, Image, LoaderCircle, Plus, RotateCcw, Save, Trash2, Upload, WandSparkles } from 'lucide-react';
import { useMemo, useState, type CSSProperties } from 'react';
import { toast } from 'sonner';

import * as api from '../../lib/api';
import { useTranslation } from '../../i18n';
import { useTheme } from '../../lib/ThemeProvider';
import {
  contrastRatio,
  customThemeToCssVariables,
  normalizeThemeResourcePlugin,
  normalizeCustomTheme,
  parseCustomTheme,
  serializeThemeResourcePlugin,
  themeResourcePluginToCustomTheme,
  themeToResourcePlugin,
  type CustomThemeDefinition,
} from '../../lib/themeProfile';
import { CollapsiblePanel } from './SettingsSection';

type Translate = ReturnType<typeof useTranslation>['t'];

function newTheme(t: Translate): CustomThemeDefinition {
  return {
    version: 2,
    id: `custom-${Date.now().toString(36)}`,
    name: t('themeStudio.defaultName'),
    baseTheme: 'dark',
    mode: 'dark',
    colors: { surface0: '#0a0a0f', surface1: '#12121a', textPrimary: '#f0f0f5', textSecondary: '#a0a0b0', accent: '#14b8a6', danger: '#ef4444' },
    effects: { surfaceOpacity: 0.9, glassBlur: 12, shadowIntensity: 1, radiusScale: 1, densityScale: 1 },
    typography: { baseSize: 16, lineHeight: 1.5, letterSpacing: 0 },
    motion: { durationScale: 1, cursorStyle: 'fluid' },
    brand: { logoVariant: 'auto', logoOpacity: 1 },
    content: {},
    components: {},
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

const COMPONENT_STYLE_SLOTS = ['rail', 'header', 'card', 'browser'] as const;

export function ThemeStudio() {
  const { t } = useTranslation();
  const {
    customThemes,
    themePlugins,
    activeThemeId,
    setTheme,
    installThemePlugin,
    uninstallThemePlugin,
    rollbackTheme,
  } = useTheme();
  const [draft, setDraft] = useState<CustomThemeDefinition>(() => newTheme(t));
  const [themeDescription, setThemeDescription] = useState('');
  const [importValue, setImportValue] = useState('');
  const [backgroundPreviewUrl, setBackgroundPreviewUrl] = useState<string | null>(null);
  const [isGeneratingTheme, setIsGeneratingTheme] = useState(false);
  const [isGeneratingBackground, setIsGeneratingBackground] = useState(false);
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
    setThemeDescription(themePlugins.find((plugin) => plugin.id === profile.id)?.description ?? '');
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
      installThemePlugin(themeToResourcePlugin(normalizeCustomTheme(draft), themeDescription));
      toast.success(t('themeStudio.saved'));
    } catch (error) {
      console.error('[theme-studio] save failed', error);
      toast.error(t('themeStudio.saveFailed'));
    }
  };
  const importJson = () => {
    try {
      const value = JSON.parse(importValue) as unknown;
      const plugin = value && typeof value === 'object' && (value as { kind?: unknown }).kind === 'theme-resource'
        ? normalizeThemeResourcePlugin(value)
        : themeToResourcePlugin(parseCustomTheme(importValue));
      const profile = themeResourcePluginToCustomTheme(plugin);
      setDraft(profile);
      setThemeDescription(plugin.description ?? '');
      setBackgroundPreviewUrl(null);
      installThemePlugin(plugin);
      setImportValue('');
      toast.success(t('themeStudio.imported'));
    } catch (error) {
      console.error('[theme-studio] invalid theme', error);
      toast.error(t('themeStudio.invalidTheme'));
    }
  };
  const generateTheme = async () => {
    if (!themeDescription.trim()) return;
    setIsGeneratingTheme(true);
    try {
      const plugin = normalizeThemeResourcePlugin(await api.generateThemeResourcePlugin(themeDescription));
      setDraft(themeResourcePluginToCustomTheme(plugin));
      setThemeDescription(plugin.description ?? '');
      setBackgroundPreviewUrl(null);
      toast.success(t('themeStudio.draftGenerated'));
    } catch (error) {
      console.error('[theme-studio] theme generation failed', error);
      toast.error(t('themeStudio.generationFailed'));
    } finally {
      setIsGeneratingTheme(false);
    }
  };
  const generateBackground = async () => {
    const direction = themeDescription.trim() || draft.name;
    setIsGeneratingBackground(true);
    try {
      const palette = [draft.colors.surface0, draft.colors.surface1, draft.colors.accent]
        .filter(Boolean)
        .join(', ');
      const asset = await api.generateThemeBackground(`${direction}. Palette: ${palette}.`);
      setBackgroundPreviewUrl(convertFileSrc(asset.path));
      setDraft((current) => ({
        ...current,
        background: {
          ...current.background,
          kind: 'image',
          value: undefined,
          assetId: asset.assetId,
          fit: 'cover',
        },
      }));
      const retainedAssetIds = [
        ...themePlugins.flatMap((plugin) => plugin.theme.background.assetId ? [plugin.theme.background.assetId] : []),
        asset.assetId,
      ];
      try {
        await api.garbageCollectThemeAssets(retainedAssetIds);
      } catch (error) {
        console.warn('[theme-studio] generated background cleanup deferred', error);
      }
      toast.success(t('themeStudio.backgroundGenerated'));
    } catch (error) {
      console.error('[theme-studio] background generation failed', error);
      toast.error(t('themeStudio.backgroundGenerationFailed'));
    } finally {
      setIsGeneratingBackground(false);
    }
  };

  return (
    <div className="mt-5 space-y-4 rounded-xl border border-border bg-surface-1 p-4" data-testid="theme-studio" data-theme-density-surface="studio">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <div className="flex items-center gap-2 text-sm font-semibold text-text-primary"><WandSparkles size={16} /> {t('themeStudio.title')}</div>
          <p className="mt-1 text-xs text-text-tertiary">{t('themeStudio.description')}</p>
        </div>
        <div className="flex flex-wrap gap-2">
          <button type="button" onClick={rollbackTheme} className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary hover:bg-surface-2"><RotateCcw size={13} /> {t('themeStudio.rollback')}</button>
          <button type="button" onClick={() => { setDraft(newTheme(t)); setThemeDescription(''); setBackgroundPreviewUrl(null); }} className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary hover:bg-surface-2"><Plus size={13} /> {t('themeStudio.new')}</button>
        </div>
      </div>

      <div className="rounded-lg border border-accent/30 bg-accent/5 p-3" data-testid="theme-plugin-generator">
        <div className="text-xs font-medium text-text-primary">{t('themeStudio.describeTitle')}</div>
        <p className="mt-1 text-xs text-text-tertiary">{t('themeStudio.describeHelp')}</p>
        <textarea value={themeDescription} onChange={(event) => setThemeDescription(event.target.value)} maxLength={4000} placeholder={t('themeStudio.describePlaceholder')} className="mt-2 min-h-20 w-full rounded-md border border-border bg-surface-0 p-2 text-sm text-text-primary" />
        <div className="mt-2 flex flex-wrap gap-2">
          <button type="button" disabled={!themeDescription.trim() || isGeneratingTheme} onClick={() => void generateTheme()} className="inline-flex items-center gap-1.5 rounded-md bg-accent px-2.5 py-1.5 text-xs font-medium text-text-inverse disabled:opacity-50">
            {isGeneratingTheme ? <LoaderCircle size={13} className="animate-spin" /> : <WandSparkles size={13} />} {t('themeStudio.generateDraft')}
          </button>
          <button type="button" disabled={isGeneratingBackground} onClick={() => void generateBackground()} className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary disabled:opacity-50">
            {isGeneratingBackground ? <LoaderCircle size={13} className="animate-spin" /> : <Image size={13} />} {t('themeStudio.generateBackground')}
          </button>
        </div>
        <p className="mt-2 text-[11px] text-text-tertiary">{t('themeStudio.previewBeforeInstall')}</p>
      </div>

      {customThemes.length > 0 && (
        <div className="flex flex-wrap gap-2">
          {customThemes.map((profile) => (
            <div key={profile.id} className={`flex items-center gap-1 rounded-md border px-2 py-1 ${activeThemeId === profile.id ? 'border-accent bg-accent/10' : 'border-border'}`}>
              <button type="button" onClick={() => void selectProfile(profile)} className="text-xs text-text-primary">{profile.name}</button>
              <button type="button" onClick={() => uninstallThemePlugin(profile.id, draft.background.assetId ? [draft.background.assetId] : [])} aria-label={t('themeStudio.deleteTheme', { name: profile.name })} className="text-text-tertiary hover:text-danger"><Trash2 size={12} /></button>
            </div>
          ))}
        </div>
      )}

      <div className="grid gap-4 lg:grid-cols-[minmax(0,1.1fr)_minmax(260px,0.9fr)]">
        <div className="space-y-4">
          <div className="grid gap-3 sm:grid-cols-3">
            <label className="text-xs text-text-secondary">{t('themeStudio.name')}<input value={draft.name} onChange={(event) => setDraft({ ...draft, name: event.target.value })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary" /></label>
            <label className="text-xs text-text-secondary">{t('themeStudio.baseTheme')}<NexaSelect value={draft.baseTheme} onChange={(event) => setDraft({ ...draft, baseTheme: event.target.value as CustomThemeDefinition['baseTheme'] })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary">{['dark', 'light', 'midnight', 'aurora', 'bloom', 'dream'].map((id) => <option key={id} value={id}>{t((`themeStudio.theme.${id}`) as Parameters<Translate>[0])}</option>)}</NexaSelect></label>
            <label className="text-xs text-text-secondary">{t('themeStudio.mode')}<NexaSelect value={draft.mode} onChange={(event) => setDraft({ ...draft, mode: event.target.value as 'dark' | 'light' })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary"><option value="dark">{t('themeStudio.dark')}</option><option value="light">{t('themeStudio.light')}</option></NexaSelect></label>
          </div>
          <CollapsiblePanel title={t('themeStudio.identityCopy')}>
            <div className="grid gap-3 sm:grid-cols-2">
              <label className="text-xs text-text-secondary sm:col-span-2">{t('themeStudio.tagline')}<input value={draft.content.tagline ?? ''} maxLength={160} onChange={(event) => setDraft({ ...draft, content: { ...draft.content, tagline: event.target.value } })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary" /></label>
              <label className="text-xs text-text-secondary">{t('themeStudio.statusText')}<input value={draft.content.statusText ?? ''} maxLength={80} onChange={(event) => setDraft({ ...draft, content: { ...draft.content, statusText: event.target.value } })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary" /></label>
              <label className="text-xs text-text-secondary">{t('themeStudio.quote')}<input value={draft.content.quote ?? ''} maxLength={240} onChange={(event) => setDraft({ ...draft, content: { ...draft.content, quote: event.target.value } })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary" /></label>
              <label className="text-xs text-text-secondary">{t('themeStudio.logoVariant')}<NexaSelect value={draft.brand.logoVariant ?? 'auto'} onChange={(event) => setDraft({ ...draft, brand: { ...draft.brand, logoVariant: event.target.value as 'auto' | 'monochrome' | 'accent' } })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary"><option value="auto">{t('themeStudio.logoAuto')}</option><option value="monochrome">{t('themeStudio.logoMonochrome')}</option><option value="accent">{t('themeStudio.logoAccent')}</option></NexaSelect></label>
              <label className="flex items-center justify-between gap-2 rounded-md border border-border bg-surface-0 px-2 py-1.5 text-xs text-text-secondary">{t('themeStudio.logoForeground')}<input type="color" value={draft.brand.logoForeground ?? draft.colors.textPrimary ?? '#f0f0f5'} onChange={(event) => setDraft({ ...draft, brand: { ...draft.brand, logoForeground: event.target.value } })} className="h-7 w-10 cursor-pointer border-0 bg-transparent" /></label>
            </div>
          </CollapsiblePanel>
          <CollapsiblePanel title={t('themeStudio.typographyMotion')}>
            <div className="space-y-4">
              <div className="grid gap-3 sm:grid-cols-2">
                <label className="text-xs text-text-secondary">{t('themeStudio.fontFamily')}<input value={draft.typography.fontFamily ?? ''} maxLength={160} onChange={(event) => setDraft({ ...draft, typography: { ...draft.typography, fontFamily: event.target.value } })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary" /></label>
                <label className="text-xs text-text-secondary">{t('themeStudio.monoFontFamily')}<input value={draft.typography.monoFontFamily ?? ''} maxLength={160} onChange={(event) => setDraft({ ...draft, typography: { ...draft.typography, monoFontFamily: event.target.value } })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary" /></label>
                <Range label={t('themeStudio.baseSize')} value={draft.typography.baseSize ?? 16} min={12} max={20} step={0.5} onChange={(value) => setDraft({ ...draft, typography: { ...draft.typography, baseSize: value } })} />
                <Range label={t('themeStudio.lineHeight')} value={draft.typography.lineHeight ?? 1.5} min={1.2} max={2} step={0.05} onChange={(value) => setDraft({ ...draft, typography: { ...draft.typography, lineHeight: value } })} />
                <Range label={t('themeStudio.motionScale')} value={draft.motion.durationScale ?? 1} min={0} max={2} step={0.1} onChange={(value) => setDraft({ ...draft, motion: { ...draft.motion, durationScale: value } })} />
                <Range label={t('themeStudio.densityScale')} value={draft.effects.densityScale ?? 1} min={0.8} max={1.25} step={0.05} onChange={(value) => setDraft({ ...draft, effects: { ...draft.effects, densityScale: value } })} />
                <label className="text-xs text-text-secondary">{t('themeStudio.cursorStyle')}<NexaSelect value={draft.motion.cursorStyle ?? 'fluid'} onChange={(event) => setDraft({ ...draft, motion: { ...draft.motion, cursorStyle: event.target.value as 'fluid' | 'precise' | 'minimal' } })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary"><option value="fluid">{t('themeStudio.cursorFluid')}</option><option value="precise">{t('themeStudio.cursorPrecise')}</option><option value="minimal">{t('themeStudio.cursorMinimal')}</option></NexaSelect></label>
              </div>
            </div>
          </CollapsiblePanel>
          <CollapsiblePanel title={t('themeStudio.componentRecipes')}>
            <p className="mb-3 text-xs leading-relaxed text-text-tertiary">{t('themeStudio.componentRecipeHelp')}</p>
            <div className="space-y-3">
              {COMPONENT_STYLE_SLOTS.map((slot) => {
                const style = draft.components[slot] ?? {};
                const update = (key: 'background' | 'borderColor' | 'boxShadow', value: string) => setDraft({
                  ...draft,
                  components: {
                    ...draft.components,
                    [slot]: { ...style, [key]: value || undefined },
                  },
                });
                return (
                  <fieldset key={slot} className="rounded-lg border border-border bg-surface-0/55 p-3">
                    <legend className="px-1 text-xs font-medium text-text-primary">{t((`themeStudio.component.${slot}`) as Parameters<Translate>[0])}</legend>
                    <div className="grid gap-2 sm:grid-cols-3">
                      <label className="text-[11px] text-text-secondary">{t('themeStudio.componentBackground')}<input value={style.background ?? ''} onChange={(event) => update('background', event.target.value)} placeholder="#12121a or gradient" className="mt-1 w-full rounded-md border border-border bg-surface-1 px-2 py-1.5 text-xs text-text-primary" /></label>
                      <label className="text-[11px] text-text-secondary">{t('themeStudio.componentBorder')}<input value={style.borderColor ?? ''} onChange={(event) => update('borderColor', event.target.value)} placeholder="#ffffff20" className="mt-1 w-full rounded-md border border-border bg-surface-1 px-2 py-1.5 text-xs text-text-primary" /></label>
                      <label className="text-[11px] text-text-secondary">{t('themeStudio.componentShadow')}<input value={style.boxShadow ?? ''} onChange={(event) => update('boxShadow', event.target.value)} placeholder="0 8px 24px #00000066" className="mt-1 w-full rounded-md border border-border bg-surface-1 px-2 py-1.5 text-xs text-text-primary" /></label>
                    </div>
                  </fieldset>
                );
              })}
            </div>
          </CollapsiblePanel>
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
                <label className="text-xs text-text-secondary">{t('themeStudio.backgroundKind')}<NexaSelect value={draft.background.kind} onChange={(event) => updateBackgroundKind(event.target.value as CustomThemeDefinition['background']['kind'])} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary"><option value="none">{t('themeStudio.none')}</option><option value="color">{t('themeStudio.colorKind')}</option><option value="gradient">{t('themeStudio.gradient')}</option><option value="image">{t('themeStudio.imageChoose')}</option></NexaSelect></label>
                <label className="text-xs text-text-secondary">{t('themeStudio.fit')}<NexaSelect value={draft.background.fit ?? 'cover'} onChange={(event) => setDraft({ ...draft, background: { ...draft.background, fit: event.target.value as 'cover' | 'contain' | 'tile' } })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary"><option value="cover">{t('themeStudio.cover')}</option><option value="contain">{t('themeStudio.contain')}</option><option value="tile">{t('themeStudio.tile')}</option></NexaSelect></label>
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
          <div
            className={`theme-${draft.baseTheme} relative min-h-64 overflow-hidden rounded-xl border border-border p-4`}
            data-testid="theme-live-preview"
            style={variables as CSSProperties}
          >
            <div
              className="absolute inset-0 scale-[1.03]"
              style={{
                backgroundColor: 'var(--theme-background-color, var(--color-surface-0))',
                backgroundImage: 'linear-gradient(color-mix(in srgb, var(--theme-background-overlay, #000) calc(var(--theme-background-dim, 0) * 100%), transparent), color-mix(in srgb, var(--theme-background-overlay, #000) calc(var(--theme-background-dim, 0) * 100%), transparent)), var(--theme-background-image, none)',
                backgroundPosition: 'var(--theme-background-position, center)',
                backgroundSize: 'var(--theme-background-fit, cover)',
                backgroundRepeat: 'var(--theme-background-repeat, no-repeat)',
                opacity: Number(variables['--theme-background-opacity'] ?? 1),
                filter: `blur(${variables['--theme-background-blur'] ?? '0px'})`,
              }}
            />
            <div
              className="relative rounded-lg border p-4 shadow-lg"
              style={{
                color: 'var(--color-text-primary)',
                background: variables['--theme-component-card-background'] ?? 'var(--theme-content-surface-0)',
                borderColor: variables['--theme-component-card-border'] ?? 'var(--color-border)',
                boxShadow: variables['--theme-component-card-shadow'],
                WebkitBackdropFilter: `blur(${variables['--theme-glass-blur'] ?? '0px'})`,
                backdropFilter: `blur(${variables['--theme-glass-blur'] ?? '0px'})`,
              }}
            >
              <div className="text-sm font-semibold">{draft.content.statusText || t('themeStudio.livePreview')}</div>
              <p className="mt-2 text-xs" style={{ color: 'var(--color-text-secondary)' }}>{draft.content.tagline || t('themeStudio.previewDescription')}</p>
              {draft.content.quote && <blockquote className="mt-3 border-l-2 pl-2 text-xs italic" style={{ color: 'var(--color-text-secondary)', borderColor: 'var(--color-accent)' }}>{draft.content.quote}</blockquote>}
              <button type="button" className="mt-4 rounded-md px-3 py-1.5 text-xs" style={{ background: 'var(--color-accent)', color: 'var(--color-text-inverse, #fff)' }}>{t('themeStudio.accentAction')}</button>
            </div>
          </div>
          {contrast !== null && contrast < 4.5 && <div className="rounded-md border border-warning/40 bg-warning/10 p-2 text-xs text-warning">{t('themeStudio.contrastWarning', { ratio: contrast.toFixed(2) })}</div>}
          <CollapsiblePanel title={t('themeStudio.importExport')}>
            <div className="space-y-3">
              <button type="button" onClick={() => void navigator.clipboard.writeText(serializeThemeResourcePlugin(draft, themeDescription))} className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary"><Download size={13} /> {t('themeStudio.copyJson')}</button>
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
