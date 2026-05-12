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

const copy = {
  en: {
    title: 'Project tools',
    description:
      'Repository-declared shortcuts the agent can discover. Runs stay approval-gated and are bound to the exact manifest hash shown here.',
    refresh: 'Refresh',
    openManifest: 'Open manifest',
    showFolder: 'Show folder',
    tools: 'tools',
    runnable: 'runnable',
    issues: 'issues',
    noTools: 'No project tools found in registered sources.',
    manifestDirs: 'Manifest folders',
    command: 'Command',
    parameters: 'Inputs',
    noParameters: 'No inputs',
    sourceRoot: 'Source root',
    manifestPath: 'Manifest',
    manifestHash: 'Manifest hash',
    invalidManifests: 'Invalid manifests',
    invalidManifestsDesc: 'These files were found but cannot be used until the manifest is fixed.',
    reads: 'Reads files',
    writes: 'May edit files',
    executes: 'Runs command',
    network: 'May use network',
    metadataOnly: 'Metadata only',
    lowRisk: 'Low risk',
    mediumRisk: 'Medium risk',
    highRisk: 'High risk',
    warnings: 'Notes',
    metadataWarning: 'This manifest is descriptive only and cannot be run.',
    writeWarning: 'This tool declares write access and may modify source files.',
    networkWarning: 'This tool declares network access.',
    copiedError: 'Could not open path',
    loadError: 'Could not load project tools',
  },
  zh: {
    title: '项目工具',
    description:
      '仓库声明的可复用本地命令。运行时仍会请求审批，并且授权会绑定到这里显示的 manifest hash。',
    refresh: '刷新',
    openManifest: '打开 manifest',
    showFolder: '显示文件夹',
    tools: '个工具',
    runnable: '可运行',
    issues: '个问题',
    noTools: '已注册源码目录里还没有项目工具。',
    manifestDirs: 'Manifest 目录',
    command: '命令',
    parameters: '输入项',
    noParameters: '无需输入',
    sourceRoot: '源码根目录',
    manifestPath: 'Manifest',
    manifestHash: 'Manifest hash',
    invalidManifests: '无效 manifest',
    invalidManifestsDesc: '这些文件已被发现，但需要修正后才能被 agent 使用。',
    reads: '读取文件',
    writes: '可能改文件',
    executes: '执行命令',
    network: '可能联网',
    metadataOnly: '仅元数据',
    lowRisk: '低风险',
    mediumRisk: '中风险',
    highRisk: '高风险',
    warnings: '注意',
    metadataWarning: '这个 manifest 只用于说明，不能运行。',
    writeWarning: '这个工具声明了写入权限，可能会修改源码文件。',
    networkWarning: '这个工具声明了网络权限。',
    copiedError: '无法打开路径',
    loadError: '无法加载项目工具',
  },
};

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

function readableWarning(warning: string, text: typeof copy.en): string {
  if (warning.includes('metadata-only')) return text.metadataWarning;
  if (warning.includes('write access')) return text.writeWarning;
  if (warning.includes('network access')) return text.networkWarning;
  return warning;
}

