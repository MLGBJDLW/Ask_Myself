import {
  customThemeToCssVariables,
  normalizeCustomTheme,
  parseCustomTheme,
  serializeCustomTheme,
  type CustomThemeDefinition,
} from '../src/lib/themeProfile';

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}
function equal(actual: unknown, expected: unknown): void {
  assert(actual === expected, `Expected ${String(expected)}, received ${String(actual)}`);
}
function throws(fn: () => unknown): void {
  let threw = false;
  try { fn(); } catch { threw = true; }
  assert(threw, 'Expected function to throw');
}

const example: CustomThemeDefinition = {
  version: 2,
  id: 'ocean',
  name: 'Ocean',
  baseTheme: 'midnight',
  mode: 'dark',
  colors: {
    surface0: '#07111f',
    textPrimary: '#f7fbff',
    accent: '#38bdf8',
    contextPrompts: '#22d3ee',
  },
  effects: { glassBlur: 18, surfaceOpacity: 0.82, radiusScale: 1.1, densityScale: 0.8 },
  typography: { fontFamily: 'Inter Variable, sans-serif', monoFontFamily: 'Cascadia Mono, monospace', baseSize: 15 },
  motion: { durationScale: 0.75, cursorStyle: 'fluid' },
  brand: { logoVariant: 'accent', logoForeground: '#bae6fd', logoOpacity: 0.9 },
  content: { tagline: 'Quiet focus', statusText: 'Ready', quote: 'Clarity over noise.' },
  components: { rail: { background: 'rgba(7, 17, 31, 0.9)', borderColor: '#164e63' } },
  background: { kind: 'gradient', value: 'linear-gradient(145deg, #07111f, #172554)', opacity: 0.9 },
};

equal(JSON.stringify(parseCustomTheme(serializeCustomTheme(example))), JSON.stringify(normalizeCustomTheme(example)));

const vars = customThemeToCssVariables(example);
equal(vars['--color-accent'], '#38bdf8');
equal(vars['--context-prompts'], '#22d3ee');
equal(vars['--theme-glass-blur'], '18px');
equal(vars['--theme-background-image'], 'linear-gradient(145deg, #07111f, #172554)');
equal(vars['--theme-font-sans'], 'Inter Variable, sans-serif');
equal(vars['--theme-duration-scale'], '0.75');
equal(vars['--theme-titlebar-height'], '1.8rem');
equal(vars['--theme-logo-foreground'], '#bae6fd');
equal(vars['--theme-component-rail-border'], '#164e63');
equal(Object.keys(vars).some((key) => key.includes('display')), false);

throws(() => normalizeCustomTheme({ ...example, colors: { accent: 'url(https://invalid)' } }));
throws(() => normalizeCustomTheme({ ...example, background: { kind: 'gradient', value: 'url(https://invalid)' } }));
throws(() => normalizeCustomTheme({ ...example, background: { kind: 'image', value: 'https://asset.localhost/file.png' } }));
throws(() => parseCustomTheme('{"version":3}'));
throws(() => normalizeCustomTheme({ ...example, typography: { fontFamily: 'url(https://invalid)' } }));
throws(() => normalizeCustomTheme({ ...example, components: { rail: { background: '#000; display:none' } } }));

const migrated = normalizeCustomTheme({ ...example, version: 1 });
equal(migrated.version, 2);

const imageTheme = normalizeCustomTheme({
  ...example,
  background: { kind: 'image', assetId: 'a'.repeat(64) },
});
equal(serializeCustomTheme(imageTheme).includes('asset.localhost'), false);
equal(customThemeToCssVariables(imageTheme, 'http://asset.localhost/theme.png')['--theme-background-image'], 'url("http://asset.localhost/theme.png")');
