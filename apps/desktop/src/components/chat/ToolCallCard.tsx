import { useState, useEffect, useMemo, useCallback } from 'react';
import type { CSSProperties } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { save as showSaveDialog } from '@tauri-apps/plugin-dialog';
import { motion, AnimatePresence, useReducedMotion } from 'framer-motion';
import { toast } from 'sonner';
import {
  Bot,
  Search,
  BookOpen,
  BrainCircuit,
  Database,
  FileText,
  FileCode2,
  FileSearch,
  List,
  ChevronDown,
  ChevronUp,
  CheckCircle2,
  XCircle,
  Wrench,
  FolderOpen,
  Globe,
  Layers,
  Network,
  PenLine,
  ClipboardList,
  ShieldCheck,
  Terminal,
  Download,
  ExternalLink,
  Image as ImageIcon,
  Save,
  Route,
  ScrollText,
  Sparkles,
  Volume2,
} from 'lucide-react';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import { FileBadge } from '../ui/FileBadge';
import { Button } from '../ui/Button';
import { Tooltip } from '../ui/Tooltip';
import { getSoftCollapseMotion } from '../../lib/uiMotion';
import type { ToolCallEvent } from '../../lib/streaming/protocol';
import {
  isPendingToolCallStatus,
  isUnsuccessfulToolCallStatus,
} from '../../lib/streaming/toolStatus';
import {
  formatToolTarget,
  formatToolArgumentsForDisplay,
  getToolInputPresentation,
  getStableFileChangeTarget,
  getToolTitleTarget,
  isCommandExecutionTool,
} from '../../lib/streaming/toolCardPresentation';
import { extractPlanArtifact, extractVerificationArtifact } from '../../lib/taskArtifacts';
import {
  extractSubagentArtifact,
  extractSubagentBatchArtifact,
  extractSubagentJudgementArtifact,
  parseSubagentArguments,
} from '../../lib/subagentArtifacts';
import { PlanPanel, VerificationPanel } from './TaskPanels';
import type { ArtifactPayload, CapabilityOwner, ToolRenderKind, ToolRunCapabilities } from '../../types/conversation';
import type { VerificationOverallStatus } from '../../lib/taskArtifacts';
import { SubagentCard } from './SubagentCard';
import { QuestionRequestPanel } from './QuestionRequestPanel';
import { extractQuestionRequest } from '../../lib/questionCards';
import {
  FileDiffPreview,
  extractDiffStatsArtifact,
  extractFileDiffArtifact,
  type DiffStatsArtifact,
} from './FileDiffPreview';
import { DiffStatsTicker } from './DiffStatsTicker';
import { isFileChangeToolRender } from './toolRenderers';
import {
  extractGraphAgentUsage,
  saveGraphAgentUsage,
  type GraphAgentUsage,
} from '../../lib/knowledgeGraphAgent';

const TERMINAL_OPEN_EVENT = 'nexa:terminal-open';

/* ------------------------------------------------------------------ */
/*  Types                                                              */
/* ------------------------------------------------------------------ */

interface SearchResultItem {
  score: number;
  source: string;
  path: string;
  title: string;
  preview: string;
}

interface TrustBoundaryArtifact {
  origin?: string;
  authority?: string;
  visibility?: string;
  mutability?: string;
  externality?: string;
  canInstruct?: boolean;
}

interface GeneratedImageArtifact {
  kind: 'generatedImage';
  path?: string;
  previewPath?: string;
  dataUrl?: string;
  mediaType?: string;
  provider?: string;
  model?: string;
  prompt?: string;
  revisedPrompt?: string;
  bytes?: number;
  saved?: boolean;
  transient?: boolean;
  suggestedFilename?: string;
  providerImageUrl?: string;
}

interface GeneratedAudioArtifact {
  kind: 'generatedAudio';
  path?: string;
  previewPath?: string;
  mediaType?: string;
  provider?: string;
  model?: string;
  voice?: string;
  bytes?: number;
}

interface ImagePromptArgs {
  prompt?: string;
  size?: string;
  quality?: string;
  outputFormat?: string;
  model?: string;
}

interface ManageSkillArgs {
  action?: string;
  skillId?: string;
  resourcePath?: string;
}

interface SkillActivationSkill {
  id?: string;
  name?: string;
  description?: string;
  builtin?: boolean;
  sourcePath?: string | null;
  interface?: {
    displayName?: string;
    shortDescription?: string;
  };
  policy?: {
    allowImplicitInvocation?: boolean;
  };
}

interface SkillActivationArtifact {
  kind: 'skillActivation';
  skill: SkillActivationSkill;
}

