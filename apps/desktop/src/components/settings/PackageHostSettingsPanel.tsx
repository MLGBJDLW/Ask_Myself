import { useEffect, useMemo, useState } from 'react';
import { AlertTriangle, CheckCircle2, PackageCheck, RefreshCw, ShieldAlert } from 'lucide-react';
import { toast } from 'sonner';
import { useTranslation, type TranslationKey } from '../../i18n';
import * as api from '../../lib/api';
import type {
  PackageHealthState,
  PackageHostRecord,
  PackageHostSnapshot,
  PackageLifecycleState,
  PackageSurfaceKind,
} from '../../types/conversation';
import { Badge, type BadgeVariant } from '../ui/Badge';
import { Button } from '../ui/Button';
import { Section } from './SettingsSection';

const healthOptions: PackageHealthState[] = ['healthy', 'warning', 'unhealthy'];

const lifecycleStateLabelKeys: Record<PackageLifecycleState, TranslationKey> = {
  discovered: 'settings.packageHost.state.discovered',
  validated: 'settings.packageHost.state.validated',
  enabled: 'settings.packageHost.state.enabled',
  disabled: 'settings.packageHost.state.disabled',
  unhealthy: 'settings.packageHost.state.unhealthy',
  blocked: 'settings.packageHost.state.blocked',
};

const healthLabelKeys: Record<PackageHealthState, TranslationKey> = {
  healthy: 'settings.packageHost.health.healthy',
  warning: 'settings.packageHost.health.warning',
  unhealthy: 'settings.packageHost.health.unhealthy',
};

const surfaceLabelKeys: Record<PackageSurfaceKind, TranslationKey> = {
  capability: 'settings.packageHost.surface.capability',
  connector: 'settings.packageHost.surface.connector',
  skill: 'settings.packageHost.surface.skill',
  workflow: 'settings.packageHost.surface.workflow',
  nativePlugin: 'settings.packageHost.surface.nativePlugin',
};

const permissionLabelKeys: Record<string, TranslationKey> = {
  read: 'settings.packageHost.permission.read',
  write: 'settings.packageHost.permission.write',
  execute: 'settings.packageHost.permission.execute',
  network: 'settings.packageHost.permission.network',
  native_code: 'settings.packageHost.permission.nativeCode',
};

function isRuntimeVisible(record: PackageHostRecord): boolean {
  return record.state === 'enabled' && record.health !== 'unhealthy';
}

function stateBadgeVariant(state: PackageLifecycleState): BadgeVariant {
  if (state === 'enabled') return 'success';
  if (state === 'unhealthy' || state === 'blocked') return 'danger';
  if (state === 'disabled') return 'muted';
  return 'info';
}

function healthBadgeVariant(health: PackageHealthState): BadgeVariant {
  if (health === 'healthy') return 'success';
  if (health === 'warning') return 'warning';
  return 'danger';
}

function formatLabel(value: string): string {
  return value
    .split(/[-_]/)
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(' ');
}

function packageSurfaceSummary(record: PackageHostRecord, t: (key: TranslationKey, params?: Record<string, string | number>) => string): string {
  const counts = record.components.reduce<Record<string, number>>((acc, component) => {
    acc[component.kind] = (acc[component.kind] ?? 0) + 1;
    return acc;
  }, {});
  return Object.entries(counts)
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([kind, count]) => `${t(surfaceLabelKeys[kind as PackageSurfaceKind] ?? 'settings.packageHost.surface.unknown')} ${count}`)
    .join(' · ');
}

function permissionSummary(record: PackageHostRecord, t: (key: TranslationKey, params?: Record<string, string | number>) => string): string {
  if (record.permissions.length === 0) return t('settings.packageHost.noPermissions');
  return record.permissions
    .map((permission) => {
      const key = permissionLabelKeys[permission.key];
      return key ? t(key) : formatLabel(permission.key);
    })
    .join(' · ');
}

