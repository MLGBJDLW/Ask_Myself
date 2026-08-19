import assert from 'node:assert/strict';
import { registerHooks } from 'node:module';
import test from 'node:test';

registerHooks({
  resolve(specifier, context, nextResolve) {
    return nextResolve(specifier === './theme' ? './theme.ts' : specifier, context);
  },
});

const {
  customThemeToCssVariables,
  normalizeThemeResourcePlugin,
  parseCustomTheme,
  serializeThemeResourcePlugin,
  shouldApplyRegistryRevision,
  themeResourcePluginToCustomTheme,
  themeToResourcePlugin,
} = await import('../src/lib/themeProfile.ts');

const profile = {
  version: 2,
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
  effects: { surfaceOpacity: 0.9, glassBlur: 14, densityScale: 0.8 },
  typography: { fontFamily: 'Inter Variable, sans-serif', baseSize: 15, lineHeight: 1.6 },
  motion: { durationScale: 0.85, cursorStyle: 'fluid' },
  brand: { logoVariant: 'accent', logoForeground: '#bae6fd', logoOpacity: 0.88 },
  content: { tagline: 'Quiet focus', statusText: 'Ready to explore', quote: 'Clarity over noise.' },
  components: { rail: { background: 'rgba(8, 19, 31, 0.92)', borderColor: '#164e63' } },
  background: {
    kind: 'gradient',
    value: 'linear-gradient(145deg, #08131f, #164e63)',
  },
};

test('theme profiles round-trip through the declarative plugin envelope', () => {
  const plugin = themeToResourcePlugin(profile, 'calm ocean dashboard');
  const serialized = serializeThemeResourcePlugin(profile, 'calm ocean dashboard');

  assert.equal(plugin.kind, 'theme-resource');
  assert.equal(plugin.manifestVersion, 2);
  assert.equal(plugin.description, 'calm ocean dashboard');
  assert.deepEqual(parseCustomTheme(serialized), themeResourcePluginToCustomTheme(plugin));
});

test('appearance synchronization discards registry responses older than the applied revision', () => {
  assert.equal(shouldApplyRegistryRevision(7, 6), false);
  assert.equal(shouldApplyRegistryRevision(7, 7), true);
  assert.equal(shouldApplyRegistryRevision(7, 8), true);
});

test('v1 theme-resource plugins migrate into the v2 declarative contract', () => {
  const legacy = { ...themeToResourcePlugin(profile), manifestVersion: 1 };
  const migrated = normalizeThemeResourcePlugin(legacy);

  assert.equal(migrated.manifestVersion, 2);
  assert.equal(migrated.theme.content.statusText, 'Ready to explore');
});

test('theme resources expose safe type, motion, brand, copy, and component slots', () => {
  const plugin = themeToResourcePlugin(profile);
  assert.equal(plugin.theme.typography.fontFamily, 'Inter Variable, sans-serif');
  assert.equal(plugin.theme.motion.cursorStyle, 'fluid');
  assert.equal(plugin.theme.brand.logoVariant, 'accent');
  assert.equal(plugin.theme.content.tagline, 'Quiet focus');
  assert.equal(plugin.theme.components.rail?.borderColor, '#164e63');
  assert.equal(customThemeToCssVariables(themeResourcePluginToCustomTheme(plugin))['--theme-titlebar-height'], '1.8rem');
});

test('theme resources keep executable CSS and remote fonts outside the contract', () => {
  assert.throws(
    () => normalizeThemeResourcePlugin({
      ...themeToResourcePlugin(profile),
      theme: { ...themeToResourcePlugin(profile).theme, colors: { accent: '#12345' } },
    }),
    /Invalid color/,
  );
  assert.throws(
    () => normalizeThemeResourcePlugin({
      ...themeToResourcePlugin(profile),
      theme: { ...themeToResourcePlugin(profile).theme, typography: { fontFamily: 'url(https://example.com/font.woff2)' } },
    }),
    /Invalid font family/,
  );
  assert.throws(
    () => normalizeThemeResourcePlugin({
      ...themeToResourcePlugin(profile),
      theme: { ...themeToResourcePlugin(profile).theme, components: { rail: { background: '#000; display:none' } } },
    }),
    /Invalid rail background/,
  );
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
