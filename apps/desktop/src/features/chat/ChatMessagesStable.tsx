import { type ComponentProps, useMemo } from 'react';
import { MessageBubble } from '../../components/chat/MessageBubble';
import { ChatMessages as BaseChatMessages } from './ChatMessages';
import {
  projectChatMessageVisibility,
  projectChatStreamingVisibility,
} from '../../lib/streaming/chatVisibility';

type ChatMessagesProps = ComponentProps<typeof BaseChatMessages>;

/**
 * Stabilizes ChatMessages' live streaming inputs before the heavy renderer sees
 * them. The base renderer has two valid render paths: stream rounds and live
 * trace timeline. When both exist during an active turn, the live-trace path is
 * the only one that can preserve prior in-turn replies while showing the next
 * streaming thinking block. Passing both makes the base renderer suppress
 * rounds and then trim those same events from the live trace, causing the user
 * to see only the latest thinking block until final replay restores the turn.
 *
 * Temporary steering messages have a similar ordering issue: while the current
 * assistant turn is still live, the in-progress reply/thinking/tool output is
 * rendered outside the persisted message list. Keeping optimistic steering in
 * that list places it directly under the turn's first user message. We render
 * it as live input after the current trace until backend persistence can place
 * it by sort order at turn completion.
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
  const messageVisibility = useMemo(
    () => projectChatMessageVisibility({
      isStreaming: props.isStreaming,
      messages: props.messages,
    }),
    [props.isStreaming, props.messages],
  );

  return (
    <>
      <BaseChatMessages
        {...props}
        messages={messageVisibility.historyMessages}
        streamRounds={visibility.streamRounds}
        traceEvents={visibility.traceEvents}
      />
      {messageVisibility.liveSteeringMessages.length > 0 && (
        <div className="shrink-0 border-t border-border/45 bg-surface-1/95 px-4 pb-2 pt-2">
          {messageVisibility.liveSteeringMessages.map((message) => (
            <MessageBubble
              key={message.id}
              msg={message}
              alwaysShowTimestamp
            />
          ))}
        </div>
      )}
    </>
  );
}
