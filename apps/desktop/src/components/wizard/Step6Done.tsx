import { useEffect, useState } from 'react';
import { motion } from 'framer-motion';
import { Brain, CheckCircle, FileText, FolderOpen, MessageSquare, Sparkles, Wrench } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '../ui/Button';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import { getModelStatus, invalidate as invalidateModelStatus } from '../../lib/modelStatusCache';
import type { ProviderPreset } from '../../lib/providerPresets';

interface Step6DoneProps {
  onFinish: () => void;
  onOpenChat: () => void;
  providerPreset: ProviderPreset | null;
  providerSaved: boolean;
  sourcePath: string | null;
  indexingCompleted: boolean;
}

/**
 * Step 6 — Summary + final CTA.
 *
 * Writes `wizard_completed=1` via the onFinish callback (handled by the
 * parent `WizardPage`).  Shows a small checklist so the user can see what
 * was and wasn't configured before proceeding.
 */
export function Step6Done({
  onFinish,
  onOpenChat,
  providerPreset,
  providerSaved,
  sourcePath,
  indexingCompleted,
}: Step6DoneProps) {
  const { t } = useTranslation();
  const [officeRuntime, setOfficeRuntime] = useState<api.OfficeRuntimeReadiness | null>(null);
  const [officePreparing, setOfficePreparing] = useState(false);
  const officeReady = officeRuntime?.status === 'ready' || officeRuntime?.status === 'degraded';

  useEffect(() => {
    let cancelled = false;
    getModelStatus('office', 'runtime', () => api.checkOfficeRuntime())
      .then((readiness) => {
        if (!cancelled) setOfficeRuntime(readiness);
      })
      .catch(() => {
        if (!cancelled) setOfficeRuntime(null);
      });
    return () => { cancelled = true; };
  }, []);

  const handlePrepareOfficeRuntime = async () => {
    if (officePreparing) return;
    setOfficePreparing(true);
    try {
      const result = await api.prepareOfficeRuntime();
      setOfficeRuntime(result.readiness);
      invalidateModelStatus('office');
      if (result.success) {
        toast.success(t('wizard.documentToolsSetupSuccess'));
      } else {
        toast.error(result.readiness.summary || t('wizard.documentToolsSetupError'));
      }
    } catch (e) {
      toast.error(t('wizard.documentToolsSetupError') + ': ' + String(e));
    } finally {
      setOfficePreparing(false);
    }
  };

  const items = [
    {
      icon: <Brain size={14} />,
      done: providerSaved,
      label: providerSaved
        ? `${providerPreset?.name ?? ''} — ${t('wizard.connectionSuccess')}`
        : `${t('wizard.providerTitle')} — ${t('wizard.skipped')}`,
    },
    {
      icon: <FolderOpen size={14} />,
      done: Boolean(sourcePath),
      label: sourcePath
        ? t('wizard.folderSelected', { folder: sourcePath })
        : `${t('wizard.sourceTitle')} — ${t('wizard.skipped')}`,
    },
    {
      icon: <Sparkles size={14} />,
      done: indexingCompleted,
      label: indexingCompleted
        ? t('wizard.indexingDoneTitle')
        : `${t('wizard.indexingTitle')} — ${t('wizard.indexingBackground')}`,
    },
    {
      icon: <FileText size={14} />,
      done: Boolean(officeReady),
      label: officeReady
        ? t('wizard.documentToolsReady')
        : t('wizard.documentToolsNeedsSetup'),
    },
  ];

  return (
    <motion.div
      key="step6"
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -8 }}
      transition={{ duration: 0.2 }}
      className="flex flex-col items-center text-center"
    >
      <div className="w-14 h-14 rounded-2xl bg-success/10 flex items-center justify-center mb-6">
        <CheckCircle size={28} className="text-success" />
      </div>
      <h2 className="text-2xl font-semibold text-text-primary mb-2">
        {t('wizard.doneTitle')}
      </h2>
      <p className="text-sm text-text-tertiary mb-6 max-w-md">
        {t('wizard.doneDescription')}
      </p>

      <div className="w-full space-y-2 mb-8">
        {items.map((item, idx) => (
          <div
            key={idx}
            className={`flex items-center gap-2 px-4 py-2.5 rounded-lg text-sm ${
              item.done ? 'bg-success/10 text-success' : 'bg-surface-2 text-text-tertiary'
            }`}
          >
            {item.icon}
            <span className="truncate text-left flex-1">{item.label}</span>
          </div>
        ))}
      </div>

      {!officeReady && (
        <div className="mb-8 w-full rounded-lg border border-border bg-surface-1 p-3 text-left">
          <div className="mb-1 flex items-center gap-2 text-sm font-medium text-text-primary">
            <FileText size={15} />
            {t('wizard.documentToolsTitle')}
          </div>
          <p className="mb-3 text-xs leading-relaxed text-text-tertiary">
            {officeRuntime?.needsPythonInstall
              ? `${t('settings.documentToolsPythonMissing')}: ${officeRuntime.pythonDownloadUrl}`
              : t('wizard.documentToolsDescription')}
          </p>
          <Button
            size="sm"
            variant="secondary"
            icon={<Wrench size={14} />}
            loading={officePreparing}
            disabled={officeRuntime !== null && !officeRuntime.canPrepare}
            onClick={handlePrepareOfficeRuntime}
          >
            {t('wizard.documentToolsSetup')}
          </Button>
        </div>
      )}

      <div className="flex w-full flex-col sm:flex-row gap-3">
        <Button size="lg" onClick={onFinish} className="flex-1">
          {t('wizard.enterApp')}
        </Button>
        <Button variant="secondary" size="lg" onClick={onOpenChat} className="flex-1">
          <MessageSquare size={16} />
          {t('wizard.openSampleChat')}
        </Button>
      </div>
    </motion.div>
  );
}
