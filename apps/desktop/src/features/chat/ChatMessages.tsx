import {
  Fragment,
  type ReactNode,
  type KeyboardEvent as ReactKeyboardEvent,
  useRef,
  useEffect,
  useMemo,
  useState,
  useCallback,
} from "react";
import { motion, AnimatePresence, useReducedMotion } from "framer-motion";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  MessageCircle,
  ChevronDown,
  AlertCircle,
  RotateCcw,
  X,
  BookOpen,
  CheckCircle2,
  Play,
} from "lucide-react";
import { useTranslation } from "../../i18n";
import { useDeveloperMode } from "../../lib/developerMode";
import { useConversationFileChanges } from '../../lib/useConversationFileChanges';
import { TurnFileChanges } from '../../components/chat/TurnFileChanges';
import { hasTimeGap } from "../../lib/relativeTime";
import {
  buildCitationMap,
} from "../../lib/citationParser";
import type { CitationCardData } from "../../lib/citationParser";
import { SOFT_FADE_TRANSITION } from "../../lib/uiMotion";
import { isCompactionSummaryMessage, isGoalMessage, isSteeringMessage } from "../../lib/chatMessageGuards";
import { getActiveGoalContext } from "../../lib/goalContext";
import type {
  StreamRoundEvent,
  ToolCallEvent,
  TraceEvent,
  TurnTiming,
} from "../../lib/streaming/protocol";
import {
  extractPersistedTraceItems,
  extractTurnTrace,
  isPersistedReasoningOnlyAssistant,
} from "../../lib/streaming/persistedTrace";
import {
  buildRoundTimelineSections,
  hasRenderableTimelineSections,
  normalizeThinking,
  persistedTraceItemsToTimelineSections,
  persistedTraceItemToTimelineSections,
  skillRefsFromTraceItems,
  toolCallToTimelineSection,
  projectLiveConversationTimeline,
  formatTurnDuration,
  turnLifecycleTimelineSections,
  type TimelineSkillRef,
  type TimelineSection,
} from "../../lib/streaming/timelineViewModel";
import {
  projectChatMessageVisibility,
  projectChatStreamingVisibility,
} from "../../lib/streaming/chatVisibility";
import {
  QuestionRequestTimelineRecord,
  ToolCallCard,
} from "../../components/chat/ToolCallCard";
import { extractQuestionRequest } from "../../lib/questionCards";
import {
  FileDiffSummaryPanel,
  extractFileDiffArtifacts,
  mergeFileDiffArtifactsByPath,
  type FileDiffArtifact,
} from "../../components/chat/FileDiffPreview";
import { ThinkingBlock } from "../../components/chat/ThinkingBlock";
import type { ThinkingSection } from "../../components/chat/ThinkingBlock";
import { MessageBubble } from "../../components/chat/MessageBubble";
import { StreamingMarkdown } from "../../components/chat/StreamingMarkdown";
import { Skeleton } from "../../components/ui/Skeleton";
import type {
  AgentTaskRun,
  ArtifactPayload,
  ConversationMessage,
  ConversationTurn,
  VisionTurnOverride,
} from "../../types/conversation";

interface ChatMessagesProps {
  conversationId?: string | null;
  messages: ConversationMessage[];
  turns: ConversationTurn[];
  streamText: string;
  streamRounds: StreamRoundEvent[];
  traceEvents: TraceEvent[];
  thinkingText: string;
  isThinking: boolean;
  toolCalls: ToolCallEvent[];
  taskRun?: AgentTaskRun | null;
  turnTiming?: TurnTiming | null;
  isStreaming: boolean;
  error?: string | null;
  onRetry?: (messageId?: string, visionTurnOverride?: VisionTurnOverride, refreshVision?: boolean) => void;
  onDismissError?: () => void;
  onDeleteMessage?: (messageId: string) => void;
  onEditAndResend?: (messageId: string, newContent: string) => void;
  onApprovePlan?: (planMarkdown: string, sourceMessageId: string) => void;
  onQuestionSubmit?: (message: string, artifact: ArtifactPayload) => void;
  onResumePaused?: () => void;
  loadingMsgs?: boolean;
  lastCached?: boolean;
  isCompacting?: boolean;
  compactCompleteVisible?: boolean;
  compactionPhaseLabel?: string;
  compactionElapsedSeconds?: number;
  compactionTerminalText?: string;
  onCancelCompaction?: () => void;
}

interface TurnNavigationItem {
  id: string;
  userMessageId: string;
  preview: string;
}

interface RenderedTurnNavigationItem {
  item: TurnNavigationItem;
  index: number;
}

const MAX_TURN_NAVIGATION_MARKERS = 160;
const ACTIVE_TURN_NAVIGATION_RADIUS = 8;

function sampleTurnNavigationItems(
  items: TurnNavigationItem[],
  activeIndex: number,
): RenderedTurnNavigationItem[] {
  if (items.length <= MAX_TURN_NAVIGATION_MARKERS) {
    return items.map((item, index) => ({ item, index }));
  }

  const indexes = new Set<number>([0, items.length - 1, activeIndex]);
  for (
    let index = Math.max(0, activeIndex - ACTIVE_TURN_NAVIGATION_RADIUS);
    index <= Math.min(items.length - 1, activeIndex + ACTIVE_TURN_NAVIGATION_RADIUS);
    index += 1
  ) {
    indexes.add(index);
  }

  const remaining = MAX_TURN_NAVIGATION_MARKERS - indexes.size;
  for (let slot = 0; slot < remaining; slot += 1) {
    const ratio = remaining <= 1 ? 0 : slot / (remaining - 1);
    indexes.add(Math.round(ratio * (items.length - 1)));
  }

  return [...indexes]
    .sort((left, right) => left - right)
    .slice(0, MAX_TURN_NAVIGATION_MARKERS)
    .map((index) => ({ item: items[index], index }));
}

function turnNavigationPreview(content: string): string {
  const compact = content.replace(/\s+/g, ' ').trim();
  if (!compact) return '…';
  return compact.length > 72 ? `${compact.slice(0, 69).trimEnd()}…` : compact;
}

function TurnNavigator({
  items,
  activeId,
  onSelect,
  messageAreaLabel,
}: {
  items: TurnNavigationItem[];
  activeId: string | null;
  onSelect: (id: string) => void;
  messageAreaLabel: string;
}) {
  const shouldReduceMotion = useReducedMotion();
  const navigatorRef = useRef<HTMLElement>(null);
  const pendingKeyboardFocusIndex = useRef<number | null>(null);
  const activeIndex = Math.max(0, items.findIndex((item) => item.id === activeId));
  const progress = items.length <= 1 ? 0 : activeIndex / (items.length - 1);
  const renderedItems = useMemo(
    () => sampleTurnNavigationItems(items, activeIndex),
    [activeIndex, items],
  );
  useEffect(() => {
    const pendingIndex = pendingKeyboardFocusIndex.current;
    if (pendingIndex == null) return;
    const target = navigatorRef.current?.querySelector<HTMLButtonElement>(
      `[data-turn-navigation-index="${pendingIndex}"]`,
    );
    if (!target) return;
    target.focus({ preventScroll: true });
    pendingKeyboardFocusIndex.current = null;
  }, [renderedItems]);
  if (items.length < 2) return null;

  const handleKeyDown = (event: ReactKeyboardEvent<HTMLElement>) => {
    const target = (event.target as HTMLElement).closest<HTMLButtonElement>(
      '[data-turn-navigation-index]',
    );
    if (!target) return;

    const currentIndex = Number(target.dataset.turnNavigationIndex);
    let nextIndex: number | null = null;
    if (event.key === 'ArrowUp' || event.key === 'ArrowLeft') {
      nextIndex = Math.max(0, currentIndex - 1);
    } else if (event.key === 'ArrowDown' || event.key === 'ArrowRight') {
      nextIndex = Math.min(items.length - 1, currentIndex + 1);
    } else if (event.key === 'Home') {
      nextIndex = 0;
    } else if (event.key === 'End') {
      nextIndex = items.length - 1;
    }
    if (nextIndex == null || nextIndex === currentIndex) return;

    event.preventDefault();
    const nextItem = items[nextIndex];
    pendingKeyboardFocusIndex.current = nextIndex;
    onSelect(nextItem.id);
  };

  return (
    <nav
      ref={navigatorRef}
      aria-label={`${messageAreaLabel} · ${items.length}`}
      aria-orientation="vertical"
      data-active-index={activeIndex}
      data-variant="thread-minimap"
      data-testid="chat-turn-navigator"
      className="pointer-events-none sticky top-1/2 z-20 ml-auto -mr-11 hidden h-px w-7 -translate-y-1/2 lg:block"
      onKeyDown={handleKeyDown}
    >
      <div className="group/rail pointer-events-auto absolute right-0 top-0 max-h-[min(68vh,34rem)] w-7 -translate-y-1/2 overflow-y-auto overflow-x-visible py-3 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
        <div className="relative flex w-full flex-col items-end py-1">
          <span
            className="absolute bottom-[9px] right-[4px] top-[9px] w-px overflow-hidden bg-border/35 transition-colors duration-200 group-hover/rail:bg-border/55"
            aria-hidden="true"
          >
            <motion.span
              data-testid="chat-turn-minimap-progress"
              className="block h-full w-full origin-top bg-linear-to-b from-accent/35 via-accent/70 to-accent"
              animate={{ scaleY: progress }}
              transition={shouldReduceMotion ? INSTANT_TRANSITION : { duration: 0.22, ease: [0.22, 1, 0.36, 1] }}
            />
          </span>
          {renderedItems.map(({ item, index }) => {
            const active = item.id === activeId;
            const distance = Math.abs(index - activeIndex);
            const proximity = Math.max(0, 1 - distance / 5);
            const markerOpacity = active ? 1 : 0.26 + proximity * 0.34;
            const label = `#${index + 1} · ${item.preview}`;
            return (
              <button
                key={item.id}
                type="button"
                aria-label={label}
                aria-current={active ? 'step' : undefined}
                aria-posinset={index + 1}
                aria-setsize={items.length}
                title={label}
                data-turn-navigation-id={item.id}
                data-turn-navigation-index={index}
                className="group relative z-10 flex h-[17px] w-7 shrink-0 items-center justify-end pr-0.5 outline-none focus-visible:ring-2 focus-visible:ring-accent/40 focus-visible:ring-offset-2 focus-visible:ring-offset-surface-1"
                onClick={() => onSelect(item.id)}
              >
                {active && (
                  <motion.span
                    layoutId="chat-turn-minimap-active-marker"
                    className="absolute right-0 h-[15px] w-[9px] rounded-l-full border border-r-0 border-accent/20 bg-accent/8 shadow-[-4px_0_14px_rgba(99,102,241,0.12)]"
                    transition={shouldReduceMotion ? INSTANT_TRANSITION : { type: 'spring', stiffness: 260, damping: 28, mass: 0.85 }}
                    aria-hidden="true"
                  />
                )}
                <motion.span
                  data-testid="chat-turn-minimap-marker"
                  data-active={active ? 'true' : 'false'}
                  className={`relative z-10 rounded-full ring-1 ring-surface-1 transition-colors duration-150 group-hover:bg-accent group-focus-visible:bg-accent ${
                    active
                      ? 'h-2.5 w-1 bg-accent shadow-[0_0_9px_rgba(110,120,255,0.68)]'
                      : 'h-1 w-1 bg-text-tertiary'
                  }`}
                  animate={{
                    opacity: markerOpacity,
                    scale: active ? 1 : 0.82 + proximity * 0.12,
                  }}
                  transition={shouldReduceMotion ? INSTANT_TRANSITION : { duration: 0.18, ease: [0.22, 1, 0.36, 1] }}
                  aria-hidden="true"
                />
                <span
                  className="pointer-events-none invisible absolute right-full top-1/2 mr-2.5 w-52 -translate-y-1/2 translate-x-0.5 rounded-lg border border-border/55 bg-surface-1/96 px-2.5 py-2 text-left text-[10px] leading-4 text-text-secondary opacity-0 shadow-[0_10px_28px_rgba(0,0,0,0.18)] backdrop-blur-md transition-[opacity,transform,visibility] duration-150 ease-out group-hover:visible group-hover:translate-x-0 group-hover:opacity-100 group-focus-visible:visible group-focus-visible:translate-x-0 group-focus-visible:opacity-100 motion-reduce:transition-none"
                  data-testid="chat-turn-preview"
                >
                  <span className="mb-0.5 block font-semibold tabular-nums text-accent/90">Turn {index + 1}</span>
                  <span className="line-clamp-2">{item.preview}</span>
                </span>
              </button>
            );
          })}
        </div>
      </div>
    </nav>
  );
}

