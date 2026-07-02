import type {
  ConversationTurn,
  ToolRenderKind,
} from '../../types/conversation';
import { appTimeMs } from '../dateTime';
import type { PersistedTraceItem, PersistedTraceSkillRef } from './persistedTrace';
import type {
  StreamRoundEvent,
  ToolCallEvent,
  TraceEvent,
} from './protocol';
import {
  isPendingToolCallStatus,
  isUnsuccessfulToolCallStatus,
} from './toolStatus';

export type TraceTone = 'muted' | 'success' | 'error';

export interface TimelineSkillRef extends PersistedTraceSkillRef {
  label: string;
  key: string;
  description?: string;
  shortDescription?: string;
  implicit?: boolean;
  activated?: boolean;
}

export type TimelineSection =
  | { kind: 'thinking'; id: string; text: string }
  | { kind: 'status'; id: string; text: string; tone?: TraceTone }
  | { kind: 'steering'; id: string; text: string }
  | { kind: 'tool'; id: string; toolCall: ToolCallEvent; trace: boolean }
  | { kind: 'reply'; id: string; text: string };

export type LiveTraceTimelineItem =
  | {
      kind: 'thinking';
      id: string;
      sections: TimelineSection[];
      isStreaming: boolean;
    }
  | { kind: 'reply'; id: string; content: string; isStreaming: boolean };

const INTERNAL_TRACE_TOOLS = new Set([
  'prepare_document_tools',
  'tool_search',
]);

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function skillRefLabel(
  id: unknown,
  name: unknown,
  displayName: unknown,
): { label: string; key: string } | null {
  let label: string | null = null;
  if (typeof displayName === 'string' && displayName.trim()) {
    label = displayName.trim();
  }

  if (!label && typeof name === 'string' && name.trim()) {
    label = name.trim();
  }

  if (!label && typeof id === 'string' && id.trim()) {
    label = id.trim();
  }

  if (!label) return null;

  const key = typeof id === 'string' && id.trim()
    ? id.trim().toLowerCase()
    : typeof name === 'string' && name.trim()
      ? name.trim().toLowerCase()
      : label.toLowerCase();

  return { label, key };
}

function skillActivationRefFromArtifacts(
  artifacts: unknown,
): TimelineSkillRef | null {
  const record = asRecord(artifacts);
  if (
    !record ||
    (record.kind !== 'skillActivation' && record.kind !== 'skill')
  ) {
    return null;
  }
  const skill = asRecord(record.skill);
  if (!skill) return null;

  const id = skill.id;
  const name = skill.name;
  const interfaceMetadata = asRecord(skill.interface);
  const displayName = interfaceMetadata?.displayName;
  const ref = skillRefLabel(id, name, displayName);
  if (!ref) return null;
  const policy = asRecord(skill.policy);
  return {
    id: typeof id === 'string' ? id : undefined,
    name: typeof name === 'string' ? name : undefined,
    displayName: typeof displayName === 'string' ? displayName : undefined,
    builtin: typeof skill.builtin === 'boolean' ? skill.builtin : undefined,
    sourcePath: typeof skill.sourcePath === 'string' ? skill.sourcePath : undefined,
    description: typeof skill.description === 'string' ? skill.description : undefined,
    shortDescription:
      typeof interfaceMetadata?.shortDescription === 'string'
        ? interfaceMetadata.shortDescription
        : undefined,
    implicit:
      typeof policy?.allowImplicitInvocation === 'boolean'
        ? policy.allowImplicitInvocation
        : undefined,
    ...ref,
    activated: true,
  };
}

function skillActivationNameFromArtifacts(artifacts: unknown): string | null {
  return skillActivationRefFromArtifacts(artifacts)?.label ?? null;
}

function loadedSkillRefFromSelection(skill: PersistedTraceSkillRef): TimelineSkillRef | null {
  if (skill.activated !== true) return null;
  const ref = skillRefLabel(skill.id, skill.name, skill.displayName);
  if (!ref) return null;

  return {
    ...skill,
    ...ref,
    activated: true,
  };
}

function isSuccessfulSkillActivation(toolCall: ToolCallEvent): boolean {
  return (
    normalizeToolName(toolCall.toolName) === 'manage_skill' &&
    toolCall.status === 'done' &&
    toolCall.isError !== true &&
    !isUnsuccessfulToolCallStatus(toolCall.status) &&
    Boolean(skillActivationNameFromArtifacts(toolCall.artifacts))
  );
}

