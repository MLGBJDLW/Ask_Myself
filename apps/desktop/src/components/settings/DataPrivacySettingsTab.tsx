import { BarChart3, Database, Loader2, Pencil, Plus, RefreshCw, Save, Shield, Trash2, X, Zap } from 'lucide-react';
import { useTranslation } from '../../i18n';
import type { IndexStats } from '../../types/index-stats';
import type { PrivacyConfig, RedactRule } from '../../types/privacy';
import type { UserMemory } from '../../types/conversation';
import type { AgentTrace, TraceSummary } from '../../types/trace';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Section, StatCard } from './SettingsSection';

interface FtsProgress {
  operation?: string | null;
}

interface DataPrivacySettingsTabProps {
  analyticsLoading: boolean;
  traceSummary: TraceSummary | null;
  recentTraces: AgentTrace[];
  stats: IndexStats | null;
  rebuildLoading: boolean;
  optimizeLoading: boolean;
  clearCacheLoading: boolean;
  ftsProgress: FtsProgress | null;
  privacyConfig: PrivacyConfig | null;
  newPattern: string;
  newRule: RedactRule;
  userMemories: UserMemory[];
  editingMemoryId: string | null;
  editingMemoryDraft: string;
  memoryLoading: boolean;
  newMemory: string;
  memoryCharLimit: number;
  saveLoading: boolean;
  onRebuild: () => void;
  onOptimize: () => void;
  onClearCache: () => void;
  onNewPatternChange: (value: string) => void;
  onAddPattern: () => void;
  onRemovePattern: (index: number) => void;
  onNewRuleChange: (rule: RedactRule) => void;
  onAddRule: () => void;
  onRemoveRule: (index: number) => void;
  onEditingMemoryDraftChange: (value: string) => void;
  onStartEditMemory: (memory: UserMemory) => void;
  onCancelEditMemory: () => void;
  onUpdateMemory: () => void;
  onDeleteMemory: (id: string) => void;
  onNewMemoryChange: (value: string) => void;
  onAddMemory: () => void;
  onSavePrivacy: () => void;
}