const INSTANT_TRANSITION = { duration: 0 };
const NEAR_BOTTOM_THRESHOLD = 96;
const FOLLOW_RELEASE_THRESHOLD = 160;
const WAITING_MOODS = [
  "(｡•́‿•̀｡)",
  "(づ｡◕‿‿◕｡)づ",
  "(๑>◡<๑)",
  "(ﾉ◕ヮ◕)ﾉ*:･ﾟ✧",
  "(ᵔ◡ᵔ)",
  "(｡•̀ᴗ-)✧",
  "(っ˘ω˘ς )",
  "(๑˃ᴗ˂)ﻭ",
  "(ง •̀_•́)ง",
  "(´｡• ᵕ •｡`)",
  "(✿◠‿◠)",
  "(๑•̀ㅂ•́)و✧",
];

function randomMood(moods: readonly string[]): string {
  return moods[Math.floor(Math.random() * moods.length)] ?? moods[0] ?? "";
}

function WaitingMoodBadge() {
  const mood = useMemo(() => randomMood(WAITING_MOODS), []);
  return (
    <span
      aria-hidden="true"
      className="thinking-mood-badge hidden rounded-md px-1.5 py-0.5 font-mono text-[11px] sm:inline-block"
    >
      {mood}
    </span>
  );
}

interface TurnSkillDisplayRef {
  key: string;
  label: string;
  description?: string;
  shortDescription?: string;
  builtin?: boolean;
  sourcePath?: string | null;
  implicit?: boolean;
  activated?: boolean;
}

const TURN_SKILL_VISIBLE_LIMIT = 6;

function asObjectRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function titleCaseSkillSlug(value: string): string {
  const normalized = value
    .replace(/^builtin[-_:]/i, "")
    .replace(/[_-]+/g, " ")
    .trim();
  if (!normalized) return value.trim();
  return normalized.replace(/\b[a-z]/g, (char) => char.toUpperCase());
}

function skillLabelKey(
  id: unknown,
  name: unknown,
  displayName: unknown,
): { label: string; key: string } | null {
  const display = typeof displayName === "string" ? displayName.trim() : "";
  const skillName = typeof name === "string" ? name.trim() : "";
  const skillId = typeof id === "string" ? id.trim() : "";
  const rawLabel = display || skillName || skillId;
  if (!rawLabel) return null;

  const label = display ? display : titleCaseSkillSlug(rawLabel);
  const key = (skillId || skillName || label).toLowerCase();
  return { label, key };
}

function turnSkillFromTimeline(skill: TimelineSkillRef): TurnSkillDisplayRef {
  return {
    key: skill.key,
    label: skill.label,
    description: skill.description,
    shortDescription: skill.shortDescription,
    builtin: skill.builtin,
    sourcePath: skill.sourcePath ?? null,
    implicit: skill.implicit,
    activated: skill.activated,
  };
}

function selectedSkillRefFromRecord(
  skill: Record<string, unknown>,
): TurnSkillDisplayRef | null {
  if (skill.enabled === false) return null;
  const ref = skillLabelKey(skill.id, skill.name, skill.displayName);
  if (!ref) return null;
  return {
    ...ref,
    description:
      typeof skill.description === "string" ? skill.description : undefined,
    shortDescription:
      typeof skill.shortDescription === "string"
        ? skill.shortDescription
        : undefined,
    builtin: typeof skill.builtin === "boolean" ? skill.builtin : undefined,
    sourcePath:
      typeof skill.sourcePath === "string" ? skill.sourcePath : undefined,
    implicit: typeof skill.implicit === "boolean" ? skill.implicit : undefined,
    activated: false,
  };
}

function findSelectedSkillsArtifact(
  artifacts: AgentTaskRun["artifacts"],
): Record<string, unknown> | null {
  if (Array.isArray(artifacts)) {
    for (const item of artifacts) {
      const selected = findSelectedSkillsArtifact(item as AgentTaskRun["artifacts"]);
      if (selected) return selected;
    }
    return null;
  }

  const record = asObjectRecord(artifacts);
  if (!record) return null;
  if (record.kind === "selectedSkills") return record;

  const nested = asObjectRecord(record.selectedSkills);
  return nested?.kind === "selectedSkills" ? nested : null;
}

function extractTaskRunSelectedSkills(
  artifacts: AgentTaskRun["artifacts"],
): TurnSkillDisplayRef[] {
  const selected = findSelectedSkillsArtifact(artifacts);
  if (!selected || !Array.isArray(selected.skills)) return [];

  const skills: TurnSkillDisplayRef[] = [];
  const seen = new Set<string>();
  for (const rawSkill of selected.skills) {
    const skill = asObjectRecord(rawSkill);
    const ref = skill ? selectedSkillRefFromRecord(skill) : null;
    if (!ref || seen.has(ref.key)) continue;
    seen.add(ref.key);
    skills.push(ref);
  }
  return skills;
}

function isActiveTaskRunStatus(status: string | null | undefined): boolean {
  return (
    status === "queued" ||
    status === "running" ||
    status === "waiting_approval" ||
    status === "cancelling"
  );
}