export function skillNamesFromTraceItems(
  items: PersistedTraceItem[] | null | undefined,
): string[] {
  return skillRefsFromTraceItems(items).map((skill) => skill.label);
}

export function skillRefsFromTraceItems(
  items: PersistedTraceItem[] | null | undefined,
): TimelineSkillRef[] {
  const skills: TimelineSkillRef[] = [];
  const indexByKey = new Map<string, number>();

  const addRef = (ref: TimelineSkillRef | null) => {
    if (!ref) return;
    const existingIndex = indexByKey.get(ref.key);
    if (existingIndex != null) {
      const existing = skills[existingIndex];
      skills[existingIndex] = {
        ...existing,
        id: ref.id ?? existing.id,
        name: ref.name ?? existing.name,
        displayName: ref.displayName ?? existing.displayName,
        builtin: ref.builtin ?? existing.builtin,
        sourcePath: ref.sourcePath ?? existing.sourcePath,
        description: ref.description ?? existing.description,
        shortDescription: ref.shortDescription ?? existing.shortDescription,
        implicit: ref.implicit ?? existing.implicit,
        activated: Boolean(existing.activated || ref.activated),
      };
      return;
    }
    indexByKey.set(ref.key, skills.length);
    skills.push(ref);
  };

  for (const item of items ?? []) {
    if (item.kind === 'skillSelection') {
      for (const skill of item.skills) {
        addRef(loadedSkillRefFromSelection(skill));
      }
      continue;
    }

    if (item.kind === 'tool' && isSuccessfulSkillActivation(item.toolCall)) {
      addRef(skillActivationRefFromArtifacts(item.toolCall.artifacts));
    }
  }

  return skills;
}

export function formatSkillSummary(names: string[]): string {
  if (names.length === 0) return 'Skills: none';
  return `Skills: ${names.join(', ')}`;
}

export function normalizeThinking(content: string): string {
  return content.replace(/\r\n/g, '\n').trim();
}

export function compactThinkingText(content: string): string {
  return normalizeThinking(content).replace(/\s+/g, ' ');
}


function mergeAdjacentThinkingSections(sections: TimelineSection[]): TimelineSection[] {
  const merged: TimelineSection[] = [];

  for (const section of sections) {
    const last = merged[merged.length - 1];
    if (last?.kind === 'thinking' && section.kind === 'thinking') {
      const separator = last.text.endsWith('\n') || section.text.startsWith('\n') ? '' : '\n';
      merged[merged.length - 1] = {
        ...last,
        text: `${last.text}${separator}${section.text}`,
      };
      continue;
    }
    merged.push(section);
  }

  return merged;
}

export function hasRenderableTimelineSection(section: TimelineSection): boolean {
  if (section.kind === 'thinking') {
    return compactThinkingText(section.text).length > 0;
  }
  if (section.kind === 'reply') {
    return section.text.trim().length > 0;
  }
  return true;
}

export function hasRenderableTimelineSections(sections: TimelineSection[]): boolean {
  return sections.some(hasRenderableTimelineSection);
}

export function isLowSignalTimelineSection(section: TimelineSection): boolean {
  if (section.kind !== 'thinking') return false;
  const text = compactThinkingText(section.text);
  if (!text) return true;
  if (/^[\p{P}\p{S}\s]+$/u.test(text)) return true;

  const hasCjk = /[\u3400-\u9fff]/.test(text);
  const wordCount = text.split(/\s+/).filter(Boolean).length;
  return !hasCjk && text.length <= 12 && wordCount <= 2;
}

export function formatRouteKind(routeKind: string): string {
  return routeKind
    .replace(/([a-z])([A-Z])/g, '$1 $2')
    .replace(/^./, (char) => char.toUpperCase());
}

export function shouldHideRouteKind(routeKind: string | null | undefined): boolean {
  const normalized = (routeKind ?? '').replace(/\s+/g, '').toLowerCase();
  return normalized === 'directresponse' || normalized === 'fileoperation';
}

export function shouldHideTraceStatus(text: string | null | undefined): boolean {
  const compact = (text ?? '').replace(/\s+/g, ' ').trim().toLowerCase();
  const normalized = compact.replace(/\s+/g, '');
  return (
    normalized === 'routeselected:directresponse' ||
    normalized === 'route:directresponse' ||
    normalized === 'routeselected:fileoperation' ||
    normalized === 'route:fileoperation' ||
    compact === 'loading tools and mcp servers' ||
    compact === 'task queued' ||
    compact === 'queued' ||
    compact === '排队' ||
    compact === '排隊' ||
    compact === '排队中' ||
    compact === '排隊中' ||
    compact === '任务已排队' ||
    compact === '任務已排隊' ||
    compact === '已使用上下文' ||
    /^subagent (judge )?queued:/.test(compact) ||
    /^status:\s*(success|cached|running)(\s*.+)?$/.test(compact)
  );
}

