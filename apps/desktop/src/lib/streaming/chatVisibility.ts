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

function latestNormalUserMessageIndex(messages: ConversationMessage[]): number {
  for (let idx = messages.length - 1; idx >= 0; idx -= 1) {
    if (isNormalUserTurnMessage(messages[idx])) return idx;
  }
  return -1;
}

function projectCompletedHistoryMessages(
  messages: ConversationMessage[],
): ConversationMessage[] {
  const lastUserIdx = latestNormalUserMessageIndex(messages);
  if (lastUserIdx < 0) {
    return messages.filter(message => !isOptimisticSteeringMessage(message));
  }

  const beforeTurnResult = messages.slice(0, lastUserIdx + 1);
  const afterLatestUser = messages.slice(lastUserIdx + 1);
  const persistedSteering = afterLatestUser.filter(
    message => isSteeringMessage(message) && !isOptimisticSteeringMessage(message),
  );
  const nonSteeringResults = afterLatestUser.filter(message => !isSteeringMessage(message));
  return [
    ...beforeTurnResult,
    ...persistedSteering,
    ...nonSteeringResults,
  ];
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
  const hasCompletedResult = hasPersistedResultAfterLatestUserMessage(input.messages);
  if (hasCompletedResult && !input.isStreaming) {
    return {
      historyMessages: projectCompletedHistoryMessages(input.messages),
      liveSteeringMessages: [],
    };
  }

  const shouldHideSteeringFromHistory = input.isStreaming;
  if (!shouldHideSteeringFromHistory && liveSteeringMessages.length === 0) {
    return {
      historyMessages: input.messages,
      liveSteeringMessages: [],
    };
  }

  const historyMessages = input.messages.filter((message) => (
    shouldHideSteeringFromHistory
      ? !isSteeringMessage(message)
      : !isOptimisticSteeringMessage(message)
  ));

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
