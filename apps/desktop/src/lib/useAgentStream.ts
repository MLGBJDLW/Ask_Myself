import { useState, useCallback, useRef } from 'react';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import * as api from './api';
import type { AgentFrontendEvent } from '../types';

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

interface UseAgentStreamReturn {
  send: (conversationId: string, message: string) => Promise<void>;
  stop: (conversationId: string) => Promise<void>;
  isStreaming: boolean;
  streamText: string;
  toolCalls: ToolCallEvent[];
  error: string | null;
  reset: () => void;
}

export function useAgentStream(): UseAgentStreamReturn {
  const [isStreaming, setIsStreaming] = useState(false);
  const [streamText, setStreamText] = useState('');
  const [toolCalls, setToolCalls] = useState<ToolCallEvent[]>([]);
  const [error, setError] = useState<string | null>(null);
  const unlistenRef = useRef<UnlistenFn | null>(null);
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const activeConversationRef = useRef<string | null>(null);
  const toolCallSeqRef = useRef(0);

  const clearStreamTimeout = useCallback(() => {
    if (timeoutRef.current) {
      clearTimeout(timeoutRef.current);
      timeoutRef.current = null;
    }
  }, []);

  const cleanup = useCallback(() => {
    clearStreamTimeout();
    if (unlistenRef.current) {
      unlistenRef.current();
      unlistenRef.current = null;
    }
  }, [clearStreamTimeout]);

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
    setToolCalls([]);
    setError(null);
  }, []);

  const send = useCallback(async (conversationId: string, message: string) => {
    // Cleanup previous listener and timeout
    cleanup();

    setIsStreaming(true);
    setError(null);
    setStreamText('');
    setToolCalls([]);
    activeConversationRef.current = conversationId;
    toolCallSeqRef.current = 0;

    // Start the inactivity timeout
    resetStreamTimeout();

    // Listen for agent events BEFORE sending the command
    unlistenRef.current = await listen<AgentFrontendEvent>('agent:event', (event) => {
      const data = event.payload;
      const raw = data as AgentFrontendEvent & Record<string, unknown>;
      const eventConversationId = data?.conversationId
        || (typeof raw.conversation_id === 'string' ? raw.conversation_id : '');
      if (!data || eventConversationId !== activeConversationRef.current) {
        return;
      }

      // Reset timeout on every received event
      resetStreamTimeout();

      switch (data.type) {
        case 'textDelta':
          setStreamText(prev => prev + (data.delta || ''));
          break;
        case 'toolCallStart':
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
            let matched = false;
            const updated = prev.map(tc => {
              if (tc.callId === resultCallId) {
                matched = true;
                return finalizeToolCall(tc, resultIsError, data.content, data.artifacts);
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
                  data.content,
                  data.artifacts,
                );
                return copy;
              }
            }
            return updated;
          });
          // After tool result, reset text for new LLM response
          setStreamText('');
          break;
        case 'done':
          setToolCalls(prev => prev.map(tc =>
            tc.status === 'running'
              ? { ...tc, status: 'done', content: tc.content || 'Completed without explicit output.' }
              : tc,
          ));
          setIsStreaming(false);
          cleanup();
          break;
        case 'error':
          setToolCalls(prev => prev.map(tc =>
            tc.status === 'running'
              ? { ...tc, status: 'error', content: tc.content || 'Tool call interrupted by agent error.', isError: true }
              : tc,
          ));
          setError((data.message as unknown as string) || 'Unknown error');
          setIsStreaming(false);
          cleanup();
          break;
      }
    });

    // Send the message
    try {
      await api.agentChat(conversationId, message);
    } catch (err) {
      setToolCalls(prev => prev.map(tc =>
        tc.status === 'running'
          ? { ...tc, status: 'error', content: tc.content || 'Agent request failed.', isError: true }
          : tc,
      ));
      setError(String(err));
      setIsStreaming(false);
      cleanup();
    }
  }, [cleanup, resetStreamTimeout]);

  const stop = useCallback(async (conversationId: string) => {
    try {
      await api.agentStop(conversationId);
    } catch (err) {
      // Ignore errors on stop
    }
    setToolCalls(prev => prev.map(tc =>
      tc.status === 'running'
        ? { ...tc, status: 'error', content: tc.content || 'Stopped by user.', isError: true }
        : tc,
    ));
    setIsStreaming(false);
    cleanup();
  }, [cleanup]);

  return { send, stop, isStreaming, streamText, toolCalls, error, reset };
}
