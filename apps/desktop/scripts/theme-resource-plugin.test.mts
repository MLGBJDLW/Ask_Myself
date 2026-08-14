import assert from 'node:assert/strict';
import { registerHooks } from 'node:module';
import test from 'node:test';

registerHooks({
  resolve(specifier, context, nextResolve) {
    return nextResolve(specifier === './theme' ? './theme.ts' : specifier, context);
  },
});

const {
  normalizeThemeResourcePlugin,
  parseCustomTheme,
  serializeThemeResourcePlugin,
  themeResourcePluginToCustomTheme,
  themeToResourcePlugin,
} = await import('../src/lib/themeProfile.ts');

const profile = {
  version: 1,
  id: 'quiet-ocean',
  name: 'Quiet Ocean',
  baseTheme: 'dark',
  mode: 'dark',
  colors: {
    surface0: '#08131f',
    surface1: '#102235',
    textPrimary: '#f2f8ff',
    textSecondary: '#a8bed1',
    accent: '#38bdf8',
  },
  effects: { surfaceOpacity: 0.9, glassBlur: 14 },
  background: {
    kind: 'gradient',
    value: 'linear-gradient(145deg, #08131f, #164e63)',
  },
};

test('theme profiles round-trip through the declarative plugin envelope', () => {
  const plugin = themeToResourcePlugin(profile, 'calm ocean dashboard');
  const serialized = serializeThemeResourcePlugin(profile, 'calm ocean dashboard');

  assert.equal(plugin.kind, 'theme-resource');
  assert.equal(plugin.manifestVersion, 1);
  assert.equal(plugin.description, 'calm ocean dashboard');
  assert.deepEqual(parseCustomTheme(serialized), themeResourcePluginToCustomTheme(plugin));
});

test('theme-resource description limits count Unicode code points like the Rust backend', () => {
  assert.equal(themeToResourcePlugin(profile, '🌊'.repeat(251)).description, '🌊'.repeat(251));
  assert.throws(
    () => themeToResourcePlugin(profile, '🌊'.repeat(501)),
    /description is too long/,
  );
});

test('theme-resource plugins reject executable or remote background content', () => {
  const plugin = themeToResourcePlugin(profile);
  const unsafe = {
    ...plugin,
    theme: {
      ...plugin.theme,
      background: {
        kind: 'gradient',
        value: 'linear-gradient(#000, #111); background: url(https://example.com/x.png)',
      },
    },
  };

  assert.throws(() => normalizeThemeResourcePlugin(unsafe), /Invalid background gradient|not allowed/);
});

test('image theme resources only accept managed sha256 asset ids', () => {
  const plugin = themeToResourcePlugin(profile);
  const unmanaged = {
    ...plugin,
    theme: {
      ...plugin.theme,
      background: { kind: 'image', assetId: 'C:/wallpaper.png' },
    },
  };

  assert.throws(() => normalizeThemeResourcePlugin(unmanaged), /managed local asset id/);
});
