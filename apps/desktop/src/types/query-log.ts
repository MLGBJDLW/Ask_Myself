export interface QueryLog {
  id: string;
  queryText: string;
  resultCount: number;
  searchTimeMs: number;
  createdAt: string;
}
