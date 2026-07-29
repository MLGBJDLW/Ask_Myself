import { convertFileSrc } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { Download, Image, Plus, RotateCcw, Save, Trash2, Upload, WandSparkles } from 'lucide-react';
import { useMemo, useState, type CSSProperties } from 'react';
import { toast } from 'sonner';

import * as api from '../../lib/api';
import { useTheme } from '../../lib/ThemeProvider';
import {
  contrastRatio,
  customThemeToCssVariables,
  normalizeCustomTheme,
  parseCustomTheme,
  serializeCustomTheme,
  type CustomThemeDefinition,
} from '../../lib/themeProfile';

function newTheme(): CustomThemeDefinition {
  return {
    version: 1,
    id: `custom-${Date.now().toString(36)}`,
    name: 'My theme',
    baseTheme: 'dark',
    mode: 'dark',
    colors: { surface0: '#0a0a0f', surface1: '#12121a', textPrimary: '#f0f0f5', textSecondary: '#a0a0b0', accent: '#14b8a6', danger: '#ef4444' },
    effects: { surfaceOpacity: 0.9, glassBlur: 12, shadowIntensity: 1, radiusScale: 1 },
    background: { kind: 'gradient', value: 'linear-gradient(145deg, #0a0a0f, #172554)', fit: 'cover', position: 'center', opacity: 1, dim: 0.15, blur: 0 },
  };
}

const COLOR_SLOTS: Array<[keyof CustomThemeDefinition['colors'], string]> = [
  ['surface0', 'Canvas'], ['surface1', 'Panel'], ['surface2', 'Raised panel'], ['surface3', 'Hover surface'], ['surface4', 'Strong surface'],
  ['textPrimary', 'Primary text'], ['textSecondary', 'Secondary text'], ['textTertiary', 'Tertiary text'], ['textInverse', 'Inverse text'],
  ['accent', 'Accent'], ['accentHover', 'Accent hover'], ['accentSubtle', 'Accent subtle'],
  ['success', 'Success'], ['warning', 'Warning'], ['danger', 'Danger'], ['info', 'Info'],
  ['border', 'Border'], ['borderHover', 'Border hover'], ['borderActive', 'Border active'],
  ['contextPrompts', 'HUD prompts'], ['contextConversation', 'HUD conversation'], ['contextToolResults', 'HUD tool results'],
  ['contextTools', 'HUD tools'], ['contextMcp', 'HUD MCP'], ['contextOverhead', 'HUD overhead'],
];

