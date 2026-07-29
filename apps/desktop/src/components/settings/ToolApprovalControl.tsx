import { useCallback, useEffect, useMemo, useState } from 'react';
import { RefreshCw, Trash2 } from 'lucide-react';
import { toast } from 'sonner';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import type {
  ToolPermissionPolicy,
  ToolPermissionPolicyList,
  ApprovalRisk,
  ToolAccessInfo,
  CapabilityOwner,
} from '../../types';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';

export type ToolApprovalMode = 'ask' | 'allow_all' | 'deny_all';

interface ToolApprovalControlProps {
  mode: ToolApprovalMode;
  onChange: (mode: ToolApprovalMode) => void;
}

function riskRank(risk: ApprovalRisk): number {
  if (risk === 'high') return 0;
  if (risk === 'medium') return 1;
  return 2;
}

function riskVariant(risk: ApprovalRisk) {
  if (risk === 'high') return 'danger' as const;
  if (risk === 'medium') return 'warning' as const;
  return 'success' as const;
}

interface ToolAccessGroup {
  owner: CapabilityOwner;
  tools: ToolAccessInfo[];
  riskLevel: ApprovalRisk;
  needsApprovalCount: number;
  canRead: boolean;
  canWrite: boolean;
  canExecute: boolean;
  canAccessNetwork: boolean;
}

function fallbackPlugin(tool: ToolAccessInfo): CapabilityOwner {
  return {
    id: tool.category || 'tooling',
    name: tool.category || 'Tooling',
    capability: tool.category || 'Tooling',
    description: tool.riskReason,
  };
}

function highestRisk(tools: ToolAccessInfo[]): ApprovalRisk {
  if (tools.some((tool) => tool.riskLevel === 'high')) return 'high';
  if (tools.some((tool) => tool.riskLevel === 'medium')) return 'medium';
  return 'low';
}

