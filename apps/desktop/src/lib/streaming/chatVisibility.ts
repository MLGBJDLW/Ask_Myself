import type { ConversationMessage } from '../../types/conversation';
import type { StreamRoundEvent, TraceEvent } from './protocol';
import {
  isGoalMessage,
  isOptimisticSteeringMessage,
  isSteeringMessage,
} from '../chatMessageGuards';

export interface ChatStreamingVisibilityInput {
  isStreaming: boolean;
  streamRounds: StreamRoundEvent[];
  traceEvents: TraceEvent[];
}

export interface ChatStreamingVisibilityProjection {
  streamRounds: StreamRoundEvent[];
  traceEvents: TraceEvent[];
  strategy: 'default' | 'traceTimeline';
}

export interface ChatMessageVisibilityInput {
  isStreaming: boolean;
  messages: ConversationMessage[];
}

export interface ChatMessageVisibilityProjection {
  historyMessages: ConversationMessage[];
  liveSteeringMessages: ConversationMessage[];
}

function isNormalUserTurnMessage(message: ConversationMessage): boolean {
  return message.role === 'user' && !isSteeringMessage(message) && !isGoalMessage(message);
}

export function hasPersistedResultAfterLatestUserMessage(
  messages: ConversationMessage[],
): boolean {
  let lastUserIdx = -1;
  for (let idx = messages.length - 1; idx >= 0; idx -= 1) {
    if (isNormalUserTurnMessage(messages[idx])) {
      lastUserIdx = idx;
      break;
    }
  }
  if (lastUserIdx < 0) return false;
  return messages
    .slice(lastUserIdx + 1)
    .some(message => message.role === 'assistant' || message.role === 'tool');
}

/**
 * ChatMessages renders live trace and completed stream rounds through two
 * separate paths. The live trace path intentionally trims events that have
 * already been materialized as `streamRounds`; however, the component also
 * hides `streamRounds` whenever a live trace timeline exists. During a new
 * streaming thinking phase this made earlier in-turn replies/thinking vanish
 * until the final persisted replay replaced the live state.
 *
 * While a turn is still streaming and both representations exist, prefer the
 * canonical trace timeline as the single source of visible truth. That keeps
 * prior reply/thinking/tool sections visible while the next thinking block is
 * streaming, and avoids rendering the same round twice.
 */
export function projectChatStreamingVisibility(
  input: ChatStreamingVisibilityInput,
): ChatStreamingVisibilityProjection {
  if (
    input.isStreaming &&
    input.streamRounds.length > 0 &&
    input.traceEvents.length > 0
  ) {
    return {
      streamRounds: [],
      traceEvents: input.traceEvents,
      strategy: 'traceTimeline',
    };
  }

  return {
    streamRounds: input.streamRounds,
    traceEvents: input.traceEvents,
    strategy: 'default',
  };
}

/**
 * Steering is an in-turn control signal rather than a new conversation turn.
 * While streaming, the backend-emitted status renders it at the actual point
 * where it was applied. Once streaming stops, both optimistic and persisted
 * steering rows must stay out of history. This invariant deliberately does not
 * depend on the assistant result having reloaded yet: `done` can flip the live
 * state before persistence catches up, which previously made steering flash at
 * the end of the turn and disappear again on the next send.
 */
export function projectChatMessageVisibility(
  input: ChatMessageVisibilityInput,
): ChatMessageVisibilityProjection {
  return {
    historyMessages: input.messages.filter(message => !isSteeringMessage(message)),
    liveSteeringMessages: input.isStreaming
      ? input.messages.filter(isOptimisticSteeringMessage)
      : [],
  };
}
