import { FilePenLine, FilePlus2 } from 'lucide-react';
import { useTranslation } from '../../i18n';
import type { ArtifactPayload } from '../../types/conversation';
import { FileBadge } from '../ui/FileBadge';

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
  operation: string;
  additions: number;
  deletions: number;
  truncated?: boolean;
  omittedLineCount?: number;
  hunks: FileDiffHunk[];
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

function normalizeLineType(value: unknown): FileDiffLineType | null {
  if (value === 'context' || value === 'addition' || value === 'deletion') return value;
  return null;
}

export function extractFileDiffArtifact(artifacts: ArtifactPayload | undefined): FileDiffArtifact | null {
  if (!isRecord(artifacts) || !isRecord(artifacts.diff)) return null;
  const diff = artifacts.diff;
  const path = typeof diff.path === 'string' ? diff.path : '';
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
    operation: typeof diff.operation === 'string' ? diff.operation : 'str_replace',
    additions: numberOrZero(diff.additions),
    deletions: numberOrZero(diff.deletions),
    truncated: diff.truncated === true,
    omittedLineCount: numberOrZero(diff.omittedLineCount),
    hunks,
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

export function FileDiffPreview({ diff, compact = false }: { diff: FileDiffArtifact; compact?: boolean }) {
  const { t } = useTranslation();
  const created = diff.operation === 'create';
  const Icon = created ? FilePlus2 : FilePenLine;
  const operationLabel = created ? t('chat.fileDiffCreated') : t('chat.fileDiffModified');
  const maxHeight = compact ? 'max-h-56' : 'max-h-80';

  return (
    <div className="overflow-hidden rounded-lg border border-border/70 bg-surface-0/80 shadow-sm">
      <div className="flex items-center gap-2 border-b border-border/60 bg-surface-1/70 px-3 py-2">
        <span className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md border border-border/60 bg-surface-0 text-text-secondary">
          <Icon size={14} strokeWidth={1.9} />
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-2">
            <span className="shrink-0 text-xs font-medium text-text-primary">{operationLabel}</span>
            <FileBadge path={diff.path} className="min-w-0 max-w-full" />
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-1.5 font-mono text-[11px] tabular-nums">
          <span className="rounded-md border border-success/20 bg-success/10 px-1.5 py-0.5 text-success">
            +{diff.additions}
          </span>
          <span className="rounded-md border border-danger/20 bg-danger/10 px-1.5 py-0.5 text-danger">
            -{diff.deletions}
          </span>
        </div>
      </div>

      <div className={`${maxHeight} overflow-auto bg-surface-0`}>
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
    </div>
  );
}
