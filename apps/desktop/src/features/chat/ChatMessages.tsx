import {
  Fragment,
  type ReactNode,
  useRef,
  useEffect,
  useLayoutEffect,
  useMemo,
  useState,
  useCallback,
} from "react";
import { motion, AnimatePresence, useReducedMotion } from "framer-motion";
import {
  MessageCircle,
  ChevronDown,
  AlertCircle,
  RotateCcw,
  X,
  Search,
  FileText,
  Link2,
  HelpCircle,
  Presentation,
  Table2,
  ClipboardList,
} from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { rehypePlugins } from "../../components/chat/markdownComponents";
import { useTranslation } from "../../i18n";
import { useTypewriter } from "../../lib/useTypewriter";
import { hasTimeGap } from "../../lib/relativeTime";
import {
  preprocessChunkCitations,
  buildCitationMap,
  extractChunkCitations,
} from "../../lib/citationParser";
import type { CitationCardData } from "../../lib/citationParser";
import { isWebUrl, sourceBasename, sourceHost } from "../../lib/sourceDisplay";
import { SOFT_FADE_TRANSITION } from "../../lib/uiMotion";
import type {
  StreamRoundEvent,
  ToolCallEvent,
  TraceEvent,
} from "../../lib/useAgentStream";
import {
  extractPersistedTraceItems,
  extractTurnTrace,
} from "../../lib/streaming/persistedTrace";
import {
  buildCurrentTimelineSections,
  buildLiveTraceTimeline,
  buildRoundTimelineSections,
  hasRenderableTimelineSections,
  isCurrentTraceActive,
  normalizeThinking,
  persistedTraceItemsToTimelineSections,
  persistedTraceItemToTimelineSections,
  toolCallToTimelineSection,
  turnLifecycleTimelineSections,
  visibleTraceEventsForTimeline,
  type TimelineSection,
} from "../../lib/streaming/timelineViewModel";
import { ToolCallCard } from "../../components/chat/ToolCallCard";
import {
  FileDiffSummaryPanel,
  extractFileDiffArtifacts,
  mergeFileDiffArtifactsByPath,
  type FileDiffArtifact,
} from "../../components/chat/FileDiffPreview";
import { ThinkingBlock } from "../../components/chat/ThinkingBlock";
import type { ThinkingSection } from "../../components/chat/ThinkingBlock";
import {
  markdownComponents,
  preprocessFilePaths,
  preprocessCitations,
  CitationContext,
} from "../../components/chat/markdownComponents";
import { MessageBubble } from "../../components/chat/MessageBubble";
import { CitationChip } from "../../components/chat/EvidenceCard";
import { Skeleton } from "../../components/ui/Skeleton";
import type {
  ArtifactPayload,
  ConversationMessage,
  ConversationTurn,
} from "../../types/conversation";

interface ChatMessagesProps {
  messages: ConversationMessage[];
  turns: ConversationTurn[];
  streamText: string;
  streamRounds: StreamRoundEvent[];
  traceEvents: TraceEvent[];
  thinkingText: string;
  isThinking: boolean;
  toolCalls: ToolCallEvent[];
  isStreaming: boolean;
  error?: string | null;
  onRetry?: () => void;
  onDismissError?: () => void;
  onDeleteMessage?: (messageId: string) => void;
  onEditAndResend?: (messageId: string, newContent: string) => void;
  loadingMsgs?: boolean;
  lastCached?: boolean;
  onSuggestionClick?: (text: string) => void;
}

const SUGGESTIONS: {
  icon: typeof Search;
  labelKey: keyof import("../../i18n").TranslationKeys;
  promptKey: keyof import("../../i18n").TranslationKeys;
}[] = [
  {
    icon: Search,
    labelKey: "chat.suggestions.search",
    promptKey: "chat.suggestions.search.prompt",
  },
  {
    icon: FileText,
    labelKey: "chat.suggestions.summarize",
    promptKey: "chat.suggestions.summarize.prompt",
  },
  {
    icon: Link2,
    labelKey: "chat.suggestions.connections",
    promptKey: "chat.suggestions.connections.prompt",
  },
  {
    icon: HelpCircle,
    labelKey: "chat.suggestions.question",
    promptKey: "chat.suggestions.question.prompt",
  },
  {
    icon: FileText,
    labelKey: "chat.suggestions.report",
    promptKey: "chat.suggestions.report.prompt",
  },
  {
    icon: ClipboardList,
    labelKey: "chat.suggestions.meeting",
    promptKey: "chat.suggestions.meeting.prompt",
  },
  {
    icon: Presentation,
    labelKey: "chat.suggestions.slides",
    promptKey: "chat.suggestions.slides.prompt",
  },
  {
    icon: Table2,
    labelKey: "chat.suggestions.table",
    promptKey: "chat.suggestions.table.prompt",
  },
];

