import type { FileType } from "./document";

export interface SearchFilters {
  sourceIds: string[];
  fileTypes: FileType[];
  dateFrom: string | null;
  dateTo: string | null;
}

export type SearchMode = 'fts' | 'hybrid' | 'fts+graph' | 'hybrid+graph';

export interface GraphEntityHit {
  id: string;
  label: string;
  entityType: string;
  score: number;
  mentionCount: number;
}

export interface GraphDocumentHit {
  documentId: string;
  sourceId: string;
  title: string;
  path: string;
  score: number;
  matchedEntities: string[];
  reasons: string[];
}

export interface GraphRetrievalReport {
  strategy: string;
  query: string;
  queryExpansionTerms: string[];
  entities: GraphEntityHit[];
  candidateDocuments: GraphDocumentHit[];
  expandedChunkIds: string[];
  boostedChunkIds: string[];
}

export interface SearchResult {
  query: string;
  totalMatches: number;
  evidenceCards: import("./evidence").EvidenceCard[];
  searchTimeMs: number;
  searchMode?: SearchMode;
  graphRetrieval?: GraphRetrievalReport | null;
}

export type ContextItemRole =
  | 'instruction'
  | 'evidence'
  | 'tool_guidance'
  | 'memory'
  | 'conversation'
  | 'source_scope';

export type ContextTrustLevel =
  | 'system'
  | 'user_selected'
  | 'retrieved_evidence'
  | 'agent_memory'
  | 'external';

export interface ContextPackItem {
  id: string;
  role: ContextItemRole;
  source: string;
  reason: string;
  trustLevel: ContextTrustLevel;
  tokenEstimate: number;
  payload: Record<string, unknown> | unknown[] | string | number | boolean | null;
}

export interface ContextPack {
  version: number;
  purpose: string;
  tokenBudget?: number | null;
  totalTokenEstimate: number;
  items: ContextPackItem[];
}
