import { useState, useCallback } from 'react';
import { motion } from 'framer-motion';
import { Brain, CheckCircle, Eye, EyeOff, Loader2 } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { WizardFooter } from './WizardFooter';
import { useTranslation } from '../../i18n';
import { PROVIDER_PRESETS, type ProviderPreset } from '../../lib/providerPresets';
import { ProviderIcon } from '../../lib/providerIcons';
import * as api from '../../lib/api';
import type { SaveAgentConfigInput } from '../../types/conversation';
import type { ConnectionTestResult } from './useWizardState';
import { modelDescriptorSummary, resolveExplicitModelSelection } from '../../lib/modelCatalog';

interface Step3ProviderProps {
  onNext: () => void;
  onPrev: () => void;
  onSkip: () => void;
  selectedPreset: ProviderPreset | null;
  apiKey: string;
  providerSaved: boolean;
  connectionTest: ConnectionTestResult;
  onPresetChange: (preset: ProviderPreset | null) => void;
  onApiKeyChange: (value: string) => void;
  onTestResult: (result: ConnectionTestResult, saved: boolean) => void;
}

/** Top 4 presets matching the spec (OpenAI / Anthropic / Google / Ollama). */
const WIZARD_PROVIDER_IDS = ['openai', 'anthropic', 'google', 'ollama'] as const;

/**
 * Step 3 — LLM provider configuration.
 *
 * Reuses {@link api.testAgentConnection} + {@link api.saveAgentConfig} so
 * the config persisted here is immediately available on the Settings page.
 */
