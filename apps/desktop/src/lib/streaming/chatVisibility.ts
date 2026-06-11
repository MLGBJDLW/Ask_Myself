import type { StreamRoundEvent, TraceEvent } from './protocol';

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
