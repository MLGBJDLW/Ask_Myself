import { type ThemeId, isThemeId } from './theme';

export type ThemeMode = 'dark' | 'light';
export type ThemeBackgroundKind = 'none' | 'color' | 'gradient' | 'image';
export type ThemeCursorStyle = 'precise' | 'fluid' | 'minimal';
export type ThemeLogoVariant = 'auto' | 'monochrome' | 'accent';
export type ThemeComponentSlot = 'rail' | 'header' | 'card' | 'browser';

export interface ThemeTypography {
  fontFamily?: string;
  monoFontFamily?: string;
  baseSize?: number;
  lineHeight?: number;
  letterSpacing?: number;
}

export interface ThemeMotion {
  durationScale?: number;
  cursorStyle?: ThemeCursorStyle;
}

export interface ThemeBrand {
  logoVariant?: ThemeLogoVariant;
  logoForeground?: string;
  logoMuted?: string;
  logoOpacity?: number;
}

export interface ThemeContent {
  tagline?: string;
  statusText?: string;
  quote?: string;
}

export interface ThemeComponentStyle {
  background?: string;
  borderColor?: string;
  boxShadow?: string;
}

export interface CustomThemeDefinition {
  version: 2;
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
    densityScale?: number;
  };
  typography: ThemeTypography;
  motion: ThemeMotion;
  brand: ThemeBrand;
  content: ThemeContent;
  components: Partial<Record<ThemeComponentSlot, ThemeComponentStyle>>;
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
  manifestVersion: 2;
  kind: 'theme-resource';
  id: string;
  name: string;
  description?: string;
  theme: Omit<CustomThemeDefinition, 'version' | 'id' | 'name'>;
}

export type ThemeCssVariables = Record<`--${string}`, string>;

export function shouldApplyRegistryRevision(currentRevision: number, incomingRevision: number): boolean {
  return Number.isFinite(incomingRevision) && incomingRevision >= currentRevision;
}

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

const THEME_VARIABLES = [
  '--theme-surface-opacity', '--theme-glass-blur', '--theme-shadow-intensity', '--theme-density-scale',
  '--theme-titlebar-height', '--theme-rail-brand-height', '--theme-space-1', '--theme-space-2',
  '--theme-space-2-5', '--theme-space-4', '--theme-space-5', '--theme-space-14',
  '--theme-background-image', '--theme-background-color', '--theme-background-fit', '--theme-background-repeat',
  '--theme-background-position', '--theme-background-opacity', '--theme-background-dim', '--theme-background-blur',
  '--theme-background-overlay', '--theme-font-sans', '--theme-font-mono', '--theme-base-font-size',
  '--theme-line-height', '--theme-letter-spacing', '--theme-duration-scale', '--theme-logo-foreground',
  '--nexa-logo-muted', '--theme-logo-opacity', '--radius-sm', '--radius-md', '--radius-lg',
  '--theme-component-rail-background', '--theme-component-rail-border', '--theme-component-rail-shadow',
  '--theme-component-header-background', '--theme-component-header-border', '--theme-component-header-shadow',
  '--theme-component-card-background', '--theme-component-card-border', '--theme-component-card-shadow',
  '--theme-component-browser-background', '--theme-component-browser-border', '--theme-component-browser-shadow',
] as const;

