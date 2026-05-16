import type { AgentFrontendEvent } from '../../types';
import {
  applyStreamBlockDeltaEvent,
  applyTextDeltaEvent,
  applyThinkingEvent,
} from './blockProjection';
import type { AgentEventType } from './eventTypes';
import {
  applyApprovalRequestedEvent,
  applyApprovalResolvedEvent,
  applyAutoCompactedEvent,
  applyDoneEvent,
  applyErrorEvent,
  applyStatusEvent,
  applyStreamResetEvent,
  applyTaskRunEvent,
  applyTaskRunUpdatedEvent,
  applyUsageUpdateEvent,
} from './liveProjection';
import {
  clearToolPreparingTimer,
  clearToolPreparingTimers,
  type InternalStreamState,
} from './state';
import {
  applyToolCallArgsDeltaEvent,
  applyToolCallProgressEvent,
  applyToolCallResultEvent,
  applyToolCallStartEvent,
  applyToolRunEvent,
  extractToolPreparingPayload,
  extractToolRunPayload,
  type ToolPreparingPayload,
  toolPreparingPayloadFromRun,
} from './toolProjection';
import { clearStreamWatchdog } from './watchdog';

type RawFrontendEvent = AgentFrontendEvent & Record<string, unknown>;

export interface LiveEventReducerCallbacks {
  scheduleToolPreparing(payload: ToolPreparingPayload): void;
}

export function applyLiveStreamEvent(
  state: InternalStreamState,
  eventType: AgentEventType,
  event: AgentFrontendEvent,
  raw: RawFrontendEvent,
  callbacks: LiveEventReducerCallbacks,
): void {
  switch (eventType) {
    case 'streamBlockDelta': {
      applyStreamBlockDeltaEvent(state, event, raw);
      break;
    }

    case 'thinking': {
      applyThinkingEvent(state, event, raw);
      break;
    }

    case 'textDelta': {
      applyTextDeltaEvent(state, event, raw);
      break;
    }

    case 'streamReset': {
      clearToolPreparingTimers(state);
      applyStreamResetEvent(state, event, raw);
      break;
    }

    case 'toolCallPreparing': {
      try {
        const payload = extractToolPreparingPayload(event, raw);
        if (payload) callbacks.scheduleToolPreparing(payload);
      } catch (err) {
        console.error('[streamStore] toolCallPreparing error:', err);
      }
      break;
    }

    case 'toolRunStarted':
    case 'toolRunUpdated':
    case 'toolRunCompleted': {
      try {
        const run = extractToolRunPayload(event, raw);
        if (!run) break;

        const preparing = toolPreparingPayloadFromRun(run);
        if (preparing) {
          callbacks.scheduleToolPreparing(preparing);
          break;
        }

        clearToolPreparingTimer(state, run.callId.trim());
        applyToolRunEvent(state, run);
      } catch (err) {
        console.error('[streamStore] toolRun event error:', err);
      }
      break;
    }

    case 'toolCallStart': {
      applyToolCallStartEvent(state, event, raw);
      break;
    }

    case 'toolCallArgsDelta': {
      applyToolCallArgsDeltaEvent(state, event, raw);
      break;
    }

    case 'toolCallProgress': {
      applyToolCallProgressEvent(state, event, raw);
      break;
    }

    case 'toolCallResult': {
      applyToolCallResultEvent(state, event, raw);
      break;
    }

    case 'usageUpdate': {
      applyUsageUpdateEvent(state, event, raw);
      break;
    }

    case 'status': {
      applyStatusEvent(state, event, raw);
      break;
    }

    case 'done': {
      clearStreamWatchdog(state);
      clearToolPreparingTimers(state);
      applyDoneEvent(state, event, raw);
      break;
    }

    case 'autoCompacted': {
      applyAutoCompactedEvent(state, event, raw);
      break;
    }

    case 'approvalRequested': {
      applyApprovalRequestedEvent(state, event, raw);
      break;
    }

    case 'approvalResolved': {
      applyApprovalResolvedEvent(state, event, raw);
      break;
    }

    case 'taskRunUpdated': {
      applyTaskRunUpdatedEvent(state, event, raw);
      break;
    }

    case 'taskRunEvent': {
      applyTaskRunEvent(state, event, raw);
      break;
    }

    case 'error': {
      clearStreamWatchdog(state);
      clearToolPreparingTimers(state);
      applyErrorEvent(state, event, raw);
      break;
    }
  }
}
