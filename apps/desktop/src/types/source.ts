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
