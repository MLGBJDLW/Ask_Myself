import { useEffect, useMemo, useState } from 'react';
import type { FormEvent } from 'react';
import { AnimatePresence, motion, useReducedMotion } from 'framer-motion';
import { AlertTriangle, Blocks, Check, ChevronDown, ChevronUp, Download, Eye, FileJson, Loader2, Pencil, Plug, Plus, RefreshCw, Search, Trash2, UserRound, X, Zap } from 'lucide-react';
import { useTranslation } from '../../i18n';
import { getSoftCollapseMotion } from '../../lib/uiMotion';
import type { PersonaProfile, SavePersonaInput } from '../../lib/api';
import type { AppConfig } from '../../types/conversation';
import type { McpServer, McpToolInfo, SaveMcpServerInput, SaveSkillInput, Skill, SkillChangeProposal } from '../../types/extensions';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { ConfirmDialog } from '../ui/ConfirmDialog';
import { McpServerForm } from './McpServerForm';
import { PackageHostSettingsPanel } from './PackageHostSettingsPanel';
import { ProjectToolsPanel } from './ProjectToolsPanel';
import { Section } from './SettingsSection';
import { SkillEditor } from './SkillEditor';
import { SkillInstaller } from './SkillInstaller';
import { SkillMarkdownPreview } from './SkillMarkdownPreview';
import { WebSearchSettingsPanel } from './WebSearchSettingsPanel';

export type SkillFilter = 'all' | 'builtin' | 'user' | 'enabled' | 'disabled';
export type McpToolState = Record<string, { tools: McpToolInfo[]; loading: boolean; error?: string }>;

interface ExtensionsSettingsTabProps {
  personas: PersonaProfile[];
  skills: Skill[];
  filteredSkills: Skill[];
  skillProposals: SkillChangeProposal[];
  skillProposalBusyId: string | null;
  showPersonaForm: boolean;
  editingPersona: PersonaProfile | null;
  deletePersonaTarget: PersonaProfile | null;
  skillSearch: string;
  skillFilter: SkillFilter;
  showSkillForm: boolean;
  editingSkill: Skill | null;
  deleteSkillTarget: Skill | null;
  viewSkill: Skill | null;
  mcpServers: McpServer[];
  mcpConfigPath: string;
  mcpConfigReloading: boolean;
  showMcpForm: boolean;
  editingMcpServer: McpServer | null;
  deleteMcpTarget: McpServer | null;
  mcpTestLoading: string | null;
  mcpToolCounts: McpToolState;
  mcpToolsExpanded: Record<string, boolean>;
  appConfig: AppConfig | null;
  appConfigLoading: boolean;
  onAddPersona: () => void;
  onSavePersona: (input: SavePersonaInput) => Promise<void>;
  onCancelPersonaForm: () => void;
  onPersonaEditorDirtyChange: (dirty: boolean) => void;
  onTogglePersona: (id: string, enabled: boolean) => void;
  onEditPersona: (persona: PersonaProfile) => void;
  onDeletePersonaTargetChange: (persona: PersonaProfile | null) => void;
  onConfirmDeletePersona: () => void;
  onSkillSearchChange: (value: string) => void;
  onSkillFilterChange: (filter: SkillFilter) => void;
  onExportAllSkills: () => void;
  onAddSkill: () => void;
  onSaveSkill: (input: SaveSkillInput) => Promise<void>;
  onCancelSkillForm: () => void;
  onSkillEditorDirtyChange: (dirty: boolean) => void;
  onViewSkillChange: (skill: Skill | null) => void;
  onToggleSkill: (id: string, enabled: boolean) => void;
  onEditSkill: (skill: Skill) => void;
  onDeleteSkillTargetChange: (skill: Skill | null) => void;
  onConfirmDeleteSkill: () => void;
  onApplySkillProposal: (id: string) => void;
  onRejectSkillProposal: (id: string) => void;
  onAddMcpServer: () => void;
  onOpenMcpConfig: () => void;
  onReloadMcpConfig: () => void;
  onSaveMcpServer: (input: SaveMcpServerInput) => Promise<void>;
  onCancelMcpForm: () => void;
  onMcpFormDirtyChange: (dirty: boolean) => void;
  onToggleMcpServer: (id: string, enabled: boolean) => void;
  onTestMcpServer: (id: string) => void;
  onEditMcpServer: (server: McpServer) => void;
  onDeleteMcpTargetChange: (server: McpServer | null) => void;
  onToggleMcpToolsExpanded: (serverId: string) => void;
  onConfirmDeleteMcpServer: () => void;
  onAppConfigChange: (config: AppConfig) => void;
  onAppConfigSave: () => void;
  onMarkAppConfigDirty: () => void;
  onPackageStateChange?: () => void;
}

interface PersonaCopy {
  personas: string;
  personasDescription: string;
  addPersona: string;
  noPersonas: string;
  name: string;
  description: string;
  instructions: string;
  enabled: string;
  defaultSkills: string;
  save: string;
  saving: string;
  cancel: string;
  builtin: string;
  disabled: string;
  defaultSkillCount: (count: number) => string;
  deleteConfirm: string;
}

function estimateTokens(text: string): number {
  if (!text) return 0;
  let tokens = 0;
  for (let index = 0; index < text.length; index++) {
    tokens += text.charCodeAt(index) > 0x2fff ? 1.5 : 0.25;
  }
  return Math.ceil(tokens);
}

function extractTriggers(description: string): string[] {
  const text = (description ?? '').trim();
  if (!text) return [];

  const firstSentence = text.split(/[.。!?！？\n]/)[0]?.trim() ?? '';
  const match = firstSentence.match(
    /^(?:Use (?:when|for)|Activates (?:on|when)|Triggers on|When)\s*:?\s*(.+)$/i,
  );
  if (!match) return [];

  return match[1]
    .split(/[,;，；]/)
    .map((item) => item.trim())
    .filter((item) => item.length > 0 && item.length <= 40)
    .slice(0, 4);
}

function compact(text: string, max = 180): string {
  const normalized = text.replace(/\s+/g, ' ').trim();
  if (normalized.length <= max) return normalized;
  return `${normalized.slice(0, Math.max(0, max - 1)).trimEnd()}…`;
}

function skillShortDescription(skill: Skill): string {
  return skill.interface?.shortDescription?.trim() || skill.description || '';
}

type DiffLineKind = 'same' | 'add' | 'remove';

interface DiffLine {
  kind: DiffLineKind;
  text: string;
}

function splitLines(text: string): string[] {
  if (!text) return [];
  return text.replace(/\r\n/g, '\n').split('\n');
}

