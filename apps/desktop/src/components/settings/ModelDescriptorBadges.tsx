import { useTranslation } from '../../i18n';
import type { ModelCatalogSurface, ModelDescriptor } from '../../lib/modelCatalog';

interface ModelDescriptorBadgesProps {
  descriptor: ModelDescriptor | null | undefined;
  className?: string;
  surface?: ModelCatalogSurface;
}

export function ModelDescriptorBadges({ descriptor, className = '', surface }: ModelDescriptorBadgesProps) {
  const { t } = useTranslation();
  if (!descriptor) return null;

  const facts = [
    descriptor.lifecycle !== 'active' && [t('settings.modelMetaStatus'), descriptor.lifecycle],
    descriptor.productReadiness !== 'product_ready' && [t('settings.modelMetaReadiness'), descriptor.productReadiness],
    descriptor.access !== 'public' && [t('settings.modelMetaAccess'), descriptor.access],
    descriptor.regions.length > 0 && [t('settings.modelMetaRegion'), descriptor.regions.join(', ')],
    [t('settings.modelMetaIo'), `${descriptor.inputModalities.join('+')}→${descriptor.outputModalities.join('+')}`],
    descriptor.source === 'discovered' && [t('settings.modelMetaSource'), t('settings.modelSourceDiscovered')],
    descriptor.replacementModelId && [t('settings.modelMetaReplacement'), descriptor.replacementModelId],
    descriptor.availableToCredential === false && [t('settings.modelMetaCredential'), t('settings.modelCredentialUnavailable')],
  ].filter(Boolean) as string[][];

  return (
    <div
      className={`flex flex-wrap gap-1.5 text-[11px] text-text-tertiary ${className}`}
      data-testid="model-descriptor-badges"
      data-catalog-surface={surface}
      aria-label={t('settings.modelMetadataAria', { model: descriptor.displayName })}
    >
      {facts.map(([label, value]) => (
        <span
          key={`${label}:${value}`}
          className="rounded-full border border-border/70 bg-surface-2 px-2 py-0.5"
          data-model-fact={`${label}:${value}`}
        >
          {label}: {value.replace(/_/g, ' ')}
        </span>
      ))}
    </div>
  );
}
