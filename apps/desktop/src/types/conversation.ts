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
  /** Timestamp set while the conversation is hidden from the active sidebar. */
  archivedAt?: string | null;
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

export type AgentRunPhase =
  | 'routing'
  | 'planning'
  | 'responding'
  | 'tooling'
  | 'approval'
  | 'compacting'
  | 'accounting'
  | 'done';

export type AgentRunEventKind =
  | 'outputDelta'
  | 'streamReset'
  | 'thinking'
  | 'status'
  | 'planUpdated'
  | 'toolPreparing'
  | 'toolStarted'
  | 'toolProgress'
  | 'toolCompleted'
  | 'approvalRequested'
  | 'approvalResolved'
  | 'recoveryAttempt'
  | 'usageUpdated'
  | 'autoCompacted'
  | 'done'
  | 'error';

export type AgentRunEventVisibility = 'user' | 'developer' | 'internal';
export type AgentRunEventPersistence = 'durable' | 'ephemeral';
export type AgentRunDisplayKind =
  | 'output'
  | 'reasoning'
  | 'status'
  | 'plan'
  | 'tool'
  | 'approval'
  | 'recovery'
  | 'steering'
  | 'usage'
  | 'compaction'
  | 'completion'
  | 'error';
export type AgentRunEventImportance = 'low' | 'normal' | 'high';

export interface AgentRunEvent {
  version: number;
  runId: string;
  turnId: string;
  eventSeq: number;
  kind: AgentRunEventKind;
  phase: AgentRunPhase;
  /** Missing only on events persisted before protocol v2 metadata shipped. */
  visibility?: AgentRunEventVisibility;
  persistence?: AgentRunEventPersistence;
  displayKind?: AgentRunDisplayKind;
  importance?: AgentRunEventImportance;
  label: string;
  status?: string | null;
  payload: ArtifactPayload | null;
  createdAt?: string | null;
}

export type RuntimeTerminalStatus = 'completed' | 'failed' | 'cancelled' | 'timed_out' | 'paused';

export type AgentTurnState =
  | 'starting'
  | 'running'
  | 'waitingApproval'
  | { terminal: RuntimeTerminalStatus };

/** Immediate runtime acknowledgement returned by `agent_chat_cmd`. */
export interface AgentTurnHandle {
  sessionId: string;
  runId: string;
  turnId: string;
  state: AgentTurnState;
}

export type TaskTimelineEventKind = 'subtask' | 'verification';

export interface TaskTimelineEvent {
  version: number;
  kind: TaskTimelineEventKind;
  label: string;
  status?: string | null;
  payload: ArtifactPayload | null;
}

export interface AgentTaskRunEvent {
  id: string;
  runId: string;
  eventType: string;
  label: string;
  status?: string | null;
  payload?: (Record<string, unknown> & {
    taskTimeline?: TaskTimelineEvent;
  }) | unknown[] | null;
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
  owner?: CapabilityOwner;
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
  /** Stable Model Catalog v2 endpoint identity. */
  providerEndpointId?: string | null;
  /** Canonical model identity retained alongside the legacy `model` field. */
  modelId?: string | null;
  modelSelectionResolution?: {
    providerId: string;
    providerEndpointId?: string | null;
    modelId: string;
    kind: 'unchanged' | 'alias' | 'replacement' | 'unverified';
    requiresUserNotice: boolean;
  } | null;
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
  providerEndpointId?: string | null;
  modelId?: string | null;
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
  cacheTtlHours: number;
  defaultSearchLimit: number;
  minSearchSimilarity: number;
  maxTextFileSize: number;
  maxVideoFileSize: number;
  maxAudioFileSize: number;
  dynamicToolVisibility?: boolean;
  toolVisibilityDefaultsVersion?: number;
  traceEnabled?: boolean;
  windowCloseBehavior?: 'exit' | 'minimize_to_tray';
  localModelRoot?: string;
  confirmDestructive?: boolean;
  shellAccessMode?: 'restricted' | 'confirm_all' | 'open';
  toolApprovalMode?: 'ask' | 'allow_all' | 'deny_all';
  autoMemoryExtraction?: boolean;
  autoSkillLearning?: boolean;
  hfMirrorBaseUrl?: string;
  ghproxyBaseUrl?: string;
  imageGeneration?: ImageGenerationConfig;
  textToSpeech?: TextToSpeechConfig;
  speechToText?: SpeechToTextConfig;
  webSearch?: WebSearchConfig;
  dreaming?: DreamingConfig;
}

