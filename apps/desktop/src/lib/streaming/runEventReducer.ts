import type {
  AgentRunEvent,
  ToolRenderKind,
  ToolRunItem,
  ToolRunStatus,
} from '../../types/conversation';
import {
  appendThinkingTraceEvent,
  applyStreamBlockDelta,
} from './blockProjection';
import {
  applyApprovalRequestedEvent,
  applyApprovalResolvedEvent,
  applyAutoCompactedEvent,
  applyConnectionStateEvent,
  applyDoneEvent,
  applyErrorEvent,
  applyStatusEvent,
  applyUsageUpdateEvent,
} from './liveProjection';
import {
  clearToolPreparingTimer,
  clearToolPreparingTimers,
  type InternalStreamState,
} from './state';
import {
  appendStatusTraceEvent,
  applyStreamResetProjection,
} from './terminalProjection';
import {
  applyToolRunEvent,
  type ToolPreparingPayload,
} from './toolProjection';
import { clearStreamWatchdog } from './watchdog';

type PayloadRecord = Record<string, unknown>;

export interface RunEventReducerCallbacks {
  scheduleToolPreparing(payload: ToolPreparingPayload): void;
}

function asRecord(value: unknown): PayloadRecord {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as PayloadRecord
    : {};
}

function optionalRecord(value: unknown): PayloadRecord | undefined {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as PayloadRecord
    : undefined;
}

function stringValue(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined;
}

