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
  version: 1,
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
  effects: { glassBlur: 18, surfaceOpacity: 0.82, radiusScale: 1.1 },
  background: { kind: 'gradient', value: 'linear-gradient(145deg, #07111f, #172554)', opacity: 0.9 },
};

equal(JSON.stringify(parseCustomTheme(serializeCustomTheme(example))), JSON.stringify(normalizeCustomTheme(example)));

const vars = customThemeToCssVariables(example);
equal(vars['--color-accent'], '#38bdf8');
equal(vars['--context-prompts'], '#22d3ee');
equal(vars['--theme-glass-blur'], '18px');
equal(vars['--theme-background-image'], 'linear-gradient(145deg, #07111f, #172554)');
equal(Object.keys(vars).some((key) => key.includes('display')), false);

throws(() => normalizeCustomTheme({ ...example, colors: { accent: 'url(https://invalid)' } }));
throws(() => normalizeCustomTheme({ ...example, background: { kind: 'gradient', value: 'url(https://invalid)' } }));
throws(() => normalizeCustomTheme({ ...example, background: { kind: 'image', value: 'https://asset.localhost/file.png' } }));
throws(() => parseCustomTheme('{"version":2}'));

const imageTheme = normalizeCustomTheme({
  ...example,
  background: { kind: 'image', assetId: 'a'.repeat(64) },
});
equal(serializeCustomTheme(imageTheme).includes('asset.localhost'), false);
equal(customThemeToCssVariables(imageTheme, 'http://asset.localhost/theme.png')['--theme-background-image'], 'url("http://asset.localhost/theme.png")');
