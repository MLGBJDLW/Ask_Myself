export interface Source {
  id: string;
  kind: string;
  rootPath: string;
  includeGlobs: string[];
  excludeGlobs: string[];
  watchEnabled: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface ScanError {
  sourceId: string;
  path: string;
  errorMessage: string;
  errorCount: number;
  firstFailedAt: string;
  lastFailedAt: string;
}