function numberValue(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

function toolRun(payload: PayloadRecord): ToolRunItem | null {
  const run = asRecord(payload.run);
  return typeof run.callId === 'string' && typeof run.toolName === 'string'
    ? run as unknown as ToolRunItem
    : null;
}

function legacyToolRenderKind(toolName: string): ToolRenderKind {
  if (toolName === 'run_shell') return 'commandExecution';
  if (toolName === 'create_file' || toolName === 'edit_file' || toolName === 'multi_edit' || toolName === 'write_note') {
    return 'fileChange';
  }
  if (toolName.includes('search') || toolName === 'fetch_url' || toolName === 'retrieve_evidence') return 'search';
  if (toolName.includes('subagent')) return 'subagent';
  if (toolName.includes('image')) return 'image';
  if (toolName === 'update_plan') return 'plan';
  if (toolName === 'record_verification') return 'verification';
  if (toolName.startsWith('mcp_')) return 'mcp';
  return 'generic';
}

function legacyToolStatus(runEvent: AgentRunEvent, payload: PayloadRecord): ToolRunStatus {
  if (runEvent.kind === 'toolPreparing') return 'preparing';
  if (runEvent.kind === 'toolStarted' || runEvent.kind === 'toolProgress') {
    return runEvent.status === 'approval_pending' ? 'approvalPending' : 'running';
  }
  if (payload.isError === true || runEvent.status === 'failed') return 'failed';
  if (runEvent.status === 'declined') return 'declined';
  if (runEvent.status === 'cancelled') return 'cancelled';
  if (runEvent.status === 'timed_out') return 'timedOut';
  return 'completed';
}

/**
 * Protocol-v2 databases may contain the retired ToolCall payload under a
 * canonical tool RunEvent kind. Normalize it once at the reducer seam so live
 * and durable projections both consume one ToolRunItem shape.
 */
function legacyToolRun(runEvent: AgentRunEvent, payload: PayloadRecord): ToolRunItem | null {
  const callId = stringValue(payload.callId)?.trim();
  const persistedToolName = stringValue(payload.toolName)?.trim();
  // Historical ToolCallProgress payloads carried only callId + note, while
  // their RunEvent label was the note itself. Leave the name empty so an
  // existing card keeps its identity instead of being renamed to "reading".
  const toolName = persistedToolName
    || (runEvent.kind === 'toolProgress' ? '' : runEvent.label.trim());
  if (!callId || (!toolName && runEvent.kind !== 'toolProgress')) return null;
  const renderKind = legacyToolRenderKind(toolName || 'unknown_tool');
  return {
    callId,
    toolName,
    owner: {
      id: 'nexa.runtime',
      name: 'Nexa Runtime',
      capability: toolName || 'unknown_tool',
      description: 'Compatibility projection for a persisted tool lifecycle event.',
    },
    providerExecuted: false,
    status: legacyToolStatus(runEvent, payload),
    arguments: stringValue(payload.arguments),
    renderKind,
    capabilities: {
      inputStreaming: 'none',
      renderKind,
      readOnly: false,
      destructive: false,
      concurrencySafe: false,
      interruptBehavior: 'cancel',
      resourceKeys: [],
    },
    content: stringValue(payload.content),
    isError: typeof payload.isError === 'boolean' ? payload.isError : undefined,
    artifacts: optionalRecord(payload.artifacts) as ToolRunItem['artifacts'],
    progressNote: stringValue(payload.note),
  };
}

function presentationTone(payload: PayloadRecord): 'muted' | 'success' | 'error' {
  return payload.tone === 'success' || payload.tone === 'error' ? payload.tone : 'muted';
}

/** Apply the versioned Run Event protocol without translating it into the retired event envelope. */
export function applyAgentRunEvent(
  state: InternalStreamState,
  runEvent: AgentRunEvent,
  callbacks: RunEventReducerCallbacks,
): void {
  const payload = asRecord(runEvent.payload);

  switch (runEvent.kind) {
    case 'outputDelta': {
      const channel = payload.channel === 'thinking' ? 'thinking' : 'answer';
      applyStreamBlockDelta(
        state,
        channel,
        stringValue(payload.blockId) ?? '',
        numberValue(payload.offset) ?? 0,
        stringValue(payload.delta) ?? '',
      );
      return;
    }

    case 'streamReset':
      clearToolPreparingTimers(state);
      applyStreamResetProjection(
        state,
        stringValue(payload.reason) ?? runEvent.label,
        { clearTools: true },
      );
      return;

    case 'thinking': {
      const content = stringValue(payload.content) ?? '';
      if (!content) return;
      state.isThinking = true;
      state.thinkingText += content;
      appendThinkingTraceEvent(state, content);
      return;
    }

    case 'status': {
      applyStatusEvent(
        state,
        stringValue(payload.content) ?? runEvent.label,
        presentationTone(payload),
        runEvent.visibility ?? 'user',
        runEvent.displayKind ?? 'status',
      );
      return;
    }

    case 'planUpdated':
      // The plan has a dedicated capsule/panel. Do not duplicate controller
      // summaries inside the user-facing Thinking timeline.
      return;

    case 'recoveryAttempt': {
      const connection = asRecord(payload.state);
      if (typeof connection.state === 'string') {
        applyConnectionStateEvent(state, connection);
        return;
      }
      appendStatusTraceEvent(
        state,
        stringValue(payload.reason) ?? runEvent.label,
        'muted',
        runEvent.visibility ?? 'user',
        runEvent.displayKind ?? 'recovery',
      );
      return;
    }

    case 'toolPreparing': {
      const run = toolRun(payload) ?? legacyToolRun(runEvent, payload);
      if (run) {
        clearToolPreparingTimer(state, run.callId.trim());
        applyToolRunEvent(state, run);
        return;
      }
      const callId = stringValue(payload.callId)?.trim();
      const toolName = stringValue(payload.toolName)?.trim();
      if (callId && toolName) {
        callbacks.scheduleToolPreparing({
          callId,
          toolName,
          argsBytes: numberValue(payload.argsBytes) ?? 0,
        });
      }
      return;
    }

    case 'toolStarted':
    case 'toolProgress':
    case 'toolCompleted': {
      const run = toolRun(payload) ?? legacyToolRun(runEvent, payload);
      if (run) {
        clearToolPreparingTimer(state, run.callId.trim());
        applyToolRunEvent(state, run);
        return;
      }
      return;
    }

    case 'approvalRequested': {
      applyApprovalRequestedEvent(state, payload.request);
      return;
    }

    case 'approvalResolved': {
      applyApprovalResolvedEvent(state, stringValue(payload.requestId));
      return;
    }

    case 'usageUpdated': {
      applyUsageUpdateEvent(
        state,
        payload.usageTotal,
        payload.lastPromptTokens,
        payload.contextBreakdown,
      );
      return;
    }

    case 'autoCompacted': {
      applyAutoCompactedEvent(state, stringValue(payload.summary) ?? '');
      return;
    }

    case 'done': {
      clearStreamWatchdog(state);
      clearToolPreparingTimers(state);
      applyDoneEvent(state, {
        status: runEvent.status ?? null,
        message: payload.message,
        messageTruncated: payload.messageTruncated,
        usageTotal: payload.usageTotal,
        lastPromptTokens: payload.lastPromptTokens,
        contextBreakdown: payload.contextBreakdown,
        cached: payload.cached,
        finishReason: payload.finishReason,
      });
      return;
    }

    case 'error': {
      clearStreamWatchdog(state);
      clearToolPreparingTimers(state);
      applyErrorEvent(
        state,
        stringValue(payload.message) ?? runEvent.label,
        runEvent.status ?? null,
      );
    }
  }
}
