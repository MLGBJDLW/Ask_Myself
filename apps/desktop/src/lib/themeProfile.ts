import { type ThemeId, isThemeId } from './theme';

export type ThemeMode = 'dark' | 'light';
export type ThemeBackgroundKind = 'none' | 'color' | 'gradient' | 'image';

export interface CustomThemeDefinition {
  version: 1;
  id: string;
  name: string;
  baseTheme: ThemeId;
  mode: ThemeMode;
  colors: {
    surface0?: string; surface1?: string; surface2?: string; surface3?: string; surface4?: string;
    textPrimary?: string; textSecondary?: string; textTertiary?: string; textInverse?: string;
    accent?: string; accentHover?: string; accentSubtle?: string;
    success?: string; warning?: string; danger?: string; info?: string;
    border?: string; borderHover?: string; borderActive?: string;
    contextPrompts?: string; contextConversation?: string; contextToolResults?: string;
    contextTools?: string; contextMcp?: string; contextOverhead?: string;
  };
  effects: {
    surfaceOpacity?: number;
    glassBlur?: number;
    shadowIntensity?: number;
    radiusScale?: number;
  };
  background: {
    kind: ThemeBackgroundKind;
    value?: string;
    assetId?: string;
    fit?: 'cover' | 'contain' | 'tile';
    position?: string;
    opacity?: number;
    dim?: number;
    blur?: number;
    overlayColor?: string;
  };
}

export interface ThemeResourcePlugin {
  manifestVersion: 1;
  kind: 'theme-resource';
  id: string;
  name: string;
  description?: string;
  theme: Omit<CustomThemeDefinition, 'version' | 'id' | 'name'>;
}

export type ThemeCssVariables = Record<`--${string}`, string>;

const COLOR_VARIABLES: Record<keyof CustomThemeDefinition['colors'], `--${string}`> = {
  surface0: '--color-surface-0', surface1: '--color-surface-1', surface2: '--color-surface-2',
  surface3: '--color-surface-3', surface4: '--color-surface-4',
  textPrimary: '--color-text-primary', textSecondary: '--color-text-secondary',
  textTertiary: '--color-text-tertiary', textInverse: '--color-text-inverse',
  accent: '--color-accent', accentHover: '--color-accent-hover', accentSubtle: '--color-accent-subtle',
  success: '--color-success', warning: '--color-warning', danger: '--color-danger', info: '--color-info',
  border: '--color-border', borderHover: '--color-border-hover', borderActive: '--color-border-active',
  contextPrompts: '--context-prompts', contextConversation: '--context-conversation',
  contextToolResults: '--context-tool-results', contextTools: '--context-tools',
  contextMcp: '--context-mcp', contextOverhead: '--context-overhead',
};

