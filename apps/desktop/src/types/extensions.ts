// MCP Server types

export interface McpServer {
  id: string;
  name: string;
  transport: string;
  command: string | null;
  args: string | null;
  url: string | null;
  envJson: string | null;
  headersJson: string | null;
  enabled: boolean;
  createdAt: string;
  updatedAt: string;
  builtinId: string | null;
}

export interface SaveMcpServerInput {
  id?: string | null;
  name: string;
  transport: string;
  command?: string | null;
  args?: string | null;
  url?: string | null;
  envJson?: string | null;
  headersJson?: string | null;
  enabled: boolean;
}

export interface McpToolInfo {
  name: string;
  description: string | null;
  inputSchema: Record<string, unknown>;
}

// Skill types

export interface Skill {
  id: string;
  name: string;
  /** Concise trigger-match description (when to activate this skill). */
  description: string;
  content: string;
  enabled: boolean;
  createdAt: string;
  updatedAt: string;
  /** True for bundled SKILL.md skills — read-only in the UI. */
  builtin?: boolean;
  resources?: SkillResourceInfo[];
}

export interface SaveSkillInput {
  id?: string | null;
  name: string;
  description: string;
  content: string;
  enabled: boolean;
  resourceBundle?: SkillResourceFile[];
}

export type SkillResourceKind = 'script' | 'reference' | 'asset';
export type SkillResourceEncoding = 'utf8' | 'base64';

export interface SkillResourceInfo {
  path: string;
  kind: SkillResourceKind;
  bytes: number;
}

export interface SkillResourceFile {
  path: string;
  kind: SkillResourceKind;
  encoding: SkillResourceEncoding;
  content: string;
}

export interface DiscoveredSkillBundle {
  skillFile: string;
  skillDir: string;
  name: string;
  description: string;
  resources: SkillResourceInfo[];
  warnings: SkillWarning[];
}

export type SkillWarningSeverity = 'info' | 'warn' | 'block';

export interface SkillWarning {
  severity: SkillWarningSeverity;
  /** Stable machine-readable identifier (e.g. `pattern.rm_rf`). */
  code: string;
  /** Human-readable English message. */
  message: string;
}
