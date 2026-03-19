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
  content: string;
  enabled: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface SaveSkillInput {
  id?: string | null;
  name: string;
  content: string;
  enabled: boolean;
}
