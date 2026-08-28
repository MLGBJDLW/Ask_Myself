import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import { NexaSelect } from '../components/ui/overlay';
import { useNavigate } from 'react-router';
import {
  Activity,
  CalendarClock,
  CheckCircle2,
  ClipboardList,
  FileSearch,
  Globe2,
  Loader2,
  PauseCircle,
  Play,
  RefreshCw,
  Repeat2,
  Save,
  ShieldCheck,
  Trash2,
  Workflow,
  XCircle,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { toast } from 'sonner';
import * as api from '../lib/api';
import { useTranslation, type TranslationKey } from '../i18n';
import { WorkflowRecorderPanel } from '../features/workflows/WorkflowRecorderPanel';
import type { AgentConfig, Source } from '../types';
import type { Project } from '../types/project';
import type { WorkflowCatalogTemplate } from '../lib/api';
import { buildWorkflowBatchPrompt } from '../lib/workflowPrompts';
import type {
  BrowserEvidenceCapture,
  LearningGovernanceSnapshot,
  SaveWorkflowAutomationInput,
  WorkflowAutomation,
  WorkflowAutomationTrigger,
  WorkflowAutomationDueRun,
  WorkflowAutomationScheduleConfig,
  WorkflowAutomationRun,
  WorkflowSchedulePreview,
  TaskOrchestratorWorkflowStartOutcome,
} from '../types/workflows';

type Tab = 'templates' | 'automations' | 'recorder' | 'governance' | 'browser';

function detectedTimezone() {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
  } catch {
    return 'UTC';
  }
}

function defaultScheduleConfig(): WorkflowAutomationScheduleConfig {
  return {
    version: 2,
    timezone: detectedTimezone(),
    misfirePolicy: 'run_latest',
    misfireGraceSeconds: 300,
    overlapPolicy: 'skip',
      executionPolicy: {
        projectId: null,
        workspacePolicy: 'deny_writes',
        sourceRootFingerprint: null,
        agentConfigId: null,
      provider: null,
      providerEndpointId: null,
      model: null,
      contextWindow: null,
      powerMode: 'standard',
      orchestrationProfile: 'balanced',
      collaborationMode: 'direct',
      executionMode: null,
    },
    legacyNeedsReview: false,
  };
}

const emptyForm: SaveWorkflowAutomationInput = {
  id: null,
  name: '',
  description: '',
  workflowTemplateId: 'report_brief',
  prompt: '',
  trigger: { kind: 'schedule', cron: '0 9 * * *' },
  sourceScope: [],
  approvalPolicy: {
    requireBeforeRun: true,
    allowedTools: [],
    riskLevel: 'medium',
  },
  scheduleConfig: defaultScheduleConfig(),
  enabled: true,
};

const TAB_ITEMS: Array<{ id: Tab; label: string; icon: LucideIcon }> = [
  { id: 'templates', label: 'templates', icon: Workflow },
  { id: 'automations', label: 'automations', icon: CalendarClock },
  { id: 'recorder', label: 'recordReplay', icon: Repeat2 },
  { id: 'governance', label: 'governance', icon: Activity },
  { id: 'browser', label: 'browserEvidence', icon: Globe2 },
];

