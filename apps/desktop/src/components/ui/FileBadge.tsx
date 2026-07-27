import { type MouseEvent } from 'react';
import { Tooltip } from './Tooltip';
import { openFileInDefaultApp, showInFileExplorer } from '../../lib/api';
import { canPreviewInApp, useFilePreview } from '../../features/preview';
import { resolveFileBadgeIcon, type FileBadgeTone } from './fileBadgeCatalog';

interface FileBadgeProps {
  path: string;
  className?: string;
}

type ColorScheme = { bg: string; text: string; border: string };

const colors = {
  red:     { bg: 'bg-red-500/10',     text: 'text-red-400',     border: 'border-red-500/20' },
  rose:    { bg: 'bg-rose-500/10',    text: 'text-rose-400',    border: 'border-rose-500/20' },
  blue:    { bg: 'bg-info/10',        text: 'text-info',        border: 'border-info/20' },
  sky:     { bg: 'bg-sky-500/10',     text: 'text-sky-400',     border: 'border-sky-500/20' },
  cyan:    { bg: 'bg-cyan-500/10',    text: 'text-cyan-400',    border: 'border-cyan-500/20' },
  teal:    { bg: 'bg-teal-500/10',    text: 'text-teal-400',    border: 'border-teal-500/20' },
  green:   { bg: 'bg-success/10',     text: 'text-success',     border: 'border-success/20' },
  emerald: { bg: 'bg-emerald-500/10', text: 'text-emerald-400', border: 'border-emerald-500/20' },
  orange:  { bg: 'bg-orange-500/10',  text: 'text-orange-400',  border: 'border-orange-500/20' },
  amber:   { bg: 'bg-amber-500/10',   text: 'text-amber-400',   border: 'border-amber-500/20' },
  yellow:  { bg: 'bg-yellow-500/10',  text: 'text-yellow-400',  border: 'border-yellow-500/20' },
  purple:  { bg: 'bg-purple-500/10',  text: 'text-purple-400',  border: 'border-purple-500/20' },
  fuchsia: { bg: 'bg-fuchsia-500/10', text: 'text-fuchsia-400', border: 'border-fuchsia-500/20' },
  pink:    { bg: 'bg-pink-500/10',    text: 'text-pink-400',    border: 'border-pink-500/20' },
  violet:  { bg: 'bg-violet-500/10',  text: 'text-violet-400',  border: 'border-violet-500/20' },
  indigo:  { bg: 'bg-indigo-500/10',  text: 'text-indigo-400',  border: 'border-indigo-500/20' },
  slate:   { bg: 'bg-slate-500/10',   text: 'text-slate-400',   border: 'border-slate-500/20' },
  gray:    { bg: 'bg-surface-3',      text: 'text-text-secondary', border: 'border-border' },
} satisfies Record<FileBadgeTone, ColorScheme>;

function isDirectory(path: string): boolean {
  return path.endsWith('/') || path.endsWith('\\');
}

export function isAbsoluteFileSystemPath(path: string): boolean {
  const trimmed = path.trim();
  if (!trimmed) return false;
  if (trimmed.startsWith('/')) return true;
  if (/^[A-Za-z]:[\\/]/.test(trimmed)) return true;
  return /^\\\\[^\\/]+[\\/][^\\/]+/.test(trimmed);
}

function basename(path: string): string {
  const normalized = path.replace(/[\\/]+$/, '');
  const lastSep = Math.max(normalized.lastIndexOf('/'), normalized.lastIndexOf('\\'));
  return lastSep === -1 ? normalized : normalized.slice(lastSep + 1);
}

export function FileBadge({ path, className = '' }: FileBadgeProps) {
  const { openFilePreview } = useFilePreview();
  const safePath = path.trim();
  if (!isAbsoluteFileSystemPath(safePath)) return null;

  const dir = isDirectory(safePath);
  const name = basename(safePath);
  const { tone, Icon, iconId } = resolveFileBadgeIcon(name, dir);
  const color = colors[tone];

  const handleClick = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.altKey) {
      showInFileExplorer(safePath);
    } else if (!dir && canPreviewInApp(safePath) && !e.ctrlKey && !e.metaKey && !e.shiftKey) {
      openFilePreview(safePath);
    } else {
      openFileInDefaultApp(safePath);
    }
  };

  const handleContextMenu = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    showInFileExplorer(safePath);
  };

  return (
    <Tooltip content={safePath} side="top">
      <button
        type="button"
        data-file-icon={iconId}
        onClick={handleClick}
        onContextMenu={handleContextMenu}
        className={`
          inline-flex items-center gap-1 px-1.5 py-0.5 text-xs font-medium
          rounded-md border cursor-pointer transition-all duration-150
          hover:brightness-125 hover:scale-[1.02] active:scale-[0.98]
          ${color.bg} ${color.text} ${color.border}
          ${className}
        `}
      >
        <Icon size={13} aria-hidden="true" className="shrink-0" />
        <span className="truncate max-w-[200px]">{name}</span>
      </button>
    </Tooltip>
  );
}
