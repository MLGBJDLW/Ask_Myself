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
  Loader2,
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
  getStableFileChangeTarget,
  getToolBriefTarget,
} from '../../lib/streaming/toolCardPresentation';
import { extractPlanArtifact, extractVerificationArtifact } from '../../lib/taskArtifacts';
import {
  extractSubagentArtifact,
  extractSubagentBatchArtifact,
  extractSubagentJudgementArtifact,
  parseSubagentArguments,
} from '../../lib/subagentArtifacts';
import { PlanPanel, VerificationPanel } from './TaskPanels';
import type { ArtifactPayload, ToolPluginInfo, ToolRenderKind, ToolRunCapabilities } from '../../types/conversation';
import type { VerificationOverallStatus } from '../../lib/taskArtifacts';
import { SubagentCard } from './SubagentCard';
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
  toolName?: string;
  arguments?: string;
  status: ToolCallCardStatus;
  plugin?: ToolPluginInfo;
  renderKind?: ToolRenderKind;
  capabilities?: ToolRunCapabilities;
  durationMs?: number;
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

type ToolTone = {
  panel: string;
  icon: string;
  detailBorder: string;
};

const TOOL_TONES: Record<string, ToolTone> = {
  search: {
    panel: 'border-info/25 border-l-info/75 bg-info/5 hover:border-info/35 hover:bg-info/10',
    icon: 'border-info/25 bg-info/10 text-info',
    detailBorder: 'border-info/25',
  },
  evidence: {
    panel: 'border-emerald-500/25 border-l-emerald-500/75 bg-emerald-500/5 hover:border-emerald-500/35 hover:bg-emerald-500/10',
    icon: 'border-emerald-500/25 bg-emerald-500/10 text-emerald-400',
    detailBorder: 'border-emerald-500/25',
  },
  code: {
    panel: 'border-blue-500/25 border-l-blue-500/75 bg-blue-500/5 hover:border-blue-500/35 hover:bg-blue-500/10',
    icon: 'border-blue-500/25 bg-blue-500/10 text-blue-400',
    detailBorder: 'border-blue-500/25',
  },
  files: {
    panel: 'border-slate-500/25 border-l-slate-500/75 bg-slate-500/5 hover:border-slate-500/35 hover:bg-slate-500/10',
    icon: 'border-slate-500/25 bg-slate-500/10 text-slate-400',
    detailBorder: 'border-slate-500/25',
  },
  edit: {
    panel: 'border-teal-500/25 border-l-teal-500/75 bg-teal-500/5 hover:border-teal-500/35 hover:bg-teal-500/10',
    icon: 'border-teal-500/25 bg-teal-500/10 text-teal-400',
    detailBorder: 'border-teal-500/25',
  },
  graph: {
    panel: 'border-violet-500/25 border-l-violet-500/75 bg-violet-500/5 hover:border-violet-500/35 hover:bg-violet-500/10',
    icon: 'border-violet-500/25 bg-violet-500/10 text-violet-400',
    detailBorder: 'border-violet-500/25',
  },
  web: {
    panel: 'border-orange-500/25 border-l-orange-500/75 bg-orange-500/5 hover:border-orange-500/35 hover:bg-orange-500/10',
    icon: 'border-orange-500/25 bg-orange-500/10 text-orange-400',
    detailBorder: 'border-orange-500/25',
  },
  shell: {
    panel: 'border-emerald-500/25 border-l-emerald-500/75 bg-emerald-500/5 hover:border-emerald-500/35 hover:bg-emerald-500/10',
    icon: 'border-emerald-500/25 bg-emerald-500/10 text-emerald-400',
    detailBorder: 'border-emerald-500/25',
  },
  image: {
    panel: 'border-fuchsia-500/25 border-l-fuchsia-500/75 bg-fuchsia-500/5 hover:border-fuchsia-500/35 hover:bg-fuchsia-500/10',
    icon: 'border-fuchsia-500/25 bg-fuchsia-500/10 text-fuchsia-400',
    detailBorder: 'border-fuchsia-500/25',
  },
  skill: {
    panel: 'border-purple-500/25 border-l-purple-500/75 bg-purple-500/5 hover:border-purple-500/35 hover:bg-purple-500/10',
    icon: 'border-purple-500/25 bg-purple-500/10 text-purple-400',
    detailBorder: 'border-purple-500/25',
  },
  plan: {
    panel: 'border-amber-500/25 border-l-amber-500/75 bg-amber-500/5 hover:border-amber-500/35 hover:bg-amber-500/10',
    icon: 'border-amber-500/25 bg-amber-500/10 text-amber-400',
    detailBorder: 'border-amber-500/25',
  },
  verification: {
    panel: 'border-success/25 border-l-success/75 bg-success/5 hover:border-success/35 hover:bg-success/10',
    icon: 'border-success/25 bg-success/10 text-success',
    detailBorder: 'border-success/25',
  },
  subagent: {
    panel: 'border-cyan-500/25 border-l-cyan-500/75 bg-cyan-500/5 hover:border-cyan-500/35 hover:bg-cyan-500/10',
    icon: 'border-cyan-500/25 bg-cyan-500/10 text-cyan-400',
    detailBorder: 'border-cyan-500/25',
  },
  default: {
    panel: 'border-border/45 border-l-border-hover bg-surface-0/35 hover:border-border/70 hover:bg-surface-0/55',
    icon: 'border-border/45 bg-surface-1/65 text-text-tertiary',
    detailBorder: 'border-border/35',
  },
};

