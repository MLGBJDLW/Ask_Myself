import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  Activity,
  BrainCircuit,
  CircleAlert,
  Database,
  KeyRound,
  Loader2,
  RefreshCw,
  RotateCcw,
  ShieldCheck,
  SlidersHorizontal,
} from 'lucide-react';
import { toast } from 'sonner';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import type {
  CapabilityRegistryProjection,
  RegistryActivationRecord,
  RegistryReadMode,
  ResolvedCapabilityRoute,
} from '../../types/capabilityRegistry';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';

interface CapabilityRegistryPanelProps {
  agentId?: string;
  refreshToken: string;
}

function displayToken(value: string): string {
  return value
    .replace(/_/g, ' ')
    .replace(/\b\w/g, (character) => character.toUpperCase());
}

function sameScope(
  left: RegistryActivationRecord['scope'],
  right: ResolvedCapabilityRoute['source'],
): boolean {
  return left.kind === right.kind && (left.id ?? null) === (right.id ?? null);
}

function routeActivation(
  route: ResolvedCapabilityRoute,
  activations: RegistryActivationRecord[],
): RegistryActivationRecord | undefined {
  return activations
    .filter((activation) => activation.capabilityId === route.capabilityId)
    .find((activation) => sameScope(activation.scope, route.source));
}