export function steeringTextFromTraceStatus(text: string | null | undefined): string | null {
  const raw = (text ?? '').trim();
  const match = raw.match(/^User steering:\s*(.+)$/i);
  if (!match) return null;
  const steeringText = match[1].trim();
  return steeringText.length > 0 ? steeringText : null;
}

function normalizeToolName(toolName: string | null | undefined): string {
  return (toolName ?? '').trim().toLowerCase();
}

function isBoardOnlyTimelineToolCall(
  toolName: string | null | undefined,
  renderKind?: ToolRenderKind,
): boolean {
  return normalizeToolName(toolName) === 'update_plan' || renderKind === 'plan';
}

function isGeneratedImageArtifact(artifacts: unknown): boolean {
  return Boolean(
    artifacts &&
    typeof artifacts === 'object' &&
    !Array.isArray(artifacts) &&
    (artifacts as Record<string, unknown>).kind === 'generatedImage',
  );
}

function isGeneratedImageToolCall(toolCall: ToolCallEvent): boolean {
  return (
    normalizeToolName(toolCall.toolName) === 'generate_image' ||
    toolCall.renderKind === 'image'
  );
}

export function shouldRenderTraceToolCall(
  toolName: string | null | undefined,
  renderKind?: ToolRenderKind,
  status?: string | null,
  isError?: boolean,
): boolean {
  if (isBoardOnlyTimelineToolCall(toolName, renderKind)) return false;
  if (isError || isUnsuccessfulToolCallStatus(status)) return true;

  const normalizedToolName = normalizeToolName(toolName);
  if (INTERNAL_TRACE_TOOLS.has(normalizedToolName)) return false;

  return true;
}

export function formatTurnStatus(status: string): string {
  switch (status) {
    case 'success':
      return 'Success';
    case 'cached':
      return 'Cached';
    case 'cancelled':
      return 'Cancelled';
    case 'max_iterations':
      return 'Max iterations';
    case 'error':
      return 'Error';
    case 'running':
    default:
      return 'Running';
  }
}

export function formatTurnDuration(turn: ConversationTurn): string | null {
  if (!turn.finishedAt) return null;
  const startedAt = appTimeMs(turn.createdAt);
  const finishedAt = appTimeMs(turn.finishedAt);
  if (
    Number.isNaN(startedAt) ||
    Number.isNaN(finishedAt) ||
    finishedAt < startedAt
  ) {
    return null;
  }
  const seconds = Math.max(0, Math.round((finishedAt - startedAt) / 1000));
  return `${seconds}s`;
}

export function turnLifecycleTimelineSections(input: {
  turn: ConversationTurn;
  routeKind?: string | null;
  traceItems?: PersistedTraceItem[] | null;
  formatSkillsSummary?: (names: string[]) => string;
}): TimelineSection[] {
  const sections: TimelineSection[] = [];
  const { turn, routeKind, traceItems, formatSkillsSummary } = input;

  if (routeKind && !shouldHideRouteKind(routeKind)) {
    sections.push({
      kind: 'status',
      id: `turn-route-${turn.id}`,
      text: `Route: ${formatRouteKind(routeKind)}`,
      tone: 'muted',
    });
  }

  if (traceItems && traceItems.length > 0) {
    const skills = skillNamesFromTraceItems(traceItems);
    sections.push({
      kind: 'status',
      id: `turn-skills-${turn.id}`,
      text: formatSkillsSummary
        ? formatSkillsSummary(skills)
        : formatSkillSummary(skills),
      tone: skills.length > 0 ? 'success' : 'muted',
    });
  }

  if (
    turn.status === 'error' ||
    turn.status === 'cancelled' ||
    turn.status === 'max_iterations'
  ) {
    const duration = formatTurnDuration(turn);
    sections.push({
      kind: 'status',
      id: `turn-status-${turn.id}`,
      text: `Status: ${formatTurnStatus(turn.status)}${duration ? ` · ${duration}` : ''}`,
      tone: 'error',
    });
  }

  return sections;
}

