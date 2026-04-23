import { useState, useCallback, useRef, useEffect } from 'react';
import * as api from './api';
import { streamStore } from './streamStore';
import type { ImageAttachment, ApprovalRequest } from '../types/conversation';
import type { StreamState } from './streamStore';

// Re-export types from streamStore for backward compatibility
export type { ToolCallEvent, StreamRoundEvent, TraceEvent, UsageTotal } from './streamStore';

type AutoCompactedInfo = { summary: string } | null;
type StreamRoundEvent = import('./streamStore').StreamRoundEvent;
type ToolCallEvent = import('./streamStore').ToolCallEvent;
type TraceEvent = import('./streamStore').TraceEvent;
type UsageTotal = import('./streamStore').UsageTotal;

// Stable references for empty collections (avoids re-renders)
const EMPTY_ROUNDS: StreamRoundEvent[] = [];
const EMPTY_TOOLS: ToolCallEvent[] = [];
const EMPTY_TRACE_EVENTS: TraceEvent[] = [];
const EMPTY_APPROVALS: ApprovalRequest[] = [];

interface UseAgentStreamReturn {
  send: (conversationId: string, message: string, attachments?: ImageAttachment[]) => Promise<void>;
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
  autoCompacted: AutoCompactedInfo;
  pendingApprovals: ApprovalRequest[];
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
          prev.autoCompacted === next.autoCompacted &&
          prev.pendingApprovals === next.pendingApprovals
        ) return prev;
        return next;
      });
    });
  }, []);

  const send = useCallback(async (conversationId: string, message: string, attachments?: ImageAttachment[]) => {
    activeConversationRef.current = conversationId;
    streamStore.startStream(conversationId);

    try {
      await api.agentChat(conversationId, message, attachments);
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

  return {
    send,
    stop,
    isStreaming: storeState?.isStreaming ?? false,
    streamText: storeState?.streamText ?? '',
    streamRounds: storeState?.streamRounds ?? EMPTY_ROUNDS,
    traceEvents: storeState?.traceEvents ?? EMPTY_TRACE_EVENTS,
    thinkingText: storeState?.thinkingText ?? '',
    isThinking: storeState?.isThinking ?? false,
    toolCalls: storeState?.toolCalls ?? EMPTY_TOOLS,
    error: storeState?.error ?? null,
    lastUsage: storeState?.lastUsage ?? null,
    lastCached: storeState?.lastCached ?? false,
    finishReason: storeState?.finishReason ?? null,
    contextOverflow: storeState?.contextOverflow ?? false,
    rateLimited: storeState?.rateLimited ?? false,
    autoCompacted: storeState?.autoCompacted ?? null,
    pendingApprovals: storeState?.pendingApprovals ?? EMPTY_APPROVALS,
    clearPreview,
    reset,
  };
}
