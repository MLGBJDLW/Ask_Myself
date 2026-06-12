import { type ComponentProps, useMemo } from 'react';
import { MessageBubble } from '../../components/chat/MessageBubble';
import { ChatMessages as BaseChatMessages } from './ChatMessages';
import {
  projectChatMessageVisibility,
  projectChatStreamingVisibility,
} from '../../lib/streaming/chatVisibility';

type ChatMessagesProps = ComponentProps<typeof BaseChatMessages>;

/**
 * Transitional adapter for ChatMessages.
 *
 * Keep the duplicate projection logic here small and explicit while the main
 * ChatMessages renderer still owns both legacy streamRounds rendering and the
 * canonical trace timeline. New behavior should be added to the projection
 * helpers, not to this wrapper, so it can be folded into ChatMessages once the
 * streamRounds UI path is retired.
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