export function ProjectToolsPanel() {
  const { locale } = useTranslation();
  const text = locale.startsWith('zh') ? copy.zh : copy.en;
  const [catalog, setCatalog] = useState<ProjectToolCatalog | null>(null);
  const [loading, setLoading] = useState(false);
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setCatalog(await api.listProjectTools());
    } catch (error) {
      toast.error(`${text.loadError}: ${String(error)}`);
    } finally {
      setLoading(false);
    }
  }, [text.loadError]);

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
      toast.error(`${text.copiedError}: ${String(error)}`);
    }
  }, [text.copiedError]);

  return (
    <Section
      icon={<Wrench size={20} />}
      title={text.title}
      delay={0.05}
      description={text.description}
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
              <div className="text-[11px] text-text-tertiary">{text.tools}</div>
            </div>
            <div className="rounded-lg bg-surface-2 px-3 py-2">
              <div className="text-lg font-semibold text-text-primary">{stats.runnable}</div>
              <div className="text-[11px] text-text-tertiary">{text.runnable}</div>
            </div>
            <div className="rounded-lg bg-surface-2 px-3 py-2">
              <div className="text-lg font-semibold text-text-primary">{stats.issues}</div>
              <div className="text-[11px] text-text-tertiary">{text.issues}</div>
            </div>
          </div>
          <Button
            variant="secondary"
            size="sm"
            icon={<RefreshCw size={14} />}
            loading={loading}
            onClick={() => void load()}
          >
            {text.refresh}
          </Button>
        </div>

        {catalog && catalog.manifestDirs.length > 0 && (
          <div className="flex flex-wrap items-center gap-2 text-xs text-text-tertiary">
            <span>{text.manifestDirs}</span>
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
            <p className="text-sm text-text-secondary">{text.noTools}</p>
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
                          {risk === 'high' ? text.highRisk : risk === 'medium' ? text.mediumRisk : text.lowRisk}
                        </Badge>
                        <Badge variant="default" className="font-mono text-[10px]">
                          {shortHash(tool.manifestHash)}
                        </Badge>
                        {!tool.runnable && (
                          <Badge variant="default" className="text-[10px]">
                            {text.metadataOnly}
                          </Badge>
                        )}
                      </div>
                      <p className="mt-1 line-clamp-2 text-xs leading-relaxed text-text-secondary">
                        {tool.description}
                      </p>
                      <div className="mt-2 flex flex-wrap gap-1.5">
                        {tool.access.read && <Badge variant="default" className="text-[10px]">{text.reads}</Badge>}
                        {tool.access.write && <Badge variant="danger" className="text-[10px]">{text.writes}</Badge>}
                        {tool.access.execute && <Badge variant="warning" className="text-[10px]">{text.executes}</Badge>}
                        {tool.access.network && <Badge variant="danger" className="text-[10px]">{text.network}</Badge>}
                      </div>
                    </div>
                    <div className="flex shrink-0 flex-wrap items-center gap-1 md:justify-end">
                      <Button
                        variant="ghost"
                        size="sm"
                        icon={<FileJson2 size={14} />}
                        onClick={() => void openPath(tool.manifestPath, false)}
                      >
                        {text.openManifest}
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        icon={<FolderOpen size={14} />}
                        onClick={() => void openPath(tool.manifestPath, true)}
                      >
                        {text.showFolder}
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
                        {text.command}
                      </div>
                      <code className="block overflow-hidden text-ellipsis whitespace-nowrap text-xs text-text-primary">
                        {command}
                      </code>
                    </div>
                  )}

                  <div className="mt-2 flex flex-wrap items-center gap-2 text-[11px] text-text-tertiary">
                    <span>{text.parameters}:</span>
                    {tool.parameterNames.length > 0 ? (
                      tool.parameterNames.map((name) => (
                        <code key={name} className="rounded border border-border/60 bg-surface-1 px-1.5 py-0.5 text-text-secondary">
                          {name}
                        </code>
                      ))
                    ) : (
                      <span>{text.noParameters}</span>
                    )}
                  </div>

                  {isExpanded && (
                    <div className="mt-3 space-y-2 rounded-md border border-border/60 bg-surface-1 p-3 text-xs">
                      <div className="grid gap-2 md:grid-cols-2">
                        <div>
                          <div className="text-[11px] text-text-tertiary">{text.sourceRoot}</div>
                          <div className="mt-0.5 truncate font-mono text-text-secondary" title={tool.sourceRoot}>
                            {compactPath(tool.sourceRoot)}
                          </div>
                        </div>
                        <div>
                          <div className="text-[11px] text-text-tertiary">{text.manifestPath}</div>
                          <div className="mt-0.5 truncate font-mono text-text-secondary" title={tool.manifestPath}>
                            {compactPath(tool.manifestPath)}
                          </div>
                        </div>
                      </div>
                      <div>
                        <div className="flex items-center gap-1.5 text-[11px] text-text-tertiary">
                          <Shield size={12} />
                          {text.manifestHash}
                        </div>
                        <div className="mt-0.5 break-all font-mono text-text-secondary">
                          {tool.manifestHash}
                        </div>
                      </div>
                      {tool.warnings.length > 0 && (
                        <div className="rounded-md border border-warning/25 bg-warning/10 px-3 py-2">
                          <div className="mb-1 flex items-center gap-1.5 text-[11px] font-medium text-warning">
                            <AlertTriangle size={12} />
                            {text.warnings}
                          </div>
                          <ul className="space-y-1 text-text-secondary">
                            {tool.warnings.map((warning) => (
                              <li key={warning}>{readableWarning(warning, text)}</li>
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
                <p className="text-sm font-medium text-text-primary">{text.invalidManifests}</p>
                <p className="text-xs text-text-tertiary">{text.invalidManifestsDesc}</p>
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
