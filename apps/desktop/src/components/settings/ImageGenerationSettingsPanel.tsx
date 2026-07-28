import { useCallback, useEffect, useMemo, useState } from "react";
import { ChevronDown, Eye, EyeOff, Image as ImageIcon, Save } from "lucide-react";
import { useTranslation } from "../../i18n";
import type {
  AgentConfig,
  AppConfig,
  ImageGenerationConfig,
  PluginManifest,
  PluginRuntimeCheck,
} from "../../types/conversation";
import * as api from "../../lib/api";
import {
  findImageProviderPreset,
  getDefaultImageModel,
  IMAGE_PROVIDER_PRESETS,
  type ImageProviderPreset,
} from "../../lib/imageProviderPresets";
import { ProviderIcon } from "../../lib/providerIcons";
import {
  findSharedProviderCredential,
  providerCredentialScope,
} from "../../lib/providerCredentials";
import { Badge } from "../ui/Badge";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";

interface ImageGenerationSettingsPanelProps {
  appConfig: AppConfig;
  agentConfigs: AgentConfig[];
  loading: boolean;
  onChange: (config: AppConfig) => void;
  onMarkDirty: () => void;
  onSave: (config?: AppConfig) => void | Promise<void>;
}

const DEFAULT_IMAGE_CONFIG: ImageGenerationConfig = {
  provider: "open_ai",
  apiStyle: "openai_images",
  apiKey: "",
  baseUrl: "https://api.openai.com/v1",
  model: "gpt-image-2",
  size: "1024x1024",
  quality: null,
  outputFormat: "png",
};

function firstOption(options: string[]): string | null {
  return options.length > 0 ? options[0] : null;
}

function firstSize(preset: ImageProviderPreset | null): string | null {
  return preset?.sizeOptions[0]?.value ?? null;
}

function normalizeUrl(value: string | null | undefined): string {
  return (value ?? "").trim().replace(/\/+$/, "").toLowerCase();
}

function isDefaultImageConfig(config: ImageGenerationConfig): boolean {
  return (
    !config.apiKey.trim() &&
    config.provider === DEFAULT_IMAGE_CONFIG.provider &&
    config.apiStyle === DEFAULT_IMAGE_CONFIG.apiStyle &&
    normalizeUrl(config.baseUrl) === normalizeUrl(DEFAULT_IMAGE_CONFIG.baseUrl) &&
    config.model === DEFAULT_IMAGE_CONFIG.model
  );
}

function configFromPreset(
  current: ImageGenerationConfig,
  preset: ImageProviderPreset,
): ImageGenerationConfig {
  const preservesCredential = providerCredentialScope(current.provider, current.baseUrl) ===
    providerCredentialScope(preset.provider, preset.baseUrl);
  return {
    ...current,
    provider: preset.provider,
    apiStyle: preset.apiStyle,
    baseUrl: preset.baseUrl,
    model: getDefaultImageModel(preset),
    size: firstSize(preset),
    quality: firstOption(preset.qualityOptions),
    outputFormat: firstOption(preset.outputFormats),
    apiKey: preservesCredential ? current.apiKey : "",
  };
}

function isImageProviderPreset(value: unknown): value is ImageProviderPreset {
  if (!value || typeof value !== "object") return false;
  const record = value as Record<string, unknown>;
  return (
    typeof record.id === "string" &&
    typeof record.name === "string" &&
    typeof record.provider === "string" &&
    typeof record.apiStyle === "string" &&
    typeof record.baseUrl === "string" &&
    Array.isArray(record.models) &&
    Array.isArray(record.sizeOptions) &&
    Array.isArray(record.qualityOptions) &&
    Array.isArray(record.outputFormats)
  );
}

function extractImageProviderPresets(plugin: PluginManifest | null): ImageProviderPreset[] {
  const catalog = plugin?.providerCatalogs?.find((item) => item.id === "imageProviders");
  const presets = (catalog?.items ?? []).filter(isImageProviderPreset);
  return presets.length > 0 ? presets : IMAGE_PROVIDER_PRESETS;
}

function fallbackPresetForConfig(
  config: ImageGenerationConfig,
  providerPresets: ImageProviderPreset[],
): ImageProviderPreset {
  return (
    providerPresets.find(
      (preset) =>
        preset.provider === config.provider &&
        preset.apiStyle === config.apiStyle,
    ) ??
    providerPresets.find((preset) => preset.provider === config.provider) ??
    providerPresets[0] ??
    IMAGE_PROVIDER_PRESETS[0]
  );
}

