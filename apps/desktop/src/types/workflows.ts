import type { AgentTaskRun } from './conversation';
import type { TaskOrchestratorQueueItem, TaskOrchestratorRun } from './trace';

export type WorkflowAutomationTrigger =
  | { kind: 'manual' }
  | { kind: 'schedule'; cron: string }
  | { kind: 'folder'; path: string; pattern: string };

export interface WorkflowAutomationApprovalPolicy {
  requireBeforeRun: boolean;
  allowedTools: string[];
  riskLevel: string;
}

export type WorkflowScheduleMisfirePolicy = 'run_latest' | 'skip';
export type WorkflowScheduleOverlapPolicy = 'skip' | 'allow';
export type WorkflowScheduleWorkspacePolicy = 'deny_writes' | 'isolated_patch';

export interface WorkflowAutomationExecutionPolicy {
  projectId?: string | null;
  workspacePolicy: WorkflowScheduleWorkspacePolicy;
  sourceRootFingerprint?: string | null;
  agentConfigId?: string | null;
  provider?: string | null;
  providerEndpointId?: string | null;
  model?: string | null;
  /** Null means use the provider/model route default. */
  contextWindow?: number | null;
  powerMode: 'standard' | 'nexus';
  orchestrationProfile: string;
  collaborationMode: string;
  executionMode?: string | null;
}

export interface WorkflowAutomationScheduleConfig {
  version: number;
  timezone: string;
  misfirePolicy: WorkflowScheduleMisfirePolicy;
  misfireGraceSeconds: number;
  overlapPolicy: WorkflowScheduleOverlapPolicy;
  executionPolicy: WorkflowAutomationExecutionPolicy;
  legacyNeedsReview: boolean;
}

export interface WorkflowScheduleOccurrencePreview {
  scheduledFor: string;
  localTime: string;
}

export interface WorkflowSchedulePreview {
  cron: string;
  timezone: string;
  occurrences: WorkflowScheduleOccurrencePreview[];
}

export interface SaveWorkflowAutomationInput {
  id?: string | null;
  name: string;
  description: string;
  workflowTemplateId: string;
  prompt: string;
  trigger: WorkflowAutomationTrigger;
  sourceScope: string[];
  approvalPolicy: WorkflowAutomationApprovalPolicy;
  scheduleConfig?: WorkflowAutomationScheduleConfig;
  enabled: boolean;
}

export interface WorkflowAutomation {
  id: string;
  name: string;
  description: string;
  workflowTemplateId: string;
  prompt: string;
  triggerKind: string;
  trigger: WorkflowAutomationTrigger;
  sourceScope: string[];
  approvalPolicy: WorkflowAutomationApprovalPolicy;
  scheduleConfig: WorkflowAutomationScheduleConfig;
  enabled: boolean;
  status: string;
  lastRunAt?: string | null;
  nextRunAt?: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface WorkflowAutomationDueRun {
  automation: WorkflowAutomation;
  prompt: string;
  dueReason: string;
  scheduledFor?: string | null;
  origin: 'schedule' | 'manual_run_now';
}

export interface WorkflowAutomationRun {
  id: string;
  automationId: string;
  taskRunId?: string | null;
  status: string;
  summary?: string | null;
  occurrenceId?: string | null;
  scheduledFor?: string | null;
  definitionRevision: number;
  attempt: number;
  createdAt: string;
  finishedAt?: string | null;
}

export interface WorkflowAutomationSchedulerEvent {
  id: string;
  automationId?: string | null;
  runId?: string | null;
  eventType: string;
  status?: string | null;
  summary: string;
  payload: Record<string, unknown> | unknown[] | null;
  createdAt: string;
}

export interface TaskOrchestratorDeliveryEnvelope {
  version: number;
  queueItem: TaskOrchestratorQueueItem;
  prompt: string;
}

export interface TaskOrchestratorExecutionTicket {
  version: number;
  delivery: TaskOrchestratorDeliveryEnvelope;
  run: TaskOrchestratorRun;
}

export interface TaskOrchestratorWorkflowLaunch {
  ticket: TaskOrchestratorExecutionTicket;
  conversationId: string;
  taskRunId: string;
  taskOrchestratorRunId: string;
}

export type TaskOrchestratorWorkflowStartOutcome =
  | { status: 'launched'; launch: TaskOrchestratorWorkflowLaunch }
  | { status: 'pending_approval'; run: WorkflowAutomationRun }
  | { status: 'skipped'; reason: string };

export interface TaskResumeCheckpoint {
  id: string;
  runId: string;
  reason: string;
  status: string;
  phase: string;
  state: Record<string, unknown> | unknown[] | null;
  resumePrompt: string;
  createdAt: string;
}

export interface TaskResumePrompt {
  run: AgentTaskRun;
  checkpoint: TaskResumeCheckpoint;
  prompt: string;
}

export interface SkillUsageStats {
  skillId: string;
  name: string;
  enabled: boolean;
  usageCount: number;
  successCount: number;
  failureCount: number;
  lastUsedAt?: string | null;
  recentFailureEvidence?: unknown;
  disableRecommended: boolean;
}

export interface LearningGovernanceSnapshot {
  skillStats: SkillUsageStats[];
  pendingProposals: number;
  proceduralMemoryCount: number;
  memoryInjectionCount: number;
  recommendations: string[];
}

export interface InvestigationGraph {
  runId: string;
  nodes: InvestigationGraphNode[];
  edges: InvestigationGraphEdge[];
  citations: string[];
  openQuestions: string[];
}

export interface InvestigationGraphNode {
  id: string;
  nodeType: string;
  label: string;
  summary?: string | null;
  status?: string | null;
  sourceUrl?: string | null;
  createdAt?: string | null;
}

export interface InvestigationGraphEdge {
  from: string;
  to: string;
  label: string;
}

export interface BrowserEvidenceCapture {
  id: string;
  url: string;
  finalUrl: string;
  title: string;
  excerpt: string;
  method: string;
  payload: Record<string, unknown> | unknown[] | null;
  createdAt: string;
}
