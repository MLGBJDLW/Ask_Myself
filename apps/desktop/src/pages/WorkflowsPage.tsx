import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import { useNavigate } from 'react-router-dom';
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
  Save,
  ShieldCheck,
  Trash2,
  Workflow,
  XCircle,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { toast } from 'sonner';
import * as api from '../lib/api';
import type { Source } from '../types';
import type { WorkflowCatalogTemplate } from '../lib/api';
import type {
  BrowserEvidenceCapture,
  LearningGovernanceSnapshot,
  SaveWorkflowAutomationInput,
  WorkflowAutomation,
  WorkflowAutomationTrigger,
  WorkflowAutomationDueRun,
} from '../types/workflows';

const EN_COPY = {
  title: 'Workflow Workbench',
  subtitle: 'Run guided templates, schedule local automations, inspect learning health, and capture browser evidence.',
  refresh: 'Refresh',
  templates: 'Templates',
  automations: 'Automations',
  governance: 'Learning',
  browserEvidence: 'Browser Evidence',
  run: 'Run',
  saveAutomation: 'Save automation',
  updateAutomation: 'Update automation',
  createAutomation: 'Create automation',
  dueNow: 'Due now',
  noDue: 'No scheduled workflows are due.',
  enabled: 'Enabled',
  disabled: 'Disabled',
  nextRun: 'Next run',
  lastRun: 'Last run',
  delete: 'Delete',
  name: 'Name',
  description: 'Description',
  prompt: 'Prompt',
  trigger: 'Trigger',
  schedule: 'Schedule',
  folder: 'Folder',
  manual: 'Manual',
  cron: 'Cron',
  folderPath: 'Folder path',
  filePattern: 'File pattern',
  sourceScope: 'Source scope',
  approvalRequired: 'Approval before run',
  riskLevel: 'Risk level',
  allowedTools: 'Allowed tools',
  capture: 'Capture',
  url: 'URL',
  mode: 'Mode',
  captured: 'Captured',
  noCapture: 'No browser evidence captured yet.',
  pendingProposals: 'Pending proposals',
  proceduralMemory: 'Procedural memory',
  memoryInjections: 'Memory injections',
  noStats: 'No skill usage has been recorded yet.',
  failures: 'failures',
  successes: 'successes',
  recommendations: 'Recommendations',
  loading: 'Loading workflows...',
};

const ZH_COPY: typeof EN_COPY = {
  title: '工作流工作台',
  subtitle: '运行模板，管理本地自动化，检查学习治理，并捕获可审计网页证据。',
  refresh: '刷新',
  templates: '模板',
  automations: '自动化',
  governance: '学习治理',
  browserEvidence: '浏览器证据',
  run: '运行',
  saveAutomation: '保存自动化',
  updateAutomation: '更新自动化',
  createAutomation: '创建自动化',
  dueNow: '待执行',
  noDue: '当前没有到期的定时工作流。',
  enabled: '已启用',
  disabled: '已停用',
  nextRun: '下次运行',
  lastRun: '上次运行',
  delete: '删除',
  name: '名称',
  description: '描述',
  prompt: '提示词',
  trigger: '触发器',
  schedule: '定时',
  folder: '文件夹',
  manual: '手动',
  cron: 'Cron',
  folderPath: '文件夹路径',
  filePattern: '文件匹配',
  sourceScope: '数据源范围',
  approvalRequired: '运行前审批',
  riskLevel: '风险等级',
  allowedTools: '允许工具',
  capture: '捕获',
  url: 'URL',
  mode: '模式',
  captured: '已捕获',
  noCapture: '还没有捕获浏览器证据。',
  pendingProposals: '待审提案',
  proceduralMemory: '流程记忆',
  memoryInjections: '记忆注入',
  noStats: '还没有记录 Skill 使用情况。',
  failures: '失败',
  successes: '成功',
  recommendations: '建议',
  loading: '正在加载工作流...',
};

type Tab = 'templates' | 'automations' | 'governance' | 'browser';

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
  enabled: true,
};

