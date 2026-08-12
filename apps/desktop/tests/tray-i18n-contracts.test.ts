// @ts-expect-error The contract runner intentionally omits Node ambient types.
import { existsSync, readFileSync } from 'node:fs';
// @ts-expect-error The contract runner intentionally omits Node ambient types.
import { join } from 'node:path';

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

const root = process.cwd();
const main = readFileSync(join(root, 'src-tauri/src/main.rs'), 'utf8');
const context = readFileSync(join(root, 'src/i18n/context.tsx'), 'utf8');
const appSettings = readFileSync(join(root, '../../crates/core/src/app_settings.rs'), 'utf8');
const settingsPage = readFileSync(join(root, 'src/features/settings/SettingsPage.tsx'), 'utf8');
const locales = ['en', 'zh-CN', 'zh-TW', 'ja', 'ko', 'fr', 'de', 'es', 'pt', 'ru'];
const keys = [
  'showNexa',
  'showCompanion',
  'hideCompanion',
  'lockCompanion',
  'unlockCompanion',
  'resetCompanion',
  'companionSettings',
  'quitNexa',
];

assert(main.includes('TrayMenuLabels'), 'native tray must use a complete localized label contract');
assert(main.includes('update_tray_menu_cmd'), 'native tray must support live locale refresh');
assert(main.includes('save_ui_locale(locale)'), 'tray locale must use independent persistence');
assert(context.includes('updateTrayMenu'), 'changing the UI locale must refresh the native tray');
assert(appSettings.includes('UI_LOCALE_KEY'), 'native locale must survive stale whole-config snapshots');
assert(settingsPage.includes('uiLocale: nextLocale'), 'the live Settings AppConfig snapshot must follow locale changes');
const english = JSON.parse(readFileSync(join(root, 'src/i18n/locales/en/tray.json'), 'utf8')) as Record<string, string>;

for (const locale of locales) {
  const path = join(root, 'src/i18n/locales', locale, 'tray.json');
  assert(existsSync(path), `${locale} must provide tray translations`);
  const values = JSON.parse(readFileSync(path, 'utf8')) as Record<string, unknown>;
  for (const key of keys) {
    assert(typeof values[key] === 'string' && values[key].trim().length > 0, `${locale} tray.${key} must be translated`);
    if (locale !== 'en') {
      assert(values[key] !== english[key], `${locale} tray.${key} must not silently fall back to English`);
    }
  }
}

console.log('ok - native tray labels follow the live UI locale');