function TurnSkillStrip({
  skills,
  live,
}: {
  skills: TurnSkillDisplayRef[];
  live: boolean;
}) {
  const { t } = useTranslation();
  if (skills.length === 0) return null;

  const visibleSkills = skills.slice(0, TURN_SKILL_VISIBLE_LIMIT);
  const hiddenCount = Math.max(0, skills.length - visibleSkills.length);

  return (
    <div className="mb-2 flex justify-start" data-testid="turn-skill-strip">
      <div className="w-full min-w-0 rounded-lg border border-border/70 bg-surface-1/75 px-3 py-2">
        <div className="flex flex-wrap items-center gap-2">
          <span className="inline-flex min-w-0 items-center gap-1.5 text-xs font-medium text-text-secondary">
            <BookOpen className="h-3.5 w-3.5 shrink-0 text-accent" />
            <span className="truncate">
              {t(live ? "chat.turnSkillsLiveTitle" : "chat.turnSkillsTitle")}
            </span>
          </span>
          <span className="rounded-md bg-accent/10 px-1.5 py-0.5 text-[11px] font-medium text-accent">
            {t("chat.turnSkillsCount", { count: String(skills.length) })}
          </span>
          <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1.5">
            {visibleSkills.map((skill) => {
              const detail =
                skill.shortDescription ||
                skill.description ||
                skill.sourcePath ||
                skill.label;
              return (
                <span
                  key={skill.key}
                  className="inline-flex min-w-0 max-w-full items-center gap-1.5 rounded-md border border-border/70 bg-surface-0/70 px-2 py-1 text-xs text-text-primary"
                  title={detail}
                >
                  <CheckCircle2
                    className={`h-3.5 w-3.5 shrink-0 ${
                      skill.activated ? "text-success" : "text-accent"
                    }`}
                  />
                  <span className="min-w-0 max-w-[12rem] truncate">
                    {skill.label}
                  </span>
                  <span className="hidden shrink-0 text-[10px] text-text-tertiary sm:inline">
                    {skill.activated
                      ? t("chat.turnSkillsActivated")
                      : t("chat.turnSkillsSelected")}
                  </span>
                </span>
              );
            })}
            {hiddenCount > 0 && (
              <span className="rounded-md border border-border/70 bg-surface-0/50 px-2 py-1 text-xs text-text-secondary">
                {t("chat.turnSkillsMore", { count: String(hiddenCount) })}
              </span>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

type MessageTraceGroup =
  | {
      type: "anchor";
      nodes: ReactNode[];
      skills?: TurnSkillDisplayRef[];
      hideMessageBubble?: boolean;
      memberIndexes?: number[];
    }
  | { type: "member" };

interface GeneratedImagePreviewItem {
  id: string;
  toolName: string;
  arguments: string;
  owner?: ToolCallEvent["owner"];
  content?: string;
  isError?: boolean;
  artifacts: ArtifactPayload;
}

function collectArtifactValues(value: unknown, out: unknown[]) {
  if (!value) return;
  if (Array.isArray(value)) {
    for (const item of value) collectArtifactValues(item, out);
    return;
  }
  if (typeof value !== "object") {
    out.push(value);
    return;
  }
  const obj = value as Record<string, unknown>;
  out.push(obj);
  for (const key of ["evidenceCards", "cards", "results", "items"]) {
    if (Array.isArray(obj[key])) {
      collectArtifactValues(obj[key], out);
    }
  }
}

function isGeneratedImageArtifact(value: unknown): value is ArtifactPayload {
  return Boolean(
    value &&
    typeof value === "object" &&
    !Array.isArray(value) &&
    (value as Record<string, unknown>).kind === "generatedImage",
  );
}

function TraceStatusRow({
  text,
  tone = "muted",
}: {
  text: string;
  tone?: "muted" | "success" | "error";
}) {
  const toneClass =
    tone === "error"
      ? "border-danger/25 bg-danger/8 text-danger"
      : tone === "success"
        ? "border-success/25 bg-success/8 text-success"
        : "border-border/45 bg-surface-0/25 text-text-tertiary";
  const dotClass =
    tone === "error"
      ? "bg-danger"
      : tone === "success"
        ? "bg-success"
        : "bg-text-tertiary/60";

  return (
    <div
      className={`inline-flex max-w-full items-center gap-1.5 rounded-full border px-2.5 py-1 text-[11px] leading-tight ${toneClass}`}
      title={text}
    >
      <span className={`h-1.5 w-1.5 shrink-0 rounded-full ${dotClass}`} />
      <span className="min-w-0 truncate">{text}</span>
    </div>
  );
}

function TraceSteeringRow({
  text,
  label,
}: {
  text: string;
  label: string;
}) {
  return (
    <div
      className="inline-flex max-w-full items-start gap-2 rounded-lg border border-pink-400/25 bg-pink-400/8 px-2.5 py-2 text-xs leading-relaxed text-text-secondary"
      title={text}
    >
      <MessageCircle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-pink-300" />
      <span className="min-w-0">
        <span className="mr-1 font-medium text-pink-200">{label}</span>
        <span className="wrap-break-word">{text}</span>
      </span>
    </div>
  );
}

function collectQuestionResponses(
  value: unknown,
  output: Map<string, Record<string, unknown>>,
  depth = 0,
) {
  if (depth > 6 || value == null) return;
  if (Array.isArray(value)) {
    value.forEach((item) => collectQuestionResponses(item, output, depth + 1));
    return;
  }
  if (typeof value !== 'object') return;
  const record = value as Record<string, unknown>;
  if (record.kind === 'questionResponse' && typeof record.requestCallId === 'string') {
    output.set(record.requestCallId, record);
  }
  Object.values(record).forEach((item) => collectQuestionResponses(item, output, depth + 1));
}

export function ChatMessages(props: ChatMessagesProps) {
  const {
    turns,
    streamText,
    thinkingText,
    isThinking,
    toolCalls,
    taskRun,
    isStreaming,
    error,
    onRetry,
    onDismissError,
    onDeleteMessage,
    onEditAndResend,
    onApprovePlan,
    onQuestionSubmit,
    onResumePaused,
    loadingMsgs,
    lastCached,
    isCompacting = false,
    compactCompleteVisible = false,
    compactionPhaseLabel,
    compactionElapsedSeconds,
    compactionTerminalText,
    onCancelCompaction,
  } = props;
  const completedFileTools = toolCalls.filter(call => call.status === 'done' || call.status === 'error').map(call => `${call.callId}:${call.status}`).join('|');
  const recordedFileChanges = useConversationFileChanges(props.conversationId, isStreaming, `${completedFileTools}:${props.messages.length}:${turns.length}`);
  const dockedFileChanges = (taskRun?.turnId ? recordedFileChanges.get(taskRun.turnId) : undefined)
    ?? [...turns].reverse().map(turn => recordedFileChanges.get(turn.id)).find(Boolean)
    ?? [...recordedFileChanges.values()].slice(-1)[0];
  const [developerMode] = useDeveloperMode();
  const streamingVisibility = useMemo(
    () => projectChatStreamingVisibility({
      isStreaming,
      streamRounds: props.streamRounds,
      traceEvents: props.traceEvents,
    }),
    [isStreaming, props.streamRounds, props.traceEvents],
  );
  const messageVisibility = useMemo(
    () => projectChatMessageVisibility({
      isStreaming,
      messages: props.messages,
    }),
    [isStreaming, props.messages],
  );
  const messages = messageVisibility.historyMessages;
  const streamRounds = streamingVisibility.streamRounds;
  const traceEvents = streamingVisibility.traceEvents;
  const activeGoalContext = useMemo(
    () => getActiveGoalContext(messages),
    [messages],
  );
  const questionResponses = useMemo(() => {
    const responses = new Map<string, Record<string, unknown>>();
    // Question responses are intentionally removed from the visible transcript,
    // but their artifacts still drive the compact answered record.
    props.messages.forEach((message) => collectQuestionResponses(message.artifacts, responses));
    return responses;
  }, [props.messages]);
  const { t } = useTranslation();
  const shouldReduceMotion = useReducedMotion();
  const scrollContainerRef = useRef<HTMLDivElement>(null);
  const autoScrollFrameRef = useRef<number | null>(null);
  const shouldAutoFollowRef = useRef(true);
  const [isNearBottom, setIsNearBottom] = useState(true);
  const [hasOverflow, setHasOverflow] = useState(false);
  const [unreadCount, setUnreadCount] = useState(0);
  const prevMsgCountRef = useRef(messages.length);
  const chunkIdCacheRef = useRef<Map<string, string[]>>(new Map());
  const pendingChunkIdsRef = useRef<string[]>([]);
  const turnAnchorRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  const turnNavigationFrameRef = useRef<number | null>(null);
  const turnByUserMessageId = useMemo(
    () => new Map(turns.map((turn) => [turn.userMessageId, turn])),
    [turns],
  );
  const turnNavigationItems = useMemo<TurnNavigationItem[]>(
    () => messages
      .filter((message) => message.role === 'user' && !isSteeringMessage(message))
      .map((message) => ({
        id: turnByUserMessageId.get(message.id)?.id ?? `message-${message.id}`,
        userMessageId: message.id,
        preview: turnNavigationPreview(message.content),
      })),
    [messages, turnByUserMessageId],
  );
  const turnNavigationByMessageId = useMemo(
    () => new Map(turnNavigationItems.map((item) => [item.userMessageId, item])),
    [turnNavigationItems],
  );
  const [activeTurnNavigationId, setActiveTurnNavigationId] = useState<string | null>(
    turnNavigationItems[0]?.id ?? null,
  );

  useEffect(() => {
    setActiveTurnNavigationId((current) =>
      current && turnNavigationItems.some((item) => item.id === current)
        ? current
        : (turnNavigationItems[0]?.id ?? null),
    );
  }, [turnNavigationItems]);

  useEffect(() => {
    const ids: string[] = [];
    for (const tc of toolCalls) {
      if (tc.status !== "done" || !tc.artifacts) continue;
      const items: unknown[] = [];
      collectArtifactValues(tc.artifacts, items);
      for (const item of items) {
        if (
          item &&
          typeof item === "object" &&
          "chunkId" in (item as Record<string, unknown>)
        ) {
          ids.push((item as Record<string, unknown>).chunkId as string);
        }
      }
    }
    if (ids.length > 0) {
      pendingChunkIdsRef.current = ids;
    }
  }, [toolCalls]);

  const prevMessagesLenRef = useRef(messages.length);
  useEffect(() => {
    if (
      messages.length > prevMessagesLenRef.current &&
      pendingChunkIdsRef.current.length > 0
    ) {
      for (let i = messages.length - 1; i >= 0; i -= 1) {
        if (messages[i].role === "assistant") {
          chunkIdCacheRef.current.set(messages[i].id, [
            ...pendingChunkIdsRef.current,
          ]);
          pendingChunkIdsRef.current = [];
          break;
        }
      }
    }
    prevMessagesLenRef.current = messages.length;
  }, [messages]);

  const displayedText = streamText;
  const displayedThinkingText = thinkingText;

  const streamingCitationLookup = useMemo(() => {
    const map = buildCitationMap(toolCalls);
    return { getCard: (id: string) => map.get(id) };
  }, [toolCalls]);

  const messageToolCalls = useMemo(() => {
    const map = new Map<number, ConversationMessage[]>();
    for (let i = 0; i < messages.length; i += 1) {
      const msg = messages[i];
      if (msg.role !== "assistant" || msg.toolCalls.length === 0) continue;
      const toolResults: ConversationMessage[] = [];
      for (let j = i + 1; j < messages.length; j += 1) {
        if (messages[j].role !== "tool") break;
        toolResults.push(messages[j]);
      }
      map.set(i, toolResults);
    }
    return map;
  }, [messages]);

  const messageCitationLookups = useMemo(() => {
    const map = new Map<
      number,
      { getCard: (id: string) => CitationCardData | undefined }
    >();
    let turnToolResults: ConversationMessage[] = [];
    for (let i = 0; i < messages.length; i += 1) {
      const msg = messages[i];
      if (msg.role === 'user') {
        turnToolResults = [];
        continue;
      }
      if (msg.role === 'assistant') {
        const directToolResults = messageToolCalls.get(i) ?? [];
        if (directToolResults.length > 0) {
          turnToolResults = [...turnToolResults, ...directToolResults];
        }
        if (turnToolResults.length > 0) {
          const citationMap = buildCitationMap(
            turnToolResults.map((result) => ({ artifacts: result.artifacts })),
          );
          map.set(i, { getCard: (id: string) => citationMap.get(id) });
        }
      }
    }
    return map;
  }, [messageToolCalls, messages]);

  const allToolCitationLookup = useMemo(() => {
    const citationMap = buildCitationMap(
      messages
        .filter((message) => message.role === "tool")
        .map((message) => ({ artifacts: message.artifacts })),
    );
    return { getCard: (id: string) => citationMap.get(id) };
  }, [messages]);

  const renderTraceReplyNode = useCallback(
    (
      key: string,
      content: string,
      isStreaming = false,
      citationLookup?: { getCard: (id: string) => CitationCardData | undefined },
    ) => {
      const effectiveCitationLookup = citationLookup ?? allToolCitationLookup;
      return (
        <div key={key} className="flex justify-start mb-4">
          <div className="chat-assistant-reply w-full min-w-0 text-sm leading-relaxed">
            <StreamingMarkdown
              content={content}
              isStreaming={isStreaming}
              citationLookup={effectiveCitationLookup}
              reduceMotion={Boolean(shouldReduceMotion)}
            />
          </div>
        </div>
      );
    },
    [
      allToolCitationLookup,
      shouldReduceMotion,
    ],
  );

  const renderThinkingTraceNode = useCallback(
    (
      key: string,
      sections: ThinkingSection[],
      isStreaming = false,
      forceExpanded = false,
    ) => (
      <div key={key} className="flex justify-start mb-1">
        <div className="w-full min-w-0">
          <ThinkingBlock
            content=""
            sections={sections}
            isStreaming={isStreaming}
            defaultExpanded={isStreaming || forceExpanded}
            collapseOnFinish={!forceExpanded}
          />
        </div>
      </div>
    ),
    [],
  );

  const renderTimelineSection = useCallback(
    (section: TimelineSection): ThinkingSection | null => {
      switch (section.kind) {
        case "thinking":
          return section.text.trim().length > 0 ? { text: section.text } : null;
        case "status":
          return {
            text: "",
            node: (
              <TraceStatusRow
                key={section.id}
                text={section.text}
                tone={section.tone}
              />
            ),
          };
        case "steering":
          return {
            text: "",
            node: (
              <TraceSteeringRow
                key={section.id}
                text={section.text}
                label={t("chat.steeringLabel")}
              />
            ),
          };
        case "reply":
          return {
            text: "",
            node: renderTraceReplyNode(section.id, section.text),
          };
        case "tool":
          return {
            text: "",
            node: (
              <ToolCallCard
                key={section.id}
                callId={section.toolCall.callId}
                toolName={section.toolCall.toolName}
                arguments={section.toolCall.arguments}
                status={section.toolCall.status}
                owner={section.toolCall.owner}
                renderKind={section.toolCall.renderKind}
                capabilities={section.toolCall.capabilities}
                durationMs={section.toolCall.durationMs}
                progressNote={section.toolCall.progressNote}
                activityEvents={section.toolCall.activityEvents}
                content={section.toolCall.content}
                isError={section.toolCall.isError}
                artifacts={section.toolCall.artifacts}
                argsStatus={section.toolCall.argsStatus}
                argsBytes={section.toolCall.argsBytes}
                trace={section.trace}
                questionAnswered={questionResponses.has(section.toolCall.callId)}
                questionResponse={questionResponses.get(section.toolCall.callId)}
                onQuestionSubmit={onQuestionSubmit}
              />
            ),
          };
        default:
          return null;
      }
    },
    [onQuestionSubmit, questionResponses, renderTraceReplyNode, t],
  );

  const renderTimelineSections = useCallback(
    (sections: TimelineSection[]): ThinkingSection[] =>
      sections
        .map(renderTimelineSection)
        .filter((section): section is ThinkingSection => Boolean(section)),
    [renderTimelineSection],
  );

  const renderTimelineTraceNode = useCallback(
    (
      key: string,
      sections: TimelineSection[],
      isStreaming = false,
    ) => {
      if (sections.length === 0) return <Fragment key={key} />;
      const ordered: Array<
        | { kind: 'trace'; id: string; sections: TimelineSection[] }
        | {
            kind: 'answeredQuestion';
            id: string;
            request: NonNullable<ReturnType<typeof extractQuestionRequest>>;
            response: Record<string, unknown>;
          }
        | {
            kind: 'pendingQuestion';
            id: string;
            section: Extract<TimelineSection, { kind: 'tool' }>;
          }
      > = [];
      const seenQuestions = new Set<string>();
      let traceSegment: TimelineSection[] = [];
      const flushTrace = () => {
        if (traceSegment.length === 0) return;
        ordered.push({
          kind: 'trace',
          id: `${key}-trace-${ordered.length}`,
          sections: traceSegment,
        });
        traceSegment = [];
      };

      for (const section of sections) {
        if (section.kind !== 'tool' || section.toolCall.toolName !== 'request_user_input') {
          traceSegment.push(section);
          continue;
        }
        const response = questionResponses.get(section.toolCall.callId);
        const request = extractQuestionRequest(
          section.toolCall.callId,
          section.toolCall.arguments,
          section.toolCall.artifacts,
        );
        if (!request) {
          traceSegment.push(section);
          continue;
        }
        if (seenQuestions.has(request.callId)) continue;
        seenQuestions.add(request.callId);
        flushTrace();
        if (response && request.interactionId) {
          ordered.push({
            kind: 'answeredQuestion',
            id: `${key}-answered-${request.interactionId}`,
            request,
            response,
          });
        } else {
          ordered.push({
            kind: 'pendingQuestion',
            id: `${key}-pending-${request.callId}`,
            section,
          });
        }
      }
      flushTrace();
      let lastTraceIndex = -1;
      ordered.forEach((item, index) => {
        if (item.kind === 'trace') lastTraceIndex = index;
      });

      return (
        <Fragment key={key}>
          {ordered.map((item, index) => item.kind === 'trace'
            ? renderThinkingTraceNode(
                item.id,
                renderTimelineSections(item.sections),
                isStreaming && index === lastTraceIndex,
                false,
              )
            : item.kind === 'answeredQuestion' ? (
                <div key={item.id} className="mb-1 flex justify-start">
                  <div className="w-full min-w-0">
                    <QuestionRequestTimelineRecord
                      request={item.request}
                      answered
                      response={item.response}
                    />
                  </div>
                </div>
              ) : (
                <div key={item.id} className="mb-1 flex justify-start">
                  <div className="w-full min-w-0">
                    {renderTimelineSections([item.section])[0]?.node}
                  </div>
                </div>
              ))}
        </Fragment>
      );
    },
    [questionResponses, renderThinkingTraceNode, renderTimelineSections],
  );

  const messageThinkingText = useMemo(() => {
    const map = new Map<number, string>();
    let lastUserIdx = -1;

    for (let i = 0; i < messages.length; i += 1) {
      const msg = messages[i];
      if (msg.role === "user") {
        lastUserIdx = i;
        continue;
      }
      if (msg.role !== "assistant" || !msg.thinking) continue;

      let renderableThinking = normalizeThinking(msg.thinking);
      if (msg.toolCalls.length === 0) {
        const priorToolRoundThinking: string[] = [];
        for (let j = lastUserIdx + 1; j < i; j += 1) {
          const prev = messages[j];
          if (
            prev.role !== "assistant" ||
            !prev.thinking ||
            prev.toolCalls.length === 0
          )
            continue;
          const segment = normalizeThinking(prev.thinking);
          if (segment) {
            priorToolRoundThinking.push(segment);
          }
        }

        const knownPrefix = priorToolRoundThinking.join("\n").trim();
        if (knownPrefix && renderableThinking.startsWith(knownPrefix)) {
          renderableThinking = renderableThinking
            .slice(knownPrefix.length)
            .replace(/^\s+/, "");
        }
      }

      if (renderableThinking) {
        map.set(i, renderableThinking);
      }
    }

    return map;
  }, [messages]);

  const messageIndexById = useMemo(() => {
    const map = new Map<string, number>();
    messages.forEach((message, index) => {
      map.set(message.id, index);
    });
    return map;
  }, [messages]);

  const reasoningOnlyAssistantIndexes = useMemo(() => {
    const indexes = new Set<number>();
    for (const turn of turns) {
      if (!turn.assistantMessageId) continue;
      const assistantIdx = messageIndexById.get(turn.assistantMessageId);
      if (assistantIdx == null) continue;
      const message = messages[assistantIdx];
      const traceItems = extractTurnTrace(turn.trace)?.items
        ?? extractPersistedTraceItems(message.artifacts);
      if (isPersistedReasoningOnlyAssistant(message, traceItems)) {
        indexes.add(assistantIdx);
      }
    }
    messages.forEach((message, index) => {
      if (indexes.has(index)) return;
      if (
        isPersistedReasoningOnlyAssistant(
          message,
          extractPersistedTraceItems(message.artifacts),
        )
      ) {
        indexes.add(index);
      }
    });
    return indexes;
  }, [messageIndexById, messages, turns]);

  const liveTaskRunSkills = useMemo(() => {
    if (!developerMode) return null;
    if (!taskRun || !isActiveTaskRunStatus(taskRun.status)) return null;
    const userIdx = messageIndexById.get(taskRun.userMessageId);
    if (userIdx == null) return null;
    const skills = extractTaskRunSelectedSkills(taskRun.artifacts);
    if (skills.length === 0) return null;
    return {
      turnId: taskRun.turnId,
      userIdx,
      skills,
    };
  }, [developerMode, messageIndexById, taskRun]);

  const turnRenderMap = useMemo(() => {
    const anchors = new Map<
      number,
      { turn: ConversationTurn; assistantIdx: number | null }
    >();
    const members = new Set<number>();

    for (const turn of turns) {
      const userIdx = messageIndexById.get(turn.userMessageId);
      if (userIdx == null) continue;
      const assistantIdx = turn.assistantMessageId
        ? (messageIndexById.get(turn.assistantMessageId) ?? null)
        : null;

      anchors.set(userIdx, { turn, assistantIdx });
      if (assistantIdx != null) {
        members.add(assistantIdx);
      }
    }

    return { anchors, members };
  }, [messageIndexById, turns]);

  const messageTraceGroups = useMemo(() => {
    const map = new Map<number, MessageTraceGroup>();
    const finalAssistantIndexes = new Set<number>();
    const statusSectionsByAssistant = new Map<number, TimelineSection[]>();
    const fallbackSectionsByAssistant = new Map<number, TimelineSection[]>();
    const skillsByAssistant = new Map<number, TurnSkillDisplayRef[]>();

    for (const turn of turns) {
      if (!turn.assistantMessageId) continue;
      const assistantIdx = messageIndexById.get(turn.assistantMessageId);
      if (assistantIdx == null) continue;

      finalAssistantIndexes.add(assistantIdx);

      const trace = extractTurnTrace(turn.trace);
      skillsByAssistant.set(
        assistantIdx,
        skillRefsFromTraceItems(trace?.items ?? null).map(turnSkillFromTimeline),
      );
      const sections = turnLifecycleTimelineSections({
        turn,
        routeKind: trace?.routeKind,
        traceItems: trace?.items ?? null,
        formatSkillsSummary: (names) =>
          names.length > 0
            ? t('chat.skillsActivated', { names: names.join(', ') })
            : t('chat.skillsActivatedNone'),
        includeDeveloper: developerMode,
      });
      const fallbackSections: TimelineSection[] = [];

      for (const [itemIdx, item] of (trace?.items ?? []).entries()) {
        const itemSections = persistedTraceItemToTimelineSections({
          item,
          id: `turn-${turn.id}-${item.kind}-${itemIdx}`,
          trace: Boolean(trace),
          includeDeveloper: developerMode,
        });
        if (item.kind === "status") {
          sections.push(...itemSections);
          continue;
        }
        fallbackSections.push(...itemSections);
      }

      statusSectionsByAssistant.set(assistantIdx, sections);
      fallbackSectionsByAssistant.set(assistantIdx, fallbackSections);
    }

    let currentGroup: number[] = [];

    const flushGroup = () => {
      if (currentGroup.length === 0) return;

      const persistedTraceCarrierIdx = [...currentGroup]
        .reverse()
        .find((idx) => Boolean(extractPersistedTraceItems(messages[idx].artifacts)));
      const finalAssistantIdx = [...currentGroup]
        .reverse()
        .find((idx) => finalAssistantIndexes.has(idx));
      const finalContentAssistantIdx = [...currentGroup]
        .reverse()
        .find((idx) => {
          const message = messages[idx];
          return message.content.trim().length > 0
            && message.toolCalls.length === 0
            && !reasoningOnlyAssistantIndexes.has(idx);
        });
      const anchorIdx = finalAssistantIdx
        ?? finalContentAssistantIdx
        ?? persistedTraceCarrierIdx
        ?? currentGroup[0];
      const persistedTraceCarrier =
        persistedTraceCarrierIdx == null ? null : messages[persistedTraceCarrierIdx];
      const persistedTraceItems =
        persistedTraceCarrier == null
          ? null
          : extractPersistedTraceItems(persistedTraceCarrier.artifacts);

      const persistedTraceSections: TimelineSection[] =
        persistedTraceItems == null
          ? []
          : persistedTraceItemsToTimelineSections({
              items: persistedTraceItems,
              idPrefix: `persisted-${persistedTraceCarrier?.id ?? "trace"}`,
              trace: true,
              includeDeveloper: developerMode,
            });
      const persistedSkills = skillRefsFromTraceItems(persistedTraceItems).map(
        turnSkillFromTimeline,
      );
      const turnTraceSkills = skillsByAssistant.get(anchorIdx);
      const traceSkills =
        turnTraceSkills && turnTraceSkills.length > 0
          ? turnTraceSkills
          : persistedSkills;

      const statusSections =
        statusSectionsByAssistant.get(anchorIdx) ?? persistedTraceSections;
      const nodes: ReactNode[] = [];
      const hiddenMembers = new Set<number>();
      const traceSections: TimelineSection[] = [...statusSections];
      let renderedTraceActivity = false;

      for (const idx of currentGroup) {
        const msg = messages[idx];
        const thinking = messageThinkingText.get(idx) ?? "";
        const inlineToolSections = msg.toolCalls.flatMap((tc, toolIdx) => {
          const toolResult = messageToolCalls
            .get(idx)
            ?.find((tr) => tr.toolCallId === tc.id);
          const status: ToolCallEvent["status"] = toolResult ? "done" : "running";
          const argumentsText = tc.arguments || "";
          return toolCallToTimelineSection({
            id: `persisted-trace-${msg.id}-${tc.id || tc.name || toolIdx}`,
            trace: true,
            toolCall: {
              callId: tc.id || `${msg.id}-${toolIdx}`,
              toolName: tc.name || "unknown_tool",
              arguments: argumentsText,
              status,
              owner: tc.owner,
              argsStatus: status === "done" ? "done" : "ready",
              argsBytes: argumentsText.length,
              content: toolResult?.content,
              artifacts: toolResult?.artifacts ?? undefined,
            },
          });
        });

        if (thinking) {
          renderedTraceActivity = true;
          traceSections.push({
            kind: "thinking",
            id: `message-thinking-${msg.id}`,
            text: thinking,
          });
        }

        const shouldRenderInlineReply =
          msg.content.trim().length > 0 &&
          !reasoningOnlyAssistantIndexes.has(idx) &&
          !(idx === anchorIdx && msg.toolCalls.length === 0);
        if (shouldRenderInlineReply) {
          traceSections.push({
            kind: "reply",
            id: `trace-reply-${msg.id}`,
            text: msg.content,
          });
        }

        if (inlineToolSections.length > 0) {
          renderedTraceActivity = true;
          traceSections.push(...inlineToolSections);
        }

        if (idx !== anchorIdx) {
          hiddenMembers.add(idx);
        }
      }

      const fallbackSectionsForAnchor =
        fallbackSectionsByAssistant.get(anchorIdx) ?? [];
      if (!renderedTraceActivity && fallbackSectionsForAnchor.length > 0) {
        traceSections.push(...fallbackSectionsForAnchor);
      }

      if (hasRenderableTimelineSections(traceSections)) {
        nodes.push(
          renderTimelineTraceNode(
            `turn-working-trace-${messages[anchorIdx].id}`,
            traceSections,
          ),
        );
      }

      if (nodes.length === 0) {
        const fallbackSections = [
          ...statusSections,
          ...fallbackSectionsForAnchor,
        ];
        if (hasRenderableTimelineSections(fallbackSections)) {
          nodes.push(
            renderTimelineTraceNode(
              `trace-fallback-${messages[anchorIdx].id}`,
              fallbackSections,
            ),
          );
        }
      }

      if (reasoningOnlyAssistantIndexes.has(anchorIdx)) {
        nodes.push(
          <div
            key={`missing-final-answer-${messages[anchorIdx].id}`}
            className="mb-3 flex justify-start"
            role="status"
          >
            <div className="flex max-w-[80%] items-start gap-2 rounded-lg border border-amber-500/20 bg-amber-500/10 px-3.5 py-2.5 text-xs text-amber-200/90">
              <AlertCircle className="mt-0.5 h-4 w-4 shrink-0 text-amber-300" />
              <div className="min-w-0 flex-1">
                <p>{t("chat.finalAnswerMissing")}</p>
                {onRetry && (
                  <button
                    type="button"
                    onClick={() => onRetry()}
                    className="mt-2 rounded-md border border-amber-400/30 px-2 py-1 text-amber-100 transition-colors hover:bg-amber-400/10"
                  >
                    {t("chat.retryFinalAnswer")}
                  </button>
                )}
              </div>
            </div>
          </div>,
        );
      }

      if (nodes.length > 0 || traceSkills.length > 0) {
        map.set(anchorIdx, {
          type: "anchor",
          nodes,
          skills: traceSkills,
          hideMessageBubble:
            messages[anchorIdx].toolCalls.length > 0
            || reasoningOnlyAssistantIndexes.has(anchorIdx),
          memberIndexes: [...currentGroup],
        });
      }

      for (const idx of hiddenMembers) {
        map.set(idx, { type: "member" });
      }

      currentGroup = [];
    };

    for (let i = 0; i < messages.length; i += 1) {
      const msg = messages[i];
      if (msg.role === "user") {
        flushGroup();
        continue;
      }
      if (msg.role === "assistant") {
        currentGroup.push(i);
      }
    }
    flushGroup();

    return map;
  }, [
    developerMode,
    messageCitationLookups,
    messageIndexById,
    messageThinkingText,
    messageToolCalls,
    messages,
    onRetry,
    reasoningOnlyAssistantIndexes,
    renderTimelineTraceNode,
    renderTraceReplyNode,
    t,
    turns,
  ]);

  const renderableMessageIndexes = useMemo(() => messages.flatMap((message, index) => {
    if (message.role === "system") {
      return isCompactionSummaryMessage(message) ? [index] : [];
    }
    if (message.role === "tool" || turnRenderMap.members.has(index)) return [];
    const traceGroup = message.role === "assistant"
      ? messageTraceGroups.get(index)
      : undefined;
    return traceGroup?.type === "member" ? [] : [index];
  }), [messageTraceGroups, messages, turnRenderMap.members]);

  const rowVirtualizer = useVirtualizer({
    count: renderableMessageIndexes.length,
    getScrollElement: () => scrollContainerRef.current,
    estimateSize: () => 280,
    overscan: 8,
    getItemKey: virtualIndex => {
      const messageIndex = renderableMessageIndexes[virtualIndex];
      const message = messages[messageIndex];
      const turn = turnRenderMap.anchors.get(messageIndex);
      return turn ? `turn-${turn.turn.id}` : message?.id ?? virtualIndex;
    },
  });

  const turnVirtualIndexById = useMemo(() => {
    const indexes = new Map<string, number>();
    renderableMessageIndexes.forEach((messageIndex, virtualIndex) => {
      const message = messages[messageIndex];
      if (message?.role !== 'user') return;
      const navigationItem = turnNavigationByMessageId.get(message.id);
      if (navigationItem) indexes.set(navigationItem.id, virtualIndex);
    });
    return indexes;
  }, [messages, renderableMessageIndexes, turnNavigationByMessageId]);

  const {
    visibleTraceEvents,
    currentTimelineSections,
    liveTraceTimeline,
    currentTraceActive,
    collapsedLiveTrace,
  } = useMemo(
    () => projectLiveConversationTimeline({
      traceEvents,
      streamRounds,
      isStreaming,
      isThinking,
      thinkingText: displayedThinkingText,
      toolCalls,
      streamText,
      displayedText,
      includeDeveloper: developerMode,
    }),
    [
      displayedText,
      displayedThinkingText,
      developerMode,
      isStreaming,
      isThinking,
      streamRounds,
      streamText,
      toolCalls,
      traceEvents,
    ],
  );


  const updateActiveTurnNavigation = useCallback(() => {
    const container = scrollContainerRef.current;
    if (!container || turnNavigationItems.length === 0) return;

    // Programmatic navigation to the first turn lands at the absolute top.
    // Keep that explicit selection stable even when a compact first turn is
    // shorter than the viewport probe used for ordinary scroll tracking.
    if (container.scrollTop <= 1) {
      const firstId = turnNavigationItems[0].id;
      setActiveTurnNavigationId((current) => current === firstId ? current : firstId);
      return;
    }

    const marker = container.scrollTop + Math.min(container.clientHeight * 0.34, 220);
    let nextActive = activeTurnNavigationId ?? turnNavigationItems[0].id;
    let nextTop = Number.NEGATIVE_INFINITY;
    const containerTop = container.getBoundingClientRect().top;
    for (const [id, anchor] of turnAnchorRefs.current) {
      const anchorTop = anchor.getBoundingClientRect().top - containerTop + container.scrollTop;
      if (anchorTop <= marker && anchorTop > nextTop) {
        nextTop = anchorTop;
        nextActive = id;
      }
    }
    setActiveTurnNavigationId((current) => current === nextActive ? current : nextActive);
  }, [activeTurnNavigationId, turnNavigationItems]);

  const scheduleTurnNavigationUpdate = useCallback(() => {
    if (turnNavigationFrameRef.current != null) return;
    turnNavigationFrameRef.current = requestAnimationFrame(() => {
      turnNavigationFrameRef.current = null;
      updateActiveTurnNavigation();
    });
  }, [updateActiveTurnNavigation]);

  const registerTurnAnchor = useCallback((id: string, element: HTMLDivElement | null) => {
    if (element) {
      turnAnchorRefs.current.set(id, element);
    } else {
      turnAnchorRefs.current.delete(id);
    }
  }, []);

  const scrollToTurn = useCallback((id: string) => {
    const container = scrollContainerRef.current;
    const anchor = turnAnchorRefs.current.get(id);
    if (!container) return;

    shouldAutoFollowRef.current = false;
    setActiveTurnNavigationId(id);
    if (autoScrollFrameRef.current != null) {
      cancelAnimationFrame(autoScrollFrameRef.current);
      autoScrollFrameRef.current = null;
    }
    const virtualIndex = turnVirtualIndexById.get(id);
    if (virtualIndex != null) {
      if (virtualIndex === 0) {
        container.scrollTo({ top: 0, behavior: 'auto' });
      } else {
        rowVirtualizer.scrollToIndex(virtualIndex, { align: 'start' });
      }
      return;
    }
    if (!anchor) return;
    const top = Math.max(
      0,
      anchor.getBoundingClientRect().top
        - container.getBoundingClientRect().top
        + container.scrollTop
        - Math.min(120, container.clientHeight * 0.16),
    );
    container.scrollTo({
      top,
      behavior: shouldReduceMotion ? 'auto' : 'smooth',
    });
  }, [rowVirtualizer, shouldReduceMotion, turnVirtualIndexById]);

  const getScrollMetrics = useCallback(() => {
    const el = scrollContainerRef.current;
    if (!el) {
      return { distanceFromBottom: 0, nearBottom: true, overflow: false };
    }
    const distanceFromBottom = Math.max(
      0,
      el.scrollHeight - el.scrollTop - el.clientHeight,
    );
    return {
      distanceFromBottom,
      nearBottom: distanceFromBottom <= NEAR_BOTTOM_THRESHOLD,
      overflow: el.scrollHeight > el.clientHeight + 8,
    };
  }, []);

  useEffect(() => {
    shouldAutoFollowRef.current = true;
    setIsNearBottom(true);
    setUnreadCount(0);
  }, [props.conversationId]);

  const scrollToContainerBottom = useCallback((behavior: ScrollBehavior) => {
    const el = scrollContainerRef.current;
    if (!el) return;

    if (autoScrollFrameRef.current != null) {
      cancelAnimationFrame(autoScrollFrameRef.current);
    }

    autoScrollFrameRef.current = requestAnimationFrame(() => {
      el.scrollTo({ top: el.scrollHeight, behavior });
      setHasOverflow(el.scrollHeight > el.clientHeight + 8);
      setIsNearBottom(true);
      setUnreadCount(0);
      autoScrollFrameRef.current = null;
    });
  }, []);

  useEffect(
    () => () => {
      if (autoScrollFrameRef.current != null) {
        cancelAnimationFrame(autoScrollFrameRef.current);
      }
      if (turnNavigationFrameRef.current != null) {
        cancelAnimationFrame(turnNavigationFrameRef.current);
      }
    },
    [],
  );

  const handleScroll = useCallback(() => {
    scheduleTurnNavigationUpdate();
    const { distanceFromBottom, nearBottom, overflow } = getScrollMetrics();
    setHasOverflow(overflow);
    setIsNearBottom(!overflow || nearBottom);

    if (!overflow) {
      setUnreadCount(0);
      return;
    }

    if (nearBottom) {
      shouldAutoFollowRef.current = true;
      setUnreadCount(0);
      return;
    }

    if (distanceFromBottom > FOLLOW_RELEASE_THRESHOLD) {
      shouldAutoFollowRef.current = false;
    }
  }, [getScrollMetrics, scheduleTurnNavigationUpdate]);

  useEffect(() => {
    const newCount = messages.length - prevMsgCountRef.current;
    if (newCount > 0 && hasOverflow && !shouldAutoFollowRef.current) {
      setUnreadCount((count) => count + newCount);
    }
    prevMsgCountRef.current = messages.length;
  }, [messages.length, hasOverflow]);

  useEffect(() => {
    if (!shouldAutoFollowRef.current) {
      const { nearBottom, overflow } = getScrollMetrics();
      setHasOverflow(overflow);
      setIsNearBottom(nearBottom);
      return;
    }

    scrollToContainerBottom("auto");
    scheduleTurnNavigationUpdate();
  }, [
    messages,
    displayedText,
    displayedThinkingText,
    streamRounds,
    traceEvents,
    toolCalls,
    taskRun,
    isCompacting,
    compactCompleteVisible,
    getScrollMetrics,
    scrollToContainerBottom,
    scheduleTurnNavigationUpdate,
  ]);

  const scrollToBottom = useCallback(() => {
    shouldAutoFollowRef.current = true;
    scrollToContainerBottom(shouldReduceMotion ? "auto" : "smooth");
  }, [scrollToContainerBottom, shouldReduceMotion]);

  const lastAssistantIdx = useMemo(() => {
    for (let i = messages.length - 1; i >= 0; i -= 1) {
      if (messages[i].role === "assistant") return i;
    }
    return -1;
  }, [messages]);

  const lastRenderableMessageRole = useMemo(() => {
    for (let i = messages.length - 1; i >= 0; i -= 1) {
      const msg = messages[i];
      if (msg.role === "tool" || msg.role === "system") continue;
      if (msg.role === "assistant" && msg.content.trim().length === 0) continue;
      return msg.role;
    }
    return null;
  }, [messages]);

  const latestUserIdx = useMemo(() => {
    for (let i = messages.length - 1; i >= 0; i -= 1) {
      if (messages[i].role === "user") return i;
    }
    return -1;
  }, [messages]);

  const fileDiffGroups = useMemo(() => {
    const ownerByToolCallId = new Map<string, number>();
    messages.forEach((msg, idx) => {
      if (msg.role !== "assistant") return;
      for (const toolCall of msg.toolCalls) {
        if (toolCall.id) ownerByToolCallId.set(toolCall.id, idx);
      }
    });

    const assignedToolMessageIndexes = new Set<number>();
    const byTurnId = new Map<string, FileDiffArtifact[]>();

    for (const turn of turns) {
      const userIdx = messageIndexById.get(turn.userMessageId);
      if (userIdx == null) continue;

      let nextUserIdx = messages.length;
      for (let idx = userIdx + 1; idx < messages.length; idx += 1) {
        if (messages[idx].role === "user") {
          nextUserIdx = idx;
          break;
        }
      }

      const diffs: FileDiffArtifact[] = [];
      for (let idx = userIdx + 1; idx < nextUserIdx; idx += 1) {
        const msg = messages[idx];
        if (msg.role !== "tool") continue;
        const messageDiffs = extractFileDiffArtifacts(msg.artifacts ?? undefined);
        if (messageDiffs.length === 0) continue;
        diffs.push(...messageDiffs);
        assignedToolMessageIndexes.add(idx);
      }

      if (diffs.length > 0) {
        byTurnId.set(turn.id, diffs);
      }
    }

    const byAssistantIdx = new Map<number, FileDiffArtifact[]>();
    for (let idx = 0; idx < messages.length; idx += 1) {
      if (assignedToolMessageIndexes.has(idx)) continue;
      const msg = messages[idx];
      if (msg.role !== "tool" || !msg.toolCallId) continue;
      const ownerIdx = ownerByToolCallId.get(msg.toolCallId);
      if (ownerIdx == null) continue;
      const messageDiffs = extractFileDiffArtifacts(msg.artifacts ?? undefined);
      if (messageDiffs.length === 0) continue;
      const current = byAssistantIdx.get(ownerIdx) ?? [];
      current.push(...messageDiffs);
      byAssistantIdx.set(ownerIdx, current);
    }

    return { byTurnId, byAssistantIdx };
  }, [messageIndexById, messages, turns]);

  const generatedImagePreviewsByAssistantIdx = useMemo(() => {
    const ownerByToolCallId = new Map<
      string,
      {
        assistantIdx: number;
        toolCall: ConversationMessage["toolCalls"][number];
      }
    >();

    messages.forEach((msg, idx) => {
      if (msg.role !== "assistant") return;
      for (const toolCall of msg.toolCalls) {
        if (toolCall.id) {
          ownerByToolCallId.set(toolCall.id, {
            assistantIdx: idx,
            toolCall,
          });
        }
      }
    });

    const byAssistantIdx = new Map<number, GeneratedImagePreviewItem[]>();
    messages.forEach((msg) => {
      if (
        msg.role !== "tool" ||
        !msg.toolCallId ||
        !isGeneratedImageArtifact(msg.artifacts)
      ) {
        return;
      }

      const owner = ownerByToolCallId.get(msg.toolCallId);
      if (!owner) return;

      const items = byAssistantIdx.get(owner.assistantIdx) ?? [];
      items.push({
        id: `generated-image-${msg.id}`,
        toolName: owner.toolCall.name || "generate_image",
        arguments: owner.toolCall.arguments || "",
        owner: owner.toolCall.owner,
        content: msg.content,
        artifacts: msg.artifacts,
      });
      byAssistantIdx.set(owner.assistantIdx, items);
    });

    return byAssistantIdx;
  }, [messages]);

  const generatedImagePreviewsForIndexes = useCallback(
    (indexes: number[]): GeneratedImagePreviewItem[] =>
      indexes.flatMap((idx) => generatedImagePreviewsByAssistantIdx.get(idx) ?? []),
    [generatedImagePreviewsByAssistantIdx],
  );

  const renderFileDiffPreviews = useCallback(
    (diffs: FileDiffArtifact[] | undefined, _keyPrefix: string) => {
      if (!diffs || diffs.length === 0) return null;
      const mergedDiffs = mergeFileDiffArtifactsByPath(diffs);
      return (
        <div className="my-2 flex justify-start" data-testid="turn-file-diff-previews">
          <div className="w-full min-w-0">
            <FileDiffSummaryPanel diffs={mergedDiffs} />
          </div>
        </div>
      );
    },
    [],
  );

  const renderGeneratedImagePreviews = useCallback(
    (items: GeneratedImagePreviewItem[] | undefined) => {
      if (!items || items.length === 0) return null;
      return (
        <div className="my-2 flex justify-start" data-testid="message-generated-image-previews">
          <div className="w-full max-w-[min(100%,40rem)] space-y-2">
            {items.map((item) => (
              <ToolCallCard
                key={item.id}
                toolName={item.toolName}
                arguments={item.arguments}
                status="done"
                owner={item.owner}
                renderKind="image"
                content={item.content}
                isError={item.isError}
                artifacts={item.artifacts}
                argsStatus="done"
                argsBytes={item.arguments.length}
              />
            ))}
          </div>
        </div>
      );
    },
    [],
  );

  const shouldRenderLiveTraceTimeline =
    liveTraceTimeline.length > 0 && (
      isStreaming || streamRounds.length === 0 || collapsedLiveTrace !== null
    );
  const shouldRenderStreamRounds =
    streamRounds.length > 0 && (isStreaming || !shouldRenderLiveTraceTimeline);
  const shouldShowStreamingText =
    !shouldRenderLiveTraceTimeline &&
    (isStreaming ||
      (streamText.trim().length > 0 &&
        (lastRenderableMessageRole == null ||
          lastRenderableMessageRole === "user")));
  const shouldRenderInlineError = Boolean(
    error && !isStreaming && traceEvents.length === 0,
  );
  const renderCompactStatus = useCallback((active: boolean, key: string) => {
    const statusText = active
      ? `${compactionPhaseLabel ?? t("chat.compacting")}${compactionElapsedSeconds != null ? ` · ${compactionElapsedSeconds}s` : ""}`
      : compactionTerminalText ?? t("chat.compactComplete");
    return (
    <motion.div
      key={key}
      initial={shouldReduceMotion ? false : { opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={shouldReduceMotion ? INSTANT_TRANSITION : SOFT_FADE_TRANSITION}
      className="my-6 flex w-full justify-center px-2"
    >
      <div
        data-testid="chat-compact-status"
        data-reduce-motion={shouldReduceMotion ? "true" : "false"}
        className="chat-compact-status-line"
        role="status"
        aria-label={statusText}
      >
        <span className="chat-compact-status-rule" aria-hidden="true" />
        <span className="chat-compact-status-text">
          {statusText}{" "}
          <span className="font-mono">{active ? "(>_<)" : "(｡•̀ᴗ-)✧"}</span>
        </span>
        {active && onCancelCompaction && (
          <button
            type="button"
            data-testid="chat-compact-cancel"
            onClick={onCancelCompaction}
            className="rounded border border-border/60 px-1.5 py-0.5 text-[10px] text-text-secondary transition-colors hover:border-danger/50 hover:text-danger"
          >
            {t("chat.cancel")}
          </button>
        )}
        <span className="chat-compact-status-rule" aria-hidden="true" />
      </div>
    </motion.div>
    );
  }, [compactionElapsedSeconds, compactionPhaseLabel, compactionTerminalText, onCancelCompaction, shouldReduceMotion, t]);

  if (
    messages.length === 0 &&
    !isStreaming &&
    !loadingMsgs &&
    !isCompacting &&
    !compactCompleteVisible
  ) {
    return <div className="min-h-0 flex-1" data-testid="chat-empty-message-space" aria-hidden="true" />;
  }

  if (loadingMsgs) {
    return (
      <div className="min-h-0 flex-1 space-y-4 overflow-x-hidden overflow-y-auto px-4 py-4">
        <div className="flex justify-end">
          <div className="max-w-[60%] rounded-lg bg-accent-subtle px-3.5 py-2.5">
            <Skeleton className="h-4 w-48" />
          </div>
        </div>
        <div className="flex justify-start">
          <div className="max-w-[80%] rounded-lg bg-surface-2 px-3.5 py-2.5 space-y-2">
            <Skeleton className="h-4 w-64" />
            <Skeleton className="h-4 w-56" />
            <Skeleton className="h-4 w-40" />
          </div>
        </div>
        <div className="flex justify-end">
          <div className="max-w-[60%] rounded-lg bg-accent-subtle px-3.5 py-2.5">
            <Skeleton className="h-4 w-36" />
          </div>
        </div>
      </div>
    );
  }

  return (
    <>
    <div
      ref={scrollContainerRef}
      onScroll={handleScroll}
      data-chat-scroll-root="true"
      className="relative min-h-0 flex-1 overflow-x-hidden overflow-y-auto px-4 py-4 lg:pr-14"
      role="log"
      aria-live="polite"
      aria-label={t("chat.messageArea")}
    >
      <TurnNavigator
        items={turnNavigationItems}
        activeId={activeTurnNavigationId}
        onSelect={scrollToTurn}
        messageAreaLabel={t("chat.messageArea")}
      />
      <div
        data-chat-virtual-list="true"
        style={{
          height: `${rowVirtualizer.getTotalSize()}px`,
          position: 'relative',
          width: '100%',
        }}
      >
      <AnimatePresence initial={false}>
        {rowVirtualizer.getVirtualItems().map((virtualRow) => {
          const idx = renderableMessageIndexes[virtualRow.index];
          const msg = messages[idx];
          const content = (() => {
          if (msg.role === "system") {
            return isCompactionSummaryMessage(msg)
              ? renderCompactStatus(false, `compact-status-${msg.id}`)
              : null;
          }
          if (msg.role === "tool") return null;
          if (turnRenderMap.members.has(idx)) return null;

          const turnRender = turnRenderMap.anchors.get(idx);
          if (turnRender && msg.role === "user") {
            const assistantMsg =
              turnRender.assistantIdx != null
                ? messages[turnRender.assistantIdx]
                : null;
            const assistantIdx = turnRender.assistantIdx ?? -1;
            const traceGroup =
              assistantIdx >= 0
                ? messageTraceGroups.get(assistantIdx)
                : undefined;
            const chunkIds = assistantMsg
              ? (chunkIdCacheRef.current.get(assistantMsg.id) ?? [])
              : [];
            const turnDiffs =
              isStreaming && idx === latestUserIdx
                ? undefined
                : fileDiffGroups.byTurnId.get(turnRender.turn.id);
            const assistantImagePreviews =
              assistantIdx >= 0
                ? generatedImagePreviewsForIndexes(
                    traceGroup?.type === "anchor" && traceGroup.memberIndexes
                      ? traceGroup.memberIndexes
                      : [assistantIdx],
                  )
                : undefined;
            const traceSkills =
              traceGroup?.type === "anchor" ? (traceGroup.skills ?? []) : [];
            const liveSkills =
              liveTaskRunSkills?.turnId === turnRender.turn.id
                ? liveTaskRunSkills.skills
                : [];
            const visibleSkills = developerMode
              ? traceSkills.length > 0 ? traceSkills : liveSkills
              : [];
            const skillsAreLive =
              traceSkills.length === 0 && liveSkills.length > 0;

            return (
              <div
                key={`turn-${turnRender.turn.id}`}
                ref={(element) => registerTurnAnchor(turnRender.turn.id, element)}
                data-chat-turn-id={turnRender.turn.id}
              >
                <MessageBubble
                  msg={msg}
                  alwaysShowTimestamp={(() => {
                    for (let p = idx - 1; p >= 0; p -= 1) {
                      const prev = messages[p];
                      if (prev.role !== "tool" && prev.role !== "system") {
                        return hasTimeGap(prev.createdAt, msg.createdAt);
                      }
                    }
                    return false;
                  })()}
                  onDeleteMessage={onDeleteMessage}
                  onRetry={onRetry}
                  onEditAndResend={onEditAndResend}
                  onApprovePlan={onApprovePlan}
                  goalStatus={
                    isGoalMessage(msg)
                      ? activeGoalContext?.sourceMessageId === msg.id
                        ? "active"
                        : "complete"
                      : undefined
                  }
                />

                {visibleSkills.length > 0 && (
                  <TurnSkillStrip skills={visibleSkills} live={skillsAreLive} />
                )}

                {traceGroup?.type === "anchor" && (
                  <>{traceGroup.nodes}</>
                )}

                {renderGeneratedImagePreviews(assistantImagePreviews)}

                {assistantMsg &&
                  assistantMsg.role === "assistant" &&
                  assistantMsg.content.trim().length > 0 &&
                  !(traceGroup?.type === "anchor" && traceGroup.hideMessageBubble) && (
                    <MessageBubble
                      msg={assistantMsg}
                      chunkIds={chunkIds}
                      queryText={msg.content}
                      citationLookup={
                        assistantIdx >= 0
                          ? messageCitationLookups.get(assistantIdx)
                          : undefined
                      }
                      isLastAssistant={
                        assistantIdx === lastAssistantIdx && !isStreaming
                      }
                      lastCached={
                        assistantIdx === lastAssistantIdx
                          ? lastCached
                          : undefined
                      }
                      onRetry={onRetry}
                      alwaysShowTimestamp={hasTimeGap(
                        msg.createdAt,
                        assistantMsg.createdAt,
                      )}
                      onDeleteMessage={onDeleteMessage}
                      onEditAndResend={onEditAndResend}
                      onApprovePlan={onApprovePlan}
                      turnDurationLabel={formatTurnDuration(turnRender.turn)}
                    />
                  )}

                {props.conversationId && recordedFileChanges.has(turnRender.turn.id) ? (
                  turnRender.turn.id !== dockedFileChanges?.turnId && !(isStreaming && idx === latestUserIdx) && <TurnFileChanges conversationId={props.conversationId} summary={recordedFileChanges.get(turnRender.turn.id)!} />
                ) : renderFileDiffPreviews(
                  turnDiffs,
                  `turn-diff-${turnRender.turn.id}`,
                )}
              </div>
            );
          }

          const queryText =
            msg.role === "assistant"
              ? (messages
                  .slice(0, idx)
                  .reverse()
                  .find((m) => m.role === "user")?.content ?? "")
              : "";
          const chunkIds = chunkIdCacheRef.current.get(msg.id) ?? [];
          const traceGroup =
            msg.role === "assistant" ? messageTraceGroups.get(idx) : undefined;
          if (traceGroup?.type === "member") return null;
          const traceSkills =
            traceGroup?.type === "anchor" ? (traceGroup.skills ?? []) : [];
          const liveUserSkills =
            msg.role === "user" && liveTaskRunSkills?.userIdx === idx
              ? liveTaskRunSkills.skills
              : [];
          const hasRenderableAssistantContent =
            msg.role !== "assistant" ||
            (msg.content.trim().length > 0 &&
              !(traceGroup?.type === "anchor" && traceGroup.hideMessageBubble));
          const assistantDiffs = (() => {
            if (
              msg.role !== "assistant" ||
              (isStreaming && latestUserIdx >= 0 && idx > latestUserIdx)
            ) {
              return undefined;
            }
            if (traceGroup?.type === "anchor" && traceGroup.memberIndexes) {
              const diffs = traceGroup.memberIndexes.flatMap(
                (memberIdx) => fileDiffGroups.byAssistantIdx.get(memberIdx) ?? [],
              );
              return diffs.length > 0 ? diffs : undefined;
            }
            return fileDiffGroups.byAssistantIdx.get(idx);
          })();
          const assistantImagePreviews =
            msg.role === "assistant"
              ? generatedImagePreviewsForIndexes(
                  traceGroup?.type === "anchor" && traceGroup.memberIndexes
                    ? traceGroup.memberIndexes
                    : [idx],
                )
              : undefined;

          const navigationItem = msg.role === 'user'
            ? turnNavigationByMessageId.get(msg.id)
            : undefined;

          return (
            <div
              key={msg.id}
              ref={navigationItem
                ? (element) => registerTurnAnchor(navigationItem.id, element)
                : undefined}
              data-chat-turn-id={navigationItem?.id}
            >
              {developerMode && traceSkills.length > 0 && (
                <TurnSkillStrip skills={traceSkills} live={false} />
              )}

              {traceGroup?.type === "anchor" && (
                <>{traceGroup.nodes}</>
              )}

              {renderGeneratedImagePreviews(assistantImagePreviews)}

              {hasRenderableAssistantContent && (
                <MessageBubble
                  msg={msg}
                  chunkIds={chunkIds}
                  queryText={queryText}
                  citationLookup={messageCitationLookups.get(idx)}
                  isLastAssistant={idx === lastAssistantIdx && !isStreaming}
                  lastCached={idx === lastAssistantIdx ? lastCached : undefined}
                  onRetry={onRetry}
                  alwaysShowTimestamp={(() => {
                    for (let p = idx - 1; p >= 0; p -= 1) {
                      const prev = messages[p];
                      if (prev.role !== "tool" && prev.role !== "system") {
                        return hasTimeGap(prev.createdAt, msg.createdAt);
                      }
                    }
                    return false;
                  })()}
                  onDeleteMessage={onDeleteMessage}
                  onEditAndResend={onEditAndResend}
                  onApprovePlan={onApprovePlan}
                  goalStatus={
                    isGoalMessage(msg)
                      ? activeGoalContext?.sourceMessageId === msg.id
                        ? "active"
                        : "complete"
                      : undefined
                  }
                />
              )}

              {developerMode && liveUserSkills.length > 0 && (
                <TurnSkillStrip skills={liveUserSkills} live />
              )}

              {renderFileDiffPreviews(assistantDiffs, `message-diff-${msg.id}`)}
            </div>
          );
          })();
          return (
            <div
              key={virtualRow.key}
              ref={rowVirtualizer.measureElement}
              data-index={virtualRow.index}
              data-chat-virtual-row="true"
              style={{
                left: 0,
                position: 'absolute',
                top: 0,
                transform: `translateY(${virtualRow.start}px)`,
                width: '100%',
              }}
            >
              {content}
            </div>
          );
        })}
      </AnimatePresence>
      </div>

      {/* ── Interleaved per-round rendering ─────────────────────────── */}
      {shouldRenderLiveTraceTimeline && collapsedLiveTrace && (
        <>
          {renderTimelineTraceNode(
            "current-turn-working-trace",
            collapsedLiveTrace.historySections,
            false,
          )}
          {renderTraceReplyNode(
            collapsedLiveTrace.finalItem.id,
            collapsedLiveTrace.finalItem.content,
            collapsedLiveTrace.finalItem.isStreaming,
            streamingCitationLookup,
          )}
        </>
      )}

      {shouldRenderLiveTraceTimeline &&
        !collapsedLiveTrace &&
        liveTraceTimeline.map((item) => (
          <motion.div
            key={item.id}
            initial={shouldReduceMotion || isStreaming ? false : { opacity: 0 }}
            animate={{ opacity: 1 }}
            layout={shouldReduceMotion || isStreaming ? false : "position"}
            transition={shouldReduceMotion ? INSTANT_TRANSITION : SOFT_FADE_TRANSITION}
          >
            {item.kind === "thinking"
              ? renderTimelineTraceNode(
                  item.id,
                  item.sections,
                  item.isStreaming,
                )
              : renderTraceReplyNode(
                  item.id,
                  item.content,
                  item.isStreaming,
                  streamingCitationLookup,
                )}
          </motion.div>
        ))}

      {!shouldRenderLiveTraceTimeline &&
        shouldRenderStreamRounds &&
        streamRounds.map((round) => {
          const roundSections = buildRoundTimelineSections(round);
          const hasThinking = roundSections.length > 0;
          const hasReply = round.reply.trim().length > 0;
          if (!hasThinking && !hasReply) return null;
          return (
            <Fragment key={`round-${round.id}`}>
              {hasThinking &&
                renderTimelineTraceNode(
                  `round-thinking-${round.id}`,
                  roundSections,
                  false,
                )}
              {hasReply &&
                renderTraceReplyNode(
                  `round-reply-${round.id}`,
                  round.reply,
                  false,
                  streamingCitationLookup,
                )}
            </Fragment>
          );
        })}

      {/* ── Current in-progress thinking (not yet in a round) ──────── */}
      {!shouldRenderLiveTraceTimeline && currentTimelineSections.length > 0 && (
        <motion.div
          initial={shouldReduceMotion || isStreaming ? false : { opacity: 0 }}
          animate={{ opacity: 1 }}
          layout={shouldReduceMotion || isStreaming ? false : "position"}
          transition={shouldReduceMotion ? INSTANT_TRANSITION : SOFT_FADE_TRANSITION}
          className="flex justify-start mb-3"
        >
          <div className="w-full min-w-0">
            <ThinkingBlock
              content=""
              sections={renderTimelineSections(currentTimelineSections)}
              isStreaming={currentTraceActive}
              defaultExpanded={currentTraceActive}
              collapseOnFinish
            />
          </div>
        </motion.div>
      )}

      {shouldShowStreamingText &&
        streamText.trim().length > 0 &&
        renderTraceReplyNode(
          "fallback-stream-reply",
          displayedText,
          true,
          streamingCitationLookup,
        )}

      {isStreaming &&
        !streamText &&
        streamRounds.length === 0 &&
        visibleTraceEvents.length === 0 &&
        toolCalls.length === 0 &&
        !displayedThinkingText &&
        !isThinking && (
          <motion.div
            initial={shouldReduceMotion || isStreaming ? false : { opacity: 0 }}
            animate={{ opacity: 1 }}
            transition={shouldReduceMotion ? INSTANT_TRANSITION : undefined}
            className="flex justify-start mb-3"
          >
            <div
              className="rounded-lg px-3.5 py-2.5 bg-surface-2"
              role="status"
              aria-label={t("chat.thinking")}
            >
              <div className="flex items-center gap-2 text-sm text-text-tertiary">
                <WaitingMoodBadge />
                <span
                  className={`thinking-status-text ${shouldReduceMotion ? "" : "thinking-status-text-active"}`}
                >
                  {t("chat.thinking")}
                </span>
              </div>
            </div>
          </motion.div>
        )}

      {taskRun?.status === 'paused' && onResumePaused && (
        <div
          className="mb-3 flex justify-start"
          data-testid="chat-paused-resume"
          role="status"
        >
          <div className="flex items-center gap-3 rounded-lg border border-accent/25 bg-accent/10 px-3.5 py-2.5 text-sm">
            <span className="text-text-secondary">{t('taskCenter.paused')}</span>
            <button
              type="button"
              onClick={onResumePaused}
              className="inline-flex items-center gap-1.5 rounded-md border border-accent/35 bg-surface-1 px-2.5 py-1 text-xs font-medium text-accent hover:bg-surface-2"
            >
              <Play className="h-3.5 w-3.5" />
              {t('taskCenter.resume')}
            </button>
          </div>
        </div>
      )}

      {(isCompacting || compactCompleteVisible) &&
        renderCompactStatus(isCompacting, "compact-status-current")}

      {shouldRenderInlineError && (
        <motion.div
          initial={shouldReduceMotion ? false : { opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          exit={
            shouldReduceMotion ? { opacity: 0, y: 0 } : { opacity: 0, y: 8 }
          }
          transition={shouldReduceMotion ? INSTANT_TRANSITION : undefined}
          className="flex justify-start mb-3"
        >
          <div className="max-w-[80%] rounded-lg px-3.5 py-2.5 bg-red-500/10 border border-red-500/20 text-sm">
            <div className="flex items-start gap-2">
              <AlertCircle className="h-4 w-4 text-red-400 mt-0.5 shrink-0" />
              <div className="flex-1 min-w-0">
                <p className="text-red-400 font-medium text-xs mb-1">
                  {t("chat.errorOccurred")}
                </p>
                <p className="text-red-300/80 text-xs break-words">{error}</p>
                <div className="flex items-center gap-2 mt-2">
                  {onRetry && (
                    <button
                      type="button"
                      onClick={() => onRetry()}
                      className="inline-flex items-center gap-1 px-2 py-1 text-xs font-medium rounded-md bg-red-500/20 text-red-300 hover:bg-red-500/30 transition-colors cursor-pointer"
                    >
                      <RotateCcw className="h-3 w-3" />
                      {t("chat.retry")}
                    </button>
                  )}
                  {onDismissError && (
                    <button
                      type="button"
                      onClick={onDismissError}
                      className="inline-flex items-center gap-1 px-2 py-1 text-xs font-medium rounded-md bg-surface-2 text-text-tertiary hover:text-text-secondary transition-colors cursor-pointer"
                    >
                      <X className="h-3 w-3" />
                      {t("chat.dismiss")}
                    </button>
                  )}
                </div>
              </div>
            </div>
          </div>
        </motion.div>
      )}

      <AnimatePresence>
        {hasOverflow && !isNearBottom && (
          <motion.button
            initial={shouldReduceMotion ? false : { opacity: 0, y: 12 }}
            animate={{ opacity: 1, y: 0 }}
            exit={
              shouldReduceMotion ? { opacity: 0, y: 0 } : { opacity: 0, y: 12 }
            }
            transition={
              shouldReduceMotion
                ? INSTANT_TRANSITION
                : { duration: 0.18, ease: "easeOut" }
            }
            type="button"
            onClick={scrollToBottom}
            title={t("chat.scrollToBottom")}
            className="sticky bottom-3 left-1/2 -translate-x-1/2 mx-auto flex items-center gap-1.5 rounded-full bg-surface-3 hover:bg-surface-4 text-text-primary shadow-md px-3 py-2 transition-colors cursor-pointer z-10"
          >
            <ChevronDown className="h-4 w-4" />
            {unreadCount > 0 && (
              <span className="text-xs font-medium tabular-nums">
                {unreadCount}
              </span>
            )}
          </motion.button>
        )}
      </AnimatePresence>
    </div>
    {props.conversationId && dockedFileChanges && <div className="relative z-30 flex shrink-0 justify-center px-4 pb-2 pt-1" data-testid="file-changes-dock">
      <TurnFileChanges key={`${props.conversationId}:${dockedFileChanges.turnId}`} conversationId={props.conversationId} summary={dockedFileChanges} docked />
    </div>}
    </>
  );
}
