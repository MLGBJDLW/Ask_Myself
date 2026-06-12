import { type ComponentProps, useMemo } from 'react';
import { ChatMessages as BaseChatMessages } from './ChatMessages';
import {
  projectChatMessageVisibility,
  projectChatStreamingVisibility,
} from '../../lib/streaming/chatVisibility';

type ChatMessagesProps = ComponentProps<typeof BaseChatMessages>;

/**
 * Transitional adapter for ChatMessages.
 *
 * Temporary steering messages are also kept out of the persisted history path
 * while streaming. The backend now emits the accepted steering text as an inline
 * status trace at the exact interruption point, which lets the trace timeline
 * show: previous thinking → user steering → reset/new thinking. Rendering the
 * optimistic message as a sibling below BaseChatMessages would put it at the
 * bottom of the chat instead of at the interruption point.
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
    <BaseChatMessages
      {...props}
      messages={messageVisibility.historyMessages}
      streamRounds={visibility.streamRounds}
      traceEvents={visibility.traceEvents}
    />
  );
}
