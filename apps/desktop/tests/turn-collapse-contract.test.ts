import { projectChatMessageVisibility } from '../src/lib/streaming/chatVisibility';
import {
  buildCollapsedLiveTrace,
  type LiveTraceTimelineItem,
} from '../src/lib/streaming/timelineViewModel';
import type { ConversationMessage } from '../src/types/conversation';

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function assertEqual<T>(actual: T, expected: T, message: string): void {
  if (actual !== expected) {
    throw new Error(`${message}: expected ${String(expected)}, got ${String(actual)}`);
  }
}

function reply(id: string, content: string): LiveTraceTimelineItem {
  return { kind: 'reply', id, content, isStreaming: false };
}

function thinking(id: string, text: string): LiveTraceTimelineItem {
  return {
    kind: 'thinking',
    id,
    isStreaming: false,
    sections: [{ kind: 'thinking', id: `${id}-section`, text }],
  };
}

function message(input: Partial<ConversationMessage> & Pick<ConversationMessage, 'id' | 'role'>): ConversationMessage {
  const { id, role, ...overrides } = input;
  return {
    id,
    conversationId: 'conversation-1',
    role,
    content: input.content ?? '',
    toolCallId: null,
    toolCalls: [],
    artifacts: null,
    tokenCount: 0,
    createdAt: '2026-07-31T00:00:00.000Z',
    sortOrder: 0,
    thinking: null,
    imageAttachments: null,
    ...overrides,
  };
}

function testFullReplyFollowedBySummaryStaysVisible(): void {
  const timeline: LiveTraceTimelineItem[] = [
    thinking('analysis', 'Inspected the repository and compared the relevant paths.'),
    reply('full-answer', 'This is the complete user-facing answer with all findings and changes.'),
    thinking('closing-check', 'Checking whether a follow-up is needed.'),
    reply('small-summary', 'Anything else to adjust?'),
  ];

  const collapsed = buildCollapsedLiveTrace({
    timeline,
    isStreaming: false,
    currentTraceActive: false,
  });

  assertEqual(
    collapsed,
    null,
    'a prior reply must never be folded into the thinking disclosure',
  );
}

function testThinkingStillCollapsesAroundSingleFinalReply(): void {
  const timeline: LiveTraceTimelineItem[] = [
    thinking('analysis', 'Inspected the repository and verified the change.'),
    reply('final-answer', 'The final answer remains outside the collapsed trace.'),
  ];

  const collapsed = buildCollapsedLiveTrace({
    timeline,
    isStreaming: false,
    currentTraceActive: false,
  });

  assert(collapsed !== null, 'thinking should still fold when there is one final reply');
  assertEqual(collapsed.finalItem.id, 'final-answer', 'the final reply remains visible');
  assertEqual(collapsed.historySections.length, 1, 'only reasoning is folded');
  assertEqual(collapsed.historySections[0].kind, 'thinking', 'folded content is reasoning');
}

function testQuestionResponseIsControlPlaneHistory(): void {
  const normalUser = message({ id: 'user-1', role: 'user', content: 'Please inspect this.' });
  const questionResponse = message({
    id: 'question-response-1',
    role: 'user',
    content: 'Proceed?\nYes',
    sortOrder: 1,
    artifacts: {
      kind: 'questionResponse',
      version: 1,
      requestCallId: 'request-input-1',
      answers: [],
    },
  });

  const projection = projectChatMessageVisibility({
    isStreaming: false,
    messages: [normalUser, questionResponse],
  });

  assertEqual(projection.historyMessages.length, 2, 'question response stays available for artifact replay');
  assertEqual(projection.historyMessages[0].role, 'user', 'ordinary user turn stays visible');
  assertEqual(projection.historyMessages[1].role, 'system', 'question continuation is hidden from normal bubbles');
  assertEqual(
    (projection.historyMessages[1].artifacts as Record<string, unknown>).kind,
    'questionResponse',
    'question response artifact remains available to mark the card answered',
  );
}

function main(): void {
  testFullReplyFollowedBySummaryStaysVisible();
  console.log('ok - full reply followed by summary stays visible');
  testThinkingStillCollapsesAroundSingleFinalReply();
  console.log('ok - single final reply still collapses surrounding thinking');
  testQuestionResponseIsControlPlaneHistory();
  console.log('ok - question response stays out of visible user turns');
}

main();
