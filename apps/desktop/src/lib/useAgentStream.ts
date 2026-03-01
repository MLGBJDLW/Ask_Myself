import { useState, useCallback, useRef, useEffect } from 'react';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import * as api from './api';
import type { AgentFrontendEvent } from '../types';
import type { ImageAttachment } from '../types/conversation';

const STREAM_TIMEOUT_MS = 30_000; // 30-second inactivity watchdog

export interface ToolCallEvent {
  callId: string;
  toolName: string;
  arguments: string;
  status: 'running' | 'done' | 'error';
  content?: string;
  isError?: boolean;
  artifacts?: Record<string, unknown>;
}

export interface StreamRoundEvent {
  id: string;
  reply: string;
  toolCalls: ToolCallEvent[];
}

type AgentEventType = AgentFrontendEvent['type'];
type AutoCompactedInfo = { summary: string } | null;

function normalizeAgentEventType(value: unknown): AgentEventType | null {
  if (typeof value !== 'string') return null;
  const raw = value.trim();
  if (!raw) return null;

  const lowered = raw
    .replace(/[_\s-]+([a-zA-Z0-9])/g, (_m, ch: string) => ch.toUpperCase())
    .replace(/^([A-Z])/, (_m, ch: string) => ch.toLowerCase());

  switch (lowered) {
    case 'thinking':
      return 'thinking';
    case 'textDelta':
      return 'textDelta';
    case 'toolCallStart':
      return 'toolCallStart';
    case 'toolCallResult':
      return 'toolCallResult';
    case 'done':
      return 'done';
    case 'error':
      return 'error';
    case 'autoCompacted':
      return 'autoCompacted';
    case 'usageUpdate':
      return 'usageUpdate';
    default:
      return null;
  }
}

function finalizeToolCall(
  tc: ToolCallEvent,
  isError: boolean | undefined,
  content: string | undefined,
  artifacts: Record<string, unknown> | undefined,
): ToolCallEvent {
  return {
    ...tc,
    status: isError ? 'error' : 'done',
    content,
    isError,
    artifacts,
  };
}

function resolveToolCallResult(
  prev: ToolCallEvent[],
  resultCallId: string,
  resultIsError: boolean | undefined,
  resultContent: string | undefined,
  resultArtifacts: Record<string, unknown> | undefined,
): { next: ToolCallEvent[]; matched: boolean } {
  let matched = false;
  const updated = prev.map(tc => {
    if (tc.callId === resultCallId) {
      matched = true;
      return finalizeToolCall(tc, resultIsError, resultContent, resultArtifacts);
    }
    return tc;
  });

  if (matched) {
    return { next: updated, matched: true };
  }

  let fallbackIndex = -1;
  for (let i = updated.length - 1; i >= 0; i -= 1) {
    if (updated[i].status === 'running') {
      fallbackIndex = i;
      break;
    }
  }
  if (fallbackIndex >= 0) {
    const copy = [...updated];
    copy[fallbackIndex] = finalizeToolCall(
      copy[fallbackIndex],
      resultIsError,
      resultContent,
      resultArtifacts,
    );
    return { next: copy, matched: true };
  }

  return { next: updated, matched: false };
}

export interface UsageTotal {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  thinkingTokens?: number;
  lastPromptTokens?: number;
}

interface UseAgentStreamReturn {
  send: (conversationId: string, message: string, attachments?: ImageAttachment[]) => Promise<void>;
  stop: (conversationId: string) => Promise<void>;
  isStreaming: boolean;
  streamText: string;
  streamRounds: StreamRoundEvent[];
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
  reset: () => void;
}