const TAB_ITEMS: Array<{ id: Tab; label: keyof typeof EN_COPY; icon: LucideIcon }> = [
  { id: 'templates', label: 'templates', icon: Workflow },
  { id: 'automations', label: 'automations', icon: CalendarClock },
  { id: 'governance', label: 'governance', icon: Activity },
  { id: 'browser', label: 'browserEvidence', icon: Globe2 },
];

function useCopy() {
  return navigator.language.toLowerCase().startsWith('zh') ? ZH_COPY : EN_COPY;
}

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

function triggerLabel(trigger: WorkflowAutomationTrigger, copy: typeof EN_COPY) {
  if (trigger.kind === 'schedule') return `${copy.schedule}: ${trigger.cron}`;
  if (trigger.kind === 'folder') return `${copy.folder}: ${trigger.path || '-'} ${trigger.pattern || ''}`;
  return copy.manual;
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
  const copy = useCopy();
  const navigate = useNavigate();
  const [tab, setTab] = useState<Tab>('templates');
  const [loading, setLoading] = useState(true);
  const [templates, setTemplates] = useState<WorkflowCatalogTemplate[]>([]);
  const [automations, setAutomations] = useState<WorkflowAutomation[]>([]);
  const [dueRuns, setDueRuns] = useState<WorkflowAutomationDueRun[]>([]);
  const [sources, setSources] = useState<Source[]>([]);
  const [governance, setGovernance] = useState<LearningGovernanceSnapshot | null>(null);
  const [form, setForm] = useState<SaveWorkflowAutomationInput>(emptyForm);
  const [browserUrl, setBrowserUrl] = useState('');
  const [browserMode, setBrowserMode] = useState('auto');
  const [capture, setCapture] = useState<BrowserEvidenceCapture | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const selectedTemplate = useMemo(
    () => templates.find((template) => template.id === form.workflowTemplateId) ?? templates[0],
    [form.workflowTemplateId, templates],
  );

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [templateItems, automationItems, dueItems, sourceItems, governanceSnapshot] = await Promise.all([
        api.listWorkflowTemplates(),
        api.listWorkflowAutomations(),
        api.listDueWorkflowAutomations(),
        api.listSources(),
        api.getLearningGovernanceSnapshot(),
      ]);
      setTemplates(templateItems);
      setAutomations(automationItems);
      setDueRuns(dueItems);
      setSources(sourceItems);
      setGovernance(governanceSnapshot);
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

  const runPrompt = useCallback((prompt: string, sourceScope?: string[]) => {
    navigate('/chat', {
      state: {
        initialMessage: prompt,
        sourceIds: sourceScope ?? [],
      },
    });
  }, [navigate]);

  const runTemplate = useCallback((template: WorkflowCatalogTemplate) => {
    runPrompt(template.promptTemplate);
  }, [runPrompt]);

  const runAutomation = useCallback(async (automation: WorkflowAutomation) => {
    setBusy(automation.id);
    try {
      const prompt = await api.previewWorkflowAutomationPrompt(automation.id);
      await api.recordWorkflowAutomationRun(automation.id, 'queued', null, 'Queued from Workflow Workbench').catch(() => undefined);
      runPrompt(prompt, automation.sourceScope);
    } catch (error) {
      toast.error(String(error));
    } finally {
      setBusy(null);
    }
  }, [runPrompt]);

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
    const effectivePrompt = (form.prompt || selectedTemplate?.promptTemplate || '').trim();
    if (!form.name.trim() || !effectivePrompt) {
      toast.error('Name and prompt are required.');
      return;
    }
    setBusy('save');
    try {
      await api.saveWorkflowAutomation({ ...form, prompt: effectivePrompt });
      setForm(emptyForm);
      await load();
      toast.success(copy.saveAutomation);
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
      toast.success(copy.captured);
    } catch (error) {
      toast.error(String(error));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="flex min-h-full flex-col bg-surface-0">
      <header className="border-b border-border bg-surface-1 px-6 py-5">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div>
            <h1 className="text-xl font-semibold tracking-normal text-text-primary">{copy.title}</h1>
            <p className="mt-1 max-w-3xl text-sm leading-6 text-text-secondary">{copy.subtitle}</p>
          </div>
          <Button onClick={() => void load()} icon={loading ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}>
            {copy.refresh}
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
              {copy[label]}
            </button>
          ))}
        </div>
      </header>

      {loading ? (
        <div className="flex flex-1 items-center justify-center text-sm text-text-tertiary">{copy.loading}</div>
      ) : (
        <main className="flex-1 space-y-6 p-6">
          {tab === 'templates' && (
            <div className="grid gap-4 xl:grid-cols-[1fr_360px]">
              <section className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
                {templates.map((template) => (
                  <article key={template.id} className="rounded-lg border border-border/70 bg-surface-1 p-4">
                    <div className="flex items-start justify-between gap-3">
                      <div className="min-w-0">
                        <h2 className="text-sm font-semibold text-text-primary">{template.label}</h2>
                        <p className="mt-1 line-clamp-3 text-xs leading-5 text-text-secondary">{template.description}</p>
                      </div>
                      <Button variant="primary" onClick={() => runTemplate(template)} icon={<Play className="h-4 w-4" />}>
                        {copy.run}
                      </Button>
                    </div>
                    <div className="mt-3 flex flex-wrap gap-1.5">
                      {template.tasks.map((task) => (
                        <span key={task.id} className="rounded-md border border-border/60 bg-surface-0 px-2 py-1 text-[11px] text-text-tertiary">
                          {task.roleLabel}
                        </span>
                      ))}
                    </div>
                  </article>
                ))}
              </section>
              <aside className="rounded-lg border border-border/70 bg-surface-1 p-4">
                <div className="flex items-center gap-2 text-sm font-semibold text-text-primary">
                  <CalendarClock className="h-4 w-4 text-accent" />
                  {copy.dueNow}
                </div>
                <div className="mt-3 space-y-2">
                  {dueRuns.length === 0 ? (
                    <p className="text-sm text-text-tertiary">{copy.noDue}</p>
                  ) : dueRuns.map((item) => (
                    <button
                      key={item.automation.id}
                      type="button"
                      onClick={() => void runAutomation(item.automation)}
                      className="block w-full rounded-md border border-border/70 bg-surface-0 p-3 text-left transition-colors hover:border-accent/60 hover:bg-accent-subtle/30"
                    >
                      <div className="text-sm font-medium text-text-primary">{item.automation.name}</div>
                      <div className="mt-1 text-xs text-text-tertiary">{item.dueReason}</div>
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
                  {form.id ? copy.updateAutomation : copy.createAutomation}
                </div>
                <div className="space-y-3">
                  <Field label={copy.name}>
                    <input className={textInputClass()} value={form.name} onChange={(event) => setForm({ ...form, name: event.target.value })} />
                  </Field>
                  <Field label={copy.description}>
                    <input className={textInputClass()} value={form.description} onChange={(event) => setForm({ ...form, description: event.target.value })} />
                  </Field>
                  <Field label={copy.templates}>
                    <select
                      className={textInputClass()}
                      value={form.workflowTemplateId}
                      onChange={(event) => {
                        const nextTemplate = templates.find((template) => template.id === event.target.value);
                        setForm({
                          ...form,
                          workflowTemplateId: event.target.value,
                          prompt: nextTemplate?.promptTemplate ?? form.prompt,
                        });
                      }}
                    >
                      {templates.map((template) => (
                        <option key={template.id} value={template.id}>{template.label}</option>
                      ))}
                    </select>
                  </Field>
                  <Field label={copy.prompt}>
                    <textarea className={textareaClass()} value={form.prompt || selectedTemplate?.promptTemplate || ''} onChange={(event) => setForm({ ...form, prompt: event.target.value })} />
                  </Field>
                  <Field label={copy.trigger}>
                    <div className="grid grid-cols-3 gap-1 rounded-md border border-border/70 bg-surface-0 p-1">
                      {[
                        ['schedule', copy.schedule],
                        ['folder', copy.folder],
                        ['manual', copy.manual],
                      ].map(([id, label]) => (
                        <button
                          key={id}
                          type="button"
                          onClick={() => updateTriggerKind(id as WorkflowAutomationTrigger['kind'])}
                          className={`h-8 rounded-md text-xs transition-colors ${form.trigger.kind === id ? 'bg-accent-subtle text-accent' : 'text-text-secondary hover:bg-surface-2'}`}
                        >
                          {label}
                        </button>
                      ))}
                    </div>
                  </Field>
                  {form.trigger.kind === 'schedule' && (
                    <Field label={copy.cron}>
                      <input className={textInputClass()} value={form.trigger.cron} onChange={(event) => setForm({ ...form, trigger: { kind: 'schedule', cron: event.target.value } })} />
                    </Field>
                  )}
                  {form.trigger.kind === 'folder' && (
                    <div className="grid gap-3 sm:grid-cols-2">
                      <Field label={copy.folderPath}>
                        <input className={textInputClass()} value={form.trigger.path} onChange={(event) => setForm({ ...form, trigger: { ...(form.trigger as Extract<WorkflowAutomationTrigger, { kind: 'folder' }>), path: event.target.value } })} />
                      </Field>
                      <Field label={copy.filePattern}>
                        <input className={textInputClass()} value={form.trigger.pattern} onChange={(event) => setForm({ ...form, trigger: { ...(form.trigger as Extract<WorkflowAutomationTrigger, { kind: 'folder' }>), pattern: event.target.value } })} />
                      </Field>
                    </div>
                  )}
                  <Field label={copy.sourceScope}>
                    <select
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
                    </select>
                  </Field>
                  <div className="grid gap-3 sm:grid-cols-2">
                    <Field label={copy.riskLevel}>
                      <select className={textInputClass()} value={form.approvalPolicy.riskLevel} onChange={(event) => setForm({ ...form, approvalPolicy: { ...form.approvalPolicy, riskLevel: event.target.value } })}>
                        <option value="low">low</option>
                        <option value="medium">medium</option>
                        <option value="high">high</option>
                      </select>
                    </Field>
                    <label className="mt-6 inline-flex h-9 items-center gap-2 rounded-md border border-border/70 bg-surface-0 px-3 text-sm text-text-secondary">
                      <input
                        type="checkbox"
                        checked={form.approvalPolicy.requireBeforeRun}
                        onChange={(event) => setForm({ ...form, approvalPolicy: { ...form.approvalPolicy, requireBeforeRun: event.target.checked } })}
                      />
                      {copy.approvalRequired}
                    </label>
                  </div>
                  <Field label={copy.allowedTools}>
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
                      {copy.saveAutomation}
                    </Button>
                    <Button onClick={() => setForm(emptyForm)}>{copy.createAutomation}</Button>
                  </div>
                </div>
              </section>

              <section className="space-y-3">
                {automations.map((automation) => (
                  <article key={automation.id} className="rounded-lg border border-border/70 bg-surface-1 p-4">
                    <div className="flex flex-wrap items-start justify-between gap-3">
                      <div className="min-w-0">
                        <div className="flex flex-wrap items-center gap-2">
                          <h2 className="text-sm font-semibold text-text-primary">{automation.name}</h2>
                          <span className={`rounded-md border px-2 py-0.5 text-[11px] ${automation.enabled ? 'border-success/30 bg-success/10 text-success' : 'border-border/70 bg-surface-0 text-text-tertiary'}`}>
                            {automation.enabled ? copy.enabled : copy.disabled}
                          </span>
                          <span className="rounded-md border border-border/70 bg-surface-0 px-2 py-0.5 text-[11px] text-text-tertiary">
                            {triggerLabel(automation.trigger, copy)}
                          </span>
                        </div>
                        <p className="mt-1 line-clamp-2 text-xs leading-5 text-text-secondary">{automation.description || automation.prompt}</p>
                        <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-[11px] text-text-tertiary">
                          <span>{copy.nextRun}: {formatTime(automation.nextRunAt)}</span>
                          <span>{copy.lastRun}: {formatTime(automation.lastRunAt)}</span>
                          <span>{automation.workflowTemplateId}</span>
                        </div>
                      </div>
                      <div className="flex flex-wrap gap-2">
                        <Button disabled={busy === automation.id} onClick={() => void runAutomation(automation)} icon={<Play className="h-4 w-4" />}>{copy.run}</Button>
                        <Button onClick={() => editAutomation(automation)} icon={<ClipboardList className="h-4 w-4" />}>{copy.updateAutomation}</Button>
                        <Button disabled={busy === automation.id} onClick={() => void toggleAutomation(automation)} icon={automation.enabled ? <PauseCircle className="h-4 w-4" /> : <CheckCircle2 className="h-4 w-4" />}>
                          {automation.enabled ? copy.disabled : copy.enabled}
                        </Button>
                        <Button variant="danger" disabled={busy === automation.id} onClick={() => void deleteAutomation(automation)} icon={<Trash2 className="h-4 w-4" />}>{copy.delete}</Button>
                      </div>
                    </div>
                  </article>
                ))}
              </section>
            </div>
          )}

          {tab === 'governance' && governance && (
            <div className="grid gap-5 xl:grid-cols-[320px_1fr]">
              <section className="space-y-3">
                {([
                  { label: copy.pendingProposals, value: governance.pendingProposals, icon: ShieldCheck },
                  { label: copy.proceduralMemory, value: governance.proceduralMemoryCount, icon: FileSearch },
                  { label: copy.memoryInjections, value: governance.memoryInjectionCount, icon: Activity },
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
                    <div className="text-sm font-semibold text-warning">{copy.recommendations}</div>
                    <ul className="mt-2 space-y-2 text-sm leading-5 text-text-secondary">
                      {governance.recommendations.map((item) => <li key={item}>{item}</li>)}
                    </ul>
                  </div>
                )}
              </section>
              <section className="rounded-lg border border-border/70 bg-surface-1">
                {governance.skillStats.length === 0 ? (
                  <div className="p-6 text-sm text-text-tertiary">{copy.noStats}</div>
                ) : (
                  <div className="divide-y divide-border/60">
                    {governance.skillStats.map((skill) => (
                      <div key={skill.skillId} className="grid gap-3 p-4 md:grid-cols-[1fr_220px]">
                        <div className="min-w-0">
                          <div className="flex items-center gap-2">
                            <h2 className="text-sm font-semibold text-text-primary">{skill.name}</h2>
                            {!skill.enabled && <span className="text-xs text-text-tertiary">{copy.disabled}</span>}
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
                            <div className="text-[11px] text-text-tertiary">uses</div>
                          </div>
                          <div className="rounded-md border border-success/30 bg-success/10 p-2">
                            <div className="text-lg font-semibold text-success">{skill.successCount}</div>
                            <div className="text-[11px] text-text-tertiary">{copy.successes}</div>
                          </div>
                          <div className="rounded-md border border-danger/30 bg-danger/10 p-2">
                            <div className="text-lg font-semibold text-danger">{skill.failureCount}</div>
                            <div className="text-[11px] text-text-tertiary">{copy.failures}</div>
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
                  {copy.browserEvidence}
                </div>
                <div className="space-y-3">
                  <Field label={copy.url}>
                    <input className={textInputClass()} value={browserUrl} onChange={(event) => setBrowserUrl(event.target.value)} placeholder="https://example.com/report" />
                  </Field>
                  <Field label={copy.mode}>
                    <select className={textInputClass()} value={browserMode} onChange={(event) => setBrowserMode(event.target.value)}>
                      <option value="auto">auto</option>
                      <option value="readability">readability</option>
                      <option value="text">text</option>
                      <option value="metadata">metadata</option>
                    </select>
                  </Field>
                  <Button variant="primary" disabled={busy === 'browser'} onClick={() => void captureBrowser()} icon={busy === 'browser' ? <Loader2 className="h-4 w-4 animate-spin" /> : <Globe2 className="h-4 w-4" />}>
                    {copy.capture}
                  </Button>
                </div>
              </section>
              <section className="rounded-lg border border-border/70 bg-surface-1 p-4">
                {!capture ? (
                  <p className="text-sm text-text-tertiary">{copy.noCapture}</p>
                ) : (
                  <div>
                    <div className="flex flex-wrap items-start justify-between gap-3">
                      <div>
                        <h2 className="text-sm font-semibold text-text-primary">{capture.title}</h2>
                        <p className="mt-1 break-all text-xs text-text-tertiary">{capture.finalUrl}</p>
                      </div>
                      <span className="rounded-md border border-success/30 bg-success/10 px-2 py-1 text-xs text-success">{copy.captured}</span>
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
