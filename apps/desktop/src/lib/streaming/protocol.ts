import type {
  AgentTaskRun,
  AgentTaskRunEvent,
  ApprovalRequest,
  ArtifactPayload,
  ToolPluginInfo,
  ToolRenderKind,
  ToolRunCapabilities,
} from '../../types/conversation';

export interface ToolCallEvent {
  callId: string;
  toolName: string;
  plugin?: ToolPluginInfo;
  arguments: string;
  status:
    | 'preparing'
    | 'starting'
    | 'approvalPending'
    | 'running'
    | 'done'
    | 'error'
    | 'declined'
    | 'cancelled'
    | 'timedOut';
  renderKind?: ToolRenderKind;
  capabilities?: ToolRunCapabilities;
  /** Assembly progress of `arguments` before execution. */
  argsStatus: 'pending' | 'streaming' | 'ready' | 'done' | 'error';
  /** Number of characters received for `arguments` so far. */
  argsBytes: number;
  content?: string;
  isError?: boolean;
  artifacts?: ArtifactPayload;
  durationMs?: number;
}

export interface StreamRoundEvent {
  id: string;
  thinking?: string;
  reply: string;
  toolCalls: ToolCallEvent[];
}

export interface TraceThinkingEvent {
  id: string;
  kind: 'thinking';
  text: string;
  blockId?: string;
  nextOffset?: number;
}

export interface TraceReplyEvent {
  id: string;
  kind: 'reply';
  text: string;
  blockId?: string;
  nextOffset?: number;
}

export interface TraceToolEvent {
  id: string;
  kind: 'tool';
  toolCall: ToolCallEvent;
}

export interface TraceStatusEvent {
  id: string;
  kind: 'status';
  text: string;
  tone?: 'muted' | 'success' | 'error';
}

export type TraceEvent = TraceThinkingEvent | TraceReplyEvent | TraceToolEvent | TraceStatusEvent;

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
  lastPromptTokens?: number;
  contextBreakdown?: ContextUsageBreakdown;
}

export interface StreamState {
  isStreaming: boolean;
  streamText: string;
  streamRounds: StreamRoundEvent[];
  traceEvents: TraceEvent[];
  thinkingText: string;
  isThinking: boolean;
  toolCalls: ToolCallEvent[];
  error: string | null;
  lastUsage: UsageTotal | null;
  lastCached: boolean;
  finishReason: string | null;
  contextOverflow: boolean;
  rateLimited: boolean;
  autoCompacted: { summary: string } | null;
  /** High-risk tool calls awaiting GUI approval. FIFO queue. */
  pendingApprovals: ApprovalRequest[];
  /** Durable task run currently associated with this stream. */
  taskRun: AgentTaskRun | null;
  /** Recent lifecycle events for the active task run. */
  taskEvents: AgentTaskRunEvent[];
}