function evidenceSourceLabel(
  card: CitationCardData | undefined,
  displayText: string | undefined,
  fallback: string,
): string {
  const sourcePath = card?.documentPath?.trim() ?? "";
  const title = card?.documentTitle?.trim() ?? "";
  if (sourcePath) {
    const host = isWebUrl(sourcePath) ? sourceHost(sourcePath) : "";
    if (host) {
      return title ? `${title} · ${host}` : host;
    }
    return title || sourceBasename(sourcePath);
  }
  return displayText || fallback;
}

function buildExplicitEvidenceItems(
  content: string,
  citationLookup:
    | { getCard: (id: string) => CitationCardData | undefined }
    | undefined,
  fallbackLabel: (index: number) => string,
) {
  const grouped = new Map<
    string,
    {
      chunkId: string;
      card?: CitationCardData;
      count: number;
      displayText?: string;
    }
  >();

  for (const entry of extractChunkCitations(content)) {
    const card = citationLookup?.getCard(entry.chunkId);
    const groupKey =
      card?.documentPath?.trim() || card?.documentTitle?.trim() || entry.chunkId;
    const existing = grouped.get(groupKey);
    if (existing) {
      existing.count += 1;
      if (!existing.card && card) existing.card = card;
      if (!existing.displayText && entry.displayText) {
        existing.displayText = entry.displayText;
      }
      continue;
    }
    grouped.set(groupKey, {
      chunkId: entry.chunkId,
      card,
      count: 1,
      displayText: entry.displayText,
    });
  }

  return Array.from(grouped.values()).map((item, index) => {
    const baseLabel = evidenceSourceLabel(
      item.card,
      item.displayText,
      fallbackLabel(index + 1),
    );
    return {
      chunkId: item.chunkId,
      displayText: item.count > 1 ? `${baseLabel} ×${item.count}` : baseLabel,
      card: item.card,
    };
  });
}

const INSTANT_TRANSITION = { duration: 0 };
const NEAR_BOTTOM_THRESHOLD = 96;
const FOLLOW_RELEASE_THRESHOLD = 160;

type MessageTraceGroup =
  | {
      type: "anchor";
      nodes: ReactNode[];
      hideMessageBubble?: boolean;
      memberIndexes?: number[];
    }
  | { type: "member" };

