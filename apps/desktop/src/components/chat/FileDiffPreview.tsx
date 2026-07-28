import { useEffect, useRef, useState } from 'react';
import { ChevronDown, ChevronRight, FileCode2, FilePenLine, FilePlus2 } from 'lucide-react';
import { useTranslation } from '../../i18n';
import type { ArtifactPayload } from '../../types/conversation';
import { FileBadge, isAbsoluteFileSystemPath } from '../ui/FileBadge';
import { DiffStatsTicker } from './DiffStatsTicker';

type FileDiffLineType = 'context' | 'addition' | 'deletion';

interface FileDiffLine {
  type: FileDiffLineType;
  oldLine: number | null;
  newLine: number | null;
  content: string;
}

interface FileDiffHunk {
  oldStart: number;
  newStart: number;
  oldLines: number;
  newLines: number;
  lines: FileDiffLine[];
}

export interface FileDiffArtifact {
  path: string;
  absolutePath?: string;
  operation: string;
  additions: number;
  deletions: number;
  truncated?: boolean;
  omittedLineCount?: number;
  hunks: FileDiffHunk[];
}

export interface DiffStatsArtifact {
  kind: 'diffStats';
  filesChanged: number;
  additions: number;
  deletions: number;
  hunks: number;
  replacements?: number;
  operation: string;
  paths: string[];
}

function diffPathKey(path: string): string {
  const raw = path.trim();
  if (!raw) return '';

  let normalized = raw;
  if (/^file:\/\//i.test(normalized)) {
    normalized = normalized.replace(/^file:\/\//i, '');
    if (/^\/[A-Za-z]:($|\/)/.test(normalized)) {
      normalized = normalized.slice(1);
    }
  }

  normalized = normalized
    .replace(/\\/g, '/')
    .replace(/\/{2,}/g, '/');

  const driveMatch = normalized.match(/^([A-Za-z]:)(\/|$)/);
  const drive = driveMatch ? driveMatch[1].toLowerCase() : '';
  if (drive) {
    normalized = normalized.slice(drive.length);
  }

  const rooted = normalized.startsWith('/');
  const parts: string[] = [];
  for (const part of normalized.split('/')) {
    if (!part || part === '.') continue;
    if (part === '..' && parts.length > 0 && parts[parts.length - 1] !== '..') {
      parts.pop();
      continue;
    }
    parts.push(part);
  }

  const body = parts.join('/');
  if (drive) return body ? `${drive}/${body}` : `${drive}/`;
  if (rooted) return body ? `/${body}` : '/';
  return body;
}

function diffPathAliasKeys(path: string | undefined): string[] {
  if (!path) return [];
  const normalized = diffPathKey(path);
  if (!normalized) return [];

  const aliases = [normalized];
  const windowsPathLike = /^[a-z]:\//i.test(normalized) || path.includes('\\');
  if (windowsPathLike) {
    const caseFolded = normalized.toLowerCase();
    if (caseFolded !== normalized) aliases.push(caseFolded);
  }
  return aliases;
}

function diffAliasKeys(diff: FileDiffArtifact): string[] {
  const aliases = new Set<string>();
  for (const alias of diffPathAliasKeys(diff.absolutePath)) aliases.add(alias);
  for (const alias of diffPathAliasKeys(diff.path)) aliases.add(alias);
  return Array.from(aliases);
}

function diffIdentityKey(diff: FileDiffArtifact): string {
  return diffAliasKeys(diff)[0] ?? diffPathKey(diff.path);
}

function mergeOperation(current: string, next: string): string {
  if (current === 'create') return 'create';
  if (current === next) return current;
  return 'multi_edit';
}

export function mergeFileDiffArtifactsByPath(diffs: FileDiffArtifact[]): FileDiffArtifact[] {
  const ordered: FileDiffArtifact[] = [];
  const byPathAlias = new Map<string, FileDiffArtifact>();

  for (const diff of diffs) {
    const aliases = diffAliasKeys(diff);
    const existing = aliases
      .map((alias) => byPathAlias.get(alias))
      .find((candidate): candidate is FileDiffArtifact => Boolean(candidate));

    if (!existing) {
      const copy = {
        ...diff,
        hunks: [...diff.hunks],
      };
      for (const alias of aliases) byPathAlias.set(alias, copy);
      ordered.push(copy);
      continue;
    }

    existing.operation = mergeOperation(existing.operation, diff.operation);
    existing.absolutePath ||= diff.absolutePath;
    existing.additions += diff.additions;
    existing.deletions += diff.deletions;
    existing.truncated = Boolean(existing.truncated || diff.truncated);
    existing.omittedLineCount = (existing.omittedLineCount ?? 0) + (diff.omittedLineCount ?? 0);
    existing.hunks = [...existing.hunks, ...diff.hunks];

    for (const alias of diffAliasKeys(existing)) byPathAlias.set(alias, existing);
    for (const alias of aliases) byPathAlias.set(alias, existing);
  }

  return ordered;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value));
}

