import { type ComponentType, type MouseEvent } from 'react';
import {
  Binary,
  BookText,
  Braces,
  Code2,
  Database,
  FileArchive,
  FileAudio,
  FileCode2,
  FileCog,
  FileImage,
  FileJson,
  FileTerminal,
  FileText,
  FileSpreadsheet,
  FileType,
  FileVideo,
  FolderOpen,
  Hash,
  NotebookText,
  Package,
  Presentation,
  type LucideProps,
} from 'lucide-react';
import { Tooltip } from './Tooltip';
import { openFileInDefaultApp, showInFileExplorer } from '../../lib/api';
import { canPreviewInApp, useFilePreview } from '../../features/preview';

interface FileBadgeProps {
  path: string;
  className?: string;
}

type ColorScheme = { bg: string; text: string; border: string };
type FileStyle = { color: ColorScheme; icon: ComponentType<LucideProps> };

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
} satisfies Record<string, ColorScheme>;

const extStyles: Record<string, FileStyle> = {
  // Documents
  '.pdf':      { color: colors.red,    icon: FileText },
  '.doc':      { color: colors.blue,   icon: FileText },
  '.docx':     { color: colors.blue,   icon: FileText },
  '.rtf':      { color: colors.sky,    icon: FileText },
  '.odt':      { color: colors.sky,    icon: FileText },
  // Spreadsheets
  '.xlsx':     { color: colors.green,  icon: FileSpreadsheet },
  '.xls':      { color: colors.green,  icon: FileSpreadsheet },
  '.ods':      { color: colors.green,  icon: FileSpreadsheet },
  '.csv':      { color: colors.emerald, icon: FileSpreadsheet },
  '.tsv':      { color: colors.emerald, icon: FileSpreadsheet },
  // Presentations
  '.pptx':     { color: colors.orange, icon: Presentation },
  '.ppt':      { color: colors.orange, icon: Presentation },
  '.odp':      { color: colors.orange, icon: Presentation },
  // Markdown
  '.md':       { color: colors.cyan,   icon: NotebookText },
  '.markdown': { color: colors.cyan,   icon: NotebookText },
  '.mdx':      { color: colors.cyan,   icon: NotebookText },
  '.rst':      { color: colors.teal,   icon: BookText },
  '.org':      { color: colors.teal,   icon: BookText },
  // Plain text
  '.txt':      { color: colors.gray,   icon: FileText },
  // Logs
  '.log':      { color: colors.amber,  icon: FileText },
  // Code
  '.ts':       { color: colors.blue,   icon: FileCode2 },
  '.tsx':      { color: colors.blue,   icon: FileCode2 },
  '.js':       { color: colors.amber,  icon: FileCode2 },
  '.jsx':      { color: colors.amber,  icon: FileCode2 },
  '.mjs':      { color: colors.amber,  icon: FileCode2 },
  '.cjs':      { color: colors.amber,  icon: FileCode2 },
  '.py':       { color: colors.yellow, icon: FileCode2 },
  '.rs':       { color: colors.orange, icon: FileCode2 },
  '.go':       { color: colors.cyan,   icon: FileCode2 },
  '.java':     { color: colors.red,    icon: FileCode2 },
  '.kt':       { color: colors.purple, icon: FileCode2 },
  '.kts':      { color: colors.purple, icon: FileCode2 },
  '.swift':    { color: colors.orange, icon: FileCode2 },
  '.c':        { color: colors.indigo, icon: FileCode2 },
  '.cc':       { color: colors.indigo, icon: FileCode2 },
  '.cpp':      { color: colors.indigo, icon: FileCode2 },
  '.cxx':      { color: colors.indigo, icon: FileCode2 },
  '.h':        { color: colors.slate,  icon: FileCode2 },
  '.hpp':      { color: colors.slate,  icon: FileCode2 },
  '.cs':       { color: colors.violet, icon: FileCode2 },
  '.rb':       { color: colors.rose,   icon: FileCode2 },
  '.php':      { color: colors.indigo, icon: FileCode2 },
  '.lua':      { color: colors.blue,   icon: FileCode2 },
  '.r':        { color: colors.sky,    icon: FileCode2 },
  '.sql':      { color: colors.cyan,   icon: Database },
  // Shell and scripts
  '.sh':       { color: colors.emerald, icon: FileTerminal },
  '.bash':     { color: colors.emerald, icon: FileTerminal },
  '.zsh':      { color: colors.emerald, icon: FileTerminal },
  '.fish':     { color: colors.emerald, icon: FileTerminal },
  '.ps1':      { color: colors.blue,    icon: FileTerminal },
  '.bat':      { color: colors.slate,   icon: FileTerminal },
  '.cmd':      { color: colors.slate,   icon: FileTerminal },
  // Config and data
  '.json':     { color: colors.yellow, icon: FileJson },
  '.jsonl':    { color: colors.yellow, icon: FileJson },
  '.toml':     { color: colors.purple, icon: FileCog },
  '.yaml':     { color: colors.purple, icon: Braces },
  '.yml':      { color: colors.purple, icon: Braces },
  '.xml':      { color: colors.orange, icon: Braces },
  '.ini':      { color: colors.slate,  icon: FileCog },
  '.conf':     { color: colors.slate,  icon: FileCog },
  '.config':   { color: colors.slate,  icon: FileCog },
  '.lock':     { color: colors.slate,  icon: Package },
  // Styles
  '.html':     { color: colors.orange, icon: Code2 },
  '.htm':      { color: colors.orange, icon: Code2 },
  '.css':      { color: colors.pink,   icon: Hash },
  '.scss':     { color: colors.pink,   icon: Hash },
  '.sass':     { color: colors.pink,   icon: Hash },
  '.less':     { color: colors.pink,   icon: Hash },
  '.vue':      { color: colors.emerald, icon: Code2 },
  '.svelte':   { color: colors.orange,  icon: Code2 },
  '.astro':    { color: colors.purple,  icon: Code2 },
  // Images
  '.jpg':      { color: colors.yellow, icon: FileImage },
  '.jpeg':     { color: colors.yellow, icon: FileImage },
  '.png':      { color: colors.cyan,   icon: FileImage },
  '.gif':      { color: colors.pink,   icon: FileImage },
  '.webp':     { color: colors.teal,   icon: FileImage },
  '.svg':      { color: colors.orange, icon: FileImage },
  '.bmp':      { color: colors.sky,    icon: FileImage },
  '.tiff':     { color: colors.sky,    icon: FileImage },
  '.tif':      { color: colors.sky,    icon: FileImage },
  // Archives and binaries
  '.zip':      { color: colors.slate,  icon: FileArchive },
  '.tar':      { color: colors.slate,  icon: FileArchive },
  '.gz':       { color: colors.slate,  icon: FileArchive },
  '.tgz':      { color: colors.slate,  icon: FileArchive },
  '.7z':       { color: colors.slate,  icon: FileArchive },
  '.rar':      { color: colors.slate,  icon: FileArchive },
  '.exe':      { color: colors.gray,   icon: Binary },
  '.dll':      { color: colors.gray,   icon: Binary },
  // Video
  '.mp4':      { color: colors.violet, icon: FileVideo },
  '.mkv':      { color: colors.violet, icon: FileVideo },
  '.webm':     { color: colors.violet, icon: FileVideo },
  '.mov':      { color: colors.violet, icon: FileVideo },
  '.avi':      { color: colors.violet, icon: FileVideo },
  '.flv':      { color: colors.violet, icon: FileVideo },
  '.wmv':      { color: colors.violet, icon: FileVideo },
  '.m4v':      { color: colors.violet, icon: FileVideo },
  '.mpeg':     { color: colors.violet, icon: FileVideo },
  '.mpg':      { color: colors.violet, icon: FileVideo },
  // Audio
  '.mp3':      { color: colors.fuchsia, icon: FileAudio },
  '.wav':      { color: colors.fuchsia, icon: FileAudio },
  '.flac':     { color: colors.fuchsia, icon: FileAudio },
  '.ogg':      { color: colors.fuchsia, icon: FileAudio },
  '.aac':      { color: colors.fuchsia, icon: FileAudio },
  '.m4a':      { color: colors.fuchsia, icon: FileAudio },
  '.wma':      { color: colors.fuchsia, icon: FileAudio },
  '.opus':     { color: colors.fuchsia, icon: FileAudio },
};

