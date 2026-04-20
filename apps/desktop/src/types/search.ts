import type { FileType } from "./document";

export interface SearchFilters {
  sourceIds: string[];
  fileTypes: FileType[];
  dateFrom: string | null;
  dateTo: string | null;
}

export interface SearchResult {
  query: string;
  totalMatches: number;
  evidenceCards: import("./evidence").EvidenceCard[];
  searchTimeMs: number;
  searchMode?: 'fts' | 'hybrid';
}
