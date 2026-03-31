export interface Highlight {
  start: number;
  end: number;
  term: string;
}

export interface EvidenceCard {
  chunkId: string;
  documentId: string;
  sourceId: string;
  sourceName: string;
  documentPath: string;
  documentTitle: string;
  content: string;
  headingPath: string[];
  score: number;
  highlights: Highlight[];
  snippet?: string;
  documentDate?: string;
  credibility?: number;
  freshnessDays?: number;
}
