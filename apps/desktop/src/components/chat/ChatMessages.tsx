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
import { rehypePlugins } from "./markdownComponents";
import { useTranslation } from "../../i18n";
import { useTypewriter } from "../../lib/useTypewriter";
import { hasTimeGap } from "../../lib/relativeTime";
import { appTimeMs } from "../../lib/dateTime";
import {
  preprocessChunkCitations,
  buildCitationMap,
} from "../../lib/citationParser";
import type { CitationCardData } from "../../lib/citationParser";
import type {
  StreamRoundEvent,
  ToolCallEvent,
  TraceEvent,
} from "../../lib/useAgentStream";
import { ToolCallCard } from "./ToolCallCard";
import { ThinkingBlock } from "./ThinkingBlock";
import type { ThinkingSection } from "./ThinkingBlock";
import {
  markdownComponents,
  preprocessFilePaths,
  preprocessCitations,
  CitationContext,
} from "./markdownComponents";
import { MessageBubble } from "./MessageBubble";
import { Skeleton } from "../ui/Skeleton";
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

const INSTANT_TRANSITION = { duration: 0 };
const NEAR_BOTTOM_THRESHOLD = 96;
const FOLLOW_RELEASE_THRESHOLD = 160;

function normalizeThinking(content: string): string {
  return content.replace(/\r\n/g, "\n").trim();
}

interface PersistedTraceToolCall {
  callId: string;
  toolName: string;
  arguments: string;
  status: "running" | "done" | "error";
  content?: string;
  isError?: boolean;
  artifacts?: ArtifactPayload;
}

type PersistedTraceItem =
  | { kind: "thinking"; text: string }
  | { kind: "reply"; text: string }
  | { kind: "tool"; toolCall: PersistedTraceToolCall }
  | { kind: "status"; text: string; tone?: "muted" | "success" | "error" };

type MessageTraceGroup =
  | { type: "anchor"; nodes: ReactNode[]; hideMessageBubble?: boolean }
  | { type: "member" };

type LiveTraceTimelineItem =
  | { kind: "thinking"; id: string; sections: ThinkingSection[]; isStreaming: boolean }
  | { kind: "reply"; id: string; content: string; isStreaming: boolean };

function formatRouteKind(routeKind: string): string {
  return routeKind
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .replace(/^./, (char) => char.toUpperCase());
}

function shouldHideRouteKind(routeKind: string | null | undefined): boolean {
  return (
    (routeKind ?? "").replace(/\s+/g, "").toLowerCase() === "directresponse"
  );
}

function shouldHideTraceStatus(text: string | null | undefined): boolean {
  const normalized = (text ?? "").replace(/\s+/g, "").toLowerCase();
  return (
    normalized === "routeselected:directresponse" ||
    normalized === "route:directresponse"
  );
}

function formatTurnStatus(status: string): string {
  switch (status) {
    case "success":
      return "Success";
    case "cached":
      return "Cached";
    case "cancelled":
      return "Cancelled";
    case "max_iterations":
      return "Max iterations";
    case "error":
      return "Error";
    case "running":
    default:
      return "Running";
  }
}

function formatTurnDuration(turn: ConversationTurn): string | null {
  if (!turn.finishedAt) return null;
  const startedAt = appTimeMs(turn.createdAt);
  const finishedAt = appTimeMs(turn.finishedAt);
  if (
    Number.isNaN(startedAt) ||
    Number.isNaN(finishedAt) ||
    finishedAt < startedAt
  )
    return null;
  const seconds = Math.max(0, Math.round((finishedAt - startedAt) / 1000));
  return `${seconds}s`;
}

