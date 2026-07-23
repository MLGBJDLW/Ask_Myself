export type QuestionCardType = 'short' | 'long' | 'single_choice' | 'multi_choice' | 'confirm';

export interface QuestionCardOption {
  label: string;
  description?: string;
}

export interface QuestionCard {
  id: string;
  title: string;
  header?: string;
  question: string;
  why?: string;
  type: QuestionCardType;
  options?: QuestionCardOption[];
  placeholder?: string;
}

export interface QuestionRequest {
  callId: string;
  questions: QuestionCard[];
  status: 'pending' | 'answered';
}

export interface FormattedQuestionResponse {
  message: string;
  artifact: Record<string, unknown>;
}

const QUESTION_CARDS_BLOCK_RE = /```question-cards[^\S\r\n]*\r?\n([\s\S]*?)```/gi;
const VALID_TYPES = new Set<QuestionCardType>(['short', 'long', 'single_choice', 'multi_choice', 'confirm']);

function asString(value: unknown): string {
  return typeof value === 'string' ? value.trim() : '';
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function normalizeOption(value: unknown): QuestionCardOption | null {
  if (typeof value === 'string') {
    const label = value.trim();
    return label ? { label } : null;
  }
  const record = asRecord(value);
  if (!record) return null;
  const label = asString(record.label);
  if (!label) return null;
  return {
    label,
    description: asString(record.description) || undefined,
  };
}

function normalizeCard(value: unknown, index: number): QuestionCard | null {
  const record = asRecord(value);
  if (!record) return null;
  const question = asString(record.question);
  if (!question) return null;
  const header = asString(record.header);
  const title = asString(record.title) || header || `Question ${index + 1}`;
  const requestedType = asString(record.type) as QuestionCardType;
  const options = Array.isArray(record.options)
    ? record.options.map(normalizeOption).filter((option): option is QuestionCardOption => Boolean(option)).slice(0, 8)
    : undefined;
  const type = VALID_TYPES.has(requestedType)
    ? requestedType
    : options?.length
      ? 'single_choice'
      : 'short';
  return {
    id: asString(record.id) || `question-${index + 1}`,
    title,
    header: header || undefined,
    question,
    why: asString(record.why) || undefined,
    type,
    options: options && options.length > 0 ? options : undefined,
    placeholder: asString(record.placeholder) || undefined,
  };
}

function normalizeQuestions(value: unknown): QuestionCard[] {
  const record = asRecord(value);
  const values = Array.isArray(value)
    ? value
    : Array.isArray(record?.questions)
      ? record.questions
      : Array.isArray(record?.cards)
        ? record.cards
        : [];
  return values
    .map((question, index) => normalizeCard(question, index))
    .filter((question): question is QuestionCard => Boolean(question))
    .slice(0, 3);
}

function parseJsonRecord(value: unknown): Record<string, unknown> | null {
  if (typeof value !== 'string') return asRecord(value);
  try {
    return asRecord(JSON.parse(value));
  } catch {
    return null;
  }
}

function findQuestionArtifact(value: unknown, depth = 0): Record<string, unknown> | null {
  if (depth > 5) return null;
  const record = asRecord(value);
  if (!record) return null;
  if (record.kind === 'questionRequest') return record;
  for (const key of ['artifacts', 'toolOutput', 'data']) {
    const nested = findQuestionArtifact(record[key], depth + 1);
    if (nested) return nested;
  }
  return null;
}

export function extractQuestionCards(content: string): QuestionCard[] {
  const cards: QuestionCard[] = [];
  for (const match of content.matchAll(QUESTION_CARDS_BLOCK_RE)) {
    try {
      cards.push(...normalizeQuestions(JSON.parse(match[1] ?? '[]')));
    } catch {
      // Ignore malformed legacy blocks and render their markdown normally.
    }
  }
  return cards;
}

export function extractQuestionRequest(
  callId: string,
  args: unknown,
  artifacts?: unknown,
): QuestionRequest | null {
  const parsedArgs = parseJsonRecord(args);
  const artifact = findQuestionArtifact(artifacts);
  const questions = normalizeQuestions(artifact?.questions).length > 0
    ? normalizeQuestions(artifact?.questions)
    : normalizeQuestions(parsedArgs?.questions);
  if (questions.length === 0) return null;
  return {
    callId: asString(artifact?.callId) || callId,
    questions,
    status: artifact?.status === 'answered' ? 'answered' : 'pending',
  };
}

export function formatQuestionResponse(
  request: QuestionRequest,
  answers: Record<string, string[]>,
): FormattedQuestionResponse {
  const normalizedAnswers = request.questions.map((question) => ({
    id: question.id,
    question: question.question,
    answers: (answers[question.id] ?? []).map((answer) => answer.trim()).filter(Boolean),
  }));
  const message = normalizedAnswers
    .map(({ question, answers: values }) => `${question}\n${values.length > 0 ? values.join(', ') : 'No answer'}`)
    .join('\n\n');
  return {
    message,
    artifact: {
      kind: 'questionResponse',
      version: 1,
      requestCallId: request.callId,
      answers: normalizedAnswers,
    },
  };
}

export function stripQuestionCardsBlocks(content: string): string {
  return content.replace(QUESTION_CARDS_BLOCK_RE, '').trim();
}
