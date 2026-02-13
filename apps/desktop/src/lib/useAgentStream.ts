import { useState, useCallback, useRef } from 'react';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import * as api from './api';
import type { AgentEvent } from '../types';

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

    // Start the inactivity timeout
    resetStreamTimeout();

    // Listen for agent events BEFORE sending the command
    unlistenRef.current = await listen<AgentEvent>('agent:event', (event) => {
      const data = event.payload;

      // Reset timeout on every received event
      resetStreamTimeout();

      switch (data.type) {
        case 'textDelta':
          setStreamText(prev => prev + (data.delta || ''));
          break;
        case 'toolCallStart':
          setToolCalls(prev => [...prev, {
            callId: data.callId!,
            toolName: data.toolName!,
            arguments: data.arguments || '',
            status: 'running',
          }]);
          break;
        case 'toolCallResult':
          setToolCalls(prev => prev.map(tc =>
            tc.callId === data.callId
              ? { ...tc, status: data.isError ? 'error' : 'done', content: data.content, isError: data.isError, artifacts: data.artifacts }
              : tc
          ));
          // After tool result, reset text for new LLM response
          setStreamText('');
          break;
        case 'done':
          setIsStreaming(false);
          cleanup();
          break;
        case 'error':
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
    setIsStreaming(false);
    cleanup();
  }, [cleanup]);

  return { send, stop, isStreaming, streamText, toolCalls, error, reset };
}
