import type { AgentFrontendEvent } from '../../types';
import type {
  AgentRunEvent,
  ApprovalRequest,
  ToolRunItem,
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
  applyToolCallProgressEvent,
  applyToolCallResultEvent,
  applyToolCallStartEvent,
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

function stringValue(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined;
}

function numberValue(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

function projectionEvent(
  runEvent: AgentRunEvent,
  payload: PayloadRecord,
  fields: Partial<AgentFrontendEvent> = {},
): AgentFrontendEvent & PayloadRecord {
  return {
    conversationId: '',
    runEvent,
    ...payload,
    ...fields,
  } as AgentFrontendEvent & PayloadRecord;
}

function toolRun(payload: PayloadRecord): ToolRunItem | null {
  const run = asRecord(payload.run);
  return typeof run.callId === 'string' && typeof run.toolName === 'string'
    ? run as unknown as ToolRunItem
    : null;
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
      const event = projectionEvent(runEvent, payload, {
        content: stringValue(payload.content) ?? runEvent.label,
        tone: presentationTone(payload),
      });
      applyStatusEvent(state, event, event);
      return;
    }

    case 'planUpdated':
      // The plan has a dedicated capsule/panel. Do not duplicate controller
      // summaries inside the user-facing Thinking timeline.
      return;

    case 'recoveryAttempt': {
      const connection = asRecord(payload.state);
      if (typeof connection.state === 'string') {
        const event = projectionEvent(runEvent, payload, {
          state: connection as unknown as AgentFrontendEvent['state'],
        });
        applyConnectionStateEvent(state, event, event, runEvent.label);
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
      const run = toolRun(payload);
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
      const run = toolRun(payload);
      if (run) {
        clearToolPreparingTimer(state, run.callId.trim());
        applyToolRunEvent(state, run);
        return;
      }

      const event = projectionEvent(runEvent, payload, {
        callId: stringValue(payload.callId),
        toolName: stringValue(payload.toolName),
        arguments: stringValue(payload.arguments),
        content: stringValue(payload.content),
        isError: typeof payload.isError === 'boolean' ? payload.isError : undefined,
      });
      if (runEvent.kind === 'toolStarted') applyToolCallStartEvent(state, event, event);
      if (runEvent.kind === 'toolProgress') applyToolCallProgressEvent(state, event, event);
      if (runEvent.kind === 'toolCompleted') applyToolCallResultEvent(state, event, event);
      return;
    }

    case 'approvalRequested': {
      const event = projectionEvent(runEvent, payload, {
        request: payload.request as ApprovalRequest | undefined,
      });
      applyApprovalRequestedEvent(state, event, event);
      return;
    }

    case 'approvalResolved': {
      const event = projectionEvent(runEvent, payload, {
        requestId: stringValue(payload.requestId),
      });
      applyApprovalResolvedEvent(state, event, event);
      return;
    }

    case 'usageUpdated': {
      const event = projectionEvent(runEvent, payload, {
        usageTotal: payload.usageTotal as AgentFrontendEvent['usageTotal'],
        contextBreakdown: payload.contextBreakdown as AgentFrontendEvent['contextBreakdown'],
      });
      applyUsageUpdateEvent(state, event, event);
      return;
    }

    case 'autoCompacted': {
      const event = projectionEvent(runEvent, payload, {
        summary: stringValue(payload.summary),
      });
      applyAutoCompactedEvent(state, event, event);
      return;
    }

    case 'done': {
      clearStreamWatchdog(state);
      clearToolPreparingTimers(state);
      const event = projectionEvent(runEvent, payload, {
        message: payload.message as AgentFrontendEvent['message'],
        usageTotal: payload.usageTotal as AgentFrontendEvent['usageTotal'],
        contextBreakdown: payload.contextBreakdown as AgentFrontendEvent['contextBreakdown'],
      });
      applyDoneEvent(state, event, event);
      return;
    }

    case 'error': {
      clearStreamWatchdog(state);
      clearToolPreparingTimers(state);
      const event = projectionEvent(runEvent, payload, {
        message: stringValue(payload.message) ?? runEvent.label,
      });
      applyErrorEvent(state, event, event);
    }
  }
}
