import type { ConversationMessage } from '../../types/conversation';
import type { StreamRoundEvent, TraceEvent } from './protocol';
import { isOptimisticSteeringMessage } from '../chatMessageGuards';

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
 * Optimistic steering messages are real user messages, but while a turn is
 * still live the assistant's current reply/thinking/tool output is not in the
 * persisted message list yet. Rendering temporary steering inside the history
 * list therefore places it right after the turn's first user message and before
 * the live trace overlay. Keep temporary steering out of the history path and
 * let the backend-emitted steering status appear inside the trace timeline at
 * the actual interruption point.
 */
export function projectChatMessageVisibility(
  input: ChatMessageVisibilityInput,
): ChatMessageVisibilityProjection {
  const liveSteeringMessages = input.messages.filter(isOptimisticSteeringMessage);
  if (liveSteeringMessages.length === 0) {
    return {
      historyMessages: input.messages,
      liveSteeringMessages: [],
    };
  }

  const historyMessages = input.messages.filter((message) => !isOptimisticSteeringMessage(message));

  if (!input.isStreaming) {
    return {
      historyMessages,
      liveSteeringMessages: [],
    };
  }

  return {
    historyMessages,
    liveSteeringMessages,
  };
}
