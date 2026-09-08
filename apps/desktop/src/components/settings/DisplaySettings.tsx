import { useState } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { Trash2, Upload } from 'lucide-react';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import { FONT_PRESETS } from '../../lib/fontCatalog';
import { useFonts } from '../../lib/FontProvider';
import { updateDisplayPreferences, useDisplayPreferences, type StreamingMode } from '../../lib/displayPreferences';
import { Button } from '../ui/Button';

export function DisplaySettings() {
  const { t } = useTranslation();
  const preferences = useDisplayPreferences();
  const { assets, reload, error: fontError } = useFonts();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [imported, setImported] = useState<number | null>(null);
  async function importFonts() {
    setBusy(true);
    setError(null);
    setImported(null);
    try {
      const selected = await open({ multiple: true, filters: [{ name: t('settings.displayFonts'), extensions: ['ttf', 'otf', 'woff', 'woff2', 'zip'] }] });
      let count = 0;
      for (const path of (Array.isArray(selected) ? selected : selected ? [selected] : [])) {
        count += (await api.importFontAssets(path)).length;
      }
      await reload();
      if (count) setImported(count);
    } catch (cause) { setError(String(cause)); await reload(); }
    finally { setBusy(false); }
  }
  async function removeFont(id: string) {
    setBusy(true);
    setError(null);
    try {
      await api.removeFontAsset(id);
      updateDisplayPreferences({
        ...(preferences.uiFontId === id ? { uiFontId: 'theme' } : {}),
        ...(preferences.codeFontId === id ? { codeFontId: 'theme' } : {}),
      });
      await reload();
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  }
  return <div data-testid="display-settings" className="space-y-6 border-t border-border pt-5">
    <div className="space-y-3">
      <p className="text-sm font-medium text-text-primary">{t('settings.displayFonts')}</p>
      <p className="text-xs leading-relaxed text-text-tertiary">{t('settings.displayFontsDesc')}</p>
      <div className="grid gap-3 sm:grid-cols-2">
        {(['uiFontId', 'codeFontId'] as const).map(slot => <label key={slot} className="space-y-2 text-xs text-text-secondary">
          <span className="block">{t(slot === 'uiFontId' ? 'settings.displayUiFont' : 'settings.displayCodeFont')}</span>
          <select data-testid={slot === 'uiFontId' ? 'ui-font-select' : 'code-font-select'} value={preferences[slot]} onChange={event => updateDisplayPreferences({ [slot]: event.target.value })} className="w-full rounded-lg border border-border bg-surface-2 px-3 py-2.5 text-sm text-text-primary">
            <option value="theme">{t('settings.displayThemeFont')}</option>
            {(['cjk', 'text', 'mono'] as const).map(kind => <optgroup key={kind} label={t(`settings.displayFontGroup.${kind}`)}>
              {FONT_PRESETS.filter(font => font.kind === kind).map(font => <option key={font.id} value={font.id}>{font.name}</option>)}
            </optgroup>)}
            {assets.length > 0 && <optgroup label={t('settings.displayImportedFonts')}>{assets.map(font => <option key={font.id} value={font.id}>{font.name}</option>)}</optgroup>}
            {preferences[slot] !== 'theme' && !FONT_PRESETS.some(font => font.id === preferences[slot]) && !assets.some(font => font.id === preferences[slot]) && <option value={preferences[slot]}>{t('settings.displayFontUnavailable')}</option>}
          </select>
        </label>)}
      </div>
      <div className="space-y-2 rounded-xl border border-border bg-surface-1 px-4 py-3" data-testid="font-preview">
        <p className="text-base">{t('settings.displayFontSample')}</p>
        <p className="text-sm text-text-secondary">The quick brown fox · 0123456789</p>
        <code className="block text-xs text-text-tertiary">const answer = {'{'} text: "你好, Nexa", value: 42 {'}'};</code>
      </div>
      <Button size="sm" variant="secondary" icon={<Upload size={14} />} disabled={busy} onClick={() => void importFonts()}>{t('settings.displayImportFonts')}</Button>
      <p className="text-xs leading-relaxed text-text-tertiary">{t('settings.displayImportFontsDesc')}</p>
      {imported !== null && <p role="status" className="text-xs text-text-secondary">{t('settings.displayFontsImported', { count: String(imported) })}</p>}
      {assets.length > 0 && <ul className="max-h-48 divide-y divide-border overflow-y-auto rounded-lg border border-border px-3">{assets.map(font => <li key={font.id} className="flex items-center gap-2 py-2">
        <span className="min-w-0 flex-1 truncate text-xs text-text-secondary">{font.name} <span className="text-text-tertiary">{font.format.toUpperCase()}</span></span>
        <button type="button" disabled={busy} aria-label={t('settings.displayRemoveFont', { name: font.name })} className="rounded p-1.5 text-text-tertiary hover:bg-surface-3 hover:text-error" onClick={() => void removeFont(font.id)}><Trash2 size={13} /></button>
      </li>)}</ul>}
      {(error || fontError) && <p role="alert" className="break-words text-xs text-error">{t('settings.displayFontError')} {error || fontError}</p>}
    </div>
    <div className="space-y-3 border-t border-border pt-5">
      <p className="text-sm font-medium text-text-primary">{t('settings.displayStreaming')}</p>
      <div className="grid gap-2 sm:grid-cols-3" role="group" aria-label={t('settings.displayStreaming')}>
        {(['chunked', 'balanced', 'smooth'] as StreamingMode[]).map(mode => <button type="button" key={mode} data-testid={`streaming-mode-${mode}`} aria-pressed={preferences.streamingMode === mode} onClick={() => updateDisplayPreferences({ streamingMode: mode })} className={`rounded-xl border p-3 text-left ${preferences.streamingMode === mode ? 'border-accent bg-accent-subtle' : 'border-border bg-surface-1 hover:bg-surface-2'}`}>
          <span className="block text-sm font-medium text-text-primary">{t(`settings.displayStreaming.${mode}`)}</span>
          <span className="mt-1 block text-xs leading-relaxed text-text-tertiary">{t(`settings.displayStreaming.${mode}Desc`)}</span>
        </button>)}
      </div>
      <p className="text-xs leading-relaxed text-text-tertiary">{t('settings.displayStreamingCost')}</p>
    </div>
  </div>;
}
