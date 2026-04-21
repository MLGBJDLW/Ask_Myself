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
}

export interface SaveSkillInput {
  id?: string | null;
  name: string;
  description: string;
  content: string;
  enabled: boolean;
}