const COMPONENT_SLOTS: ThemeComponentSlot[] = ['rail', 'header', 'card', 'browser'];
const SAFE_COLOR = /^(?:#(?:[0-9a-f]{3}|[0-9a-f]{4}|[0-9a-f]{6}|[0-9a-f]{8})|(?:rgb|rgba|hsl|hsla|oklch|oklab)\([0-9a-z.% ,/+\-]+\)|transparent)$/i;
const SAFE_GRADIENT = /^(?:linear|radial|conic)-gradient\([#0-9a-z.%(), /+\-]+\)$/i;
const SAFE_FONT = /^[a-z0-9 _,'".\-]+$/i;
const SAFE_SHADOW = /^[#0-9a-z.%(), /+\-]+$/i;

type ThemeInput = Partial<Omit<CustomThemeDefinition, 'version'>> & { version?: number };
type ThemePluginInput = Partial<Omit<ThemeResourcePlugin, 'manifestVersion'>> & { manifestVersion?: number };

export function normalizeCustomTheme(value: unknown): CustomThemeDefinition {
  if (!value || typeof value !== 'object') throw new Error('Theme profile must be an object');
  const input = value as ThemeInput;
  if (input.version !== 1 && input.version !== 2) throw new Error('Unsupported theme profile version');
  if (!input.id || !/^[a-z0-9][a-z0-9_-]{0,63}$/i.test(input.id)) throw new Error('Invalid theme id');
  if (!input.name?.trim() || Array.from(input.name.trim()).length > 80) throw new Error('Invalid theme name');
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
    densityScale: clampOptional(input.effects?.densityScale, 0.8, 1.25),
  };
  const typography: ThemeTypography = {
    fontFamily: safeFont(input.typography?.fontFamily),
    monoFontFamily: safeFont(input.typography?.monoFontFamily),
    baseSize: clampOptional(input.typography?.baseSize, 12, 20),
    lineHeight: clampOptional(input.typography?.lineHeight, 1.2, 2),
    letterSpacing: clampOptional(input.typography?.letterSpacing, -0.04, 0.12),
  };
  const cursorStyle = input.motion?.cursorStyle;
  if (cursorStyle && !['precise', 'fluid', 'minimal'].includes(cursorStyle)) throw new Error('Invalid cursor style');
  const motion: ThemeMotion = {
    durationScale: clampOptional(input.motion?.durationScale, 0, 2),
    ...(cursorStyle ? { cursorStyle } : {}),
  };
  const logoVariant = input.brand?.logoVariant;
  if (logoVariant && !['auto', 'monochrome', 'accent'].includes(logoVariant)) throw new Error('Invalid logo variant');
  const brand: ThemeBrand = {
    ...(logoVariant ? { logoVariant } : {}),
    logoForeground: safeOptionalColor(input.brand?.logoForeground, 'logo foreground'),
    logoMuted: safeOptionalColor(input.brand?.logoMuted, 'logo muted color'),
    logoOpacity: clampOptional(input.brand?.logoOpacity, 0.4, 1),
  };
  const content: ThemeContent = {
    tagline: normalizePlainText(input.content?.tagline, 160),
    statusText: normalizePlainText(input.content?.statusText, 80),
    quote: normalizePlainText(input.content?.quote, 240),
  };
  const components: CustomThemeDefinition['components'] = {};
  for (const slot of COMPONENT_SLOTS) {
    const style = input.components?.[slot];
    if (style) components[slot] = normalizeComponentStyle(style, slot);
  }

  const background = input.background ?? { kind: 'none' as const };
  if (!['none', 'color', 'gradient', 'image'].includes(background.kind)) throw new Error('Invalid background kind');
  const backgroundValue = background.value?.trim();
  if (background.kind === 'color' && (!backgroundValue || !SAFE_COLOR.test(backgroundValue))) throw new Error('Invalid background color');
  if (background.kind === 'gradient' && (!backgroundValue || !SAFE_GRADIENT.test(backgroundValue))) throw new Error('Invalid background gradient');
  const assetId = background.assetId?.trim();
  if (background.kind === 'image' && (!assetId || !/^[0-9a-f]{64}$/i.test(assetId))) throw new Error('Theme images must reference a managed local asset id');
  if (backgroundValue && /url\s*\(|@import|[;{}]/i.test(backgroundValue)) throw new Error('External URLs and CSS rules are not allowed');

  return {
    version: 2,
    id: input.id,
    name: input.name.trim(),
    baseTheme: input.baseTheme,
    mode: input.mode,
    colors,
    effects,
    typography,
    motion,
    brand,
    content,
    components,
    background: {
      kind: background.kind,
      ...(background.kind !== 'image' && backgroundValue ? { value: backgroundValue } : {}),
      ...(background.kind === 'image' && assetId ? { assetId } : {}),
      fit: background.fit ?? 'cover',
      position: safePosition(background.position),
      opacity: clampOptional(background.opacity, 0, 1),
      dim: clampOptional(background.dim, 0, 1),
      blur: clampOptional(background.blur, 0, 32),
      ...(background.overlayColor ? { overlayColor: safeOptionalColor(background.overlayColor, 'background overlay') } : {}),
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
  if (normalized.effects.densityScale !== undefined) {
    const density = normalized.effects.densityScale;
    variables['--theme-density-scale'] = String(density);
    variables['--theme-titlebar-height'] = `${2.25 * density}rem`;
    variables['--theme-rail-brand-height'] = `${3.5 * density}rem`;
    variables['--theme-space-1'] = `${0.25 * density}rem`;
    variables['--theme-space-2'] = `${0.5 * density}rem`;
    variables['--theme-space-2-5'] = `${0.625 * density}rem`;
    variables['--theme-space-4'] = `${density}rem`;
    variables['--theme-space-5'] = `${1.25 * density}rem`;
    variables['--theme-space-14'] = `${3.5 * density}rem`;
  }
  if (normalized.effects.radiusScale !== undefined) {
    variables['--radius-sm'] = `${6 * normalized.effects.radiusScale}px`;
    variables['--radius-md'] = `${10 * normalized.effects.radiusScale}px`;
    variables['--radius-lg'] = `${16 * normalized.effects.radiusScale}px`;
  }
  if (normalized.typography.fontFamily) variables['--theme-font-sans'] = normalized.typography.fontFamily;
  if (normalized.typography.monoFontFamily) variables['--theme-font-mono'] = normalized.typography.monoFontFamily;
  if (normalized.typography.baseSize !== undefined) variables['--theme-base-font-size'] = `${normalized.typography.baseSize}px`;
  if (normalized.typography.lineHeight !== undefined) variables['--theme-line-height'] = String(normalized.typography.lineHeight);
  if (normalized.typography.letterSpacing !== undefined) variables['--theme-letter-spacing'] = `${normalized.typography.letterSpacing}em`;
  if (normalized.motion.durationScale !== undefined) variables['--theme-duration-scale'] = String(normalized.motion.durationScale);
  if (normalized.brand.logoForeground) variables['--theme-logo-foreground'] = normalized.brand.logoForeground;
  if (normalized.brand.logoMuted) variables['--nexa-logo-muted'] = normalized.brand.logoMuted;
  if (normalized.brand.logoOpacity !== undefined) variables['--theme-logo-opacity'] = String(normalized.brand.logoOpacity);
  for (const slot of COMPONENT_SLOTS) {
    const style = normalized.components[slot];
    if (style?.background) variables[`--theme-component-${slot}-background`] = style.background;
    if (style?.borderColor) variables[`--theme-component-${slot}-border`] = style.borderColor;
    if (style?.boxShadow) variables[`--theme-component-${slot}-shadow`] = style.boxShadow;
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

export function themeToResourcePlugin(theme: CustomThemeDefinition, description?: string): ThemeResourcePlugin {
  const normalized = normalizeCustomTheme(theme);
  return normalizeThemeResourcePlugin({
    manifestVersion: 2,
    kind: 'theme-resource',
    id: normalized.id,
    name: normalized.name,
    ...(description?.trim() ? { description: description.trim() } : {}),
    theme: resourceThemeFromProfile(normalized),
  });
}

export function normalizeThemeResourcePlugin(value: unknown): ThemeResourcePlugin {
  if (!value || typeof value !== 'object') throw new Error('Theme resource plugin must be an object');
  const input = value as ThemePluginInput;
  if (![1, 2].includes(input.manifestVersion ?? 0) || input.kind !== 'theme-resource') throw new Error('Unsupported theme resource plugin');
  if (!input.theme || typeof input.theme !== 'object') throw new Error('Theme resource plugin is missing its theme');
  const profile = normalizeCustomTheme({ ...input.theme, version: input.manifestVersion, id: input.id, name: input.name });
  const description = input.description?.trim();
  if (description && Array.from(description).length > 500) throw new Error('Theme resource description is too long');
  return {
    manifestVersion: 2,
    kind: 'theme-resource',
    id: profile.id,
    name: profile.name,
    ...(description ? { description } : {}),
    theme: resourceThemeFromProfile(profile),
  };
}

export function themeResourcePluginToCustomTheme(value: unknown): CustomThemeDefinition {
  const plugin = normalizeThemeResourcePlugin(value);
  return normalizeCustomTheme({ ...plugin.theme, version: 2, id: plugin.id, name: plugin.name });
}

export function serializeThemeResourcePlugin(theme: CustomThemeDefinition, description?: string): string {
  return JSON.stringify(themeToResourcePlugin(theme, description), null, 2);
}

function isThemeResourcePlugin(value: unknown): value is ThemeResourcePlugin {
  return Boolean(value && typeof value === 'object' && (value as { kind?: unknown }).kind === 'theme-resource');
}

function resourceThemeFromProfile(profile: CustomThemeDefinition): ThemeResourcePlugin['theme'] {
  return {
    baseTheme: profile.baseTheme,
    mode: profile.mode,
    colors: profile.colors,
    effects: profile.effects,
    typography: profile.typography,
    motion: profile.motion,
    brand: profile.brand,
    content: profile.content,
    components: profile.components,
    background: profile.background,
  };
}

export function applyCustomTheme(theme: CustomThemeDefinition, resolvedBackgroundUrl?: string): void {
  const normalized = normalizeCustomTheme(theme);
  const root = document.documentElement;
  clearCustomThemeVariables();
  for (const [key, value] of Object.entries(customThemeToCssVariables(normalized, resolvedBackgroundUrl))) root.style.setProperty(key, value);
  root.dataset.customTheme = 'true';
  root.dataset.themeMode = normalized.mode;
  root.dataset.cursorStyle = normalized.motion.cursorStyle ?? 'fluid';
  root.dataset.logoVariant = normalized.brand.logoVariant ?? 'auto';
}

export function clearCustomThemeVariables(): void {
  const root = document.documentElement;
  for (const variable of Object.values(COLOR_VARIABLES)) root.style.removeProperty(variable);
  for (const variable of THEME_VARIABLES) root.style.removeProperty(variable);
  delete root.dataset.customTheme;
  delete root.dataset.themeMode;
  delete root.dataset.cursorStyle;
  delete root.dataset.logoVariant;
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

function safeOptionalColor(value: unknown, label: string): string | undefined {
  if (value === undefined || value === '') return undefined;
  if (typeof value !== 'string' || !SAFE_COLOR.test(value.trim())) throw new Error(`Invalid ${label}`);
  return value.trim();
}

function safeFont(value: unknown): string | undefined {
  if (value === undefined || value === '') return undefined;
  if (typeof value !== 'string') throw new Error('Invalid font family');
  const trimmed = value.trim();
  if (!trimmed || trimmed.length > 160 || !SAFE_FONT.test(trimmed) || /url\s*\(|@import/i.test(trimmed)) throw new Error('Invalid font family');
  return trimmed;
}

function normalizePlainText(value: unknown, maxLength: number): string | undefined {
  if (value === undefined || value === '') return undefined;
  if (typeof value !== 'string') throw new Error('Theme content must be plain text');
  const trimmed = value.trim().replace(/[\r\n\t]+/g, ' ');
  if (!trimmed) return undefined;
  if (Array.from(trimmed).length > maxLength) throw new Error(`Theme content exceeds ${maxLength} characters`);
  return trimmed;
}

function normalizeComponentStyle(value: ThemeComponentStyle, slot: ThemeComponentSlot): ThemeComponentStyle {
  const background = value.background?.trim();
  if (background && !SAFE_COLOR.test(background) && !SAFE_GRADIENT.test(background)) throw new Error(`Invalid ${slot} background`);
  const borderColor = safeOptionalColor(value.borderColor, `${slot} border`);
  const boxShadow = value.boxShadow?.trim();
  if (boxShadow && (boxShadow.length > 240 || !SAFE_SHADOW.test(boxShadow) || /url\s*\(|@import|[;{}]/i.test(boxShadow))) throw new Error(`Invalid ${slot} shadow`);
  return {
    ...(background ? { background } : {}),
    ...(borderColor ? { borderColor } : {}),
    ...(boxShadow ? { boxShadow } : {}),
  };
}

function safePosition(value: string | undefined): string {
  if (!value) return 'center';
  if (!/^[a-z0-9.% +\-]+$/i.test(value)) throw new Error('Invalid background position');
  return value;
}

function escapeCssUrl(value: string): string { return value.replace(/["\\\n\r]/g, (char) => `\\${char}`); }

function hexRgb(value: string): [number, number, number] | null {
  const match = /^#([0-9a-f]{6})$/i.exec(value.trim());
  if (!match) return null;
  return [0, 2, 4].map((offset) => Number.parseInt(match[1].slice(offset, offset + 2), 16)) as [number, number, number];
}