function extractPersistedTraceItems(
  artifacts: ConversationMessage["artifacts"],
): PersistedTraceItem[] | null {
  if (!artifacts || Array.isArray(artifacts) || typeof artifacts !== "object")
    return null;
  const record = artifacts as Record<string, unknown>;
  if (record.kind !== "traceTimeline" || !Array.isArray(record.items))
    return null;

  const items: PersistedTraceItem[] = [];
  for (const rawItem of record.items) {
    if (!rawItem || typeof rawItem !== "object") continue;
    const item = rawItem as Record<string, unknown>;
    if (item.kind === "thinking" && typeof item.text === "string") {
      items.push({ kind: "thinking", text: item.text });
      continue;
    }
    if (item.kind === "reply" && typeof item.text === "string") {
      items.push({ kind: "reply", text: item.text });
      continue;
    }
    if (item.kind === "status" && typeof item.text === "string") {
      items.push({
        kind: "status",
        text: item.text,
        tone:
          item.tone === "success" || item.tone === "error"
            ? item.tone
            : "muted",
      });
      continue;
    }
    if (
      item.kind === "tool" &&
      item.toolCall &&
      typeof item.toolCall === "object"
    ) {
      const toolCall = item.toolCall as Record<string, unknown>;
      if (
        typeof toolCall.callId !== "string" ||
        typeof toolCall.toolName !== "string"
      )
        continue;
      items.push({
        kind: "tool",
        toolCall: {
          callId: toolCall.callId,
          toolName: toolCall.toolName,
          arguments:
            typeof toolCall.arguments === "string" ? toolCall.arguments : "",
          status:
            toolCall.status === "error"
              ? "error"
              : toolCall.status === "running"
                ? "running"
                : "done",
          content:
            typeof toolCall.content === "string" ? toolCall.content : undefined,
          isError:
            typeof toolCall.isError === "boolean"
              ? toolCall.isError
              : undefined,
          artifacts:
            toolCall.artifacts && typeof toolCall.artifacts === "object"
              ? (toolCall.artifacts as ArtifactPayload)
              : undefined,
        },
      });
    }
  }

  return items.length > 0 ? items : null;
}

