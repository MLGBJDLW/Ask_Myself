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

function artifactContainsKind(value: unknown, kind: string, depth = 0): boolean {
  if (depth > 6 || value == null) return false;
  if (Array.isArray(value)) {
    return value.some((item) => artifactContainsKind(item, kind, depth + 1));
  }
  if (typeof value !== 'object') return false;
  const record = value as Record<string, unknown>;
  if (record.kind === kind) return true;
  return Object.values(record).some((item) => artifactContainsKind(item, kind, depth + 1));
}

export function isQuestionResponseMessage(message: ConversationMessage): boolean {
  return message.role === 'user' && artifactContainsKind(message.artifacts, 'questionResponse');
}

function isNormalUserTurnMessage(message: ConversationMessage): boolean {
  return (
    message.role === 'user' &&
    !isSteeringMessage(message) &&
    !isGoalMessage(message) &&
    !isQuestionResponseMessage(message)
  );
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
 * Steering and structured interaction continuations are control-plane events,
 * not ordinary chat turns. Steering stays out of history entirely. A question
 * response remains in the projected collection as a system row so card state
 * can still discover its `questionResponse` artifact, while normal message and
 * turn rendering naturally omit it. This preserves durable audit/replay data
 * without adding a duplicate user bubble after a card selection.
 */
export function projectChatMessageVisibility(
  input: ChatMessageVisibilityInput,
): ChatMessageVisibilityProjection {
  return {
    historyMessages: input.messages
      .filter(message => !isSteeringMessage(message))
      .map(message => isQuestionResponseMessage(message)
        ? { ...message, role: 'system' as const }
        : message),
    liveSteeringMessages: input.isStreaming
      ? input.messages.filter(isOptimisticSteeringMessage)
      : [],
  };
}
