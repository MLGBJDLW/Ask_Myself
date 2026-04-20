export interface OcrConfig {
  enabled: boolean;
  confidenceThreshold: number;
  llmFallbackEnabled: boolean;
  detLimitSideLen: number;
  useCls: boolean;
  modelPath: string;
  languages: string[];
}

export interface OcrDownloadProgress {
  filename: string;
  bytesDownloaded: number;
  totalBytes: number | null;
  fileIndex: number;
  totalFiles: number;
}