export function toolCallToTimelineSection(input: {
  id: string;
  toolCall: ToolCallEvent;
  trace: boolean;
}): TimelineSection[] {
  const { id, toolCall, trace } = input;
  if (
    isGeneratedImageToolCall(toolCall) &&
    !isPendingToolCallStatus(toolCall.status) &&
    !toolCall.isError &&
    isGeneratedImageArtifact(toolCall.artifacts)
  ) {
    return [];
  }

  if (
    !shouldRenderTraceToolCall(
      toolCall.toolName,
      toolCall.renderKind,
      toolCall.status,
      toolCall.isError,
    )
  ) {
    return [];
  }

  return [{ kind: 'tool', id, toolCall, trace }];
}

export function persistedTraceItemToTimelineSections(input: {
  item: PersistedTraceItem;
  id: string;
  trace: boolean;
}): TimelineSection[] {
  const { item, id, trace } = input;
  switch (item.kind) {
    case 'status':
      {
        const steeringText = steeringTextFromTraceStatus(item.text);
        if (steeringText) {
          return [{ kind: 'steering', id, text: steeringText }];
        }
      }
      if (shouldHideTraceStatus(item.text)) return [];
      return [{
        kind: 'status',
        id,
        text: item.text,
        tone: item.tone,
      }];
    case 'thinking':
      return item.text.trim().length > 0
        ? [{ kind: 'thinking', id, text: item.text }]
        : [];
    case 'reply':
      return item.text.trim().length > 0
        ? [{ kind: 'reply', id, text: item.text }]
        : [];
    case 'tool':
      return toolCallToTimelineSection({
        id,
        toolCall: item.toolCall,
        trace,
      });
    default:
      return [];
  }
}

export function persistedTraceItemsToTimelineSections(input: {
  items: PersistedTraceItem[] | null | undefined;
  idPrefix: string;
  trace: boolean;
}): TimelineSection[] {
  const { items, idPrefix, trace } = input;
  return (items ?? []).flatMap((item, index) =>
    persistedTraceItemToTimelineSections({
      item,
      id: `${idPrefix}-${item.kind}-${index}`,
      trace,
    }),
  );
}

export function visibleTraceEventsForTimeline(traceEvents: TraceEvent[]): TraceEvent[] {
  return traceEvents.filter(
    (event) => !(
      event.kind === 'status' &&
      !steeringTextFromTraceStatus(event.text) &&
      shouldHideTraceStatus(event.text)
    ),
  );
}

export function traceEventToTimelineSections(event: TraceEvent): TimelineSection[] {
  if (event.kind === 'reply') return [];
  if (event.kind === 'thinking') {
    return event.text.trim().length > 0
      ? [{ kind: 'thinking', id: event.id, text: event.text }]
      : [];
  }
  if (event.kind === 'tool') {
    return toolCallToTimelineSection({
      id: event.id,
      toolCall: event.toolCall,
      trace: true,
    });
  }
  {
    const steeringText = steeringTextFromTraceStatus(event.text);
    if (steeringText) return [{ kind: 'steering', id: event.id, text: steeringText }];
  }
  return [{
    kind: 'status',
    id: event.id,
    text: event.text,
    tone: event.tone,
  }];
}

export function buildRoundTimelineSections(round: StreamRoundEvent): TimelineSection[] {
  const sections: TimelineSection[] = [];
  if (round.thinking?.trim()) {
    sections.push({
      kind: 'thinking',
      id: `round-thinking-${round.id}`,
      text: round.thinking,
    });
  }

  for (const toolCall of round.toolCalls) {
    sections.push(
      ...toolCallToTimelineSection({
        id: `round-tool-${round.id}-${toolCall.callId}`,
        toolCall,
        trace: true,
      }),
    );
  }

  return sections;
}

export function buildCurrentTimelineSections(input: {
  visibleTraceEvents: TraceEvent[];
  streamRounds: StreamRoundEvent[];
}): TimelineSection[] {
  const { visibleTraceEvents, streamRounds } = input;
  if (streamRounds.length === 0) {
    return visibleTraceEvents.flatMap(traceEventToTimelineSections);
  }

  return traceEventsAfterStreamRounds(visibleTraceEvents, streamRounds)
    .flatMap(traceEventToTimelineSections);
}