export function Step3Provider({
  onNext,
  onPrev,
  onSkip,
  selectedPreset,
  apiKey,
  providerSaved,
  connectionTest,
  onPresetChange,
  onApiKeyChange,
  onTestResult,
}: Step3ProviderProps) {
  const { t } = useTranslation();
  const [showApiKey, setShowApiKey] = useState(false);
  const [testing, setTesting] = useState(false);
  const [selectedModelId, setSelectedModelId] = useState('');

  const presets = WIZARD_PROVIDER_IDS
    .map(id => PROVIDER_PRESETS.find(p => p.id === id))
    .filter((p): p is ProviderPreset => Boolean(p));

  const handleTest = useCallback(async () => {
    if (!selectedPreset) return;
    // Ollama / local providers may not require a key – guard separately.
    if (selectedPreset.requiresApiKey && !apiKey.trim()) return;

    setTesting(true);
    onTestResult(null, false);
    try {
      const selectedModel = resolveExplicitModelSelection(selectedPreset.models, selectedModelId);
      if (!selectedModel) return;
      const config: SaveAgentConfigInput = {
        id: null,
        name: selectedPreset.name,
        provider: selectedPreset.provider,
        apiKey: apiKey.trim(),
        baseUrl: selectedPreset.baseUrl,
        model: selectedModel.id,
        providerEndpointId: selectedModel.descriptor.endpointIds[0] ?? null,
        modelId: selectedModel.descriptor.id,
        temperature: null,
        maxTokens: null,
        contextWindow: null,
        isDefault: true,
        reasoningEnabled: null,
        thinkingBudget: null,
        reasoningEffort: null,
        maxIterations: null,
        summarizationModel: null,
        summarizationProvider: null,
        imageGenerationModel: null,
        subagentAllowedTools: null,
      };
      const catalog = await api.testAgentConnection(config);
      if (catalog.models.length > 0) {
        await api.saveAgentConfig(config);
        onTestResult('success', true);
        toast.success(t('wizard.connectionSuccess'));
      } else {
        onTestResult('failed', false);
        toast.error(t('wizard.connectionFailed'));
      }
    } catch {
      onTestResult('failed', false);
      toast.error(t('wizard.connectionFailed'));
    } finally {
      setTesting(false);
    }
  }, [selectedPreset, selectedModelId, apiKey, onTestResult, t]);

  const needsKey = selectedPreset?.requiresApiKey ?? false;
  const canTest = Boolean(selectedPreset && selectedModelId)
    && (!needsKey || apiKey.trim().length > 0);

  return (
    <motion.div
      key="step3"
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -8 }}
      transition={{ duration: 0.2 }}
      className="flex flex-col items-center text-center"
    >
      <div className="w-14 h-14 rounded-2xl bg-accent/10 flex items-center justify-center mb-6">
        <Brain size={28} className="text-accent" />
      </div>
      <h2 className="text-2xl font-semibold text-text-primary mb-2">
        {t('wizard.providerTitle')}
      </h2>
      <p className="text-sm text-text-tertiary mb-6 max-w-md">
        {t('wizard.providerDescription')}
      </p>

      <div className="w-full grid grid-cols-2 sm:grid-cols-4 gap-2 mb-4">
        {presets.map(p => (
          <button
            key={p.id}
            type="button"
            onClick={() => {
              onPresetChange(p);
              setSelectedModelId('');
              onTestResult(null, false);
            }}
            className={`rounded-lg border px-3 py-2.5 text-sm font-medium text-left transition-all ${
              selectedPreset?.id === p.id
                ? 'border-accent bg-accent/10 text-accent'
                : 'border-border bg-surface-1 text-text-secondary hover:border-border-hover'
            }`}
          >
            <span className="inline-flex items-center gap-2">
              <ProviderIcon provider={p.provider} providerId={p.id} baseUrl={p.baseUrl} size="sm" />
              {p.name}
            </span>
          </button>
        ))}
      </div>

      {selectedPreset && (
        <label className="mb-4 w-full space-y-2 text-left text-sm font-medium text-text-primary">
          <span>{t('settings.defaultModel')}</span>
          <select
            value={selectedModelId}
            onChange={(event) => {
              setSelectedModelId(event.target.value);
              onTestResult(null, false);
            }}
            className="h-10 w-full rounded-md border border-border bg-surface-1 px-3.5 text-sm text-text-primary focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent/30"
          >
            <option value="" disabled>—</option>
            {selectedPreset.models.map((model) => (
              <option
                key={model.id}
                value={model.id}
                disabled={model.descriptor.lifecycle === 'removed'
                  || model.descriptor.availableToCredential === false}
              >
                {model.name} · {modelDescriptorSummary(model.descriptor)}
              </option>
            ))}
          </select>
        </label>
      )}

      {selectedPreset && needsKey && (
        <div className="w-full space-y-3 mb-4">
          <div className="relative">
            <Input
              type={showApiKey ? 'text' : 'password'}
              placeholder={t('wizard.apiKey')}
              value={apiKey}
              onChange={e => {
                onApiKeyChange(e.target.value);
                onTestResult(null, false);
              }}
              className="pr-10"
            />
            <button
              type="button"
              onClick={() => setShowApiKey(v => !v)}
              className="absolute right-3 top-1/2 -translate-y-1/2 text-text-tertiary hover:text-text-secondary"
              aria-label={showApiKey ? 'Hide API key' : 'Show API key'}
            >
              {showApiKey ? <EyeOff size={14} /> : <Eye size={14} />}
            </button>
          </div>
          <Button
            variant="secondary"
            size="md"
            onClick={handleTest}
            loading={testing}
            disabled={!canTest}
            className="w-full"
          >
            {testing ? (
              <Loader2 size={14} className="animate-spin" />
            ) : connectionTest === 'success' ? (
              <CheckCircle size={14} className="text-success" />
            ) : null}
            {connectionTest === 'success'
              ? t('wizard.connectionSuccess')
              : t('wizard.testConnection')}
          </Button>
        </div>
      )}

      {selectedPreset && !needsKey && (
        <div className="w-full mb-4">
          <Button
            variant="secondary"
            size="md"
            onClick={handleTest}
            loading={testing}
            disabled={!canTest}
            className="w-full"
          >
            {connectionTest === 'success'
              ? t('wizard.connectionSuccess')
              : t('wizard.testConnection')}
          </Button>
        </div>
      )}

      <WizardFooter
        onPrev={onPrev}
        onSkip={onSkip}
        onNext={onNext}
        primaryDisabled={!providerSaved}
      />
    </motion.div>
  );
}
