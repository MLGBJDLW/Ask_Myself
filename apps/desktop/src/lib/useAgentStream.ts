import { useState, useCallback, useRef, useEffect } from 'react';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import * as api from './api';
import type { AgentFrontendEvent } from '../types';
import { useTranslation } from '../i18n';
import type { ArtifactPayload, ImageAttachment } from '../types/conversation';

const STREAM_TIMEOUT_MS = 30_000; // 30-second inactivity watchdog

export interface ToolCallEvent {
  callId: string;
  toolName: string;
  arguments: string;
  status: 'running' | 'done' | 'error';
  content?: string;
  isError?: boolean;
  artifacts?: ArtifactPayload;
}

export interface StreamRoundEvent {
  id: string;
  thinking?: string;
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
  artifacts: ArtifactPayload | undefined,
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
  resultArtifacts: ArtifactPayload | undefined,
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

function extractMessageText(message: unknown): string | null {
  if (!message || typeof message !== 'object') {
    return null;
  }

  const record = message as Record<string, unknown>;
  if (typeof record.content === 'string' && record.content.trim().length > 0) {
    return record.content;
  }

  if (!Array.isArray(record.parts)) {
    return null;
  }

  const text = record.parts
    .map(part => {
      if (!part || typeof part !== 'object') {
        return '';
      }
      const item = part as Record<string, unknown>;
      return typeof item.text === 'string' ? item.text : '';
    })
    .join('');

  return text.trim().length > 0 ? text : null;
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
  clearPreview: () => void;
  reset: () => void;
}

export function useAgentStream(): UseAgentStreamReturn {
  const { t } = useTranslation();
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
  const activeConversationRef = useRef<string | null>(null);
  const toolCallSeqRef = useRef(0);
  const roundSeqRef = useRef(0);
  const activeRoundIdRef = useRef<string | null>(null);
  const activeRoundAcceptingStartsRef = useRef(false);
  const streamTextRef = useRef('');
  const thinkingTextRef = useRef('');

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
          ? { ...tc, status: 'error', content: tc.content || t('chat.toolTimeout'), isError: true }
          : tc,
      ));
      setStreamRounds(prev => prev.map(round => ({
        ...round,
        toolCalls: round.toolCalls.map(tc =>
          tc.status === 'running'
            ? { ...tc, status: 'error', content: tc.content || t('chat.toolTimeout'), isError: true }
            : tc,
        ),
      })));
      setThinkingText('');
      thinkingTextRef.current = '';
      setIsThinking(false);
      setError(t('chat.connectionLost'));
      setIsStreaming(false);
      activeRoundIdRef.current = null;
      activeRoundAcceptingStartsRef.current = false;
      cleanup();
    }, STREAM_TIMEOUT_MS);
  }, [clearStreamTimeout, cleanup, t]);

  const clearPreview = useCallback(() => {
    setStreamText('');
    streamTextRef.current = '';
    setStreamRounds([]);
    setThinkingText('');
    thinkingTextRef.current = '';
    setIsThinking(false);
    setToolCalls([]);
    activeRoundIdRef.current = null;
    activeRoundAcceptingStartsRef.current = false;
  }, []);

  const reset = useCallback(() => {
    clearPreview();
    setError(null);
    setLastUsage(null);
    setLastCached(false);
    setFinishReason(null);
    setContextOverflow(false);
    setRateLimited(false);
    setAutoCompacted(null);
  }, [clearPreview]);

  const consumeThinkingSegment = useCallback(() => {
    const captured = thinkingTextRef.current;
    if (!captured.trim()) {
      return '';
    }
    thinkingTextRef.current = '';
    setThinkingText('');
    return captured;
  }, []);

  const send = useCallback(async (conversationId: string, message: string, attachments?: ImageAttachment[]) => {
    // Cleanup previous listener and timeout
    cleanup();

    setIsStreaming(true);
    setError(null);
    setStreamText('');
    streamTextRef.current = '';
    setStreamRounds([]);
    setThinkingText('');
    thinkingTextRef.current = '';
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
    activeRoundAcceptingStartsRef.current = false;

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
          const thinkingDelta = (typeof data.content === 'string' ? data.content : (typeof raw.content === 'string' ? raw.content : ''));
          if (!thinkingDelta) break;
          setIsThinking(true);
          setThinkingText(prev => {
            const next = prev + thinkingDelta;
            thinkingTextRef.current = next;
            return next;
          });
          break;
        case 'textDelta':
          setIsThinking(false);
          if (activeRoundIdRef.current) {
            // New assistant text after a tool round should start a fresh preview segment.
            activeRoundIdRef.current = null;
            activeRoundAcceptingStartsRef.current = false;
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
          const roundThinking = consumeThinkingSegment();
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
            activeRoundAcceptingStartsRef.current = true;
            setStreamRounds(prev => [...prev, {
              id: roundId,
              thinking: roundThinking || undefined,
              reply: currentReply,
              toolCalls: [nextCall],
            }]);
            setStreamText('');
            streamTextRef.current = '';
          } else {
            if (activeRoundIdRef.current && activeRoundAcceptingStartsRef.current) {
              const mergeRoundId = activeRoundIdRef.current;
              setStreamRounds(prev => prev.map(round => round.id === mergeRoundId
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
                    return {
                      ...round,
                      thinking: round.thinking || roundThinking || undefined,
                      toolCalls: nextToolCalls,
                    };
                  }
                  return {
                    ...round,
                    thinking: round.thinking || roundThinking || undefined,
                    toolCalls: [...round.toolCalls, nextCall],
                  };
                })()
                : round,
              ));
            } else {
              const roundId = `stream-round-${Date.now()}-${roundSeqRef.current++}`;
              activeRoundIdRef.current = roundId;
              activeRoundAcceptingStartsRef.current = true;
              setStreamRounds(prev => [...prev, {
                id: roundId,
                thinking: roundThinking || undefined,
                reply: '',
                toolCalls: [nextCall],
              }]);
            }
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
            : ((raw.artifacts && typeof raw.artifacts === 'object') ? raw.artifacts as ArtifactPayload : undefined);

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
          activeRoundAcceptingStartsRef.current = false;
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
          // Capture final round before clearing thinking/streaming state
          const finalThinking = thinkingTextRef.current;
          const finalReply = streamTextRef.current;
          const capturedFinalRound = finalThinking.trim() || finalReply.trim();
          if (capturedFinalRound) {
            const roundId = `stream-round-${Date.now()}-${roundSeqRef.current++}`;
            setStreamRounds(prev => [...prev, {
              id: roundId,
              thinking: finalThinking || undefined,
              reply: finalReply,
              toolCalls: [],
            }]);
            setStreamText('');
            streamTextRef.current = '';
          }
          setIsThinking(false);
          setThinkingText('');
          thinkingTextRef.current = '';
          if (!capturedFinalRound) {
            const doneMessage = data.message ?? raw.message;
            const doneText = extractMessageText(doneMessage);
            if (doneText) {
              setStreamText(doneText);
              streamTextRef.current = doneText;
            }
          }
          setToolCalls(prev => prev.map(tc =>
            tc.status === 'running'
              ? { ...tc, status: 'done', content: tc.content || t('chat.toolNoOutput') }
              : tc,
          ));
          setStreamRounds(prev => prev.map(round => ({
            ...round,
            toolCalls: round.toolCalls.map(tc =>
              tc.status === 'running'
                ? { ...tc, status: 'done', content: tc.content || t('chat.toolNoOutput') }
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
          activeRoundAcceptingStartsRef.current = false;
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
          setIsThinking(false);
          setThinkingText('');
          thinkingTextRef.current = '';
          setToolCalls(prev => prev.map(tc =>
            tc.status === 'running'
              ? { ...tc, status: 'error', content: tc.content || t('chat.toolInterrupted'), isError: true }
              : tc,
          ));
          setStreamRounds(prev => prev.map(round => ({
            ...round,
            toolCalls: round.toolCalls.map(tc =>
              tc.status === 'running'
                ? { ...tc, status: 'error', content: tc.content || t('chat.toolInterrupted'), isError: true }
                : tc,
            ),
          })));
          const errMsg = (typeof data.message === 'string' ? data.message : (typeof raw.message === 'string' ? raw.message : t('chat.unknownError')));
          // Detect context overflow
          if (/context.*(window|overflow|exceeded)|ContextOverflow/i.test(errMsg)) {
            setContextOverflow(true);
          }
          // Detect rate limiting
          if (/rate.?limit/i.test(errMsg)) {
            setRateLimited(true);
            setError(t('chat.rateLimited'));
          } else {
            setError(errMsg);
          }
          setIsStreaming(false);
          activeRoundIdRef.current = null;
          activeRoundAcceptingStartsRef.current = false;
          cleanup();
          break;
        }
      }
    });

    // Send the message
    try {
      await api.agentChat(conversationId, message, attachments);
    } catch (err) {
      setIsThinking(false);
      setThinkingText('');
      thinkingTextRef.current = '';
      setToolCalls(prev => prev.map(tc =>
        tc.status === 'running'
          ? { ...tc, status: 'error', content: tc.content || t('chat.agentRequestFailed'), isError: true }
          : tc,
      ));
      setStreamRounds(prev => prev.map(round => ({
        ...round,
        toolCalls: round.toolCalls.map(tc =>
          tc.status === 'running'
            ? { ...tc, status: 'error', content: tc.content || t('chat.agentRequestFailed'), isError: true }
            : tc,
        ),
      })));
      setError(String(err));
      setIsStreaming(false);
      activeRoundIdRef.current = null;
      activeRoundAcceptingStartsRef.current = false;
      cleanup();
    }
  }, [cleanup, resetStreamTimeout, consumeThinkingSegment, t]);

  const stop = useCallback(async (conversationId: string) => {
    try {
      await api.agentStop(conversationId);
    } catch (err) {
      // Ignore errors on stop
    }
    setIsThinking(false);
    setThinkingText('');
    thinkingTextRef.current = '';
    setToolCalls(prev => prev.map(tc =>
      tc.status === 'running'
        ? { ...tc, status: 'error', content: tc.content || t('chat.stoppedByUser'), isError: true }
        : tc,
    ));
    setStreamRounds(prev => prev.map(round => ({
      ...round,
      toolCalls: round.toolCalls.map(tc =>
        tc.status === 'running'
          ? { ...tc, status: 'error', content: tc.content || t('chat.stoppedByUser'), isError: true }
          : tc,
      ),
    })));
    setIsStreaming(false);
    activeRoundIdRef.current = null;
    activeRoundAcceptingStartsRef.current = false;
    cleanup();
  }, [cleanup, t]);

  // Clean up listeners and timeouts on unmount to prevent memory leaks
  useEffect(() => {
    return () => cleanup();
  }, [cleanup]);

  return { send, stop, isStreaming, streamText, streamRounds, thinkingText, isThinking, toolCalls, error, lastUsage, lastCached, finishReason, contextOverflow, rateLimited, autoCompacted, clearPreview, reset };
}
