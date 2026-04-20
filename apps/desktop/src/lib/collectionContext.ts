export interface CollectionContextMeta {
  title: string;
  description?: string;
  queryText?: string;
  sourceIds: string[];
}

const HEADER = '## Collection Context';

export function buildCollectionContextPrompt(meta: CollectionContextMeta, evidenceLines: string): string {
  const parts = [
    HEADER,
    `Title: ${meta.title}`,
    meta.description ? `Description: ${meta.description}` : '',
    meta.queryText ? `Base query: ${meta.queryText}` : '',
    meta.sourceIds.length > 0 ? `Source IDs: ${meta.sourceIds.join(', ')}` : '',
    '',
    'Use this collection and its saved evidence as your primary working set.',
    'If the collection is insufficient, say so explicitly before widening to the full knowledge base.',
    'When widening scope, explain why extra retrieval was needed.',
    evidenceLines ? `Saved evidence:\n${evidenceLines}` : '',
  ].filter(Boolean);

  return parts.join('\n');
}

export function parseCollectionContextPrompt(prompt: string): CollectionContextMeta | null {
  if (!prompt.includes(HEADER)) return null;

  const lines = prompt.split(/\r?\n/);
  const start = lines.findIndex((line) => line.trim() === HEADER);
  if (start === -1) return null;

  const block = lines.slice(start + 1, start + 8);
  const read = (prefix: string) => {
    const line = block.find((entry) => entry.startsWith(prefix));
    return line ? line.slice(prefix.length).trim() : '';
  };

  const title = read('Title: ');
  if (!title) return null;

  const sourceIdsRaw = read('Source IDs: ');
  return {
    title,
    description: read('Description: ') || undefined,
    queryText: read('Base query: ') || undefined,
    sourceIds: sourceIdsRaw
      ? sourceIdsRaw.split(',').map((value) => value.trim()).filter(Boolean)
      : [],
  };
}