export interface DreamingConfig {
  enabled: boolean;
  idle: boolean;
  afterScan: boolean;
  afterSuccessfulTurn: boolean;
  schedule: boolean;
  idleIntervalMinutes: number;
  scheduleIntervalMinutes: number;
  maxArtifactsPerRun: number;
  maxRunsPerDay: number;
  localOnly: boolean;
  sourceIds: string[];
  projectIds: string[];
}

export type WebSearchProviderProfile = 'default' | 'free' | 'free_verified' | 'max_evidence';
export type WebSearchReranker = 'auto' | 'none' | 'docs_first' | 'research' | 'news_balanced';
export type WebSearchProviderHealth = 'healthy' | 'degraded' | 'blocked' | 'disabled';
export type WebSearchProviderMode = 'built_in_first' | 'custom_first' | 'custom_only';
export type WebSearchCustomProviderPreset =
  | 'brave'
  | 'tavily'
  | 'anysearch'
  | 'serpapi_google'
  | 'searxng';
export type WebSearchEngine =
  | 'baidu'
  | 'sogou'
  | 'google'
  | 'bing'
  | 'duckduckgo'
  | 'brave'
  | 'tavily'
  | 'anysearch'
  | 'serpapi_google'
  | 'searxng';

export interface WebSearchConfig {
  providerProfile: WebSearchProviderProfile;
  reranker: WebSearchReranker;
  providerMode: WebSearchProviderMode;
  customProviders: WebSearchCustomProviderConfig[];
}

export interface WebSearchCustomProviderConfig {
  id: string;
  preset: WebSearchCustomProviderPreset;
  name: string;
  enabled: boolean;
  apiKey: string;
  baseUrl: string | null;
  priority: number;
}

export interface WebSearchProviderStatus {
  engine: WebSearchEngine;
  id: string;
  label: string;
  health: WebSearchProviderHealth;
  builtIn: boolean;
  enabledByProfile: boolean;
  enabled: boolean;
  configured: boolean;
  requiresApiKey: boolean;
  requiresBaseUrl: boolean;
  lastErrorCode?: string;
  nextRetrySeconds?: number;
}

export interface ImageGenerationConfig {
  provider: string;
  apiStyle: string;
  apiKey: string;
  baseUrl: string | null;
  model: string;
  size: string | null;
  quality: string | null;
  outputFormat: string | null;
}

export interface TextToSpeechConfig {
  provider: string;
  apiStyle: string;
  apiKey: string;
  baseUrl: string | null;
  model: string;
  voice: string;
  outputFormat: string;
  speed: number;
  executablePath?: string | null;
  modelPath?: string | null;
  tokensPath?: string | null;
  voicesPath?: string | null;
  dataDir?: string | null;
  lexiconPath?: string | null;
  numThreads?: number;
  autoSpeakFinalAnswers?: boolean;
}

export interface SpeechToTextConfig {
  provider: string;
  apiStyle: 'local_whisper' | 'openai_transcription' | 'sherpa_onnx' | string;
  apiKey: string;
  baseUrl: string | null;
  model: string;
  language: string | null;
  executablePath?: string | null;
  sherpaModelFamily?: 'sense_voice' | 'zipformer' | string;
  modelPath?: string | null;
  tokensPath?: string | null;
  encoderPath?: string | null;
  decoderPath?: string | null;
  joinerPath?: string | null;
  numThreads?: number;
}

