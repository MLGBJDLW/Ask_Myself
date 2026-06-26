import { streamStore } from '../src/lib/streamStore';
import type { AgentFrontendEvent, ToolRunItem } from '../src/types/conversation';

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function assertEqual<T>(actual: T, expected: T, message: string): void {
  if (actual !== expected) {
    throw new Error(`${message}: expected ${String(expected)}, got ${String(actual)}`);
  }
}

function preparingFileRun(
  additions: number,
  content: string,
  receivedBytes: number,
): ToolRunItem {
  return {
    callId: 'call-live-file',
    toolName: 'create_file',
    plugin: {
      id: 'file-workspace',
      name: 'File Workspace',
      capability: 'Scoped file work',
      description: 'Writes scoped workspace files',
    },
    status: 'preparing',
    arguments: `{"path":"notes.md","content":${JSON.stringify(content)}}`,
    renderKind: 'fileChange',
    capabilities: {
      inputStreaming: 'uiPreview',
      renderKind: 'fileChange',
      readOnly: false,
      destructive: true,
      concurrencySafe: false,
      interruptBehavior: 'block',
      resourceKeys: ['file:notes.md'],
    },
    artifacts: {
      kind: 'fileChangePreview',
      preview: true,
      inputProgress: {
        receivedBytes,
        argumentsComplete: false,
      },
      diffStats: {
        kind: 'diffStats',
        path: 'notes.md',
        operation: 'create',
        additions,
        deletions: 0,
        filesChanged: 1,
        hunks: 1,
      },
      diff: {
        path: 'notes.md',
        operation: 'create',
        additions,
        deletions: 0,
        hunks: [{
          oldStart: 0,
          newStart: 1,
          oldLines: 0,
          newLines: additions,
          lines: content.split('\n').map((line, index) => ({
            type: 'addition',
            oldLine: null,
            newLine: index + 1,
            content: line,
          })),
        }],
      },
    },
  };
}

const conversationId = 'conversation-live-file-preview';
streamStore.startStream(conversationId);

const started: AgentFrontendEvent = {
  conversationId,
  type: 'toolRunStarted',
  run: preparingFileRun(2, 'first\nsecond', 4096),
};
streamStore.dispatch(conversationId, started);

let state = streamStore.getStream(conversationId);
assert(state, 'preparing run should create stream state');
assert(state.toolCalls.length === 1, 'preparing run should create one tool card immediately');
assert(state.toolCalls[0].status === 'preparing', 'tool card should remain in preparing state');
assert(
  (state.toolCalls[0].artifacts as Record<string, unknown> | undefined)?.kind === 'fileChangePreview',
  'semantic file preview artifact should survive projection',
);
assert(
  ((state.toolCalls[0].artifacts as Record<string, unknown>).diffStats as Record<string, unknown>).additions === 2,
  'initial additions should be visible before tool completion',
);
assertEqual(state.toolCalls[0].argsBytes, 4096, 'streamed argument byte count');
const initialDiff = (state.toolCalls[0].artifacts as Record<string, unknown>).diff as Record<string, unknown>;
const initialHunks = initialDiff.hunks as Array<Record<string, unknown>>;
const initialLines = initialHunks[0].lines as Array<Record<string, unknown>>;
assertEqual(initialLines[0].type, 'addition', 'first live line type');
assertEqual(initialLines[1].content, 'second', 'second live addition content');

const updated: AgentFrontendEvent = {
  conversationId,
  type: 'toolRunUpdated',
  run: preparingFileRun(3, 'first\nsecond\nthird', 8192),
};
streamStore.dispatch(conversationId, updated);

state = streamStore.getStream(conversationId);
assert(state, 'updated run should preserve stream state');
assert(state.toolCalls.length === 1, 'preparing updates should patch rather than duplicate the card');
assert(
  ((state.toolCalls[0].artifacts as Record<string, unknown>).diffStats as Record<string, unknown>).additions === 3,
  'updated additions should stream into the existing card',
);
assert(
  state.toolCalls[0].arguments.includes('third'),
  'latest partial arguments should replace the stale placeholder arguments',
);
assertEqual(state.toolCalls[0].argsBytes, 8192, 'updated streamed argument byte count');
const updatedDiff = (state.toolCalls[0].artifacts as Record<string, unknown>).diff as Record<string, unknown>;
const updatedHunks = updatedDiff.hunks as Array<Record<string, unknown>>;
const updatedLines = updatedHunks[0].lines as Array<Record<string, unknown>>;
assertEqual(updatedLines.length, 3, 'live diff line count');
assertEqual(updatedLines[2].content, 'third', 'latest live addition content');

streamStore.clearStream(conversationId);