export function ToolApprovalControl({ mode, onChange }: ToolApprovalControlProps) {
  const { t } = useTranslation();
  const [policies, setPolicies] = useState<ToolPermissionPolicyList>({ persisted: [], session: [] });
  const [accessMap, setAccessMap] = useState<ToolAccessInfo[]>([]);
  const [loading, setLoading] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    const [policyResult, accessResult] = await Promise.allSettled([
      api.listToolPermissionPolicies(),
      api.listToolAccessMap(),
    ]);
    if (policyResult.status === 'fulfilled') {
      setPolicies({
        persisted: Array.isArray(policyResult.value?.persisted) ? policyResult.value.persisted : [],
        session: Array.isArray(policyResult.value?.session) ? policyResult.value.session : [],
      });
    } else {
      console.error('[approval] list policies failed', policyResult.reason);
    }
    if (accessResult.status === 'fulfilled') {
      setAccessMap(Array.isArray(accessResult.value) ? accessResult.value : []);
    } else {
      console.error('[approval] list tool access map failed', accessResult.reason);
    }
    setLoading(false);
  }, []);

  useEffect(() => { void load(); }, [load]);

  const remove = useCallback(async (p: ToolPermissionPolicy, scope: 'session' | 'forever') => {
    try {
      await api.deleteToolPermissionPolicy(scope, p.permissionKey);
      await load();
    } catch (err) {
      console.error('[approval] delete policy failed', err);
      toast.error(String(err));
    }
  }, [load]);

  const clearAll = useCallback(async () => {
    try {
      await api.clearToolPermissionPolicies();
      await load();
    } catch (err) {
      toast.error(String(err));
    }
  }, [load]);

  const options: Array<{ value: ToolApprovalMode; label: string; desc: string }> = [
    { value: 'ask', label: t('settings.toolApprovalAsk'), desc: t('settings.toolApprovalAskDesc') },
    { value: 'allow_all', label: t('settings.toolApprovalAllowAll'), desc: t('settings.toolApprovalAllowAllDesc') },
    { value: 'deny_all', label: t('settings.toolApprovalDenyAll'), desc: t('settings.toolApprovalDenyAllDesc') },
  ];
  const accessGroups = useMemo(
    () => {
      const groups = new Map<string, ToolAccessGroup>();
      for (const tool of accessMap) {
        const owner = tool.owner ?? fallbackPlugin(tool);
        const existing = groups.get(owner.id);
        if (existing) {
          existing.tools.push(tool);
          existing.riskLevel = highestRisk(existing.tools);
          existing.needsApprovalCount = existing.tools.filter((item) => item.needsApproval).length;
          existing.canRead ||= tool.canRead;
          existing.canWrite ||= tool.canWrite;
          existing.canExecute ||= tool.canExecute;
          existing.canAccessNetwork ||= tool.canAccessNetwork;
        } else {
          groups.set(owner.id, {
            owner,
            tools: [tool],
            riskLevel: tool.riskLevel,
            needsApprovalCount: tool.needsApproval ? 1 : 0,
            canRead: tool.canRead,
            canWrite: tool.canWrite,
            canExecute: tool.canExecute,
            canAccessNetwork: tool.canAccessNetwork,
          });
        }
      }
      return [...groups.values()]
        .map((group) => ({
          ...group,
          tools: [...group.tools].sort(
            (left, right) =>
              riskRank(left.riskLevel) - riskRank(right.riskLevel)
              || left.name.localeCompare(right.name),
          ),
        }))
        .sort(
          (left, right) =>
            riskRank(left.riskLevel) - riskRank(right.riskLevel)
            || left.owner.name.localeCompare(right.owner.name),
        );
    },
    [accessMap],
  );

  return (
    <div className="space-y-2">
      <label className="text-sm font-medium text-text-primary">{t('settings.toolApproval')}</label>
      <p className="text-xs text-text-tertiary">
        {t('settings.toolApprovalDesc')}
      </p>
      <div className="grid gap-2 md:grid-cols-3">
        {options.map((o) => (
          <label
            key={o.value}
            className={`cursor-pointer rounded-lg border p-3 transition-colors ${
              mode === o.value ? 'border-accent bg-accent/10' : 'border-border bg-surface-2'
            }`}
          >
            <div className="flex items-start gap-3">
              <input
                type="radio"
                name="tool-approval-mode"
                value={o.value}
                checked={mode === o.value}
                onChange={() => onChange(o.value)}
                className="mt-1"
              />
              <div className="space-y-1">
                <div className="text-sm font-medium text-text-primary">{o.label}</div>
                <div className="text-xs text-text-tertiary">{o.desc}</div>
              </div>
            </div>
          </label>
        ))}
      </div>

      <div className="mt-3 rounded-lg border border-border bg-surface-2 p-3 space-y-2">
        <div className="flex items-center justify-between">
          <div className="text-sm font-medium text-text-primary">{t('settings.toolApprovalRemembered')}</div>
          <div className="flex items-center gap-2">
            <Button size="sm" variant="ghost" onClick={() => void load()} loading={loading}>
              {t('settings.toolApprovalRefresh')}
            </Button>
            {(policies.persisted.length > 0 || policies.session.length > 0) && (
              <Button size="sm" variant="ghost" onClick={() => void clearAll()}>
                {t('common.clearAll')}
              </Button>
            )}
          </div>
        </div>

        {policies.persisted.length === 0 && policies.session.length === 0 ? (
          <div className="text-xs text-text-tertiary">{t('settings.toolApprovalNoRemembered')}</div>
        ) : (
          <div className="space-y-1">
            {policies.persisted.map((p) => (
              <div key={`f-${p.permissionKey ?? p.toolName}`} className="flex items-center justify-between text-sm">
                <div className="flex items-center gap-2">
                  <Badge variant="default" className="text-[10px]">{t('settings.toolApprovalForever')}</Badge>
                  <span className="text-text-primary">{p.toolName}</span>
                  {p.targetKind && p.targetValue && (
                    <span className="text-xs text-text-tertiary">{p.targetKind}: {p.targetValue}</span>
                  )}
                  <span className="text-xs text-text-tertiary">{p.decision}</span>
                </div>
                <Button size="sm" variant="ghost" icon={<Trash2 size={12} />} onClick={() => void remove(p, 'forever')}>
                  {t('common.remove')}
                </Button>
              </div>
            ))}
            {policies.session.map((p) => (
              <div key={`s-${p.permissionKey ?? p.toolName}`} className="flex items-center justify-between text-sm">
                <div className="flex items-center gap-2">
                  <Badge variant="default" className="text-[10px]">{t('settings.toolApprovalSession')}</Badge>
                  <span className="text-text-primary">{p.toolName}</span>
                  {p.targetKind && p.targetValue && (
                    <span className="text-xs text-text-tertiary">{p.targetKind}: {p.targetValue}</span>
                  )}
                  <span className="text-xs text-text-tertiary">{p.decision}</span>
                </div>
                <Button size="sm" variant="ghost" icon={<Trash2 size={12} />} onClick={() => void remove(p, 'session')}>
                  {t('common.remove')}
                </Button>
              </div>
            ))}
          </div>
        )}
      </div>

      <div className="mt-3 rounded-lg border border-border bg-surface-2 p-3 space-y-3">
        <div className="flex flex-wrap items-start justify-between gap-2">
          <div>
            <div className="text-sm font-medium text-text-primary">{t('settings.toolAccessOverviewTitle')}</div>
            <div className="mt-0.5 text-xs text-text-tertiary">{t('settings.toolAccessOverviewDesc')}</div>
          </div>
          <Button
            size="sm"
            variant="ghost"
            icon={<RefreshCw size={12} />}
            onClick={() => void load()}
            loading={loading}
          >
            {t('settings.toolApprovalRefresh')}
          </Button>
        </div>

        {accessGroups.length === 0 ? (
          <div className="text-xs text-text-tertiary">{t('settings.toolAccessOverviewNoTools')}</div>
        ) : (
          <div className="max-h-96 space-y-2 overflow-auto pr-1">
            {accessGroups.map((group) => (
              <details
                key={group.owner.id}
                className="group rounded-md border border-border/60 bg-surface-1"
                open={group.riskLevel === 'high'}
              >
                <summary className="flex cursor-pointer list-none flex-wrap items-center gap-2 px-3 py-2">
                  <div className="min-w-0 flex-1">
                    <div className="flex min-w-0 flex-wrap items-center gap-2">
                      <span className="truncate text-sm font-medium text-text-primary">{group.owner.name}</span>
                      <Badge variant={riskVariant(group.riskLevel)} className="text-[10px]">
                        {group.riskLevel === 'high'
                          ? t('settings.toolRiskHigh')
                          : group.riskLevel === 'medium'
                            ? t('settings.toolRiskMedium')
                            : t('settings.toolRiskLow')}
                      </Badge>
                      <span className="text-[11px] text-text-tertiary">{group.tools.length}</span>
                      {group.needsApprovalCount > 0 && (
                        <Badge variant="warning" className="text-[10px]">
                          {t('settings.toolAccessNeedsApproval')} {group.needsApprovalCount}
                        </Badge>
                      )}
                    </div>
                    <div className="mt-0.5 truncate text-xs text-text-tertiary">
                      {group.owner.capability}
                    </div>
                  </div>
                  <div className="flex shrink-0 flex-wrap justify-end gap-1">
                    {group.canRead && <Badge variant="default" className="text-[10px]">{t('settings.toolAccessRead')}</Badge>}
                    {group.canWrite && <Badge variant="danger" className="text-[10px]">{t('settings.toolAccessWrite')}</Badge>}
                    {group.canExecute && <Badge variant="warning" className="text-[10px]">{t('settings.toolAccessExecute')}</Badge>}
                    {group.canAccessNetwork && <Badge variant="info" className="text-[10px]">{t('settings.toolAccessNetwork')}</Badge>}
                  </div>
                </summary>
                <div className="border-t border-border/50 px-3 py-2">
                  <div className="mb-2 text-xs leading-relaxed text-text-tertiary">
                    {group.owner.description}
                  </div>
                  <div className="grid gap-1.5">
                    {group.tools.map((tool) => (
                      <div key={tool.name} className="flex flex-wrap items-center justify-between gap-2 rounded-md bg-surface-0/55 px-2 py-1.5">
                        <div className="min-w-0">
                          <div className="flex min-w-0 flex-wrap items-center gap-2">
                            <span className="truncate font-mono text-xs text-text-primary">{tool.name}</span>
                            <Badge variant={riskVariant(tool.riskLevel)} className="text-[10px]">
                              {tool.riskLevel === 'high'
                                ? t('settings.toolRiskHigh')
                                : tool.riskLevel === 'medium'
                                  ? t('settings.toolRiskMedium')
                                  : t('settings.toolRiskLow')}
                            </Badge>
                          </div>
                          <div className="mt-0.5 line-clamp-2 text-[11px] leading-relaxed text-text-tertiary">
                            {tool.riskReason}
                          </div>
                        </div>
                        <div className="flex shrink-0 flex-wrap justify-end gap-1">
                          {tool.canRead && <Badge variant="default" className="text-[10px]">{t('settings.toolAccessRead')}</Badge>}
                          {tool.canWrite && <Badge variant="danger" className="text-[10px]">{t('settings.toolAccessWrite')}</Badge>}
                          {tool.canExecute && <Badge variant="warning" className="text-[10px]">{t('settings.toolAccessExecute')}</Badge>}
                          {tool.canAccessNetwork && <Badge variant="info" className="text-[10px]">{t('settings.toolAccessNetwork')}</Badge>}
                        </div>
                      </div>
                    ))}
                  </div>
                </div>
              </details>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
