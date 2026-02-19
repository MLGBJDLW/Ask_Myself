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

export interface BatchProgress {
  operation: string;
  sourceIndex: number;
  sourceCount: number;
  sourceId: string;
  phase: string;
  current: number;
  total: number;
  currentFile: string | null;
}

export interface FtsProgress {
  operation: string;
  phase: string;
}

export interface DownloadProgress {
  filename: string;
  bytesDownloaded: number;
  totalBytes: number | null;
  fileIndex: number;
  totalFiles: number;
}
