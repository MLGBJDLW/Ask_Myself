export type QuestionCardType = 'short' | 'long' | 'single_choice' | 'multi_choice' | 'confirm';

export interface QuestionCard {
  id: string;
  title: string;
  question: string;
  why?: string;
  type: QuestionCardType;
  options?: string[];
  placeholder?: string;
}

const QUESTION_CARDS_BLOCK_RE = /```question-cards\s*\n([\s\S]*?)```/gi;
const VALID_TYPES = new Set<QuestionCardType>(['short', 'long', 'single_choice', 'multi_choice', 'confirm']);

function asString(value: unknown): string {
  return typeof value === 'string' ? value.trim() : '';
}

function normalizeCard(value: unknown, index: number): QuestionCard | null {
  if (!value || typeof value !== 'object') return null;
  const record = value as Record<string, unknown>;
  const title = asString(record.title) || `Question ${index + 1}`;
  const question = asString(record.question);
  if (!question) return null;
  const rawType = asString(record.type) as QuestionCardType;
  const type = VALID_TYPES.has(rawType) ? rawType : 'short';
  const options = Array.isArray(record.options)
    ? record.options.map(asString).filter(Boolean).slice(0, 8)
    : undefined;
  return {
    id: asString(record.id) || `question-${index + 1}`,
    title,
    question,
    why: asString(record.why) || undefined,
    type,
    options: options && options.length > 0 ? options : undefined,
    placeholder: asString(record.placeholder) || undefined,
  };
}

export function extractQuestionCards(content: string): QuestionCard[] {
  const cards: QuestionCard[] = [];
  for (const match of content.matchAll(QUESTION_CARDS_BLOCK_RE)) {
    try {
      const parsed = JSON.parse(match[1] ?? '[]');
      const values = Array.isArray(parsed) ? parsed : Array.isArray(parsed.cards) ? parsed.cards : [];
      values.forEach((value: unknown, index: number) => {
        const card = normalizeCard(value, cards.length + index);
        if (card) cards.push(card);
      });
    } catch {
      // Ignore malformed drafts and let the markdown render normally.
    }
  }
  return cards;
}

export function stripQuestionCardsBlocks(content: string): string {
  return content.replace(QUESTION_CARDS_BLOCK_RE, '').trim();
}
