export interface Conversation {
  id: string;
  title: string;
  provider: string;
  model: string;
  systemPrompt: string;
  createdAt: string;
  updatedAt: string;
}

export type ArtifactPayload = Record<string, unknown> | unknown[];
export type MessageArtifacts = ArtifactPayload | null;

export interface ConversationMessage {
  id: string;
  conversationId: string;
  role: 'system' | 'user' | 'assistant' | 'tool';
  content: string;
  toolCallId: string | null;
  toolCalls: ToolCallRequest[];
  artifacts: MessageArtifacts;
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
  /** Optional whitelist of built-in tools that delegated subagents may use. */
  subagentAllowedTools: string[] | null;
  /** Max number of delegated workers allowed to run concurrently. */
  subagentMaxParallel?: number | null;
  /** Max number of delegated worker/judge calls allowed per turn. */
  subagentMaxCallsPerTurn?: number | null;
  /** Soft token budget for delegated workers and judges per turn. */
  subagentTokenBudget?: number | null;
  toolTimeoutSecs?: number | null;
  agentTimeoutSecs?: number | null;
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
  /** Optional whitelist of built-in tools that delegated subagents may use. */
  subagentAllowedTools: string[] | null;
  /** Max number of delegated workers allowed to run concurrently. */
  subagentMaxParallel?: number | null;
  /** Max number of delegated worker/judge calls allowed per turn. */
  subagentMaxCallsPerTurn?: number | null;
  /** Soft token budget for delegated workers and judges per turn. */
  subagentTokenBudget?: number | null;
}

export interface AppConfig {
  toolTimeoutSecs: number;
  agentTimeoutSecs: number;
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
  type: 'textDelta' | 'toolCallStart' | 'toolCallResult' | 'thinking' | 'done' | 'error' | 'autoCompacted' | 'usageUpdate';
  delta?: string;
  callId?: string;
  toolName?: string;
  arguments?: string;
  content?: string;
  isError?: boolean;
  artifacts?: ArtifactPayload;
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
  artifacts?: ArtifactPayload;
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

export interface UserMemory {
  id: string;
  content: string;
  createdAt: string;
  updatedAt: string;
}