interface PackageRowProps {
  record: PackageHostRecord;
  busy: boolean;
  onToggleEnabled: (record: PackageHostRecord) => void;
  onHealthChange: (record: PackageHostRecord, health: PackageHealthState) => void;
}

function PackageRow({ record, busy, onToggleEnabled, onHealthChange }: PackageRowProps) {
  const { t } = useTranslation();
  const runtimeVisible = isRuntimeVisible(record);
  const enabled = record.state === 'enabled';

  return (
    <div className="rounded-lg border border-border bg-surface-2 px-4 py-3">
      <div className="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <p className="min-w-0 truncate text-sm font-semibold text-text-primary">{formatLabel(record.id)}</p>
            <Badge variant={stateBadgeVariant(record.state)}>{t(lifecycleStateLabelKeys[record.state])}</Badge>
            <Badge variant={healthBadgeVariant(record.health)}>{t(healthLabelKeys[record.health])}</Badge>
            {runtimeVisible ? (
              <Badge variant="accent" icon={<CheckCircle2 size={11} />}>{t('settings.packageHost.runtimeVisible')}</Badge>
            ) : (
              <Badge variant="muted">{t('settings.packageHost.hiddenStatus')}</Badge>
            )}
          </div>
          <p className="mt-1 text-xs text-text-tertiary">{record.id}</p>
          <div className="mt-3 grid gap-2 text-xs text-text-secondary sm:grid-cols-3">
            <div className="min-w-0">
              <p className="text-[11px] uppercase text-text-tertiary">{t('settings.packageHost.version')}</p>
              <p className="mt-0.5 truncate">{record.version ?? t('settings.packageHost.builtin')}</p>
            </div>
            <div className="min-w-0">
              <p className="text-[11px] uppercase text-text-tertiary">{t('settings.packageHost.components')}</p>
              <p className="mt-0.5 truncate">{packageSurfaceSummary(record, t) || t('settings.packageHost.none')}</p>
            </div>
            <div className="min-w-0">
              <p className="text-[11px] uppercase text-text-tertiary">{t('settings.packageHost.permissions')}</p>
              <p className="mt-0.5 truncate">{permissionSummary(record, t)}</p>
            </div>
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-2 lg:justify-end">
          <label className="flex items-center gap-2 rounded-md border border-border bg-surface-1 px-2.5 py-1.5 text-xs text-text-secondary">
            <span>{t('settings.packageHost.healthLabel')}</span>
            <select
              value={record.health}
              disabled={busy}
              onChange={(event) => onHealthChange(record, event.target.value as PackageHealthState)}
              className="rounded border border-border bg-surface-0 px-2 py-1 text-xs text-text-primary outline-none focus:border-accent"
            >
              {healthOptions.map((health) => (
                <option key={health} value={health}>{t(healthLabelKeys[health])}</option>
              ))}
            </select>
          </label>
          <Button
            variant={enabled ? 'secondary' : 'primary'}
            size="sm"
            loading={busy}
            onClick={() => onToggleEnabled(record)}
            disabled={record.state === 'blocked'}
          >
            {enabled ? t('settings.packageHost.disable') : t('settings.packageHost.enable')}
          </Button>
        </div>
      </div>
    </div>
  );
}

interface PackageHostSettingsPanelProps {
  onPackageStateChange?: () => void;
}

