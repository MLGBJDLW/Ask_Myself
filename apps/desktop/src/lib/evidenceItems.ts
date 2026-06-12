import { extractChunkCitations } from './citationParser';
import type { CitationCardData } from './citationParser';
import { isWebUrl, sourceBasename, sourceHost } from './sourceDisplay';

export interface EvidenceItem {
  chunkId: string;
  displayText: string;
  card?: CitationCardData;
}

export interface EvidenceLookup {
  getCard(chunkId: string): CitationCardData | undefined;
}

interface EvidenceGroup {
  chunkId: string;
  card?: CitationCardData;
  count: number;
  displayText?: string;
}

export function evidenceSourceLabel(
  card: CitationCardData | undefined,
  displayText: string | undefined,
  fallback: string,
): string {
  const sourcePath = card?.documentPath?.trim() ?? '';
  const title = card?.documentTitle?.trim() ?? '';
  if (sourcePath) {
    const host = isWebUrl(sourcePath) ? sourceHost(sourcePath) : '';
    if (host) {
      return title ? `${title} · ${host}` : host;
    }
    return title || sourceBasename(sourcePath);
  }
  return displayText || fallback;
}

export function buildEvidenceItemsFromContent(
  content: string,
  citationLookup: EvidenceLookup | undefined,
  fallbackLabel: (index: number) => string,
  options: { dedupeChunks?: boolean; sortByFrequency?: boolean } = {},
): EvidenceItem[] {
  const grouped = new Map<string, EvidenceGroup>();
  const seenChunks = new Set<string>();
  const dedupeChunks = options.dedupeChunks ?? false;

  for (const entry of extractChunkCitations(content)) {
    if (dedupeChunks) {
      if (seenChunks.has(entry.chunkId)) continue;
      seenChunks.add(entry.chunkId);
    }

    const card = citationLookup?.getCard(entry.chunkId);
    const groupKey = card?.documentPath?.trim() || card?.documentTitle?.trim() || entry.chunkId;
    const existing = grouped.get(groupKey);
    if (existing) {
      existing.count += 1;
      if (!existing.card && card) existing.card = card;
      if (!existing.displayText && entry.displayText) {
        existing.displayText = entry.displayText;
      }
      continue;
    }

    grouped.set(groupKey, {
      chunkId: entry.chunkId,
      card,
      count: 1,
      displayText: entry.displayText,
    });
  }

  let items = Array.from(grouped.values());
  if (options.sortByFrequency) {
    items = items.sort((a, b) => {
      if (b.count !== a.count) return b.count - a.count;
      const aLabel = a.card?.documentTitle || a.card?.documentPath || a.displayText || a.chunkId;
      const bLabel = b.card?.documentTitle || b.card?.documentPath || b.displayText || b.chunkId;
      return aLabel.localeCompare(bLabel);
    });
  }

  return items.map((item, index) => {
    const baseLabel = evidenceSourceLabel(
      item.card,
      item.displayText,
      fallbackLabel(index + 1),
    );
    return {
      chunkId: item.chunkId,
      displayText: item.count > 1 ? `${baseLabel} ×${item.count}` : baseLabel,
      card: item.card,
    };
  });
}
