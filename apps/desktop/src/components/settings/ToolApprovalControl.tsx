import { useCallback, useEffect, useState } from 'react';
import { Trash2 } from 'lucide-react';
import { toast } from 'sonner';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import type { ApprovalPolicy, ApprovalPolicyList } from '../../types';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';

export type ToolApprovalMode = 'ask' | 'allow_all' | 'deny_all';

interface ToolApprovalControlProps {
  mode: ToolApprovalMode;
  onChange: (mode: ToolApprovalMode) => void;
}

export function ToolApprovalControl({ mode, onChange }: ToolApprovalControlProps) {
  const { t } = useTranslation();
  const [policies, setPolicies] = useState<ApprovalPolicyList>({ persisted: [], session: [] });
  const [loading, setLoading] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const list = await api.listToolApprovalPolicies();
      setPolicies(list);
    } catch (err) {
      console.error('[approval] list policies failed', err);
    } finally {
      setLoading(false);
    }
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
    </div>
  );
}
