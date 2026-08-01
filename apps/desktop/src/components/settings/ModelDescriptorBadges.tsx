import { modelDescriptorFacts, type ModelDescriptor } from '../../lib/modelCatalog';

interface ModelDescriptorBadgesProps {
  descriptor: ModelDescriptor | null | undefined;
  className?: string;
}

function factLabel(fact: string): string {
  const separator = fact.indexOf(':');
  if (separator < 0) return fact;
  const key = fact.slice(0, separator);
  const value = fact.slice(separator + 1);
  const labels: Record<string, string> = {
    lifecycle: 'status',
    readiness: 'readiness',
    access: 'access',
    region: 'region',
    io: 'I/O',
    source: 'source',
    verified: 'verified',
    replacement: 'replace with',
    credential: 'credential',
  };
  return `${labels[key] ?? key}: ${value.replace(/_/g, ' ')}`;
}

export function ModelDescriptorBadges({ descriptor, className = '' }: ModelDescriptorBadgesProps) {
  if (!descriptor) return null;

  return (
    <div
      className={`flex flex-wrap gap-1.5 text-[11px] text-text-tertiary ${className}`}
      data-testid="model-descriptor-badges"
      aria-label={`Model metadata for ${descriptor.displayName}`}
    >
      {modelDescriptorFacts(descriptor).map((fact) => (
        <span
          key={fact}
          className="rounded-full border border-border/70 bg-surface-2 px-2 py-0.5"
          data-model-fact={fact}
        >
          {factLabel(fact)}
        </span>
      ))}
    </div>
  );
}
