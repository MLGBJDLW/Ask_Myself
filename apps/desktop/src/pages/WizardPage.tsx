import { useCallback } from 'react';
import { AnimatePresence } from 'framer-motion';
import { useNavigate } from 'react-router-dom';
import { toast } from 'sonner';
import { useTranslation } from '../i18n';
import * as api from '../lib/api';
import { useWizardState, WIZARD_TOTAL_STEPS } from '../components/wizard/useWizardState';
import { Step1Welcome } from '../components/wizard/Step1Welcome';
import { Step2Language } from '../components/wizard/Step2Language';
import { Step3Provider } from '../components/wizard/Step3Provider';
import { Step4Source } from '../components/wizard/Step4Source';
import { Step5Indexing } from '../components/wizard/Step5Indexing';
import { Step6Done } from '../components/wizard/Step6Done';

/**
 * First-run setup wizard (route: `/wizard`).
 *
 * Flow: Welcome → Language → LLM Provider → Folder → Indexing → Done.
 *
 * Persistence contract:
 *   - Each step persists its own field to the backend via existing commands.
 *   - Only the final "Enter app" CTA writes `wizard_completed=1`.
 *   - Any failure is non-blocking: user may skip with partial state.
 */
export function WizardPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { step, form, updateForm, goNext, goPrev } = useWizardState();

  const finishWizard = useCallback(
    async (openChat: boolean) => {
      try {
        await api.setWizardCompleted();
      } catch (e) {
        // Non-fatal: user still lands on the main app; they just might see the
        // wizard again on next launch.  Surface a toast so it's not silent.
        toast.error(`${t('wizard.completeError')}: ${String(e)}`);
      }
      navigate(openChat ? '/chat' : '/', { replace: true });
    },
    [navigate, t],
  );

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-surface-0">
      <div className="w-full max-w-lg mx-auto px-6 py-8">
        {/* Progress indicator */}
        <div className="flex items-center justify-center gap-2 mb-6" aria-hidden>
          {Array.from({ length: WIZARD_TOTAL_STEPS }, (_, i) => (
            <div
              key={i}
              className={`h-1.5 rounded-full transition-all duration-300 ${
                i + 1 === step
                  ? 'w-10 bg-accent'
                  : i + 1 < step
                    ? 'w-6 bg-accent/40'
                    : 'w-6 bg-border'
              }`}
            />
          ))}
        </div>
        <p className="text-center text-xs text-text-tertiary mb-6">
          {t('wizard.step', { current: String(step), total: String(WIZARD_TOTAL_STEPS) })}
        </p>

        <AnimatePresence mode="wait">
          {step === 1 && <Step1Welcome onNext={goNext} />}
          {step === 2 && (
            <Step2Language
              onNext={goNext}
              onPrev={goPrev}
              onSkip={goNext}
              value={form.locale}
              onChange={locale => updateForm('locale', locale)}
            />
          )}
          {step === 3 && (
            <Step3Provider
              onNext={goNext}
              onPrev={goPrev}
              onSkip={goNext}
              selectedPreset={form.providerPreset}
              apiKey={form.apiKey}
              providerSaved={form.providerSaved}
              connectionTest={form.connectionTest}
              onPresetChange={p => updateForm('providerPreset', p)}
              onApiKeyChange={v => updateForm('apiKey', v)}
              onTestResult={(result, saved) => {
                updateForm('connectionTest', result);
                updateForm('providerSaved', saved);
              }}
            />
          )}
          {step === 4 && (
            <Step4Source
              onNext={goNext}
              onPrev={goPrev}
              onSkip={goNext}
              sourcePath={form.sourcePath}
              sourceId={form.sourceId}
              onSourceAdded={(path, id) => {
                updateForm('sourcePath', path);
                updateForm('sourceId', id);
              }}
            />
          )}
          {step === 5 && (
            <Step5Indexing
              onNext={goNext}
              onPrev={goPrev}
              onSkip={goNext}
              sourceId={form.sourceId}
              indexingCompleted={form.indexingCompleted}
              onCompleted={() => updateForm('indexingCompleted', true)}
            />
          )}
          {step === 6 && (
            <Step6Done
              onFinish={() => void finishWizard(false)}
              onOpenChat={() => void finishWizard(true)}
              providerPreset={form.providerPreset}
              providerSaved={form.providerSaved}
              sourcePath={form.sourcePath}
              indexingCompleted={form.indexingCompleted}
            />
          )}
        </AnimatePresence>
      </div>
    </div>
  );
}