export type ProviderType =
  | 'open_ai'
  | 'openrouter'
  | 'anthropic'
  | 'google'
  | 'deep_seek'
  | 'ollama'
  | 'lm_studio'
  | 'azure_open_ai'
  | 'zhipu'
  | 'moonshot'
  | 'qwen'
  | 'alibaba_model_studio'
  | 'siliconflow'
  | 'doubao'
  | 'yi'
  | 'baichuan'
  | 'custom';

export interface ContextUsageSegment {
  kind: string;
  tokens: number;
}

export interface ContextUsageBreakdown {
  totalTokens: number;
  segments: ContextUsageSegment[];
}

export interface UsageTotal {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  thinkingTokens?: number;
  toolPromptTokens?: number;
  cacheReadTokens?: number;
  cacheMissTokens?: number;
  cacheCreationTokens?: number;
  lastPromptTokens?: number;
  contextBreakdown?: ContextUsageBreakdown;
}

export interface UsageSnapshot {
  source: 'provider' | 'normalized' | 'estimated';
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  thinkingTokens: number;
  cacheReadTokens: number;
  cacheMissTokens: number;
  cacheCreationTokens: number;
  lastPromptTokens: number;
  contextBreakdown?: ContextUsageBreakdown;
  providerRaw: unknown;
}

export interface AgentEvent {
  type:
    | 'textDelta'
    | 'streamBlockDelta'
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
    | 'steering'
    | 'done'
    | 'error'
    | 'autoCompacted'
    | 'usageUpdate'
    | 'approvalRequested'
    | 'approvalResolved'
    | 'taskRunUpdated'
    | 'taskRunEvent';
  delta?: string;
  blockId?: string;
  channel?: 'answer' | 'thinking';
  offset?: number;
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
  usageTotal?: UsageTotal;
  contextBreakdown?: ContextUsageBreakdown;
  taskRun?: AgentTaskRun;
  taskEvent?: AgentTaskRunEvent;
}

export type ApprovalRisk = 'low' | 'medium' | 'high';
export type ApprovalDecisionValue = 'allow_once' | 'allow_session' | 'deny' | 'never';

export interface CapabilityOwner {
  id: string;
  name: string;
  capability: string;
  description: string;
}

export type EcosystemSurfaceKind =
  | 'core_platform'
  | 'capability_package'
  | 'connector'
  | 'skill_package'
  | 'workflow_package'
  | 'adapter'
  | 'host_surface'
  | 'native_plugin';

export interface CapabilityProviderCatalog {
  id: string;
  label: string;
  itemKind: string;
  items: unknown[];
}

export interface CapabilitySettingsSchema {
  configKey: string;
  fields: CapabilitySettingsField[];
}

export interface CapabilitySettingsField {
  key: string;
  label: string;
  kind: string;
  required: boolean;
  secret: boolean;
  description: string;
  optionsSource?: string;
  defaultValue?: unknown;
}

export type CapabilityRuntimeStatus = 'pass' | 'warning' | 'error' | 'unknown';
export type CapabilityCheckSeverity = 'info' | 'warning' | 'error';

export interface CapabilityRuntimeCheck {
  id: string;
  label: string;
  status: CapabilityRuntimeStatus;
  severity: CapabilityCheckSeverity;
  message: string;
}

export interface CapabilityPackagePermissions {
  read: boolean;
  write: boolean;
  execute: boolean;
  network: boolean;
  nativeCode: boolean;
}

export interface CapabilityPackageView extends CapabilityOwner {
  builtIn: boolean;
  surface: EcosystemSurfaceKind;
  version: number;
  tools: string[];
  skills: string[];
  settingsSurfaces: string[];
  workflows: string[];
  permissions: CapabilityPackagePermissions;
  settingsSchema?: CapabilitySettingsSchema | null;
  providerCatalogs?: CapabilityProviderCatalog[];
  runtimeChecks?: CapabilityRuntimeCheck[];
}