function presetForAgentConfig(
  config: AgentConfig,
  providerPresets: ImageProviderPreset[],
): ImageProviderPreset | null {
  if (config.provider === "custom") return null;

  const credentialScope = providerCredentialScope(config.provider, config.baseUrl);
  const candidates = providerPresets.filter(
    (preset) => providerCredentialScope(preset.provider, preset.baseUrl) === credentialScope,
  );
  if (candidates.length === 0) return null;

  const baseUrl = normalizeUrl(config.baseUrl);
  if (config.provider === "qwen") {
    // Token Plan text credentials use a dedicated endpoint and cannot be
    // reused with the DashScope image-generation API.
    if (baseUrl.includes("token-plan.")) return null;
    const regionPreset = baseUrl.includes("dashscope-intl")
      ? candidates.find((preset) => preset.id.includes("intl"))
      : candidates.find((preset) => preset.id.includes("cn"));
    if (regionPreset) return regionPreset;
  }

  return candidates.find((preset) => preset.id !== "custom-openai-images") ?? candidates[0] ?? null;
}

function runtimeCheckVariant(check: PluginRuntimeCheck) {
  if (check.status === "error" || check.severity === "error") return "danger" as const;
  if (check.status === "warning" || check.severity === "warning") return "warning" as const;
  if (check.status === "pass") return "success" as const;
  return "default" as const;
}