function formatTime(value?: string | null) {
  if (!value) return '-';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString([], {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function workflowKey(key: string): TranslationKey {
  return `workflows.${key}` as TranslationKey;
}

type WorkflowT = (key: string, params?: Record<string, string | number>) => string;

function triggerLabel(trigger: WorkflowAutomationTrigger, tr: WorkflowT) {
  if (trigger.kind === 'schedule') return `${tr('schedule')}: ${trigger.cron}`;
  if (trigger.kind === 'folder') return `${tr('folder')}: ${trigger.path || '-'} ${trigger.pattern || ''}`;
  return tr('manual');
}

function templateText(template: WorkflowCatalogTemplate, field: 'label' | 'description' | 'prompt', tr: WorkflowT) {
  return tr(`template.${template.id}.${field}`);
}

function taskRoleLabel(roleId: string, fallback: string, tr: WorkflowT) {
  const value = tr(`role.${roleId}`);
  return value.startsWith('workflows.role.') ? fallback : value;
}

function dueReasonLabel(item: WorkflowAutomationDueRun, tr: WorkflowT) {
  if (item.origin === 'manual_run_now') return tr('manual');
  if (item.automation.trigger.kind === 'folder') return tr('folderDueReason');
  return triggerLabel(item.automation.trigger, tr);
}

function recommendationLabel(value: string, tr: WorkflowT) {
  const failed = value.match(/^Review (\d+) skill\(s\) with recent failure evidence before broad reuse\.$/);
  if (failed) return tr('recommendationReviewFailedSkills', { count: failed[1] });
  const stale = value.match(/^Consider disabling or rewriting (\d+) enabled skill\(s\) with no recorded usage\.$/);
  if (stale) return tr('recommendationDisableStaleSkills', { count: stale[1] });
  const proposals = value.match(/^Review (\d+) pending skill proposal\(s\) before they affect future tasks\.$/);
  if (proposals) return tr('recommendationReviewPendingProposals', { count: proposals[1] });
  return value;
}

function Button({
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

function textInputClass() {
  return 'h-9 w-full rounded-md border border-border/70 bg-surface-0 px-3 text-sm text-text-primary outline-none transition-colors focus:border-accent/70';
}

function textareaClass() {
  return 'min-h-24 w-full resize-y rounded-md border border-border/70 bg-surface-0 px-3 py-2 text-sm leading-6 text-text-primary outline-none transition-colors focus:border-accent/70';
}

export function WorkflowsPage() {
  const { t } = useTranslation();
  const tr = useCallback((key: string, params?: Record<string, string | number>) => t(workflowKey(key), params), [t]);
  const navigate = useNavigate();
  const [tab, setTab] = useState<Tab>('templates');
  const [loading, setLoading] = useState(true);
  const [templates, setTemplates] = useState<WorkflowCatalogTemplate[]>([]);
  const [automations, setAutomations] = useState<WorkflowAutomation[]>([]);
  const [dueRuns, setDueRuns] = useState<WorkflowAutomationDueRun[]>([]);
  const [approvalRuns, setApprovalRuns] = useState<WorkflowAutomationRun[]>([]);
  const [sources, setSources] = useState<Source[]>([]);
  const [agentConfigs, setAgentConfigs] = useState<AgentConfig[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [governance, setGovernance] = useState<LearningGovernanceSnapshot | null>(null);
  const [form, setForm] = useState<SaveWorkflowAutomationInput>(emptyForm);
  const [browserUrl, setBrowserUrl] = useState('');
  const [browserMode, setBrowserMode] = useState('auto');
  const [capture, setCapture] = useState<BrowserEvidenceCapture | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [schedulePreview, setSchedulePreview] = useState<WorkflowSchedulePreview | null>(null);
  const [schedulePreviewError, setSchedulePreviewError] = useState<string | null>(null);

  const selectedTemplate = useMemo(
    () => templates.find((template) => template.id === form.workflowTemplateId) ?? templates[0],
    [form.workflowTemplateId, templates],
  );
  const scheduleConfig = form.scheduleConfig ?? defaultScheduleConfig();
  const approvalRunsByAutomation = useMemo(() => {
    const grouped = new Map<string, WorkflowAutomationRun[]>();
    for (const run of approvalRuns) {
      const existing = grouped.get(run.automationId) ?? [];
      existing.push(run);
      grouped.set(run.automationId, existing);
    }
    for (const runs of grouped.values()) {
      runs.sort((left, right) => (left.scheduledFor ?? '').localeCompare(right.scheduledFor ?? '') || left.id.localeCompare(right.id));
    }
    return grouped;
  }, [approvalRuns]);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [templateItems, automationItems, dueItems, approvalItems, sourceItems, governanceSnapshot, configItems, projectItems] = await Promise.all([
        api.listWorkflowTemplates(),
        api.listWorkflowAutomations(),
        api.listDueWorkflowAutomations(),
        api.listWorkflowAutomationApprovals(),
        api.listSources(),
        api.getLearningGovernanceSnapshot(),
        api.listAgentConfigs(),
        api.listProjects(),
      ]);
      setTemplates(templateItems);
      setAutomations(automationItems);
      setDueRuns(dueItems);
      setApprovalRuns(approvalItems);
      setSources(sourceItems);
      setGovernance(governanceSnapshot);
      setAgentConfigs(configItems);
      setProjects(projectItems);
      setForm((current) => ({
        ...current,
        workflowTemplateId: current.workflowTemplateId || templateItems[0]?.id || 'report_brief',
      }));
    } catch (error) {
      toast.error(String(error));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (form.trigger.kind !== 'schedule') {
      setSchedulePreview(null);
      setSchedulePreviewError(null);
      return;
    }
    const scheduleConfig = form.scheduleConfig ?? defaultScheduleConfig();
    const timer = window.setTimeout(() => {
      void api.previewWorkflowAutomationSchedule(
        form.trigger.kind === 'schedule' ? form.trigger.cron : '',
        scheduleConfig.timezone,
      ).then((preview) => {
        setSchedulePreview(preview);
        setSchedulePreviewError(null);
      }).catch((error) => {
        setSchedulePreview(null);
        setSchedulePreviewError(String(error));
      });
    }, 250);
    return () => window.clearTimeout(timer);
  }, [form.scheduleConfig, form.trigger]);

  const runPrompt = useCallback((
    prompt: string,
    sourceScope?: string[],
    taskOrchestratorRunId?: string | null,
  ) => {
    navigate('/chat', {
      state: {
        initialMessage: prompt,
        sourceIds: sourceScope ?? [],
        taskOrchestratorRunId: taskOrchestratorRunId ?? null,
      },
    });
  }, [navigate]);

  const runTemplate = useCallback((template: WorkflowCatalogTemplate) => {
    runPrompt(buildWorkflowBatchPrompt(template, templateText(template, 'prompt', tr)));
  }, [runPrompt, tr]);

  const runAutomation = useCallback(async (automation: WorkflowAutomation) => {
    setBusy(automation.id);
    try {
      const outcome = await api.startWorkflowAutomationRun(
        automation.id,
        null,
        tr('queuedFromWorkbench'),
      );
      if (outcome.status === 'launched') {
        navigate(`/chat/${outcome.launch.conversationId}`);
      } else if (outcome.status === 'pending_approval') {
        toast.success(tr('pendingApproval'));
        await load();
      } else {
        toast.error(outcome.reason);
        await load();
      }
    } catch (error) {
      toast.error(String(error));
    } finally {
      setBusy(null);
    }
  }, [load, navigate, tr]);

  const runDueAutomation = useCallback(async (item: WorkflowAutomationDueRun) => {
    setBusy(item.automation.id);
    try {
      const outcome: TaskOrchestratorWorkflowStartOutcome =
        await api.startDueWorkflowAutomationRun(item.automation.id);
      if (outcome.status === 'launched') {
        navigate(`/chat/${outcome.launch.conversationId}`);
      } else if (outcome.status === 'pending_approval') {
        toast.success(tr('pendingApproval'));
        await load();
      } else {
        toast.error(outcome.reason);
        await load();
      }
    } catch (error) {
      toast.error(String(error));
    } finally {
      setBusy(null);
    }
  }, [load, navigate, tr]);

  const approveAutomationRun = useCallback(async (run: WorkflowAutomationRun) => {
    setBusy(run.id);
    try {
      const launch = await api.approveWorkflowAutomationRun(run.id);
      toast.success(tr('approvalQueued'));
      navigate(`/chat/${launch.conversationId}`);
    } catch (error) {
      toast.error(String(error));
      await load();
    } finally {
      setBusy(null);
    }
  }, [load, navigate, tr]);

  const denyAutomationRun = useCallback(async (run: WorkflowAutomationRun) => {
    setBusy(run.id);
    try {
      await api.denyWorkflowAutomationRun(run.id);
      toast.success(tr('approvalDenied'));
      await load();
    } catch (error) {
      toast.error(String(error));
    } finally {
      setBusy(null);
    }
  }, [load, tr]);

  const editAutomation = useCallback((automation: WorkflowAutomation) => {
    setForm({
      id: automation.id,
      name: automation.name,
      description: automation.description,
      workflowTemplateId: automation.workflowTemplateId,
      prompt: automation.prompt,
      trigger: automation.trigger,
      sourceScope: automation.sourceScope,
      approvalPolicy: automation.approvalPolicy,
      scheduleConfig: {
        ...automation.scheduleConfig,
        version: 2,
        legacyNeedsReview: false,
      },
      enabled: automation.enabled,
    });
    setTab('automations');
  }, []);

  const updateTriggerKind = (kind: WorkflowAutomationTrigger['kind']) => {
    setForm((current) => ({
      ...current,
      trigger: kind === 'schedule'
        ? { kind: 'schedule', cron: '0 9 * * *' }
        : kind === 'folder'
          ? { kind: 'folder', path: '', pattern: '*.pdf' }
          : { kind: 'manual' },
    }));
  };

  const saveAutomation = async () => {
    const effectivePrompt = (form.prompt || (selectedTemplate ? templateText(selectedTemplate, 'prompt', tr) : '')).trim();
    if (!form.name.trim() || !effectivePrompt) {
      toast.error(tr('namePromptRequired'));
      return;
    }
    setBusy('save');
    try {
      if (form.trigger.kind === 'schedule') {
        const scheduleConfig = form.scheduleConfig ?? defaultScheduleConfig();
        await api.previewWorkflowAutomationSchedule(
          form.trigger.cron,
          scheduleConfig.timezone,
        );
      }
      await api.saveWorkflowAutomation({ ...form, prompt: effectivePrompt });
      setForm(emptyForm);
      await load();
      toast.success(tr('saveAutomation'));
    } catch (error) {
      toast.error(String(error));
    } finally {
      setBusy(null);
    }
  };

  const toggleAutomation = async (automation: WorkflowAutomation) => {
    setBusy(automation.id);
    try {
      await api.setWorkflowAutomationEnabled(automation.id, !automation.enabled);
      await load();
    } catch (error) {
      toast.error(String(error));
    } finally {
      setBusy(null);
    }
  };

  const deleteAutomation = async (automation: WorkflowAutomation) => {
    setBusy(automation.id);
    try {
      await api.deleteWorkflowAutomation(automation.id);
      await load();
    } catch (error) {
      toast.error(String(error));
    } finally {
      setBusy(null);
    }
  };

  const captureBrowser = async () => {
    const trimmed = browserUrl.trim();
    if (!trimmed) return;
    setBusy('browser');
    try {
      const result = await api.captureBrowserEvidence(trimmed, 6000, browserMode);
      setCapture(result);
      toast.success(tr('captured'));
    } catch (error) {
      toast.error(String(error));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="flex min-h-full flex-col bg-surface-0" data-theme-surface="page">
      <header className="border-b border-border bg-surface-1 px-6 py-5">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div>
            <h1 className="text-xl font-semibold tracking-normal text-text-primary">{tr('title')}</h1>
            <p className="mt-1 max-w-3xl text-sm leading-6 text-text-secondary">{tr('subtitle')}</p>
          </div>
          <Button onClick={() => void load()} icon={loading ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}>
            {tr('refresh')}
          </Button>
        </div>
        <div className="mt-4 flex flex-wrap gap-1 rounded-md border border-border/70 bg-surface-0 p-1">
          {TAB_ITEMS.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              type="button"
              onClick={() => setTab(id)}
              className={`inline-flex h-9 items-center gap-2 rounded-md px-3 text-sm transition-colors ${
                tab === id ? 'bg-accent-subtle text-accent' : 'text-text-secondary hover:bg-surface-2 hover:text-text-primary'
              }`}
            >
              <Icon className="h-4 w-4" />
              {tr(label)}
            </button>
          ))}
        </div>
      </header>

      {loading ? (
        <div className="flex flex-1 items-center justify-center text-sm text-text-tertiary">{tr('loading')}</div>
      ) : (
        <main className="flex-1 space-y-6 p-6">
          {tab === 'templates' && (
            <div className="grid gap-4 xl:grid-cols-[1fr_360px]">
              <section className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
                {templates.map((template) => (
                  <article key={template.id} className="rounded-lg border border-border/70 bg-surface-1 p-4">
                    <div className="flex items-start justify-between gap-3">
                      <div className="min-w-0">
                        <h2 className="text-sm font-semibold text-text-primary">{templateText(template, 'label', tr)}</h2>
                        <p className="mt-1 line-clamp-3 text-xs leading-5 text-text-secondary">{templateText(template, 'description', tr)}</p>
                      </div>
                      <Button variant="primary" onClick={() => runTemplate(template)} icon={<Play className="h-4 w-4" />}>
                        {tr('run')}
                      </Button>
                    </div>
                    <div className="mt-3 flex flex-wrap gap-1.5">
                      {template.tasks.map((task) => (
                        <span key={task.id} className="rounded-md border border-border/60 bg-surface-0 px-2 py-1 text-[11px] text-text-tertiary">
                          {taskRoleLabel(task.roleId, task.roleLabel, tr)}
                        </span>
                      ))}
                    </div>
                  </article>
                ))}
              </section>
              <aside className="rounded-lg border border-border/70 bg-surface-1 p-4">
                <div className="flex items-center gap-2 text-sm font-semibold text-text-primary">
                  <CalendarClock className="h-4 w-4 text-accent" />
                  {tr('dueNow')}
                </div>
                <div className="mt-3 space-y-2">
                  {dueRuns.length === 0 ? (
                    <p className="text-sm text-text-tertiary">{tr('noDue')}</p>
                  ) : dueRuns.map((item) => (
                    <button
                      key={item.automation.id}
                      type="button"
                      onClick={() => void runDueAutomation(item)}
                      className="block w-full rounded-md border border-border/70 bg-surface-0 p-3 text-left transition-colors hover:border-accent/60 hover:bg-accent-subtle/30"
                    >
                      <div className="text-sm font-medium text-text-primary">{item.automation.name}</div>
                      <div className="mt-1 text-xs text-text-tertiary">{dueReasonLabel(item, tr)}</div>
                    </button>
                  ))}
                </div>
              </aside>
            </div>
          )}

          {tab === 'automations' && (
            <div className="grid gap-5 xl:grid-cols-[430px_1fr]">
              <section className="rounded-lg border border-border/70 bg-surface-1 p-4">
                <div className="mb-4 flex items-center gap-2 text-sm font-semibold text-text-primary">
                  <Save className="h-4 w-4 text-accent" />
                  {form.id ? tr('updateAutomation') : tr('createAutomation')}
                </div>
                <div className="space-y-3">
                  <Field label={tr('name')}>
                    <input className={textInputClass()} value={form.name} onChange={(event) => setForm({ ...form, name: event.target.value })} />
                  </Field>
                  <Field label={tr('description')}>
                    <input className={textInputClass()} value={form.description} onChange={(event) => setForm({ ...form, description: event.target.value })} />
                  </Field>
                  <Field label={tr('templates')}>
                    <NexaSelect
                      className={textInputClass()}
                      value={form.workflowTemplateId}
                      onChange={(event) => {
                        const nextTemplate = templates.find((template) => template.id === event.target.value);
                        setForm({
                          ...form,
                          workflowTemplateId: event.target.value,
                          prompt: nextTemplate ? templateText(nextTemplate, 'prompt', tr) : form.prompt,
                        });
                      }}
                    >
                      {templates.map((template) => (
                        <option key={template.id} value={template.id}>{templateText(template, 'label', tr)}</option>
                      ))}
                    </NexaSelect>
                  </Field>
                  <Field label={tr('prompt')}>
                    <textarea className={textareaClass()} value={form.prompt || (selectedTemplate ? templateText(selectedTemplate, 'prompt', tr) : '')} onChange={(event) => setForm({ ...form, prompt: event.target.value })} />
                  </Field>
                  <Field label={tr('trigger')}>
                    <div className="grid grid-cols-3 gap-1 rounded-md border border-border/70 bg-surface-0 p-1">
                      {[
                        ['schedule', tr('schedule')],
                        ['folder', tr('folder')],
                        ['manual', tr('manual')],
                      ].map(([id, label]) => (
                        <button
                          key={id}
                          type="button"
                          aria-label={label}
                          onClick={() => updateTriggerKind(id as WorkflowAutomationTrigger['kind'])}
                          className={`h-8 rounded-md text-xs transition-colors ${form.trigger.kind === id ? 'bg-accent-subtle text-accent' : 'text-text-secondary hover:bg-surface-2'}`}
                        >
                          {label}
                        </button>
                      ))}
                    </div>
                  </Field>
                  {form.trigger.kind === 'schedule' && (
                    <div className="space-y-3 rounded-md border border-border/70 bg-surface-0/50 p-3">
                      <div className="grid gap-3 sm:grid-cols-2">
                        <Field label={tr('cron')}>
                          <input className={textInputClass()} value={form.trigger.cron} onChange={(event) => setForm({ ...form, trigger: { kind: 'schedule', cron: event.target.value } })} />
                        </Field>
                        <Field label={tr('timezone')}>
                          <input
                            className={textInputClass()}
                            value={scheduleConfig.timezone}
                            onChange={(event) => setForm({
                              ...form,
                              scheduleConfig: { ...scheduleConfig, timezone: event.target.value },
                            })}
                            placeholder="Asia/Shanghai"
                          />
                        </Field>
                      </div>
                      <p className="text-[11px] leading-5 text-text-tertiary">{tr('cronHelp')}</p>
                      {schedulePreviewError ? (
                        <div className="rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-xs text-danger">
                          {schedulePreviewError}
                        </div>
                      ) : schedulePreview && (
                        <div className="rounded-md border border-border/60 bg-surface-1 px-3 py-2">
                          <div className="text-[11px] font-medium uppercase tracking-[0.12em] text-text-tertiary">{tr('nextOccurrences')}</div>
                          <ol className="mt-1 grid gap-1 text-xs text-text-secondary sm:grid-cols-2">
                            {schedulePreview.occurrences.map((occurrence) => (
                              <li key={occurrence.scheduledFor}>{occurrence.localTime}</li>
                            ))}
                          </ol>
                        </div>
                      )}
                      <div className="grid gap-3 sm:grid-cols-2">
                        <Field label={tr('misfirePolicy')}>
                          <NexaSelect className={textInputClass()} value={scheduleConfig.misfirePolicy} onChange={(event) => setForm({
                            ...form,
                            scheduleConfig: { ...scheduleConfig, misfirePolicy: event.target.value as WorkflowAutomationScheduleConfig['misfirePolicy'] },
                          })}>
                            <option value="run_latest">{tr('misfireRunLatest')}</option>
                            <option value="skip">{tr('misfireSkip')}</option>
                          </NexaSelect>
                        </Field>
                        <Field label={tr('overlapPolicy')}>
                          <NexaSelect className={textInputClass()} value={scheduleConfig.overlapPolicy} onChange={(event) => setForm({
                            ...form,
                            scheduleConfig: { ...scheduleConfig, overlapPolicy: event.target.value as WorkflowAutomationScheduleConfig['overlapPolicy'] },
                          })}>
                            <option value="skip">{tr('overlapSkip')}</option>
                            <option value="allow">{tr('overlapAllow')}</option>
                          </NexaSelect>
                        </Field>
                      </div>
                      <Field label={tr('workspacePolicy')}>
                        <NexaSelect className={textInputClass()} value={scheduleConfig.executionPolicy.workspacePolicy} onChange={(event) => {
                          const isolated = event.target.value === 'isolated_patch';
                          setForm({
                            ...form,
                            scheduleConfig: {
                              ...scheduleConfig,
                              overlapPolicy: isolated ? 'skip' : scheduleConfig.overlapPolicy,
                              executionPolicy: {
                                ...scheduleConfig.executionPolicy,
                                workspacePolicy: isolated ? 'isolated_patch' : 'deny_writes',
                                sourceRootFingerprint: null,
                                orchestrationProfile: isolated ? 'codeUltra' : scheduleConfig.executionPolicy.orchestrationProfile,
                                executionMode: isolated ? null : scheduleConfig.executionPolicy.executionMode,
                              },
                            },
                          });
                        }}>
                          <option value="deny_writes">{tr('workspaceDenyWrites')}</option>
                          <option value="isolated_patch">{tr('workspaceIsolatedPatch')}</option>
                        </NexaSelect>
                      </Field>
                      {scheduleConfig.executionPolicy.workspacePolicy === 'isolated_patch' && (
                        <div className="rounded-md border border-accent/25 bg-accent/8 px-3 py-2 text-xs leading-5 text-text-secondary">
                          {tr('workspaceIsolatedHelp')}
                        </div>
                      )}
                      <Field label={tr('project')}>
                        <NexaSelect className={textInputClass()} value={scheduleConfig.executionPolicy.projectId ?? ''} onChange={(event) => setForm({
                          ...form,
                          scheduleConfig: {
                            ...scheduleConfig,
                            executionPolicy: {
                              ...scheduleConfig.executionPolicy,
                              projectId: event.target.value || null,
                            },
                          },
                        })}>
                          <option value="">{tr('projectNone')}</option>
                          {projects.map((project) => <option key={project.id} value={project.id}>{project.name}</option>)}
                        </NexaSelect>
                      </Field>
                      <Field label={tr('agentConfig')}>
                        <NexaSelect className={textInputClass()} value={scheduleConfig.executionPolicy.agentConfigId ?? ''} onChange={(event) => {
                          const selected = agentConfigs.find((config) => config.id === event.target.value);
                          setForm({
                            ...form,
                            scheduleConfig: {
                              ...scheduleConfig,
                              executionPolicy: {
                                ...scheduleConfig.executionPolicy,
                                agentConfigId: selected?.id ?? null,
                                provider: selected?.provider ?? null,
                                providerEndpointId: selected?.providerEndpointId ?? null,
                                model: selected?.model ?? null,
                                contextWindow: null,
                              },
                            },
                          });
                        }}>
                          <option value="">{tr('agentConfigDefault')}</option>
                          {agentConfigs.map((config) => <option key={config.id} value={config.id}>{config.name} · {config.model}</option>)}
                        </NexaSelect>
                      </Field>
                      <div className="grid gap-3 sm:grid-cols-2">
                        <Field label={tr('model')}>
                          <input className={textInputClass()} value={scheduleConfig.executionPolicy.model ?? ''} onChange={(event) => setForm({
                            ...form,
                            scheduleConfig: { ...scheduleConfig, executionPolicy: { ...scheduleConfig.executionPolicy, model: event.target.value || null } },
                          })} placeholder={tr('providerDefault')} />
                        </Field>
                        <Field label={tr('contextWindow')}>
                          <input type="number" min={1024} className={textInputClass()} value={scheduleConfig.executionPolicy.contextWindow ?? ''} onChange={(event) => setForm({
                            ...form,
                            scheduleConfig: {
                              ...scheduleConfig,
                              executionPolicy: { ...scheduleConfig.executionPolicy, contextWindow: event.target.value ? Number(event.target.value) : null },
                            },
                          })} placeholder={tr('providerDefault')} />
                        </Field>
                      </div>
                      <div className="grid gap-3 sm:grid-cols-2">
                        <Field label={tr('powerMode')}>
                          <NexaSelect className={textInputClass()} value={scheduleConfig.executionPolicy.powerMode} onChange={(event) => setForm({
                            ...form,
                            scheduleConfig: { ...scheduleConfig, executionPolicy: { ...scheduleConfig.executionPolicy, powerMode: event.target.value as 'standard' | 'nexus' } },
                          })}>
                            <option value="standard">Standard</option>
                            <option value="nexus">Nexus</option>
                          </NexaSelect>
                        </Field>
                        <Field label={tr('orchestrationProfile')}>
                          <NexaSelect className={textInputClass()} value={scheduleConfig.executionPolicy.orchestrationProfile} onChange={(event) => setForm({
                            ...form,
                            scheduleConfig: { ...scheduleConfig, executionPolicy: { ...scheduleConfig.executionPolicy, orchestrationProfile: event.target.value } },
                          })}>
                            <option value="balanced">Balanced</option>
                            <option value="deep">Deep</option>
                            <option value="codeUltra">Code Ultra</option>
                            <option value="researchUltra">Research Ultra</option>
                          </NexaSelect>
                        </Field>
                      </div>
                    </div>
                  )}
                  {form.trigger.kind === 'folder' && (
                    <div className="grid gap-3 sm:grid-cols-2">
                      <Field label={tr('folderPath')}>
                        <input className={textInputClass()} value={form.trigger.path} onChange={(event) => setForm({ ...form, trigger: { ...(form.trigger as Extract<WorkflowAutomationTrigger, { kind: 'folder' }>), path: event.target.value } })} />
                      </Field>
                      <Field label={tr('filePattern')}>
                        <input className={textInputClass()} value={form.trigger.pattern} onChange={(event) => setForm({ ...form, trigger: { ...(form.trigger as Extract<WorkflowAutomationTrigger, { kind: 'folder' }>), pattern: event.target.value } })} />
                      </Field>
                    </div>
                  )}
                  <Field label={tr('sourceScope')}>
                    <NexaSelect
                      multiple
                      className="min-h-28 w-full rounded-md border border-border/70 bg-surface-0 px-3 py-2 text-sm text-text-primary outline-none focus:border-accent/70"
                      value={form.sourceScope}
                      onChange={(event) => {
                        const selected = Array.from(event.target.selectedOptions).map((option) => option.value);
                        setForm({ ...form, sourceScope: selected });
                      }}
                    >
                      {sources.map((source) => (
                        <option key={source.id} value={source.id}>{source.rootPath}</option>
                      ))}
                    </NexaSelect>
                  </Field>
                  <div className="grid gap-3 sm:grid-cols-2">
                    <Field label={tr('riskLevel')}>
                      <NexaSelect className={textInputClass()} value={form.approvalPolicy.riskLevel} onChange={(event) => setForm({ ...form, approvalPolicy: { ...form.approvalPolicy, riskLevel: event.target.value } })}>
                        <option value="low">{tr('riskLow')}</option>
                        <option value="medium">{tr('riskMedium')}</option>
                        <option value="high">{tr('riskHigh')}</option>
                      </NexaSelect>
                    </Field>
                    <label className="mt-6 inline-flex h-9 items-center gap-2 rounded-md border border-border/70 bg-surface-0 px-3 text-sm text-text-secondary">
                      <input
                        type="checkbox"
                        checked={form.approvalPolicy.requireBeforeRun}
                        onChange={(event) => setForm({ ...form, approvalPolicy: { ...form.approvalPolicy, requireBeforeRun: event.target.checked } })}
                      />
                      {tr('approvalRequired')}
                    </label>
                  </div>
                  {form.trigger.kind === 'schedule' && form.approvalPolicy.requireBeforeRun && (
                    <div className="rounded-md border border-warning/30 bg-warning/10 px-3 py-2 text-xs leading-5 text-warning">
                      {tr('scheduledApprovalWarning')}
                    </div>
                  )}
                  <Field label={tr('allowedTools')}>
                    <input
                      className={textInputClass()}
                      value={form.approvalPolicy.allowedTools.join(', ')}
                      onChange={(event) => setForm({
                        ...form,
                        approvalPolicy: {
                          ...form.approvalPolicy,
                          allowedTools: event.target.value.split(',').map((item) => item.trim()).filter(Boolean),
                        },
                      })}
                    />
                  </Field>
                  <div className="flex flex-wrap gap-2">
                    <Button variant="primary" disabled={busy === 'save'} onClick={() => void saveAutomation()} icon={busy === 'save' ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}>
                      {tr('saveAutomation')}
                    </Button>
                    <Button onClick={() => setForm(emptyForm)}>{tr('createAutomation')}</Button>
                  </div>
                </div>
              </section>

              <section className="space-y-3">
                {automations.map((automation) => {
                  const pendingApprovals = approvalRunsByAutomation.get(automation.id) ?? [];
                  return (
                  <article key={automation.id} className="rounded-lg border border-border/70 bg-surface-1 p-4">
                    <div className="flex flex-wrap items-start justify-between gap-3">
                      <div className="min-w-0">
                        <div className="flex flex-wrap items-center gap-2">
                          <h2 className="text-sm font-semibold text-text-primary">{automation.name}</h2>
                          <span className={`rounded-md border px-2 py-0.5 text-[11px] ${automation.enabled ? 'border-success/30 bg-success/10 text-success' : 'border-border/70 bg-surface-0 text-text-tertiary'}`}>
                            {automation.enabled ? tr('enabled') : tr('disabled')}
                          </span>
                          <span className="rounded-md border border-border/70 bg-surface-0 px-2 py-0.5 text-[11px] text-text-tertiary">
                            {triggerLabel(automation.trigger, tr)}
                          </span>
                        </div>
                        <p className="mt-1 line-clamp-2 text-xs leading-5 text-text-secondary">{automation.description || automation.prompt}</p>
                        <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-[11px] text-text-tertiary">
                          <span>{tr('nextRun')}: {formatTime(automation.nextRunAt)}</span>
                          <span>{tr('lastRun')}: {formatTime(automation.lastRunAt)}</span>
                          {automation.trigger.kind === 'schedule' && <span>{automation.scheduleConfig.timezone}</span>}
                          {automation.scheduleConfig.legacyNeedsReview && <span className="text-warning">{tr('legacyNeedsReview')}</span>}
                          <span>{automation.workflowTemplateId}</span>
                        </div>
                      </div>
                      <div className="flex flex-wrap gap-2">
                        <Button disabled={busy === automation.id || pendingApprovals.length > 0} onClick={() => void runAutomation(automation)} icon={<Play className="h-4 w-4" />}>{tr('run')}</Button>
                        <Button onClick={() => editAutomation(automation)} icon={<ClipboardList className="h-4 w-4" />}>{tr('updateAutomation')}</Button>
                        <Button disabled={busy === automation.id} onClick={() => void toggleAutomation(automation)} icon={automation.enabled ? <PauseCircle className="h-4 w-4" /> : <CheckCircle2 className="h-4 w-4" />}>
                          {automation.enabled ? tr('disabled') : tr('enabled')}
                        </Button>
                        <Button variant="danger" disabled={busy === automation.id} onClick={() => void deleteAutomation(automation)} icon={<Trash2 className="h-4 w-4" />}>{tr('delete')}</Button>
                      </div>
                    </div>
                    {pendingApprovals.length > 0 && (
                      <div className="mt-3 space-y-2">
                        {pendingApprovals.map((pendingApproval) => (
                          <div key={pendingApproval.id} className="flex flex-wrap items-center justify-between gap-3 rounded-md border border-warning/35 bg-warning/10 px-3 py-2">
                            <div className="text-xs leading-5 text-warning">
                              <div className="font-semibold">{tr('pendingApproval')}</div>
                              <div>{formatTime(pendingApproval.scheduledFor)} · rev {pendingApproval.definitionRevision} · {tr('approvalAttempt', { count: pendingApproval.attempt })}</div>
                            </div>
                            <div className="flex gap-2">
                              <Button variant="primary" disabled={busy === pendingApproval.id} onClick={() => void approveAutomationRun(pendingApproval)} icon={<ShieldCheck className="h-4 w-4" />}>
                                {tr('approve')}
                              </Button>
                              <Button variant="danger" disabled={busy === pendingApproval.id} onClick={() => void denyAutomationRun(pendingApproval)} icon={<XCircle className="h-4 w-4" />}>
                                {tr('deny')}
                              </Button>
                            </div>
                          </div>
                        ))}
                      </div>
                    )}
                  </article>
                  );
                })}
              </section>
            </div>
          )}

          {tab === 'recorder' && (
            <WorkflowRecorderPanel
              templates={templates}
              sources={sources}
              tr={tr}
              onReplay={runPrompt}
              onSaved={load}
            />
          )}

          {tab === 'governance' && governance && (
            <div className="grid gap-5 xl:grid-cols-[320px_1fr]">
              <section className="space-y-3">
                {([
                  { label: tr('pendingProposals'), value: governance.pendingProposals, icon: ShieldCheck },
                  { label: tr('proceduralMemory'), value: governance.proceduralMemoryCount, icon: FileSearch },
                  { label: tr('memoryInjections'), value: governance.memoryInjectionCount, icon: Activity },
                ] satisfies Array<{ label: string; value: number; icon: LucideIcon }>).map(({ label, value, icon: Icon }) => (
                  <div key={label} className="rounded-lg border border-border/70 bg-surface-1 p-4">
                    <div className="flex items-center gap-2 text-xs font-medium text-text-tertiary">
                      <Icon className="h-4 w-4 text-accent" />
                      {label}
                    </div>
                    <div className="mt-2 text-2xl font-semibold text-text-primary">{String(value)}</div>
                  </div>
                ))}
                {governance.recommendations.length > 0 && (
                  <div className="rounded-lg border border-warning/30 bg-warning/10 p-4">
                    <div className="text-sm font-semibold text-warning">{tr('recommendations')}</div>
                    <ul className="mt-2 space-y-2 text-sm leading-5 text-text-secondary">
                      {governance.recommendations.map((item) => <li key={item}>{recommendationLabel(item, tr)}</li>)}
                    </ul>
                  </div>
                )}
              </section>
              <section className="rounded-lg border border-border/70 bg-surface-1">
                {governance.skillStats.length === 0 ? (
                  <div className="p-6 text-sm text-text-tertiary">{tr('noStats')}</div>
                ) : (
                  <div className="divide-y divide-border/60">
                    {governance.skillStats.map((skill) => (
                      <div key={skill.skillId} className="grid gap-3 p-4 md:grid-cols-[1fr_220px]">
                        <div className="min-w-0">
                          <div className="flex items-center gap-2">
                            <h2 className="text-sm font-semibold text-text-primary">{skill.name}</h2>
                            {!skill.enabled && <span className="text-xs text-text-tertiary">{tr('disabled')}</span>}
                            {skill.disableRecommended && <XCircle className="h-4 w-4 text-warning" />}
                          </div>
                          {skill.recentFailureEvidence != null && (
                            <pre className="mt-2 max-h-28 overflow-auto rounded-md bg-surface-0 p-2 text-xs leading-5 text-text-secondary">
                              {JSON.stringify(skill.recentFailureEvidence, null, 2)}
                            </pre>
                          )}
                        </div>
                        <div className="grid grid-cols-3 gap-2 text-center">
                          <div className="rounded-md border border-border/60 bg-surface-0 p-2">
                            <div className="text-lg font-semibold text-text-primary">{skill.usageCount}</div>
                            <div className="text-[11px] text-text-tertiary">{tr('uses')}</div>
                          </div>
                          <div className="rounded-md border border-success/30 bg-success/10 p-2">
                            <div className="text-lg font-semibold text-success">{skill.successCount}</div>
                            <div className="text-[11px] text-text-tertiary">{tr('successes')}</div>
                          </div>
                          <div className="rounded-md border border-danger/30 bg-danger/10 p-2">
                            <div className="text-lg font-semibold text-danger">{skill.failureCount}</div>
                            <div className="text-[11px] text-text-tertiary">{tr('failures')}</div>
                          </div>
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              </section>
            </div>
          )}

          {tab === 'browser' && (
            <div className="grid gap-5 xl:grid-cols-[420px_1fr]">
              <section className="rounded-lg border border-border/70 bg-surface-1 p-4">
                <div className="mb-4 flex items-center gap-2 text-sm font-semibold text-text-primary">
                  <Globe2 className="h-4 w-4 text-accent" />
                  {tr('browserEvidence')}
                </div>
                <div className="space-y-3">
                  <Field label={tr('url')}>
                    <input className={textInputClass()} value={browserUrl} onChange={(event) => setBrowserUrl(event.target.value)} placeholder={tr('browserUrlPlaceholder')} />
                  </Field>
                  <Field label={tr('mode')}>
                    <NexaSelect className={textInputClass()} value={browserMode} onChange={(event) => setBrowserMode(event.target.value)}>
                      <option value="auto">{tr('modeAuto')}</option>
                      <option value="readability">{tr('modeReadability')}</option>
                      <option value="text">{tr('modeText')}</option>
                      <option value="metadata">{tr('modeMetadata')}</option>
                    </NexaSelect>
                  </Field>
                  <Button variant="primary" disabled={busy === 'browser'} onClick={() => void captureBrowser()} icon={busy === 'browser' ? <Loader2 className="h-4 w-4 animate-spin" /> : <Globe2 className="h-4 w-4" />}>
                    {tr('capture')}
                  </Button>
                </div>
              </section>
              <section className="rounded-lg border border-border/70 bg-surface-1 p-4">
                {!capture ? (
                  <p className="text-sm text-text-tertiary">{tr('noCapture')}</p>
                ) : (
                  <div>
                    <div className="flex flex-wrap items-start justify-between gap-3">
                      <div>
                        <h2 className="text-sm font-semibold text-text-primary">{capture.title}</h2>
                        <p className="mt-1 break-all text-xs text-text-tertiary">{capture.finalUrl}</p>
                      </div>
                      <span className="rounded-md border border-success/30 bg-success/10 px-2 py-1 text-xs text-success">{tr('captured')}</span>
                    </div>
                    <pre className="mt-4 max-h-[520px] overflow-auto whitespace-pre-wrap rounded-md border border-border/60 bg-surface-0 p-3 text-xs leading-5 text-text-secondary">
                      {capture.excerpt}
                    </pre>
                  </div>
                )}
              </section>
            </div>
          )}
        </main>
      )}
    </div>
  );
}
