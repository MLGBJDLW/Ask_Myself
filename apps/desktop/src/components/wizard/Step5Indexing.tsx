import { useCallback, useEffect, useRef, useState } from 'react';
import { motion } from 'framer-motion';
import { CheckCircle, Loader2, Sparkles } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '../ui/Button';
import { WizardFooter } from './WizardFooter';
import { useTranslation } from '../../i18n';
import { useProgress, progressStore } from '../../lib/progressStore';
import * as api from '../../lib/api';

interface Step5IndexingProps {
  onNext: () => void;
  onPrev: () => void;
  onSkip: () => void;
  sourceId: string | null;
  indexingCompleted: boolean;
  onCompleted: () => void;
}

/**
 * Step 5 — Background indexing with progress feedback.
 *
 * Kicks off `api.scanSource` automatically on mount (once per source).
 * Progress events arrive via the existing {@link progressStore} — we don't
 * subscribe to any new Tauri events.
 */
export function Step5Indexing({
  onNext,
  onPrev,
  onSkip,
  sourceId,
  indexingCompleted,
  onCompleted,
}: Step5IndexingProps) {
  const { t } = useTranslation();
  const progress = useProgress();
  const [scanning, setScanning] = useState(false);
  const [filesAdded, setFilesAdded] = useState<number | null>(null);
  const startedForRef = useRef<string | null>(null);

  const runScan = useCallback(async (id: string) => {
    setScanning(true);
    setFilesAdded(null);
    progressStore.update('scanProgress', null);
    try {
      const result = await api.scanSource(id);
      setFilesAdded(result.filesAdded + result.filesUpdated);
      onCompleted();
      toast.success(t('wizard.indexingDone', { count: String(result.filesAdded) }));
    } catch (e) {
      toast.error(`${t('wizard.indexingError')}: ${String(e)}`);
    } finally {
      setScanning(false);
      progressStore.update('scanProgress', null);
    }
  }, [onCompleted, t]);

  // Auto-start the scan once when we arrive on this step with a source.
  useEffect(() => {
    if (!sourceId || indexingCompleted) return;
    if (startedForRef.current === sourceId) return;
    startedForRef.current = sourceId;
    void runScan(sourceId);
  }, [sourceId, indexingCompleted, runScan]);

  const scan = progress.scanProgress;
  const pct = scan && scan.total > 0
    ? Math.min(100, Math.round((scan.current / scan.total) * 100))
    : null;

  const primaryLabel = scanning
    ? t('wizard.indexingInProgress')
    : indexingCompleted
      ? t('wizard.continue')
      : t('wizard.continueLater');

  return (
    <motion.div
      key="step5"
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -8 }}
      transition={{ duration: 0.2 }}
      className="flex flex-col items-center text-center"
    >
      <div className={`w-14 h-14 rounded-2xl flex items-center justify-center mb-6 ${
        indexingCompleted ? 'bg-success/10' : 'bg-accent/10'
      }`}>
        {indexingCompleted ? (
          <CheckCircle size={28} className="text-success" />
        ) : scanning ? (
          <Loader2 size={28} className="text-accent animate-spin" />
        ) : (
          <Sparkles size={28} className="text-accent" />
        )}
      </div>
      <h2 className="text-2xl font-semibold text-text-primary mb-2">
        {indexingCompleted ? t('wizard.indexingDoneTitle') : t('wizard.indexingTitle')}
      </h2>
      <p className="text-sm text-text-tertiary mb-6 max-w-md">
        {t('wizard.indexingDescription')}
      </p>

      {sourceId && (scanning || indexingCompleted) && (
        <div className="w-full mb-4">
          <div className="h-2 w-full rounded-full bg-surface-2 overflow-hidden">
            <div
              className="h-full bg-accent transition-[width] duration-300"
              style={{ width: `${indexingCompleted ? 100 : (pct ?? 10)}%` }}
            />
          </div>
          <p className="mt-2 text-xs text-text-tertiary">
            {scan && !indexingCompleted
              ? t('wizard.indexingProgress', {
                  current: String(scan.current),
                  total: String(scan.total),
                })
              : indexingCompleted
                ? t('wizard.indexingDone', { count: String(filesAdded ?? 0) })
                : t('wizard.indexingStarting')}
          </p>
        </div>
      )}

      {!sourceId && (
        <p className="text-xs text-text-tertiary mb-4">
          {t('wizard.indexingNoSource')}
        </p>
      )}

      {sourceId && !scanning && !indexingCompleted && (
        <Button variant="secondary" size="md" onClick={() => runScan(sourceId)} className="mb-2">
          {t('wizard.indexingRetry')}
        </Button>
      )}

      <WizardFooter
        onPrev={onPrev}
        onSkip={scanning ? undefined : onSkip}
        onNext={onNext}
        primaryLabel={primaryLabel}
      />
    </motion.div>
  );
}