export function ImageGenerationSettingsPanel({
  appConfig,
  agentConfigs,
  loading,
  onChange,
  onMarkDirty,
  onSave,
}: ImageGenerationSettingsPanelProps) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const [showKey, setShowKey] = useState(false);
  const [preferAgentDefaults, setPreferAgentDefaults] = useState(true);
  const [plugin, setPlugin] = useState<PluginManifest | null>(null);
  const storedImageConfig = appConfig.imageGeneration ?? DEFAULT_IMAGE_CONFIG;
  const loadPlugin = useCallback(async () => {
    try {
      const plugins = await api.listBuiltinPlugins();
      setPlugin(plugins.find((candidate) => candidate.id === "image-generation") ?? null);
    } catch (error) {
      console.error("[image-plugin] failed to load plugin manifest", error);
      setPlugin(null);
    }
  }, []);

  useEffect(() => {
    void loadPlugin();
  }, [loadPlugin]);

  const providerPresets = useMemo(() => extractImageProviderPresets(plugin), [plugin]);
  const preferredAgentPreset = useMemo(() => {
    const imageCapableConfigs = agentConfigs.filter(
      (config) => config.apiKey.trim() && presetForAgentConfig(config, providerPresets),
    );
    const preferredConfig =
      imageCapableConfigs.find((config) => config.isDefault) ?? imageCapableConfigs[0] ?? null;

    return preferredConfig ? presetForAgentConfig(preferredConfig, providerPresets) : null;
  }, [agentConfigs, providerPresets]);
  const imageConfig =
    preferAgentDefaults && preferredAgentPreset && isDefaultImageConfig(storedImageConfig)
      ? configFromPreset(storedImageConfig, preferredAgentPreset)
      : storedImageConfig;
  const activePreset = useMemo(
    () =>
      findImageProviderPreset({
        provider: imageConfig.provider,
        apiStyle: imageConfig.apiStyle,
        baseUrl: imageConfig.baseUrl,
      }, providerPresets) ?? fallbackPresetForConfig(imageConfig, providerPresets),
    [imageConfig.apiStyle, imageConfig.baseUrl, imageConfig.provider, providerPresets],
  );
  const hasPresetModel =
    activePreset.models.length > 0 &&
    activePreset.models.some((model) => model.id === imageConfig.model);
  const sharedKeySource = useMemo(
    () => findSharedProviderCredential(agentConfigs, activePreset.provider, activePreset.baseUrl),
    [activePreset.baseUrl, activePreset.provider, agentConfigs],
  );
  const resolvedApiKey = imageConfig.apiKey.trim() || sharedKeySource?.apiKey.trim() || "";
  const usesSharedProviderKey = !imageConfig.apiKey.trim() && Boolean(sharedKeySource);
  const configured = Boolean(resolvedApiKey && imageConfig.model.trim());
  const materializedImageConfig = useMemo(
    () => ({
      ...imageConfig,
      apiKey: resolvedApiKey,
    }),
    [imageConfig, resolvedApiKey],
  );
  const materializedAppConfig = useMemo(
    () => ({
      ...appConfig,
      imageGeneration: materializedImageConfig,
    }),
    [appConfig, materializedImageConfig],
  );

  const updateImageConfig = (next: ImageGenerationConfig) => {
    setPreferAgentDefaults(false);
    onChange({ ...appConfig, imageGeneration: next });
    onMarkDirty();
  };

  const applyPreset = (presetId: string) => {
    const preset =
      providerPresets.find((candidate) => candidate.id === presetId) ??
      providerPresets[0] ??
      IMAGE_PROVIDER_PRESETS[0];
    updateImageConfig(configFromPreset(imageConfig, preset));
  };

  const currentPresetId = activePreset.id;
  const runtimeChecks = (plugin?.runtimeChecks ?? []).filter((check) => check.status !== "unknown");
  const runtimeCheckLabels: Record<string, string> = {
    "provider-preset": t('settings.provider'),
    "api-key": t('settings.apiKey'),
    "base-url": t('settings.baseUrl'),
    model: t('settings.model'),
  };

  const handleSave = async () => {
    await onSave(materializedAppConfig);
    void loadPlugin();
  };

  return (
    <div
      className="rounded-lg border border-border bg-surface-2"
      data-testid="image-generation-settings-panel"
    >
      <button
        type="button"
        aria-expanded={expanded}
        aria-label={expanded ? t('settings.collapseImageGeneration') : t('settings.expandImageGeneration')}
        onClick={() => setExpanded((value) => !value)}
        className="flex w-full items-center gap-3 p-3 text-left transition-colors hover:bg-surface-3/40"
      >
        <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-accent/10 text-accent">
          <ImageIcon size={18} />
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="text-sm font-semibold text-text-primary">{t('settings.imageGeneration')}</h3>
            <Badge
              variant="default"
              className={
                configured
                  ? "border-success/20 bg-success/10 text-success"
                  : "border-warning/25 bg-warning/10 text-warning"
              }
            >
              {configured ? t('settings.configured') : t('settings.needsApiKey')}
            </Badge>
            <Badge variant="default" className="border-border bg-surface-1 text-text-secondary">
              {usesSharedProviderKey && sharedKeySource
                ? t('settings.providerApiKeySource', { provider: sharedKeySource.name })
                : imageConfig.apiKey.trim()
                  ? t('settings.dedicatedApiKeySource')
                  : t('settings.noApiKeySource')}
            </Badge>
          </div>
          <p className="mt-0.5 truncate text-xs text-text-tertiary">
            {activePreset.name} · {imageConfig.model || t('settings.model')}
          </p>
          {runtimeChecks.length > 0 && (
            <div className="mt-2 flex flex-wrap gap-1.5">
              {runtimeChecks.map((check) => (
                <Badge
                  key={check.id}
                  variant={runtimeCheckVariant(check)}
                  className="text-[10px]"
                >
                  {runtimeCheckLabels[check.id] ?? check.label}
                </Badge>
              ))}
            </div>
          )}
        </div>
        <ChevronDown
          size={16}
          className={`shrink-0 text-text-tertiary transition-transform ${expanded ? "rotate-180" : ""}`}
        />
      </button>

      {expanded && (
        <div className="border-t border-border px-4 py-4">
          <p className="mb-4 text-xs text-text-tertiary">
            {t('settings.imageGenerationDesc')}
          </p>
          <div className="grid gap-4 md:grid-cols-2">
          <div className="space-y-2">
            <label className="text-sm font-medium text-text-primary">{t('settings.provider')}</label>
            <select
              value={currentPresetId}
              onChange={(event) => applyPreset(event.target.value)}
              className="h-10 w-full cursor-pointer rounded-md border border-border bg-surface-1 px-3.5 text-sm text-text-primary transition-colors hover:border-border-hover focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent/30"
            >
              {providerPresets.map((preset) => (
                <option key={preset.id} value={preset.id}>
                  {preset.name}
                </option>
              ))}
            </select>
            <div className="flex items-center gap-2 text-xs text-text-tertiary">
              <ProviderIcon
                provider={activePreset.provider}
                providerId={activePreset.id}
                baseUrl={activePreset.baseUrl}
                size="sm"
              />
              <span className="truncate">{activePreset.name}</span>
            </div>
          </div>

          <div className="space-y-2">
            <label className="text-sm font-medium text-text-primary">{t('settings.apiKey')}</label>
            <div className="relative">
              <Input
                type={showKey ? "text" : "password"}
                value={imageConfig.apiKey}
                onChange={(event) =>
                  updateImageConfig({ ...imageConfig, apiKey: event.target.value })
                }
                placeholder="sk-..."
                className="pr-10"
              />
              <button
                type="button"
                onClick={() => setShowKey((value) => !value)}
                className="absolute right-3 top-1/2 -translate-y-1/2 text-text-tertiary transition-colors hover:text-text-secondary"
                aria-label={showKey ? t('settings.hideKey') : t('settings.showKey')}
              >
                {showKey ? <EyeOff size={14} /> : <Eye size={14} />}
              </button>
            </div>
            <p className="text-xs text-text-tertiary">
              {usesSharedProviderKey && sharedKeySource
                ? t('settings.providerApiKeySource', { provider: sharedKeySource.name })
                : imageConfig.apiKey.trim()
                  ? t('settings.dedicatedApiKeySource')
                  : t('settings.noApiKeySource')}
            </p>
          </div>

          <div className="space-y-2">
            <label className="text-sm font-medium text-text-primary">{t('settings.baseUrl')}</label>
            <Input
              value={imageConfig.baseUrl ?? ""}
              onChange={(event) =>
                updateImageConfig({ ...imageConfig, baseUrl: event.target.value || null })
              }
              placeholder={activePreset.baseUrl}
            />
          </div>

          <div className="space-y-2">
            <label className="text-sm font-medium text-text-primary">{t('settings.model')}</label>
            {activePreset.models.length > 0 && hasPresetModel ? (
              <select
                value={imageConfig.model}
                onChange={(event) =>
                  updateImageConfig({ ...imageConfig, model: event.target.value })
                }
                className="h-10 w-full cursor-pointer rounded-md border border-border bg-surface-1 px-3.5 text-sm text-text-primary transition-colors hover:border-border-hover focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent/30"
              >
                {activePreset.models.map((model) => (
                  <option key={model.id} value={model.id}>
                    {model.name}{model.recommended ? " *" : ""}
                  </option>
                ))}
              </select>
            ) : (
              <Input
                value={imageConfig.model}
                onChange={(event) =>
                  updateImageConfig({ ...imageConfig, model: event.target.value })
                }
                placeholder={getDefaultImageModel(activePreset) || "model-name"}
              />
            )}
          </div>

          {activePreset.sizeOptions.length > 0 && (
            <div className="space-y-2">
              <label className="text-sm font-medium text-text-primary">{t('settings.defaultSize')}</label>
              <select
                value={imageConfig.size ?? ""}
                onChange={(event) =>
                  updateImageConfig({ ...imageConfig, size: event.target.value || null })
                }
                className="h-10 w-full cursor-pointer rounded-md border border-border bg-surface-1 px-3.5 text-sm text-text-primary transition-colors hover:border-border-hover focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent/30"
              >
                {activePreset.sizeOptions.map((size) => (
                  <option key={size.value} value={size.value}>
                    {size.label}
                  </option>
                ))}
              </select>
            </div>
          )}

          {activePreset.qualityOptions.length > 0 && (
            <div className="space-y-2">
              <label className="text-sm font-medium text-text-primary">{t('settings.quality')}</label>
              <select
                value={imageConfig.quality ?? ""}
                onChange={(event) =>
                  updateImageConfig({ ...imageConfig, quality: event.target.value || null })
                }
                className="h-10 w-full cursor-pointer rounded-md border border-border bg-surface-1 px-3.5 text-sm text-text-primary transition-colors hover:border-border-hover focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent/30"
              >
                {activePreset.qualityOptions.map((quality) => (
                  <option key={quality} value={quality}>
                    {quality}
                  </option>
                ))}
              </select>
            </div>
          )}

          {activePreset.outputFormats.length > 1 && (
            <div className="space-y-2">
              <label className="text-sm font-medium text-text-primary">{t('settings.outputFormat')}</label>
              <select
                value={imageConfig.outputFormat ?? ""}
                onChange={(event) =>
                  updateImageConfig({ ...imageConfig, outputFormat: event.target.value || null })
                }
                className="h-10 w-full cursor-pointer rounded-md border border-border bg-surface-1 px-3.5 text-sm text-text-primary transition-colors hover:border-border-hover focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent/30"
              >
                {activePreset.outputFormats.map((format) => (
                  <option key={format} value={format}>
                    {format}
                  </option>
                ))}
              </select>
            </div>
          )}
          </div>
          <div className="mt-4 flex justify-end border-t border-border pt-3">
            <Button
              type="button"
              variant="primary"
              size="sm"
              icon={<Save size={14} />}
              loading={loading}
              onClick={() => void handleSave()}
              disabled={!imageConfig.model.trim() || !resolvedApiKey}
            >
              {t('common.save')}
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
