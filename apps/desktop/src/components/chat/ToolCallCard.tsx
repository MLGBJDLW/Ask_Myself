import { useState } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import {
  Search,
  BookOpen,
  FileText,
  List,
  ChevronDown,
  ChevronUp,
  Loader2,
  CheckCircle2,
  XCircle,
  Wrench,
  FolderOpen,
  Globe,
  Layers,
  PenLine,
} from 'lucide-react';
import { useTranslation } from '../../i18n';

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

interface ToolCallCardProps {
  toolName?: string;
  arguments?: string;
  status: 'running' | 'done' | 'error';
  content?: string;
  isError?: boolean;
  artifacts?: Record<string, unknown>;
}

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

const TOOL_ICONS: Record<string, typeof Search> = {
  search: Search,
  playbook: BookOpen,
  file: FileText,
  summarize: List,
  list_dir: FolderOpen,
  fetch_url: Globe,
  chunk_context: Layers,
  write_note: PenLine,
};

function getToolIcon(name?: string) {
  const lower = (name || '').toLowerCase();
  for (const [key, Icon] of Object.entries(TOOL_ICONS)) {
    if (lower.includes(key)) return Icon;
  }
  return Wrench;
}

function formatArgs(raw?: string): string {
  if (!raw) return '';
  try {
    const parsed = JSON.parse(raw);
    return Object.entries(parsed)
      .map(([k, v]) => `${k}: ${JSON.stringify(v)}`)
      .join(', ');
  } catch {
    return raw;
  }
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function ToolCallCard({
  toolName,
  arguments: args,
  status,
  content,
  isError,
  artifacts,
}: ToolCallCardProps) {
  const [expanded, setExpanded] = useState(false);
  const { t } = useTranslation();
  const safeToolName =
    typeof toolName === 'string' && toolName.trim().length > 0
      ? toolName
      : 'unknown_tool';
  const Icon = getToolIcon(safeToolName);
  const formattedArgs = formatArgs(args);

  const statusConfig = {
    running: { icon: Loader2, text: t('chat.toolRunning'), color: 'text-accent', spin: true },
    done: { icon: CheckCircle2, text: t('chat.toolDone'), color: 'text-success', spin: false },
    error: { icon: XCircle, text: t('chat.toolError'), color: 'text-danger', spin: false },
  }[status];

  const StatusIcon = statusConfig.icon;

  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
      className="bg-surface-1 border border-border rounded-lg overflow-hidden my-2"
    >
      {/* Header */}
      <button
        onClick={() => setExpanded((p) => !p)}
        aria-expanded={expanded}
        aria-label={expanded ? t('common.collapse') : t('common.expand')}
        className="flex items-center gap-2 w-full px-3 py-2 text-left hover:bg-surface-2
          transition-colors duration-fast ease-out cursor-pointer"
      >
        <Icon className="h-4 w-4 shrink-0 text-text-tertiary" />
        <span className="text-xs font-medium text-text-primary truncate">{safeToolName}</span>
        <span className="text-[11px] text-text-tertiary truncate flex-1">
          {formattedArgs || '-'}
        </span>
        <StatusIcon
          className={`h-3.5 w-3.5 shrink-0 ${statusConfig.color} ${statusConfig.spin ? 'animate-spin' : ''}`}
        />
        {content ? (
          expanded ? (
            <ChevronUp className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
          ) : (
            <ChevronDown className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
          )
        ) : null}
      </button>

      {/* Expandable result */}
      <AnimatePresence>
        {expanded && content && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: 'auto', opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
            className="overflow-hidden"
          >
            <div className="border-t border-border px-3 py-2">
              <pre
                className={`text-xs whitespace-pre-wrap break-words max-h-48 overflow-y-auto
                  ${isError ? 'text-danger' : 'text-text-secondary'}`}
              >
                {content}
              </pre>
              {artifacts && (
                <div className="mt-2 text-[11px] text-text-tertiary">
                  {JSON.stringify(artifacts, null, 2).slice(0, 500)}
                </div>
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </motion.div>
  );
}