function buildLineDiff(before: string, after: string): DiffLine[] {
  const oldLines = splitLines(before);
  const newLines = splitLines(after);
  if (oldLines.join('\n') === newLines.join('\n')) {
    return oldLines.map((text) => ({ kind: 'same', text }));
  }

  if (oldLines.length * newLines.length > 60000) {
    return [
      ...oldLines.map((text) => ({ kind: 'remove' as const, text })),
      ...newLines.map((text) => ({ kind: 'add' as const, text })),
    ];
  }

  const table = Array.from({ length: oldLines.length + 1 }, () =>
    Array.from({ length: newLines.length + 1 }, () => 0),
  );
  for (let i = oldLines.length - 1; i >= 0; i--) {
    for (let j = newLines.length - 1; j >= 0; j--) {
      table[i][j] =
        oldLines[i] === newLines[j]
          ? table[i + 1][j + 1] + 1
          : Math.max(table[i + 1][j], table[i][j + 1]);
    }
  }

  const diff: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < oldLines.length && j < newLines.length) {
    if (oldLines[i] === newLines[j]) {
      diff.push({ kind: 'same', text: oldLines[i] });
      i += 1;
      j += 1;
    } else if (table[i + 1][j] >= table[i][j + 1]) {
      diff.push({ kind: 'remove', text: oldLines[i] });
      i += 1;
    } else {
      diff.push({ kind: 'add', text: newLines[j] });
      j += 1;
    }
  }
  while (i < oldLines.length) {
    diff.push({ kind: 'remove', text: oldLines[i] });
    i += 1;
  }
  while (j < newLines.length) {
    diff.push({ kind: 'add', text: newLines[j] });
    j += 1;
  }
  return diff;
}

function findProposalTargetSkill(proposal: SkillChangeProposal | null, skills: Skill[]): Skill | null {
  if (!proposal) return null;
  if (proposal.skillId) {
    const byId = skills.find((skill) => skill.id === proposal.skillId);
    if (byId) return byId;
  }
  return skills.find((skill) => skill.name === proposal.name) ?? null;
}

const PERSONA_INSTRUCTIONS_TEMPLATE = `Role:
Identity:
Communication style:
Operating principles:
- 
Default skill bindings:
- Select durable method/playbook skills in the Default skills list below.
Boundaries:
- Persona instructions shape voice and workflow emphasis only.
- Methods, templates, checklists, and domain playbooks belong in skills, not this persona body.
- They do not override user instructions, privacy rules, source scope, or tool safety.`;

function PersonaEditor({
  persona,
  skills,
  copy,
  onSave,
  onCancel,
  onDirtyChange,
}: {
  persona?: PersonaProfile;
  skills: Skill[];
  copy: PersonaCopy;
  onSave: (input: SavePersonaInput) => Promise<void>;
  onCancel: () => void;
  onDirtyChange: (dirty: boolean) => void;
}) {
  const { t } = useTranslation();
  const isBuiltin = persona?.builtin === true;
  const [name, setName] = useState(persona?.name ?? '');
  const [description, setDescription] = useState(persona?.description ?? '');
  const [instructions, setInstructions] = useState(persona?.instructions ?? '');
  const [enabled, setEnabled] = useState(persona?.enabled ?? true);
  const [defaultSkillIds, setDefaultSkillIds] = useState<string[]>(persona?.defaultSkillIds ?? []);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    setName(persona?.name ?? '');
    setDescription(persona?.description ?? '');
    setInstructions(persona?.instructions ?? '');
    setEnabled(persona?.enabled ?? true);
    setDefaultSkillIds(persona?.defaultSkillIds ?? []);
    onDirtyChange(false);
  }, [onDirtyChange, persona]);

  const selectedSkillIds = useMemo(() => new Set(defaultSkillIds), [defaultSkillIds]);
  const update = (fn: () => void) => {
    fn();
    onDirtyChange(true);
  };
  const toggleSkill = (skillId: string) => {
    update(() => {
      setDefaultSkillIds((prev) =>
        prev.includes(skillId) ? prev.filter((id) => id !== skillId) : [...prev, skillId],
      );
    });
  };

  const handleSubmit = async (event: FormEvent) => {
    event.preventDefault();
    setSaving(true);
    try {
      await onSave({
        id: persona?.id ?? null,
        name,
        description,
        instructions,
        enabled,
        defaultSkillIds,
      });
      onDirtyChange(false);
    } finally {
      setSaving(false);
    }
  };

  return (
    <form onSubmit={handleSubmit} className="space-y-4 rounded-lg border border-border bg-surface-2 p-4">
      <div className="grid gap-3 md:grid-cols-2">
        <label className="space-y-1">
          <span className="text-xs font-medium text-text-secondary">{copy.name}</span>
          <input
            value={name}
            onChange={(event) => update(() => setName(event.target.value))}
            className="w-full rounded-md border border-border bg-surface-1 px-3 py-2 text-sm text-text-primary focus:border-accent focus:outline-none"
            required
            disabled={isBuiltin}
          />
        </label>
        <label className="space-y-1">
          <span className="text-xs font-medium text-text-secondary">{copy.description}</span>
          <input
            value={description}
            onChange={(event) => update(() => setDescription(event.target.value))}
            className="w-full rounded-md border border-border bg-surface-1 px-3 py-2 text-sm text-text-primary focus:border-accent focus:outline-none"
            disabled={isBuiltin}
          />
        </label>
      </div>
      <label className="space-y-1 block">
        <span className="flex items-center justify-between gap-2 text-xs font-medium text-text-secondary">
          <span>{copy.instructions}</span>
          <button
            type="button"
            onClick={() => update(() => setInstructions((value) => value.trim() ? value : PERSONA_INSTRUCTIONS_TEMPLATE))}
            className="text-[11px] font-medium text-accent hover:text-accent-hover"
            disabled={isBuiltin}
          >
            {t('settings.skillUseTemplate')}
          </button>
        </span>
        <textarea
          value={instructions}
          onChange={(event) => update(() => setInstructions(event.target.value))}
          rows={7}
          className="w-full resize-y rounded-md border border-border bg-surface-1 px-3 py-2 text-sm text-text-primary focus:border-accent focus:outline-none"
          required
          disabled={isBuiltin}
        />
      </label>
      <div className="flex items-center justify-between rounded-md border border-border bg-surface-1 px-3 py-2">
        <span className="text-sm text-text-secondary">{copy.enabled}</span>
        <button
          type="button"
          onClick={() => update(() => setEnabled((value) => !value))}
          disabled={isBuiltin}
          className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors duration-fast cursor-pointer ${
            enabled ? 'bg-accent' : 'bg-surface-3'
          }`}
        >
          <span className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform duration-fast ${
            enabled ? 'translate-x-6' : 'translate-x-1'
          }`} />
        </button>
      </div>
      {skills.length > 0 && (
        <div className="space-y-2">
          <p className="text-xs font-medium text-text-secondary">{copy.defaultSkills}</p>
          <div className="grid max-h-52 gap-2 overflow-auto rounded-md border border-border bg-surface-1 p-2 md:grid-cols-2">
            {skills.map((skill) => (
              <label
                key={skill.id}
                className="flex min-w-0 items-start gap-2 rounded-md px-2 py-1.5 text-xs text-text-secondary hover:bg-surface-2"
              >
                <input
                  type="checkbox"
                  checked={selectedSkillIds.has(skill.id)}
                  onChange={() => toggleSkill(skill.id)}
                  className="mt-0.5"
                />
                <span className="min-w-0">
                  <span className="block truncate text-text-primary">{skill.name}</span>
                  {skill.description && (
                    <span className="block truncate text-[11px] text-text-tertiary">
                      {skill.description}
                    </span>
                  )}
                </span>
              </label>
            ))}
          </div>
        </div>
      )}
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" size="sm" onClick={onCancel}>
          {copy.cancel}
        </Button>
        <Button type="submit" variant="primary" size="sm" disabled={saving}>
          {saving ? copy.saving : copy.save}
        </Button>
      </div>
    </form>
  );
}