const TOOL_TONE_KEYS = Object.keys(TOOL_TONES).filter((key) => key !== 'default');

function getToolTone(name?: string): ToolTone {
  const lower = (name || '').toLowerCase();
  if (lower.includes('knowledge_graph') || lower.includes('related_concepts') || lower.includes('chunk_context')) {
    return TOOL_TONES.graph;
  }
  if (lower.includes('retrieve') || lower.includes('evidence') || lower.includes('document')) {
    return TOOL_TONES.evidence;
  }
  if (lower.includes('code')) return TOOL_TONES.code;
  if (lower.includes('grep') || lower.includes('glob') || lower.includes('read_file') || lower.includes('list_dir')) {
    return TOOL_TONES.files;
  }
  if (lower.includes('edit') || lower.includes('patch') || lower.includes('write') || lower.includes('create_file')) {
    return TOOL_TONES.edit;
  }
  if (lower.includes('web') || lower.includes('url') || lower.includes('download') || lower.includes('desktop')) {
    return TOOL_TONES.web;
  }
  if (lower.includes('shell') || lower.includes('terminal') || lower.includes('command')) return TOOL_TONES.shell;
  if (lower.includes('image')) return TOOL_TONES.image;
  if (lower.includes('skill')) return TOOL_TONES.skill;
  if (lower.includes('plan')) return TOOL_TONES.plan;
  if (lower.includes('verification') || lower.includes('security') || lower.includes('approval')) {
    return TOOL_TONES.verification;
  }
  if (lower.includes('subagent')) return TOOL_TONES.subagent;
  const key = TOOL_TONE_KEYS.find((candidate) => lower.includes(candidate));
  return key ? (TOOL_TONES[key] ?? TOOL_TONES.default) : TOOL_TONES.default;
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

function getToolBriefLabel(
  name: string,
  args?: string,
  displayNameOverride?: string | null,
  targetOverride?: string | null,
): string {
  const label = displayNameOverride ?? getToolDisplayName(name);
  const target = targetOverride ?? getToolBriefTarget(args);
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
  toolName,
  arguments: args,
  status,
  plugin,
  renderKind,
  capabilities,
  durationMs,
  content,
  isError,
  artifacts,
  compact,
  inline,
  trace,
  argsStatus,
  argsBytes,
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
  const fileChangeDisplayName = getFileChangeDisplayName(
    safeToolName,
    isFileChangeRender,
    diffStats?.operation ?? fileDiff?.operation,
  );
  const fileChangeTarget = isFileChangeRender
    ? getStableFileChangeTarget(fileDiff, diffStats)
    : null;
  const formattedArgs = formatArgs(args);
  const briefLabel = skillActivationName
    ? (
        skillActivation
          ? t('chat.skillActivatedLabel', { name: skillActivationName })
          : t('chat.skillActivatingLabel', { name: skillActivationName })
      )
    : getToolBriefLabel(safeToolName, args, fileChangeDisplayName, fileChangeTarget);
  const briefResult = getToolBriefResult(status, t, content, safeToolName);
  const isPending = isPendingToolCallStatus(status);
  const argsByteLabel = formatByteCount(
    typeof argsBytes === 'number' ? argsBytes : (args ? args.length : 0),
  );
  const durationLabel = formatDurationMs(durationMs);
  const resourceKeyCount = Array.isArray(capabilities?.resourceKeys)
    ? capabilities.resourceKeys.length
    : 0;
  const capabilitySummary = capabilities
    ? [
        plugin?.name ?? null,
        capabilities.readOnly ? t('chat.capabilityReadOnly') : t('chat.capabilityWrites'),
        capabilities.concurrencySafe ? t('chat.capabilityParallel') : t('chat.capabilitySerial'),
        capabilities.interruptBehavior === 'cancel' ? t('chat.capabilityCancellable') : t('chat.capabilityBlocking'),
        resourceKeyCount > 0 ? t('chat.capabilityResources', { count: String(resourceKeyCount) }) : null,
      ].filter(Boolean).join(' · ')
    : null;
  const rawStreamingArgsPreview =
    isPending && (argsStatus === 'streaming' || status === 'starting' || status === 'approvalPending') && args
      ? args.length > 500 ? args.slice(0, 500) + '\u2026' : args
      : null;
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
  const graphUsage = useMemo(() => extractGraphAgentUsage(artifacts), [artifacts]);
  const imageArgs = useMemo(() => parseImagePromptArgs(args), [args]);
  const isImageRender = renderKind === 'image' || safeToolName.toLowerCase() === 'generate_image';
  const showImagePendingPreview = isImageRender && isPending && !generatedImage;
  const isSearchDone =
    safeToolName.toLowerCase().includes('search') && status === 'done' && !!content;
  const searchItems = useMemo(
    () => (isSearchDone ? parseSearchResults(content!) : null),
    [isSearchDone, content],
  );

  const [expanded, setExpanded] = useState(false);

  useEffect(() => {
    if (!isPending && graphUsage) {
      saveGraphAgentUsage({
        ...graphUsage,
        createdAt: new Date().toISOString(),
      });
    }
  }, [graphUsage, isPending]);

  // Auto-collapse file mutation details when execution finishes; users can manually re-open.
  useEffect(() => {
    if (!isPending) {
      setExpanded(false);
    }
  }, [isPending]);

  useEffect(() => {
    if (isPending && isFileChangeRender && fileDiff) {
      setExpanded(true);
    }
  }, [fileDiff, isFileChangeRender, isPending]);

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

  const statusConfig = {
    preparing: { icon: Loader2, text: t('chat.toolRunning'), color: 'text-accent', spin: true },
    starting: { icon: Loader2, text: t('chat.toolRunning'), color: 'text-accent', spin: true },
    approvalPending: { icon: Loader2, text: t('chat.toolRunning'), color: 'text-accent', spin: true },
    running: { icon: Loader2, text: t('chat.toolRunning'), color: 'text-accent', spin: true },
    done: { icon: CheckCircle2, text: t('chat.toolDone'), color: 'text-success', spin: false },
    error: { icon: XCircle, text: t('chat.toolError'), color: 'text-danger', spin: false },
    declined: { icon: XCircle, text: t('chat.toolError'), color: 'text-danger', spin: false },
    cancelled: { icon: XCircle, text: t('chat.toolError'), color: 'text-danger', spin: false },
    timedOut: { icon: XCircle, text: t('chat.toolError'), color: 'text-danger', spin: false },
  }[status];
  const baseHeaderSummary = skillActivation
    ? t('chat.skillActivationReady')
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
          ? t('chat.generatedImageLoading')
        : graphUsage
          ? t('chat.graphContextSummary', {
              nodes: String(graphUsage.usedGraphNodes.length),
              edges: String(graphUsage.usedGraphEdges.length),
              documents: String(graphUsage.usedDocuments.length),
            })
        : diffStats
          ? isPending
            ? statusConfig.text
            : `${diffStats.operation === 'create' ? t('chat.fileDiffCreated') : t('chat.fileDiffModified')}`
        : status === 'done' && content
          ? briefResult
          : statusConfig.text;
  const headerSummary =
    isPending && argsByteLabel
      ? `${baseHeaderSummary} · ${argsByteLabel}`
      : !isPending && durationLabel
        ? `${baseHeaderSummary} · ${durationLabel}`
      : baseHeaderSummary;

  const StatusIcon = statusConfig.icon;
  const traceActive = isPending && !shouldReduceMotion;
  const traceSoft = status !== 'error';
  const visibleFormattedArgs = fileDiff || diffStats ? null : formattedArgs;
  const streamingArgsPreview = fileDiff || diffStats ? null : rawStreamingArgsPreview;
  const liveFileDiff = trace && isPending && Boolean(fileDiff);
  const detailsExpanded = expanded;
  const expandableDetails = Boolean(
    visibleFormattedArgs ||
    content ||
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
    showImagePendingPreview ||
    streamingArgsPreview,
  );
  const failedStatus = isUnsuccessfulToolCallStatus(status);
  const toolTone = getToolTone(safeToolName);
  const traceToneClass = failedStatus
    ? 'border-danger/25 border-l-danger/75 bg-danger/10 hover:border-danger/35 hover:bg-danger/15'
    : toolTone.panel;
  const traceIconToneClass = failedStatus
    ? 'border-danger/25 bg-danger/10 text-danger'
    : toolTone.icon;
  const statusBadgeClass = failedStatus
    ? 'border-danger/25 bg-danger/10 text-danger'
    : isPending
      ? 'border-accent/25 bg-accent/10 text-accent'
      : 'border-success/20 bg-success/10 text-success';
  const traceDetailBorderClass = failedStatus
    ? 'border-danger/25'
    : toolTone.detailBorder;
  const tracePreviewText = headerSummary;

  if (trace) {
    return (
      <div className="my-1 max-w-full">
        <button
          type="button"
          onClick={() => expandableDetails && setExpanded((prev) => !prev)}
          aria-expanded={expandableDetails ? detailsExpanded : undefined}
          className={`group inline-grid min-h-9 max-w-full grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-x-2 rounded-lg border border-l-2 px-2 py-1.5 text-left align-top shadow-[0_1px_0_rgba(255,255,255,0.035)] transition-colors disabled:cursor-default sm:max-w-[36rem] ${expandableDetails ? 'cursor-pointer' : 'cursor-default'} ${traceToneClass}`}
          disabled={!expandableDetails}
          title={capabilitySummary ?? undefined}
        >
          <span className={`flex h-6 w-6 shrink-0 items-center justify-center rounded-md border ${traceIconToneClass}`}>
            <Icon className="h-3.5 w-3.5 shrink-0" />
          </span>
          <span className="min-w-0 py-0.5">
            <span className="block truncate text-[11px] font-medium leading-4 text-text-primary">
              {briefLabel}
            </span>
            {tracePreviewText && (
              <span className="block truncate text-[10px] leading-3 text-text-tertiary">
                {tracePreviewText}
              </span>
            )}
          </span>
          <span className="flex shrink-0 items-center gap-1 pl-1">
            {diffStats ? (
              <span className="inline-flex">
                <DiffStatsTicker
                  additions={diffStats.additions}
                  deletions={diffStats.deletions}
                  filesChanged={diffStats.filesChanged}
                  replacements={diffStats.replacements}
                  compact
                  live={isPending}
                />
              </span>
            ) : null}
            <span className={`inline-flex h-5 w-5 items-center justify-center rounded-md border ${statusBadgeClass}`}>
              <StatusIcon className={`h-3 w-3 shrink-0 ${statusConfig.spin ? 'animate-spin' : ''}`} />
            </span>
            {expandableDetails && (
              detailsExpanded
                ? <ChevronUp className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
                : <ChevronDown className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
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
                {streamingArgsPreview ? (
                  <pre className="whitespace-pre-wrap break-words rounded-md border border-border/35 bg-surface-0/45 px-2 py-1 text-[11px] leading-relaxed text-text-tertiary">
                    {streamingArgsPreview}
                  </pre>
                ) : visibleFormattedArgs ? (
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
                    </div>
                    <div className="text-sm text-text-primary">{subagentJudgement.summary}</div>
                    {subagentJudgement.rationale && (
                      <div className="mt-2 text-xs text-text-secondary">{subagentJudgement.rationale}</div>
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
                    {content && (
                      <pre className={`whitespace-pre-wrap break-words text-[11px] leading-relaxed ${isError ? 'text-danger' : 'text-text-secondary'}`}>
                        {content}
                      </pre>
                    )}
                  </>
                ) : content ? (
                  <pre className={`whitespace-pre-wrap break-words text-[11px] leading-relaxed ${isError ? 'text-danger' : 'text-text-secondary'}`}>
                    {content}
                  </pre>
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
      <div className="my-1 max-w-full">
        <button
          type="button"
          onClick={() => expandableDetails && setExpanded((p) => !p)}
          aria-expanded={expandableDetails ? expanded : undefined}
          disabled={!expandableDetails}
          className={`inline-grid min-h-8 max-w-full grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-x-1.5 rounded-lg border border-l-2 px-1.5 py-1 text-left align-top shadow-[0_1px_0_rgba(255,255,255,0.035)] transition-colors disabled:cursor-default sm:max-w-[32rem] ${expandableDetails ? 'cursor-pointer' : 'cursor-default'} ${traceToneClass}`}
        >
          <span className={`flex h-5 w-5 shrink-0 items-center justify-center rounded-md border ${traceIconToneClass}`}>
            <Icon className="h-3 w-3 shrink-0" />
          </span>
          <span className="min-w-0">
            <span className="block truncate text-[11px] font-medium leading-4 text-text-primary">
              {briefLabel}
            </span>
            {headerSummary && (
              <span className="hidden truncate text-[10px] leading-3 text-text-tertiary sm:block">
                {headerSummary}
              </span>
            )}
          </span>
          <span className="flex shrink-0 items-center gap-1 pl-1">
            {diffStats ? (
              <span className="inline-flex">
                <DiffStatsTicker
                  additions={diffStats.additions}
                  deletions={diffStats.deletions}
                  filesChanged={diffStats.filesChanged}
                  replacements={diffStats.replacements}
                  compact
                  live={isPending}
                />
              </span>
            ) : null}
            <span className={`inline-flex h-5 w-5 items-center justify-center rounded-md border ${statusBadgeClass}`}>
              <StatusIcon
                className={`h-2.5 w-2.5 shrink-0 ${statusConfig.spin ? 'animate-spin' : ''}`}
              />
            </span>
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
                {streamingArgsPreview ? (
                  <pre className="whitespace-pre-wrap break-words rounded-md border border-border/35 bg-surface-0/45 px-1.5 py-0.5 text-[10px] text-text-tertiary">
                    {streamingArgsPreview}
                  </pre>
                ) : visibleFormattedArgs ? (
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
                    {content && (
                      <pre className={`text-[11px] whitespace-pre-wrap break-words max-h-32 overflow-y-auto ${isError ? 'text-danger' : 'text-text-tertiary'}`}>
                        {content}
                      </pre>
                    )}
                  </div>
                ) : content ? (
                  <pre className={`text-[11px] whitespace-pre-wrap break-words max-h-32 overflow-y-auto ${isError ? 'text-danger' : 'text-text-tertiary'}`}>
                    {content}
                  </pre>
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
        className="chat-image-panel my-2 overflow-hidden rounded-md border border-border/55 bg-surface-0/55"
        data-state={generatedImage ? 'ready' : 'loading'}
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
          <span className={`inline-flex shrink-0 items-center gap-1 text-[11px] ${statusConfig.color}`}>
            <StatusIcon className={`h-3.5 w-3.5 ${statusConfig.spin ? 'animate-spin' : ''}`} />
            <span>{headerSummary}</span>
          </span>
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

  if (subagentRun) {
    return (
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.2, ease: [0.16, 1, 0.3, 1] }}
        className="my-2"
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
        className="my-2 rounded-lg border border-border/60 bg-surface-0/55 p-3"
      >
        <div className="mb-3 flex flex-wrap items-center gap-1.5 text-xs text-text-secondary">
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
              className="rounded-full border border-border/55 bg-surface-1/70 px-2 py-0.5"
              title={subagentBatch.workflowTemplateDescription ?? undefined}
            >
              {subagentBatch.workflowTemplateLabel}
            </span>
          )}
          {typeof subagentBatch.completedRuns === 'number' && (
            <span className="rounded-full border border-border/55 bg-surface-1/70 px-2 py-0.5">
              {t('chat.subagentCompletedCount', { count: String(subagentBatch.completedRuns) })}
            </span>
          )}
          {typeof subagentBatch.failedRuns === 'number' && subagentBatch.failedRuns > 0 && (
            <span className="rounded-full border border-danger/25 bg-danger/10 px-2 py-0.5 text-danger">
              {t('chat.subagentFailedCount', { count: String(subagentBatch.failedRuns) })}
            </span>
          )}
        </div>
        <div className="space-y-2">
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
        className="my-2 rounded-lg border border-border/60 bg-surface-0/55 p-3"
      >
        <div className="mb-2 flex flex-wrap items-center gap-1.5 text-xs text-text-secondary">
          <span className="font-medium text-text-primary">
            {subagentJudgement.task || t('chat.subagentJudgementFallback')}
          </span>
          <span className="rounded-full border border-border/55 bg-surface-1/70 px-2 py-0.5">
            {subagentJudgement.decisionMode}
          </span>
          {subagentJudgement.confidence && (
            <span className="rounded-full border border-border/55 bg-surface-1/70 px-2 py-0.5">
              {t('chat.subagentConfidence', { value: subagentJudgement.confidence })}
            </span>
          )}
          {subagentJudgement.winnerIds.length > 0 && (
            <span className="rounded-full border border-accent/25 bg-accent/10 px-2 py-0.5 text-accent">
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
      className="chat-trace-panel my-1 overflow-hidden rounded-lg border border-border/60 bg-surface-0/70 shadow-[0_1px_0_rgba(255,255,255,0.04)]"
      data-trace-soft={traceSoft ? 'true' : 'false'}
      data-trace-active={traceActive ? 'true' : 'false'}
    >
      {/* Header */}
      <button
        onClick={() => expandableDetails && setExpanded((p) => !p)}
        aria-expanded={expandableDetails ? expanded : undefined}
        aria-label={expandableDetails ? (expanded ? t('common.collapse') : t('common.expand')) : briefLabel}
        disabled={!expandableDetails}
        className="grid w-full grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2 px-3 py-2 text-left hover:bg-surface-1/85
          transition-colors duration-fast ease-out cursor-pointer disabled:cursor-default disabled:hover:bg-transparent"
      >
        <span className={`flex h-7 w-7 shrink-0 items-center justify-center rounded-md border ${traceIconToneClass}`}>
          <Icon className="h-4 w-4 shrink-0" />
        </span>
        <span className="min-w-0">
          <span className="block truncate text-xs font-medium leading-5 text-text-primary">{briefLabel}</span>
          <span className="block truncate text-[11px] leading-4 text-text-tertiary">
            {headerSummary}
          </span>
        </span>
        <span className="flex shrink-0 items-center gap-1.5 pl-1">
          {diffStats ? (
            <span className="inline-flex">
              <DiffStatsTicker
                additions={diffStats.additions}
                deletions={diffStats.deletions}
                filesChanged={diffStats.filesChanged}
                replacements={diffStats.replacements}
                live={isPending}
              />
            </span>
          ) : null}
          <span className={`inline-flex h-6 w-6 items-center justify-center rounded-md border ${statusBadgeClass}`}>
            <StatusIcon
              className={`h-3.5 w-3.5 shrink-0 ${statusConfig.spin ? 'animate-spin' : ''}`}
            />
          </span>
          {expandableDetails ? (
            expanded ? (
              <ChevronUp className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
            ) : (
              <ChevronDown className="h-3.5 w-3.5 shrink-0 text-text-tertiary" />
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
              {streamingArgsPreview && (
                <pre
                  className="mb-2 whitespace-pre-wrap break-words rounded-md bg-surface-0/60 px-2 py-1 text-[11px] text-text-tertiary max-h-48 overflow-y-auto"
                >
                  {streamingArgsPreview}
                </pre>
              )}
              {visibleFormattedArgs && !streamingArgsPreview && (
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
                  {content && (
                    <pre
                      className={`text-xs whitespace-pre-wrap break-words max-h-48 overflow-y-auto
                        ${isError ? 'text-danger' : 'text-text-secondary'}`}
                    >
                      {content}
                    </pre>
                  )}
                </div>
              ) : graphUsage ? (
                <KnowledgeGraphUsagePanel usage={graphUsage} />
              ) : searchItems ? (
                <>
                  {trustBoundary && <TrustBoundaryPills boundary={trustBoundary} />}
                  <SearchResultCards items={searchItems} />
                </>
              ) : content ? (
                <pre
                  className={`text-xs whitespace-pre-wrap break-words max-h-48 overflow-y-auto
                    ${isError ? 'text-danger' : 'text-text-secondary'}`}
                >
                  {content}
                </pre>
              ) : null}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </motion.div>
  );
}