const namedFileStyles: Record<string, FileStyle> = {
  dockerfile: { color: colors.blue, icon: FileTerminal },
  makefile: { color: colors.slate, icon: FileTerminal },
  justfile: { color: colors.slate, icon: FileTerminal },
  license: { color: colors.green, icon: BookText },
  copying: { color: colors.green, icon: BookText },
  readme: { color: colors.cyan, icon: NotebookText },
};

const defaultStyle: FileStyle = { color: colors.gray, icon: FileType };
const dirStyle: FileStyle = { color: colors.gray, icon: FolderOpen };

function getStyleForPath(filename: string): FileStyle {
  const lower = filename.toLowerCase();
  const namedStyle = namedFileStyles[lower] ?? namedFileStyles[lower.split('.')[0]];
  if (namedStyle) return namedStyle;
  if (lower.startsWith('.env')) return { color: colors.green, icon: FileCog };

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
  const { openFilePreview } = useFilePreview();
  const dir = isDirectory(path);
  const name = basename(path);
  const { color, icon: Icon } = dir ? dirStyle : getStyleForPath(name);

  const handleClick = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.altKey) {
      showInFileExplorer(path);
    } else if (!dir && canPreviewInApp(path) && !e.ctrlKey && !e.metaKey && !e.shiftKey) {
      openFilePreview(path);
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