export function traceEventsAfterStreamRounds(
  visibleTraceEvents: TraceEvent[],
  streamRounds: StreamRoundEvent[],
): TraceEvent[] {
  if (streamRounds.length === 0) return visibleTraceEvents;

  const roundCallIds = new Set<string>();
  for (const round of streamRounds) {
    for (const toolCall of round.toolCalls) {
      roundCallIds.add(toolCall.callId);
    }
  }
  if (roundCallIds.size === 0) return visibleTraceEvents;

  let cutoffIdx = -1;
  for (let i = visibleTraceEvents.length - 1; i >= 0; i -= 1) {
    const event = visibleTraceEvents[i];
    if (event.kind === 'tool' && roundCallIds.has(event.toolCall.callId)) {
      cutoffIdx = i;
      break;
    }
  }

  return visibleTraceEvents.slice(cutoffIdx + 1);
}

export function isCurrentTraceActive(input: {
  isStreaming: boolean;
  isThinking: boolean;
  thinkingText: string;
  toolCalls: ToolCallEvent[];
  visibleTraceEvents: TraceEvent[];
}): boolean {
  const {
    isStreaming,
    isThinking,
    thinkingText,
    toolCalls,
    visibleTraceEvents,
  } = input;

  if (!isStreaming) return false;
  const lastVisibleEvent = visibleTraceEvents[visibleTraceEvents.length - 1];
  if (!lastVisibleEvent) {
    return (
      isThinking ||
      thinkingText.trim().length > 0 ||
      toolCalls.some((toolCall) => isPendingToolCallStatus(toolCall.status))
    );
  }
  return lastVisibleEvent.kind !== 'reply';
}

export function buildLiveTraceTimeline(input: {
  visibleTraceEvents: TraceEvent[];
  isStreaming: boolean;
  currentTraceActive: boolean;
  streamText: string;
  displayedText: string;
}): LiveTraceTimelineItem[] {
  const {
    visibleTraceEvents,
    isStreaming,
    currentTraceActive,
    streamText,
    displayedText,
  } = input;
  const items: LiveTraceTimelineItem[] = [];
  let activeSections: TimelineSection[] = [];

  const flushThinking = (id: string, streaming = false) => {
    const renderableSections = mergeAdjacentThinkingSections(activeSections).filter(
      (section) =>
        hasRenderableTimelineSection(section) &&
        !isLowSignalTimelineSection(section),
    );
    activeSections = [];
    if (renderableSections.length === 0) return;
    items.push({
      kind: 'thinking',
      id,
      sections: renderableSections,
      isStreaming: streaming,
    });
  };

  const appendReply = (
    id: string,
    content: string,
    streaming = false,
    options: { mergeAdjacent?: boolean } = {},
  ) => {
    if (!content) return;
    const mergeAdjacent = options.mergeAdjacent ?? true;
    const lastItem = items[items.length - 1];
    if (mergeAdjacent && lastItem?.kind === 'reply') {
      items[items.length - 1] = {
        ...lastItem,
        content: lastItem.content + content,
        isStreaming: lastItem.isStreaming || streaming,
      };
      return;
    }
    items.push({
      kind: 'reply',
      id,
      content,
      isStreaming: streaming,
    });
  };

  const activeStreamText = streamText.trim().length > 0 ? streamText : '';
  const lastReplyEventIndex = activeStreamText
    ? (() => {
        for (let index = visibleTraceEvents.length - 1; index >= 0; index -= 1) {
          if (visibleTraceEvents[index].kind === 'reply') return index;
        }
        return -1;
      })()
    : -1;
  let renderedActiveStreamReply = false;

  visibleTraceEvents.forEach((event, index) => {
    if (event.kind === 'reply') {
      flushThinking(`${event.id}-before-reply`);
      const isActiveStreamReply =
        isStreaming &&
        index === lastReplyEventIndex &&
        activeStreamText.length > 0 &&
        event.text === streamText;
      appendReply(
        event.id,
        isActiveStreamReply ? displayedText : event.text,
        isActiveStreamReply,
      );
      renderedActiveStreamReply = renderedActiveStreamReply || isActiveStreamReply;
      return;
    }

    activeSections = [...activeSections, ...traceEventToTimelineSections(event)];
  });

  flushThinking('trace-thinking-tail');

  if (!isStreaming) return items;

  if (activeStreamText && !renderedActiveStreamReply) {
    appendReply('live-stream-reply-tail', displayedText, true, { mergeAdjacent: false });
  }

  if (items.length === 0) return items;

  const lastItem = items[items.length - 1];
  if (currentTraceActive && lastItem.kind !== 'reply') {
    items[items.length - 1] = {
      ...lastItem,
      isStreaming: true,
    };
  }

  return items;
}
