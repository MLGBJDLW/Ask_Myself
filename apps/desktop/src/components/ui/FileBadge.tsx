import { type ComponentType, type MouseEvent } from 'react';
import {
  FileText,
  FileSpreadsheet,
  FolderOpen,
  Presentation,
  Film,
  Music,
  type LucideProps,
} from 'lucide-react';
import { Tooltip } from './Tooltip';
import { openFileInDefaultApp, showInFileExplorer } from '../../lib/api';

interface FileBadgeProps {
  path: string;
  className?: string;
}

type ColorScheme = { bg: string; text: string; border: string };
type FileStyle = { color: ColorScheme; icon: ComponentType<LucideProps> };

const colors = {
  red:    { bg: 'bg-red-500/10',    text: 'text-red-400',    border: 'border-red-500/20' },
  blue:   { bg: 'bg-info/10',       text: 'text-info',       border: 'border-info/20' },
  green:  { bg: 'bg-success/10',    text: 'text-success',    border: 'border-success/20' },
  orange: { bg: 'bg-warning/10',    text: 'text-warning',    border: 'border-warning/20' },
  teal:   { bg: 'bg-teal-500/10',   text: 'text-teal-400',   border: 'border-teal-500/20' },
  amber:  { bg: 'bg-amber-500/10',  text: 'text-amber-400',  border: 'border-amber-500/20' },
  purple: { bg: 'bg-purple-500/10', text: 'text-purple-400', border: 'border-purple-500/20' },
  pink:   { bg: 'bg-pink-500/10',   text: 'text-pink-400',   border: 'border-pink-500/20' },
  violet: { bg: 'bg-violet-500/10', text: 'text-violet-400', border: 'border-violet-500/20' },
  gray:   { bg: 'bg-surface-3',     text: 'text-text-secondary', border: 'border-border' },
} satisfies Record<string, ColorScheme>;

const extStyles: Record<string, FileStyle> = {
  // Documents
  '.pdf':      { color: colors.red,    icon: FileText },
  '.docx':     { color: colors.blue,   icon: FileText },
  // Spreadsheets
  '.xlsx':     { color: colors.green,  icon: FileSpreadsheet },
  '.xls':      { color: colors.green,  icon: FileSpreadsheet },
  // Presentations
  '.pptx':     { color: colors.orange, icon: Presentation },
  // Markdown
  '.md':       { color: colors.teal,   icon: FileText },
  '.markdown': { color: colors.teal,   icon: FileText },
  // Plain text
  '.txt':      { color: colors.gray,   icon: FileText },
  // Logs
  '.log':      { color: colors.amber,  icon: FileText },
  // Code (JS/TS)
  '.ts':       { color: colors.blue,   icon: FileText },
  '.tsx':      { color: colors.blue,   icon: FileText },
  '.js':       { color: colors.blue,   icon: FileText },
  '.jsx':      { color: colors.blue,   icon: FileText },
  // Code (Rust)
  '.rs':       { color: colors.orange, icon: FileText },
  // Config
  '.json':     { color: colors.purple, icon: FileText },
  '.toml':     { color: colors.purple, icon: FileText },
  '.yaml':     { color: colors.purple, icon: FileText },
  '.yml':      { color: colors.purple, icon: FileText },
  // Styles
  '.css':      { color: colors.pink,   icon: FileText },
  '.scss':     { color: colors.pink,   icon: FileText },
  '.sass':     { color: colors.pink,   icon: FileText },
  '.less':     { color: colors.pink,   icon: FileText },
  // Video
  '.mp4':      { color: colors.violet, icon: Film },
  '.mkv':      { color: colors.violet, icon: Film },
  '.webm':     { color: colors.violet, icon: Film },
  '.mov':      { color: colors.violet, icon: Film },
  '.avi':      { color: colors.violet, icon: Film },
  '.flv':      { color: colors.violet, icon: Film },
  '.wmv':      { color: colors.violet, icon: Film },
  '.m4v':      { color: colors.violet, icon: Film },
  '.mpeg':     { color: colors.violet, icon: Film },
  '.mpg':      { color: colors.violet, icon: Film },
  // Audio
  '.mp3':      { color: colors.pink,   icon: Music },
  '.wav':      { color: colors.pink,   icon: Music },
  '.flac':     { color: colors.pink,   icon: Music },
  '.ogg':      { color: colors.pink,   icon: Music },
  '.aac':      { color: colors.pink,   icon: Music },
  '.m4a':      { color: colors.pink,   icon: Music },
  '.wma':      { color: colors.pink,   icon: Music },
  '.opus':     { color: colors.pink,   icon: Music },
};

const defaultStyle: FileStyle = { color: colors.gray, icon: FileText };
const dirStyle: FileStyle = { color: colors.gray, icon: FolderOpen };

function getStyleForPath(filename: string): FileStyle {
  const dot = filename.lastIndexOf('.');
  if (dot === -1) return defaultStyle;
  return extStyles[filename.slice(dot).toLowerCase()] ?? defaultStyle;
}

function isDirectory(path: string): boolean {
  return path.endsWith('/') || path.endsWith('\\');
}

function basename(path: string): string {
  const normalized = path.replace(/[\\/]+$/, '');
  const lastSep = Math.max(normalized.lastIndexOf('/'), normalized.lastIndexOf('\\'));
  return lastSep === -1 ? normalized : normalized.slice(lastSep + 1);
}

export function FileBadge({ path, className = '' }: FileBadgeProps) {
  const dir = isDirectory(path);
  const name = basename(path);
  const { color, icon: Icon } = dir ? dirStyle : getStyleForPath(name);

  const handleClick = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.altKey) {
      showInFileExplorer(path);
    } else {
      openFileInDefaultApp(path);
    }
  };

  const handleContextMenu = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    showInFileExplorer(path);
  };

  return (
    <Tooltip content={path} side="top">
      <button
        type="button"
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
        <Icon size={12} strokeWidth={2} className="shrink-0" />
        <span className="truncate max-w-[200px]">{name}</span>
      </button>
    </Tooltip>
  );
}