function extractTurnTrace(
  trace: ConversationTurn["trace"],
): { routeKind?: string; items: PersistedTraceItem[] } | null {
  if (!trace || Array.isArray(trace) || typeof trace !== "object") return null;
  const record = trace as Record<string, unknown>;
  if (record.kind !== "turnTrace" || !Array.isArray(record.items)) return null;
  const items = extractPersistedTraceItems({
    kind: "traceTimeline",
    items: record.items,
  } as unknown as ConversationMessage["artifacts"]);
  if (!items || items.length === 0) return null;
  return {
    routeKind:
      typeof record.routeKind === "string" ? record.routeKind : undefined,
    items,
  };
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

  return (
    <div
      className={`rounded-md border px-3 py-2 text-[11px] leading-relaxed ${toneClass}`}
    >
      {text}
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
      const arr = Array.isArray(tc.artifacts)
        ? tc.artifacts
        : Object.values(tc.artifacts);
      for (const item of arr) {
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

  const renderTraceReplyNode = useCallback(
    (
      key: string,
      content: string,
      isStreaming = false,
      citationLookup?: { getCard: (id: string) => CitationCardData | undefined },
    ) => (
      <div key={key} className="flex justify-start mb-4">
        <div className="w-full max-w-[min(100%,72rem)] text-sm leading-relaxed text-text-primary">
          <CitationContext.Provider
            value={citationLookup ?? { getCard: () => undefined }}
          >
            <div className="relative">
              <div className="prose-chat">
                <ReactMarkdown
                  remarkPlugins={remarkPlugins}
                  rehypePlugins={rehypePlugins}
                  components={markdownComponents}
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
    ),
    [preprocessStreamingMarkdown, remarkPlugins, shouldReduceMotion],
  );

  const renderThinkingTraceNode = useCallback(
    (key: string, sections: ThinkingSection[], isStreaming = false) => (
      <div key={key} className="flex justify-start mb-1">
        <div className="w-full max-w-[min(100%,72rem)]">
          <ThinkingBlock
            content=""
            sections={sections}
            isStreaming={isStreaming}
            defaultExpanded
            collapseOnFinish={false}
          />
        </div>
      </div>
    ),
    [],
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
    const statusSectionsByAssistant = new Map<number, ThinkingSection[]>();
    const fallbackSectionsByAssistant = new Map<number, ThinkingSection[]>();

    for (const turn of turns) {
      if (!turn.assistantMessageId) continue;
      const assistantIdx = messageIndexById.get(turn.assistantMessageId);
      if (assistantIdx == null) continue;

      finalAssistantIndexes.add(assistantIdx);

      const sections: ThinkingSection[] = [];
      const fallbackSections: ThinkingSection[] = [];
      const trace = extractTurnTrace(turn.trace);
      if (trace?.routeKind && !shouldHideRouteKind(trace.routeKind)) {
        sections.push({
          text: "",
          node: (
            <TraceStatusRow
              key={`turn-route-${turn.id}`}
              text={`Route: ${formatRouteKind(trace.routeKind)}`}
              tone="muted"
            />
          ),
        });
      }

      const duration = formatTurnDuration(turn);
      sections.push({
        text: "",
        node: (
          <TraceStatusRow
            key={`turn-status-${turn.id}`}
            text={`Status: ${formatTurnStatus(turn.status)}${duration ? ` · ${duration}` : ""}`}
            tone={
              turn.status === "error"
                ? "error"
                : turn.status === "success" || turn.status === "cached"
                  ? "success"
                  : "muted"
            }
          />
        ),
      });

      for (const [itemIdx, item] of (trace?.items ?? []).entries()) {
        if (item.kind === "status") {
          if (shouldHideTraceStatus(item.text)) continue;
          sections.push({
            text: "",
            node: (
              <TraceStatusRow
                key={`turn-status-${turn.id}-${itemIdx}`}
                text={item.text}
                tone={item.tone}
              />
            ),
          });
          continue;
        }
        if (item.kind === "thinking") {
          fallbackSections.push({ text: item.text });
          continue;
        }
        if (item.kind === "reply") {
          fallbackSections.push({
            text: "",
            node: renderTraceReplyNode(
              `turn-reply-${turn.id}-${itemIdx}`,
              item.text,
            ),
          });
          continue;
        }
        fallbackSections.push({
          text: "",
          node: (
            <ToolCallCard
              key={`turn-tool-${turn.id}-${item.toolCall.callId}-${itemIdx}`}
              toolName={item.toolCall.toolName}
              arguments={item.toolCall.arguments}
              status={item.toolCall.status}
              content={item.toolCall.content}
              isError={item.toolCall.isError}
              artifacts={item.toolCall.artifacts}
              trace={Boolean(trace)}
            />
          ),
        });
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

      const persistedStatusSections: ThinkingSection[] =
        persistedTraceCarrierIdx == null
          ? []
          : (extractPersistedTraceItems(messages[persistedTraceCarrierIdx].artifacts) ?? [])
              .flatMap((item, itemIdx) => {
                if (item.kind !== "status" || shouldHideTraceStatus(item.text)) {
                  return [];
                }
                return {
                  text: "",
                  node: (
                    <TraceStatusRow
                      key={`persisted-status-${messages[persistedTraceCarrierIdx].id}-${itemIdx}`}
                      text={item.text}
                      tone={item.tone}
                    />
                  ),
                };
              });

      const statusSections =
        statusSectionsByAssistant.get(anchorIdx) ?? persistedStatusSections;
      const nodes: ReactNode[] = [];
      const hiddenMembers = new Set<number>();
      let activeSections: ThinkingSection[] = [...statusSections];
      const hasRenderableSections = (sections: ThinkingSection[]) =>
        sections.some(
          (section) =>
            section.text.trim().length > 0 || Boolean(section.node || section.toolCallCards),
        );
      const flushThinkingNode = (key: string) => {
        if (!hasRenderableSections(activeSections)) return;
        nodes.push(renderThinkingTraceNode(key, activeSections));
        activeSections = [];
      };

      for (const idx of currentGroup) {
        const msg = messages[idx];
        const thinking = messageThinkingText.get(idx) ?? "";
        const renderedToolCalls = msg.toolCalls
          .filter((tc) => tc.name !== "update_plan")
          .map((tc, toolIdx) => {
            const toolResult = messageToolCalls
              .get(idx)
              ?.find((tr) => tr.toolCallId === tc.id);
            return (
              <ToolCallCard
                key={`persisted-trace-${msg.id}-${tc.id || tc.name || toolIdx}`}
                toolName={tc.name || "unknown_tool"}
                arguments={tc.arguments || ""}
                status={toolResult ? "done" : "running"}
                content={toolResult?.content}
                artifacts={toolResult?.artifacts ?? undefined}
                trace={true}
              />
            );
          });

        if (thinking) {
          activeSections.push({ text: thinking });
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

        if (renderedToolCalls.length > 0) {
          activeSections.push({
            text: "",
            node: <div className="mt-1 space-y-1">{renderedToolCalls}</div>,
          });
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
        if (hasRenderableSections(fallbackSections)) {
          nodes.push(
            renderThinkingTraceNode(
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
    renderThinkingTraceNode,
    renderTraceReplyNode,
    turns,
  ]);

  const visibleTraceEvents = useMemo(
    () =>
      traceEvents.filter(
        (event) =>
          !(event.kind === "status" && shouldHideTraceStatus(event.text)),
      ),
    [traceEvents],
  );

  const traceEventToSections = useCallback(
    (event: TraceEvent): ThinkingSection[] => {
      if (event.kind === "reply") return [];
      if (event.kind === "thinking") {
        return event.text.trim().length > 0 ? [{ text: event.text }] : [];
      }
      if (event.kind === "tool") {
        return [
          {
            text: "",
            node: (
              <ToolCallCard
                key={`stream-trace-${event.id}`}
                toolName={event.toolCall.toolName}
                arguments={event.toolCall.arguments}
                status={event.toolCall.status}
                content={event.toolCall.content}
                isError={event.toolCall.isError}
                artifacts={event.toolCall.artifacts}
                trace={true}
              />
            ),
          },
        ];
      }
      return [
        {
          text: "",
          node: (
            <TraceStatusRow
              key={`stream-status-${event.id}`}
              text={event.text}
              tone={event.tone}
            />
          ),
        },
      ];
    },
    [],
  );

  const streamTraceSections = useMemo<ThinkingSection[]>(() => {
    return visibleTraceEvents.flatMap(traceEventToSections);
  }, [traceEventToSections, visibleTraceEvents]);

  /**
   * Build ThinkingSection[] for a single streaming round.
   *
   * `streamStore` closes the previous visible reply when the next tool phase
   * starts, so a round's `reply` belongs before that round's thinking/tool
   * trace in the rendered timeline.
   */
  const buildRoundSections = useCallback(
    (round: StreamRoundEvent): ThinkingSection[] => {
      const sections: ThinkingSection[] = [];
      if (round.thinking?.trim()) {
        sections.push({ text: round.thinking });
      }
      for (const tc of round.toolCalls) {
        sections.push({
          text: "",
          node: (
            <ToolCallCard
              key={`round-tool-${round.id}-${tc.callId}`}
              toolName={tc.toolName}
              arguments={tc.arguments}
              status={tc.status}
              content={tc.content}
              isError={tc.isError}
              artifacts={tc.artifacts}
              trace={true}
            />
          ),
        });
      }
      return sections;
    },
    [],
  );

  /**
   * Trace events that are NOT already captured inside a completed round.
   * We find the last tool trace event whose callId belongs to a round,
   * then treat everything after that index as "current / in-progress".
   */
  const currentThinkingSections = useMemo<ThinkingSection[]>(() => {
    if (streamRounds.length === 0) return streamTraceSections;

    const roundCallIds = new Set<string>();
    for (const round of streamRounds) {
      for (const tc of round.toolCalls) {
        roundCallIds.add(tc.callId);
      }
    }

    // Find last trace event index belonging to a completed round
    let cutoffIdx = -1;
    for (let i = visibleTraceEvents.length - 1; i >= 0; i--) {
      const ev = visibleTraceEvents[i];
      if (ev.kind === "tool" && roundCallIds.has(ev.toolCall.callId)) {
        cutoffIdx = i;
        break;
      }
    }

    const currentEvents = visibleTraceEvents.slice(cutoffIdx + 1);
    return currentEvents.flatMap(traceEventToSections);
  }, [traceEventToSections, visibleTraceEvents, streamRounds, streamTraceSections]);

  const currentTraceActive = useMemo(() => {
    if (!isStreaming) return false;
    const lastVisibleEvent = visibleTraceEvents[visibleTraceEvents.length - 1];
    if (!lastVisibleEvent) {
      return (
        isThinking ||
        thinkingText.trim().length > 0 ||
        toolCalls.some((toolCall) => toolCall.status === "running")
      );
    }
    return lastVisibleEvent.kind !== "reply";
  }, [isStreaming, isThinking, thinkingText, toolCalls, visibleTraceEvents]);

  const liveTraceTimeline = useMemo<LiveTraceTimelineItem[]>(() => {
    const items: LiveTraceTimelineItem[] = [];
    let activeSections: ThinkingSection[] = [];

    const flushThinking = (id: string, streaming = false) => {
      if (activeSections.length === 0) return;
      items.push({
        kind: "thinking",
        id,
        sections: activeSections,
        isStreaming: streaming,
      });
      activeSections = [];
    };

    for (const event of visibleTraceEvents) {
      if (event.kind === "reply") {
        flushThinking(`${event.id}-before-reply`);
        items.push({
          kind: "reply",
          id: event.id,
          content: event.text,
          isStreaming: false,
        });
        continue;
      }

      activeSections = [...activeSections, ...traceEventToSections(event)];
    }

    flushThinking("trace-thinking-tail");

    if (!isStreaming || items.length === 0) return items;

    const lastItem = items[items.length - 1];
    if (lastItem.kind === "reply") {
      items[items.length - 1] = {
        ...lastItem,
        content: streamText.trim().length > 0 ? displayedText : lastItem.content,
        isStreaming: true,
      };
      return items;
    }

    if (currentTraceActive) {
      items[items.length - 1] = {
        ...lastItem,
        isStreaming: true,
      };
    }

    return items;
  }, [
    currentTraceActive,
    displayedText,
    isStreaming,
    streamText,
    traceEventToSections,
    visibleTraceEvents,
  ]);

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

          return (
            <div key={msg.id}>
              {traceGroup?.type === "anchor" && (
                <>{traceGroup.nodes}</>
              )}

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
            layout={!shouldReduceMotion}
            transition={shouldReduceMotion ? INSTANT_TRANSITION : undefined}
          >
            {item.kind === "thinking"
              ? renderThinkingTraceNode(item.id, item.sections, item.isStreaming)
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
          const roundSections = buildRoundSections(round);
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
                renderThinkingTraceNode(
                  `round-thinking-${round.id}`,
                  roundSections,
                  false,
                )}
            </Fragment>
          );
        })}

      {/* ── Current in-progress thinking (not yet in a round) ──────── */}
      {!shouldRenderLiveTraceTimeline && currentThinkingSections.length > 0 && (
        <motion.div
          initial={shouldReduceMotion || isStreaming ? false : { opacity: 0 }}
          animate={{ opacity: 1 }}
          layout={!shouldReduceMotion}
          transition={shouldReduceMotion ? INSTANT_TRANSITION : undefined}
          className="flex justify-start mb-3"
        >
          <div className="w-full max-w-[min(100%,72rem)]">
            <ThinkingBlock
              content=""
              sections={currentThinkingSections}
              isStreaming={currentTraceActive}
              defaultExpanded
              collapseOnFinish={false}
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
