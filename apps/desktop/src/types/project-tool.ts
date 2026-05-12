export interface ProjectToolAccess {
  read: boolean;
  write: boolean;
  execute: boolean;
  network: boolean;
}

export interface ProjectToolCommand {
  program: string;
  args: string[];
  cwd?: string | null;
  timeoutSecs?: number | null;
}

export interface ProjectToolSummary {
  name: string;
  description: string;
  manifestHash: string;
  manifestPath: string;
  sourceRoot: string;
  runnable: boolean;
  access: ProjectToolAccess;
  command?: ProjectToolCommand | null;
  commandPreview?: string | null;
  parameterNames: string[];
  warnings: string[];
}

export interface ProjectToolManifestError {
  path: string;
  message: string;
}

export interface ProjectToolCatalog {
  kind: 'projectToolCatalog';
  manifestDirs: string[];
  tools: ProjectToolSummary[];
  errors: ProjectToolManifestError[];
}
