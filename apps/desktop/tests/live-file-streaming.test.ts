import { streamStore } from '../src/lib/streamStore';
import type { AgentFrontendEvent, ToolRunItem } from '../src/types/conversation';

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function preparingFileRun(additions: number, content: string): ToolRunItem {
  return {
    callId: 'call-live-file',
    toolName: 'create_file',
    owner: {
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
        hunks: [],
      },
    },
  };
}

const conversationId = 'conversation-live-file-preview';
streamStore.startStream(conversationId);

const started: AgentFrontendEvent = {
  conversationId,
  runEvent: {
    version: 2,
    runId: 'run-live-file',
    turnId: 'turn-live-file',
    eventSeq: 1,
    kind: 'toolStarted',
    phase: 'tooling',
    label: 'create_file',
    status: 'preparing',
    payload: { run: preparingFileRun(2, 'first\nsecond') },
  },
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

const updated: AgentFrontendEvent = {
  conversationId,
  runEvent: {
    version: 2,
    runId: 'run-live-file',
    turnId: 'turn-live-file',
    eventSeq: 2,
    kind: 'toolProgress',
    phase: 'tooling',
    label: 'create_file',
    status: 'preparing',
    payload: { run: preparingFileRun(3, 'first\nsecond\nthird') },
  },
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

streamStore.clearStream(conversationId);