export function PackageHostSettingsPanel({ onPackageStateChange }: PackageHostSettingsPanelProps) {
  const { t } = useTranslation();
  const [snapshot, setSnapshot] = useState<PackageHostSnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [busyPackageId, setBusyPackageId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const loadSnapshot = async () => {
    setLoading(true);
    setError(null);
    try {
      setSnapshot(await api.getPackageHostSnapshot());
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void loadSnapshot();
  }, []);

  const stats = useMemo(() => {
    const records = snapshot?.records ?? [];
    return {
      total: records.length,
      runtime: records.filter(isRuntimeVisible).length,
      hidden: records.filter((record) => !isRuntimeVisible(record)).length,
      unhealthy: records.filter((record) => record.health === 'unhealthy').length,
    };
  }, [snapshot]);

  const updateSnapshot = async (
    record: PackageHostRecord,
    action: () => Promise<PackageHostSnapshot>,
  ) => {
    setBusyPackageId(record.id);
    setError(null);
    try {
      setSnapshot(await action());
      onPackageStateChange?.();
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      toast.error(message);
    } finally {
      setBusyPackageId(null);
    }
  };

  const handleToggleEnabled = (record: PackageHostRecord) => {
    void updateSnapshot(record, () => api.setPackageHostPackageEnabled(record.id, record.state !== 'enabled'));
  };

  const handleHealthChange = (record: PackageHostRecord, health: PackageHealthState) => {
    if (record.health === health) return;
    void updateSnapshot(record, () => api.setPackageHostPackageHealth(record.id, health));
  };

  return (
    <Section
      icon={<PackageCheck size={20} />}
      title={t('settings.packageHost.title')}
      description={t('settings.packageHost.description')}
      delay={0.005}
      collapsible
      defaultOpen={false}
      summary={
        <span className="rounded-full border border-border/60 bg-surface-2 px-2 py-1 text-[11px] text-text-secondary">
          {stats.runtime}/{stats.total}
        </span>
      }
    >
      <div className="space-y-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="grid flex-1 grid-cols-2 gap-2 sm:grid-cols-4">
            <div className="rounded-lg bg-surface-2 px-3 py-2">
              <p className="text-[11px] uppercase text-text-tertiary">{t('settings.packageHost.packages')}</p>
              <p className="mt-1 text-lg font-semibold text-text-primary">{stats.total}</p>
            </div>
            <div className="rounded-lg bg-surface-2 px-3 py-2">
              <p className="text-[11px] uppercase text-text-tertiary">{t('settings.packageHost.runtime')}</p>
              <p className="mt-1 text-lg font-semibold text-success">{stats.runtime}</p>
            </div>
            <div className="rounded-lg bg-surface-2 px-3 py-2">
              <p className="text-[11px] uppercase text-text-tertiary">{t('settings.packageHost.hidden')}</p>
              <p className="mt-1 text-lg font-semibold text-text-secondary">{stats.hidden}</p>
            </div>
            <div className="rounded-lg bg-surface-2 px-3 py-2">
              <p className="text-[11px] uppercase text-text-tertiary">{t('settings.packageHost.unhealthy')}</p>
              <p className="mt-1 text-lg font-semibold text-danger">{stats.unhealthy}</p>
            </div>
          </div>
          <Button
            variant="secondary"
            size="sm"
            icon={<RefreshCw size={14} />}
            loading={loading}
            onClick={() => { void loadSnapshot(); }}
          >
            {t('settings.packageHost.refresh')}
          </Button>
        </div>

        {error && (
          <div className="flex items-start gap-2 rounded-lg border border-danger/30 bg-danger/10 px-3 py-2 text-sm text-danger">
            <ShieldAlert size={16} className="mt-0.5 shrink-0" />
            <span className="min-w-0 break-words">{error}</span>
          </div>
        )}

        {loading && !snapshot ? (
          <div className="flex items-center gap-2 rounded-lg border border-border bg-surface-2 px-4 py-6 text-sm text-text-secondary">
            <RefreshCw size={16} className="animate-spin" />
            {t('settings.packageHost.loading')}
          </div>
        ) : snapshot && snapshot.records.length > 0 ? (
          <div className="space-y-2">
            {snapshot.records.map((record) => (
              <PackageRow
                key={record.id}
                record={record}
                busy={busyPackageId === record.id}
                onToggleEnabled={handleToggleEnabled}
                onHealthChange={handleHealthChange}
              />
            ))}
          </div>
        ) : (
          <div className="flex items-center gap-2 rounded-lg border border-border bg-surface-2 px-4 py-6 text-sm text-text-secondary">
            <AlertTriangle size={16} />
            {t('settings.packageHost.noPackages')}
          </div>
        )}
      </div>
    </Section>
  );
}