const SAFE_COLOR = /^(?:#[0-9a-f]{3,8}|(?:rgb|rgba|hsl|hsla|oklch|oklab)\([0-9a-z.% ,/+\-]+\)|transparent)$/i;
const SAFE_GRADIENT = /^(?:linear|radial|conic)-gradient\([#0-9a-z.%(), /+\-]+\)$/i;

export function normalizeCustomTheme(value: unknown): CustomThemeDefinition {
  if (!value || typeof value !== 'object') throw new Error('Theme profile must be an object');
  const input = value as Partial<CustomThemeDefinition>;
  if (input.version !== 1) throw new Error('Unsupported theme profile version');
  if (!input.id || !/^[a-z0-9][a-z0-9_-]{0,63}$/i.test(input.id)) throw new Error('Invalid theme id');
  if (!input.name?.trim() || input.name.trim().length > 80) throw new Error('Invalid theme name');
  if (!input.baseTheme || !isThemeId(input.baseTheme)) throw new Error('Invalid base theme');
  if (input.mode !== 'dark' && input.mode !== 'light') throw new Error('Invalid theme mode');

  const colors: CustomThemeDefinition['colors'] = {};
  for (const key of Object.keys(COLOR_VARIABLES) as Array<keyof CustomThemeDefinition['colors']>) {
    const color = input.colors?.[key];
    if (color === undefined || color === '') continue;
    if (typeof color !== 'string' || !SAFE_COLOR.test(color.trim())) throw new Error(`Invalid color: ${key}`);
    colors[key] = color.trim();
  }
  const effects = {
    surfaceOpacity: clampOptional(input.effects?.surfaceOpacity, 0.35, 1),
    glassBlur: clampOptional(input.effects?.glassBlur, 0, 48),
    shadowIntensity: clampOptional(input.effects?.shadowIntensity, 0, 2),
    radiusScale: clampOptional(input.effects?.radiusScale, 0.5, 2),
  };
  const background = input.background ?? { kind: 'none' as const };
  if (!['none', 'color', 'gradient', 'image'].includes(background.kind)) throw new Error('Invalid background kind');
  const backgroundValue = background.value?.trim();
  if (background.kind === 'color' && backgroundValue && !SAFE_COLOR.test(backgroundValue)) throw new Error('Invalid background color');
  if (background.kind === 'gradient' && backgroundValue && !SAFE_GRADIENT.test(backgroundValue)) throw new Error('Invalid background gradient');
  const assetId = background.assetId?.trim();
  if (background.kind === 'image' && (!assetId || !/^[0-9a-f]{64}$/i.test(assetId))) throw new Error('Theme images must reference a managed local asset id');
  if (backgroundValue && /url\s*\(|@import|[;{}]/i.test(backgroundValue)) throw new Error('External URLs and CSS rules are not allowed');

  return {
    version: 1,
    id: input.id,
    name: input.name.trim(),
    baseTheme: input.baseTheme,
    mode: input.mode,
    colors,
    effects,
    background: {
      kind: background.kind,
      ...(background.kind !== 'image' && backgroundValue ? { value: backgroundValue } : {}),
      ...(background.kind === 'image' && assetId ? { assetId } : {}),
      fit: background.fit ?? 'cover',
      position: safePosition(background.position),
      opacity: clampOptional(background.opacity, 0, 1),
      dim: clampOptional(background.dim, 0, 1),
      blur: clampOptional(background.blur, 0, 32),
      ...(background.overlayColor && SAFE_COLOR.test(background.overlayColor) ? { overlayColor: background.overlayColor } : {}),
    },
  };
}

export function customThemeToCssVariables(
  theme: CustomThemeDefinition,
  resolvedBackgroundUrl?: string,
): ThemeCssVariables {
  const normalized = normalizeCustomTheme(theme);
  const variables: ThemeCssVariables = {};
  for (const [key, variable] of Object.entries(COLOR_VARIABLES) as Array<[keyof typeof COLOR_VARIABLES, `--${string}`]>) {
    const value = normalized.colors[key];
    if (value) variables[variable] = value;
  }
  if (normalized.effects.surfaceOpacity !== undefined) variables['--theme-surface-opacity'] = String(normalized.effects.surfaceOpacity);
  if (normalized.effects.glassBlur !== undefined) variables['--theme-glass-blur'] = `${normalized.effects.glassBlur}px`;
  if (normalized.effects.shadowIntensity !== undefined) variables['--theme-shadow-intensity'] = String(normalized.effects.shadowIntensity);
  if (normalized.effects.radiusScale !== undefined) {
    variables['--radius-sm'] = `${6 * normalized.effects.radiusScale}px`;
    variables['--radius-md'] = `${10 * normalized.effects.radiusScale}px`;
    variables['--radius-lg'] = `${16 * normalized.effects.radiusScale}px`;
  }
  const background = normalized.background;
  if (background.kind === 'gradient' && background.value) variables['--theme-background-image'] = background.value;
  if (background.kind === 'color' && background.value) variables['--theme-background-color'] = background.value;
  if (background.kind === 'image' && resolvedBackgroundUrl) variables['--theme-background-image'] = `url("${escapeCssUrl(resolvedBackgroundUrl)}")`;
  variables['--theme-background-fit'] = background.fit === 'tile' ? 'auto' : background.fit ?? 'cover';
  variables['--theme-background-repeat'] = background.fit === 'tile' ? 'repeat' : 'no-repeat';
  variables['--theme-background-position'] = background.position ?? 'center';
  variables['--theme-background-opacity'] = String(background.opacity ?? 1);
  variables['--theme-background-dim'] = String(background.dim ?? 0);
  variables['--theme-background-blur'] = `${background.blur ?? 0}px`;
  variables['--theme-background-overlay'] = background.overlayColor ?? '#000000';
  return variables;
}

export function serializeCustomTheme(theme: CustomThemeDefinition): string {
  return JSON.stringify(normalizeCustomTheme(theme), null, 2);
}

export function parseCustomTheme(json: string): CustomThemeDefinition {
  const value = JSON.parse(json) as unknown;
  if (isThemeResourcePlugin(value)) return themeResourcePluginToCustomTheme(value);
  return normalizeCustomTheme(value);
}

export function themeToResourcePlugin(
  theme: CustomThemeDefinition,
  description?: string,
): ThemeResourcePlugin {
  const normalized = normalizeCustomTheme(theme);
  return normalizeThemeResourcePlugin({
    manifestVersion: 1,
    kind: 'theme-resource',
    id: normalized.id,
    name: normalized.name,
    ...(description?.trim() ? { description: description.trim() } : {}),
    theme: resourceThemeFromProfile(normalized),
  });
}

export function normalizeThemeResourcePlugin(value: unknown): ThemeResourcePlugin {
  if (!value || typeof value !== 'object') throw new Error('Theme resource plugin must be an object');
  const input = value as Partial<ThemeResourcePlugin>;
  if (input.manifestVersion !== 1 || input.kind !== 'theme-resource') throw new Error('Unsupported theme resource plugin');
  if (!input.theme || typeof input.theme !== 'object') throw new Error('Theme resource plugin is missing its theme');
  const profile = normalizeCustomTheme({
    ...input.theme,
    version: 1,
    id: input.id,
    name: input.name,
  });
  const description = input.description?.trim();
  if (description && Array.from(description).length > 500) throw new Error('Theme resource description is too long');
  return {
    manifestVersion: 1,
    kind: 'theme-resource',
    id: profile.id,
    name: profile.name,
    ...(description ? { description } : {}),
    theme: resourceThemeFromProfile(profile),
  };
}

export function themeResourcePluginToCustomTheme(value: unknown): CustomThemeDefinition {
  const plugin = normalizeThemeResourcePlugin(value);
  return normalizeCustomTheme({
    ...plugin.theme,
    version: 1,
    id: plugin.id,
    name: plugin.name,
  });
}

export function serializeThemeResourcePlugin(
  theme: CustomThemeDefinition,
  description?: string,
): string {
  return JSON.stringify(themeToResourcePlugin(theme, description), null, 2);
}

function isThemeResourcePlugin(value: unknown): value is ThemeResourcePlugin {
  return Boolean(
    value
    && typeof value === 'object'
    && (value as Partial<ThemeResourcePlugin>).kind === 'theme-resource',
  );
}

function resourceThemeFromProfile(
  profile: CustomThemeDefinition,
): ThemeResourcePlugin['theme'] {
  return {
    baseTheme: profile.baseTheme,
    mode: profile.mode,
    colors: profile.colors,
    effects: profile.effects,
    background: profile.background,
  };
}

export function applyCustomTheme(theme: CustomThemeDefinition, resolvedBackgroundUrl?: string): void {
  const root = document.documentElement;
  clearCustomThemeVariables();
  for (const [key, value] of Object.entries(customThemeToCssVariables(theme, resolvedBackgroundUrl))) root.style.setProperty(key, value);
  root.dataset.customTheme = 'true';
  root.dataset.themeMode = theme.mode;
}

export function clearCustomThemeVariables(): void {
  const root = document.documentElement;
  for (const variable of Object.values(COLOR_VARIABLES)) root.style.removeProperty(variable);
  for (const variable of ['--theme-surface-opacity', '--theme-glass-blur', '--theme-shadow-intensity', '--theme-background-image', '--theme-background-color', '--theme-background-fit', '--theme-background-repeat', '--theme-background-position', '--theme-background-opacity', '--theme-background-dim', '--theme-background-blur', '--theme-background-overlay', '--radius-sm', '--radius-md', '--radius-lg']) root.style.removeProperty(variable);
  delete root.dataset.customTheme;
  delete root.dataset.themeMode;
}

export function contrastRatio(foreground: string, background: string): number | null {
  const fg = hexRgb(foreground);
  const bg = hexRgb(background);
  if (!fg || !bg) return null;
  const luminance = ([r, g, b]: [number, number, number]) => {
    const values = [r, g, b].map((channel) => {
      const value = channel / 255;
      return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
    });
    return values[0] * 0.2126 + values[1] * 0.7152 + values[2] * 0.0722;
  };
  const [light, dark] = [luminance(fg), luminance(bg)].sort((a, b) => b - a);
  return (light + 0.05) / (dark + 0.05);
}

function clampOptional(value: unknown, min: number, max: number): number | undefined {
  if (value === undefined) return undefined;
  if (typeof value !== 'number' || !Number.isFinite(value)) throw new Error('Invalid numeric theme value');
  return Math.min(max, Math.max(min, value));
}
function safePosition(value: string | undefined): string {
  if (!value || !/^[a-z0-9.% +\-]+$/i.test(value)) return 'center';
  return value;
}
function escapeCssUrl(value: string): string { return value.replace(/["\\\n\r]/g, (char) => `\\${char}`); }
function hexRgb(value: string): [number, number, number] | null {
  const match = /^#([0-9a-f]{6})$/i.exec(value.trim());
  if (!match) return null;
  return [0, 2, 4].map((offset) => Number.parseInt(match[1].slice(offset, offset + 2), 16)) as [number, number, number];
}
