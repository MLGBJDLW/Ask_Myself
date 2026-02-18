export interface IngestResult {
  sourceId: string;
  filesScanned: number;
  filesAdded: number;
  filesUpdated: number;
  filesSkipped: number;
  filesFailed: number;
  errors: string[];
}

export interface ScanProgress {
  sourceId: string;
  phase: string;
  current: number;
  total: number;
  currentFile: string | null;
}
