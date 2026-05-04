import { useState } from 'react';
import { AnimatePresence, motion, useReducedMotion } from 'framer-motion';
import {
  AlertTriangle,
  Bot,
  CheckCircle,
  ChevronDown,
  Loader2,
  RefreshCw,
  Wrench,
  XCircle,
} from 'lucide-react';
import { useTranslation } from '../../i18n';
import type { OfficeDependencyStatus, OfficeRuntimeReadiness } from '../../lib/api';
import { getSoftCollapseMotion } from '../../lib/uiMotion';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';

interface OfficeRuntimePanelProps {
  readiness: OfficeRuntimeReadiness | null;
  preparing: boolean;
  onPrepare: () => void;
  onRefresh: () => void;
  onAskAiPrepare: () => void;
}

export function OfficeRuntimePanel({
  readiness,
  preparing,
  onPrepare,
  onRefresh,
  onAskAiPrepare,
}: OfficeRuntimePanelProps) {
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();
  const [detailsOpen, setDetailsOpen] = useState(false);
  const status = readiness?.status ?? 'missing';
  const statusMeta = (() => {
    if (!readiness) {
      return { label: t('settings.documentToolsChecking'), variant: 'info' as const, icon: <Loader2 size={14} className="animate-spin" /> };
    }
    switch (status) {
      case 'ready':
        return { label: t('settings.documentToolsReady'), variant: 'success' as const, icon: <CheckCircle size={14} /> };
      case 'degraded':
        return { label: t('settings.documentToolsDegraded'), variant: 'warning' as const, icon: <AlertTriangle size={14} /> };
      case 'blocked':
        return { label: t('settings.documentToolsBlocked'), variant: 'danger' as const, icon: <XCircle size={14} /> };
      default:
        return { label: t('settings.documentToolsMissing'), variant: 'warning' as const, icon: <AlertTriangle size={14} /> };
    }
  })();
  const requiredDeps = readiness?.dependencies.filter((dep) => dep.required) ?? [];
  const optionalDeps = readiness?.dependencies.filter((dep) => !dep.required) ?? [];
  const canPrepare = Boolean(readiness?.canPrepare) || !readiness;

  const renderDep = (dep: OfficeDependencyStatus) => (
    <div key={dep.id} className="flex flex-col gap-2 py-2.5 sm:flex-row sm:items-start sm:justify-between">
      <div className="min-w-0">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm font-medium text-text-primary">{dep.label}</span>
          <Badge variant={dep.status === 'ready' ? 'success' : dep.status === 'broken' ? 'danger' : 'warning'} className="shrink-0">
            {dep.status === 'ready'
              ? t('settings.modelReady')
              : dep.status === 'broken'
                ? t('settings.documentToolsBlocked')
                : t('settings.documentToolsMissing')}
          </Badge>
        </div>
        {(dep.version || dep.path) && (
          <p className="mt-1 truncate text-xs text-text-tertiary">
            {dep.version ? `v${dep.version}` : dep.path}
          </p>
        )}
      </div>
      {dep.detail && dep.status !== 'ready' && (
        <p className="max-w-sm text-left text-xs leading-relaxed text-text-tertiary sm:text-right">
          {dep.detail}
        </p>
      )}
    </div>
  );

  return (
    <div className="rounded-lg border border-border bg-surface-1 p-4">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <h4 className="text-sm font-medium text-text-primary">{t('settings.documentTools')}</h4>
            <Badge variant={statusMeta.variant} className="gap-1">
              {statusMeta.icon}
              {statusMeta.label}
            </Badge>
          </div>
          <p className="mt-1 text-xs leading-relaxed text-text-tertiary">
            {readiness?.summary ?? t('settings.documentToolsDesc')}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <Button
            variant="secondary"
            size="sm"
            icon={<Bot size={14} />}
            onClick={onAskAiPrepare}
          >
            {t('chat.askAi')}
          </Button>
          <Button
            variant="ghost"
            size="sm"
            icon={<RefreshCw size={14} />}
            iconOnly
            onClick={onRefresh}
            title={t('settings.documentToolsRefresh')}
            aria-label={t('settings.documentToolsRefresh')}
          />
          <Button
            variant={readiness?.status === 'blocked' ? 'secondary' : 'primary'}
            size="sm"
            icon={<Wrench size={14} />}
            loading={preparing}
            disabled={!canPrepare}
            onClick={onPrepare}
          >
            {preparing ? t('settings.documentToolsPreparing') : t('settings.documentToolsPrepare')}
          </Button>
          <button
            type="button"
            onClick={() => setDetailsOpen((value) => !value)}
            className="rounded-md p-1.5 text-text-tertiary transition-colors hover:bg-surface-3 hover:text-text-secondary"
            aria-expanded={detailsOpen}
            aria-label={detailsOpen ? t('common.collapse') : t('common.expand')}
          >
            <ChevronDown
              size={16}
              className={`transition-transform ${detailsOpen ? 'rotate-180' : ''}`}
            />
          </button>
        </div>
      </div>

      <AnimatePresence initial={false}>
        {detailsOpen && readiness && (
          <motion.div
            {...getSoftCollapseMotion(!!shouldReduceMotion)}
            className="overflow-hidden"
          >
            <div className="mt-4 space-y-4 border-t border-border pt-4">
              <div className="grid gap-3 text-xs sm:grid-cols-2">
                <div className="min-w-0">
                  <p className="font-medium text-text-secondary">{t('settings.documentToolsManagedEnv')}</p>
                  <p className="mt-1 truncate text-text-tertiary" title={readiness.appManagedEnvPath}>
                    {readiness.appManagedEnvPath}
                  </p>
                </div>
                <div className="min-w-0">
                  <p className="font-medium text-text-secondary">{t('settings.documentToolsPython')}</p>
                  <p className="mt-1 truncate text-text-tertiary" title={readiness.pythonPath ?? readiness.pythonDownloadUrl}>
                    {readiness.pythonPath ?? t('settings.documentToolsPythonMissing')}
                  </p>
                </div>
              </div>

              {readiness.needsPythonInstall && (
                <div className="rounded-md border border-warning/30 bg-warning/10 px-3 py-2 text-xs leading-relaxed text-warning">
                  {t('settings.documentToolsPythonMissing')}: <span className="break-all">{readiness.pythonDownloadUrl}</span>
                </div>
              )}

              <div className="grid gap-4 lg:grid-cols-2">
                <div>
                  <p className="mb-1 text-xs font-medium text-text-secondary">{t('settings.documentToolsRequired')}</p>
                  <div className="divide-y divide-border">{requiredDeps.map(renderDep)}</div>
                </div>
                <div>
                  <p className="mb-1 text-xs font-medium text-text-secondary">{t('settings.documentToolsOptional')}</p>
                  <div className="divide-y divide-border">{optionalDeps.map(renderDep)}</div>
                </div>
              </div>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
