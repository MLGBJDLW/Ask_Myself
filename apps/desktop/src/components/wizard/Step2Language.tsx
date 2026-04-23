import { motion } from 'framer-motion';
import { Languages } from 'lucide-react';
import { useTranslation } from '../../i18n';
import type { Locale } from '../../i18n/types';
import { WizardFooter } from './WizardFooter';

interface Step2LanguageProps {
  onNext: () => void;
  onPrev: () => void;
  onSkip: () => void;
  value: Locale | null;
  onChange: (locale: Locale) => void;
}

/**
 * Step 2 — Language picker.
 *
 * Reuses the {@link useTranslation} provider so switching immediately
 * re-renders every subsequent step in the chosen language.
 */
export function Step2Language({ onNext, onPrev, onSkip, value, onChange }: Step2LanguageProps) {
  const { t, locale, setLocale, availableLocales } = useTranslation();
  const selected: Locale = value ?? locale;

  const handleSelect = (next: Locale) => {
    setLocale(next);
    onChange(next);
  };

  return (
    <motion.div
      key="step2"
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -8 }}
      transition={{ duration: 0.2 }}
      className="flex flex-col items-center text-center"
    >
      <div className="w-14 h-14 rounded-2xl bg-accent/10 flex items-center justify-center mb-6">
        <Languages size={28} className="text-accent" />
      </div>
      <h2 className="text-2xl font-semibold text-text-primary mb-2">
        {t('wizard.languageTitle')}
      </h2>
      <p className="text-sm text-text-tertiary mb-6 max-w-md">
        {t('wizard.languageDescription')}
      </p>

      <div className="w-full grid grid-cols-2 sm:grid-cols-3 gap-2">
        {availableLocales.map(l => (
          <button
            key={l.code}
            type="button"
            onClick={() => handleSelect(l.code)}
            className={`rounded-lg border px-3 py-2.5 text-sm font-medium transition-all ${
              selected === l.code
                ? 'border-accent bg-accent/10 text-accent ring-1 ring-accent/20'
                : 'border-border bg-surface-1 text-text-secondary hover:border-border-hover'
            }`}
          >
            {l.name}
          </button>
        ))}
      </div>

      <WizardFooter onPrev={onPrev} onSkip={onSkip} onNext={onNext} />
    </motion.div>
  );
}
