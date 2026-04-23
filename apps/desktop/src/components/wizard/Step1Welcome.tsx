import { motion } from 'framer-motion';
import { ChevronRight } from 'lucide-react';
import { Button } from '../ui/Button';
import { Logo } from '../Logo';
import { useTranslation } from '../../i18n';

interface Step1WelcomeProps {
  onNext: () => void;
}

/** Step 1 — Welcome splash with a single CTA. */
export function Step1Welcome({ onNext }: Step1WelcomeProps) {
  const { t } = useTranslation();
  return (
    <motion.div
      key="step1"
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -8 }}
      transition={{ duration: 0.2 }}
      className="flex flex-col items-center text-center"
    >
      <Logo size={72} className="mb-6" />
      <h1 className="text-3xl font-bold text-text-primary mb-3">{t('wizard.welcomeTitle')}</h1>
      <p className="text-base text-text-secondary mb-2 max-w-md">{t('wizard.welcomeSubtitle')}</p>
      <p className="text-sm text-text-tertiary mb-8 max-w-md">{t('wizard.welcomeDescription')}</p>
      <Button size="lg" onClick={onNext}>
        {t('wizard.getStarted')}
        <ChevronRight size={16} />
      </Button>
    </motion.div>
  );
}