export function ThemeStudio() {
  const { customThemes, activeThemeId, setTheme, saveCustomTheme, deleteCustomTheme } = useTheme();
  const [draft, setDraft] = useState<CustomThemeDefinition>(newTheme);
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
    const selected = await open({ multiple: false, filters: [{ name: 'Images', extensions: ['png', 'jpg', 'jpeg', 'webp', 'gif'] }] });
    if (!selected) return;
    try {
      const asset = await api.importThemeBackground(selected);
      setBackgroundPreviewUrl(convertFileSrc(asset.path));
      setDraft((current) => ({
        ...current,
        background: { ...current.background, kind: 'image', assetId: asset.assetId, value: undefined, fit: 'cover' },
      }));
    } catch (error) {
      toast.error(`Background import failed: ${String(error)}`);
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
        toast.error(`Managed background is unavailable: ${String(error)}`);
      }
    }
  };
  const save = () => {
    try {
      saveCustomTheme(normalizeCustomTheme(draft));
      toast.success('Custom theme saved');
    } catch (error) {
      toast.error(String(error));
    }
  };
  const importJson = () => {
    try {
      const profile = parseCustomTheme(importValue);
      setDraft(profile);
      setBackgroundPreviewUrl(null);
      saveCustomTheme(profile);
      setImportValue('');
      toast.success('Theme imported');
    } catch (error) {
      toast.error(`Invalid theme: ${String(error)}`);
    }
  };

  return (
    <div className="mt-5 space-y-4 rounded-xl border border-border bg-surface-1 p-4" data-testid="theme-studio">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <div className="flex items-center gap-2 text-sm font-semibold text-text-primary"><WandSparkles size={16} /> Theme Studio</div>
          <p className="mt-1 text-xs text-text-tertiary">Clone semantic color slots, effects, and a managed local background. Arbitrary CSS and remote URLs are blocked.</p>
        </div>
        <button type="button" onClick={() => { setDraft(newTheme()); setBackgroundPreviewUrl(null); }} className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary hover:bg-surface-2"><Plus size={13} /> New</button>
      </div>

      {customThemes.length > 0 && (
        <div className="flex flex-wrap gap-2">
          {customThemes.map((profile) => (
            <div key={profile.id} className={`flex items-center gap-1 rounded-md border px-2 py-1 ${activeThemeId === profile.id ? 'border-accent bg-accent/10' : 'border-border'}`}>
              <button type="button" onClick={() => void selectProfile(profile)} className="text-xs text-text-primary">{profile.name}</button>
              <button type="button" onClick={() => deleteCustomTheme(profile.id, draft.background.assetId ? [draft.background.assetId] : [])} aria-label={`Delete ${profile.name}`} className="text-text-tertiary hover:text-danger"><Trash2 size={12} /></button>
            </div>
          ))}
        </div>
      )}

      <div className="grid gap-4 lg:grid-cols-[minmax(0,1.1fr)_minmax(260px,0.9fr)]">
        <div className="space-y-4">
          <div className="grid gap-3 sm:grid-cols-3">
            <label className="text-xs text-text-secondary">Name<input value={draft.name} onChange={(event) => setDraft({ ...draft, name: event.target.value })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary" /></label>
            <label className="text-xs text-text-secondary">Base theme<select value={draft.baseTheme} onChange={(event) => setDraft({ ...draft, baseTheme: event.target.value as CustomThemeDefinition['baseTheme'] })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary">{['dark', 'light', 'midnight', 'aurora', 'bloom', 'dream'].map((id) => <option key={id}>{id}</option>)}</select></label>
            <label className="text-xs text-text-secondary">Mode<select value={draft.mode} onChange={(event) => setDraft({ ...draft, mode: event.target.value as 'dark' | 'light' })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary"><option value="dark">dark</option><option value="light">light</option></select></label>
          </div>
          <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
            {COLOR_SLOTS.map(([key, label]) => (
              <label key={key} className="flex items-center justify-between gap-2 rounded-md border border-border bg-surface-0 px-2 py-1.5 text-xs text-text-secondary">
                {label}<input type="color" value={draft.colors[key] ?? '#64748b'} onChange={(event) => updateColor(key, event.target.value)} className="h-7 w-10 cursor-pointer border-0 bg-transparent" />
              </label>
            ))}
          </div>
          <div className="grid gap-3 sm:grid-cols-2">
            <Range label="Surface opacity" value={draft.effects.surfaceOpacity ?? 1} min={0.35} max={1} step={0.05} onChange={(value) => setDraft({ ...draft, effects: { ...draft.effects, surfaceOpacity: value } })} />
            <Range label="Glass blur" value={draft.effects.glassBlur ?? 0} min={0} max={48} onChange={(value) => setDraft({ ...draft, effects: { ...draft.effects, glassBlur: value } })} />
            <Range label="Shadow intensity" value={draft.effects.shadowIntensity ?? 1} min={0} max={2} step={0.1} onChange={(value) => setDraft({ ...draft, effects: { ...draft.effects, shadowIntensity: value } })} />
            <Range label="Radius scale" value={draft.effects.radiusScale ?? 1} min={0.5} max={2} step={0.1} onChange={(value) => setDraft({ ...draft, effects: { ...draft.effects, radiusScale: value } })} />
            <Range label="Background opacity" value={draft.background.opacity ?? 1} min={0} max={1} step={0.05} onChange={(value) => setDraft({ ...draft, background: { ...draft.background, opacity: value } })} />
            <Range label="Background dim" value={draft.background.dim ?? 0} min={0} max={1} step={0.05} onChange={(value) => setDraft({ ...draft, background: { ...draft.background, dim: value } })} />
            <Range label="Background blur" value={draft.background.blur ?? 0} min={0} max={32} onChange={(value) => setDraft({ ...draft, background: { ...draft.background, blur: value } })} />
          </div>
          <div className="grid gap-3 sm:grid-cols-2">
            <label className="text-xs text-text-secondary">Background kind<select value={draft.background.kind} onChange={(event) => updateBackgroundKind(event.target.value as CustomThemeDefinition['background']['kind'])} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary"><option value="none">none</option><option value="color">color</option><option value="gradient">gradient</option><option value="image">image (choose file)</option></select></label>
            <label className="text-xs text-text-secondary">Fit<select value={draft.background.fit ?? 'cover'} onChange={(event) => setDraft({ ...draft, background: { ...draft.background, fit: event.target.value as 'cover' | 'contain' | 'tile' } })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary"><option value="cover">cover</option><option value="contain">contain</option><option value="tile">tile</option></select></label>
            {(draft.background.kind === 'color' || draft.background.kind === 'gradient') && <label className="text-xs text-text-secondary sm:col-span-2">Background value<input value={draft.background.value ?? ''} onChange={(event) => setDraft({ ...draft, background: { ...draft.background, value: event.target.value } })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary" /></label>}
            <label className="text-xs text-text-secondary">Position<input value={draft.background.position ?? 'center'} onChange={(event) => setDraft({ ...draft, background: { ...draft.background, position: event.target.value } })} className="mt-1 w-full rounded-md border border-border bg-surface-0 px-2 py-1.5 text-text-primary" /></label>
            <label className="text-xs text-text-secondary">Overlay color<input type="color" value={draft.background.overlayColor ?? '#000000'} onChange={(event) => setDraft({ ...draft, background: { ...draft.background, overlayColor: event.target.value } })} className="mt-1 block h-8 w-full cursor-pointer rounded-md border border-border bg-surface-0" /></label>
          </div>
          <div className="flex flex-wrap gap-2">
            <button type="button" onClick={() => void importBackground()} className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary hover:bg-surface-2"><Image size={13} /> Import background</button>
            <button type="button" onClick={() => { const replacement = newTheme(); setDraft({ ...replacement, id: draft.id, name: draft.name, baseTheme: draft.baseTheme, mode: draft.mode }); setBackgroundPreviewUrl(null); }} className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary hover:bg-surface-2"><RotateCcw size={13} /> Reset draft</button>
            <button type="button" onClick={save} className="inline-flex items-center gap-1.5 rounded-md bg-accent px-2.5 py-1.5 text-xs font-medium text-text-inverse"><Save size={13} /> Save and apply</button>
            <button type="button" onClick={() => void navigator.clipboard.writeText(serializeCustomTheme(draft))} className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary"><Download size={13} /> Copy JSON</button>
          </div>
        </div>

        <div className="space-y-3">
          <div className="relative min-h-64 overflow-hidden rounded-xl border border-border p-4" style={variables as CSSProperties}>
            <div className="absolute inset-0" style={{ backgroundColor: variables['--theme-background-color'], backgroundImage: variables['--theme-background-image'], backgroundPosition: variables['--theme-background-position'], backgroundSize: variables['--theme-background-fit'], backgroundRepeat: variables['--theme-background-repeat'], opacity: Number(variables['--theme-background-opacity'] ?? 1), filter: `blur(${variables['--theme-background-blur'] ?? '0px'})` }} />
            <div className="relative rounded-lg border p-4 shadow-lg" style={{ color: variables['--color-text-primary'], background: variables['--color-surface-0'] }}>
              <div className="text-sm font-semibold">Live preview</div>
              <p className="mt-2 text-xs" style={{ color: variables['--color-text-secondary'] }}>Semantic tokens keep messages, status colors, and context usage consistent.</p>
              <button type="button" className="mt-4 rounded-md px-3 py-1.5 text-xs" style={{ background: variables['--color-accent'], color: variables['--color-text-inverse'] ?? '#fff' }}>Accent action</button>
            </div>
          </div>
          {contrast !== null && contrast < 4.5 && <div className="rounded-md border border-warning/40 bg-warning/10 p-2 text-xs text-warning">Primary text contrast is {contrast.toFixed(2)}:1; 4.5:1 is recommended.</div>}
          <textarea value={importValue} onChange={(event) => setImportValue(event.target.value)} placeholder="Paste Theme JSON to import" className="min-h-20 w-full rounded-md border border-border bg-surface-0 p-2 text-xs text-text-primary" />
          <button type="button" disabled={!importValue.trim()} onClick={importJson} className="inline-flex items-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-xs text-text-secondary disabled:opacity-50"><Upload size={13} /> Import JSON</button>
        </div>
      </div>
    </div>
  );
}

function Range({ label, value, min, max, step = 1, onChange }: { label: string; value: number; min: number; max: number; step?: number; onChange(value: number): void }) {
  return <label className="text-xs text-text-secondary">{label}: <span className="tabular-nums text-text-primary">{value}</span><input type="range" value={value} min={min} max={max} step={step} onChange={(event) => onChange(Number(event.target.value))} className="mt-1 block w-full accent-accent" /></label>;
}
