import {
  buildCollapsedLiveTrace,
  type LiveTraceTimelineItem,
} from '../src/lib/streaming/timelineViewModel';

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

function main(): void {
  testFullReplyFollowedBySummaryStaysVisible();
  console.log('ok - full reply followed by summary stays visible');
  testThinkingStillCollapsesAroundSingleFinalReply();
  console.log('ok - single final reply still collapses surrounding thinking');
}

main();
