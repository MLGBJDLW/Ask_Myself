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
