export interface Conversation {
  id: string;
  title: string;
  provider: string;
  model: string;
  systemPrompt: string;
  collectionContext?: {
    title: string;
    description?: string | null;
    queryText?: string | null;
    sourceIds: string[];
  } | null;
  projectId?: string | null;
  /** `true` if the title is still auto-generated. Becomes `false` after a user rename. */
  titleIsAuto?: boolean;
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

export interface ConversationTurn {
  id: string;
  conversationId: string;
  userMessageId: string;
  assistantMessageId: string | null;
  status: string;
  routeKind?: string | null;
  trace?: Record<string, unknown> | unknown[] | null;
  createdAt: string;
  updatedAt: string;
  finishedAt?: string | null;
}

export interface AgentTaskRun {
  id: string;
  conversationId: string;
  turnId: string;
  userMessageId: string;
  status: string;
  phase: string;
  title: string;
  routeKind?: string | null;
  summary?: string | null;
  errorMessage?: string | null;
  provider?: string | null;
  model?: string | null;
  plan?: Record<string, unknown> | unknown[] | null;
  artifacts?: Record<string, unknown> | unknown[] | null;
  createdAt: string;
  updatedAt: string;
  startedAt?: string | null;
  finishedAt?: string | null;
}

export interface AgentTaskRunEvent {
  id: string;
  runId: string;
  eventType: string;
  label: string;
  status?: string | null;
  payload?: Record<string, unknown> | unknown[] | null;
  createdAt: string;
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
  /** Optional whitelist of delegated tool names that subagents may use. */
  subagentAllowedTools: string[] | null;
  /** Optional whitelist of enabled skill IDs that delegated subagents may inherit. */
  subagentAllowedSkillIds?: string[] | null;
  /** Max number of delegated workers allowed to run concurrently. */
  subagentMaxParallel?: number | null;
  /** Max number of delegated worker/judge calls allowed per turn. */
  subagentMaxCallsPerTurn?: number | null;
  /** Soft token budget for delegated workers and judges per turn. */
  subagentTokenBudget?: number | null;
  toolTimeoutSecs?: number | null;
  agentTimeoutSecs?: number | null;
  dynamicToolVisibility?: boolean | null;
  traceEnabled?: boolean | null;
  requireToolConfirmation?: boolean | null;
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
  /** Optional whitelist of delegated tool names that subagents may use. */
  subagentAllowedTools: string[] | null;
  /** Optional whitelist of enabled skill IDs that delegated subagents may inherit. */
  subagentAllowedSkillIds?: string[] | null;
  /** Max number of delegated workers allowed to run concurrently. */
  subagentMaxParallel?: number | null;
  /** Max number of delegated worker/judge calls allowed per turn. */
  subagentMaxCallsPerTurn?: number | null;
  /** Soft token budget for delegated workers and judges per turn. */
  subagentTokenBudget?: number | null;
  dynamicToolVisibility?: boolean | null;
  traceEnabled?: boolean | null;
  requireToolConfirmation?: boolean | null;
}

export interface AppConfig {
  toolTimeoutSecs: number;
  agentTimeoutSecs: number;
  cacheTtlHours: number;
  defaultSearchLimit: number;
  minSearchSimilarity: number;
  maxTextFileSize: number;
  maxVideoFileSize: number;
  maxAudioFileSize: number;
  llmTimeoutSecs: number;
  mcpCallTimeoutSecs: number;
  dynamicToolVisibility?: boolean;
  traceEnabled?: boolean;
  confirmDestructive?: boolean;
  shellAccessMode?: 'restricted' | 'confirm_all' | 'open';
  toolApprovalMode?: 'ask' | 'allow_all' | 'deny_all';
  hfMirrorBaseUrl?: string;
  ghproxyBaseUrl?: string;
}

export type ProviderType =
  | 'open_ai'
  | 'anthropic'
  | 'google'
  | 'deep_seek'
  | 'ollama'
  | 'lm_studio'
  | 'azure_open_ai'
  | 'zhipu'
  | 'moonshot'
  | 'qwen'
  | 'doubao'
  | 'yi'
  | 'baichuan'
  | 'custom';

export interface AgentEvent {
  type:
    | 'textDelta'
    | 'streamReset'
    | 'toolCallStart'
    | 'toolCallArgsDelta'
    | 'toolCallProgress'
    | 'toolCallResult'
    | 'thinking'
    | 'status'
    | 'done'
    | 'error'
    | 'autoCompacted'
    | 'usageUpdate'
    | 'approvalRequested'
    | 'approvalResolved'
    | 'taskRunUpdated'
    | 'taskRunEvent';
  delta?: string;
  reason?: string;
  callId?: string;
  toolName?: string;
  arguments?: string;
  /** Appended arguments fragment for streaming tool calls. */
  argumentsDelta?: string;
  /** Optional ordering index for argument deltas. */
  index?: number;
  /** Progress heartbeat note from a long-running tool. */
  note?: string;
  content?: string;
  tone?: 'muted' | 'success' | 'error';
  isError?: boolean;
  artifacts?: ArtifactPayload;
  // `Done` events carry a full ConversationMessage; `Error` events carry a plain string.
  message?: ConversationMessage | string;
  usageTotal?: { promptTokens: number; completionTokens: number; totalTokens: number; thinkingTokens?: number; lastPromptTokens?: number };
  taskRun?: AgentTaskRun;
  taskEvent?: AgentTaskRunEvent;
}

export type ApprovalRisk = 'low' | 'medium' | 'high';
export type ApprovalDecisionValue = 'allow_once' | 'allow_session' | 'deny' | 'never';

export interface ApprovalRequest {
  id: string;
  toolName: string;
  argumentsPreview: string;
  riskLevel: ApprovalRisk;
  reason: string;
}

export interface ApprovalPolicy {
  toolName: string;
  decision: string;
  createdAt?: string;
}

export interface ApprovalPolicyList {
  persisted: ApprovalPolicy[];
  session: ApprovalPolicy[];
}

export interface AgentFrontendEvent {
  conversationId: string;
  type: AgentEvent['type'];
  summary?: string;
  delta?: string;
  reason?: string;
  callId?: string;
  toolName?: string;
  arguments?: string;
  argumentsDelta?: string;
  index?: number;
  note?: string;
  content?: string;
  tone?: 'muted' | 'success' | 'error';
  isError?: boolean;
  artifacts?: ArtifactPayload;
  message?: ConversationMessage | string;
  usageTotal?: { promptTokens: number; completionTokens: number; totalTokens: number; thinkingTokens?: number; lastPromptTokens?: number };
  request?: ApprovalRequest;
  requestId?: string;
  decision?: ApprovalDecisionValue;
  taskRun?: AgentTaskRun;
  taskEvent?: AgentTaskRunEvent;
}

export interface ConversationStats {
  totalConversations: number;
  totalMessages: number;
  oldestConversation: string | null;
  dbSizeBytes: number;
}

export interface ConversationSearchResult {
  conversationId: string;
  conversationTitle: string | null;
  messagePreview: string;
  messageRole: string;
  timestamp: string;
  relevanceScore: number;
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
