import type { AgentRunEvent } from './conversation';

export interface TraceSummary {
  totalSessions: number;
  totalToolCalls: number;
  totalInputTokens: number;
  totalOutputTokens: number;
  avgIterationsPerSession: number;
  avgToolsPerSession: number;
  avgContextUsagePct: number;
  successRate: number;
  cacheHitRate: number;
  topTools: [string, number][];
  sessionsLast7Days: number;
  tokensLast7Days: number;
}

export interface AgentTrace {
  id: string;
  conversationId: string;
  startedAt: string;
  finishedAt: string | null;
  userMessagePreview: string;
  totalIterations: number;
  totalToolCalls: number;
  totalInputTokens: number;
  totalOutputTokens: number;
  peakContextUsagePct: number;
  toolsOffered: number;
  cacheHit: boolean;
  compactionCount: number;
  outcome: string;
  errorMessage: string | null;
  modelId: string;
}

export type TrajectoryRedactionProfile =
  | 'full_local_private'
  | 'sanitized_local'
  | 'shareable_minimal'
  | 'eval_fixture';

export type RuntimeHostSurface = 'desktop' | 'cli' | 'ide' | 'mcp' | 'acp' | 'gateway' | 'test';

export interface RuntimeSourceScope {
  sourceIds: string[];
  collectionId?: string | null;
  workingDirectory?: string | null;
}

export interface RuntimeSkillContext {
  availableSkillIds: string[];
  loadedSkillIds: string[];
  trustState?: string | null;
}

export interface RuntimePackageContext {
  enabledPackageIds: string[];
  disabledPackageIds: string[];
}

export interface AgentSessionConfig {
  version: number;
  sessionId: string;
  conversationId?: string | null;
  taskRunId?: string | null;
  hostSurface: RuntimeHostSurface;
  provider?: string | null;
  model?: string | null;
  reasoningEnabled?: boolean | null;
  thinkingBudget?: number | null;
  reasoningEffort?: string | null;
  sourceScope: RuntimeSourceScope;
  approvalMode: string;
  shellAccessMode: string;
  executionMode: string;
  traceEnabled: boolean;
  skillContext: RuntimeSkillContext;
  packageContext: RuntimePackageContext;
  metadata: unknown;
}

export type TaskOrchestratorState =
  | 'draft'
  | 'queued'
  | 'running'
  | 'waiting_approval'
  | 'paused'
  | 'resuming'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'timed_out'
  | 'disabled';

export interface TaskRunOwnership {
  userId?: string | null;
  profileId?: string | null;
  sourceScope: string[];
  packageId?: string | null;
  workflowId?: string | null;
  sessionId?: string | null;
}

export interface TaskStatusProjection {
  rawStatus: string;
  state: TaskOrchestratorState;
}

export interface TaskOrchestratorQueueItem {
  version: number;
  queueId: string;
  taskDefinitionId: string;
  state: TaskOrchestratorState;
  ownership: TaskRunOwnership;
  triggerKind: string;
  dueReason: string;
  prompt: string;
  approvalRequired: boolean;
  allowedTools: string[];
  riskLevel?: string | null;
}

export type TaskOrchestratorRunKind = 'agent_task' | 'workflow_automation';

export interface TaskOrchestratorRun {
  version: number;
  runId: string;
  taskRunId?: string | null;
  taskDefinitionId?: string | null;
  kind: TaskOrchestratorRunKind;
  status: TaskStatusProjection;
  ownership: TaskRunOwnership;
  triggerKind?: string | null;
  approvalRequired: boolean;
  allowedTools: string[];
  riskLevel?: string | null;
  summary?: string | null;
  createdAt?: string | null;
  finishedAt?: string | null;
}

export interface TrajectoryMetrics {
  eventCount: number;
  toolCallCount: number;
  approvalCount: number;
  taskQueueItemCount: number;
  taskRunCount: number;
}

export interface TrajectorySanitizationReport {
  profile: TrajectoryRedactionProfile;
  redactedFields: string[];
}

export interface Trajectory {
  trajectoryId: string;
  schemaVersion: number;
  createdAt: string;
  productVersion?: string | null;
  sessionConfig: AgentSessionConfig;
  userInputSummary: string;
  rawUserInput?: string | null;
  toolsOffered: string[];
  skillsAvailable: string[];
  skillsActivated: string[];
  approvals: unknown[];
  taskQueueItems: TaskOrchestratorQueueItem[];
  taskRuns: TaskOrchestratorRun[];
  runEvents: AgentRunEvent[];
  toolCalls: unknown[];
  retrievedEvidence: unknown[];
  finalAnswer?: string | null;
  outcome?: string | null;
  metrics: TrajectoryMetrics;
  sanitization: TrajectorySanitizationReport;
}

export interface TrajectoryStoreSummary {
  trajectoryId: string;
  schemaVersion: number;
  sourceKind: string;
  sourceRunId?: string | null;
  userInputSummary: string;
  outcome?: string | null;
  eventCount: number;
  toolCallCount: number;
  approvalCount: number;
  taskRunCount: number;
  redactionProfile: TrajectoryRedactionProfile;
  createdAt: string;
  updatedAt: string;
}

export type EvalAssertionKind =
  | 'trajectoryAvailability'
  | 'eventOrder'
  | 'toolUse'
  | 'approvalBehavior'
  | 'taskOrchestration'
  | 'evidenceSupport'
  | 'finalAnswerContract';

export interface EvalAssertion {
  kind: EvalAssertionKind;
  description: string;
}

export interface EvalCase {
  id: string;
  name: string;
  trajectoryId?: string | null;
  assertions: EvalAssertion[];
  allowedNondeterminism: string[];
}

export interface EvalPack {
  version: number;
  id: string;
  name: string;
  cases: EvalCase[];
}

export interface EvalFailure {
  caseId: string;
  assertion: EvalAssertionKind;
  message: string;
}

export interface EvalReport {
  packId: string;
  passed: boolean;
  failures: EvalFailure[];
}

export interface StoredTrajectoryEvalCaseReport {
  trajectoryId: string;
  sourceKind: string;
  sourceRunId?: string | null;
  userInputSummary: string;
  passed: boolean;
  failures: EvalFailure[];
  replayTerminalStatus?: RuntimeTerminalStatus | null;
  replayEventCount?: number | null;
}

export interface StoredTrajectoryEvalReport {
  status: string;
  total: number;
  passed: number;
  failed: number;
  cases: StoredTrajectoryEvalCaseReport[];
}

export type TrajectoryReplayCheck =
  | 'runtimeContract'
  | 'eventKindSequence'
  | 'toolCallSequence'
  | 'approvalSequence'
  | 'taskOrchestration'
  | 'evidenceIds'
  | 'finalAnswer'
  | 'outcome';

export interface TrajectoryReplayRequest {
  expectedTrajectoryId: string;
  replayedTrajectoryId: string;
  checks: TrajectoryReplayCheck[];
}

export interface TrajectoryReplayMismatch {
  check: TrajectoryReplayCheck;
  message: string;
  expected: unknown;
  actual: unknown;
}

export interface TrajectoryReplayReport {
  expectedTrajectoryId: string;
  replayedTrajectoryId: string;
  passed: boolean;
  mismatches: TrajectoryReplayMismatch[];
}

export type RuntimeTerminalStatus = 'completed' | 'failed' | 'cancelled' | 'timed_out';

export interface TrajectoryReplayExecution {
  trajectoryId: string;
  runId: string;
  turnId: string;
  terminalStatus: RuntimeTerminalStatus;
  eventCount: number;
  finalMessage?: string | null;
  events: AgentRunEvent[];
}
