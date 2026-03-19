/**
 * Citation parser for [cite:CHUNK_ID] and [cite:CHUNK_ID|display text] markers.
 *
 * Transforms citation markers into markdown links that ReactMarkdown can render,
 * using the `cite:` URI scheme so the custom link component can detect and
 * render them as clickable citation chips.
 *
 * Handles streaming gracefully: incomplete markers (e.g. `[cite:abc` without
 * a closing bracket) are left as-is until the full marker arrives.
 */

/**
 * Matches `[cite:CHUNK_ID]` or `[cite:CHUNK_ID|display text]`.
 * - Group 1: chunk ID (UUID or any non-bracket, non-pipe chars)
 * - Group 2: optional display text after the pipe
 */
const CITE_REGEX = /\[cite:([^\]|]+?)(?:\|([^\]]*))?\]/g;

export interface ParsedChunkCitation {
  chunkId: string;
  displayText?: string;
}

/**
 * Replace `[cite:...]` markers with markdown links that the custom `a`
 * component can intercept.
 *
 * - `[cite:ABC123]` → `[¹](cite:ABC123)` (auto-numbered)
 * - `[cite:ABC123|design doc]` → `[design doc](cite:ABC123)`
 *
 * Citations are numbered sequentially per call so superscripts stay consistent
 * within a single message.
 */
export function preprocessChunkCitations(content: string): string {
  let counter = 0;
  return content.replace(CITE_REGEX, (_match, chunkId: string, displayText?: string) => {
    counter++;
    const id = chunkId.trim();
    if (displayText != null && displayText.trim().length > 0) {
      return `[${displayText.trim()}](cite:${id})`;
    }
    // Superscript number: ¹ ² ³ etc.
    return `[${toSuperscript(counter)}](cite:${id})`;
  });
}

export function extractChunkCitations(content: string): ParsedChunkCitation[] {
  const matches = content.matchAll(CITE_REGEX);
  const citations: ParsedChunkCitation[] = [];

  for (const match of matches) {
    const chunkId = match[1]?.trim();
    if (!chunkId) continue;
    const displayText = match[2]?.trim();
    citations.push({ chunkId, displayText: displayText || undefined });
  }

  return citations;
}

/** Convert a number to Unicode superscript characters. */
function toSuperscript(n: number): string {
  const superscripts = '⁰¹²³⁴⁵⁶⁷⁸⁹';
  return String(n)
    .split('')
    .map((d) => superscripts[+d] ?? d)
    .join('');
}

/**
 * Build a lookup map of chunk_id → EvidenceCard-like data from tool call
 * artifacts. Works with both streaming `ToolCallEvent[]` and persisted
 * `ConversationMessage[]` tool results.
 */
export interface CitationCardData {
  chunkId: string;
  documentPath: string;
  documentTitle: string;
  sourceName: string;
  content: string;
  score: number;
  headingPath: string[];
}

/** Extract citation card data from a raw artifact value (unknown shape). */
function extractCard(item: unknown): CitationCardData | null {
  if (!item || typeof item !== 'object') return null;
  const obj = item as Record<string, unknown>;
  const chunkId = (obj.chunkId ?? obj.chunk_id) as string | undefined;
  if (!chunkId) return null;
  return {
    chunkId: String(chunkId),
    documentPath: String(obj.documentPath ?? obj.document_path ?? obj.path ?? ''),
    documentTitle: String(obj.documentTitle ?? obj.document_title ?? obj.title ?? ''),
    sourceName: String(obj.sourceName ?? obj.source_name ?? obj.source ?? ''),
    content: String(obj.content ?? ''),
    score: Number(obj.score ?? obj.relevanceScore ?? obj.relevance_score ?? 0),
    headingPath: Array.isArray(obj.headingPath ?? obj.heading_path)
      ? (obj.headingPath ?? obj.heading_path) as string[]
      : [],
  };
}

export interface ToolCallArtifacts {
  artifacts?: import('../types/conversation').MessageArtifacts;
}

/**
 * Build a Map<chunkId, CitationCardData> from an array of tool call events
 * that may contain evidence card artifacts.
 */
export function buildCitationMap(
  toolCalls: ToolCallArtifacts[],
): Map<string, CitationCardData> {
  const map = new Map<string, CitationCardData>();
  for (const tc of toolCalls) {
    if (!tc.artifacts) continue;
    const items = Array.isArray(tc.artifacts) ? tc.artifacts : Object.values(tc.artifacts);
    for (const item of items) {
      const card = extractCard(item);
      if (card) {
        map.set(card.chunkId, card);
      }
    }
  }
  return map;
}

/* ------------------------------------------------------------------ */
/*  Inline citation preprocessing: [doc:], [file:], [url:]             */
/* ------------------------------------------------------------------ */

const DOC_REGEX = /\[doc:([^\]|]+?)(?:\|([^\]]*))?\]/g;
const FILE_REGEX_CITE = /\[file:([^\]|]+?)(?:\|([^\]]*))?\]/g;
const URL_REGEX = /\[url:([^\]|]+?)(?:\|([^\]]*))?\]/g;

/**
 * Convert `[doc:ID|label]`, `[file:PATH|label]`, `[url:URL|label]` markers
 * into markdown links with custom URI schemes so the MarkdownLink component
 * can render them as clickable inline badges.
 */
export function preprocessInlineCitations(content: string): string {
  return content
    .replace(DOC_REGEX, (_m, id: string, label?: string) => {
      const trimId = id.trim();
      const display = label?.trim() || trimId.split('/').pop() || trimId;
      return `[${display}](doc:${trimId})`;
    })
    .replace(FILE_REGEX_CITE, (_m, path: string, label?: string) => {
      const trimPath = path.trim();
      const filename = trimPath.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || trimPath;
      const display = label?.trim() || filename;
      return `[${display}](file:${trimPath})`;
    })
    .replace(URL_REGEX, (_m, url: string, label?: string) => {
      const trimUrl = url.trim();
      let display = label?.trim();
      if (!display) {
        try {
          display = new URL(trimUrl).hostname.replace(/^www\./, '');
        } catch {
          display = trimUrl;
        }
      }
      return `[${display}](url:${trimUrl})`;
    });
}
