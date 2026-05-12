import { useCallback, useEffect, useMemo, useState } from 'react';
import { RefreshCw, Trash2 } from 'lucide-react';
import { toast } from 'sonner';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import type { ApprovalPolicy, ApprovalPolicyList, ApprovalRisk, ToolAccessInfo } from '../../types';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';

export type ToolApprovalMode = 'ask' | 'allow_all' | 'deny_all';

interface ToolApprovalControlProps {
  mode: ToolApprovalMode;
  onChange: (mode: ToolApprovalMode) => void;
}

const accessCopy = {
  en: {
    title: 'Tool capability overview',
    description: 'What the agent is allowed to ask for, shown before any specific approval prompt appears.',
    noTools: 'No tool capability data loaded.',
    read: 'Reads',
    write: 'Writes',
    execute: 'Executes',
    network: 'Network',
    approval: 'Needs approval',
    noApproval: 'No approval',
    low: 'Low',
    medium: 'Medium',
    high: 'High',
  },
  zh: {
    title: '工具能力总览',
    description: '这里先说明 agent 可能请求哪些能力；真正执行高风险工具时仍会弹出具体审批。',
    noTools: '还没有加载到工具能力数据。',
    read: '读取',
    write: '写入',
    execute: '执行',
    network: '联网',
    approval: '需要审批',
    noApproval: '无需审批',
    low: '低',
    medium: '中',
    high: '高',
  },
};

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

export function ToolApprovalControl({ mode, onChange }: ToolApprovalControlProps) {
  const { t, locale } = useTranslation();
  const copy = locale.startsWith('zh') ? accessCopy.zh : accessCopy.en;
  const [policies, setPolicies] = useState<ApprovalPolicyList>({ persisted: [], session: [] });
  const [accessMap, setAccessMap] = useState<ToolAccessInfo[]>([]);
  const [loading, setLoading] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    const [policyResult, accessResult] = await Promise.allSettled([
      api.listToolApprovalPolicies(),
      api.listToolAccessMap(),
    ]);
    if (policyResult.status === 'fulfilled') {
      setPolicies(policyResult.value);
    } else {
      console.error('[approval] list policies failed', policyResult.reason);
    }
    if (accessResult.status === 'fulfilled') {
      setAccessMap(accessResult.value);
    } else {
      console.error('[approval] list tool access map failed', accessResult.reason);
    }
    setLoading(false);
  }, []);

  useEffect(() => { void load(); }, [load]);

  const remove = useCallback(async (p: ApprovalPolicy, scope: 'session' | 'forever') => {
    try {
      await api.deleteToolApprovalPolicy(p.toolName, scope, p.permissionKey);
      await load();
    } catch (err) {
      console.error('[approval] delete policy failed', err);
      toast.error(String(err));
    }
  }, [load]);

  const clearAll = useCallback(async () => {
    try {
      await api.clearToolApprovalPolicies();
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
  const sortedAccessMap = useMemo(
    () =>
      [...accessMap].sort(
        (left, right) =>
          riskRank(left.riskLevel) - riskRank(right.riskLevel)
          || left.name.localeCompare(right.name),
      ),
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
            <div className="text-sm font-medium text-text-primary">{copy.title}</div>
            <div className="mt-0.5 text-xs text-text-tertiary">{copy.description}</div>
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

        {sortedAccessMap.length === 0 ? (
          <div className="text-xs text-text-tertiary">{copy.noTools}</div>
        ) : (
          <div className="max-h-96 space-y-2 overflow-auto pr-1">
            {sortedAccessMap.map((tool) => (
              <div key={tool.name} className="rounded-md border border-border/60 bg-surface-1 px-3 py-2">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div className="min-w-0">
                    <div className="flex min-w-0 flex-wrap items-center gap-2">
                      <span className="truncate font-mono text-xs text-text-primary">{tool.name}</span>
                      <Badge variant={riskVariant(tool.riskLevel)} className="text-[10px]">
                        {tool.riskLevel === 'high'
                          ? copy.high
                          : tool.riskLevel === 'medium'
                            ? copy.medium
                            : copy.low}
                      </Badge>
                      <Badge variant={tool.needsApproval ? 'warning' : 'default'} className="text-[10px]">
                        {tool.needsApproval ? copy.approval : copy.noApproval}
                      </Badge>
                    </div>
                    <div className="mt-1 text-xs leading-relaxed text-text-tertiary">
                      {tool.riskReason}
                    </div>
                  </div>
                  <div className="flex shrink-0 flex-wrap justify-end gap-1">
                    {tool.canRead && <Badge variant="default" className="text-[10px]">{copy.read}</Badge>}
                    {tool.canWrite && <Badge variant="danger" className="text-[10px]">{copy.write}</Badge>}
                    {tool.canExecute && <Badge variant="warning" className="text-[10px]">{copy.execute}</Badge>}
                    {tool.canAccessNetwork && <Badge variant="info" className="text-[10px]">{copy.network}</Badge>}
                  </div>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
