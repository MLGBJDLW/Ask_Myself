import { useMemo } from 'react';

import { useTranslation } from '../../i18n';
import type { ModelCatalogSurface, ModelDescriptor } from '../../lib/modelCatalog';
import { NexaCombobox } from '../ui/overlay/Combobox';

export interface CatalogModelPickerItem {
  id: string;
  name: string;
  recommended?: boolean;
  descriptor: ModelDescriptor;
  secondary?: string | null;
}

interface CatalogModelPickerProps {
  models: readonly CatalogModelPickerItem[];
  value: string;
  onValueChange: (value: string) => void;
  surface: ModelCatalogSurface;
  className?: string;
  dataTestId?: string;
  placeholder?: string;
}

function modalityLabel(modality: string, t: ReturnType<typeof useTranslation>['t']): string {
  const labels: Record<string, string> = {
    text: t('settings.modelModalityText'),
    image: t('settings.modelModalityImage'),
    audio: t('settings.modelModalityAudio'),
    video: t('settings.modelModalityVideo'),
    file: t('settings.modelModalityFile'),
    embedding: t('settings.modelModalityEmbedding'),
  };
  return labels[modality] ?? modality;
}

export function catalogModelOptionDescription(
  model: CatalogModelPickerItem,
  t: ReturnType<typeof useTranslation>['t'],
): string {
  const descriptor = model.descriptor;
  const io = `${descriptor.inputModalities.map((item) => modalityLabel(item, t)).join('+')}→${descriptor.outputModalities.map((item) => modalityLabel(item, t)).join('+')}`;
  const region = descriptor.regions.length > 0 ? descriptor.regions.join(', ') : null;
  const source = descriptor.source === 'discovered' ? t('settings.modelSourceDiscovered') : null;
  const credential = descriptor.availableToCredential === false
    ? t('settings.modelCredentialUnavailable')
    : null;
  return [model.id, model.secondary, credential, source, region, io].filter(Boolean).join(' · ');
}

/** Searchable model picker with a compact selected label and two-line catalog rows. */
export function CatalogModelPicker({
  models,
  value,
  onValueChange,
  surface,
  className = '',
  dataTestId,
  placeholder,
}: CatalogModelPickerProps) {
  const { t } = useTranslation();
  const options = useMemo(() => models.map((model) => ({
    value: model.id,
    label: `${model.name}${model.recommended ? ' ★' : ''}`,
    description: catalogModelOptionDescription(model, t),
    keywords: [model.id, model.descriptor.family, model.descriptor.providerId, ...(model.descriptor.aliases ?? [])],
    disabled: model.descriptor.lifecycle === 'removed' || model.descriptor.availableToCredential === false,
  })), [models, t]);

  return (
    <div data-catalog-surface={surface}>
      <NexaCombobox
        ariaLabel={t('settings.modelPickerAria')}
        className={`h-10 w-full cursor-pointer rounded-md border border-border bg-surface-1 px-3.5 text-sm text-text-primary transition-colors hover:border-border-hover focus:border-accent ${className}`}
        dataTestId={dataTestId}
        emptyLabel={t('settings.modelSearchNoResults')}
        onValueChange={onValueChange}
        options={options}
        placeholder={placeholder ?? t('settings.selectModel')}
        searchPlaceholder={t('settings.modelSearchPlaceholder')}
        value={value}
      />
    </div>
  );
}
