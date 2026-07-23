import {
  extractQuestionCards,
  extractQuestionRequest,
  formatQuestionResponse,
} from '../src/lib/questionCards';

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function assertEqual<T>(actual: T, expected: T, message: string): void {
  if (actual !== expected) {
    throw new Error(`${message}: expected ${String(expected)}, got ${String(actual)}`);
  }
}

const legacy = extractQuestionCards(`\`\`\`question-cards\r\n[{"id":"scope","question":"Which scope?","type":"single_choice","options":["App","Repo"]}]\r\n\`\`\``);
assertEqual(legacy.length, 1, 'legacy CRLF blocks render');
assertEqual(legacy[0].options?.[0]?.label, 'App', 'string options normalize to labels');

const request = extractQuestionRequest(
  'call-question-1',
  JSON.stringify({
    questions: [{
      id: 'scope',
      header: 'Scope',
      question: 'Which scope?',
      type: 'single_choice',
      options: [{ label: 'App', description: 'Only this app.' }, { label: 'Repo', description: 'Whole repository.' }],
    }],
  }),
  {
    kind: 'questionRequest',
    version: 1,
    callId: 'call-question-1',
    status: 'pending',
  },
);

assert(request, 'tool arguments produce a question request');
assertEqual(request.questions[0].options?.[1]?.description, 'Whole repository.', 'rich option descriptions survive');

const response = formatQuestionResponse(request, { scope: ['Repo'] });
assert(response.message.includes('Which scope?'), 'response includes question text');
assert(response.message.includes('Repo'), 'response includes selected answer');
assertEqual(response.artifact.kind, 'questionResponse', 'response carries a typed artifact');
assertEqual(response.artifact.requestCallId, 'call-question-1', 'response links to the tool call');
