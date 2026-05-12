import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  AlertTriangle,
  ChevronDown,
  FileJson2,
  FolderOpen,
  RefreshCw,
  Shield,
  Terminal,
  Wrench,
} from 'lucide-react';
import { toast } from 'sonner';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import type { ProjectToolCatalog, ProjectToolSummary } from '../../types/project-tool';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { Section } from './SettingsSection';

function shortHash(hash: string): string {
  return hash.length > 12 ? hash.slice(0, 12) : hash;
}

function compactPath(path: string, max = 92): string {
  if (path.length <= max) return path;
  return `...${path.slice(-(max - 3))}`;
}

function riskFor(tool: ProjectToolSummary): 'low' | 'medium' | 'high' {
  if (tool.access.write || tool.access.network) return 'high';
  if (tool.runnable || tool.access.execute) return 'medium';
  return 'low';
}

function badgeVariantForRisk(risk: 'low' | 'medium' | 'high') {
  if (risk === 'high') return 'danger' as const;
  if (risk === 'medium') return 'warning' as const;
  return 'success' as const;
}

function formatCommand(tool: ProjectToolSummary): string {
  if (tool.commandPreview) return tool.commandPreview;
  if (!tool.command) return '';
  return [tool.command.program, ...(tool.command.args ?? [])].join(' ');
}

function readableWarning(warning: string, t: ReturnType<typeof useTranslation>['t']): string {
  if (warning.includes('metadata-only')) return t('settings.projectTools.metadataWarning');
  if (warning.includes('write access')) return t('settings.projectTools.writeWarning');
  if (warning.includes('network access')) return t('settings.projectTools.networkWarning');
  return warning;
}

