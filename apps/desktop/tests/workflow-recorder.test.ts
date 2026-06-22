import {
  buildRecordedWorkflowPrompt,
  hasRecordableWorkflow,
  splitRecordingLines,
  type RecordedWorkflowPromptInput,
} from '../src/lib/workflowRecorder';

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function assertEqual<T>(actual: T, expected: T, message: string): void {
  if (actual !== expected) {
    throw new Error(`${message}: expected ${String(expected)}, got ${String(actual)}`);
  }
}

function assertDeepEqual<T>(actual: T, expected: T, message: string): void {
  const actualJson = JSON.stringify(actual);
  const expectedJson = JSON.stringify(expected);
  if (actualJson !== expectedJson) {
    throw new Error(`${message}: expected ${expectedJson}, got ${actualJson}`);
  }
}

function recording(overrides: Partial<RecordedWorkflowPromptInput> = {}): RecordedWorkflowPromptInput {
  return {
    name: 'Weekly report export',
    objective: 'Download the recurring weekly report and summarize anomalies.',
    context: 'The report source changes date ranges every week.',
    variableInputs: ['date range', 'recipient'],
    steps: [
      { id: 'step-1', kind: 'action', text: 'Open the reporting page.' },
      { id: 'step-2', kind: 'decision', text: 'Choose the weekly date range.' },
      { id: 'step-3', kind: 'check', text: 'Confirm the exported CSV has rows.' },
    ],
    preferences: ['Name files with ISO dates.'],
    successCriteria: ['CSV exported', 'Summary includes anomalies'],
    safetyNotes: ['Do not send the report until the user approves.'],
    ...overrides,
  };
}

assertDeepEqual(
  splitRecordingLines('- client\n* date range\n\noutput folder'),
  ['client', 'date range', 'output folder'],
  'line parser strips bullets and empty lines',
);

const prompt = buildRecordedWorkflowPrompt(recording({
  replayValues: ['date range = 2026-06-15..2026-06-21'],
}));

assert(prompt.includes('Workflow: Weekly report export'), 'prompt includes workflow name');
assert(prompt.includes('Variable inputs that may change between replays'), 'prompt includes variable section');
assert(prompt.includes('2. [Decision] Choose the weekly date range.'), 'prompt preserves step kind labels');
assert(prompt.includes('Runtime values for this replay'), 'prompt includes direct replay values');
assert(prompt.includes('Adapt the workflow semantically'), 'prompt avoids low-level replay semantics');
assert(prompt.includes('Verify the success criteria'), 'prompt carries verification contract');

assertEqual(hasRecordableWorkflow(recording()), true, 'complete recording is recordable');
assertEqual(
  hasRecordableWorkflow(recording({ objective: '', steps: [{ id: 'empty', kind: 'action', text: '' }] })),
  false,
  'missing objective and steps are not recordable',
);
