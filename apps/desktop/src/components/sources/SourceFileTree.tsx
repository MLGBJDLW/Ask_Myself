import { useCallback, useEffect, useState, type ReactNode } from 'react';
import {
  AlertTriangle,
  ChevronRight,
  ExternalLink,
  FileText,
  Folder,
  FolderOpen,
  LocateFixed,
  RefreshCw,
} from 'lucide-react';
import { toast } from 'sonner';
import * as api from '../../lib/api';
import { canPreviewInApp, useFilePreview } from '../../lib/filePreviewContext';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';

interface SourceFileTreeProps {
  sourceId: string;
}

function formatBytes(bytes?: number | null): string {
  if (!bytes || bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 10 || unit === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[unit]}`;
}

export function SourceFileTree({ sourceId }: SourceFileTreeProps) {
  const { openFilePreview } = useFilePreview();
  const [childrenByPath, setChildrenByPath] = useState<Record<string, api.SourceTreeNode[]>>({});
  const [truncatedByPath, setTruncatedByPath] = useState<Record<string, boolean>>({});
  const [expandedPaths, setExpandedPaths] = useState<Set<string>>(new Set());
  const [loadingPaths, setLoadingPaths] = useState<Set<string>>(new Set(['']));
  const [error, setError] = useState<string | null>(null);

  const loadPath = useCallback(async (relativePath = '', force = false) => {
    if (!force && childrenByPath[relativePath]) return;
    setLoadingPaths((prev) => new Set(prev).add(relativePath));
    setError(null);
    try {
      const tree = await api.listSourceTree(sourceId, relativePath, 1, 300);
      setChildrenByPath((prev) => ({ ...prev, [relativePath]: tree.nodes }));
      setTruncatedByPath((prev) => ({ ...prev, [relativePath]: tree.truncated }));
    } catch (e) {
      const message = String(e);
      setError(message);
      toast.error(message);
    } finally {
      setLoadingPaths((prev) => {
        const next = new Set(prev);
        next.delete(relativePath);
        return next;
      });
    }
  }, [childrenByPath, sourceId]);

  useEffect(() => {
    setChildrenByPath({});
    setTruncatedByPath({});
    setExpandedPaths(new Set());
    setLoadingPaths(new Set(['']));
    setError(null);
    void loadPath('', true);
  }, [sourceId]);

  const refresh = () => {
    setChildrenByPath({});
    setTruncatedByPath({});
    setExpandedPaths(new Set());
    void loadPath('', true);
  };

  const toggleDirectory = async (node: api.SourceTreeNode) => {
    const next = new Set(expandedPaths);
    if (next.has(node.relativePath)) {
      next.delete(node.relativePath);
      setExpandedPaths(next);
      return;
    }
    next.add(node.relativePath);
    setExpandedPaths(next);
    await loadPath(node.relativePath);
  };

  const openFile = (node: api.SourceTreeNode) => {
    if (canPreviewInApp(node.path)) {
      openFilePreview(node.path);
    } else {
      void api.openFileInDefaultApp(node.path);
    }
  };

  const renderNodes = (nodes: api.SourceTreeNode[], level: number): ReactNode => (
    <div className={level === 0 ? 'space-y-0.5' : 'space-y-0.5'}>
      {nodes.map((node) => {
        const isDirectory = node.kind === 'directory';
        const isExpanded = expandedPaths.has(node.relativePath);
        const childNodes = childrenByPath[node.relativePath] ?? [];
        const loading = loadingPaths.has(node.relativePath);
        return (
          <div key={node.relativePath || node.path}>
            <div
              className="group flex min-h-8 items-center gap-2 rounded-md px-2 py-1 text-xs text-text-secondary hover:bg-surface-1 hover:text-text-primary"
              style={{ paddingLeft: 8 + level * 14 }}
            >
              {isDirectory ? (
                <button
                  type="button"
                  onClick={() => void toggleDirectory(node)}
                  className="flex h-5 w-5 shrink-0 items-center justify-center rounded text-text-tertiary hover:bg-surface-3 hover:text-text-primary"
                  aria-label={isExpanded ? '收起文件夹' : '展开文件夹'}
                  title={isExpanded ? '收起文件夹' : '展开文件夹'}
                >
                  <ChevronRight size={13} className={`transition-transform ${isExpanded ? 'rotate-90' : ''}`} />
                </button>
              ) : (
                <span className="h-5 w-5 shrink-0" />
              )}

              {isDirectory ? (
                isExpanded ? <FolderOpen size={14} className="shrink-0 text-accent" /> : <Folder size={14} className="shrink-0 text-accent" />
              ) : (
                <FileText size={14} className="shrink-0 text-text-tertiary" />
              )}

              <button
                type="button"
                onClick={() => isDirectory ? void toggleDirectory(node) : openFile(node)}
                className="min-w-0 flex-1 truncate text-left font-mono"
                title={node.path}
              >
                {node.name}
              </button>

              {!isDirectory && (
                <>
                  <span className="hidden shrink-0 text-[10px] text-text-tertiary sm:inline">{formatBytes(node.sizeBytes)}</span>
                  <Badge variant={node.indexed ? 'success' : 'default'}>
                    {node.indexed ? '已索引' : '未索引'}
                  </Badge>
                  {node.chunkCount ? (
                    <span className="hidden shrink-0 text-[10px] text-text-tertiary sm:inline">
                      {node.chunkCount} chunks
                    </span>
                  ) : null}
                  <div className="flex shrink-0 items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100">
                    <Button
                      variant="ghost"
                      size="sm"
                      iconOnly
                      title="预览或打开"
                      aria-label="预览或打开"
                      icon={<ExternalLink size={13} />}
                      onClick={() => openFile(node)}
                    />
                    <Button
                      variant="ghost"
                      size="sm"
                      iconOnly
                      title="在文件管理器中显示"
                      aria-label="在文件管理器中显示"
                      icon={<LocateFixed size={13} />}
                      onClick={() => void api.showInFileExplorer(node.path)}
                    />
                  </div>
                </>
              )}
            </div>

            {isDirectory && isExpanded && (
              <div>
                {loading ? (
                  <div
                    className="flex h-8 items-center gap-2 px-2 text-xs text-text-tertiary"
                    style={{ paddingLeft: 28 + (level + 1) * 14 }}
                  >
                    <RefreshCw size={12} className="animate-spin" />
                    加载中
                  </div>
                ) : childNodes.length > 0 ? (
                  renderNodes(childNodes, level + 1)
                ) : (
                  <div
                    className="h-8 px-2 py-2 text-xs text-text-tertiary"
                    style={{ paddingLeft: 28 + (level + 1) * 14 }}
                  >
                    空文件夹
                  </div>
                )}
                {truncatedByPath[node.relativePath] && (
                  <div
                    className="px-2 py-1 text-[11px] text-amber-500"
                    style={{ paddingLeft: 28 + (level + 1) * 14 }}
                  >
                    当前目录结果已截断，请进入更深目录继续查看。
                  </div>
                )}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );

  const rootNodes = childrenByPath[''] ?? [];
  const rootLoading = loadingPaths.has('');

  return (
    <div className="mt-3 rounded-md border border-border bg-surface-1">
      <div className="flex items-center justify-between border-b border-border px-3 py-2">
        <div className="flex items-center gap-2 text-xs font-medium text-text-secondary">
          <Folder size={14} className="text-accent" />
          文件树
        </div>
        <Button
          variant="ghost"
          size="sm"
          iconOnly
          title="刷新文件树"
          aria-label="刷新文件树"
          icon={<RefreshCw size={13} />}
          loading={rootLoading}
          onClick={refresh}
        />
      </div>

      {error && (
        <div className="flex items-center gap-2 border-b border-border px-3 py-2 text-xs text-amber-500">
          <AlertTriangle size={13} />
          <span className="min-w-0 truncate">{error}</span>
        </div>
      )}

      <div className="max-h-80 overflow-auto p-1.5">
        {rootLoading && rootNodes.length === 0 ? (
          <div className="flex h-16 items-center justify-center gap-2 text-xs text-text-tertiary">
            <RefreshCw size={13} className="animate-spin" />
            加载文件树
          </div>
        ) : rootNodes.length > 0 ? (
          renderNodes(rootNodes, 0)
        ) : (
          <div className="flex h-16 items-center justify-center text-xs text-text-tertiary">
            没有匹配当前包含/排除规则的文件。
          </div>
        )}
        {truncatedByPath[''] && (
          <div className="px-2 py-1 text-[11px] text-amber-500">
            根目录结果已截断，请展开子文件夹继续缩小范围。
          </div>
        )}
      </div>
    </div>
  );
}