export function ProjectToolsPanel() {
  const { t } = useTranslation();
  const [catalog, setCatalog] = useState<ProjectToolCatalog | null>(null);
  const [loading, setLoading] = useState(false);
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setCatalog(await api.listProjectTools());
    } catch (error) {
      toast.error(`${t('settings.projectTools.loadError')}: ${String(error)}`);
    } finally {
      setLoading(false);
    }
  }, [t]);

  useEffect(() => {
    void load();
  }, [load]);

  const stats = useMemo(() => {
    const tools = catalog?.tools ?? [];
    return {
      total: tools.length,
      runnable: tools.filter((tool) => tool.runnable).length,
      issues: catalog?.errors.length ?? 0,
    };
  }, [catalog]);

  const openPath = useCallback(async (path: string, reveal: boolean) => {
    try {
      if (reveal) {
        await api.showInFileExplorer(path);
      } else {
        await api.openFileInDefaultApp(path);
      }
    } catch (error) {
      toast.error(`${t('settings.projectTools.openPathError')}: ${String(error)}`);
    }
  }, [t]);

  return (
    <Section
      icon={<Wrench size={20} />}
      title={t('settings.projectTools.title')}
      delay={0.05}
      description={t('settings.projectTools.description')}
      collapsible
      defaultOpen={false}
      summary={
        <div className="flex items-center gap-1.5">
          <span className="rounded-full border border-border/60 bg-surface-2 px-2 py-1 text-[11px] text-text-secondary">
            {stats.total}
          </span>
          {stats.issues > 0 && (
            <span className="rounded-full border border-danger/30 bg-danger/10 px-2 py-1 text-[11px] text-danger">
              {stats.issues}
            </span>
          )}
        </div>
      }
    >
      <div className="space-y-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="grid min-w-0 flex-1 grid-cols-3 gap-2">
            <div className="rounded-lg bg-surface-2 px-3 py-2">
              <div className="text-lg font-semibold text-text-primary">{stats.total}</div>
              <div className="text-[11px] text-text-tertiary">{t('settings.projectTools.tools')}</div>
            </div>
            <div className="rounded-lg bg-surface-2 px-3 py-2">
              <div className="text-lg font-semibold text-text-primary">{stats.runnable}</div>
              <div className="text-[11px] text-text-tertiary">{t('settings.projectTools.runnable')}</div>
            </div>
            <div className="rounded-lg bg-surface-2 px-3 py-2">
              <div className="text-lg font-semibold text-text-primary">{stats.issues}</div>
              <div className="text-[11px] text-text-tertiary">{t('settings.projectTools.issues')}</div>
            </div>
          </div>
          <Button
            variant="secondary"
            size="sm"
            icon={<RefreshCw size={14} />}
            loading={loading}
            onClick={() => void load()}
          >
            {t('settings.toolApprovalRefresh')}
          </Button>
        </div>

        {catalog && catalog.manifestDirs.length > 0 && (
          <div className="flex flex-wrap items-center gap-2 text-xs text-text-tertiary">
            <span>{t('settings.projectTools.manifestDirs')}</span>
            {catalog.manifestDirs.map((dir) => (
              <code key={dir} className="rounded border border-border bg-surface-2 px-1.5 py-0.5 text-[11px] text-text-secondary">
                {dir}
              </code>
            ))}
          </div>
        )}

        {stats.total === 0 ? (
          <div className="rounded-lg border border-dashed border-border bg-surface-2/60 px-4 py-8 text-center">
            <FileJson2 size={28} className="mx-auto mb-2 text-text-tertiary" />
            <p className="text-sm text-text-secondary">{t('settings.projectTools.noTools')}</p>
          </div>
        ) : (
          <div className="space-y-3">
            {catalog?.tools.map((tool) => {
              const key = `${tool.manifestPath}:${tool.manifestHash}`;
              const isExpanded = expanded[key] ?? false;
              const risk = riskFor(tool);
              const command = formatCommand(tool);
              return (
                <div
                  key={key}
                  className="rounded-lg border border-border bg-surface-2 p-4 transition-colors hover:bg-surface-3/50"
                >
                  <div className="flex flex-col gap-3 md:flex-row md:items-start md:justify-between">
                    <div className="min-w-0 flex-1">
                      <div className="flex min-w-0 flex-wrap items-center gap-2">
                        <p className="truncate text-sm font-medium text-text-primary">{tool.name}</p>
                        <Badge variant={badgeVariantForRisk(risk)} className="text-[10px]">
                          {risk === 'high'
                            ? t('settings.toolRiskHigh')
                            : risk === 'medium'
                              ? t('settings.toolRiskMedium')
                              : t('settings.toolRiskLow')}
                        </Badge>
                        <Badge variant="default" className="font-mono text-[10px]">
                          {shortHash(tool.manifestHash)}
                        </Badge>
                        {!tool.runnable && (
                          <Badge variant="default" className="text-[10px]">
                            {t('settings.projectTools.metadataOnly')}
                          </Badge>
                        )}
                      </div>
                      <p className="mt-1 line-clamp-2 text-xs leading-relaxed text-text-secondary">
                        {tool.description}
                      </p>
                      <div className="mt-2 flex flex-wrap gap-1.5">
                        {tool.access.read && <Badge variant="default" className="text-[10px]">{t('settings.projectTools.reads')}</Badge>}
                        {tool.access.write && <Badge variant="danger" className="text-[10px]">{t('settings.projectTools.writes')}</Badge>}
                        {tool.access.execute && <Badge variant="warning" className="text-[10px]">{t('settings.projectTools.executes')}</Badge>}
                        {tool.access.network && <Badge variant="danger" className="text-[10px]">{t('settings.projectTools.network')}</Badge>}
                      </div>
                    </div>
                    <div className="flex shrink-0 flex-wrap items-center gap-1 md:justify-end">
                      <Button
                        variant="ghost"
                        size="sm"
                        icon={<FileJson2 size={14} />}
                        onClick={() => void openPath(tool.manifestPath, false)}
                      >
                        {t('settings.projectTools.openManifest')}
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        icon={<FolderOpen size={14} />}
                        onClick={() => void openPath(tool.manifestPath, true)}
                      >
                        {t('settings.projectTools.showFolder')}
                      </Button>
                      <button
                        type="button"
                        onClick={() => setExpanded((prev) => ({ ...prev, [key]: !isExpanded }))}
                        className="rounded p-1.5 text-text-tertiary transition-colors hover:bg-accent/10 hover:text-accent"
                        aria-expanded={isExpanded}
                      >
                        <ChevronDown size={15} className={`transition-transform ${isExpanded ? 'rotate-180' : ''}`} />
                      </button>
                    </div>
                  </div>

                  {command && (
                    <div className="mt-3 rounded-md border border-border/60 bg-surface-1 px-3 py-2">
                      <div className="mb-1 flex items-center gap-1.5 text-[11px] uppercase text-text-tertiary">
                        <Terminal size={12} />
                        {t('settings.projectTools.command')}
                      </div>
                      <code className="block overflow-hidden text-ellipsis whitespace-nowrap text-xs text-text-primary">
                        {command}
                      </code>
                    </div>
                  )}

                  <div className="mt-2 flex flex-wrap items-center gap-2 text-[11px] text-text-tertiary">
                    <span>{t('settings.projectTools.parameters')}:</span>
                    {tool.parameterNames.length > 0 ? (
                      tool.parameterNames.map((name) => (
                        <code key={name} className="rounded border border-border/60 bg-surface-1 px-1.5 py-0.5 text-text-secondary">
                          {name}
                        </code>
                      ))
                    ) : (
                      <span>{t('settings.projectTools.noParameters')}</span>
                    )}
                  </div>

                  {isExpanded && (
                    <div className="mt-3 space-y-2 rounded-md border border-border/60 bg-surface-1 p-3 text-xs">
                      <div className="grid gap-2 md:grid-cols-2">
                        <div>
                          <div className="text-[11px] text-text-tertiary">{t('settings.projectTools.sourceRoot')}</div>
                          <div className="mt-0.5 truncate font-mono text-text-secondary" title={tool.sourceRoot}>
                            {compactPath(tool.sourceRoot)}
                          </div>
                        </div>
                        <div>
                          <div className="text-[11px] text-text-tertiary">{t('settings.projectTools.manifestPath')}</div>
                          <div className="mt-0.5 truncate font-mono text-text-secondary" title={tool.manifestPath}>
                            {compactPath(tool.manifestPath)}
                          </div>
                        </div>
                      </div>
                      <div>
                        <div className="flex items-center gap-1.5 text-[11px] text-text-tertiary">
                          <Shield size={12} />
                          {t('settings.projectTools.manifestHash')}
                        </div>
                        <div className="mt-0.5 break-all font-mono text-text-secondary">
                          {tool.manifestHash}
                        </div>
                      </div>
                      {tool.warnings.length > 0 && (
                        <div className="rounded-md border border-warning/25 bg-warning/10 px-3 py-2">
                          <div className="mb-1 flex items-center gap-1.5 text-[11px] font-medium text-warning">
                            <AlertTriangle size={12} />
                            {t('settings.projectTools.warnings')}
                          </div>
                          <ul className="space-y-1 text-text-secondary">
                            {tool.warnings.map((warning) => (
                              <li key={warning}>{readableWarning(warning, t)}</li>
                            ))}
                          </ul>
                        </div>
                      )}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}

        {catalog && catalog.errors.length > 0 && (
          <div className="rounded-lg border border-danger/25 bg-danger/10 p-3">
            <div className="mb-2 flex items-center gap-2">
              <AlertTriangle size={15} className="text-danger" />
              <div>
                <p className="text-sm font-medium text-text-primary">{t('settings.projectTools.invalidManifests')}</p>
                <p className="text-xs text-text-tertiary">{t('settings.projectTools.invalidManifestsDesc')}</p>
              </div>
            </div>
            <div className="space-y-2">
              {catalog.errors.map((error) => (
                <div key={error.path} className="rounded-md border border-border/60 bg-surface-1 px-3 py-2">
                  <div className="truncate font-mono text-xs text-text-primary" title={error.path}>
                    {compactPath(error.path)}
                  </div>
                  <div className="mt-1 text-xs text-danger">{error.message}</div>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>
    </Section>
  );
}
