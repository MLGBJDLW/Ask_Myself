import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import {
  ClipboardList,
  ListPlus,
  Loader2,
  Play,
  Repeat2,
  Save,
  ShieldCheck,
  Trash2,
} from 'lucide-react';
import { toast } from 'sonner';
import * as api from '../../lib/api';
import type { WorkflowCatalogTemplate } from '../../lib/api';
import {
  buildRecordedWorkflowPrompt,
  hasRecordableWorkflow,
  splitRecordingLines,
  type RecordedWorkflowPromptInput,
  type WorkflowRecordingStep,
  type WorkflowRecordingStepKind,
} from '../../lib/workflowRecorder';
import type { Source } from '../../types';

type WorkflowT = (key: string, params?: Record<string, string | number>) => string;

interface WorkflowRecorderPanelProps {
  templates: WorkflowCatalogTemplate[];
  sources: Source[];
  tr: WorkflowT;
  onReplay: (prompt: string, sourceScope?: string[]) => void;
  onSaved: () => Promise<void> | void;
}

interface RecorderDraftState {
  name: string;
  objective: string;
  context: string;
  variableInputs: string;
  steps: WorkflowRecordingStep[];
  preferences: string;
  successCriteria: string;
  safetyNotes: string;
  replayValues: string;
  sourceScope: string[];
}

const STEP_KIND_OPTIONS: WorkflowRecordingStepKind[] = ['action', 'decision', 'check', 'note'];

