export interface Conversation {
  id: string;
  title: string;
  provider: string;
  model: string;
  systemPrompt: string;
  createdAt: string;
  updatedAt: string;
}

export interface ConversationMessage {
  id: string;
  conversationId: string;
  role: 'system' | 'user' | 'assistant' | 'tool';
  content: string;
  toolCallId: string | null;
  toolCalls: ToolCallRequest[];
  tokenCount: number;
  createdAt: string;
  sortOrder: number;
}

export interface ToolCallRequest {
  id: string;
  name: string;
  arguments: string;
}

export interface AgentConfig {
  id: string;
  name: string;
  provider: string;
  apiKey: string;
  baseUrl: string | null;
  model: string;
  temperature: number | null;
  maxTokens: number | null;
  contextWindow: number | null;
  isDefault: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface SaveAgentConfigInput {
  id: string | null;
  name: string;
  provider: string;
  apiKey: string;
  baseUrl: string | null;
  model: string;
  temperature: number | null;
  maxTokens: number | null;
  contextWindow: number | null;
  isDefault: boolean;
}

export type ProviderType =
  | 'open_ai'
  | 'anthropic'
  | 'google'
  | 'deep_seek'
  | 'ollama'
  | 'lm_studio'
  | 'azure_open_ai'
  | 'custom';

export interface AgentEvent {
  type: 'textDelta' | 'toolCallStart' | 'toolCallResult' | 'thinking' | 'done' | 'error';
  delta?: string;
  callId?: string;
  toolName?: string;
  arguments?: string;
  content?: string;
  isError?: boolean;
  artifacts?: Record<string, unknown>;
  // `Done` events carry a full ConversationMessage; `Error` events carry a plain string.
  message?: ConversationMessage | string;
  usageTotal?: { promptTokens: number; completionTokens: number; totalTokens: number };
}

export interface AgentFrontendEvent {
  conversationId: string;
  type: AgentEvent['type'];
  delta?: string;
  callId?: string;
  toolName?: string;
  arguments?: string;
  content?: string;
  isError?: boolean;
  artifacts?: Record<string, unknown>;
  message?: ConversationMessage | string;
  usageTotal?: { promptTokens: number; completionTokens: number; totalTokens: number };
}
