import { useState, useCallback, useRef, useEffect } from 'react';
import * as api from './api';
import { streamStore } from './streamStore';
import type {
  ImageAttachment,
  ApprovalRequest,
  ArtifactPayload,
  UsageTotal,
} from '../types/conversation';
import type { StreamState } from './streamStore';
import type { StreamRoundEvent, ToolCallEvent, TraceEvent } from './streaming/protocol';
import type {
  AgentCollaborationMode,
  AgentExecutionMode,
  AgentPowerMode,
  CustomOrchestrationOptions,
  MoaPresetId,
  OrchestrationProfile,
} from './api';

type AutoCompactedInfo = { summary: string } | null;

// Stable references for empty collections (avoids re-renders)
const EMPTY_ROUNDS: StreamRoundEvent[] = [];
const EMPTY_TOOLS: ToolCallEvent[] = [];
const EMPTY_TRACE_EVENTS: TraceEvent[] = [];
const EMPTY_APPROVALS: ApprovalRequest[] = [];
const EMPTY_TASK_EVENTS: NonNullable<StreamState['taskEvents']> = [];

interface UseAgentStreamReturn {
  send: (
    conversationId: string,
    message: string,
    attachments?: ImageAttachment[],
    agentConfigId?: string | null,
    personaId?: string | null,
    skillIds?: string[],
    executionMode?: AgentExecutionMode | null,
    powerMode?: AgentPowerMode | null,
    collaborationMode?: AgentCollaborationMode | null,
    moaPreset?: MoaPresetId | null,
    orchestrationProfile?: OrchestrationProfile | null,
    customOrchestration?: CustomOrchestrationOptions | null,
    userArtifacts?: ArtifactPayload | null,
    taskOrchestratorRunId?: string | null,
  ) => Promise<void>;
  stop: (conversationId: string) => Promise<void>;
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
  connectionState: StreamState['connectionState'];
  autoCompacted: AutoCompactedInfo;
  pendingApprovals: ApprovalRequest[];
  taskRun: StreamState['taskRun'];
  taskEvents: StreamState['taskEvents'];
  turnHandle: StreamState['turnHandle'];
  turnTiming: StreamState['turnTiming'];
  clearPreview: () => void;
  reset: () => void;
}

/**
 * Hook that reads/writes stream state from the global StreamStore.
 *
 * @param watchConversationId  Optional conversation to watch — when provided,
 *   the hook returns that conversation's streaming state from the store.
 *   Falls back to the conversation set by the last `send()` call.
 */