interface GeneratedImagePreviewItem {
  id: string;
  toolName: string;
  arguments: string;
  plugin?: ToolCallEvent["plugin"];
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

export function ChatMessages({
  messages,
  turns,
  streamText,
  streamRounds,
  traceEvents,
  thinkingText,
  isThinking,
  toolCalls,
  isStreaming,
  error,
  onRetry,
  onDismissError,
  onDeleteMessage,
  onEditAndResend,
  loadingMsgs,
  lastCached,
  onSuggestionClick,
}: ChatMessagesProps) {
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

  const typewriterText = useTypewriter(streamText, isStreaming, {
    charsPerTick: 5,
    intervalMs: 30,
  });
  const displayedText = shouldReduceMotion ? streamText : typewriterText;

  const [debouncedMarkdown, setDebouncedMarkdown] = useState("");
  const latestDisplayedTextRef = useRef(displayedText);
  const markdownThrottleTimerRef = useRef<ReturnType<typeof setTimeout> | null>(
    null,
  );

  useEffect(() => {
    latestDisplayedTextRef.current = displayedText;
  }, [displayedText]);

  useEffect(() => {
    const flushImmediately =
      shouldReduceMotion || !isStreaming || displayedText.length <= 240;

    if (flushImmediately) {
      if (markdownThrottleTimerRef.current) {
        clearTimeout(markdownThrottleTimerRef.current);
        markdownThrottleTimerRef.current = null;
      }
      setDebouncedMarkdown(displayedText);
      return;
    }

    if (markdownThrottleTimerRef.current) {
      return;
    }

    const throttleMs = isStreaming ? 150 : 60;
    markdownThrottleTimerRef.current = setTimeout(() => {
      markdownThrottleTimerRef.current = null;
      setDebouncedMarkdown(latestDisplayedTextRef.current);
    }, throttleMs);
  }, [displayedText, isStreaming, shouldReduceMotion]);

  useEffect(
    () => () => {
      if (markdownThrottleTimerRef.current) {
        clearTimeout(markdownThrottleTimerRef.current);
        markdownThrottleTimerRef.current = null;
      }
    },
    [],
  );

  const remarkPlugins = useMemo(() => [remarkGfm], []);

  const preprocessStreamingMarkdown = useCallback(
    (content: string) =>
      preprocessFilePaths(
        preprocessCitations(preprocessChunkCitations(content)),
      ),
    [],
  );

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
      const evidenceItems = buildExplicitEvidenceItems(
        content,
        effectiveCitationLookup,
        (index) => t("chat.evidenceSourceLabel", { index: String(index) }),
      );

      return (
        <div key={key} className="flex justify-start mb-4">
          <div className="w-full max-w-[min(100%,72rem)] text-sm leading-relaxed text-text-primary">
            {evidenceItems.length > 0 && (
              <div className="mb-3 rounded-xl border border-border/70 bg-surface-1/70 px-2.5 py-2">
                <div className="mb-1 flex items-center justify-between gap-2">
                  <span className="text-[11px] font-medium text-text-secondary">
                    {t("chat.answerEvidence")}
                  </span>
                  <span className="text-[10px] text-text-tertiary">
                    {t("chat.answerEvidenceSummary", {
                      count: String(evidenceItems.length),
                    })}
                  </span>
                </div>
                <div className="flex flex-wrap gap-1.5">
                  {evidenceItems.map((item) => (
                    <CitationChip
                      key={item.chunkId}
                      chunkId={item.chunkId}
                      displayText={item.displayText}
                      card={item.card}
                    />
                  ))}
                </div>
              </div>
            )}
            <CitationContext.Provider
              value={effectiveCitationLookup}
            >
              <div className="relative">
                <div className="prose-chat">
                  <ReactMarkdown
                    remarkPlugins={remarkPlugins}
                    rehypePlugins={rehypePlugins}
                    components={markdownComponents}
                    urlTransform={(url) => url}
                  >
                    {preprocessStreamingMarkdown(content)}
                  </ReactMarkdown>
                </div>
                {isStreaming && (
                  <span
                    className={`streaming-caret-overlay ${shouldReduceMotion ? "" : "animate-pulse"}`}
                  />
                )}
              </div>
            </CitationContext.Provider>
          </div>
        </div>
      );
    },
    [
      allToolCitationLookup,
      preprocessStreamingMarkdown,
      remarkPlugins,
      shouldReduceMotion,
      t,
    ],
  );

  const renderThinkingTraceNode = useCallback(
    (key: string, sections: ThinkingSection[], isStreaming = false) => (
      <div key={key} className="flex justify-start mb-1">
        <div className="w-full max-w-[min(100%,72rem)]">
          <ThinkingBlock
            content=""
            sections={sections}
            isStreaming={isStreaming}
            defaultExpanded={isStreaming}
            collapseOnFinish
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
                toolName={section.toolCall.toolName}
                arguments={section.toolCall.arguments}
                status={section.toolCall.status}
                plugin={section.toolCall.plugin}
                renderKind={section.toolCall.renderKind}
                capabilities={section.toolCall.capabilities}
                durationMs={section.toolCall.durationMs}
                content={section.toolCall.content}
                isError={section.toolCall.isError}
                artifacts={section.toolCall.artifacts}
                argsStatus={section.toolCall.argsStatus}
                argsBytes={section.toolCall.argsBytes}
                trace={section.trace}
              />
            ),
          };
        default:
          return null;
      }
    },
    [renderTraceReplyNode],
  );

  const renderTimelineSections = useCallback(
    (sections: TimelineSection[]): ThinkingSection[] =>
      sections
        .map(renderTimelineSection)
        .filter((section): section is ThinkingSection => Boolean(section)),
    [renderTimelineSection],
  );

  const renderTimelineTraceNode = useCallback(
    (key: string, sections: TimelineSection[], isStreaming = false) =>
      renderThinkingTraceNode(
        key,
        renderTimelineSections(sections),
        isStreaming,
      ),
    [renderThinkingTraceNode, renderTimelineSections],
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

    for (const turn of turns) {
      if (!turn.assistantMessageId) continue;
      const assistantIdx = messageIndexById.get(turn.assistantMessageId);
      if (assistantIdx == null) continue;

      finalAssistantIndexes.add(assistantIdx);

      const trace = extractTurnTrace(turn.trace);
      const sections = turnLifecycleTimelineSections({
        turn,
        routeKind: trace?.routeKind,
      });
      const fallbackSections: TimelineSection[] = [];

      for (const [itemIdx, item] of (trace?.items ?? []).entries()) {
        const itemSections = persistedTraceItemToTimelineSections({
          item,
          id: `turn-${turn.id}-${item.kind}-${itemIdx}`,
          trace: Boolean(trace),
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
      const anchorIdx = finalAssistantIdx ?? persistedTraceCarrierIdx ?? currentGroup[0];

      const persistedTraceSections: TimelineSection[] =
        persistedTraceCarrierIdx == null
          ? []
          : persistedTraceItemsToTimelineSections({
              items: extractPersistedTraceItems(
                messages[persistedTraceCarrierIdx].artifacts,
              ),
              idPrefix: `persisted-${messages[persistedTraceCarrierIdx].id}`,
              trace: true,
            });

      const statusSections =
        statusSectionsByAssistant.get(anchorIdx) ?? persistedTraceSections;
      const nodes: ReactNode[] = [];
      const hiddenMembers = new Set<number>();
      let activeSections: TimelineSection[] = [...statusSections];
      const flushThinkingNode = (key: string) => {
        if (!hasRenderableTimelineSections(activeSections)) return;
        nodes.push(renderTimelineTraceNode(key, activeSections));
        activeSections = [];
      };

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
              plugin: tc.plugin,
              argsStatus: status === "done" ? "done" : "ready",
              argsBytes: argumentsText.length,
              content: toolResult?.content,
              artifacts: toolResult?.artifacts ?? undefined,
            },
          });
        });

        if (thinking) {
          activeSections.push({
            kind: "thinking",
            id: `message-thinking-${msg.id}`,
            text: thinking,
          });
        }

        const shouldRenderInlineReply =
          msg.content.trim().length > 0 &&
          !(idx === anchorIdx && msg.toolCalls.length === 0);
        if (shouldRenderInlineReply) {
          flushThinkingNode(`trace-thinking-before-reply-${msg.id}`);
          nodes.push(
            renderTraceReplyNode(
              `trace-reply-${msg.id}`,
              msg.content,
              false,
              messageCitationLookups.get(idx),
            ),
          );
        }

        if (inlineToolSections.length > 0) {
          activeSections.push(...inlineToolSections);
        }

        if (idx !== anchorIdx) {
          hiddenMembers.add(idx);
        }
      }

      flushThinkingNode(`trace-thinking-tail-${messages[anchorIdx].id}`);

      if (nodes.length === 0) {
        const fallbackSections = [
          ...statusSections,
          ...(fallbackSectionsByAssistant.get(anchorIdx) ?? []),
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

      if (nodes.length > 0) {
        map.set(anchorIdx, {
          type: "anchor",
          nodes,
          hideMessageBubble: messages[anchorIdx].toolCalls.length > 0,
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
    messageCitationLookups,
    messageIndexById,
    messageThinkingText,
    messageToolCalls,
    messages,
    renderTimelineTraceNode,
    renderTraceReplyNode,
    turns,
  ]);

  const visibleTraceEvents = useMemo(
    () => visibleTraceEventsForTimeline(traceEvents),
    [traceEvents],
  );

  const currentTimelineSections = useMemo(
    () => buildCurrentTimelineSections({ visibleTraceEvents, streamRounds }),
    [visibleTraceEvents, streamRounds],
  );

  const currentTraceActive = useMemo(
    () =>
      isCurrentTraceActive({
        isStreaming,
        isThinking,
        thinkingText,
        toolCalls,
        visibleTraceEvents,
      }),
    [isStreaming, isThinking, thinkingText, toolCalls, visibleTraceEvents],
  );

  const liveTraceTimeline = useMemo(
    () =>
      buildLiveTraceTimeline({
        visibleTraceEvents,
        isStreaming,
        currentTraceActive,
        streamText,
        displayedText,
      }),
    [
      currentTraceActive,
      displayedText,
      isStreaming,
      streamText,
      visibleTraceEvents,
    ],
  );

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
    },
    [],
  );

  const handleScroll = useCallback(() => {
    const { distanceFromBottom, nearBottom, overflow } = getScrollMetrics();
    setHasOverflow(overflow);
    setIsNearBottom(!overflow || nearBottom);

    if (!overflow || nearBottom) {
      shouldAutoFollowRef.current = true;
      setUnreadCount(0);
      return;
    }

    if (distanceFromBottom > FOLLOW_RELEASE_THRESHOLD) {
      shouldAutoFollowRef.current = false;
    }
  }, [getScrollMetrics]);

  useEffect(() => {
    const newCount = messages.length - prevMsgCountRef.current;
    if (newCount > 0 && hasOverflow && !shouldAutoFollowRef.current) {
      setUnreadCount((count) => count + newCount);
    }
    prevMsgCountRef.current = messages.length;
  }, [messages.length, hasOverflow]);

  useLayoutEffect(() => {
    const { nearBottom, overflow } = getScrollMetrics();
    setHasOverflow(overflow);
    if (!overflow) {
      shouldAutoFollowRef.current = true;
      setIsNearBottom(true);
      setUnreadCount(0);
      return;
    }

    if (!shouldAutoFollowRef.current) {
      setIsNearBottom(nearBottom);
      return;
    }

    scrollToContainerBottom("auto");
  }, [
    messages,
    debouncedMarkdown,
    streamRounds,
    traceEvents,
    toolCalls,
    getScrollMetrics,
    scrollToContainerBottom,
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
        plugin: owner.toolCall.plugin,
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
          <div className="w-full max-w-[min(100%,72rem)]">
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
                plugin={item.plugin}
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

  const shouldRenderLiveTraceTimeline = liveTraceTimeline.length > 0;
  const shouldRenderStreamRounds =
    !shouldRenderLiveTraceTimeline && streamRounds.length > 0;
  const shouldShowStreamingText =
    !shouldRenderLiveTraceTimeline &&
    (isStreaming ||
      (streamText.trim().length > 0 &&
        (lastRenderableMessageRole == null ||
          lastRenderableMessageRole === "user")));
  const shouldRenderInlineError = Boolean(
    error && !isStreaming && traceEvents.length === 0,
  );

  if (messages.length === 0 && !isStreaming && !loadingMsgs) {
    return (
      <div className="flex-1 flex items-center justify-center">
        <div className="text-center max-w-md w-full px-4">
          <div className="p-4 rounded-2xl bg-surface-2 text-text-tertiary inline-block mb-4">
            <MessageCircle className="h-8 w-8" />
          </div>
          <p className="text-sm text-text-tertiary mb-6">
            {t("chat.placeholder")}
          </p>
          {onSuggestionClick && (
            <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
              {SUGGESTIONS.map((s, i) => {
                const Icon = s.icon;
                const prompt = t(s.promptKey);
                return (
                  <motion.button
                    key={s.labelKey}
                    type="button"
                    initial={shouldReduceMotion ? false : { opacity: 0, y: 12 }}
                    animate={{ opacity: 1, y: 0 }}
                    transition={
                      shouldReduceMotion
                        ? INSTANT_TRANSITION
                        : { delay: i * 0.07, duration: 0.3, ease: "easeOut" }
                    }
                    onClick={() => onSuggestionClick(prompt)}
                    className="bg-surface-1 hover:bg-surface-2 border border-border rounded-lg p-4 cursor-pointer transition-colors text-left"
                  >
                    <Icon className="h-4 w-4 text-accent mb-2" />
                    <p className="text-sm font-medium text-text-primary mb-1">
                      {t(s.labelKey)}
                    </p>
                    <p className="text-xs text-text-tertiary truncate">
                      {prompt}
                    </p>
                  </motion.button>
                );
              })}
            </div>
          )}
        </div>
      </div>
    );
  }

  if (loadingMsgs) {
    return (
      <div className="flex-1 overflow-y-auto px-4 py-4 space-y-4">
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
    <div
      ref={scrollContainerRef}
      onScroll={handleScroll}
      data-chat-scroll-root="true"
      className="flex-1 overflow-y-auto px-4 py-4 relative"
      role="log"
      aria-live="polite"
      aria-label={t("chat.messageArea")}
    >
      <AnimatePresence initial={false}>
        {messages.map((msg, idx) => {
          if (msg.role === "tool" || msg.role === "system") return null;
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

            return (
              <div key={`turn-${turnRender.turn.id}`}>
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
                  onEditAndResend={onEditAndResend}
                />

                {traceGroup?.type === "anchor" && (
                  <>{traceGroup.nodes}</>
                )}

                {renderGeneratedImagePreviews(assistantImagePreviews)}

                {assistantMsg &&
                  assistantMsg.role === "assistant" &&
                  assistantMsg.content.trim().length > 0 && (
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
                    />
                  )}

                {renderFileDiffPreviews(
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

          return (
            <div key={msg.id}>
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
                />
              )}

              {renderFileDiffPreviews(assistantDiffs, `message-diff-${msg.id}`)}
            </div>
          );
        })}
      </AnimatePresence>

      {/* ── Interleaved per-round rendering ─────────────────────────── */}
      {shouldRenderLiveTraceTimeline &&
        liveTraceTimeline.map((item) => (
          <motion.div
            key={item.id}
            initial={shouldReduceMotion || isStreaming ? false : { opacity: 0 }}
            animate={{ opacity: 1 }}
            layout={shouldReduceMotion ? false : "position"}
            transition={shouldReduceMotion ? INSTANT_TRANSITION : SOFT_FADE_TRANSITION}
          >
            {item.kind === "thinking"
              ? renderTimelineTraceNode(item.id, item.sections, item.isStreaming)
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
              {hasReply &&
                renderTraceReplyNode(
                  `round-reply-${round.id}`,
                  round.reply,
                  false,
                  streamingCitationLookup,
                )}
              {hasThinking &&
                renderTimelineTraceNode(
                  `round-thinking-${round.id}`,
                  roundSections,
                  false,
                )}
            </Fragment>
          );
        })}

      {/* ── Current in-progress thinking (not yet in a round) ──────── */}
      {!shouldRenderLiveTraceTimeline && currentTimelineSections.length > 0 && (
        <motion.div
          initial={shouldReduceMotion || isStreaming ? false : { opacity: 0 }}
          animate={{ opacity: 1 }}
          layout={shouldReduceMotion ? false : "position"}
          transition={shouldReduceMotion ? INSTANT_TRANSITION : SOFT_FADE_TRANSITION}
          className="flex justify-start mb-3"
        >
          <div className="w-full max-w-[min(100%,72rem)]">
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
        !thinkingText &&
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
                <div className="flex gap-1">
                  <span
                    className={`w-1.5 h-1.5 rounded-full bg-text-tertiary ${shouldReduceMotion ? "" : "animate-bounce"}`}
                    style={{ animationDelay: "0ms" }}
                  />
                  <span
                    className={`w-1.5 h-1.5 rounded-full bg-text-tertiary ${shouldReduceMotion ? "" : "animate-bounce"}`}
                    style={{ animationDelay: "150ms" }}
                  />
                  <span
                    className={`w-1.5 h-1.5 rounded-full bg-text-tertiary ${shouldReduceMotion ? "" : "animate-bounce"}`}
                    style={{ animationDelay: "300ms" }}
                  />
                </div>
                {t("chat.thinking")}
              </div>
            </div>
          </motion.div>
        )}

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
                      onClick={onRetry}
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
  );
}