export function ExtensionsSettingsTab({
  personas,
  skills,
  filteredSkills,
  skillProposals,
  skillProposalBusyId,
  showPersonaForm,
  editingPersona,
  deletePersonaTarget,
  skillSearch,
  skillFilter,
  showSkillForm,
  editingSkill,
  deleteSkillTarget,
  viewSkill,
  mcpServers,
  mcpConfigPath,
  mcpConfigReloading,
  showMcpForm,
  editingMcpServer,
  deleteMcpTarget,
  mcpTestLoading,
  mcpToolCounts,
  mcpToolsExpanded,
  appConfig,
  appConfigLoading,
  onAddPersona,
  onSavePersona,
  onCancelPersonaForm,
  onPersonaEditorDirtyChange,
  onTogglePersona,
  onEditPersona,
  onDeletePersonaTargetChange,
  onConfirmDeletePersona,
  onSkillSearchChange,
  onSkillFilterChange,
  onExportAllSkills,
  onAddSkill,
  onSaveSkill,
  onCancelSkillForm,
  onSkillEditorDirtyChange,
  onViewSkillChange,
  onToggleSkill,
  onEditSkill,
  onDeleteSkillTargetChange,
  onConfirmDeleteSkill,
  onApplySkillProposal,
  onRejectSkillProposal,
  onAddMcpServer,
  onOpenMcpConfig,
  onReloadMcpConfig,
  onSaveMcpServer,
  onCancelMcpForm,
  onMcpFormDirtyChange,
  onToggleMcpServer,
  onTestMcpServer,
  onEditMcpServer,
  onDeleteMcpTargetChange,
  onToggleMcpToolsExpanded,
  onConfirmDeleteMcpServer,
  onAppConfigChange,
  onAppConfigSave,
  onMarkAppConfigDirty,
  onPackageStateChange,
}: ExtensionsSettingsTabProps) {
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();
  const personaCopy: PersonaCopy = {
    personas: t('settings.personas'),
    personasDescription: t('settings.personasDescription'),
    addPersona: t('settings.addPersona'),
    noPersonas: t('settings.noPersonas'),
    name: t('settings.personaName'),
    description: t('settings.personaDescription'),
    instructions: t('settings.personaInstructions'),
    enabled: t('settings.personaEnabled'),
    defaultSkills: t('settings.personaDefaultSkills'),
    save: t('common.save'),
    saving: t('settings.personaSaving'),
    cancel: t('common.cancel'),
    builtin: t('settings.personaBuiltIn'),
    disabled: t('settings.personaDisabled'),
    defaultSkillCount: (count: number) => t('settings.personaDefaultSkillCount', { count }),
    deleteConfirm: t('settings.deletePersonaConfirm'),
  };
  const extensionCopy = {
    toolCount: (count: number) => t('settings.extensions.toolCount', { count }),
    connectionFailed: t('settings.extensions.connectionFailed'),
    availableTools: t('settings.extensions.availableTools'),
    toggleTools: t('settings.extensions.toggleTools'),
  };
  const [previewProposal, setPreviewProposal] = useState<SkillChangeProposal | null>(null);
  const [applyProposalTarget, setApplyProposalTarget] = useState<SkillChangeProposal | null>(null);
  const previewTargetSkill = useMemo(
    () => findProposalTargetSkill(previewProposal, skills),
    [previewProposal, skills],
  );
  const previewDiff = useMemo(
    () =>
      previewProposal
        ? buildLineDiff(
            previewProposal.action === 'patch' ? previewTargetSkill?.content ?? '' : '',
            previewProposal.content,
          )
        : [],
    [previewProposal, previewTargetSkill],
  );
  const confirmApplyProposal = () => {
    if (!applyProposalTarget) return;
    onApplySkillProposal(applyProposalTarget.id);
    setApplyProposalTarget(null);
  };

  return (
    <>
      <PackageHostSettingsPanel onPackageStateChange={onPackageStateChange} />

      {appConfig && (
        <WebSearchSettingsPanel
          appConfig={appConfig}
          loading={appConfigLoading}
          onChange={onAppConfigChange}
          onMarkDirty={onMarkAppConfigDirty}
          onSave={onAppConfigSave}
        />
      )}

      <Section
        icon={<UserRound size={20} />}
        title={personaCopy.personas}
        delay={0.01}
        description={personaCopy.personasDescription}
        collapsible
        defaultOpen={false}
        summary={
          <span className="rounded-full border border-border/60 bg-surface-2 px-2 py-1 text-[11px] text-text-secondary">
            {personas.length}
          </span>
        }
      >
        {showPersonaForm ? (
          <PersonaEditor
            persona={editingPersona ?? undefined}
            skills={skills}
            copy={personaCopy}
            onSave={onSavePersona}
            onCancel={onCancelPersonaForm}
            onDirtyChange={onPersonaEditorDirtyChange}
          />
        ) : (
          <div className="space-y-4">
            <div className="flex justify-end">
              <Button variant="primary" size="sm" icon={<Plus size={14} />} onClick={onAddPersona}>
                {personaCopy.addPersona}
              </Button>
            </div>
            {personas.length === 0 ? (
              <div className="py-8 text-center">
                <UserRound size={32} className="mx-auto mb-3 text-text-tertiary" />
                <p className="text-sm text-text-secondary">{personaCopy.noPersonas}</p>
              </div>
            ) : (
              <div className="space-y-3">
                {personas.map((persona) => {
                  const defaultSkillCount = persona.defaultSkillIds?.length ?? 0;
                  const defaultSkillNames = (persona.defaultSkillIds ?? [])
                    .map((skillId) => skills.find((skill) => skill.id === skillId)?.name ?? skillId)
                    .slice(0, 6);
                  return (
                    <motion.div
                      key={persona.id}
                      initial={{ opacity: 0, y: 20 }}
                      animate={{ opacity: 1, y: 0 }}
                      className="rounded-lg border border-border bg-surface-2 p-4 transition-colors hover:bg-surface-3/50"
                    >
                      <div className="flex items-start justify-between gap-3">
                        <div className="min-w-0 flex-1">
                          <div className="flex flex-wrap items-center gap-2">
                            <p className="truncate text-sm font-medium text-text-primary">{persona.name}</p>
                            {persona.builtin && (
                              <Badge variant="default" className="text-[10px] shrink-0 border-accent/40 text-accent">
                                {personaCopy.builtin}
                              </Badge>
                            )}
                            {!persona.enabled && !persona.builtin && (
                              <Badge variant="default" className="text-[10px] shrink-0 border-border text-text-tertiary">
                                {personaCopy.disabled}
                              </Badge>
                            )}
                            {defaultSkillCount > 0 && (
                              <Badge variant="default" className="text-[10px] shrink-0">
                                {personaCopy.defaultSkillCount(defaultSkillCount)}
                              </Badge>
                            )}
                          </div>
                          {persona.description ? (
                            <p className="mt-0.5 line-clamp-2 text-xs text-text-secondary">
                              {persona.description}
                            </p>
                          ) : (
                            <p className="mt-0.5 truncate text-xs text-text-tertiary">
                              {persona.instructions.slice(0, 100)}{persona.instructions.length > 100 ? '...' : ''}
                            </p>
                          )}
                        </div>
                        <div className="flex shrink-0 items-center gap-1">
                          {!persona.builtin && (
                            <button
                              onClick={() => onTogglePersona(persona.id, !persona.enabled)}
                              className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors duration-fast cursor-pointer ${
                                persona.enabled ? 'bg-accent' : 'bg-surface-3'
                              }`}
                            >
                              <span className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform duration-fast ${
                                persona.enabled ? 'translate-x-6' : 'translate-x-1'
                              }`} />
                            </button>
                          )}
                          <button
                            onClick={() => onEditPersona(persona)}
                            className="rounded p-1.5 text-text-tertiary hover:text-accent hover:bg-accent/10 transition-colors cursor-pointer"
                            aria-label={t('common.edit')}
                          >
                            <Pencil size={14} />
                          </button>
                          {!persona.builtin && (
                            <button
                              onClick={() => onDeletePersonaTargetChange(persona)}
                              className="rounded p-1.5 text-text-tertiary hover:text-danger hover:bg-danger/10 transition-colors cursor-pointer"
                              aria-label={t('common.delete')}
                            >
                              <Trash2 size={14} />
                            </button>
                          )}
                        </div>
                      </div>
                      <details className="mt-3 rounded-md border border-border/60 bg-surface-1 px-3 py-2 text-xs">
                        <summary className="cursor-pointer select-none text-text-secondary">{t('settings.skillPreview')}</summary>
                        <div className="mt-2 space-y-2 text-text-secondary">
                          <p className="whitespace-pre-wrap leading-5">
                            {persona.instructions.trim() || t('settings.personaDefaultInstructions')}
                          </p>
                          {defaultSkillNames.length > 0 && (
                            <div className="flex flex-wrap gap-1">
                              {defaultSkillNames.map((skillName) => (
                                <span
                                  key={skillName}
                                  className="rounded-md border border-border/50 bg-surface-0 px-1.5 py-0.5 text-[11px] text-text-tertiary"
                                >
                                  {skillName}
                                </span>
                              ))}
                            </div>
                          )}
                        </div>
                      </details>
                    </motion.div>
                  );
                })}
              </div>
            )}
          </div>
        )}
      </Section>

      <Section
        icon={<Blocks size={20} />}
        title={t('settings.skills')}
        delay={0.03}
        description={t('settings.skillsDescription')}
        collapsible
        defaultOpen={false}
        summary={
          <span className="rounded-full border border-border/60 bg-surface-2 px-2 py-1 text-[11px] text-text-secondary">
            {skills.length}
          </span>
        }
      >
        {showSkillForm ? (
          <SkillEditor
            skill={editingSkill ?? undefined}
            onSave={onSaveSkill}
            onCancel={onCancelSkillForm}
            onDirtyChange={onSkillEditorDirtyChange}
          />
        ) : (
          <div className="space-y-4">
            {skillProposals.length > 0 && (
              <div className="rounded-lg border border-accent/25 bg-accent/5 p-3">
                <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
                  <div>
                    <p className="text-sm font-medium text-text-primary">
                      {t('settings.skillProposals')}
                    </p>
                    <p className="text-xs text-text-tertiary">
                      {t('settings.skillProposalsDescription')}
                    </p>
                  </div>
                  <Badge variant="default" className="text-[10px] border-accent/40 text-accent">
                    {t('settings.skillProposalPendingCount', {
                      count: String(skillProposals.length),
                    })}
                  </Badge>
                </div>
                <div className="space-y-2">
                  {skillProposals.map((proposal) => {
                    const busy = skillProposalBusyId === proposal.id;
                    return (
                      <div
                        key={proposal.id}
                        className="rounded-md border border-border bg-surface-2 px-3 py-2"
                      >
                        <div className="flex items-start justify-between gap-3">
                          <div className="min-w-0 flex-1">
                            <div className="flex flex-wrap items-center gap-2">
                              <p className="truncate text-sm font-medium text-text-primary">
                                {proposal.name}
                              </p>
                              <Badge variant="default" className="text-[10px]">
                                {proposal.action === 'patch'
                                  ? t('settings.skillProposalPatch')
                                  : t('settings.skillProposalCreate')}
                              </Badge>
                              {proposal.source === 'auto_trace_review' && (
                                <Badge variant="default" className="text-[10px] border-accent/40 text-accent">
                                  {t('settings.skillProposalAuto')}
                                </Badge>
                              )}
                              <Badge variant="default" className="text-[10px]">
                                {t('settings.skillProposalConfidence', {
                                  value: `${Math.round((proposal.confidence ?? 0) * 100)}%`,
                                })}
                              </Badge>
                              {proposal.warnings.length > 0 && (
                                <Badge
                                  variant="default"
                                  className="text-[10px] border-warning/40 text-warning"
                                >
                                  {t('settings.skillProposalWarnings', {
                                    count: String(proposal.warnings.length),
                                  })}
                                </Badge>
                              )}
                            </div>
                            {proposal.description && (
                              <p className="mt-1 text-xs text-text-secondary">
                                {compact(proposal.description, 160)}
                              </p>
                            )}
                            {proposal.rationale && (
                              <p className="mt-1 text-xs text-text-tertiary">
                                {t('settings.skillProposalRationale')}: {compact(proposal.rationale, 220)}
                              </p>
                            )}
                            {proposal.source === 'auto_trace_review' && (
                              <p className="mt-1 text-xs text-accent">
                                {t('settings.skillProposalEvidence')}: {proposal.source}
                              </p>
                            )}
                            <p className="mt-1 font-mono text-[11px] text-text-tertiary">
                              {compact(proposal.content, 220)}
                            </p>
                          </div>
                          <div className="flex shrink-0 items-center gap-1">
                            <Button
                              variant="ghost"
                              size="sm"
                              icon={<Eye size={14} />}
                              onClick={() => setPreviewProposal(proposal)}
                              disabled={busy || !!skillProposalBusyId}
                            >
                              {t('settings.skillProposalPreview')}
                            </Button>
                            <Button
                              variant="ghost"
                              size="sm"
                              icon={busy ? <Loader2 size={14} className="animate-spin" /> : <X size={14} />}
                              onClick={() => onRejectSkillProposal(proposal.id)}
                              disabled={busy || !!skillProposalBusyId}
                            >
                              {t('settings.skillProposalReject')}
                            </Button>
                            <Button
                              variant="primary"
                              size="sm"
                              icon={busy ? <Loader2 size={14} className="animate-spin" /> : <Check size={14} />}
                              onClick={() => setApplyProposalTarget(proposal)}
                              disabled={busy || !!skillProposalBusyId}
                            >
                              {t('settings.skillProposalApply')}
                            </Button>
                          </div>
                        </div>
                      </div>
                    );
                  })}
                </div>
              </div>
            )}
            <div className="space-y-3">
              <div className="flex flex-wrap items-center gap-2">
                <div className="relative min-w-55 flex-1">
                  <Search
                    size={14}
                    className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-text-tertiary"
                  />
                  <input
                    type="text"
                    value={skillSearch}
                    onChange={(event) => onSkillSearchChange(event.target.value)}
                    placeholder={t('settings.skillSearchPlaceholder')}
                    className="w-full rounded-md border border-border bg-surface-2 py-1.5 pl-8 pr-3 text-sm text-text-primary placeholder:text-text-tertiary focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent"
                  />
                </div>
                <Button
                  variant="ghost"
                  size="sm"
                  icon={<Download size={14} />}
                  onClick={onExportAllSkills}
                  disabled={skills.length === 0}
                >
                  {t('settings.skillExportAll')}
                </Button>
                <SkillInstaller skills={skills} onInstalled={onPackageStateChange} />
                <Button variant="primary" size="sm" icon={<Plus size={14} />} onClick={onAddSkill}>
                  {t('settings.addSkill')}
                </Button>
              </div>
              <div className="flex flex-wrap items-center gap-1.5">
                {([
                  ['all', t('settings.skillFilterAll')],
                  ['builtin', t('settings.skillFilterBuiltin')],
                  ['user', t('settings.skillFilterUser')],
                  ['enabled', t('settings.skillFilterEnabled')],
                  ['disabled', t('settings.skillFilterDisabled')],
                ] as const).map(([id, label]) => (
                  <button
                    key={id}
                    type="button"
                    onClick={() => onSkillFilterChange(id)}
                    className={`rounded-full border px-2.5 py-0.5 text-[11px] transition-colors ${
                      skillFilter === id
                        ? 'border-accent/50 bg-accent/15 text-accent'
                        : 'border-border bg-surface-2 text-text-secondary hover:text-text-primary'
                    }`}
                  >
                    {label}
                  </button>
                ))}
              </div>
            </div>

            {skills.length === 0 ? (
              <div className="py-8 text-center">
                <Blocks size={32} className="mx-auto mb-3 text-text-tertiary" />
                <p className="text-sm text-text-secondary">{t('settings.noSkills')}</p>
              </div>
            ) : filteredSkills.length === 0 ? (
              <div className="py-8 text-center">
                <Search size={28} className="mx-auto mb-3 text-text-tertiary" />
                <p className="text-sm text-text-secondary">{t('settings.skillNoResults')}</p>
              </div>
            ) : (
              <div className="space-y-3">
                {filteredSkills.map((skill) => {
                  const triggers = extractTriggers(skill.description);
                  const shortDescription = skillShortDescription(skill);
                  const resourceCount = skill.resources?.length ?? 0;
                  return (
                    <motion.div
                      key={skill.id}
                      initial={{ opacity: 0, y: 20 }}
                      animate={{ opacity: 1, y: 0 }}
                      className="flex items-center justify-between rounded-lg border border-border bg-surface-2 p-4 transition-colors hover:bg-surface-3/50"
                    >
                      <div className="min-w-0 flex-1">
                        <div className="flex flex-wrap items-center gap-2">
                          <p className="text-sm font-medium text-text-primary truncate">{skill.name}</p>
                          {skill.builtin && (
                            <Badge variant="default" className="text-[10px] shrink-0 border-accent/40 text-accent">
                              {t('settings.skillBuiltIn')}
                            </Badge>
                          )}
                          <Badge variant="default" className="text-[10px] shrink-0">
                            ~{estimateTokens(skill.content)} tok
                          </Badge>
                          {resourceCount > 0 && (
                            <Badge variant="default" className="text-[10px] shrink-0">
                              {resourceCount} resources
                            </Badge>
                          )}
                          {skill.policy?.allowImplicitInvocation === false && (
                            <Badge variant="default" className="text-[10px] shrink-0 border-warning/40 text-warning">
                              explicit
                            </Badge>
                          )}
                          {!skill.enabled && !skill.builtin && (
                            <Badge variant="default" className="text-[10px] shrink-0 border-border text-text-tertiary">
                              {t('settings.skillFilterDisabled')}
                            </Badge>
                          )}
                        </div>
                        {shortDescription ? (
                          <p className="mt-0.5 text-xs text-text-secondary line-clamp-2">
                            {shortDescription}
                          </p>
                        ) : (
                          <p className="mt-0.5 text-xs text-text-tertiary truncate">
                            {skill.content.slice(0, 80)}{skill.content.length > 80 ? '…' : ''}
                          </p>
                        )}
                        {triggers.length > 0 && (
                          <div className="mt-1.5 flex flex-wrap gap-1">
                            {triggers.map((trigger) => (
                              <span
                                key={trigger}
                                className="inline-flex items-center rounded-full border border-border bg-surface-3/60 px-1.5 py-0.5 text-[10px] text-text-tertiary"
                              >
                                {trigger}
                              </span>
                            ))}
                          </div>
                        )}
                      </div>
                      <div className="flex items-center gap-1 shrink-0 ml-3">
                        <button
                          onClick={() => onViewSkillChange(skill)}
                          className="rounded p-1.5 text-text-tertiary hover:text-accent hover:bg-accent/10 transition-colors cursor-pointer"
                          aria-label={t('settings.skillViewBtn')}
                          title={t('settings.skillViewBtn')}
                        >
                          <Eye size={14} />
                        </button>
                        {!skill.builtin && (
                          <button
                            onClick={() => onToggleSkill(skill.id, !skill.enabled)}
                            className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors duration-fast cursor-pointer ${
                              skill.enabled ? 'bg-accent' : 'bg-surface-3'
                            }`}
                          >
                            <span className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform duration-fast ${
                              skill.enabled ? 'translate-x-6' : 'translate-x-1'
                            }`} />
                          </button>
                        )}
                        {!skill.builtin && (
                          <button
                            onClick={() => onEditSkill(skill)}
                            className="rounded p-1.5 text-text-tertiary hover:text-accent hover:bg-accent/10 transition-colors cursor-pointer"
                            aria-label={t('common.edit')}
                          >
                            <Pencil size={14} />
                          </button>
                        )}
                        {!skill.builtin && (
                          <button
                            onClick={() => onDeleteSkillTargetChange(skill)}
                            className="rounded p-1.5 text-text-tertiary hover:text-danger hover:bg-danger/10 transition-colors cursor-pointer"
                            aria-label={t('common.delete')}
                          >
                            <Trash2 size={14} />
                          </button>
                        )}
                      </div>
                    </motion.div>
                  );
                })}
              </div>
            )}
          </div>
        )}
      </Section>

      <ProjectToolsPanel />

      <Section
        icon={<Plug size={20} />}
        title={t('settings.mcpServers')}
        delay={0.06}
        description={t('settings.mcpServersDescription')}
        collapsible
        defaultOpen={false}
        summary={
          <span className="rounded-full border border-border/60 bg-surface-2 px-2 py-1 text-[11px] text-text-secondary">
            {mcpServers.length}
          </span>
        }
      >
        {showMcpForm ? (
          <McpServerForm
            server={editingMcpServer ?? undefined}
            onSave={onSaveMcpServer}
            onCancel={onCancelMcpForm}
            onDirtyChange={onMcpFormDirtyChange}
          />
        ) : (
          <div className="space-y-4">
            <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-border/60 bg-surface-1/55 p-3">
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2 text-xs font-medium text-text-primary">
                  <FileJson size={14} className="text-accent" />
                  {t('settings.mcpConfigFile')}
                </div>
                <p className="mt-1 truncate font-mono text-[10px] text-text-tertiary" title={mcpConfigPath}>
                  {mcpConfigPath || t('common.loading')}
                </p>
                <p className="mt-1 text-[11px] leading-4 text-text-tertiary">
                  {t('settings.mcpConfigDescription')}
                </p>
              </div>
              <div className="flex shrink-0 flex-wrap items-center gap-2">
                <Button variant="ghost" size="sm" icon={<FileJson size={14} />} onClick={onOpenMcpConfig}>
                  {t('settings.mcpOpenConfig')}
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  icon={<RefreshCw size={14} className={mcpConfigReloading ? 'animate-spin' : ''} />}
                  onClick={onReloadMcpConfig}
                  disabled={mcpConfigReloading}
                >
                  {t('settings.mcpReloadConfig')}
                </Button>
                <Button variant="primary" size="sm" icon={<Plus size={14} />} onClick={onAddMcpServer}>
                  {t('settings.addMcpServer')}
                </Button>
              </div>
            </div>
            {mcpServers.length === 0 ? (
              <div className="py-8 text-center">
                <Plug size={32} className="mx-auto mb-3 text-text-tertiary" />
                <p className="text-sm text-text-secondary">{t('settings.noMcpServers')}</p>
              </div>
            ) : (
              <div className="space-y-3">
                {mcpServers.map((server) => (
                  <motion.div
                    key={server.id}
                    initial={{ opacity: 0, y: 20 }}
                    animate={{ opacity: 1, y: 0 }}
                    className="rounded-lg border border-border bg-surface-2 transition-colors hover:bg-surface-3/50"
                  >
                    <div className="flex items-center justify-between p-4">
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-2">
                          <p className="text-sm font-medium text-text-primary truncate">{server.name}</p>
                          {server.builtinId && (
                            <Badge variant="default" className="ml-1 text-xs">{t('settings.mcpBuiltIn')}</Badge>
                          )}
                          {server.id.startsWith('user-json:') && (
                            <Badge variant="default" className="ml-1 text-xs">JSON</Badge>
                          )}
                          <Badge variant="default" className="text-[10px] shrink-0">{server.transport}</Badge>
                          {server.enabled && mcpToolCounts[server.id] && !mcpToolCounts[server.id].loading && !mcpToolCounts[server.id].error && (
                            <Badge variant="default" className="text-[10px] shrink-0 bg-accent/10 text-accent border-accent/20">
                              {extensionCopy.toolCount(mcpToolCounts[server.id].tools.length)}
                            </Badge>
                          )}
                          {server.enabled && mcpToolCounts[server.id]?.error && !mcpToolCounts[server.id].loading && (
                            <Badge
                              variant="default"
                              className="text-[10px] shrink-0 bg-danger/10 text-danger border-danger/20 cursor-help max-w-45 truncate"
                              title={mcpToolCounts[server.id].error}
                            >
                              <AlertTriangle size={10} className="inline mr-0.5 -mt-px" />
                              {extensionCopy.connectionFailed}
                            </Badge>
                          )}
                          {server.enabled && mcpToolCounts[server.id]?.loading && (
                            <Loader2 size={12} className="animate-spin text-text-tertiary" />
                          )}
                        </div>
                        <p className="mt-0.5 text-xs text-text-tertiary truncate">
                          {server.transport === 'stdio' ? server.command : server.url}
                        </p>
                      </div>
                      <div className="flex items-center gap-1 shrink-0 ml-3">
                        {server.enabled && mcpToolCounts[server.id]?.tools.length > 0 && (
                          <button
                            onClick={() => onToggleMcpToolsExpanded(server.id)}
                            className="rounded p-1.5 text-text-tertiary hover:text-accent hover:bg-accent/10 transition-colors cursor-pointer"
                            aria-label={extensionCopy.toggleTools}
                          >
                            {mcpToolsExpanded[server.id] ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
                          </button>
                        )}
                        <button
                          onClick={() => onToggleMcpServer(server.id, !server.enabled)}
                          className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors duration-fast cursor-pointer ${
                            server.enabled ? 'bg-accent' : 'bg-surface-3'
                          }`}
                        >
                          <span className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform duration-fast ${
                            server.enabled ? 'translate-x-6' : 'translate-x-1'
                          }`} />
                        </button>
                        <button
                          onClick={() => onTestMcpServer(server.id)}
                          disabled={mcpTestLoading === server.id}
                          className="rounded p-1.5 text-text-tertiary hover:text-accent hover:bg-accent/10 transition-colors cursor-pointer disabled:opacity-50"
                          aria-label={t('settings.mcpTestConnection')}
                        >
                          {mcpTestLoading === server.id ? <Loader2 size={14} className="animate-spin" /> : <Zap size={14} />}
                        </button>
                        {!server.id.startsWith('user-json:') && (
                          <button
                            onClick={() => onEditMcpServer(server)}
                            className="rounded p-1.5 text-text-tertiary hover:text-accent hover:bg-accent/10 transition-colors cursor-pointer"
                            aria-label={t('common.edit')}
                          >
                            <Pencil size={14} />
                          </button>
                        )}
                        {!server.builtinId && !server.id.startsWith('user-json:') && (
                          <button
                            onClick={() => onDeleteMcpTargetChange(server)}
                            className="rounded p-1.5 text-text-tertiary hover:text-danger hover:bg-danger/10 transition-colors cursor-pointer"
                            aria-label={t('common.delete')}
                          >
                            <Trash2 size={14} />
                          </button>
                        )}
                      </div>
                    </div>
                    <AnimatePresence initial={false}>
                      {mcpToolsExpanded[server.id] && mcpToolCounts[server.id]?.tools.length > 0 && (
                        <motion.div
                          {...getSoftCollapseMotion(!!shouldReduceMotion)}
                          className="overflow-hidden"
                        >
                          <div className="px-4 pb-3 border-t border-border/50">
                            <p className="text-[10px] text-text-tertiary uppercase tracking-wider mt-2 mb-1.5">{extensionCopy.availableTools}</p>
                            <div className="flex flex-wrap gap-1.5">
                              {mcpToolCounts[server.id].tools.map((tool) => (
                                <span
                                  key={tool.name}
                                  title={tool.description ?? tool.name}
                                  className="inline-flex items-center px-2 py-0.5 rounded text-[11px] font-mono
                                    bg-surface-3 text-text-secondary border border-border/50"
                                >
                                  {tool.name}
                                </span>
                              ))}
                            </div>
                          </div>
                        </motion.div>
                      )}
                    </AnimatePresence>
                  </motion.div>
                ))}
              </div>
            )}
          </div>
        )}
      </Section>

      {previewProposal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
          <div
            className="absolute inset-0 bg-black/60 backdrop-blur-sm"
            onClick={() => setPreviewProposal(null)}
            aria-hidden="true"
          />
          <div
            role="dialog"
            aria-modal="true"
            aria-label={previewProposal.name}
            className="relative z-10 flex max-h-[88vh] w-full max-w-4xl flex-col overflow-hidden rounded-lg border border-border bg-surface-2 shadow-lg"
          >
            <div className="flex items-center justify-between border-b border-border px-5 py-3">
              <div className="min-w-0">
                <div className="flex min-w-0 flex-wrap items-center gap-2">
                  <h2 className="truncate text-sm font-semibold text-text-primary">
                    {previewProposal.name}
                  </h2>
                  <Badge variant="default" className="text-[10px]">
                    {previewProposal.action === 'patch'
                      ? t('settings.skillProposalPatch')
                      : t('settings.skillProposalCreate')}
                  </Badge>
                  {previewProposal.source === 'auto_trace_review' && (
                    <Badge variant="default" className="text-[10px] border-accent/40 text-accent">
                      {t('settings.skillProposalAuto')}
                    </Badge>
                  )}
                  <Badge variant="default" className="text-[10px]">
                    {t('settings.skillProposalConfidence', {
                      value: `${Math.round((previewProposal.confidence ?? 0) * 100)}%`,
                    })}
                  </Badge>
                </div>
                {previewProposal.rationale && (
                  <p className="mt-1 line-clamp-2 text-xs text-text-tertiary">
                    {previewProposal.rationale}
                  </p>
                )}
              </div>
              <button
                onClick={() => setPreviewProposal(null)}
                className="rounded-md p-1 text-text-tertiary transition-colors hover:bg-surface-3 hover:text-text-primary"
                aria-label={t('common.close')}
              >
                <X size={16} />
              </button>
            </div>
            <div className="min-h-0 flex-1 overflow-auto px-5 py-4">
              {previewProposal.warnings.length > 0 && (
                <div className="mb-3 rounded-md border border-warning/30 bg-warning/10 px-3 py-2">
                  <p className="text-xs font-medium text-warning">
                    {t('settings.skillProposalWarnings', {
                      count: String(previewProposal.warnings.length),
                    })}
                  </p>
                  <ul className="mt-1 space-y-1">
                    {previewProposal.warnings.map((warning) => (
                      <li key={`${warning.code}-${warning.message}`} className="text-xs text-warning/80">
                        {warning.message}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
              {previewProposal.action === 'patch' && !previewTargetSkill && (
                <div className="mb-3 rounded-md border border-warning/30 bg-warning/10 px-3 py-2 text-xs text-warning">
                  {t('settings.skillProposalCurrentMissing')}
                </div>
              )}
              <div className="mb-2 flex items-center justify-between gap-2">
                <p className="text-xs font-medium text-text-secondary">
                  {previewProposal.action === 'create'
                    ? t('settings.skillProposalNewContent')
                    : t('settings.skillProposalDiffPreview')}
                </p>
                <span className="text-[11px] text-text-tertiary">
                  {previewDiff.filter((line) => line.kind === 'add').length}+
                  {' / '}
                  {previewDiff.filter((line) => line.kind === 'remove').length}-
                </span>
              </div>
              <div className="overflow-hidden rounded-md border border-border bg-surface-1">
                {previewDiff.length === 0 ? (
                  <p className="px-3 py-4 text-center text-xs text-text-tertiary">
                    {t('settings.skillProposalNoChanges')}
                  </p>
                ) : (
                  <div className="max-h-[54vh] overflow-auto py-2 font-mono text-[11px] leading-5">
                    {previewDiff.map((line, index) => (
                      <div
                        key={`${index}-${line.kind}`}
                        className={`flex gap-2 px-3 ${
                          line.kind === 'add'
                            ? 'bg-success/10 text-success'
                            : line.kind === 'remove'
                              ? 'bg-danger/10 text-danger'
                              : 'text-text-tertiary'
                        }`}
                      >
                        <span className="w-4 shrink-0 select-none text-right">
                          {line.kind === 'add' ? '+' : line.kind === 'remove' ? '-' : ' '}
                        </span>
                        <code className="min-w-0 flex-1 whitespace-pre-wrap break-words">
                          {line.text || ' '}
                        </code>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            </div>
            <div className="flex justify-end gap-2 border-t border-border px-5 py-3">
              <Button
                variant="ghost"
                size="sm"
                onClick={() => {
                  const proposal = previewProposal;
                  setPreviewProposal(null);
                  onRejectSkillProposal(proposal.id);
                }}
                disabled={!!skillProposalBusyId}
              >
                {t('settings.skillProposalReject')}
              </Button>
              <Button
                variant="primary"
                size="sm"
                icon={<Check size={14} />}
                onClick={() => {
                  setApplyProposalTarget(previewProposal);
                  setPreviewProposal(null);
                }}
                disabled={!!skillProposalBusyId}
              >
                {t('settings.skillProposalApply')}
              </Button>
            </div>
          </div>
        </div>
      )}

      <ConfirmDialog
        open={!!deletePersonaTarget}
        onClose={() => onDeletePersonaTargetChange(null)}
        onConfirm={onConfirmDeletePersona}
        title={t('common.delete')}
        message={personaCopy.deleteConfirm}
        confirmText={t('common.delete')}
        variant="danger"
      />

      <ConfirmDialog
        open={!!deleteSkillTarget}
        onClose={() => onDeleteSkillTargetChange(null)}
        onConfirm={onConfirmDeleteSkill}
        title={t('common.delete')}
        message={t('settings.deleteSkillConfirm')}
        confirmText={t('common.delete')}
        variant="danger"
      />

      <ConfirmDialog
        open={!!applyProposalTarget}
        onClose={() => setApplyProposalTarget(null)}
        onConfirm={confirmApplyProposal}
        title={t('settings.skillProposalApplyConfirmTitle')}
        message={t('settings.skillProposalApplyConfirmMessage')}
        confirmText={t('settings.skillProposalApply')}
        variant="warning"
        loading={!!applyProposalTarget && skillProposalBusyId === applyProposalTarget.id}
      />

      {viewSkill && (
        <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
          <div
            className="absolute inset-0 bg-black/60 backdrop-blur-sm"
            onClick={() => onViewSkillChange(null)}
            aria-hidden="true"
          />
          <div
            role="dialog"
            aria-modal="true"
            aria-label={viewSkill.name}
            className="relative z-10 flex max-h-[85vh] w-full max-w-3xl flex-col overflow-hidden rounded-lg border border-border bg-surface-2 shadow-lg"
          >
            <div className="flex items-center justify-between border-b border-border px-5 py-3">
              <div className="flex min-w-0 items-center gap-2">
                <h2 className="truncate text-sm font-semibold text-text-primary">
                  {viewSkill.name}
                </h2>
                {viewSkill.builtin && (
                  <Badge variant="default" className="text-[10px] shrink-0 border-accent/40 text-accent">
                    {t('settings.skillBuiltIn')}
                  </Badge>
                )}
              </div>
              <button
                onClick={() => onViewSkillChange(null)}
                className="rounded-md p-1 text-text-tertiary transition-colors hover:bg-surface-3 hover:text-text-primary"
                aria-label={t('common.close')}
              >
                <X size={16} />
              </button>
            </div>
            <div className="overflow-auto px-5 py-4">
              <div className="mb-3 grid gap-2 rounded-md border border-border bg-surface-1 px-3 py-2 text-xs text-text-secondary md:grid-cols-2">
                <div>
                  <span className="text-text-tertiary">{t('settings.skillShortDescription')}</span>
                  <p className="mt-0.5 text-text-primary">{skillShortDescription(viewSkill) || '—'}</p>
                </div>
                <div>
                  <span className="text-text-tertiary">{t('settings.skillPolicy')}</span>
                  <p className="mt-0.5 text-text-primary">
                    {t('settings.skillImplicitPolicy', {
                      value: String(viewSkill.policy?.allowImplicitInvocation ?? true),
                    })}
                  </p>
                </div>
                <div>
                  <span className="text-text-tertiary">{t('settings.skillSource')}</span>
                  <p className="mt-0.5 truncate text-text-primary" title={viewSkill.sourcePath ?? undefined}>
                    {viewSkill.sourcePath ?? (viewSkill.builtin ? t('common.bundled') : t('common.userDefined'))}
                  </p>
                </div>
                <div>
                  <span className="text-text-tertiary">{t('settings.skillResources')}</span>
                  <p className="mt-0.5 text-text-primary">
                    {viewSkill.resources?.length ? viewSkill.resources.map((resource) => resource.path).slice(0, 3).join(', ') : t('common.none')}
                  </p>
                </div>
              </div>
              <SkillMarkdownPreview
                content={viewSkill.content}
                fallbackName={viewSkill.name}
                fallbackDescription={viewSkill.description}
              />
            </div>
          </div>
        </div>
      )}

      <ConfirmDialog
        open={!!deleteMcpTarget}
        onClose={() => onDeleteMcpTargetChange(null)}
        onConfirm={onConfirmDeleteMcpServer}
        title={t('common.delete')}
        message={t('settings.deleteMcpServerConfirm')}
        confirmText={t('common.delete')}
        variant="danger"
      />
    </>
  );
}