function newStep(kind: WorkflowRecordingStepKind = 'action'): WorkflowRecordingStep {
  const id = typeof crypto !== 'undefined' && 'randomUUID' in crypto
    ? crypto.randomUUID()
    : `step-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  return { id, kind, text: '' };
}

function emptyDraft(): RecorderDraftState {
  return {
    name: '',
    objective: '',
    context: '',
    variableInputs: '',
    steps: [newStep()],
    preferences: '',
    successCriteria: '',
    safetyNotes: '',
    replayValues: '',
    sourceScope: [],
  };
}

function textInputClass() {
  return 'h-9 w-full rounded-md border border-border/70 bg-surface-0 px-3 text-sm text-text-primary outline-none transition-colors focus:border-accent/70';
}

function textareaClass(extra = '') {
  return `w-full resize-y rounded-md border border-border/70 bg-surface-0 px-3 py-2 text-sm leading-6 text-text-primary outline-none transition-colors focus:border-accent/70 ${extra}`;
}

function RecorderButton({
  children,
  icon,
  onClick,
  disabled,
  variant = 'secondary',
}: {
  children: ReactNode;
  icon?: ReactNode;
  onClick?: () => void;
  disabled?: boolean;
  variant?: 'primary' | 'secondary' | 'danger';
}) {
  const tone = variant === 'primary'
    ? 'bg-accent text-white hover:bg-accent-hover'
    : variant === 'danger'
      ? 'border-danger/40 text-danger hover:bg-danger/10'
      : 'border-border/70 text-text-secondary hover:bg-surface-2 hover:text-text-primary';
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className={`inline-flex h-9 items-center gap-2 rounded-md border px-3 text-sm font-medium transition-colors disabled:pointer-events-none disabled:opacity-45 ${tone}`}
    >
      {icon}
      {children}
    </button>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  return (
    <label className="block min-w-0">
      <span className="mb-1.5 block text-xs font-medium text-text-tertiary">{label}</span>
      {children}
    </label>
  );
}

function parseDraft(draft: RecorderDraftState): RecordedWorkflowPromptInput {
  return {
    name: draft.name,
    objective: draft.objective,
    context: draft.context,
    variableInputs: splitRecordingLines(draft.variableInputs),
    steps: draft.steps,
    preferences: splitRecordingLines(draft.preferences),
    successCriteria: splitRecordingLines(draft.successCriteria),
    safetyNotes: splitRecordingLines(draft.safetyNotes),
  };
}

function templateLabel(template: WorkflowCatalogTemplate, tr: WorkflowT) {
  const localized = tr(`template.${template.id}.label`);
  return localized.startsWith('workflows.template.') ? template.label : localized;
}

export function WorkflowRecorderPanel({
  templates,
  sources,
  tr,
  onReplay,
  onSaved,
}: WorkflowRecorderPanelProps) {
  const [draft, setDraft] = useState<RecorderDraftState>(() => emptyDraft());
  const [templateId, setTemplateId] = useState('connector_background');
  const [busy, setBusy] = useState<string | null>(null);
  const [savedAutomationId, setSavedAutomationId] = useState<string | null>(null);

  const fallbackTemplateId = useMemo(
    () => templates.find((template) => template.id === 'connector_background')?.id
      ?? templates[0]?.id
      ?? 'connector_background',
    [templates],
  );

  useEffect(() => {
    if (!templates.some((template) => template.id === templateId)) {
      setTemplateId(fallbackTemplateId);
    }
  }, [fallbackTemplateId, templateId, templates]);

  const recording = useMemo(() => parseDraft(draft), [draft]);
  const replayRecording = useMemo<RecordedWorkflowPromptInput>(() => ({
    ...recording,
    replayValues: splitRecordingLines(draft.replayValues),
  }), [draft.replayValues, recording]);
  const canRecord = hasRecordableWorkflow(recording);
  const preview = useMemo(
    () => (canRecord ? buildRecordedWorkflowPrompt(replayRecording) : ''),
    [canRecord, replayRecording],
  );

  const updateStep = useCallback((id: string, patch: Partial<WorkflowRecordingStep>) => {
    setDraft((current) => ({
      ...current,
      steps: current.steps.map((step) => (step.id === id ? { ...step, ...patch } : step)),
    }));
  }, []);

  const removeStep = useCallback((id: string) => {
    setDraft((current) => ({
      ...current,
      steps: current.steps.length > 1
        ? current.steps.filter((step) => step.id !== id)
        : [{ ...current.steps[0], text: '' }],
    }));
  }, []);

  const saveRecording = useCallback(async () => {
    if (!canRecord) {
      toast.error(tr('recordingRequired'));
      return;
    }
    setBusy('save');
    try {
      const saved = await api.saveWorkflowAutomation({
        id: savedAutomationId,
        name: draft.name.trim(),
        description: draft.objective.trim(),
        workflowTemplateId: templateId,
        prompt: buildRecordedWorkflowPrompt(recording),
        trigger: { kind: 'manual' },
        sourceScope: draft.sourceScope,
        approvalPolicy: {
          requireBeforeRun: true,
          allowedTools: [],
          riskLevel: 'medium',
        },
        enabled: true,
      });
      setSavedAutomationId(saved.id);
      await onSaved();
      toast.success(tr('recordingSaved'));
    } catch (error) {
      toast.error(String(error));
    } finally {
      setBusy(null);
    }
  }, [canRecord, draft.name, draft.objective, draft.sourceScope, onSaved, recording, savedAutomationId, templateId, tr]);

  const replayRecordingNow = useCallback(() => {
    if (!canRecord) {
      toast.error(tr('recordingRequired'));
      return;
    }
    onReplay(buildRecordedWorkflowPrompt(replayRecording), draft.sourceScope);
  }, [canRecord, draft.sourceScope, onReplay, replayRecording, tr]);

  return (
    <div className="grid gap-5 xl:grid-cols-[430px_1fr]">
      <section className="rounded-lg border border-border/70 bg-surface-1 p-4">
        <div className="mb-4 flex items-center gap-2 text-sm font-semibold text-text-primary">
          <Repeat2 className="h-4 w-4 text-accent" />
          {tr('recordingTitle')}
        </div>

        <div className="space-y-3">
          <Field label={tr('recordingName')}>
            <input
              className={textInputClass()}
              value={draft.name}
              onChange={(event) => setDraft({ ...draft, name: event.target.value })}
              placeholder={tr('recordingNamePlaceholder')}
            />
          </Field>
          <Field label={tr('recordingObjective')}>
            <textarea
              className={textareaClass('min-h-20')}
              value={draft.objective}
              onChange={(event) => setDraft({ ...draft, objective: event.target.value })}
              placeholder={tr('recordingObjectivePlaceholder')}
            />
          </Field>
          <Field label={tr('recordingContext')}>
            <textarea
              className={textareaClass('min-h-20')}
              value={draft.context}
              onChange={(event) => setDraft({ ...draft, context: event.target.value })}
              placeholder={tr('recordingContextPlaceholder')}
            />
          </Field>

          <div className="grid gap-3 sm:grid-cols-2">
            <Field label={tr('recordingVariables')}>
              <textarea
                className={textareaClass('min-h-28')}
                value={draft.variableInputs}
                onChange={(event) => setDraft({ ...draft, variableInputs: event.target.value })}
                placeholder={tr('recordingVariablesPlaceholder')}
              />
            </Field>
            <Field label={tr('recordingReplayValues')}>
              <textarea
                className={textareaClass('min-h-28')}
                value={draft.replayValues}
                onChange={(event) => setDraft({ ...draft, replayValues: event.target.value })}
                placeholder={tr('recordingReplayValuesPlaceholder')}
              />
            </Field>
          </div>

          <Field label={tr('recordingWorkflowTemplate')}>
            <select
              className={textInputClass()}
              value={templateId}
              onChange={(event) => setTemplateId(event.target.value)}
            >
              {templates.map((template) => (
                <option key={template.id} value={template.id}>{templateLabel(template, tr)}</option>
              ))}
            </select>
          </Field>

          <Field label={tr('recordingSourceScope')}>
            <select
              multiple
              className="min-h-24 w-full rounded-md border border-border/70 bg-surface-0 px-3 py-2 text-sm text-text-primary outline-none focus:border-accent/70"
              value={draft.sourceScope}
              onChange={(event) => {
                const selected = Array.from(event.target.selectedOptions).map((option) => option.value);
                setDraft({ ...draft, sourceScope: selected });
              }}
            >
              {sources.map((source) => (
                <option key={source.id} value={source.id}>{source.rootPath}</option>
              ))}
            </select>
          </Field>
        </div>
      </section>

      <section className="space-y-5">
        <div className="rounded-lg border border-border/70 bg-surface-1 p-4">
          <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
            <div className="flex items-center gap-2 text-sm font-semibold text-text-primary">
              <ClipboardList className="h-4 w-4 text-accent" />
              {tr('recordingSteps')}
            </div>
            <RecorderButton
              onClick={() => setDraft((current) => ({ ...current, steps: [...current.steps, newStep()] }))}
              icon={<ListPlus className="h-4 w-4" />}
            >
              {tr('recordingAddStep')}
            </RecorderButton>
          </div>

          <div className="space-y-2">
            {draft.steps.map((step, index) => (
              <div key={step.id} className="grid gap-2 rounded-md border border-border/60 bg-surface-0 p-2 md:grid-cols-[120px_1fr_44px]">
                <select
                  className={textInputClass()}
                  value={step.kind}
                  aria-label={`${tr('recordingStepKind')} ${index + 1}`}
                  onChange={(event) => updateStep(step.id, { kind: event.target.value as WorkflowRecordingStepKind })}
                >
                  {STEP_KIND_OPTIONS.map((kind) => (
                    <option key={kind} value={kind}>{tr(`recordingStepKind.${kind}`)}</option>
                  ))}
                </select>
                <input
                  className={textInputClass()}
                  value={step.text}
                  onChange={(event) => updateStep(step.id, { text: event.target.value })}
                  placeholder={tr('recordingStepPlaceholder')}
                />
                <button
                  type="button"
                  className="inline-flex h-9 items-center justify-center rounded-md border border-border/70 text-text-tertiary transition-colors hover:border-danger/40 hover:bg-danger/10 hover:text-danger"
                  onClick={() => removeStep(step.id)}
                  aria-label={tr('recordingRemoveStep')}
                >
                  <Trash2 className="h-4 w-4" />
                </button>
              </div>
            ))}
          </div>
        </div>

        <div className="grid gap-3 md:grid-cols-3">
          <Field label={tr('recordingPreferences')}>
            <textarea
              className={textareaClass('min-h-28')}
              value={draft.preferences}
              onChange={(event) => setDraft({ ...draft, preferences: event.target.value })}
              placeholder={tr('recordingPreferencesPlaceholder')}
            />
          </Field>
          <Field label={tr('recordingSuccessCriteria')}>
            <textarea
              className={textareaClass('min-h-28')}
              value={draft.successCriteria}
              onChange={(event) => setDraft({ ...draft, successCriteria: event.target.value })}
              placeholder={tr('recordingSuccessCriteriaPlaceholder')}
            />
          </Field>
          <Field label={tr('recordingSafetyNotes')}>
            <textarea
              className={textareaClass('min-h-28')}
              value={draft.safetyNotes}
              onChange={(event) => setDraft({ ...draft, safetyNotes: event.target.value })}
              placeholder={tr('recordingSafetyNotesPlaceholder')}
            />
          </Field>
        </div>

        <div className="rounded-lg border border-border/70 bg-surface-1 p-4">
          <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
            <div className="flex items-center gap-2 text-sm font-semibold text-text-primary">
              <ShieldCheck className="h-4 w-4 text-accent" />
              {tr('recordingPreview')}
            </div>
            <div className="flex flex-wrap gap-2">
              <RecorderButton
                variant="primary"
                disabled={busy === 'save'}
                onClick={() => void saveRecording()}
                icon={busy === 'save' ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}
              >
                {tr('recordingSave')}
              </RecorderButton>
              <RecorderButton
                disabled={!canRecord}
                onClick={replayRecordingNow}
                icon={<Play className="h-4 w-4" />}
              >
                {tr('recordingReplay')}
              </RecorderButton>
              <RecorderButton onClick={() => {
                setDraft(emptyDraft());
                setSavedAutomationId(null);
              }}>
                {tr('recordingReset')}
              </RecorderButton>
            </div>
          </div>
          {preview ? (
            <pre className="max-h-[420px] overflow-auto whitespace-pre-wrap rounded-md border border-border/60 bg-surface-0 p-3 text-xs leading-5 text-text-secondary">
              {preview}
            </pre>
          ) : (
            <div className="rounded-md border border-border/60 bg-surface-0 p-4 text-sm text-text-tertiary">
              {tr('recordingEmptyPreview')}
            </div>
          )}
        </div>
      </section>
    </div>
  );
}
