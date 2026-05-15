import { useMemo, useState } from "react";
import { Eye, EyeOff, Image as ImageIcon, Save } from "lucide-react";
import type { AppConfig, ImageGenerationConfig } from "../../types/conversation";
import {
  findImageProviderPreset,
  getDefaultImageModel,
  IMAGE_PROVIDER_PRESETS,
  type ImageProviderPreset,
} from "../../lib/imageProviderPresets";
import { ProviderIcon } from "../../lib/providerIcons";
import { Badge } from "../ui/Badge";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";

interface ImageGenerationSettingsPanelProps {
  appConfig: AppConfig;
  loading: boolean;
  onChange: (config: AppConfig) => void;
  onMarkDirty: () => void;
  onSave: () => void;
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

function fallbackPresetForConfig(config: ImageGenerationConfig): ImageProviderPreset {
  return (
    IMAGE_PROVIDER_PRESETS.find(
      (preset) =>
        preset.provider === config.provider &&
        preset.apiStyle === config.apiStyle,
    ) ??
    IMAGE_PROVIDER_PRESETS.find((preset) => preset.provider === config.provider) ??
    IMAGE_PROVIDER_PRESETS[0]
  );
}

export function ImageGenerationSettingsPanel({
  appConfig,
  loading,
  onChange,
  onMarkDirty,
  onSave,
}: ImageGenerationSettingsPanelProps) {
  const [showKey, setShowKey] = useState(false);
  const imageConfig = appConfig.imageGeneration ?? DEFAULT_IMAGE_CONFIG;
  const activePreset = useMemo(
    () =>
      findImageProviderPreset({
        provider: imageConfig.provider,
        apiStyle: imageConfig.apiStyle,
        baseUrl: imageConfig.baseUrl,
      }) ?? fallbackPresetForConfig(imageConfig),
    [imageConfig.apiStyle, imageConfig.baseUrl, imageConfig.provider],
  );
  const hasPresetModel =
    activePreset.models.length > 0 &&
    activePreset.models.some((model) => model.id === imageConfig.model);
  const configured = Boolean(imageConfig.apiKey.trim() && imageConfig.model.trim());

  const updateImageConfig = (next: ImageGenerationConfig) => {
    onChange({ ...appConfig, imageGeneration: next });
    onMarkDirty();
  };

  const applyPreset = (presetId: string) => {
    const preset =
      IMAGE_PROVIDER_PRESETS.find((candidate) => candidate.id === presetId) ??
      IMAGE_PROVIDER_PRESETS[0];
    updateImageConfig({
      ...imageConfig,
      provider: preset.provider,
      apiStyle: preset.apiStyle,
      baseUrl: preset.baseUrl,
      model: getDefaultImageModel(preset),
      size: firstSize(preset),
      quality: firstOption(preset.qualityOptions),
      outputFormat: firstOption(preset.outputFormats),
    });
  };

  const currentPresetId = activePreset.id;

  return (
    <div className="rounded-xl border border-border bg-surface-2">
      <div className="flex items-start gap-3 p-4">
        <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-accent/10 text-accent">
          <ImageIcon size={18} />
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="text-sm font-semibold text-text-primary">Image generation</h3>
            <Badge
              variant="default"
              className={
                configured
                  ? "border-success/20 bg-success/10 text-success"
                  : "border-warning/25 bg-warning/10 text-warning"
              }
            >
              {configured ? "Configured" : "Needs API key"}
            </Badge>
          </div>
          <p className="mt-0.5 text-xs text-text-tertiary">
            Dedicated provider for generate_image. This is separate from chat LLM providers.
          </p>
        </div>
        <Button
          type="button"
          variant="primary"
          size="sm"
          icon={<Save size={14} />}
          loading={loading}
          onClick={onSave}
          disabled={!imageConfig.model.trim() || !imageConfig.apiKey.trim()}
        >
          Save
        </Button>
      </div>

      <div className="border-t border-border px-4 py-4">
        <div className="grid gap-4 md:grid-cols-2">
          <div className="space-y-2">
            <label className="text-sm font-medium text-text-primary">Provider</label>
            <select
              value={currentPresetId}
              onChange={(event) => applyPreset(event.target.value)}
              className="h-10 w-full cursor-pointer rounded-md border border-border bg-surface-1 px-3.5 text-sm text-text-primary transition-colors hover:border-border-hover focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent/30"
            >
              {IMAGE_PROVIDER_PRESETS.map((preset) => (
                <option key={preset.id} value={preset.id}>
                  {preset.name}
                </option>
              ))}
            </select>
            <div className="flex items-center gap-2 text-xs text-text-tertiary">
              <ProviderIcon provider={activePreset.provider} size="sm" />
              <span className="truncate">{activePreset.description}</span>
            </div>
          </div>

          <div className="space-y-2">
            <label className="text-sm font-medium text-text-primary">API key</label>
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
                aria-label={showKey ? "Hide key" : "Show key"}
              >
                {showKey ? <EyeOff size={14} /> : <Eye size={14} />}
              </button>
            </div>
          </div>

          <div className="space-y-2">
            <label className="text-sm font-medium text-text-primary">Base URL</label>
            <Input
              value={imageConfig.baseUrl ?? ""}
              onChange={(event) =>
                updateImageConfig({ ...imageConfig, baseUrl: event.target.value || null })
              }
              placeholder={activePreset.baseUrl}
            />
          </div>

          <div className="space-y-2">
            <label className="text-sm font-medium text-text-primary">Model</label>
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
              <label className="text-sm font-medium text-text-primary">Default size</label>
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
              <label className="text-sm font-medium text-text-primary">Quality</label>
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
              <label className="text-sm font-medium text-text-primary">Output format</label>
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
      </div>
    </div>
  );
}
