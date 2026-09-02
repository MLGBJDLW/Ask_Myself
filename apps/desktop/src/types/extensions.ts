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

export interface McpConfigReloadReport {
  path: string;
  imported: number;
  removed: number;
  disabledAfterChange: number;
}

export interface UserExtensionLayout {
  version: number;
  root: string;
  capabilitiesDir: string;
  skillsDir: string;
  themesDir: string;
  workflowsDir: string;
  connectorsDir: string;
  mcpConfigPath: string;
  legacyAppDataDir: string;
}

export interface RegisteredSkillFileSyncReport {
  updated: number;
  unchanged: number;
  unregistered: number;
  rejected: string[];
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
  interface?: SkillInterfaceMetadata;
  dependencies?: SkillDependencies;
  policy?: SkillPolicy;
  sourcePath?: string | null;
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

export interface SkillInterfaceMetadata {
  displayName: string;
  shortDescription: string;
  iconSmall?: string | null;
  iconLarge?: string | null;
  defaultPrompt?: string | null;
}

export interface SkillDependencies {
  tools: SkillToolDependency[];
}

export interface SkillToolDependency {
  type: string;
  value: string;
  description?: string | null;
  transport?: string | null;
  url?: string | null;
}

export interface SkillPolicy {
  allowImplicitInvocation: boolean;
}

export type SkillResourceKind = 'script' | 'reference' | 'metadata' | 'asset';
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

export type SkillChangeAction = 'create' | 'patch';
export type SkillProposalStatus = 'pending' | 'applied' | 'rejected';

export interface SkillChangeProposal {
  id: string;
  action: SkillChangeAction;
  skillId: string | null;
  name: string;
  description: string;
  content: string;
  resourceBundle: SkillResourceFile[];
  rationale: string;
  warnings: SkillWarning[];
  status: SkillProposalStatus;
  conversationId: string | null;
  source: string;
  confidence: number;
  evidence: unknown;
  createdAt: string;
  updatedAt: string;
  appliedAt: string | null;
  rejectedAt: string | null;
}

export interface AppliedSkillChange {
  proposal: SkillChangeProposal;
  skill: Skill;
}