export type PackageSurfaceKind =
  | 'capability'
  | 'connector'
  | 'skill'
  | 'workflow'
  | 'nativePlugin';

export type PackageLifecycleState =
  | 'discovered'
  | 'validated'
  | 'enabled'
  | 'disabled'
  | 'unhealthy'
  | 'blocked';

export type PackageHealthState = 'healthy' | 'warning' | 'unhealthy';

export interface PackagePermission {
  key: string;
  description: string;
}

export interface PackageComponent {
  id: string;
  packageId: string;
  kind: PackageSurfaceKind;
  enabled: boolean;
}

export interface PackageHostRecord {
  id: string;
  version?: string | null;
  state: PackageLifecycleState;
  health: PackageHealthState;
  dependencies: string[];
  permissions: PackagePermission[];
  components: PackageComponent[];
}

export interface PackageHostSnapshot {
  version: number;
  records: PackageHostRecord[];
}

export interface ToolAccessInfo {
  name: string;
  owner: CapabilityOwner;
  category: string;
  canRead: boolean;
  canWrite: boolean;
  canExecute: boolean;
  canAccessNetwork: boolean;
  needsApproval: boolean;
  riskLevel: ApprovalRisk;
  riskReason: string;
  renderKind: ToolRenderKind;
  inputStreaming: ToolInputStreamingMode;
  readOnly: boolean;
  destructive: boolean;
  concurrencySafe: boolean;
  interruptBehavior: ToolInterruptBehavior;
  resourceKeys: string[];
}

export interface ApprovalRequest {
  id: string;
  toolName: string;
  permissionKey: string;
  targetKind: string;
  targetValue: string;
  argumentsPreview: string;
  riskLevel: ApprovalRisk;
  reason: string;
  checkpointPreview?: {
    planned: boolean;
    targetPaths: string[];
    note: string;
  } | null;
}

export interface ToolPermissionPolicy {
  toolName: string;
  decision: string;
  createdAt?: string;
  permissionKey: string;
  targetKind: string;
  targetValue: string;
}

export interface ToolPermissionPolicyList {
  persisted: ToolPermissionPolicy[];
  session: ToolPermissionPolicy[];
}

export interface AgentFrontendEvent {
  conversationId: string;
  runEvent?: AgentRunEvent;
  type?: AgentEvent['type'];
  eventSeq?: number;
  summary?: string;
  delta?: string;
  blockId?: string;
  channel?: 'answer' | 'thinking';
  offset?: number;
  reason?: string;
  callId?: string;
  toolName?: string;
  argsBytes?: number;
  arguments?: string;
  argumentsDelta?: string;
  index?: number;
  note?: string;
  activity?: ActivityEvent;
  run?: ToolRunItem;
  content?: string;
  tone?: 'muted' | 'success' | 'error';
  isError?: boolean;
  artifacts?: ArtifactPayload;
  message?: ConversationMessage | string;
  usageTotal?: UsageTotal;
  contextBreakdown?: ContextUsageBreakdown;
  request?: ApprovalRequest;
  requestId?: string;
  decision?: ApprovalDecisionValue;
  taskRun?: AgentTaskRun;
  taskEvent?: AgentTaskRunEvent;
}

export type ActivityEventKind =
  | 'started'
  | 'stdout_chunk'
  | 'stderr_chunk'
  | 'progress'
  | 'ready_url'
  | 'cwd_changed'
  | 'command_started'
  | 'command_finished'
  | 'prompt_detected'
  | 'input_requested'
  | 'browser_observation'
  | 'desktop_observation'
  | 'state_changed'
  | 'completed'
  | 'failed'
  | 'cancelled';

export interface ActivityEvent {
  activityId: string;
  seq: number;
  timestamp: string;
  kind: ActivityEventKind;
  payload: Record<string, unknown>;
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
  owner: CapabilityOwner;
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
  source?: 'manual' | 'auto_extracted' | 'dream' | 'imported';
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