export function useAgentStream(): UseAgentStreamReturn {
  const [isStreaming, setIsStreaming] = useState(false);
  const [streamText, setStreamText] = useState('');
  const [streamRounds, setStreamRounds] = useState<StreamRoundEvent[]>([]);
  const [thinkingText, setThinkingText] = useState('');
  const [isThinking, setIsThinking] = useState(false);
  const [toolCalls, setToolCalls] = useState<ToolCallEvent[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [lastUsage, setLastUsage] = useState<UsageTotal | null>(null);
  const [lastCached, setLastCached] = useState(false);
  const [finishReason, setFinishReason] = useState<string | null>(null);
  const [contextOverflow, setContextOverflow] = useState(false);
  const [rateLimited, setRateLimited] = useState(false);
  const [autoCompacted, setAutoCompacted] = useState<AutoCompactedInfo>(null);
  const unlistenRef = useRef<UnlistenFn | null>(null);
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const thinkingTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const activeConversationRef = useRef<string | null>(null);
  const toolCallSeqRef = useRef(0);
  const roundSeqRef = useRef(0);
  const activeRoundIdRef = useRef<string | null>(null);
  const streamTextRef = useRef('');

  const clearStreamTimeout = useCallback(() => {
    if (timeoutRef.current) {
      clearTimeout(timeoutRef.current);
      timeoutRef.current = null;
    }
  }, []);

  const clearThinkingTimeout = useCallback(() => {
    if (thinkingTimeoutRef.current) {
      clearTimeout(thinkingTimeoutRef.current);
      thinkingTimeoutRef.current = null;
    }
  }, []);

  const cleanup = useCallback(() => {
    clearStreamTimeout();
    clearThinkingTimeout();
    if (unlistenRef.current) {
      unlistenRef.current();
      unlistenRef.current = null;
    }
  }, [clearStreamTimeout, clearThinkingTimeout]);

  const resetStreamTimeout = useCallback(() => {
    clearStreamTimeout();
    timeoutRef.current = setTimeout(() => {
      setToolCalls(prev => prev.map(tc =>
        tc.status === 'running'
          ? { ...tc, status: 'error', content: tc.content || 'Tool call timed out.', isError: true }
          : tc,
      ));
      setStreamRounds(prev => prev.map(round => ({
        ...round,
        toolCalls: round.toolCalls.map(tc =>
          tc.status === 'running'
            ? { ...tc, status: 'error', content: tc.content || 'Tool call timed out.', isError: true }
            : tc,
        ),
      })));
      setError('Connection lost \u2014 no response received for 30 seconds. You can retry your message.');
      setIsStreaming(false);
      activeRoundIdRef.current = null;
      cleanup();
    }, STREAM_TIMEOUT_MS);
  }, [clearStreamTimeout, cleanup]);

  const reset = useCallback(() => {
    setStreamText('');
    streamTextRef.current = '';
    setStreamRounds([]);
    setThinkingText('');
    setIsThinking(false);
    setToolCalls([]);
    setError(null);
    setLastUsage(null);
    setLastCached(false);
    setFinishReason(null);
    setContextOverflow(false);
    setRateLimited(false);
    setAutoCompacted(null);
    activeRoundIdRef.current = null;
    clearThinkingTimeout();
  }, [clearThinkingTimeout]);

  const send = useCallback(async (conversationId: string, message: string, attachments?: ImageAttachment[]) => {
    // Cleanup previous listener and timeout
    cleanup();

    setIsStreaming(true);
    setError(null);
    setStreamText('');
    streamTextRef.current = '';
    setStreamRounds([]);
    setThinkingText('');
    setIsThinking(false);
    setToolCalls([]);
    setLastCached(false);
    setFinishReason(null);
    setContextOverflow(false);
    setRateLimited(false);
    setAutoCompacted(null);
    activeConversationRef.current = conversationId;
    toolCallSeqRef.current = 0;
    roundSeqRef.current = 0;
    activeRoundIdRef.current = null;

    // Start the inactivity timeout
    resetStreamTimeout();

    // Listen for agent events BEFORE sending the command
    unlistenRef.current = await listen<AgentFrontendEvent>('agent:event', (event) => {
      const data = event.payload;
      const raw = data as AgentFrontendEvent & Record<string, unknown>;
      const eventType = normalizeAgentEventType(raw.type);
      const eventConversationId = data?.conversationId
        || (typeof raw.conversation_id === 'string' ? raw.conversation_id : '');
      if (!data || !eventType || eventConversationId !== activeConversationRef.current) {
        return;
      }

      // Reset timeout on every received event
      resetStreamTimeout();

      switch (eventType) {
        case 'thinking':
          setIsThinking(true);
          setThinkingText(prev => prev + (typeof data.content === 'string' ? data.content : (typeof raw.content === 'string' ? raw.content : '')));
          clearThinkingTimeout();
          thinkingTimeoutRef.current = setTimeout(() => {
            setIsThinking(false);
          }, 900);
          break;
        case 'textDelta':
          if (activeRoundIdRef.current) {
            // New assistant text after a tool round should start a fresh preview segment.
            activeRoundIdRef.current = null;
          }
          setStreamText(prev => {
            const delta = (typeof data.delta === 'string' ? data.delta : (typeof raw.delta === 'string' ? raw.delta : ''));
            const next = prev + delta;
            streamTextRef.current = next;
            return next;
          });
          break;
        case 'toolCallStart':
          setIsThinking(false);
          const incomingCallIdRaw = (typeof data.callId === 'string' && data.callId)
            || (typeof raw.call_id === 'string' ? raw.call_id : '');
          const incomingCallId = incomingCallIdRaw.trim();
          const callId = incomingCallId || `tool-call-${Date.now()}-${toolCallSeqRef.current++}`;
          const toolNameRaw = (typeof data.toolName === 'string' && data.toolName)
            || (typeof raw.tool_name === 'string' ? raw.tool_name : '');
          const toolName = toolNameRaw.trim()
            ? toolNameRaw
            : 'unknown_tool';
          const argsRaw = data.arguments ?? raw.arguments;
          const argumentsText = typeof argsRaw === 'string'
            ? argsRaw
            : (argsRaw == null ? '' : String(argsRaw));
          const nextCall: ToolCallEvent = {
            callId,
            toolName,
            arguments: argumentsText,
            status: 'running',
          };

          const currentReply = streamTextRef.current;
          if (currentReply.trim().length > 0) {
            const roundId = `stream-round-${Date.now()}-${roundSeqRef.current++}`;
            activeRoundIdRef.current = roundId;
            setStreamRounds(prev => [...prev, {
              id: roundId,
              reply: currentReply,
              toolCalls: [nextCall],
            }]);
            setStreamText('');
            streamTextRef.current = '';
          } else {
            setStreamRounds(prev => {
              if (activeRoundIdRef.current) {
                return prev.map(round => round.id === activeRoundIdRef.current
                  ? (() => {
                    const existingIdx = round.toolCalls.findIndex(tc => tc.callId === nextCall.callId);
                    if (existingIdx >= 0) {
                      const nextToolCalls = [...round.toolCalls];
                      nextToolCalls[existingIdx] = {
                        ...nextToolCalls[existingIdx],
                        toolName: nextCall.toolName,
                        arguments: nextCall.arguments,
                        status: 'running',
                      };
                      return { ...round, toolCalls: nextToolCalls };
                    }
                    return { ...round, toolCalls: [...round.toolCalls, nextCall] };
                  })()
                  : round,
                );
              }
              const roundId = `stream-round-${Date.now()}-${roundSeqRef.current++}`;
              activeRoundIdRef.current = roundId;
              return [...prev, {
                id: roundId,
                reply: '',
                toolCalls: [nextCall],
              }];
            });
          }

          setToolCalls(prev => {
            const existing = prev.findIndex(tc => tc.callId === callId);
            if (existing >= 0) {
              const copy = [...prev];
              copy[existing] = {
                ...copy[existing],
                toolName,
                arguments: argumentsText,
                status: 'running',
              };
              return copy;
            }

            return [...prev, nextCall];
          });
          break;
        case 'toolCallResult':
          const resultCallId = (typeof data.callId === 'string' && data.callId)
            || (typeof raw.call_id === 'string' ? raw.call_id : '')
            || '';
          const resultIsError = (typeof data.isError === 'boolean' ? data.isError : undefined)
            ?? (typeof raw.is_error === 'boolean' ? raw.is_error : undefined);
          const resultContent = (typeof data.content === 'string' ? data.content : undefined)
            ?? (typeof raw.content === 'string' ? raw.content : undefined);
          const resultArtifacts = (data.artifacts && typeof data.artifacts === 'object')
            ? data.artifacts
            : ((raw.artifacts && typeof raw.artifacts === 'object') ? raw.artifacts as Record<string, unknown> : undefined);

          setToolCalls(prev => {
            const { next } = resolveToolCallResult(
              prev,
              resultCallId,
              resultIsError,
              resultContent,
              resultArtifacts,
            );
            return next;
          });
          setStreamRounds(prev => {
            const copy = [...prev];
            for (let i = copy.length - 1; i >= 0; i -= 1) {
              const round = copy[i];
              const resolved = resolveToolCallResult(
                round.toolCalls,
                resultCallId,
                resultIsError,
                resultContent,
                resultArtifacts,
              );
              if (resolved.matched) {
                copy[i] = { ...round, toolCalls: resolved.next };
                return copy;
              }
            }
            return prev;
          });
          break;
        case 'usageUpdate': {
          const uUsage = data.usageTotal ?? (raw.usage_total as UsageTotal | undefined);
          if (uUsage) {
            const uLpt = (raw.lastPromptTokens ?? raw.last_prompt_tokens) as number | undefined;
            setLastUsage({
              ...uUsage,
              lastPromptTokens: uLpt ?? uUsage.lastPromptTokens,
            });
          }
          break;
        }
        case 'done': {
          clearThinkingTimeout();
          setIsThinking(false);
          const doneMessage = data.message ?? raw.message;
          if (doneMessage && typeof doneMessage === 'object' && 'content' in doneMessage) {
            const content = (doneMessage as { content?: unknown }).content;
            if (typeof content === 'string') {
              setStreamText(content);
              streamTextRef.current = content;
            }
          }
          setToolCalls(prev => prev.map(tc =>
            tc.status === 'running'
              ? { ...tc, status: 'done', content: tc.content || 'Completed without explicit output.' }
              : tc,
          ));
          setStreamRounds(prev => prev.map(round => ({
            ...round,
            toolCalls: round.toolCalls.map(tc =>
              tc.status === 'running'
                ? { ...tc, status: 'done', content: tc.content || 'Completed without explicit output.' }
                : tc,
            ),
          })));
          const usage = data.usageTotal ?? (raw.usage_total as UsageTotal | undefined);
          if (usage) {
            // Include lastPromptTokens from the event (serde camelCase field).
            const lastPrompt = (raw.lastPromptTokens ?? raw.last_prompt_tokens) as number | undefined;
            setLastUsage({
              ...usage,
              lastPromptTokens: lastPrompt ?? usage.lastPromptTokens,
            });
          }
          // Extract cached flag from done event
          const cached = raw.cached ?? false;
          setLastCached(Boolean(cached));
          // Extract finish reason
          const fr = raw.finishReason ?? raw.finish_reason ?? null;
          setFinishReason(typeof fr === 'string' ? fr : null);
          setIsStreaming(false);
          activeRoundIdRef.current = null;
          cleanup();
          break;
        }
        case 'autoCompacted': {
          const summary = (typeof data.summary === 'string' ? data.summary : '')
            || (typeof raw.summary === 'string' ? raw.summary as string : '');
          setAutoCompacted({ summary });
          break;
        }
        case 'error': {
          clearThinkingTimeout();
          setIsThinking(false);
          setToolCalls(prev => prev.map(tc =>
            tc.status === 'running'
              ? { ...tc, status: 'error', content: tc.content || 'Tool call interrupted by agent error.', isError: true }
              : tc,
          ));
          setStreamRounds(prev => prev.map(round => ({
            ...round,
            toolCalls: round.toolCalls.map(tc =>
              tc.status === 'running'
                ? { ...tc, status: 'error', content: tc.content || 'Tool call interrupted by agent error.', isError: true }
                : tc,
            ),
          })));
          const errMsg = (typeof data.message === 'string' ? data.message : (typeof raw.message === 'string' ? raw.message : 'Unknown error'));
          // Detect context overflow
          if (/context.*(window|overflow|exceeded)|ContextOverflow/i.test(errMsg)) {
            setContextOverflow(true);
          }
          // Detect rate limiting
          if (/rate.?limit/i.test(errMsg)) {
            setRateLimited(true);
            setError('Rate limited by provider. Please wait and try again.');
          } else {
            setError(errMsg);
          }
          setIsStreaming(false);
          activeRoundIdRef.current = null;
          cleanup();
          break;
        }
      }
    });

    // Send the message
    try {
      await api.agentChat(conversationId, message, attachments);
    } catch (err) {
      clearThinkingTimeout();
      setIsThinking(false);
      setToolCalls(prev => prev.map(tc =>
        tc.status === 'running'
          ? { ...tc, status: 'error', content: tc.content || 'Agent request failed.', isError: true }
          : tc,
      ));
      setStreamRounds(prev => prev.map(round => ({
        ...round,
        toolCalls: round.toolCalls.map(tc =>
          tc.status === 'running'
            ? { ...tc, status: 'error', content: tc.content || 'Agent request failed.', isError: true }
            : tc,
        ),
      })));
      setError(String(err));
      setIsStreaming(false);
      activeRoundIdRef.current = null;
      cleanup();
    }
  }, [cleanup, resetStreamTimeout, clearThinkingTimeout]);

  const stop = useCallback(async (conversationId: string) => {
    try {
      await api.agentStop(conversationId);
    } catch (err) {
      // Ignore errors on stop
    }
    clearThinkingTimeout();
    setIsThinking(false);
    setToolCalls(prev => prev.map(tc =>
      tc.status === 'running'
        ? { ...tc, status: 'error', content: tc.content || 'Stopped by user.', isError: true }
        : tc,
    ));
    setStreamRounds(prev => prev.map(round => ({
      ...round,
      toolCalls: round.toolCalls.map(tc =>
        tc.status === 'running'
          ? { ...tc, status: 'error', content: tc.content || 'Stopped by user.', isError: true }
          : tc,
      ),
    })));
    setIsStreaming(false);
    activeRoundIdRef.current = null;
    cleanup();
  }, [cleanup, clearThinkingTimeout]);

  // Clean up listeners and timeouts on unmount to prevent memory leaks
  useEffect(() => {
    return () => cleanup();
  }, [cleanup]);

  return { send, stop, isStreaming, streamText, streamRounds, thinkingText, isThinking, toolCalls, error, lastUsage, lastCached, finishReason, contextOverflow, rateLimited, autoCompacted, reset };
}
