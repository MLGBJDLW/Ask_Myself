import type {
  ArtifactPayload,
  ConversationMessage,
  ConversationTurn,
  ToolPluginInfo,
  ToolRenderKind,
  ToolRunCapabilities,
} from '../../types/conversation';
import type { ToolCallEvent } from './protocol';
import {
  defaultArgsStatusForToolCall,
  normalizePersistedToolCallStatus,
} from './toolStatus';

export type PersistedTraceItem =
  | { kind: 'thinking'; text: string }
  | { kind: 'reply'; text: string }
  | { kind: 'tool'; toolCall: ToolCallEvent }
  | { kind: 'skillSelection'; skills: PersistedTraceSkillRef[] }
  | { kind: 'status'; text: string; tone?: 'muted' | 'success' | 'error' };

export interface PersistedTraceSkillRef {
  id?: string;
  name?: string;
  displayName?: string;
  builtin?: boolean;
  sourcePath?: string;
}

export interface TurnTraceProjection {
  routeKind?: string;
  items: PersistedTraceItem[];
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function traceTone(value: unknown): 'muted' | 'success' | 'error' {
  return value === 'success' || value === 'error' ? value : 'muted';
}

function persistedToolCallFromRecord(toolCall: Record<string, unknown>): ToolCallEvent | null {
  if (
    typeof toolCall.callId !== 'string'
    || typeof toolCall.toolName !== 'string'
  ) {
    return null;
  }

  const argumentsText = typeof toolCall.arguments === 'string' ? toolCall.arguments : '';
  const isError = typeof toolCall.isError === 'boolean' ? toolCall.isError : undefined;
  const status = normalizePersistedToolCallStatus(toolCall.status, isError);
  const argsStatus = (
    toolCall.argsStatus === 'pending'
    || toolCall.argsStatus === 'streaming'
    || toolCall.argsStatus === 'ready'
    || toolCall.argsStatus === 'done'
    || toolCall.argsStatus === 'error'
  )
    ? toolCall.argsStatus
    : defaultArgsStatusForToolCall(status, argumentsText);
  const plugin = asRecord(toolCall.plugin);
  const capabilities = asRecord(toolCall.capabilities);
  const artifacts = asRecord(toolCall.artifacts);

  return {
    callId: toolCall.callId,
    toolName: toolCall.toolName,
    arguments: argumentsText,
    status,
    renderKind:
      typeof toolCall.renderKind === 'string'
        ? toolCall.renderKind as ToolRenderKind
        : undefined,
    plugin: plugin ? plugin as unknown as ToolPluginInfo : undefined,
    capabilities: capabilities ? capabilities as unknown as ToolRunCapabilities : undefined,
    argsStatus,
    argsBytes:
      typeof toolCall.argsBytes === 'number'
        ? toolCall.argsBytes
        : argumentsText.length,
    durationMs:
      typeof toolCall.durationMs === 'number'
        ? toolCall.durationMs
        : undefined,
    content:
      typeof toolCall.content === 'string' ? toolCall.content : undefined,
    isError,
    artifacts: artifacts ? artifacts as ArtifactPayload : undefined,
  };
}

function persistedSkillRefFromRecord(skill: Record<string, unknown>): PersistedTraceSkillRef | null {
  const id = typeof skill.id === 'string' ? skill.id.trim() : '';
  const name = typeof skill.name === 'string' ? skill.name.trim() : '';
  const displayName = typeof skill.displayName === 'string' ? skill.displayName.trim() : '';
  if (!id && !name && !displayName) return null;

  return {
    id: id || undefined,
    name: name || undefined,
    displayName: displayName || undefined,
    builtin: typeof skill.builtin === 'boolean' ? skill.builtin : undefined,
    sourcePath: typeof skill.sourcePath === 'string' ? skill.sourcePath : undefined,
  };
}

export function extractPersistedTraceItems(
  artifacts: ConversationMessage['artifacts'] | unknown,
): PersistedTraceItem[] | null {
  const record = asRecord(artifacts);
  if (!record || record.kind !== 'traceTimeline' || !Array.isArray(record.items)) {
    return null;
  }

  const items: PersistedTraceItem[] = [];
  for (const rawItem of record.items) {
    const item = asRecord(rawItem);
    if (!item) continue;

    if (item.kind === 'thinking' && typeof item.text === 'string') {
      items.push({ kind: 'thinking', text: item.text });
      continue;
    }

    if (item.kind === 'reply' && typeof item.text === 'string') {
      items.push({ kind: 'reply', text: item.text });
      continue;
    }

    if (item.kind === 'status' && typeof item.text === 'string') {
      items.push({
        kind: 'status',
        text: item.text,
        tone: traceTone(item.tone),
      });
      continue;
    }

    if (item.kind === 'tool') {
      const toolCall = asRecord(item.toolCall);
      const projected = toolCall ? persistedToolCallFromRecord(toolCall) : null;
      if (projected) items.push({ kind: 'tool', toolCall: projected });
      continue;
    }

    if (item.kind === 'skillSelection' && Array.isArray(item.skills)) {
      const skills = item.skills
        .map((rawSkill) => asRecord(rawSkill))
        .filter((skill): skill is Record<string, unknown> => Boolean(skill))
        .map(persistedSkillRefFromRecord)
        .filter((skill): skill is PersistedTraceSkillRef => Boolean(skill));
      if (skills.length > 0) items.push({ kind: 'skillSelection', skills });
    }
  }

  return items.length > 0 ? items : null;
}

export function extractTurnTrace(
  trace: ConversationTurn['trace'],
): TurnTraceProjection | null {
  const record = asRecord(trace);
  if (!record || record.kind !== 'turnTrace' || !Array.isArray(record.items)) {
    return null;
  }

  const items = extractPersistedTraceItems({
    kind: 'traceTimeline',
    items: record.items,
  } as ConversationMessage['artifacts']);
  if (!items || items.length === 0) return null;

  return {
    routeKind:
      typeof record.routeKind === 'string' ? record.routeKind : undefined,
    items,
  };
}
