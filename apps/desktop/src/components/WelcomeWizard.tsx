import { useState, useCallback } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import {
  FolderOpen,
  CheckCircle,
  ChevronRight,
  Sparkles,
  Brain,
  Loader2,
  Eye,
  EyeOff,
} from 'lucide-react';
import { open } from '@tauri-apps/plugin-dialog';
import { toast } from 'sonner';
import { Logo } from './Logo';
import { Button } from './ui/Button';
import { Input } from './ui/Input';
import { useTranslation } from '../i18n';
import * as api from '../lib/api';
import type { SaveAgentConfigInput } from '../types/conversation';
import { PROVIDER_PRESETS, type ProviderPreset } from '../lib/providerPresets';

interface WelcomeWizardProps {
  onComplete: () => void;
}

const TOTAL_STEPS = 4;

export function WelcomeWizard({ onComplete }: WelcomeWizardProps) {
  const { t } = useTranslation();
  const [step, setStep] = useState(1);

  // Source state
  const [selectedFolder, setSelectedFolder] = useState<string | null>(null);
  const [addingSource, setAddingSource] = useState(false);

  // AI provider state
  const [selectedPreset, setSelectedPreset] = useState<ProviderPreset | null>(null);
  const [apiKey, setApiKey] = useState('');
  const [showApiKey, setShowApiKey] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<'success' | 'failed' | null>(null);

  // Summary state
  const [sourceAdded, setSourceAdded] = useState(false);
  const [providerSaved, setProviderSaved] = useState(false);

  const handleSelectFolder = useCallback(async () => {
    const selected = await open({
      directory: true,
      multiple: false,
      title: t('welcome.selectFolder'),
    });
    if (selected) {
      const path = typeof selected === 'string' ? selected : selected[0];
      setAddingSource(true);
      try {
        await api.addSource(path, ['**/*'], []);
        setSelectedFolder(path);
        setSourceAdded(true);
        toast.success(t('welcome.folderSelected', { folder: path }));
      } catch {
        toast.error(t('sources.addError'));
      } finally {
        setAddingSource(false);
      }
    }
  }, [t]);

  const handleTestConnection = useCallback(async () => {
    if (!selectedPreset || !apiKey.trim()) return;
    setTesting(true);
    setTestResult(null);
    try {
      const config: SaveAgentConfigInput = {
        id: null,
        name: selectedPreset.name,
        provider: selectedPreset.provider,
        apiKey: apiKey.trim(),
        baseUrl: selectedPreset.baseUrl,
        model: selectedPreset.models.find(m => m.recommended)?.id ?? selectedPreset.models[0].id,
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
        subagentAllowedTools: null,
      };
      const models = await api.testAgentConnection(config);
      if (models && models.length > 0) {
        setTestResult('success');
        // Save the config
        await api.saveAgentConfig(config);
        setProviderSaved(true);
        toast.success(t('welcome.connectionSuccess'));
      } else {
        setTestResult('failed');
        toast.error(t('welcome.connectionFailed'));
      }
    } catch {
      setTestResult('failed');
      toast.error(t('welcome.connectionFailed'));
    } finally {
      setTesting(false);
    }
  }, [selectedPreset, apiKey, t]);

  const topPresets = PROVIDER_PRESETS.filter(p => p.requiresApiKey).slice(0, 6);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-surface-0">
      <div className="w-full max-w-lg mx-auto px-6">
        {/* Step indicator */}
        <div className="flex items-center justify-center gap-2 mb-8">
          {Array.from({ length: TOTAL_STEPS }, (_, i) => (
            <div
              key={i}
              className={`h-1.5 rounded-full transition-all duration-300 ${
                i + 1 === step
                  ? 'w-8 bg-accent'
                  : i + 1 < step
                    ? 'w-8 bg-accent/40'
                    : 'w-8 bg-border'
              }`}
            />
          ))}
        </div>

        <p className="text-center text-xs text-text-tertiary mb-6">
          {t('welcome.step', { current: String(step), total: String(TOTAL_STEPS) })}
        </p>

        <AnimatePresence mode="wait">
          {/* Step 1: Welcome */}
          {step === 1 && (
            <motion.div
              key="step1"
              initial={{ opacity: 0, x: 20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -20 }}
              transition={{ duration: 0.2 }}
              className="flex flex-col items-center text-center"
            >
              <Logo size={72} className="mb-6" />
              <h1 className="text-2xl font-bold text-text-primary mb-2">
                {t('welcome.title')}
              </h1>
              <p className="text-sm text-text-secondary mb-2">
                {t('welcome.subtitle')}
              </p>
              <p className="text-sm text-text-tertiary mb-8 max-w-sm">
                {t('welcome.description')}
              </p>
              <Button size="lg" onClick={() => setStep(2)}>
                {t('welcome.getStarted')}
                <ChevronRight size={16} />
              </Button>
            </motion.div>
          )}

          {/* Step 2: Add Knowledge Source */}
          {step === 2 && (
            <motion.div
              key="step2"
              initial={{ opacity: 0, x: 20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -20 }}
              transition={{ duration: 0.2 }}
              className="flex flex-col items-center text-center"
            >
              <div className="w-14 h-14 rounded-2xl bg-accent/10 flex items-center justify-center mb-6">
                <FolderOpen size={28} className="text-accent" />
              </div>
              <h2 className="text-xl font-semibold text-text-primary mb-2">
                {t('welcome.addSource')}
              </h2>
              <p className="text-sm text-text-tertiary mb-6 max-w-sm">
                {t('welcome.addSourceDescription')}
              </p>

              {selectedFolder ? (
                <div className="flex items-center gap-2 px-4 py-3 rounded-lg bg-success/10 border border-success/20 text-success text-sm mb-6 max-w-full">
                  <CheckCircle size={16} className="shrink-0" />
                  <span className="truncate">{selectedFolder}</span>
                </div>
              ) : (
                <Button
                  size="lg"
                  onClick={handleSelectFolder}
                  loading={addingSource}
                  className="mb-6"
                >
                  <FolderOpen size={16} />
                  {t('welcome.selectFolder')}
                </Button>
              )}

              <div className="flex gap-3">
                <Button variant="ghost" onClick={() => setStep(3)}>
                  {t('welcome.skip')}
                </Button>
                {selectedFolder && (
                  <Button onClick={() => setStep(3)}>
                    {t('welcome.getStarted')}
                    <ChevronRight size={16} />
                  </Button>
                )}
              </div>
            </motion.div>
          )}

          {/* Step 3: Configure AI Provider */}
          {step === 3 && (
            <motion.div
              key="step3"
              initial={{ opacity: 0, x: 20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -20 }}
              transition={{ duration: 0.2 }}
              className="flex flex-col items-center text-center"
            >
              <div className="w-14 h-14 rounded-2xl bg-accent/10 flex items-center justify-center mb-6">
                <Brain size={28} className="text-accent" />
              </div>
              <h2 className="text-xl font-semibold text-text-primary mb-2">
                {t('welcome.configureAI')}
              </h2>
              <p className="text-sm text-text-tertiary mb-6 max-w-sm">
                {t('welcome.configureAIDescription')}
              </p>

              {/* Provider selector */}
              <div className="w-full grid grid-cols-3 gap-2 mb-4">
                {topPresets.map(preset => (
                  <button
                    key={preset.id}
                    onClick={() => { setSelectedPreset(preset); setTestResult(null); }}
                    className={`px-3 py-2.5 rounded-lg border text-sm text-left transition-all ${
                      selectedPreset?.id === preset.id
                        ? 'border-accent bg-accent/10 text-accent'
                        : 'border-border bg-surface-1 text-text-secondary hover:border-border-hover'
                    }`}
                  >
                    <span className="mr-1.5">{preset.icon}</span>
                    {preset.name}
                  </button>
                ))}
              </div>

              {/* API key input */}
              {selectedPreset && selectedPreset.requiresApiKey && (
                <div className="w-full space-y-3 mb-4">
                  <div className="relative">
                    <Input
                      type={showApiKey ? 'text' : 'password'}
                      placeholder={t('welcome.apiKey')}
                      value={apiKey}
                      onChange={e => { setApiKey(e.target.value); setTestResult(null); }}
                      className="pr-10"
                    />
                    <button
                      type="button"
                      onClick={() => setShowApiKey(!showApiKey)}
                      className="absolute right-3 top-1/2 -translate-y-1/2 text-text-tertiary hover:text-text-secondary"
                    >
                      {showApiKey ? <EyeOff size={14} /> : <Eye size={14} />}
                    </button>
                  </div>
                  <Button
                    variant="secondary"
                    size="md"
                    onClick={handleTestConnection}
                    loading={testing}
                    disabled={!apiKey.trim()}
                    className="w-full"
                  >
                    {testing ? (
                      <Loader2 size={14} className="animate-spin" />
                    ) : testResult === 'success' ? (
                      <CheckCircle size={14} className="text-success" />
                    ) : null}
                    {testResult === 'success'
                      ? t('welcome.connectionSuccess')
                      : t('welcome.testConnection')}
                  </Button>
                </div>
              )}

              <div className="flex gap-3 mt-2">
                <Button variant="ghost" onClick={() => setStep(4)}>
                  {t('welcome.skip')}
                </Button>
                {testResult === 'success' && (
                  <Button onClick={() => setStep(4)}>
                    <ChevronRight size={16} />
                  </Button>
                )}
              </div>
            </motion.div>
          )}

          {/* Step 4: All Set */}
          {step === 4 && (
            <motion.div
              key="step4"
              initial={{ opacity: 0, x: 20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -20 }}
              transition={{ duration: 0.2 }}
              className="flex flex-col items-center text-center"
            >
              <div className="w-14 h-14 rounded-2xl bg-success/10 flex items-center justify-center mb-6">
                <Sparkles size={28} className="text-success" />
              </div>
              <h2 className="text-xl font-semibold text-text-primary mb-2">
                {t('welcome.allSet')}
              </h2>
              <p className="text-sm text-text-tertiary mb-6">
                {t('welcome.allSetDescription')}
              </p>

              {/* Summary */}
              <div className="w-full space-y-2 mb-8">
                <div className={`flex items-center gap-2 px-4 py-2.5 rounded-lg text-sm ${
                  sourceAdded ? 'bg-success/10 text-success' : 'bg-surface-2 text-text-tertiary'
                }`}>
                  <FolderOpen size={14} />
                  <span>
                    {sourceAdded
                      ? t('welcome.folderSelected', { folder: selectedFolder ?? '' })
                      : t('welcome.addSource') + ' — ' + t('welcome.skip')}
                  </span>
                </div>
                <div className={`flex items-center gap-2 px-4 py-2.5 rounded-lg text-sm ${
                  providerSaved ? 'bg-success/10 text-success' : 'bg-surface-2 text-text-tertiary'
                }`}>
                  <Brain size={14} />
                  <span>
                    {providerSaved
                      ? selectedPreset?.name + ' — ' + t('welcome.connectionSuccess')
                      : t('welcome.configureAI') + ' — ' + t('welcome.skip')}
                  </span>
                </div>
              </div>

              <Button size="lg" onClick={onComplete}>
                {t('welcome.startUsing')}
                <ChevronRight size={16} />
              </Button>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </div>
  );
}