function numberOrNull(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

function numberOrZero(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0;
}

function stringOrNull(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null;
}

function normalizeLineType(value: unknown): FileDiffLineType | null {
  if (value === 'context' || value === 'addition' || value === 'deletion') return value;
  return null;
}

function parseFileDiffArtifact(
  diff: unknown,
  fallbackAbsolutePath?: string | null,
): FileDiffArtifact | null {
  if (!isRecord(diff)) return null;
  const path = typeof diff.path === 'string' ? diff.path : '';
  const absolutePath = stringOrNull(diff.absolutePath) ?? fallbackAbsolutePath ?? undefined;
  const hunksSource = Array.isArray(diff.hunks) ? diff.hunks : [];
  const hunks: FileDiffHunk[] = hunksSource.flatMap((hunk) => {
    if (!isRecord(hunk) || !Array.isArray(hunk.lines)) return [];
    const lines = hunk.lines.flatMap((line) => {
      if (!isRecord(line)) return [];
      const type = normalizeLineType(line.type);
      if (!type) return [];
      return [{
        type,
        oldLine: numberOrNull(line.oldLine),
        newLine: numberOrNull(line.newLine),
        content: typeof line.content === 'string' ? line.content : '',
      }];
    });

    return [{
      oldStart: numberOrZero(hunk.oldStart),
      newStart: numberOrZero(hunk.newStart),
      oldLines: numberOrZero(hunk.oldLines),
      newLines: numberOrZero(hunk.newLines),
      lines,
    }];
  });

  if (!path || hunks.length === 0) return null;
  return {
    path,
    absolutePath,
    operation: typeof diff.operation === 'string' ? diff.operation : 'str_replace',
    additions: numberOrZero(diff.additions),
    deletions: numberOrZero(diff.deletions),
    truncated: diff.truncated === true,
    omittedLineCount: numberOrZero(diff.omittedLineCount),
    hunks,
  };
}

export function extractFileDiffArtifacts(artifacts: ArtifactPayload | undefined): FileDiffArtifact[] {
  if (!isRecord(artifacts)) return [];
  const absolutePathsByPath = new Map<string, string>();
  if (Array.isArray(artifacts.fileChanges)) {
    for (const change of artifacts.fileChanges) {
      if (!isRecord(change)) continue;
      const path = stringOrNull(change.path);
      const absolutePath = stringOrNull(change.absolutePath);
      if (path && absolutePath) {
        for (const alias of diffPathAliasKeys(path)) {
          absolutePathsByPath.set(alias, absolutePath);
        }
      }
    }
  }
  if (isRecord(artifacts.checkpoint)) {
    const path = stringOrNull(artifacts.checkpoint.path);
    const absolutePath = stringOrNull(artifacts.checkpoint.absolutePath);
    if (path && absolutePath) {
      for (const alias of diffPathAliasKeys(path)) {
        absolutePathsByPath.set(alias, absolutePath);
      }
    }
  }

  const parseWithFallback = (diff: unknown): FileDiffArtifact | null => {
    const directPath = isRecord(diff) ? stringOrNull(diff.path) : null;
    const fallback = directPath
      ? diffPathAliasKeys(directPath)
          .map((alias) => absolutePathsByPath.get(alias))
          .find((absolutePath): absolutePath is string => Boolean(absolutePath))
      : null;
    return parseFileDiffArtifact(diff, fallback);
  };

  if (Array.isArray(artifacts.diffs)) {
    return artifacts.diffs.flatMap((diff) => {
      const parsed = parseWithFallback(diff);
      return parsed ? [parsed] : [];
    });
  }

  const parsed = parseWithFallback(artifacts.diff);
  return parsed ? [parsed] : [];
}

export function extractFileDiffArtifact(artifacts: ArtifactPayload | undefined): FileDiffArtifact | null {
  return extractFileDiffArtifacts(artifacts)[0] ?? null;
}

export function extractDiffStatsArtifact(artifacts: ArtifactPayload | undefined): DiffStatsArtifact | null {
  if (isRecord(artifacts) && isRecord(artifacts.diffStats)) {
    const stats = artifacts.diffStats;
    const paths = Array.isArray(stats.paths)
      ? stats.paths.filter((path): path is string => typeof path === 'string')
      : [];
    return {
      kind: 'diffStats',
      filesChanged: numberOrZero(stats.filesChanged),
      additions: numberOrZero(stats.additions),
      deletions: numberOrZero(stats.deletions),
      hunks: numberOrZero(stats.hunks),
      replacements: typeof stats.replacements === 'number' && Number.isFinite(stats.replacements)
        ? stats.replacements
        : undefined,
      operation: typeof stats.operation === 'string' ? stats.operation : 'edit',
      paths,
    };
  }

  const diffs = extractFileDiffArtifacts(artifacts);
  if (diffs.length === 0) return null;
  const paths = Array.from(new Set(diffs.map((diff) => diff.path)));
  return {
    kind: 'diffStats',
    filesChanged: paths.length,
    additions: diffs.reduce((total, diff) => total + diff.additions, 0),
    deletions: diffs.reduce((total, diff) => total + diff.deletions, 0),
    hunks: diffs.reduce((total, diff) => total + diff.hunks.length, 0),
    operation: diffs.length === 1 ? diffs[0].operation : 'multi_edit',
    paths,
  };
}

function lineClassName(type: FileDiffLineType): string {
  if (type === 'addition') {
    return 'bg-success/10 text-text-primary';
  }
  if (type === 'deletion') {
    return 'bg-danger/10 text-text-primary';
  }
  return 'text-text-secondary hover:bg-surface-1/60';
}

function markerClassName(type: FileDiffLineType): string {
  if (type === 'addition') return 'text-success';
  if (type === 'deletion') return 'text-danger';
  return 'text-text-tertiary';
}

function marker(type: FileDiffLineType): string {
  if (type === 'addition') return '+';
  if (type === 'deletion') return '-';
  return ' ';
}

function lineNumber(value: number | null): string {
  return value == null ? '' : String(value);
}

function FileDiffBody({
  diff,
  compact = false,
  live = false,
}: {
  diff: FileDiffArtifact;
  compact?: boolean;
  live?: boolean;
}) {
  const { t } = useTranslation();
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const maxHeight = compact ? 'max-h-72' : 'max-h-[32rem]';
  const liveLineCount = diff.hunks.reduce((count, hunk) => count + hunk.lines.length, 0);

  useEffect(() => {
    if (!live) return;
    const container = scrollRef.current;
    if (!container) return;
    container.scrollTop = container.scrollHeight;
  }, [live, liveLineCount]);

  return (
    <div ref={scrollRef} className={`${maxHeight} overflow-auto bg-surface-0`}>
      <div className="min-w-max py-1 font-mono text-[11px] leading-5">
        {diff.hunks.map((hunk, hunkIndex) => (
          <div key={`hunk-${hunkIndex}`}>
            <div className="grid border-y border-border/40 bg-surface-2/70 px-2 text-[10px] text-text-tertiary">
              <span>
                @@ -{hunk.oldStart},{hunk.oldLines} +{hunk.newStart},{hunk.newLines} @@
              </span>
            </div>
            {hunk.lines.map((line, lineIndex) => (
              <div
                key={`line-${hunkIndex}-${lineIndex}`}
                className={`grid min-h-5 items-start px-2 transition-colors ${lineClassName(line.type)}`}
                style={{ gridTemplateColumns: '3.25rem 3.25rem 1.5rem minmax(0, 1fr)' }}
              >
                <span className="select-none pr-3 text-right text-text-tertiary/70">
                  {lineNumber(line.oldLine)}
                </span>
                <span className="select-none pr-3 text-right text-text-tertiary/70">
                  {lineNumber(line.newLine)}
                </span>
                <span className={`select-none ${markerClassName(line.type)}`}>
                  {marker(line.type)}
                </span>
                <span className="whitespace-pre pr-4 text-left">{line.content || ' '}</span>
              </div>
            ))}
          </div>
        ))}
        {diff.truncated && diff.omittedLineCount ? (
          <div className="border-t border-border/40 bg-surface-1/70 px-3 py-2 text-xs text-text-tertiary">
            {t('chat.fileDiffLinesOmitted', { count: String(diff.omittedLineCount) })}
          </div>
        ) : null}
      </div>
    </div>
  );
}

function basename(path: string): string {
  const normalized = path.replace(/\\/g, '/');
  return normalized.split('/').filter(Boolean).pop() ?? path;
}

function summarizeOperation(diffs: FileDiffArtifact[]): string {
  if (diffs.every((diff) => diff.operation === 'create')) return 'create';
  if (diffs.some((diff) => diff.operation === 'create')) return 'mixed';
  return 'edit';
}

export function FileDiffSummaryPanel({ diffs }: { diffs: FileDiffArtifact[] }) {
  const { t } = useTranslation();
  const [panelOpen, setPanelOpen] = useState(true);
  const [showAll, setShowAll] = useState(false);
  const [expandedPaths, setExpandedPaths] = useState<Set<string>>(new Set());
  const visibleLimit = 4;
  const visibleDiffs = showAll ? diffs : diffs.slice(0, visibleLimit);
  const hiddenCount = Math.max(0, diffs.length - visibleDiffs.length);
  const additions = diffs.reduce((total, diff) => total + diff.additions, 0);
  const deletions = diffs.reduce((total, diff) => total + diff.deletions, 0);
  const operation = summarizeOperation(diffs);
  const operationLabel =
    operation === 'create'
      ? t('chat.fileDiffCreated')
      : operation === 'mixed'
        ? t('chat.fileDiffSummaryMixed')
        : t('chat.fileDiffModified');

  const togglePath = (path: string) => {
    setExpandedPaths((current) => {
      const next = new Set(current);
      if (next.has(path)) {
        next.delete(path);
      } else {
        next.add(path);
      }
      return next;
    });
  };

  return (
    <div
      className="w-full overflow-hidden rounded-lg border border-border/70 bg-surface-0 shadow-sm ring-1 ring-black/[0.02]"
      data-testid="file-diff-summary-panel"
    >
      <div className="flex items-center gap-3 border-b border-border/60 bg-surface-1/90 px-3 py-2.5">
        <button
          type="button"
          onClick={() => setPanelOpen((current) => !current)}
          aria-expanded={panelOpen}
          aria-label={`${panelOpen ? t('common.collapse') : t('common.expand')} ${t('chat.fileDiffSummaryTitle', { count: String(diffs.length) })}`}
          className="inline-flex min-w-0 flex-1 items-center gap-3 rounded-md text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/40"
        >
          <span className="inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-border/70 bg-surface-0 text-text-secondary shadow-[inset_0_1px_0_rgba(255,255,255,0.04)]">
            <FileCode2 size={17} strokeWidth={1.9} />
          </span>
          <span className="min-w-0">
            <span className="block truncate text-sm font-semibold text-text-primary">
              {t('chat.fileDiffSummaryTitle', { count: String(diffs.length) })}
            </span>
            <span className="mt-0.5 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-text-tertiary">
              <span>{operationLabel}</span>
              {diffs[0] && (
                <span className="truncate">
                  {basename(diffs[0].path)}
                  {diffs.length > 1 ? ` +${diffs.length - 1}` : ''}
                </span>
              )}
            </span>
          </span>
        </button>
        <DiffStatsTicker
          additions={additions}
          deletions={deletions}
          filesChanged={diffs.length}
          compact
          live={false}
          showFiles={false}
          showReplacements={false}
        />
        <ChevronDown
          size={16}
          className={`shrink-0 text-text-tertiary transition-transform ${panelOpen ? 'rotate-180' : ''}`}
        />
      </div>

      {panelOpen && (
        <div className="divide-y divide-border/45">
          {visibleDiffs.map((diff, index) => {
            const key = diffIdentityKey(diff);
            const created = diff.operation === 'create';
            const Icon = created ? FilePlus2 : FilePenLine;
            const expanded = expandedPaths.has(key);
            const previewPath = diff.absolutePath || diff.path;
            const rowOperationLabel = created ? t('chat.fileDiffCreated') : t('chat.fileDiffModified');

            return (
              <div key={`${key}-${index}`} data-testid="file-diff-preview">
                <button
                  type="button"
                  onClick={() => togglePath(key)}
                  aria-expanded={expanded}
                  aria-label={`${expanded ? t('common.collapse') : t('common.expand')} ${rowOperationLabel} ${diff.path}`}
                  className="flex w-full items-center gap-2.5 px-3 py-2 text-left transition-colors hover:bg-surface-1/65 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent/35"
                >
                  <ChevronRight
                    className={`h-3.5 w-3.5 shrink-0 text-text-tertiary transition-transform ${expanded ? 'rotate-90' : ''}`}
                  />
                  <span className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md border border-border/60 bg-surface-1 text-text-secondary">
                    <Icon size={14} strokeWidth={1.9} />
                  </span>
                  <span className="shrink-0 text-xs font-medium text-text-secondary">{rowOperationLabel}</span>
                  {isAbsoluteFileSystemPath(previewPath) ? (
                    <FileBadge path={previewPath} className="min-w-0 flex-1" />
                  ) : (
                    <span
                      className="min-w-0 flex-1 truncate text-xs font-medium text-text-secondary"
                      title={previewPath}
                    >
                      {basename(previewPath)}
                    </span>
                  )}
                  <DiffStatsTicker
                    additions={diff.additions}
                    deletions={diff.deletions}
                    compact
                    live={false}
                    showFiles={false}
                    showReplacements={false}
                  />
                </button>
                {expanded && (
                  <div className="border-t border-border/40">
                    <FileDiffBody diff={diff} compact />
                  </div>
                )}
              </div>
            );
          })}

          {hiddenCount > 0 || showAll ? (
            <button
              type="button"
              onClick={() => setShowAll((current) => !current)}
              className="flex w-full items-center gap-1.5 px-3 py-2 text-left text-xs text-text-tertiary transition-colors hover:bg-surface-1/65 hover:text-text-secondary"
            >
              <ChevronDown
                size={14}
                className={`transition-transform ${showAll ? 'rotate-180' : ''}`}
              />
              {showAll
                ? t('chat.fileDiffShowLess')
                : t('chat.fileDiffShowMore', { count: String(hiddenCount) })}
            </button>
          ) : null}
        </div>
      )}
    </div>
  );
}

export function FileDiffPreview({
  diff,
  compact = false,
  defaultOpen = false,
  live = false,
}: {
  diff: FileDiffArtifact;
  compact?: boolean;
  defaultOpen?: boolean;
  live?: boolean;
}) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(defaultOpen);

  useEffect(() => {
    if (defaultOpen) setExpanded(true);
  }, [defaultOpen]);
  const created = diff.operation === 'create';
  const Icon = created ? FilePlus2 : FilePenLine;
  const operationLabel = created ? t('chat.fileDiffCreated') : t('chat.fileDiffModified');
  const previewPath = diff.absolutePath || diff.path;
  const ToggleIcon = expanded ? ChevronDown : ChevronRight;
  const panelClassName = live
    ? 'w-full overflow-hidden rounded-xl border border-accent/30 bg-surface-0 shadow-[0_12px_36px_rgba(0,0,0,0.14)] ring-1 ring-accent/10'
    : 'w-full overflow-hidden rounded-lg border border-border/70 bg-surface-0 shadow-sm ring-1 ring-black/[0.02]';
  const headerClassName = live
    ? 'flex items-center gap-2 border-b border-accent/20 bg-gradient-to-r from-accent/12 via-surface-1/95 to-surface-1/80 px-3 py-2.5'
    : 'flex items-center gap-2 border-b border-border/60 bg-surface-1/85 px-3 py-2';

  return (
    <div
      className={panelClassName}
      data-testid="file-diff-preview"
    >
      <div className={headerClassName}>
        <button
          type="button"
          onClick={() => setExpanded((current) => !current)}
          aria-expanded={expanded}
          aria-label={`${expanded ? t('common.collapse') : t('common.expand')} ${operationLabel} ${diff.path}`}
          className="inline-flex shrink-0 items-center gap-2 rounded-md px-1 text-left transition-colors hover:bg-surface-0/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/40"
        >
          <ToggleIcon className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
          <span className={`inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md border ${live ? 'border-accent/25 bg-accent/10 text-accent' : 'border-border/60 bg-surface-0 text-text-secondary'}`}>
            <Icon size={14} strokeWidth={1.9} />
          </span>
          <span className="shrink-0 text-xs font-medium text-text-primary">{operationLabel}</span>
          {live ? (
            <span className="inline-flex items-center gap-1 rounded-full border border-accent/20 bg-accent/10 px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-[0.14em] text-accent">
              <span className="h-1.5 w-1.5 rounded-full bg-accent motion-safe:animate-pulse" aria-hidden="true" />
              Live
            </span>
          ) : null}
        </button>
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-2">
            {isAbsoluteFileSystemPath(previewPath) ? (
              <FileBadge path={previewPath} className="min-w-0 max-w-full" />
            ) : (
              <span
                className="inline-flex min-w-0 max-w-full items-center rounded-md border border-border/60 bg-surface-0 px-1.5 py-0.5 text-xs font-medium text-text-secondary"
                title={previewPath}
              >
                <span className="truncate">{basename(previewPath)}</span>
              </span>
            )}
          </div>
        </div>
        <DiffStatsTicker
          additions={diff.additions}
          deletions={diff.deletions}
          compact={compact}
          live={live}
          showFiles={false}
          showReplacements={false}
        />
      </div>

      {expanded ? (
        <FileDiffBody diff={diff} compact={compact} live={live} />
      ) : null}
    </div>
  );
}
