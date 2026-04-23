import { useCallback, useState } from 'react';
import { motion } from 'framer-motion';
import { CheckCircle, FolderOpen } from 'lucide-react';
import { open } from '@tauri-apps/plugin-dialog';
import { toast } from 'sonner';
import { Button } from '../ui/Button';
import { WizardFooter } from './WizardFooter';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';

interface Step4SourceProps {
  onNext: () => void;
  onPrev: () => void;
  onSkip: () => void;
  sourcePath: string | null;
  sourceId: string | null;
  onSourceAdded: (path: string, sourceId: string) => void;
}

/**
 * Step 4 — Knowledge folder picker.
 *
 * Uses the existing Tauri dialog + `api.addSource` path (no new backend
 * logic), so the persisted source is immediately visible on the Sources
 * page.  Failure here is non-blocking: user can skip and retry later.
 */
export function Step4Source({
  onNext,
  onPrev,
  onSkip,
  sourcePath,
  sourceId,
  onSourceAdded,
}: Step4SourceProps) {
  const { t } = useTranslation();
  const [adding, setAdding] = useState(false);

  const handleSelect = useCallback(async () => {
    const selected = await open({
      directory: true,
      multiple: false,
      title: t('wizard.selectFolder'),
    });
    if (!selected) return;
    const path = typeof selected === 'string' ? selected : selected[0];
    if (!path) return;

    setAdding(true);
    try {
      const source = await api.addSource(path, ['**/*'], []);
      onSourceAdded(path, source.id);
      toast.success(t('wizard.folderSelected', { folder: path }));
    } catch {
      toast.error(t('wizard.folderError'));
    } finally {
      setAdding(false);
    }
  }, [t, onSourceAdded]);

  return (
    <motion.div
      key="step4"
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -8 }}
      transition={{ duration: 0.2 }}
      className="flex flex-col items-center text-center"
    >
      <div className="w-14 h-14 rounded-2xl bg-accent/10 flex items-center justify-center mb-6">
        <FolderOpen size={28} className="text-accent" />
      </div>
      <h2 className="text-2xl font-semibold text-text-primary mb-2">
        {t('wizard.sourceTitle')}
      </h2>
      <p className="text-sm text-text-tertiary mb-6 max-w-md">
        {t('wizard.sourceDescription')}
      </p>

      {sourcePath ? (
        <div className="flex items-center gap-2 px-4 py-3 rounded-lg bg-success/10 border border-success/20 text-success text-sm mb-2 max-w-full">
          <CheckCircle size={16} className="shrink-0" />
          <span className="truncate">{sourcePath}</span>
        </div>
      ) : (
        <Button size="lg" onClick={handleSelect} loading={adding} className="mb-2">
          <FolderOpen size={16} />
          {t('wizard.selectFolder')}
        </Button>
      )}

      <WizardFooter
        onPrev={onPrev}
        onSkip={onSkip}
        onNext={onNext}
        primaryDisabled={!sourceId}
      />
    </motion.div>
  );
}
