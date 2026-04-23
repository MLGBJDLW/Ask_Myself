import { ChevronLeft, ChevronRight } from 'lucide-react';
import { Button } from '../ui/Button';
import { useTranslation } from '../../i18n';

interface WizardFooterProps {
  /** Zero if the current step has no "back" target (step 1). */
  onPrev?: () => void;
  /** Called when the primary/"continue" button is pressed. */
  onNext?: () => void;
  /** Called when the secondary/"skip" button is pressed (hidden if omitted). */
  onSkip?: () => void;
  /** Label override for the primary button (defaults to "Continue"). */
  primaryLabel?: string;
  /** Disable the primary button (e.g. while required fields are empty). */
  primaryDisabled?: boolean;
  /** Show a loading spinner on the primary button. */
  primaryLoading?: boolean;
}

/**
 * Shared footer rendered underneath every wizard step.  Keeps the visual
 * layout consistent (prev bottom-left, skip + next bottom-right).
 */
export function WizardFooter({
  onPrev,
  onNext,
  onSkip,
  primaryLabel,
  primaryDisabled,
  primaryLoading,
}: WizardFooterProps) {
  const { t } = useTranslation();

  return (
    <div className="mt-8 flex w-full items-center justify-between gap-3">
      <div>
        {onPrev && (
          <Button variant="ghost" size="md" onClick={onPrev}>
            <ChevronLeft size={16} />
            {t('wizard.back')}
          </Button>
        )}
      </div>
      <div className="flex items-center gap-2">
        {onSkip && (
          <Button variant="ghost" size="md" onClick={onSkip}>
            {t('wizard.skip')}
          </Button>
        )}
        {onNext && (
          <Button
            size="md"
            onClick={onNext}
            disabled={primaryDisabled}
            loading={primaryLoading}
          >
            {primaryLabel ?? t('wizard.continue')}
            <ChevronRight size={16} />
          </Button>
        )}
      </div>
    </div>
  );
}
