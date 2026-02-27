import { useState, useCallback, useRef, useEffect } from 'react';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import * as api from './api';
import type { AgentFrontendEvent } from '../types';
import type { ImageAttachment } from '../types/conversation';

const STREAM_TIMEOUT_MS = 120_000; // 120 seconds

interface ToolCallEvent {
  callId: string;
  toolName: string;
  arguments: string;
  status: 'running' | 'done' | 'error';
  content?: string;
  isError?: boolean;
  artifacts?: Record<string, unknown>;
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
      setError('Agent response timed out. Please try again.');
      setIsStreaming(false);
      cleanup();
    }, STREAM_TIMEOUT_MS);
  }, [clearStreamTimeout, cleanup]);

  const reset = useCallback(() => {
    setStreamText('');
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
    clearThinkingTimeout();
  }, [clearThinkingTimeout]);

  const send = useCallback(async (conversationId: string, message: string, attachments?: ImageAttachment[]) => {
    // Cleanup previous listener and timeout
    cleanup();

    setIsStreaming(true);
    setError(null);
    setStreamText('');
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
          setStreamText(prev => prev + (typeof data.delta === 'string' ? data.delta : (typeof raw.delta === 'string' ? raw.delta : '')));
          break;
        case 'toolCallStart':
          // A tool round started. Keep only the *current* round's streamed text
          // so the final assistant reply doesn't concatenate prior rounds.
          setStreamText('');
          setThinkingText('');
          setIsThinking(false);
          setToolCalls(prev => {
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

            return [...prev, {
              callId,
              toolName,
              arguments: argumentsText,
              status: 'running',
            }];
          });
          break;
        case 'toolCallResult':
          setToolCalls(prev => {
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
            let matched = false;
            const updated = prev.map(tc => {
              if (tc.callId === resultCallId) {
                matched = true;
                return finalizeToolCall(tc, resultIsError, resultContent, resultArtifacts);
              }
              return tc;
            });

            // Fallback for providers that return inconsistent/missing call IDs.
            if (!matched) {
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
                return copy;
              }
            }
            return updated;
          });
          // Keep text as-is here; round reset happens on toolCallStart.
          break;
        case 'done': {
          clearThinkingTimeout();
          setIsThinking(false);
          setToolCalls(prev => prev.map(tc =>
            tc.status === 'running'
              ? { ...tc, status: 'done', content: tc.content || 'Completed without explicit output.' }
              : tc,
          ));
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
      setError(String(err));
      setIsStreaming(false);
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
    setIsStreaming(false);
    cleanup();
  }, [cleanup, clearThinkingTimeout]);

  // Clean up listeners and timeouts on unmount to prevent memory leaks
  useEffect(() => {
    return () => cleanup();
  }, [cleanup]);

  return { send, stop, isStreaming, streamText, thinkingText, isThinking, toolCalls, error, lastUsage, lastCached, finishReason, contextOverflow, rateLimited, autoCompacted, reset };
}
