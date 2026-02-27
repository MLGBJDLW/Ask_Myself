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
  thinking: string | null;
  /** Optimistic-only: image attachments sent with this user message. */
  imageAttachments?: ImageAttachment[] | null;
}

export interface ToolCallRequest {
  id: string;
  name: string;
  arguments: string;
}

export interface ImageAttachment {
  base64Data: string;
  mediaType: string;
  originalName: string;
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
  reasoningEnabled: boolean | null;
  thinkingBudget: number | null;
  reasoningEffort: string | null;
  maxIterations: number | null;
  /** Optional cheaper model for summarization (e.g. "gpt-4o-mini"). */
  summarizationModel: string | null;
  /** Optional provider override for summarization (e.g. "open_ai"). */
  summarizationProvider: string | null;
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
  reasoningEnabled: boolean | null;
  thinkingBudget: number | null;
  reasoningEffort: string | null;
  maxIterations: number | null;
  /** Optional cheaper model for summarization (e.g. "gpt-4o-mini"). */
  summarizationModel: string | null;
  /** Optional provider override for summarization (e.g. "open_ai"). */
  summarizationProvider: string | null;
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
  type: 'textDelta' | 'toolCallStart' | 'toolCallResult' | 'thinking' | 'done' | 'error' | 'autoCompacted';
  delta?: string;
  callId?: string;
  toolName?: string;
  arguments?: string;
  content?: string;
  isError?: boolean;
  artifacts?: Record<string, unknown>;
  // `Done` events carry a full ConversationMessage; `Error` events carry a plain string.
  message?: ConversationMessage | string;
  usageTotal?: { promptTokens: number; completionTokens: number; totalTokens: number; thinkingTokens?: number; lastPromptTokens?: number };
}

export interface AgentFrontendEvent {
  conversationId: string;
  type: AgentEvent['type'];
  summary?: string;
  delta?: string;
  callId?: string;
  toolName?: string;
  arguments?: string;
  content?: string;
  isError?: boolean;
  artifacts?: Record<string, unknown>;
  message?: ConversationMessage | string;
  usageTotal?: { promptTokens: number; completionTokens: number; totalTokens: number; thinkingTokens?: number; lastPromptTokens?: number };
}

export interface ConversationStats {
  totalConversations: number;
  totalMessages: number;
  oldestConversation: string | null;
  dbSizeBytes: number;
}

export interface Checkpoint {
  id: string;
  conversationId: string;
  label: string;
  messageCount: number;
  estimatedTokens: number;
  createdAt: string;
}