export function useAgentStream(watchConversationId?: string | null): UseAgentStreamReturn {
  const [storeState, setStoreState] = useState<StreamState | null>(() => {
    if (watchConversationId) {
      return streamStore.getStream(watchConversationId) ?? null;
    }
    return null;
  });

  const watchIdRef = useRef(watchConversationId);
  const activeConversationRef = useRef<string | null>(watchConversationId ?? null);

  // Sync when watched conversation changes externally
  useEffect(() => {
    watchIdRef.current = watchConversationId ?? null;
    if (watchConversationId) {
      setStoreState(streamStore.getStream(watchConversationId) ?? null);
    } else if (!activeConversationRef.current) {
      setStoreState(null);
    }
  }, [watchConversationId]);

  // Subscribe to store — update React state when watched conversation changes
  useEffect(() => {
    return streamStore.subscribe((changedId) => {
      const convId = watchIdRef.current ?? activeConversationRef.current;
      if (!convId || changedId !== convId) return;
      const next = streamStore.getStream(convId) ?? null;
      setStoreState(prev => {
        if (prev === null && next === null) return prev;
        if (prev === null || next === null) return next;
        if (
          prev.isStreaming === next.isStreaming &&
          prev.turnHandle === next.turnHandle &&
          prev.streamText === next.streamText &&
          prev.thinkingText === next.thinkingText &&
          prev.isThinking === next.isThinking &&
          prev.streamRounds === next.streamRounds &&
          prev.toolCalls === next.toolCalls &&
          prev.traceEvents === next.traceEvents &&
          prev.error === next.error &&
          prev.lastUsage === next.lastUsage &&
          prev.lastCached === next.lastCached &&
          prev.finishReason === next.finishReason &&
          prev.contextOverflow === next.contextOverflow &&
          prev.rateLimited === next.rateLimited &&
          prev.connectionState === next.connectionState &&
          prev.autoCompacted === next.autoCompacted &&
          prev.pendingApprovals === next.pendingApprovals &&
          prev.taskRun === next.taskRun &&
          prev.taskEvents === next.taskEvents &&
          prev.turnTiming === next.turnTiming
        ) return prev;
        return next;
      });
    });
  }, []);

  const send = useCallback(async (
    conversationId: string,
    message: string,
    attachments?: ImageAttachment[],
    agentConfigId?: string | null,
    personaId?: string | null,
    skillIds?: string[],
    executionMode?: AgentExecutionMode | null,
    powerMode?: AgentPowerMode | null,
    collaborationMode?: AgentCollaborationMode | null,
    moaPreset?: MoaPresetId | null,
    orchestrationProfile?: OrchestrationProfile | null,
    customOrchestration?: CustomOrchestrationOptions | null,
    userArtifacts?: ArtifactPayload | null,
    taskOrchestratorRunId?: string | null,
  ) => {
    activeConversationRef.current = conversationId;
    streamStore.startStream(conversationId);

    try {
      const handle = await api.agentChat(
        conversationId,
        message,
        attachments,
        agentConfigId,
        personaId,
        skillIds,
        executionMode,
        powerMode,
        collaborationMode,
        moaPreset,
        orchestrationProfile,
        customOrchestration,
        userArtifacts,
        taskOrchestratorRunId,
      );
      streamStore.bindTurnHandle(conversationId, handle);
    } catch (err) {
      streamStore.sendError(conversationId, String(err));
    }
  }, []);

  const stop = useCallback(async (conversationId: string) => {
    try {
      await api.agentStop(conversationId);
    } catch { /* ignore */ }
    streamStore.stopStream(conversationId);
  }, []);

  const clearPreview = useCallback(() => {
    const convId = watchIdRef.current ?? activeConversationRef.current;
    if (convId) streamStore.clearPreview(convId);
  }, []);

  const reset = useCallback(() => {
    const convId = watchIdRef.current ?? activeConversationRef.current;
    if (convId) streamStore.clearStream(convId);
    activeConversationRef.current = null;
  }, []);

  // Effects synchronize the subscription after a route change. Reading the
  // watched stream directly prevents one stale render from showing the prior
  // conversation's run state in the meantime.
  const resolvedState = watchConversationId
    ? streamStore.getStream(watchConversationId) ?? null
    : storeState;

  return {
    send,
    stop,
    turnHandle: resolvedState?.turnHandle ?? null,
    turnTiming: resolvedState?.turnTiming ?? null,
    isStreaming: resolvedState?.isStreaming ?? false,
    streamText: resolvedState?.streamText ?? '',
    streamRounds: resolvedState?.streamRounds ?? EMPTY_ROUNDS,
    traceEvents: resolvedState?.traceEvents ?? EMPTY_TRACE_EVENTS,
    thinkingText: resolvedState?.thinkingText ?? '',
    isThinking: resolvedState?.isThinking ?? false,
    toolCalls: resolvedState?.toolCalls ?? EMPTY_TOOLS,
    error: resolvedState?.error ?? null,
    lastUsage: resolvedState?.lastUsage ?? null,
    lastCached: resolvedState?.lastCached ?? false,
    finishReason: resolvedState?.finishReason ?? null,
    contextOverflow: resolvedState?.contextOverflow ?? false,
    rateLimited: resolvedState?.rateLimited ?? false,
    connectionState: resolvedState?.connectionState ?? null,
    autoCompacted: resolvedState?.autoCompacted ?? null,
    pendingApprovals: resolvedState?.pendingApprovals ?? EMPTY_APPROVALS,
    taskRun: resolvedState?.taskRun ?? null,
    taskEvents: resolvedState?.taskEvents ?? EMPTY_TASK_EVENTS,
    clearPreview,
    reset,
  };
}

/** Subscribe to the global run registry without coupling it to the open chat. */
export function useRunningConversationIds(): ReadonlySet<string> {
  const [runningIds, setRunningIds] = useState<ReadonlySet<string>>(
    () => new Set(streamStore.getRunningConversationIds()),
  );

  useEffect(() => streamStore.subscribe(() => {
    const nextIds = streamStore.getRunningConversationIds();
    setRunningIds((previous) => {
      if (previous.size === nextIds.length && nextIds.every(id => previous.has(id))) {
        return previous;
      }
      return new Set(nextIds);
    });
  }), []);

  return runningIds;
}