function formatCompact(value: number): string {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)}K`;
  return String(value);
}

export function DataPrivacySettingsTab({
  analyticsLoading,
  traceSummary,
  recentTraces,
  stats,
  rebuildLoading,
  optimizeLoading,
  clearCacheLoading,
  ftsProgress,
  privacyConfig,
  newPattern,
  newRule,
  userMemories,
  editingMemoryId,
  editingMemoryDraft,
  memoryLoading,
  newMemory,
  memoryCharLimit,
  saveLoading,
  onRebuild,
  onOptimize,
  onClearCache,
  onNewPatternChange,
  onAddPattern,
  onRemovePattern,
  onNewRuleChange,
  onAddRule,
  onRemoveRule,
  onEditingMemoryDraftChange,
  onStartEditMemory,
  onCancelEditMemory,
  onUpdateMemory,
  onDeleteMemory,
  onNewMemoryChange,
  onAddMemory,
  onSavePrivacy,
}: DataPrivacySettingsTabProps) {
  const { t } = useTranslation();

  return (
    <>
      <Section
        icon={<BarChart3 size={20} />}
        title={t('analytics.title')}
        delay={0.02}
        collapsible
        defaultOpen={false}
        summary={traceSummary ? (
          <span className="rounded-full border border-border/60 bg-surface-2 px-2 py-1 text-[11px] text-text-secondary">
            {formatCompact(traceSummary.totalSessions)} {t('analytics.totalSessions')}
          </span>
        ) : undefined}
      >
        {analyticsLoading && !traceSummary ? (
          <div className="flex items-center gap-2 text-sm text-text-tertiary">
            <Loader2 size={14} className="animate-spin" />
            <span>{t('common.loading')}</span>
          </div>
        ) : traceSummary && traceSummary.totalSessions > 0 ? (
          <div className="space-y-5">
            <div className="grid grid-cols-3 gap-3">
              <StatCard label={t('analytics.totalSessions')} value={formatCompact(traceSummary.totalSessions)} />
              <StatCard label={t('analytics.successRate')} value={`${(traceSummary.successRate * 100).toFixed(1)}%`} />
              <StatCard label={t('analytics.cacheHitRate')} value={`${(traceSummary.cacheHitRate * 100).toFixed(1)}%`} />
              <StatCard label={t('analytics.totalTokens')} value={formatCompact(traceSummary.totalInputTokens + traceSummary.totalOutputTokens)} />
              <StatCard label={t('analytics.avgIterations')} value={traceSummary.avgIterationsPerSession.toFixed(1)} />
              <StatCard label={t('analytics.avgContextUsage')} value={`${(traceSummary.avgContextUsagePct * 100).toFixed(1)}%`} />
              <StatCard label={t('analytics.sessionsLast7Days')} value={formatCompact(traceSummary.sessionsLast7Days)} />
              <StatCard label={t('analytics.tokensLast7Days')} value={formatCompact(traceSummary.tokensLast7Days)} />
            </div>

            {traceSummary.topTools.length > 0 && (
              <div>
                <h3 className="mb-2 text-sm font-medium text-text-primary">{t('analytics.topTools')}</h3>
                <div className="overflow-hidden rounded-lg border border-border">
                  <table className="w-full text-sm">
                    <thead>
                      <tr className="border-b border-border bg-surface-2">
                        <th className="px-3 py-2 text-left text-xs font-medium text-text-tertiary">{t('analytics.toolName')}</th>
                        <th className="px-3 py-2 text-right text-xs font-medium text-text-tertiary">{t('analytics.count')}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {traceSummary.topTools.map(([name, count]) => (
                        <tr key={name} className="border-b border-border last:border-0 hover:bg-surface-2/50 transition-colors">
                          <td className="px-3 py-1.5 font-mono text-xs text-text-primary">{name}</td>
                          <td className="px-3 py-1.5 text-right text-text-secondary">{formatCompact(count)}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </div>
            )}

            {recentTraces.length > 0 && (
              <div>
                <h3 className="mb-2 text-sm font-medium text-text-primary">{t('analytics.recentSessions')}</h3>
                <div className="overflow-hidden rounded-lg border border-border">
                  <table className="w-full text-sm">
                    <thead>
                      <tr className="border-b border-border bg-surface-2">
                        <th className="px-3 py-2 text-left text-xs font-medium text-text-tertiary">{t('analytics.message')}</th>
                        <th className="px-3 py-2 text-left text-xs font-medium text-text-tertiary">{t('analytics.outcome')}</th>
                        <th className="px-3 py-2 text-right text-xs font-medium text-text-tertiary">{t('analytics.iterations')}</th>
                        <th className="px-3 py-2 text-right text-xs font-medium text-text-tertiary">{t('analytics.tools')}</th>
                        <th className="px-3 py-2 text-right text-xs font-medium text-text-tertiary">{t('analytics.tokens')}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {recentTraces.map((trace) => (
                        <tr key={trace.id} className="border-b border-border last:border-0 hover:bg-surface-2/50 transition-colors">
                          <td className="px-3 py-1.5 text-text-primary truncate" style={{ maxWidth: 200 }} title={trace.userMessagePreview}>{trace.userMessagePreview || '—'}</td>
                          <td className="px-3 py-1.5">
                            <Badge variant={trace.outcome === 'success' ? 'success' : trace.outcome === 'error' ? 'danger' : 'default'}>
                              {trace.outcome}
                            </Badge>
                          </td>
                          <td className="px-3 py-1.5 text-right text-text-secondary">{trace.totalIterations}</td>
                          <td className="px-3 py-1.5 text-right text-text-secondary">{trace.totalToolCalls}</td>
                          <td className="px-3 py-1.5 text-right text-text-secondary">{formatCompact(trace.totalInputTokens + trace.totalOutputTokens)}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </div>
            )}
          </div>
        ) : (
          <p className="text-sm text-text-tertiary">{t('analytics.noData')}</p>
        )}
      </Section>

      <Section
        icon={<Database size={20} />}
        title={t('settings.indexSection')}
        delay={0.05}
        collapsible
        defaultOpen
        summary={stats ? (
          <span className="rounded-full border border-border/60 bg-surface-2 px-2 py-1 text-[11px] text-text-secondary">
            {stats.totalDocuments} {t('settings.totalDocs')}
          </span>
        ) : undefined}
      >
        <div className="mb-5 grid grid-cols-3 gap-3">
          <StatCard label={t('settings.totalDocs')} value={stats?.totalDocuments ?? '—'} />
          <StatCard label={t('settings.totalChunks')} value={stats?.totalChunks ?? '—'} />
          <StatCard label={t('settings.ftsEntries')} value={stats?.ftsRows ?? '—'} />
        </div>

        <div className="flex items-center gap-3">
          <Button variant="secondary" size="sm" icon={<RefreshCw size={14} />} loading={rebuildLoading} onClick={onRebuild}>
            {t('settings.rebuildIndex')}
          </Button>
          <Button variant="secondary" size="sm" icon={<Zap size={14} />} loading={optimizeLoading} onClick={onOptimize}>
            {t('settings.optimizeIndex')}
          </Button>
          <Button variant="secondary" size="sm" icon={<Trash2 size={14} />} loading={clearCacheLoading} onClick={onClearCache}>
            {t('settings.clearCache')}
          </Button>
        </div>
        {ftsProgress && (
          <div className="mt-2">
            <div className="flex items-center gap-2 text-xs text-muted">
              <RefreshCw size={12} className="animate-spin" />
              <span>{ftsProgress.operation === 'rebuild-fts' ? t('settings.rebuildingIndex') : t('settings.optimizingIndex')}</span>
            </div>
            <div className="w-full bg-surface-3 rounded h-1 mt-1 overflow-hidden">
              <div className="bg-accent h-1 rounded animate-pulse w-full" />
            </div>
          </div>
        )}
      </Section>

      <Section
        icon={<Shield size={20} />}
        title={t('settings.privacySection')}
        delay={0.1}
        collapsible
        defaultOpen={false}
        summary={privacyConfig ? (
          <span className="rounded-full border border-border/60 bg-surface-2 px-2 py-1 text-[11px] text-text-secondary">
            {privacyConfig.excludePatterns.length + privacyConfig.redactPatterns.length} {t('settings.redactRules')}
          </span>
        ) : undefined}
      >
        {privacyConfig && (
          <div className="space-y-6">
            <div>
              <h3 className="mb-2 text-sm font-medium text-text-primary">{t('settings.excludePatterns')}</h3>
              <p className="mb-3 text-xs text-text-tertiary">{t('settings.excludePatternsDesc')}</p>

              {privacyConfig.excludePatterns.length > 0 && (
                <div className="mb-3 flex flex-wrap gap-2">
                  {privacyConfig.excludePatterns.map((pattern, index) => (
                    <Badge key={index} variant="default" className="gap-1.5 pl-2.5 pr-1.5 py-1">
                      <span className="font-mono text-[11px]">{pattern}</span>
                      <button
                        onClick={() => onRemovePattern(index)}
                        className="ml-0.5 rounded hover:bg-surface-4 p-0.5 text-text-tertiary hover:text-danger transition-colors cursor-pointer"
                        aria-label={`${t('common.remove')} ${pattern}`}
                      >
                        <Trash2 size={12} />
                      </button>
                    </Badge>
                  ))}
                </div>
              )}

              <div className="flex gap-2">
                <Input
                  placeholder="*.log, .git/**, node_modules/**"
                  value={newPattern}
                  onChange={(event) => onNewPatternChange(event.target.value)}
                  onKeyDown={(event) => { if (event.key === 'Enter') onAddPattern(); }}
                  className="flex-1"
                />
                <Button variant="ghost" size="md" icon={<Plus size={16} />} onClick={onAddPattern} disabled={!newPattern.trim()}>
                  {t('settings.addPattern')}
                </Button>
              </div>
            </div>

            <div>
              <h3 className="mb-2 text-sm font-medium text-text-primary">{t('settings.redactRules')}</h3>
              <p className="mb-3 text-xs text-text-tertiary">{t('settings.redactRulesDesc')}</p>

              {privacyConfig.redactPatterns.length > 0 && (
                <div className="mb-3 overflow-hidden rounded-lg border border-border">
                  <table className="w-full text-sm">
                    <thead>
                      <tr className="border-b border-border bg-surface-2">
                        <th className="px-3 py-2 text-left text-xs font-medium text-text-tertiary">{t('settings.ruleName')}</th>
                        <th className="px-3 py-2 text-left text-xs font-medium text-text-tertiary">{t('settings.rulePattern')}</th>
                        <th className="px-3 py-2 text-left text-xs font-medium text-text-tertiary">{t('settings.ruleReplacement')}</th>
                        <th className="w-10 px-3 py-2" />
                      </tr>
                    </thead>
                    <tbody>
                      {privacyConfig.redactPatterns.map((rule, index) => (
                        <tr key={index} className="border-b border-border last:border-0 hover:bg-surface-2/50 transition-colors">
                          <td className="px-3 py-2 text-text-primary">{rule.name}</td>
                          <td className="px-3 py-2 font-mono text-xs text-text-secondary">{rule.pattern}</td>
                          <td className="px-3 py-2 font-mono text-xs text-text-secondary">{rule.replacement}</td>
                          <td className="px-3 py-2 text-right">
                            <button
                              onClick={() => onRemoveRule(index)}
                              className="rounded p-1 text-text-tertiary hover:text-danger hover:bg-danger/10 transition-colors cursor-pointer"
                              aria-label={`${t('common.delete')} ${rule.name}`}
                            >
                              <Trash2 size={14} />
                            </button>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}

              <div className="flex gap-2">
                <Input
                  placeholder={t('settings.ruleName')}
                  value={newRule.name}
                  onChange={(event) => onNewRuleChange({ ...newRule, name: event.target.value })}
                  className="flex-1"
                />
                <Input
                  placeholder={t('settings.rulePattern')}
                  value={newRule.pattern}
                  onChange={(event) => onNewRuleChange({ ...newRule, pattern: event.target.value })}
                  className="flex-1"
                />
                <Input
                  placeholder={t('settings.ruleReplacement')}
                  value={newRule.replacement}
                  onChange={(event) => onNewRuleChange({ ...newRule, replacement: event.target.value })}
                  className="flex-1"
                />
                <Button variant="ghost" size="md" icon={<Plus size={16} />} onClick={onAddRule} disabled={!newRule.name.trim() || !newRule.pattern.trim()}>
                  {t('settings.addRule')}
                </Button>
              </div>
            </div>

            <div>
              <h3 className="mb-2 text-sm font-medium text-text-primary">{t('settings.memorySection')}</h3>
              <p className="mb-3 text-xs text-text-tertiary">{t('settings.memoryDescription')}</p>

              <div className="space-y-2 mb-3">
                {userMemories.length === 0 && (
                  <div className="rounded-md border border-dashed border-border px-3 py-2 text-xs text-text-tertiary">
                    {t('settings.memoryEmpty')}
                  </div>
                )}
                {userMemories.map((memory) => (
                  <div key={memory.id} className="flex items-start gap-2 rounded-md border border-border bg-surface-2 px-3 py-2">
                    {editingMemoryId === memory.id ? (
                      <div className="flex-1 space-y-2">
                        <Input
                          value={editingMemoryDraft}
                          onChange={(event) => onEditingMemoryDraftChange(event.target.value)}
                          maxLength={memoryCharLimit}
                          disabled={memoryLoading}
                          className="w-full"
                        />
                        <div className="flex items-center justify-between gap-2">
                          <p className="text-xs text-text-tertiary">
                            {editingMemoryDraft.length}/{memoryCharLimit}
                          </p>
                          <div className="flex items-center gap-1">
                            <button
                              type="button"
                              onClick={onUpdateMemory}
                              disabled={!editingMemoryDraft.trim() || memoryLoading}
                              className="rounded p-1 text-text-tertiary hover:text-accent hover:bg-accent/10 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                              aria-label={t('common.save')}
                            >
                              <Save size={14} />
                            </button>
                            <button
                              type="button"
                              onClick={onCancelEditMemory}
                              disabled={memoryLoading}
                              className="rounded p-1 text-text-tertiary hover:text-text-primary hover:bg-surface-3 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                              aria-label={t('common.cancel')}
                            >
                              <X size={14} />
                            </button>
                          </div>
                        </div>
                      </div>
                    ) : (
                      <>
                        <p className="flex-1 text-sm text-text-primary whitespace-pre-wrap" style={{ overflowWrap: 'break-word' }}>
                          {memory.content}
                        </p>
                        <div className="flex items-center gap-1">
                          <button
                            type="button"
                            onClick={() => onStartEditMemory(memory)}
                            disabled={memoryLoading}
                            className="mt-0.5 rounded p-1 text-text-tertiary hover:text-accent hover:bg-accent/10 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                            aria-label={t('common.edit')}
                          >
                            <Pencil size={14} />
                          </button>
                          <button
                            type="button"
                            onClick={() => onDeleteMemory(memory.id)}
                            disabled={memoryLoading}
                            className="mt-0.5 rounded p-1 text-text-tertiary hover:text-danger hover:bg-danger/10 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                            aria-label={t('common.delete')}
                          >
                            <Trash2 size={14} />
                          </button>
                        </div>
                      </>
                    )}
                  </div>
                ))}
              </div>

              <div className="flex gap-2">
                <Input
                  placeholder={t('settings.memoryPlaceholder')}
                  value={newMemory}
                  onChange={(event) => onNewMemoryChange(event.target.value)}
                  maxLength={memoryCharLimit}
                  onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); onAddMemory(); } }}
                  className="flex-1"
                />
                <Button variant="ghost" size="md" icon={<Plus size={16} />} onClick={onAddMemory} loading={memoryLoading} disabled={!newMemory.trim()}>
                  {t('settings.addMemory')}
                </Button>
              </div>
              <p className="mt-2 text-xs text-text-tertiary">
                {t('settings.memoryCharHelper', { length: String(newMemory.length), limit: String(memoryCharLimit) })}
              </p>
            </div>

            <div className="flex justify-end border-t border-border pt-4">
              <Button variant="primary" size="md" icon={<Save size={16} />} loading={saveLoading} onClick={onSavePrivacy}>
                {t('settings.saveConfig')}
              </Button>
            </div>
          </div>
        )}
      </Section>
    </>
  );
}
