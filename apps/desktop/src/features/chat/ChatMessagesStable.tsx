import { type ComponentProps, useMemo } from 'react';
import { ChatMessages as BaseChatMessages } from './ChatMessages';
import { projectChatStreamingVisibility } from '../../lib/streaming/chatVisibility';

type ChatMessagesProps = ComponentProps<typeof BaseChatMessages>;

/**
 * Stabilizes ChatMessages' live streaming inputs before the heavy renderer sees
 * them. The base renderer has two valid render paths: stream rounds and live
 * trace timeline. When both exist during an active turn, the live-trace path is
 * the only one that can preserve prior in-turn replies while showing the next
 * streaming thinking block. Passing both makes the base renderer suppress
 * rounds and then trim those same events from the live trace, causing the user
 * to see only the latest thinking block until final replay restores the turn.
 */
export function ChatMessages(props: ChatMessagesProps) {
  const visibility = useMemo(
    () => projectChatStreamingVisibility({
      isStreaming: props.isStreaming,
      streamRounds: props.streamRounds,
      traceEvents: props.traceEvents,
    }),
    [props.isStreaming, props.streamRounds, props.traceEvents],
  );

  return (
    <BaseChatMessages
      {...props}
      streamRounds={visibility.streamRounds}
      traceEvents={visibility.traceEvents}
    />
  );
}