export function CapabilityRegistryPanel({ agentId, refreshToken }: CapabilityRegistryPanelProps) {
  const { t } = useTranslation();
  const [projection, setProjection] = useState<CapabilityRegistryProjection | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [switchingCapability, setSwitchingCapability] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setProjection(await api.getCapabilityRegistryProjection({ agentId }));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  }, [agentId]);

  useEffect(() => {
    void load();
  }, [load, refreshToken]);

  const definitionsById = useMemo(
    () => new Map(projection?.modelDefinitions.map((definition) => [definition.id, definition]) ?? []),
    [projection],
  );

  const setReadMode = async (route: ResolvedCapabilityRoute, mode: RegistryReadMode) => {
    if (!projection) return;
    const activation = routeActivation(route, projection.activations);
    if (!activation) return;
    setSwitchingCapability(route.capabilityId);
    try {
      await api.setCapabilityRegistryReadMode(
        route.capabilityId,
        activation.scope,
        mode,
        activation.registryRevision,
      );
      toast.success(t('settings.capabilityRegistryModeUpdated'));
      await load();
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : t('settings.capabilityRegistrySwitchFailed'));
    } finally {
      setSwitchingCapability(null);
    }
  };

  if (loading && !projection) {
    return (
      <div className="flex items-center justify-center rounded-xl border border-border bg-surface-2 py-10 text-text-tertiary">
        <Loader2 size={18} className="animate-spin" />
      </div>
    );
  }

  if (error || !projection) {
    return (
      <div className="flex items-start justify-between gap-3 rounded-xl border border-danger/30 bg-danger/5 p-4">
        <div className="flex min-w-0 items-start gap-2 text-sm text-danger">
          <CircleAlert size={16} className="mt-0.5 shrink-0" />
          <span className="break-words">{error ?? t('settings.capabilityRegistryLoadFailed')}</span>
        </div>
        <Button type="button" variant="ghost" size="sm" icon={<RefreshCw size={13} />} onClick={() => void load()}>
          {t('settings.webSearchRefresh')}
        </Button>
      </div>
    );
  }

  const targetedDefinitionIds = new Set(
    projection.modelTargets
      .map((target) => target.modelDefinitionId)
      .filter((id): id is string => Boolean(id)),
  );
  const unifiedModels = [
    ...projection.modelTargets.map((target) => ({
      key: target.id,
      modelId: target.upstreamModelId,
      availability: target.availability,
      definition: target.modelDefinitionId ? definitionsById.get(target.modelDefinitionId) : undefined,
    })),
    ...projection.modelDefinitions
      .filter((definition) => !targetedDefinitionIds.has(definition.id))
      .sort((left, right) => Number(right.descriptor.recommended) - Number(left.descriptor.recommended))
      .map((definition) => ({
        key: definition.id,
        modelId: definition.descriptor.id,
        availability: definition.descriptor.productReadiness,
        definition,
      })),
  ];
  const visibleModels = unifiedModels.slice(0, 16);

  return (
    <div className="space-y-4" data-testid="capability-registry-panel">
      <div className="flex items-start justify-between gap-4">
        <div>
          <div className="flex items-center gap-2">
            <Database size={17} className="text-accent" />
            <h3 className="text-sm font-semibold text-text-primary">{t('settings.capabilityRegistryTitle')}</h3>
            <Badge variant="default" className="text-[10px]">v{projection.schemaVersion}</Badge>
          </div>
          <p className="mt-1 text-xs leading-5 text-text-tertiary">{t('settings.capabilityRegistryDesc')}</p>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          icon={<RefreshCw size={13} className={loading ? 'animate-spin' : ''} />}
          disabled={loading}
          onClick={() => void load()}
        >
          {t('settings.webSearchRefresh')}
        </Button>
      </div>

      <div className="grid gap-4 xl:grid-cols-2">
        <section className="rounded-xl border border-border bg-surface-2 p-4" data-testid="registry-connections">
          <div className="mb-3 flex items-center justify-between gap-3">
            <div className="flex items-center gap-2 text-sm font-semibold text-text-primary">
              <KeyRound size={15} className="text-accent" />
              {t('settings.capabilityRegistryConnections')}
            </div>
            <Badge variant="default" className="text-[10px]">{projection.connections.length}</Badge>
          </div>
          <div className="space-y-2">
            {projection.connections.length === 0 ? (
              <p className="text-xs text-text-tertiary">{t('settings.noProviders')}</p>
            ) : projection.connections.map((connection) => (
              <div key={connection.id} className="rounded-lg border border-border/70 bg-surface-1 px-3 py-2.5">
                <div className="flex items-center justify-between gap-3">
                  <span className="truncate text-sm font-medium text-text-primary">{displayToken(connection.providerId)}</span>
                  <Badge
                    variant="default"
                    className={`text-[10px] ${connection.health === 'configured' ? 'border-success/20 bg-success/10 text-success' : ''}`}
                  >
                    {displayToken(connection.health)}
                  </Badge>
                </div>
                <p className="mt-1 truncate font-mono text-[11px] text-text-tertiary" title={connection.baseUrl || connection.endpointId}>
                  {connection.baseUrl || connection.endpointId}
                </p>
                <p className="mt-1 text-[10px] text-text-tertiary">
                  {displayToken(connection.source.kind)} · r{connection.sourceRevision}
                </p>
              </div>
            ))}
          </div>
        </section>

        <section className="rounded-xl border border-border bg-surface-2 p-4" data-testid="registry-models">
          <div className="mb-3 flex items-center justify-between gap-3">
            <div className="flex items-center gap-2 text-sm font-semibold text-text-primary">
              <Activity size={15} className="text-accent" />
              {t('settings.models')}
            </div>
            <Badge variant="default" className="text-[10px]">{unifiedModels.length}</Badge>
          </div>
          <div className="space-y-2">
            {visibleModels.length === 0 ? (
              <p className="text-xs text-text-tertiary">{t('settings.capabilityRegistryNoModels')}</p>
            ) : visibleModels.map((model) => {
              const definition = model.definition;
              const modalities = [
                ...(definition?.descriptor.inputModalities ?? []),
                ...(definition?.descriptor.outputModalities ?? []),
              ].filter((value, index, values) => values.indexOf(value) === index);
              return (
                <div key={model.key} className="rounded-lg border border-border/70 bg-surface-1 px-3 py-2.5">
                  <div className="flex items-center justify-between gap-3">
                    <span className="truncate font-mono text-xs text-text-primary">{model.modelId}</span>
                    <Badge variant="default" className="text-[10px]">{displayToken(model.availability)}</Badge>
                  </div>
                  <div className="mt-1.5 flex flex-wrap gap-1">
                    {modalities.map((modality) => (
                      <span key={modality} className="rounded-full bg-surface-3 px-1.5 py-0.5 text-[10px] text-text-tertiary">
                        {modality}
                      </span>
                    ))}
                  </div>
                </div>
              );
            })}
            {unifiedModels.length > visibleModels.length && (
              <p className="text-center text-[11px] text-text-tertiary">
                +{unifiedModels.length - visibleModels.length} {t('settings.models').toLocaleLowerCase()}
              </p>
            )}
          </div>
        </section>
      </div>

      <section className="rounded-xl border border-border bg-surface-2 p-4" data-testid="registry-capabilities">
        <div className="mb-3 flex items-center justify-between gap-3">
          <div className="flex items-center gap-2 text-sm font-semibold text-text-primary">
            <BrainCircuit size={15} className="text-accent" />
            {t('settings.capabilityRegistryCapabilities')}
          </div>
          <Badge variant="default" className="text-[10px]">{projection.capabilities.length}</Badge>
        </div>
        <div className="space-y-2">
          {projection.capabilities.length === 0 ? (
            <p className="text-xs text-text-tertiary">{t('settings.capabilityRegistryNoCapabilities')}</p>
          ) : projection.capabilities.map((route) => {
            const activation = routeActivation(route, projection.activations);
            const primary = route.primary;
            const reasons = primary?.eligibility.reasonCodes ?? [];
            const mode = activation?.readMode ?? 'legacy';
            const hasRegistryRuntime = route.capabilityId === 'text_generation';
            return (
              <div key={`${route.capabilityId}:${route.source.kind}:${route.source.id ?? ''}`} className="rounded-lg border border-border/70 bg-surface-1 p-3">
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <div className="min-w-0">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="text-sm font-medium text-text-primary">{displayToken(route.capabilityId)}</span>
                      <Badge variant="default" className={`text-[10px] ${mode === 'registry' ? 'border-success/20 bg-success/10 text-success' : ''}`}>
                        {displayToken(mode)}
                      </Badge>
                      <Badge variant="default" className="text-[10px]">
                        {displayToken(route.fallbackMode)} fallback
                      </Badge>
                      <span className="text-[10px] text-text-tertiary">
                        {displayToken(route.source.kind)} · r{route.sourceRevision}
                      </span>
                    </div>
                    <p className="mt-1 truncate font-mono text-xs text-text-secondary">
                      {primary?.target.upstreamModelId ?? '—'}
                      {route.fallbacks.length > 0 ? ` → ${route.fallbacks.map((value) => value.target.upstreamModelId).join(' → ')}` : ''}
                    </p>
                    {!primary?.eligibility.eligible && reasons.length > 0 && (
                      <p className="mt-1 text-[11px] text-warning">{reasons.map(displayToken).join(' · ')}</p>
                    )}
                  </div>
                  {activation && hasRegistryRuntime && (
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      icon={switchingCapability === route.capabilityId ? <Loader2 size={13} className="animate-spin" /> : <RotateCcw size={13} />}
                      disabled={switchingCapability === route.capabilityId}
                      onClick={() => void setReadMode(route, mode === 'registry' ? 'legacy' : 'registry')}
                    >
                      {mode === 'registry' ? t('settings.capabilityRegistryUseLegacy') : t('settings.capabilityRegistryUseRegistry')}
                    </Button>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      </section>

      <div className="grid gap-3 md:grid-cols-2">
        <section className="rounded-xl border border-border/70 bg-surface-2 p-3" data-testid="registry-permissions-owner">
          <div className="flex items-center gap-2 text-sm font-medium text-text-primary">
            <ShieldCheck size={15} className="text-accent" />
            {t('settings.packageHost.permissions')}
          </div>
          <p className="mt-1 text-xs leading-5 text-text-tertiary">{t('settings.capabilityRegistryPermissionsDesc')}</p>
        </section>
        <section className="rounded-xl border border-border/70 bg-surface-2 p-3" data-testid="registry-advanced-owner">
          <div className="flex items-center gap-2 text-sm font-medium text-text-primary">
            <SlidersHorizontal size={15} className="text-accent" />
            {t('settings.advancedSettings')}
          </div>
          <p className="mt-1 text-xs leading-5 text-text-tertiary">{t('settings.capabilityRegistryAdvancedDesc')}</p>
        </section>
      </div>
    </div>
  );
}