function KnowledgeGraphUsagePanel({
  usage,
  compact = false,
}: {
  usage: GraphAgentUsage;
  compact?: boolean;
}) {
  const { t } = useTranslation();
  const nodeLimit = compact ? 4 : 8;
  const bundleLimit = compact ? 3 : 5;
  const docLimit = compact ? 3 : 6;
  const tokenEstimate = usage.tokenEstimate ?? null;
  const usedGraphBundles = usage.usedGraphBundles ?? [];

  return (
    <div className={`rounded-md border border-border/55 bg-surface-0/55 ${compact ? 'p-2' : 'p-3'}`}>
      <div className="mb-2 flex min-w-0 flex-wrap items-center gap-1.5 text-xs text-text-secondary">
        <span className="inline-flex items-center gap-1 font-medium text-text-primary">
          <Layers className="h-3.5 w-3.5 text-accent" />
          {t('chat.usedGraphNodes', { count: String(usage.usedGraphNodes.length) })}
        </span>
        <span className="rounded-full border border-border/55 bg-surface-1/70 px-2 py-0.5">
          {t('chat.usedGraphEdges', { count: String(usage.usedGraphEdges.length) })}
        </span>
        {usedGraphBundles.length > 0 && (
          <span className="rounded-full border border-info/25 bg-info/10 px-2 py-0.5 text-info">
            {t('chat.usedGraphBundles', { count: String(usedGraphBundles.length) })}
          </span>
        )}
        <span className="rounded-full border border-border/55 bg-surface-1/70 px-2 py-0.5">
          {t('chat.usedGraphDocuments', { count: String(usage.usedDocuments.length) })}
        </span>
        {tokenEstimate && (
          <span className="rounded-full border border-success/25 bg-success/10 px-2 py-0.5 text-success">
            {t('chat.graphTokenSavings', { saved: String(tokenEstimate.savedPctEstimate) })}
          </span>
        )}
      </div>
      {usage.usedGraphNodes.length > 0 && (
        <div className="flex flex-wrap gap-1.5">
          {usage.usedGraphNodes.slice(0, nodeLimit).map((node) => (
            <span
              key={node.id}
              className="max-w-full truncate rounded-md border border-border/55 bg-surface-1 px-2 py-1 text-[11px] text-text-secondary"
              title={node.description || node.label}
            >
              {node.label}
            </span>
          ))}
          {usage.usedGraphNodes.length > nodeLimit && (
            <span className="rounded-md border border-border/45 px-2 py-1 text-[11px] text-text-tertiary">
              +{usage.usedGraphNodes.length - nodeLimit}
            </span>
          )}
        </div>
      )}
      {usedGraphBundles.length > 0 && (
        <div className="mt-2 flex flex-wrap gap-1.5">
          {usedGraphBundles.slice(0, bundleLimit).map((bundle) => (
            <span
              key={bundle.id}
              className="max-w-full truncate rounded-md border border-info/25 bg-info/10 px-2 py-1 text-[11px] text-info"
              title={bundle.relationTypes.join(', ')}
            >
              {bundle.relationCount}x {bundle.sourceLabel ?? bundle.source} / {bundle.targetLabel ?? bundle.target}
            </span>
          ))}
          {usedGraphBundles.length > bundleLimit && (
            <span className="rounded-md border border-border/45 px-2 py-1 text-[11px] text-text-tertiary">
              +{usedGraphBundles.length - bundleLimit}
            </span>
          )}
        </div>
      )}
      {usage.usedDocuments.length > 0 && (
        <div className="mt-2 space-y-1">
          {usage.usedDocuments.slice(0, docLimit).map((doc) => (
            <div key={doc.documentId} className="flex min-w-0 items-center gap-1.5 text-[11px] text-text-tertiary">
              <FileText className="h-3 w-3 shrink-0" />
              <span className="truncate">{doc.title}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

type ToolCallCardStatus = ToolCallEvent['status'];

interface ToolCallCardProps {
  callId?: string;
  toolName?: string;
  arguments?: string;
  status: ToolCallCardStatus;
  owner?: CapabilityOwner;
  renderKind?: ToolRenderKind;
  capabilities?: ToolRunCapabilities;
  durationMs?: number;
  progressNote?: string;
  content?: string;
  isError?: boolean;
  artifacts?: ArtifactPayload;
  compact?: boolean;
  inline?: boolean;
  trace?: boolean;
  /** Assembly progress of `arguments` before execution. */
  argsStatus?: ToolCallEvent['argsStatus'];
  /** Total characters of `arguments` received so far. */
  argsBytes?: number;
  onQuestionSubmit?: (message: string, artifact: ArtifactPayload) => void;
  questionAnswered?: boolean;
}

function formatByteCount(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '';
  if (bytes < 1024) return `${bytes} B`;
  const kb = bytes / 1024;
  if (kb < 1024) return `${kb.toFixed(kb < 10 ? 1 : 0)} KB`;
  return `${(kb / 1024).toFixed(1)} MB`;
}

function formatDurationMs(durationMs: number | undefined): string {
  if (typeof durationMs !== 'number' || !Number.isFinite(durationMs) || durationMs < 0) return '';
  if (durationMs < 1000) return `${Math.round(durationMs)} ms`;
  const seconds = durationMs / 1000;
  if (seconds < 60) return `${seconds.toFixed(seconds < 10 ? 1 : 0)} s`;
  const minutes = Math.floor(seconds / 60);
  const remainingSeconds = Math.round(seconds % 60);
  return `${minutes}m ${remainingSeconds}s`;
}

function DiffStatsSummaryPanel({ stats }: { stats: DiffStatsArtifact }) {
  const { t } = useTranslation();
  const path = stats.paths[0];
  return (
    <div className="flex flex-wrap items-center gap-2 rounded-lg border border-border/60 bg-surface-0/65 px-3 py-2">
      <DiffStatsTicker
        additions={stats.additions}
        deletions={stats.deletions}
        filesChanged={stats.filesChanged}
        replacements={stats.replacements}
      />
      {path && <FileBadge path={path} className="min-w-0 max-w-full" />}
      {stats.hunks > 0 && (
        <span className="rounded-md border border-border/60 bg-surface-1 px-2 py-1 text-[11px] text-text-tertiary">
          {t('chat.diffHunks', { count: String(stats.hunks) })}
        </span>
      )}
    </div>
  );
}

function visibleToolResult(
  content: string | undefined,
  structuredResult: boolean,
  failed: boolean,
): string | null {
  const trimmed = content?.trim();
  if (!trimmed) return null;
  if (structuredResult && !failed) return null;
  if (!failed && /^(?:ok|done|success|completed)[.!]?$/i.test(trimmed)) return null;
  try {
    const parsed = JSON.parse(trimmed) as unknown;
    return JSON.stringify(parsed, null, 2);
  } catch {
    return trimmed;
  }
}

function toolResultHeaderSummary(content: string | undefined): string | null {
  const trimmed = content?.trim();
  if (!trimmed) return null;

  let summary = trimmed;
  try {
    const parsed = JSON.parse(trimmed) as unknown;
    if (typeof parsed === 'string') {
      summary = parsed;
    } else if (parsed && typeof parsed === 'object') {
      const record = parsed as Record<string, unknown>;
      const candidate = [record.message, record.error, record.summary, record.detail]
        .find((value): value is string => typeof value === 'string' && value.trim().length > 0);
      if (candidate) summary = candidate;
    }
  } catch {
    // Plain-text failures are already the most useful summary.
  }

  const firstLine = summary.split(/\r?\n/, 1)[0]?.trim() ?? '';
  if (!firstLine) return null;
  return firstLine.length > 96 ? `${firstLine.slice(0, 95)}…` : firstLine;
}

function ToolResultSurface({
  content,
  error,
  compact = false,
}: {
  content: string;
  error?: boolean;
  compact?: boolean;
}) {
  return (
    <div className="tool-result-surface" data-result-tone={error ? 'error' : 'default'}>
      <pre className={`whitespace-pre-wrap break-words overflow-y-auto font-mono ${
        compact ? 'max-h-36 text-[10px] leading-relaxed' : 'max-h-56 text-[11px] leading-relaxed'
      } ${error ? 'text-danger' : 'text-text-secondary'}`}>
        {content}
      </pre>
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

const TOOL_ICONS: Record<string, typeof Search> = {
  search_playbooks: BookOpen,
  search_sessions: BookOpen,
  search_by_date: Search,
  search: Search,
  code_intelligence: FileCode2,
  grep_files: FileSearch,
  search_files: FileSearch,
  glob_files: FolderOpen,
  read_files: FileText,
  read_file: FileText,
  get_document_info: FileText,
  database: Database,
  compare_documents: Layers,
  summarize_document: List,
  retrieve_evidence: BookOpen,
  query_knowledge_graph: Network,
  get_related_concepts: Network,
  list_documents: List,
  list_sources: BookOpen,
  compile_document: ClipboardList,
  desktop_automation: Globe,
  project_tool: Wrench,
  playbook: BookOpen,
  multi_edit: PenLine,
  edit_file: PenLine,
  create_file: PenLine,
  apply_patch: PenLine,
  file: FileText,
  summarize: List,
  list_dir: FolderOpen,
  web_search: Globe,
  web_research_context: Globe,
  fetch_url: Globe,
  download_asset: Download,
  chunk_context: Network,
  write_note: PenLine,
  update_plan: ClipboardList,
  record_verification: ShieldCheck,
  run_shell: Terminal,
  shell_command: Terminal,
  generate_image: ImageIcon,
  manage_skill: BrainCircuit,
  activate_skill: BrainCircuit,
  spawn_subagent: Bot,
  subagent: Bot,
  route: Route,
  memory: ScrollText,
  model: Sparkles,
};

const TOOL_LABELS: Record<string, string> = {
  search_playbooks: 'Search playbooks',
  search_sessions: 'Search sessions',
  search_by_date: 'Search by date',
  search: 'Search',
  code_intelligence: 'Inspect code',
  grep_files: 'Search files',
  glob_files: 'Find files',
  read_files: 'Read files',
  read_file: 'Read file',
  get_document_info: 'Inspect document',
  compare_documents: 'Compare documents',
  summarize_document: 'Summarize document',
  retrieve_evidence: 'Retrieve evidence',
  query_knowledge_graph: 'Query graph',
  get_related_concepts: 'Find related concepts',
  list_documents: 'List documents',
  list_sources: 'List sources',
  compile_document: 'Compile document',
  desktop_automation: 'Use desktop',
  project_tool: 'Project tool',
  playbook: 'Run playbook',
  multi_edit: 'Edit files',
  edit_file: 'Edit file',
  file: 'Open file',
  summarize: 'Summarize',
  list_dir: 'List folder',
  web_search: 'Search web',
  web_research_context: 'Build web context',
  fetch_url: 'Fetch URL',
  download_asset: 'Download asset',
  chunk_context: 'Chunk context',
  write_note: 'Write note',
  update_plan: 'Update plan',
  record_verification: 'Record verification',
  run_shell: 'Run command',
  shell_command: 'Run command',
  apply_patch: 'Edit files',
  generate_image: 'Generate image',
  manage_skill: 'Use skill',
  activate_skill: 'Activate skill',
};

const TOOL_LABEL_KEYS = Object.keys(TOOL_LABELS).sort((a, b) => b.length - a.length);
const TOOL_LABEL_SUBSTRING_KEYS = TOOL_LABEL_KEYS.filter((key) => key !== 'file');

type ToolTone =
  | 'search'
  | 'evidence'
  | 'code'
  | 'files'
  | 'edit'
  | 'graph'
  | 'web'
  | 'shell'
  | 'image'
  | 'skill'
  | 'plan'
  | 'verification'
  | 'subagent'
  | 'default';

function getToolTone(name?: string): ToolTone {
  const lower = (name || '').toLowerCase();
  if (lower.includes('knowledge_graph') || lower.includes('related_concepts') || lower.includes('chunk_context')) {
    return 'graph';
  }
  if (lower.includes('retrieve') || lower.includes('evidence') || lower.includes('document')) {
    return 'evidence';
  }
  if (lower.includes('code')) return 'code';
  if (lower.includes('grep') || lower.includes('glob') || lower.includes('read_file') || lower.includes('list_dir')) {
    return 'files';
  }
  if (lower.includes('edit') || lower.includes('patch') || lower.includes('write') || lower.includes('create_file')) {
    return 'edit';
  }
  if (lower.includes('web') || lower.includes('url') || lower.includes('download') || lower.includes('desktop')) {
    return 'web';
  }
  if (lower.includes('shell') || lower.includes('terminal') || lower.includes('command')) return 'shell';
  if (lower.includes('image')) return 'image';
  if (lower.includes('skill')) return 'skill';
  if (lower.includes('plan')) return 'plan';
  if (lower.includes('verification') || lower.includes('security') || lower.includes('approval')) {
    return 'verification';
  }
  if (lower.includes('subagent')) return 'subagent';
  if (lower.includes('search')) return 'search';
  return 'default';
}

function getToolIcon(name?: string) {
  const lower = (name || '').toLowerCase();
  for (const [key, Icon] of Object.entries(TOOL_ICONS)) {
    if (lower.includes(key)) return Icon;
  }
  return Wrench;
}

function toolLeafName(name: string): string {
  const parts = name
    .split(/[.:/]/)
    .filter(Boolean)
    .map((part) => part.trim())
    .filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1] : name.trim();
}

function humanizeToolName(name: string): string {
  const leaf = toolLeafName(name).replace(/[_-]+/g, ' ').trim();
  if (!leaf) return 'Tool';
  return leaf.replace(/\b[a-z]/g, (char) => char.toUpperCase());
}

function getToolDisplayName(name: string): string {
  const lower = name.toLowerCase();
  const leaf = toolLeafName(lower);
  if (TOOL_LABELS[lower]) return TOOL_LABELS[lower];
  if (TOOL_LABELS[leaf]) return TOOL_LABELS[leaf];
  const key = TOOL_LABEL_SUBSTRING_KEYS.find((candidate) => lower.includes(candidate));
  return key ? TOOL_LABELS[key] : humanizeToolName(name);
}

function getFileChangeDisplayName(name: string, isFileChange: boolean, operation?: string): string | null {
  if (!isFileChange) return null;
  const lower = name.toLowerCase();
  if (operation === 'create' || lower.includes('create_file')) return TOOL_LABELS.create_file;
  if (lower.includes('multi_edit') || lower.includes('apply_patch')) return TOOL_LABELS.multi_edit;
  if (lower.includes('write_note')) return TOOL_LABELS.write_note;
  if (lower.includes('download_asset')) return TOOL_LABELS.download_asset;
  return TOOL_LABELS.edit_file;
}

function parseSearchResults(content: string): SearchResultItem[] | null {
  const blocks = content.split(/---\s*Result\s+\d+\s*\(score:\s*([\d.]+)\)\s*---/);
  // blocks[0] is preamble (e.g. "Found N results:"), then pairs of [score, body]
  if (blocks.length < 3) return null;

  const items: SearchResultItem[] = [];
  for (let i = 1; i < blocks.length; i += 2) {
    const score = parseFloat(blocks[i]);
    const body = (blocks[i + 1] || '').trim();

    const get = (key: string): string => {
      const m = body.match(new RegExp(`^${key}:\\s*(.+)`, 'm'));
      return m ? m[1].trim() : '';
    };

    const contentMatch = body.match(/^Content:\s*\n([\s\S]*)/m);
    const preview = contentMatch ? contentMatch[1].trim().slice(0, 200) : '';

    items.push({ score, source: get('Source'), path: get('Path'), title: get('Title'), preview });
  }
  return items.length > 0 ? items : null;
}

function parseArgsRecord(raw?: string): Record<string, unknown> | null {
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw);
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed)
      ? parsed as Record<string, unknown>
      : null;
  } catch {
    return null;
  }
}

function getManagedServiceDisplayName(name: string, rawArgs?: string): string | null {
  if (toolLeafName(name.toLowerCase()) !== 'run_shell') return null;
  const args = parseArgsRecord(rawArgs);
  if (!args) return null;
  const action = typeof args.service_action === 'string'
    ? args.service_action.trim().toLowerCase()
    : 'run';
  if (action === 'status') return 'Check service';
  if (action === 'stop') return 'Stop service';
  return args.background === true ? 'Start service' : null;
}

const FILE_TARGET_ARG_KEYS = [
  'path',
  'file',
  'filename',
  'filePath',
  'filepath',
  'targetPath',
  'target_path',
  'resourcePath',
  'resource_path',
  'absolutePath',
  'absolute_path',
];

const FILE_NEW_TEXT_ARG_KEYS = [
  'content',
  'text',
  'body',
  'new_str',
  'newStr',
  'newString',
  'new_string',
  'replacement',
  'newContent',
  'new_content',
];

const FILE_OLD_TEXT_ARG_KEYS = [
  'old_str',
  'oldStr',
  'oldString',
  'old_string',
  'oldContent',
  'old_content',
  'search',
  'old',
];

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function decodeJsonStringFragment(raw: string, start: number): string {
  let out = '';
  let escaped = false;
  for (let i = start; i < raw.length; i += 1) {
    const char = raw[i];
    if (escaped) {
      if (char === 'n') out += '\n';
      else if (char === 'r') out += '\r';
      else if (char === 't') out += '\t';
      else if (char === 'b') out += '\b';
      else if (char === 'f') out += '\f';
      else if (char === 'u') {
        const hex = raw.slice(i + 1, i + 5);
        if (/^[0-9a-fA-F]{4}$/.test(hex)) {
          out += String.fromCharCode(parseInt(hex, 16));
          i += 4;
        } else {
          out += char;
        }
      } else {
        out += char;
      }
      escaped = false;
      continue;
    }
    if (char === '\\') {
      escaped = true;
      continue;
    }
    if (char === '"') break;
    out += char;
  }
  return out;
}

function extractJsonStringFieldFragment(raw: string | undefined, key: string): string | null {
  if (!raw) return null;
  const match = new RegExp(`"${escapeRegExp(key)}"\\s*:\\s*"`, 'i').exec(raw);
  if (!match) return null;
  const value = decodeJsonStringFragment(raw, match.index + match[0].length);
  return value.trim().length > 0 ? value : '';
}

function extractStringArg(raw: string | undefined, keys: string[]): string | null {
  const parsed = parseArgsRecord(raw);
  if (parsed) {
    for (const key of keys) {
      const value = parsed[key];
      if (typeof value === 'string') return value;
    }
  }
  for (const key of keys) {
    const value = extractJsonStringFieldFragment(raw, key);
    if (value != null) return value;
  }
  return null;
}

function countTextLines(value: string | null): number {
  if (!value) return 0;
  const normalized = value.replace(/\r\n/g, '\n').replace(/\r/g, '\n');
  const withoutTrailingNewline = normalized.endsWith('\n')
    ? normalized.slice(0, -1)
    : normalized;
  if (!withoutTrailingNewline) return 0;
  return withoutTrailingNewline.split('\n').length;
}

function countPatchLines(patch: string): { additions: number; deletions: number } {
  let additions = 0;
  let deletions = 0;
  for (const line of patch.replace(/\r\n/g, '\n').replace(/\r/g, '\n').split('\n')) {
    if (line.startsWith('+++') || line.startsWith('---')) continue;
    if (line.startsWith('+')) additions += 1;
    if (line.startsWith('-')) deletions += 1;
  }
  return { additions, deletions };
}

function inferFileChangeOperation(toolName: string): string {
  const lower = toolName.toLowerCase();
  if (lower.includes('create_file')) return 'create';
  if (lower.includes('multi_edit') || lower.includes('apply_patch')) return 'multi_edit';
  if (lower.includes('download_asset')) return 'download';
  return 'edit';
}

function deriveFileChangeStatsFromArgs({
  rawArgs,
  toolName,
  isFileChange,
}: {
  rawArgs?: string;
  toolName: string;
  isFileChange: boolean;
}): { stats: DiffStatsArtifact; target: string | null } | null {
  if (!isFileChange || !rawArgs) return null;
  const operation = inferFileChangeOperation(toolName);
  const path = extractStringArg(rawArgs, FILE_TARGET_ARG_KEYS);
  const patch = extractStringArg(rawArgs, ['patch', 'diff']);
  const countedPatch = patch ? countPatchLines(patch) : null;
  const newText = countedPatch ? null : extractStringArg(rawArgs, FILE_NEW_TEXT_ARG_KEYS);
  const oldText = countedPatch ? null : extractStringArg(rawArgs, FILE_OLD_TEXT_ARG_KEYS);
  const additions = countedPatch?.additions ?? countTextLines(newText);
  const deletions = operation === 'create'
    ? 0
    : countedPatch?.deletions ?? countTextLines(oldText);

  if (!path && additions === 0 && deletions === 0) return null;

  return {
    target: path ? formatToolTarget('path', path) : null,
    stats: {
      kind: 'diffStats',
      filesChanged: 1,
      additions,
      deletions,
      hunks: additions > 0 || deletions > 0 ? 1 : 0,
      operation,
      paths: path ? [path] : [],
    },
  };
}

function getToolBriefLabel(
  name: string,
  args?: string,
  displayNameOverride?: string | null,
  targetOverride?: string | null,
  argsStatus?: ToolCallEvent['argsStatus'],
  renderKind?: ToolRenderKind,
): string {
  const label = displayNameOverride
    ?? getManagedServiceDisplayName(name, args)
    ?? getToolDisplayName(name);
  const target = getToolTitleTarget({
    toolName: name,
    renderKind,
    args,
    argsStatus,
    targetOverride,
  });
  return target ? `${label} \u00b7 ${target}` : label;
}

function getToolBriefResult(
  status: ToolCallCardStatus,
  t: ReturnType<typeof useTranslation>['t'],
  content?: string,
  toolName?: string,
): string {
  if (isPendingToolCallStatus(status)) return '\u2026';
  if (status === 'error' || status === 'timedOut') return t('chat.toolBriefError');
  if (status === 'declined') return t('chat.toolBriefDeclined');
  if (status === 'cancelled') return t('chat.toolBriefCancelled');
  const lower = (toolName || '').toLowerCase();
  if (lower.includes('search') && content) {
    const match = content.match(/Found (\d+) result/i);
    if (match) return t('search.results', { count: match[1] });
  }
  if (content) {
    const lines = content.split('\n').length;
    if (lines > 3) return t('chat.toolBriefLines', { count: String(lines) });
  }
  return t('chat.toolBriefDone');
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value));
}

function extractTrustBoundary(
  artifacts: ArtifactPayload | undefined,
): TrustBoundaryArtifact | null {
  if (!isRecord(artifacts)) return null;
  const boundary = artifacts.trustBoundary;
  if (!isRecord(boundary)) return null;
  return boundary as TrustBoundaryArtifact;
}

function extractGeneratedImageArtifact(
  artifacts: ArtifactPayload | undefined,
): GeneratedImageArtifact | null {
  if (!isRecord(artifacts)) return null;
  if (artifacts.kind !== 'generatedImage') return null;
  return artifacts as unknown as GeneratedImageArtifact;
}

function extractGeneratedAudioArtifact(
  artifacts: ArtifactPayload | undefined,
): GeneratedAudioArtifact | null {
  if (!isRecord(artifacts)) return null;
  if (artifacts.kind !== 'generatedAudio') return null;
  return artifacts as unknown as GeneratedAudioArtifact;
}

function extractSkillActivationArtifact(
  artifacts: ArtifactPayload | undefined,
): SkillActivationArtifact | null {
  if (!isRecord(artifacts)) return null;
  if (artifacts.kind !== 'skillActivation') return null;
  const skill = artifacts.skill;
  if (!isRecord(skill)) return null;
  return {
    kind: 'skillActivation',
    skill: skill as SkillActivationSkill,
  };
}

function skillDisplayName(skill: SkillActivationSkill): string {
  const displayName = skill.interface?.displayName?.trim();
  if (displayName) return displayName;
  const name = skill.name?.trim();
  if (name) return name;
  return skill.id?.trim() || 'skill';
}

function parseManageSkillArgs(raw?: string): ManageSkillArgs | null {
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    return {
      action: typeof parsed.action === 'string' ? parsed.action : undefined,
      skillId:
        typeof parsed.skill_id === 'string'
          ? parsed.skill_id
          : typeof parsed.skillId === 'string'
            ? parsed.skillId
            : undefined,
      resourcePath:
        typeof parsed.resource_path === 'string'
          ? parsed.resource_path
          : typeof parsed.resourcePath === 'string'
            ? parsed.resourcePath
            : undefined,
    };
  } catch {
    return null;
  }
}

function parseImagePromptArgs(raw?: string): ImagePromptArgs {
  if (!raw) return {};
  try {
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    return {
      prompt: typeof parsed.prompt === 'string' ? parsed.prompt : undefined,
      size: typeof parsed.size === 'string' ? parsed.size : undefined,
      quality: typeof parsed.quality === 'string' ? parsed.quality : undefined,
      outputFormat: typeof parsed.output_format === 'string'
        ? parsed.output_format
        : typeof parsed.outputFormat === 'string'
          ? parsed.outputFormat
          : undefined,
      model: typeof parsed.model === 'string' ? parsed.model : undefined,
    };
  } catch {
    return {};
  }
}

function imageAspectStyle(size?: string): CSSProperties {
  const raw = (size ?? '').trim();
  const pixelMatch = raw.match(/(\d{2,5})\s*[x*]\s*(\d{2,5})/i);
  if (pixelMatch) {
    const width = Number(pixelMatch[1]);
    const height = Number(pixelMatch[2]);
    if (width > 0 && height > 0) return { aspectRatio: `${width} / ${height}` };
  }
  const ratioMatch = raw.match(/(\d{1,3})\s*:\s*(\d{1,3})/);
  if (ratioMatch) {
    const width = Number(ratioMatch[1]);
    const height = Number(ratioMatch[2]);
    if (width > 0 && height > 0) return { aspectRatio: `${width} / ${height}` };
  }
  return { aspectRatio: '1 / 1' };
}

function generatedImagePreviewPath(image: GeneratedImageArtifact): string {
  return image.previewPath || image.path || '';
}

function GeneratedAudioPreview({ audio }: { audio: GeneratedAudioArtifact }) {
  const previewPath = audio.previewPath || audio.path || '';
  const previewSrc = useMemo(() => {
    if (!previewPath) return '';
    try {
      return convertFileSrc(previewPath);
    } catch {
      return previewPath;
    }
  }, [previewPath]);

  return (
    <div className="space-y-2" data-testid="generated-audio-preview">
      <audio
        controls
        preload="metadata"
        src={previewSrc}
        className="h-10 w-full accent-accent"
        data-testid="generated-audio-player"
      />
      <div className="flex flex-wrap gap-1.5 text-[11px] text-text-tertiary">
        {[audio.provider, audio.model, audio.voice, audio.mediaType]
          .filter(Boolean)
          .map((item) => (
            <span key={String(item)} className="rounded-md border border-border/50 bg-surface-0/50 px-1.5 py-0.5">
              {String(item)}
            </span>
          ))}
        {typeof audio.bytes === 'number' && (
          <span className="rounded-md border border-border/50 bg-surface-0/50 px-1.5 py-0.5">
            {formatByteCount(audio.bytes)}
          </span>
        )}
      </div>
    </div>
  );
}

function extensionForMediaType(mediaType?: string): string {
  const lower = (mediaType ?? '').toLowerCase();
  if (lower.includes('jpeg') || lower.includes('jpg')) return 'jpg';
  if (lower.includes('webp')) return 'webp';
  if (lower.includes('gif')) return 'gif';
  return 'png';
}

function generatedImageSuggestedFilename(image: GeneratedImageArtifact): string {
  const raw = (image.suggestedFilename ?? '').trim();
  if (raw) return raw;
  return `generated-image.${extensionForMediaType(image.mediaType)}`;
}

function GeneratedImagePreview({
  image,
  compact = false,
}: {
  image: GeneratedImageArtifact;
  compact?: boolean;
}) {
  const { t } = useTranslation();
  const previewPath = generatedImagePreviewPath(image);
  const [previewSrc, setPreviewSrc] = useState(image.dataUrl ?? '');
  const [imageError, setImageError] = useState('');
  const [isSaving, setIsSaving] = useState(false);
  const [savedPath, setSavedPath] = useState('');
  const prompt = image.revisedPrompt || image.prompt;
  const maxHeight = compact ? 'max-h-32' : 'max-h-[28rem]';
  const suggestedFilename = generatedImageSuggestedFilename(image);
  const outputExtension = extensionForMediaType(image.mediaType);

  useEffect(() => {
    let cancelled = false;
    setImageError('');
    if (image.dataUrl) {
      setPreviewSrc(image.dataUrl);
      return () => {
        cancelled = true;
      };
    }
    if (!previewPath) {
      setPreviewSrc('');
      setImageError(t('chat.generatedImageLoadFailed'));
      return () => {
        cancelled = true;
      };
    }

    setPreviewSrc('');
    api.readGeneratedImageDataUrl(previewPath, image.mediaType)
      .then((dataUrl) => {
        if (!cancelled) setPreviewSrc(dataUrl);
      })
      .catch(() => {
        if (!cancelled) setPreviewSrc(convertFileSrc(previewPath));
      });

    return () => {
      cancelled = true;
    };
  }, [image.dataUrl, image.mediaType, previewPath, t]);

  const handleSave = useCallback(async () => {
    if (isSaving) return;
    const dataUrlForSave = image.dataUrl || (previewSrc.startsWith('data:image/') ? previewSrc : '');
    if (!previewPath && !dataUrlForSave) {
      toast.error(t('chat.generatedImageLoadFailed'));
      return;
    }

    setIsSaving(true);
    try {
      const outputPath = await showSaveDialog({
        defaultPath: suggestedFilename,
        filters: [
          {
            name: t('chat.generatedImageAlt'),
            extensions: [outputExtension],
          },
        ],
      });
      if (!outputPath) return;

      const result = await api.saveGeneratedImage({
        outputPath,
        sourcePath: previewPath || null,
        dataUrl: previewPath ? null : dataUrlForSave,
        mediaType: image.mediaType ?? null,
      });
      setSavedPath(result.path);
      toast.success(t('chat.generatedImageSaveSuccess'));
    } catch (error) {
      toast.error(`${t('chat.generatedImageSaveFailed')}: ${String(error)}`);
    } finally {
      setIsSaving(false);
    }
  }, [
    image.dataUrl,
    image.mediaType,
    isSaving,
    outputExtension,
    previewPath,
    previewSrc,
    suggestedFilename,
    t,
  ]);

  const handleOpenSaved = useCallback(() => {
    if (!savedPath) return;
    api.openFileInDefaultApp(savedPath).catch((error) => {
      toast.error(String(error));
    });
  }, [savedPath]);

  const handleRevealSaved = useCallback(() => {
    if (!savedPath) return;
    api.showInFileExplorer(savedPath).catch((error) => {
      toast.error(String(error));
    });
  }, [savedPath]);

  return (
    <div className={compact ? 'space-y-1.5' : 'space-y-2.5'} data-testid="generated-image-preview">
      <div className="overflow-hidden rounded-md border border-border/60 bg-surface-0">
        {previewSrc && !imageError ? (
          <img
            src={previewSrc}
            alt={image.prompt || t('chat.generatedImageAlt')}
            className={`${maxHeight} w-full object-contain`}
            data-testid="generated-image-img"
            onError={() => setImageError(t('chat.generatedImageLoadFailed'))}
          />
        ) : (
          <div className={`${compact ? 'min-h-28' : 'min-h-64'} flex items-center justify-center px-4 text-center text-xs text-text-tertiary`}>
            {imageError || t('chat.generatedImageLoading')}
          </div>
        )}
      </div>
      <div className="grid gap-1.5 text-[11px] text-text-tertiary">
        <div className="flex flex-wrap gap-1.5">
          {!compact && (
            <span className="rounded-md border border-accent/25 bg-accent/10 px-1.5 py-0.5 text-accent">
              {savedPath ? t('chat.generatedImageSaved') : t('chat.generatedImageUnsaved')}
            </span>
          )}
          {image.provider && (
            <span className="rounded-md border border-border/50 bg-surface-0/50 px-1.5 py-0.5">
              {image.provider}
            </span>
          )}
          {image.model && (
            <span className="rounded-md border border-border/50 bg-surface-0/50 px-1.5 py-0.5">
              {image.model}
            </span>
          )}
          {image.mediaType && (
            <span className="rounded-md border border-border/50 bg-surface-0/50 px-1.5 py-0.5">
              {image.mediaType.replace('image/', '').toUpperCase()}
            </span>
          )}
          {typeof image.bytes === 'number' && (
            <span className="rounded-md border border-border/50 bg-surface-0/50 px-1.5 py-0.5">
              {formatByteCount(image.bytes)}
            </span>
          )}
        </div>
        {!compact && prompt && (
          <div className="line-clamp-2 text-text-secondary">{prompt}</div>
        )}
        {!compact && (
          <div className="flex flex-wrap items-center gap-1.5">
            <Button
              type="button"
              size="sm"
              variant="secondary"
              icon={<Save className="h-3.5 w-3.5" />}
              loading={isSaving}
              onClick={handleSave}
              disabled={!previewPath && !previewSrc}
              aria-label={t('chat.generatedImageSaveAs')}
            >
              {isSaving ? t('chat.generatedImageSaving') : t('chat.generatedImageSaveAs')}
            </Button>
            {savedPath && (
              <>
                <Tooltip content={savedPath} side="bottom">
                  <span className="min-w-0 max-w-[22rem] truncate text-text-secondary">
                    {savedPath}
                  </span>
                </Tooltip>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  icon={<ExternalLink className="h-3.5 w-3.5" />}
                  onClick={handleOpenSaved}
                >
                  {t('chat.generatedImageOpen')}
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  icon={<FolderOpen className="h-3.5 w-3.5" />}
                  onClick={handleRevealSaved}
                >
                  {t('chat.generatedImageReveal')}
                </Button>
              </>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

function ImageGenerationPendingPreview({
  prompt,
  size,
  compact = false,
}: {
  prompt?: string;
  size?: string;
  compact?: boolean;
}) {
  const shouldReduceMotion = useReducedMotion();
  const aspectStyle = imageAspectStyle(size);

  return (
    <div
      className={`chat-image-loading relative overflow-hidden rounded-md border border-accent/25 bg-surface-0/80 ${
        compact ? 'min-h-28' : 'min-h-64'
      }`}
      data-reduce-motion={shouldReduceMotion ? 'true' : 'false'}
      style={aspectStyle}
    >
      <div className="absolute inset-0 opacity-80">
        <div className="absolute left-[9%] top-[11%] h-[16%] w-[28%] rounded-md border border-border/55 bg-surface-1/60" />
        <div className="absolute right-[9%] top-[12%] h-[9%] w-[18%] rounded-full border border-border/50 bg-surface-1/55" />
        <div className="absolute bottom-[18%] left-[8%] h-[35%] w-[84%] rounded-md border border-border/50 bg-surface-1/45" />
        <div className="absolute bottom-[24%] left-[13%] h-[20%] w-[24%] rounded-md border border-border/45 bg-surface-2/45" />
        <div className="absolute bottom-[25%] right-[14%] h-[24%] w-[36%] rounded-md border border-border/45 bg-surface-2/35" />
      </div>
      <div className="chat-image-loading-grid absolute inset-0" />
      <div className="chat-image-loading-scan absolute inset-y-0 w-1/3" />
      <div className="absolute inset-0 flex items-center justify-center">
        <motion.div
          animate={shouldReduceMotion ? undefined : { opacity: [0.64, 1, 0.64], scale: [0.96, 1.04, 0.96] }}
          transition={{ duration: 2.4, repeat: Infinity, ease: 'easeInOut' }}
          className="flex h-14 w-14 items-center justify-center rounded-md border border-accent/30 bg-surface-0/80 shadow-glow"
        >
          <img src="/logo-small.svg" alt="" className="h-8 w-8 opacity-90" />
        </motion.div>
      </div>
      {!compact && prompt && (
        <div className="absolute inset-x-0 bottom-0 border-t border-border/40 bg-surface-0/75 px-3 py-2">
          <div className="line-clamp-2 text-xs leading-5 text-text-secondary">{prompt}</div>
        </div>
      )}
    </div>
  );
}

function verificationStatusLabel(
  status: VerificationOverallStatus,
  t: ReturnType<typeof useTranslation>['t'],
) {
  switch (status) {
    case 'passed':
      return t('chat.verificationPassed');
    case 'failed':
      return t('chat.verificationFailed');
    case 'partial':
      return t('chat.verificationPartial');
    case 'pending':
    default:
      return t('chat.verificationPending');
  }
}

function trustAuthorityLabel(authority: string | undefined, t: ReturnType<typeof useTranslation>['t']) {
  switch (authority) {
    case 'evidence':
      return t('chat.contextManifestRoleEvidence');
    case 'observation':
      return t('chat.trustAuthorityObservation');
    default:
      return authority || t('chat.contextManifestRoleEvidence');
  }
}

function trustVisibilityLabel(visibility: string | undefined, t: ReturnType<typeof useTranslation>['t']) {
  switch (visibility) {
    case 'source_scope':
      return t('chat.trustVisibilitySourceScope');
    case 'workspace':
      return t('chat.trustVisibilityWorkspace');
    case 'current_chat':
      return t('chat.trustVisibilityCurrentChat');
    default:
      return visibility || '';
  }
}

function buildSubagentRun(
  toolName: string,
  args: string | undefined,
  status: ToolCallCardStatus,
  content: string | undefined,
  isError: boolean | undefined,
  artifacts: ArtifactPayload | undefined,
) {
  if (toolName !== 'spawn_subagent') return null;
  const artifact = extractSubagentArtifact(artifacts);
  const parsedArgs = parseSubagentArguments(args);
  const task = artifact?.task ?? parsedArgs?.task;
  if (!task) return null;
  const runStatus: 'running' | 'done' | 'error' =
    isPendingToolCallStatus(status)
      ? 'running'
      : status === 'done'
        ? 'done'
        : 'error';
  return {
    id: `${toolName}-${task}`,
    status: runStatus,
    task,
    roleId: artifact?.roleId ?? parsedArgs?.roleId ?? null,
    roleName: artifact?.roleName ?? null,
    role: artifact?.role ?? parsedArgs?.role ?? null,
    expectedOutput: artifact?.expectedOutput ?? parsedArgs?.expectedOutput ?? null,
    acceptanceCriteria: artifact?.acceptanceCriteria ?? parsedArgs?.acceptanceCriteria ?? null,
    evidenceChunkIds: artifact?.evidenceChunkIds ?? parsedArgs?.evidenceChunkIds ?? null,
    evidenceHandoff: artifact?.evidenceHandoff ?? null,
    requestedSourceScope: artifact?.requestedSourceScope ?? parsedArgs?.sourceIds ?? null,
    effectiveSourceScope: artifact?.effectiveSourceScope ?? null,
    requestedAllowedTools: artifact?.requestedAllowedTools ?? parsedArgs?.allowedTools ?? null,
    allowedSkills: artifact?.allowedSkills ?? null,
    parallelGroup: artifact?.parallelGroup ?? parsedArgs?.parallelGroup ?? null,
    deliverableStyle: artifact?.deliverableStyle ?? parsedArgs?.deliverableStyle ?? null,
    returnSections: artifact?.returnSections ?? parsedArgs?.returnSections ?? null,
    result: artifact?.result ?? undefined,
    finishReason: artifact?.finishReason ?? null,
    usageTotal: artifact?.usageTotal ?? null,
    toolEvents: artifact?.toolEvents ?? [],
    thinking: artifact?.thinking ?? null,
    sourceScopeApplied: artifact?.sourceScopeApplied ?? false,
    allowedTools: artifact?.allowedTools ?? null,
    argumentsText: args,
    isError,
    content,
  };
}

/* ------------------------------------------------------------------ */
/*  Sub-components                                                     */
/* ------------------------------------------------------------------ */

function SearchResultCards({ items }: { items: SearchResultItem[] }) {
  return (
    <div className="space-y-2">
      {items.map((item, i) => (
        <div
          key={i}
          className="flex items-start gap-2 p-2 rounded-md bg-surface-0/50 border border-border/50"
        >
          {/* Score indicator */}
          <div
            className={`shrink-0 w-1 h-8 rounded-full ${
              item.score >= 0.8
                ? 'bg-success'
                : item.score >= 0.5
                  ? 'bg-warning'
                  : 'bg-text-tertiary'
            }`}
          />
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2 mb-0.5">
              <FileBadge path={item.path} />
              <span className="text-[11px] text-text-tertiary">
                {(item.score * 100).toFixed(0)}%
              </span>
            </div>
            {item.title && (
              <div className="text-xs font-medium text-text-primary truncate">
                {item.title}
              </div>
            )}
            {item.preview && (
              <div className="text-[11px] text-text-secondary line-clamp-2 mt-0.5">
                {item.preview}
              </div>
            )}
          </div>
        </div>
      ))}
    </div>
  );
}

function TrustBoundaryPills({ boundary }: { boundary: TrustBoundaryArtifact }) {
  const { t } = useTranslation();
  const visibilityLabel = trustVisibilityLabel(boundary.visibility, t);
  return (
    <div className="mb-2 flex flex-wrap gap-1.5 text-[10px] text-text-tertiary">
      <span className="rounded-md border border-border/50 bg-surface-0/50 px-1.5 py-0.5">
        {trustAuthorityLabel(boundary.authority, t)}
      </span>
      <span className="rounded-md border border-border/50 bg-surface-0/50 px-1.5 py-0.5">
        {t('chat.trustCanInstruct')}: {boundary.canInstruct ? t('chat.trustCanInstructYes') : t('chat.trustCanInstructNo')}
      </span>
      {visibilityLabel && (
        <span className="rounded-md border border-border/50 bg-surface-0/50 px-1.5 py-0.5">
          {visibilityLabel}
        </span>
      )}
    </div>
  );
}

function SkillActivationPanel({
  activation,
  compact = false,
}: {
  activation: SkillActivationArtifact;
  compact?: boolean;
}) {
  const { t } = useTranslation();
  const skill = activation.skill;
  const name = skillDisplayName(skill);
  const description =
    skill.interface?.shortDescription?.trim() ||
    skill.description?.trim() ||
    '';
  const sourceLabel = skill.builtin
    ? t('chat.skillActivationBuiltin')
    : t('chat.skillActivationUser');
  const policyLabel = skill.policy?.allowImplicitInvocation === false
    ? t('chat.skillActivationExplicit')
    : t('chat.skillActivationImplicit');

  return (
    <div className={`rounded-md border border-success/20 bg-success/8 ${compact ? 'p-2' : 'p-3'}`}>
      <div className="flex min-w-0 items-center gap-2">
        <BookOpen className="h-3.5 w-3.5 shrink-0 text-success" />
        <div className="min-w-0 truncate text-xs font-medium text-text-primary">
          {name}
        </div>
      </div>
      {description && (
        <div className="mt-1 line-clamp-2 text-[11px] leading-relaxed text-text-secondary">
          {description}
        </div>
      )}
      <div className="mt-2 flex flex-wrap gap-1.5 text-[10px] text-text-tertiary">
        <span className="rounded-md border border-success/20 bg-surface-0/45 px-1.5 py-0.5 text-success">
          {t('chat.skillActivationReady')}
        </span>
        <span className="rounded-md border border-border/50 bg-surface-0/45 px-1.5 py-0.5">
          {sourceLabel}
        </span>
        <span className="rounded-md border border-border/50 bg-surface-0/45 px-1.5 py-0.5">
          {policyLabel}
        </span>
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*  Component                                                          */
/* ------------------------------------------------------------------ */

export function ToolCallCard({
  callId,
  toolName,
  arguments: args,
  status,
  owner,
  renderKind,
  capabilities,
  durationMs,
  progressNote,
  content,
  isError,
  artifacts,
  compact,
  inline,
  trace,
  argsStatus,
  onQuestionSubmit,
  questionAnswered,
}: ToolCallCardProps) {
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();
  const safeToolName =
    typeof toolName === 'string' && toolName.trim().length > 0
      ? toolName
      : 'unknown_tool';
  const Icon = getToolIcon(safeToolName);
  const manageSkillArgs = parseManageSkillArgs(args);
  const skillActivation = extractSkillActivationArtifact(artifacts);
  const skillActivationName = skillActivation
    ? skillDisplayName(skillActivation.skill)
    : manageSkillArgs?.action === 'activate_skill'
      ? manageSkillArgs.skillId
      : undefined;
  const fileDiff = useMemo(() => extractFileDiffArtifact(artifacts), [artifacts]);
  const diffStats = useMemo(() => extractDiffStatsArtifact(artifacts), [artifacts]);
  const isFileChangeRender = isFileChangeToolRender(safeToolName, renderKind);
  const isCommandExecutionRender = isCommandExecutionTool(safeToolName, renderKind);
  const isPending = isPendingToolCallStatus(status);
  const argumentFileChangeStats = useMemo(
    () => deriveFileChangeStatsFromArgs({
      rawArgs: args,
      toolName: safeToolName,
      isFileChange: isFileChangeRender,
    }),
    [args, isFileChangeRender, safeToolName],
  );
  const headerDiffStats = diffStats ?? argumentFileChangeStats?.stats ?? null;
  const fileChangeDisplayName = getFileChangeDisplayName(
    safeToolName,
    isFileChangeRender,
    headerDiffStats?.operation ?? fileDiff?.operation,
  );
  const fileChangeTarget = isFileChangeRender
    ? getStableFileChangeTarget(fileDiff, headerDiffStats) ?? argumentFileChangeStats?.target ?? null
    : null;
  const briefTargetOverride = isFileChangeRender ? (fileChangeTarget ?? '') : fileChangeTarget;
  const formattedArgs = formatToolArgumentsForDisplay(args);
  const briefLabel = skillActivationName
    ? (
        skillActivation
          ? t('chat.skillActivatedLabel', { name: skillActivationName })
          : t('chat.skillActivatingLabel', { name: skillActivationName })
      )
    : getToolBriefLabel(
        safeToolName,
        args,
        fileChangeDisplayName,
        briefTargetOverride,
        argsStatus,
        renderKind,
      );
  const briefResult = getToolBriefResult(status, t, content, safeToolName);
  const durationLabel = formatDurationMs(durationMs);
  const resourceKeyCount = Array.isArray(capabilities?.resourceKeys)
    ? capabilities.resourceKeys.length
    : 0;
  const capabilitySummary = capabilities
    ? [
        owner?.name ?? null,
        capabilities.readOnly ? t('chat.capabilityReadOnly') : t('chat.capabilityWrites'),
        capabilities.concurrencySafe ? t('chat.capabilityParallel') : t('chat.capabilitySerial'),
        capabilities.interruptBehavior === 'cancel' ? t('chat.capabilityCancellable') : t('chat.capabilityBlocking'),
        resourceKeyCount > 0 ? t('chat.capabilityResources', { count: String(resourceKeyCount) }) : null,
      ].filter(Boolean).join(' · ')
    : null;
  const inputPresentation = getToolInputPresentation({
    toolName: safeToolName,
    renderKind,
    argsStatus,
    status,
  });
  const subagentRun = useMemo(
    () => buildSubagentRun(safeToolName, args, status, content, isError, artifacts),
    [safeToolName, args, status, content, isError, artifacts],
  );
  const subagentBatch = useMemo(() => extractSubagentBatchArtifact(artifacts), [artifacts]);
  const subagentJudgement = useMemo(() => extractSubagentJudgementArtifact(artifacts), [artifacts]);
  const planArtifact = useMemo(() => extractPlanArtifact(artifacts), [artifacts]);
  const verificationArtifact = useMemo(() => extractVerificationArtifact(artifacts), [artifacts]);
  const trustBoundary = useMemo(() => extractTrustBoundary(artifacts), [artifacts]);
  const generatedImage = useMemo(() => extractGeneratedImageArtifact(artifacts), [artifacts]);
  const generatedAudio = useMemo(() => extractGeneratedAudioArtifact(artifacts), [artifacts]);
  const graphUsage = useMemo(() => extractGraphAgentUsage(artifacts), [artifacts]);
  const questionRequest = useMemo(
    () => safeToolName === 'request_user_input'
      ? extractQuestionRequest(callId ?? '', args, artifacts)
      : null,
    [args, artifacts, callId, safeToolName],
  );
  const imageArgs = useMemo(() => parseImagePromptArgs(args), [args]);
  const isImageRender = renderKind === 'image' || safeToolName.toLowerCase() === 'generate_image';
  const showImagePendingPreview = isImageRender && isPending && !generatedImage;
  const isSearchDone =
    safeToolName.toLowerCase().includes('search') && status === 'done' && !!content;
  const searchItems = useMemo(
    () => (isSearchDone ? parseSearchResults(content!) : null),
    [isSearchDone, content],
  );
  const openTerminalDock = useCallback(() => {
    window.dispatchEvent(new Event(TERMINAL_OPEN_EVENT));
  }, []);

  const [expanded, setExpanded] = useState(
    () => !isPending && Boolean(subagentBatch || subagentJudgement),
  );

  useEffect(() => {
    if (!isPending && graphUsage) {
      saveGraphAgentUsage({
        ...graphUsage,
        createdAt: new Date().toISOString(),
      });
    }
  }, [graphUsage, isPending]);

  // Completed tool details collapse into the timeline summary. Collaboration
  // results stay open because their worker evidence and judgement are primary
  // output, rather than low-value implementation detail.
  useEffect(() => {
    if (!isPending) {
      setExpanded(Boolean(subagentBatch || subagentJudgement));
    }
  }, [isPending, subagentBatch, subagentJudgement]);

  if (questionRequest && !inline) {
    return (
      <QuestionRequestPanel
        request={questionRequest}
        answered={questionAnswered}
        onSubmit={onQuestionSubmit}
      />
    );
  }

  if (inline) {
    return (
      <span className="inline-flex items-center gap-1">
        <Icon className="h-2.5 w-2.5 shrink-0" />
        <span className="font-medium text-text-secondary">{briefLabel}</span>
        <span className="text-text-tertiary/40">→</span>
        <span>{briefResult}</span>
      </span>
    );
  }

  const failedStatus = isUnsuccessfulToolCallStatus(status);
  const failedResultSummary = failedStatus ? toolResultHeaderSummary(content) : null;
  const baseHeaderSummary = skillActivation
    ? t('chat.skillActivationReady')
    : subagentBatch
      ? [
          subagentBatch.batchGoal || t('chat.subagentParallelRun'),
          typeof subagentBatch.effectiveMaxParallel === 'number'
            ? t('chat.subagentParallelCount', { count: String(subagentBatch.effectiveMaxParallel) })
            : null,
        ].filter((value): value is string => Boolean(value)).join(' · ')
    : subagentJudgement
      ? subagentJudgement.task || t('chat.subagentJudgementFallback')
    : planArtifact
      ? t('chat.planStepsCompleted', {
          completed: String(planArtifact.steps.filter(step => step.status === 'completed').length),
          total: String(planArtifact.steps.length),
        })
      : verificationArtifact
        ? t('chat.verificationStatus', {
          status: verificationStatusLabel(verificationArtifact.overallStatus ?? 'pending', t),
        })
      : searchItems
        ? t('search.results', { count: String(searchItems.length) })
        : generatedImage
          ? t('chat.generatedImageReady')
        : showImagePendingPreview
          ? null
        : graphUsage
          ? t('chat.graphContextSummary', {
              nodes: String(graphUsage.usedGraphNodes.length),
              edges: String(graphUsage.usedGraphEdges.length),
              documents: String(graphUsage.usedDocuments.length),
            })
        : headerDiffStats
          ? isPending
            ? null
            : `${headerDiffStats.operation === 'create' ? t('chat.fileDiffCreated') : t('chat.fileDiffModified')}`
        : isPending && progressNote
          ? progressNote
        : failedStatus
          ? failedResultSummary ?? briefResult
          : null;
  const headerSummary = [
    baseHeaderSummary,
    !isPending && durationLabel ? durationLabel : null,
  ].filter((value): value is string => Boolean(value)).join(' · ') || null;

  // The moving edge communicates activity without spending header space on a
  // spinner or a redundant visible "running" label. Terminal states retain a
  // compact mark with an accessible label.
  const StatusIcon = isPending ? null : failedStatus ? XCircle : CheckCircle2;
  const statusLabel = isPending ? t('chat.toolRunning') : briefResult;
  const toolCardState = isPending ? 'running' : failedStatus ? 'error' : 'done';
  const toolCardAriaLabel = [
    briefLabel,
    headerDiffStats ? `+${headerDiffStats.additions}, -${headerDiffStats.deletions}` : null,
    statusLabel,
  ].filter((value): value is string => Boolean(value)).join(', ');
  const traceSoft = !failedStatus;
  const hasStructuredResult = Boolean(
    searchItems ||
    subagentRun ||
    subagentBatch ||
    subagentJudgement ||
    skillActivation ||
    planArtifact ||
    verificationArtifact ||
    fileDiff ||
    diffStats ||
    generatedImage ||
    generatedAudio ||
    graphUsage,
  );
  const visibleResultContent = visibleToolResult(content, hasStructuredResult, failedStatus);
  const hideCompletedArgs = !isPending && !failedStatus && Boolean(
    visibleResultContent || hasStructuredResult,
  );
  const visibleFormattedArgs =
    fileDiff || diffStats || isFileChangeRender || inputPresentation !== 'final' || hideCompletedArgs
      ? null
      : formattedArgs;
  const liveFileDiff = trace && isPending && Boolean(fileDiff);
  const detailsExpanded = expanded;
  const expandableDetails = Boolean(
    visibleFormattedArgs ||
    visibleResultContent ||
    searchItems ||
    subagentRun ||
    subagentBatch ||
    subagentJudgement ||
    skillActivation ||
    planArtifact ||
    verificationArtifact ||
    fileDiff ||
    diffStats ||
    generatedImage ||
    graphUsage ||
    showImagePendingPreview,
  );
  const toolTone = getToolTone(safeToolName);
  const traceToneClass = 'tool-tone-surface';
  const traceIconToneClass = 'tool-tone-icon';
  const statusBadgeClass = failedStatus
    ? 'border-danger/25 bg-danger/10 text-danger'
    : 'border-success/20 bg-success/10 text-success';
  const traceDetailBorderClass = 'tool-tone-detail';
  const tracePreviewText = headerSummary;

  if (trace) {
    return (
      <div className="my-0.5 max-w-full">
        <button
          type="button"
          onClick={() => expandableDetails && setExpanded((prev) => !prev)}
          aria-expanded={expandableDetails ? detailsExpanded : undefined}
          aria-label={toolCardAriaLabel}
          aria-busy={isPending}
          className={`chat-tool-card group inline-grid min-h-8 max-w-full grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-x-2 overflow-hidden rounded-md border border-l-2 px-2 py-1 text-left transition-colors disabled:cursor-default sm:max-w-[36rem] ${expandableDetails ? 'cursor-pointer' : 'cursor-default'} ${traceToneClass}`}
          disabled={!expandableDetails}
          title={capabilitySummary ?? undefined}
          data-testid="tool-call-card"
          data-tool-state={toolCardState}
          data-tool-tone={toolTone}
        >
          <span className={`flex h-5 w-5 shrink-0 items-center justify-center rounded-md border ${traceIconToneClass}`}>
            <Icon className="h-3 w-3 shrink-0" />
          </span>
          <span className="flex min-w-0 items-baseline gap-1.5">
            <span className="min-w-0 truncate text-[11px] font-medium leading-4 text-text-primary">
              {briefLabel}
            </span>
            {tracePreviewText && (
              <span className="hidden min-w-0 truncate text-[10px] leading-3 text-text-tertiary sm:inline">
                {tracePreviewText}
              </span>
            )}
          </span>
          <span className="flex shrink-0 items-center gap-1 pl-1">
            {headerDiffStats ? (
              <span className="inline-flex">
                <DiffStatsTicker
                  additions={headerDiffStats.additions}
                  deletions={headerDiffStats.deletions}
                  filesChanged={headerDiffStats.filesChanged}
                  replacements={headerDiffStats.replacements}
                  compact
                  live={isPending}
                  testIdPrefix="tool-card-header"
                />
              </span>
            ) : null}
            {StatusIcon && (
              <span
                className={`inline-flex h-5 w-5 items-center justify-center rounded-md border ${statusBadgeClass}`}
                data-testid="tool-card-status"
                role="img"
                aria-label={statusLabel}
                title={statusLabel}
              >
                <StatusIcon className="h-3 w-3 shrink-0" />
              </span>
            )}
            {expandableDetails && (
              detailsExpanded
                ? <ChevronUp className="h-3 w-3 shrink-0 text-text-tertiary" />
                : <ChevronDown className="h-3 w-3 shrink-0 text-text-tertiary" />
            )}
          </span>
        </button>

        <AnimatePresence initial={false}>
          {detailsExpanded && expandableDetails && (
            <motion.div
              {...getSoftCollapseMotion(!!shouldReduceMotion)}
              className="overflow-hidden"
            >
              <div className={`ml-3 mt-1.5 max-w-[36rem] space-y-2 border-l py-1.5 pl-3 pr-1 ${traceDetailBorderClass}`}>
                {visibleFormattedArgs ? (
                  <div className="break-words rounded-md border border-border/35 bg-surface-0/45 px-2 py-1 text-[11px] leading-relaxed text-text-tertiary">
                    {visibleFormattedArgs}
                  </div>
                ) : null}
                {skillActivation ? (
                  <SkillActivationPanel activation={skillActivation} compact />
                ) : generatedImage ? (
                  <GeneratedImagePreview image={generatedImage} compact />
                ) : showImagePendingPreview ? (
                  <ImageGenerationPendingPreview
                    prompt={imageArgs.prompt}
                    size={imageArgs.size}
                    compact
                  />
                ) : subagentRun ? (
                  <SubagentCard run={subagentRun} compact defaultOpen />
                ) : subagentBatch ? (
                  <div className="space-y-2">
                    {subagentBatch.runs.map((run) => (
                      <SubagentCard key={run.id} run={run} compact defaultOpen />
                    ))}
                  </div>
                ) : subagentJudgement ? (
                  <div className="border-l border-border/50 pl-3">
                    <div className="mb-1 flex flex-wrap items-center gap-1.5 text-xs text-text-secondary">
                      <span className="font-medium text-text-primary">
                        {subagentJudgement.task || t('chat.subagentJudgementFallback')}
                      </span>
                      <span className="rounded-full border border-border/60 bg-surface-0/55 px-2 py-0.5">
                        {subagentJudgement.decisionMode}
                      </span>
                      {subagentJudgement.confidence && (
                        <span className="rounded-full border border-border/60 bg-surface-0/55 px-2 py-0.5">
                          {t('chat.subagentConfidence', { value: subagentJudgement.confidence })}
                        </span>
                      )}
                      {subagentJudgement.winnerIds.length > 0 && (
                        <span className="rounded-full border border-accent/25 bg-accent/10 px-2 py-0.5 text-accent">
                          {t('chat.subagentWinners', { value: subagentJudgement.winnerIds.join(', ') })}
                        </span>
                      )}
                    </div>
                    <div className="text-sm text-text-primary">{subagentJudgement.summary}</div>
                    {subagentJudgement.rationale && (
                      <div className="mt-2 text-xs text-text-secondary">{subagentJudgement.rationale}</div>
                    )}
                    {subagentJudgement.rubric && subagentJudgement.rubric.length > 0 && (
                      <div className="mt-3">
                        <div className="mb-1 text-[11px] uppercase tracking-[0.14em] text-text-tertiary">
                          {t('chat.subagentRubric')}
                        </div>
                        <div className="flex flex-wrap gap-1.5">
                          {subagentJudgement.rubric.map((item, index) => (
                            <span
                              key={`trace-judge-rubric-${index}`}
                              className="inline-flex items-center rounded-md border border-border/60 bg-surface-0 px-2 py-1 text-[11px] text-text-secondary"
                            >
                              {item}
                            </span>
                          ))}
                        </div>
                      </div>
                    )}
                  </div>
                ) : searchItems ? (
                  <>
                    {trustBoundary && <TrustBoundaryPills boundary={trustBoundary} />}
                    <SearchResultCards items={searchItems} />
                  </>
                ) : graphUsage ? (
                  <KnowledgeGraphUsagePanel usage={graphUsage} compact />
                ) : planArtifact ? (
                  <PlanPanel plan={planArtifact} />
                ) : verificationArtifact ? (
                  <VerificationPanel verification={verificationArtifact} />
                ) : fileDiff ? (
                  <FileDiffPreview diff={fileDiff} compact live={liveFileDiff} />
                ) : diffStats ? (
                  <>
                    <DiffStatsSummaryPanel stats={diffStats} />
                    {visibleResultContent && (
                      <ToolResultSurface content={visibleResultContent} error={failedStatus} compact />
                    )}
                  </>
                ) : visibleResultContent ? (
                  <ToolResultSurface content={visibleResultContent} error={failedStatus} compact />
                ) : null}
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    );
  }

  if (compact) {
    return (
      <div className="my-0.5 max-w-full">
        <button
          type="button"
          onClick={() => expandableDetails && setExpanded((p) => !p)}
          aria-expanded={expandableDetails ? expanded : undefined}
          aria-label={toolCardAriaLabel}
          aria-busy={isPending}
          disabled={!expandableDetails}
          className={`chat-tool-card inline-grid min-h-8 max-w-full grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-x-2 overflow-hidden rounded-md border border-l-2 px-2 py-1 text-left transition-colors disabled:cursor-default sm:max-w-[32rem] ${expandableDetails ? 'cursor-pointer' : 'cursor-default'} ${traceToneClass}`}
          data-testid="tool-call-card"
          data-tool-state={toolCardState}
          data-tool-tone={toolTone}
        >
          <span className={`flex h-5 w-5 shrink-0 items-center justify-center rounded-md border ${traceIconToneClass}`}>
            <Icon className="h-3 w-3 shrink-0" />
          </span>
          <span className="flex min-w-0 items-baseline gap-1.5">
            <span className="min-w-0 truncate text-[11px] font-medium leading-4 text-text-primary">
              {briefLabel}
            </span>
            {headerSummary && (
              <span className="hidden min-w-0 truncate text-[10px] leading-3 text-text-tertiary sm:inline">
                {headerSummary}
              </span>
            )}
          </span>
          <span className="flex shrink-0 items-center gap-1 pl-1">
            {headerDiffStats ? (
              <span className="inline-flex">
                <DiffStatsTicker
                  additions={headerDiffStats.additions}
                  deletions={headerDiffStats.deletions}
                  filesChanged={headerDiffStats.filesChanged}
                  replacements={headerDiffStats.replacements}
                  compact
                  live={isPending}
                  testIdPrefix="tool-card-header"
                />
              </span>
            ) : null}
            {StatusIcon && (
              <span
                className={`inline-flex h-5 w-5 items-center justify-center rounded-md border ${statusBadgeClass}`}
                data-testid="tool-card-status"
                role="img"
                aria-label={statusLabel}
                title={statusLabel}
              >
                <StatusIcon className="h-2.5 w-2.5 shrink-0" />
              </span>
            )}
            {expandableDetails && (
              detailsExpanded
                ? <ChevronUp className="h-3 w-3 shrink-0 text-text-tertiary" />
                : <ChevronDown className="h-3 w-3 shrink-0 text-text-tertiary" />
            )}
          </span>
        </button>
        <AnimatePresence initial={false}>
          {detailsExpanded && expandableDetails && (
            <motion.div
              {...getSoftCollapseMotion(!!shouldReduceMotion)}
              className="overflow-hidden"
            >
              <div className={`ml-3 mt-1.5 max-w-[32rem] space-y-1.5 border-l py-1 pl-2.5 pr-1 ${traceDetailBorderClass}`}>
                {visibleFormattedArgs ? (
                  <div className="break-words rounded-md border border-border/35 bg-surface-0/45 px-1.5 py-0.5 text-[10px] text-text-tertiary">
                    {visibleFormattedArgs}
                  </div>
                ) : null}
                {skillActivation ? (
                  <SkillActivationPanel activation={skillActivation} compact />
                ) : generatedImage ? (
                  <GeneratedImagePreview image={generatedImage} compact />
                ) : showImagePendingPreview ? (
                  <ImageGenerationPendingPreview
                    prompt={imageArgs.prompt}
                    size={imageArgs.size}
                    compact
                  />
                ) : subagentRun ? (
                  <SubagentCard run={subagentRun} compact defaultOpen />
                ) : subagentBatch ? (
                  <div className="space-y-1.5">
                    {subagentBatch.runs.map((run) => (
                      <SubagentCard key={run.id} run={run} compact defaultOpen />
                    ))}
                  </div>
                ) : subagentJudgement ? (
                  <div className="border-l border-border/50 pl-2.5">
                    <div className="mb-1 flex flex-wrap items-center gap-1.5 text-[11px] text-text-secondary">
                      <span className="font-medium text-text-primary">
                        {subagentJudgement.task || t('chat.subagentJudgementFallback')}
                      </span>
                      <span className="rounded-full border border-border/60 bg-surface-0/55 px-1.5 py-0.5">
                        {subagentJudgement.decisionMode}
                      </span>
                    </div>
                    <div className="text-xs text-text-primary">{subagentJudgement.summary}</div>
                  </div>
                ) : searchItems ? (
                  <>
                    {trustBoundary && <TrustBoundaryPills boundary={trustBoundary} />}
                    <SearchResultCards items={searchItems} />
                  </>
                ) : graphUsage ? (
                  <KnowledgeGraphUsagePanel usage={graphUsage} compact />
                ) : planArtifact ? (
                  <PlanPanel plan={planArtifact} />
                ) : verificationArtifact ? (
                  <VerificationPanel verification={verificationArtifact} />
                ) : fileDiff ? (
                  <FileDiffPreview diff={fileDiff} compact live={liveFileDiff} />
                ) : diffStats ? (
                  <div className="space-y-1.5">
                    <DiffStatsSummaryPanel stats={diffStats} />
                    {visibleResultContent && (
                      <ToolResultSurface content={visibleResultContent} error={failedStatus} compact />
                    )}
                  </div>
                ) : visibleResultContent ? (
                  <ToolResultSurface content={visibleResultContent} error={failedStatus} compact />
                ) : null}
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    );
  }

  if (isImageRender && (generatedImage || showImagePendingPreview)) {
    const imageMeta = [
      generatedImage?.provider,
      generatedImage?.model ?? imageArgs.model,
      imageArgs.size,
      imageArgs.quality,
      imageArgs.outputFormat,
    ].filter(Boolean);

    return (
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
        className="chat-image-panel chat-tool-card tool-tone-surface my-2 overflow-hidden rounded-md border"
        data-state={generatedImage ? 'ready' : 'loading'}
        data-testid="tool-call-card"
        data-tool-state={toolCardState}
        data-tool-tone={toolTone}
        aria-busy={isPending}
      >
        <div className="flex min-h-11 items-center gap-2 border-b border-border/40 px-3 py-2">
          <Icon className="h-4 w-4 shrink-0 text-accent" />
          <div className="min-w-0 flex-1">
            <div className="truncate text-xs font-medium text-text-primary">{briefLabel}</div>
            <div className="mt-0.5 flex min-w-0 flex-wrap gap-1.5 text-[11px] text-text-tertiary">
              {imageMeta.slice(0, 4).map((item) => (
                <span
                  key={String(item)}
                  className="rounded-md border border-border/45 bg-surface-0/45 px-1.5 py-0.5"
                >
                  {String(item)}
                </span>
              ))}
            </div>
          </div>
          {StatusIcon && (
            <span
              className={`inline-flex h-5 w-5 shrink-0 items-center justify-center rounded-md border ${statusBadgeClass}`}
              data-testid="tool-card-status"
              role="img"
              aria-label={statusLabel}
              title={statusLabel}
            >
              <StatusIcon className="h-3 w-3" />
            </span>
          )}
        </div>
        <div className="px-3 py-3">
          {generatedImage ? (
            <GeneratedImagePreview image={generatedImage} />
          ) : (
            <ImageGenerationPendingPreview
              prompt={imageArgs.prompt}
              size={imageArgs.size}
            />
          )}
        </div>
      </motion.div>
    );
  }

  if (generatedAudio) {
    return (
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
        className="chat-tool-card tool-tone-surface my-2 overflow-hidden rounded-md border"
        data-testid="tool-call-card"
        data-tool-state={toolCardState}
        data-tool-tone={toolTone}
      >
        <div className="flex min-h-11 items-center gap-2 border-b border-border/40 px-3 py-2">
          <Volume2 className="h-4 w-4 shrink-0 text-accent" />
          <div className="min-w-0 flex-1 truncate text-xs font-medium text-text-primary">
            {briefLabel}
          </div>
          {StatusIcon && (
            <span
              className={`inline-flex h-5 w-5 shrink-0 items-center justify-center rounded-md border ${statusBadgeClass}`}
              data-testid="tool-card-status"
              role="img"
              aria-label={statusLabel}
              title={statusLabel}
            >
              <StatusIcon className="h-3 w-3" />
            </span>
          )}
        </div>
        <div className="px-3 py-3">
          <GeneratedAudioPreview audio={generatedAudio} />
        </div>
      </motion.div>
    );
  }

  if (subagentRun) {
    return (
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
        className="my-0.5"
      >
        <SubagentCard run={subagentRun} />
      </motion.div>
    );
  }

  if (subagentBatch) {
    return (
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
        className="chat-tool-card tool-tone-surface my-0.5 overflow-hidden rounded-md border p-2"
        data-testid="tool-call-card"
        data-tool-state={toolCardState}
        data-tool-tone={toolTone}
        aria-busy={isPending}
      >
        <div className="mb-2 flex flex-wrap items-center gap-1.5 text-[11px] text-text-secondary">
          <span className="font-medium text-text-primary">
            {subagentBatch.batchGoal || t('chat.subagentParallelRun')}
          </span>
          {typeof subagentBatch.effectiveMaxParallel === 'number' && (
            <span className="rounded-full border border-border/55 bg-surface-1/70 px-2 py-0.5">
              {t('chat.subagentParallelCount', { count: String(subagentBatch.effectiveMaxParallel) })}
            </span>
          )}
          {subagentBatch.workflowTemplateLabel && (
            <span
              className="rounded-full border border-border/55 bg-surface-1/70 px-1.5 py-0.5"
              title={subagentBatch.workflowTemplateDescription ?? undefined}
            >
              {subagentBatch.workflowTemplateLabel}
            </span>
          )}
          {typeof subagentBatch.completedRuns === 'number' && (
            <span className="rounded-full border border-border/55 bg-surface-1/70 px-1.5 py-0.5">
              {t('chat.subagentCompletedCount', { count: String(subagentBatch.completedRuns) })}
            </span>
          )}
          {typeof subagentBatch.failedRuns === 'number' && subagentBatch.failedRuns > 0 && (
            <span className="rounded-full border border-danger/25 bg-danger/10 px-1.5 py-0.5 text-danger">
              {t('chat.subagentFailedCount', { count: String(subagentBatch.failedRuns) })}
            </span>
          )}
        </div>
        <div className="space-y-1.5">
          {subagentBatch.runs.map(run => (
            <SubagentCard key={run.id} run={run} compact />
          ))}
        </div>
      </motion.div>
    );
  }

  if (subagentJudgement) {
    return (
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
        className="chat-tool-card tool-tone-surface my-0.5 overflow-hidden rounded-md border p-2"
        data-testid="tool-call-card"
        data-tool-state={toolCardState}
        data-tool-tone={toolTone}
        aria-busy={isPending}
      >
        <div className="mb-1.5 flex flex-wrap items-center gap-1 text-[11px] text-text-secondary">
          <span className="font-medium text-text-primary">
            {subagentJudgement.task || t('chat.subagentJudgementFallback')}
          </span>
          <span className="rounded-full border border-border/55 bg-surface-1/70 px-1.5 py-0.5">
            {subagentJudgement.decisionMode}
          </span>
          {subagentJudgement.confidence && (
            <span className="rounded-full border border-border/55 bg-surface-1/70 px-1.5 py-0.5">
              {t('chat.subagentConfidence', { value: subagentJudgement.confidence })}
            </span>
          )}
          {subagentJudgement.winnerIds.length > 0 && (
            <span className="rounded-full border border-accent/25 bg-accent/10 px-1.5 py-0.5 text-accent">
              {t('chat.subagentWinners', { value: subagentJudgement.winnerIds.join(', ') })}
            </span>
          )}
        </div>
        <div className="border-l border-border/50 pl-3 text-sm text-text-primary">
          {subagentJudgement.summary}
        </div>
        {subagentJudgement.rationale && (
          <div className="mt-2 border-l border-border/35 pl-3 text-xs text-text-secondary">
            {subagentJudgement.rationale}
          </div>
        )}
        {subagentJudgement.rubric && subagentJudgement.rubric.length > 0 && (
          <div className="mt-3">
            <div className="mb-1 text-[11px] uppercase tracking-[0.14em] text-text-tertiary">
              {t('chat.subagentRubric')}
            </div>
            <div className="flex flex-wrap gap-1.5">
              {subagentJudgement.rubric.map((item, index) => (
                <span
                  key={`judge-rubric-${index}`}
                  className="inline-flex items-center rounded-md border border-border/60 bg-surface-0 px-2 py-1 text-[11px] text-text-secondary"
                >
                  {item}
                </span>
              ))}
            </div>
          </div>
        )}
      </motion.div>
    );
  }

  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
      className="chat-tool-card chat-trace-panel tool-tone-surface my-0.5 overflow-hidden rounded-md border"
      data-trace-soft={traceSoft ? 'true' : 'false'}
      data-testid="tool-call-card"
      data-tool-state={toolCardState}
      data-tool-tone={toolTone}
      aria-busy={isPending}
    >
      {/* Header */}
      <button
        onClick={() => expandableDetails && setExpanded((p) => !p)}
        aria-expanded={expandableDetails ? expanded : undefined}
        aria-label={toolCardAriaLabel}
        disabled={!expandableDetails}
        className="grid min-h-8 w-full grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-1.5 px-2 py-1 text-left hover:bg-surface-1/85
          transition-colors duration-fast ease-out cursor-pointer disabled:cursor-default disabled:hover:bg-transparent"
      >
        <span className={`flex h-5 w-5 shrink-0 items-center justify-center rounded-md border ${traceIconToneClass}`}>
          <Icon className="h-3 w-3 shrink-0" />
        </span>
        <span className="flex min-w-0 items-baseline gap-1.5">
          <span className="min-w-0 truncate text-[11px] font-medium leading-4 text-text-primary">{briefLabel}</span>
          {headerSummary && (
            <span className="hidden min-w-0 truncate text-[10px] leading-3 text-text-tertiary sm:inline">
              {headerSummary}
            </span>
          )}
        </span>
        <span className="flex shrink-0 items-center gap-1.5 pl-1">
          {headerDiffStats ? (
            <span className="inline-flex">
              <DiffStatsTicker
                additions={headerDiffStats.additions}
                deletions={headerDiffStats.deletions}
                filesChanged={headerDiffStats.filesChanged}
                replacements={headerDiffStats.replacements}
                live={isPending}
                testIdPrefix="tool-card-header"
              />
            </span>
          ) : null}
          {StatusIcon && (
            <span
              className={`inline-flex h-5 w-5 items-center justify-center rounded-md border ${statusBadgeClass}`}
              data-testid="tool-card-status"
              role="img"
              aria-label={statusLabel}
              title={statusLabel}
            >
              <StatusIcon className="h-3 w-3 shrink-0" />
            </span>
          )}
          {expandableDetails ? (
            expanded ? (
              <ChevronUp className="h-3 w-3 shrink-0 text-text-tertiary" />
            ) : (
              <ChevronDown className="h-3 w-3 shrink-0 text-text-tertiary" />
            )
          ) : null}
        </span>
      </button>

      {/* Expandable result */}
      <AnimatePresence>
        {expanded && expandableDetails && (
          <motion.div
            {...getSoftCollapseMotion(!!shouldReduceMotion)}
            className="overflow-hidden"
          >
            <div className="border-t border-border px-3 py-2">
              {isCommandExecutionRender && (
                <div className="mb-2 flex flex-wrap items-center gap-2">
                  <Button
                    type="button"
                    size="sm"
                    variant="secondary"
                    icon={<Terminal className="h-3.5 w-3.5" />}
                    onClick={openTerminalDock}
                  >
                    {t('shortcuts.toggleTerminal')}
                  </Button>
                </div>
              )}
              {visibleFormattedArgs && (
                <div className="mb-2 rounded-md bg-surface-0/60 px-2 py-1 text-[11px] text-text-tertiary break-words">
                  {visibleFormattedArgs}
                </div>
              )}
              {skillActivation ? (
                <SkillActivationPanel activation={skillActivation} />
              ) : generatedImage ? (
                <GeneratedImagePreview image={generatedImage} />
              ) : showImagePendingPreview ? (
                <ImageGenerationPendingPreview
                  prompt={imageArgs.prompt}
                  size={imageArgs.size}
                />
              ) : planArtifact ? (
                <PlanPanel plan={planArtifact} />
              ) : verificationArtifact ? (
                <VerificationPanel verification={verificationArtifact} />
              ) : fileDiff ? (
                <FileDiffPreview diff={fileDiff} />
              ) : diffStats ? (
                <div className="space-y-2">
                  <DiffStatsSummaryPanel stats={diffStats} />
                  {visibleResultContent && (
                    <ToolResultSurface content={visibleResultContent} error={failedStatus} />
                  )}
                </div>
              ) : graphUsage ? (
                <KnowledgeGraphUsagePanel usage={graphUsage} />
              ) : searchItems ? (
                <>
                  {trustBoundary && <TrustBoundaryPills boundary={trustBoundary} />}
                  <SearchResultCards items={searchItems} />
                </>
              ) : visibleResultContent ? (
                <ToolResultSurface content={visibleResultContent} error={failedStatus} />
              ) : null}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </motion.div>
  );
}
