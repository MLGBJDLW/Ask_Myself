import { AnimatePresence, motion } from 'framer-motion';
import { AlertTriangle, Blocks, ChevronDown, ChevronUp, Download, Eye, Loader2, Pencil, Plug, Plus, Search, Trash2, X, Zap } from 'lucide-react';
import { useTranslation } from '../../i18n';
import type { McpServer, McpToolInfo, SaveMcpServerInput, SaveSkillInput, Skill } from '../../types/extensions';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { ConfirmDialog } from '../ui/ConfirmDialog';
import { McpServerForm } from './McpServerForm';
import { Section } from './SettingsSection';
import { SkillEditor } from './SkillEditor';
import { SkillMarkdownPreview } from './SkillMarkdownPreview';

export type SkillFilter = 'all' | 'builtin' | 'user' | 'enabled' | 'disabled';
export type McpToolState = Record<string, { tools: McpToolInfo[]; loading: boolean; error?: string }>;

interface ExtensionsSettingsTabProps {
  skills: Skill[];
  filteredSkills: Skill[];
  skillSearch: string;
  skillFilter: SkillFilter;
  showSkillForm: boolean;
  editingSkill: Skill | null;
  deleteSkillTarget: Skill | null;
  viewSkill: Skill | null;
  mcpServers: McpServer[];
  showMcpForm: boolean;
  editingMcpServer: McpServer | null;
  deleteMcpTarget: McpServer | null;
  mcpTestLoading: string | null;
  mcpToolCounts: McpToolState;
  mcpToolsExpanded: Record<string, boolean>;
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
  onAddMcpServer: () => void;
  onSaveMcpServer: (input: SaveMcpServerInput) => Promise<void>;
  onCancelMcpForm: () => void;
  onMcpFormDirtyChange: (dirty: boolean) => void;
  onToggleMcpServer: (id: string, enabled: boolean) => void;
  onTestMcpServer: (id: string) => void;
  onEditMcpServer: (server: McpServer) => void;
  onDeleteMcpTargetChange: (server: McpServer | null) => void;
  onToggleMcpToolsExpanded: (serverId: string) => void;
  onConfirmDeleteMcpServer: () => void;
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

export function ExtensionsSettingsTab({
  skills,
  filteredSkills,
  skillSearch,
  skillFilter,
  showSkillForm,
  editingSkill,
  deleteSkillTarget,
  viewSkill,
  mcpServers,
  showMcpForm,
  editingMcpServer,
  deleteMcpTarget,
  mcpTestLoading,
  mcpToolCounts,
  mcpToolsExpanded,
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
  onAddMcpServer,
  onSaveMcpServer,
  onCancelMcpForm,
  onMcpFormDirtyChange,
  onToggleMcpServer,
  onTestMcpServer,
  onEditMcpServer,
  onDeleteMcpTargetChange,
  onToggleMcpToolsExpanded,
  onConfirmDeleteMcpServer,
}: ExtensionsSettingsTabProps) {
  const { t } = useTranslation();
  const extensionCopy = {
    toolCount: (count: number) => t('settings.extensions.toolCount', { count }),
    connectionFailed: t('settings.extensions.connectionFailed'),
    availableTools: t('settings.extensions.availableTools'),
    toggleTools: t('settings.extensions.toggleTools'),
  };

  return (
    <>
      <Section
        icon={<Blocks size={20} />}
        title={t('settings.skills')}
        delay={0.03}
        description={t('settings.skillsDescription')}
        collapsible
        defaultOpen={showSkillForm || skills.length <= 6}
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
                              built-in
                            </Badge>
                          )}
                          <Badge variant="default" className="text-[10px] shrink-0">
                            ~{estimateTokens(skill.content)} tok
                          </Badge>
                          {!skill.enabled && !skill.builtin && (
                            <Badge variant="default" className="text-[10px] shrink-0 border-border text-text-tertiary">
                              {t('settings.skillFilterDisabled')}
                            </Badge>
                          )}
                        </div>
                        {skill.description ? (
                          <p className="mt-0.5 text-xs text-text-secondary line-clamp-2">
                            {skill.description}
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

      <Section
        icon={<Plug size={20} />}
        title={t('settings.mcpServers')}
        delay={0.06}
        description={t('settings.mcpServersDescription')}
        collapsible
        defaultOpen={showMcpForm || mcpServers.length <= 3}
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
            <div className="flex justify-end">
              <Button variant="primary" size="sm" icon={<Plus size={14} />} onClick={onAddMcpServer}>
                {t('settings.addMcpServer')}
              </Button>
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
                        <button
                          onClick={() => onEditMcpServer(server)}
                          className="rounded p-1.5 text-text-tertiary hover:text-accent hover:bg-accent/10 transition-colors cursor-pointer"
                          aria-label={t('common.edit')}
                        >
                          <Pencil size={14} />
                        </button>
                        {!server.builtinId && (
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
                          initial={{ height: 0, opacity: 0 }}
                          animate={{ height: 'auto', opacity: 1 }}
                          exit={{ height: 0, opacity: 0 }}
                          transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
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

      <ConfirmDialog
        open={!!deleteSkillTarget}
        onClose={() => onDeleteSkillTargetChange(null)}
        onConfirm={onConfirmDeleteSkill}
        title={t('common.delete')}
        message={t('settings.deleteSkillConfirm')}
        confirmText={t('common.delete')}
        variant="danger"
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
                    built-in
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
