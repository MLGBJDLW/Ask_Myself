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
  personaId?: string | null;
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

export interface AgentTaskRunListItem {
  run: AgentTaskRun;
  conversationTitle?: string | null;
  projectId?: string | null;
  projectName?: string | null;
  userMessagePreview: string;
  eventCount: number;
  subtaskTotal: number;
  subtaskCompleted: number;
  subtaskFailed: number;
  subtaskRunning: number;
  artifactKinds: string[];
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

export interface AgentSubtaskRun {
  id: string;
  parentRunId: string;
  label: string;
  role: string;
  status: string;
  phase: string;
  input?: Record<string, unknown> | unknown[] | null;
  output?: Record<string, unknown> | unknown[] | null;
  errorMessage?: string | null;
  tokenBudget?: number | null;
  createdAt: string;
  updatedAt: string;
  startedAt?: string | null;
  finishedAt?: string | null;
}

export interface AgentExecutionGraph {
  runId: string;
  nodes: AgentExecutionGraphNode[];
  edges: AgentExecutionGraphEdge[];
}

export interface AgentExecutionGraphNode {
  id: string;
  nodeType: string;
  label: string;
  role: string;
  status: string;
  phase: string;
  summary?: string | null;
  errorMessage?: string | null;
  input?: Record<string, unknown> | unknown[] | null;
  output?: Record<string, unknown> | unknown[] | null;
  tokenBudget?: number | null;
  startedAt?: string | null;
  finishedAt?: string | null;
}

export interface AgentExecutionGraphEdge {
  from: string;
  to: string;
  label: string;
}

export interface AgentTaskArtifactSummary {
  id: string;
  runId: string;
  kind: string;
  title: string;
  summary?: string | null;
  paths: string[];
  source: string;
  createdAt: string;
  payload: Record<string, unknown> | unknown[] | string | number | boolean | null;
}

export interface AgentTaskArtifact {
  id: string;
  runId: string;
  kind: string;
  title: string;
  summary?: string | null;
  content: string;
  paths: string[];
  payload?: Record<string, unknown> | unknown[] | string | number | boolean | null;
  source: string;
  version: number;
  createdAt: string;
  updatedAt: string;
}

export interface AgentTaskArtifactVersion {
  id: string;
  artifactId: string;
  version: number;
  title: string;
  summary?: string | null;
  content: string;
  paths: string[];
  payload?: Record<string, unknown> | unknown[] | string | number | boolean | null;
  createdAt: string;
}

export interface CreateAgentTaskArtifactInput {
  kind: string;
  title: string;
  summary?: string | null;
  content: string;
  paths: string[];
  payload?: Record<string, unknown> | unknown[] | string | number | boolean | null;
  source?: string | null;
}

export interface UpdateAgentTaskArtifactInput {
  title: string;
  summary?: string | null;
  content: string;
  paths: string[];
  payload?: Record<string, unknown> | unknown[] | string | number | boolean | null;
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
  /** Optional model for image generation. */
  imageGenerationModel: string | null;
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
  /** Optional model for image generation. */
  imageGenerationModel: string | null;
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
  autoMemoryExtraction?: boolean;
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
    | 'toolCallPreparing'
    | 'toolCallStart'
    | 'toolCallArgsDelta'
    | 'toolCallProgress'
    | 'toolCallResult'
    | 'toolRunStarted'
    | 'toolRunUpdated'
    | 'toolRunCompleted'
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
  /** Number of argument characters assembled when the backend entered preparing state. */
  argsBytes?: number;
  arguments?: string;
  /** Legacy appended arguments fragment for streaming tool calls. */
  argumentsDelta?: string;
  /** Optional ordering index for argument deltas. */
  index?: number;
  /** Progress heartbeat note from a long-running tool. */
  note?: string;
  run?: ToolRunItem;
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

export interface ToolAccessInfo {
  name: string;
  category: string;
  canRead: boolean;
  canWrite: boolean;
  canExecute: boolean;
  canAccessNetwork: boolean;
  needsApproval: boolean;
  riskLevel: ApprovalRisk;
  riskReason: string;
}

export interface ApprovalRequest {
  id: string;
  toolName: string;
  argumentsPreview: string;
  riskLevel: ApprovalRisk;
  reason: string;
  checkpointPreview?: {
    planned: boolean;
    targetPaths: string[];
    note: string;
  } | null;
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
  argsBytes?: number;
  arguments?: string;
  argumentsDelta?: string;
  index?: number;
  note?: string;
  run?: ToolRunItem;
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

export type ToolRunStatus =
  | 'preparing'
  | 'approvalPending'
  | 'running'
  | 'completed'
  | 'failed'
  | 'declined'
  | 'cancelled'
  | 'timedOut';

export type ToolRenderKind =
  | 'generic'
  | 'commandExecution'
  | 'fileChange'
  | 'search'
  | 'subagent'
  | 'image'
  | 'plan'
  | 'verification'
  | 'mcp';

export type ToolInputStreamingMode = 'none' | 'uiPreview' | 'toolConsumesPartial';
export type ToolInterruptBehavior = 'block' | 'cancel';

export interface ToolRunCapabilities {
  inputStreaming: ToolInputStreamingMode;
  renderKind: ToolRenderKind;
  readOnly: boolean;
  destructive: boolean;
  concurrencySafe: boolean;
  interruptBehavior: ToolInterruptBehavior;
  resourceKeys: string[];
}

export interface ToolRunItem {
  callId: string;
  toolName: string;
  status: ToolRunStatus;
  arguments?: string;
  renderKind: ToolRenderKind;
  capabilities: ToolRunCapabilities;
  content?: string;
  isError?: boolean;
  artifacts?: ArtifactPayload;
  progressNote?: string;
  durationMs?: number;
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

export interface CheckpointBranch {
  conversation: Conversation;
  sourceCheckpoint: Checkpoint;
  messageCount: number;
}

export interface FileCheckpoint {
  id: string;
  conversationId: string | null;
  toolCallId: string;
  toolName: string;
  operation: string;
  path: string;
  absolutePath: string;
  existedBefore: boolean;
  bytesBefore: number;
  hashBefore: string | null;
  createdAt: string;
}

export interface FileCheckpointRestore {
  checkpoint: FileCheckpoint;
  action: string;
  bytesWritten: number;
}

export interface UserMemory {
  id: string;
  content: string;
  source?: 'manual' | 'auto_extracted';
  createdAt: string;
  updatedAt: string;
}

export interface AgentProceduralMemory {
  id: string;
  title: string;
  content: string;
  tags: string[];
  source: string;
  confidence: number;
  createdAt: string;
  updatedAt: string;
}
