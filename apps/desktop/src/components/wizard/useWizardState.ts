import { useCallback, useState } from 'react';
import type { Locale } from '../../i18n/types';
import type { ProviderPreset } from '../../lib/providerPresets';

export const WIZARD_TOTAL_STEPS = 6;

/**
 * Connection-test result kept in the wizard form state so that the
 * "Save & continue" button can surface the most recent attempt.
 */
export type ConnectionTestResult = 'success' | 'failed' | null;

/** Persistent form data collected across wizard steps. */
export interface WizardForm {
  /** Chosen UI locale (propagated to I18nProvider as soon as step 2 completes). */
  locale: Locale | null;

  /** Provider preset selected in step 3. */
  providerPreset: ProviderPreset | null;
  /** API key typed by the user (never persisted until "test connection" succeeds). */
  apiKey: string;
  /** Whether the provider was saved to DB successfully. */
  providerSaved: boolean;
  /** Last result returned by `api.testAgentConnection`. */
  connectionTest: ConnectionTestResult;

  /** Folder path picked in step 4. */
  sourcePath: string | null;
  /** Id of the persisted source (after `api.addSource`). */
  sourceId: string | null;

  /** Whether the background scan / index ran to completion. */
  indexingCompleted: boolean;
}

const EMPTY_FORM: WizardForm = {
  locale: null,
  providerPreset: null,
  apiKey: '',
  providerSaved: false,
  connectionTest: null,
  sourcePath: null,
  sourceId: null,
  indexingCompleted: false,
};

/**
 * Single, intentionally-simple state manager for the setup wizard.
 *
 * Keeps step index (1-based) + accumulated form data.  Callers are expected
 * to persist each field to the backend as it is collected; this hook only
 * tracks local UI state.
 */
export function useWizardState(initialStep = 1) {
  const [step, setStep] = useState<number>(initialStep);
  const [form, setForm] = useState<WizardForm>(EMPTY_FORM);

  const updateForm = useCallback(<K extends keyof WizardForm>(key: K, value: WizardForm[K]) => {
    setForm(prev => ({ ...prev, [key]: value }));
  }, []);

  const goNext = useCallback(() => {
    setStep(prev => Math.min(prev + 1, WIZARD_TOTAL_STEPS));
  }, []);

  const goPrev = useCallback(() => {
    setStep(prev => Math.max(prev - 1, 1));
  }, []);

  const goTo = useCallback((target: number) => {
    setStep(Math.max(1, Math.min(target, WIZARD_TOTAL_STEPS)));
  }, []);

  return { step, form, updateForm, goNext, goPrev, goTo };
}
