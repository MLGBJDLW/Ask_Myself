import type {
  AgentRunEvent,
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

      // The public Run Event protocol exposes one typed ToolRun lifecycle.
      // Legacy ToolCall fragments are provider/core assembly details and are
      // deliberately ignored at this seam.
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
