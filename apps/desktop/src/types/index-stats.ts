export interface IndexStats {
  totalSources: number;
  totalDocuments: number;
  totalChunks: number;
  ftsRows: number;
  isSynced: boolean;
}
