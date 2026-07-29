import { convertFileSrc } from '@tauri-apps/api/core';
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';

import * as api from '../../lib/api';
import { finalAnswerToSpeechText } from '../../lib/autoSpeech';
import { classifyMediaError, mediaErrorMessage, playableMediaType, type SpeechPlaybackErrorCode } from './speechPlaybackRuntime';

export type SpeechPlaybackState =
  | { status: 'idle' }
  | { status: 'synthesizing'; messageId: string }
  | { status: 'playing'; messageId: string }
  | { status: 'paused'; messageId: string }
  | { status: 'error'; messageId: string; code: SpeechPlaybackErrorCode; error: string };

interface SpeechRequest { messageId: string; text: string }
interface SpeechPlaybackContextValue {
  state: SpeechPlaybackState;
  speakMessage(messageId: string, text: string): Promise<void>;
  pause(): void;
  resume(): Promise<void>;
  stop(): void;
  retry(): Promise<void>;
}

const SpeechPlaybackContext = createContext<SpeechPlaybackContextValue | null>(null);

export function SpeechPlaybackProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<SpeechPlaybackState>({ status: 'idle' });
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const requestRef = useRef<SpeechRequest | null>(null);
  const generationRef = useRef(0);

  const stop = useCallback(() => {
    generationRef.current += 1;
    const audio = audioRef.current;
    if (audio) {
      audio.pause();
      audio.removeAttribute('src');
      audio.replaceChildren();
      audio.load();
    }
    audioRef.current = null;
    setState({ status: 'idle' });
  }, []);

  const speakMessage = useCallback(async (messageId: string, markdown: string) => {
    const text = finalAnswerToSpeechText(markdown);
    if (!text) return;
    generationRef.current += 1;
    const generation = generationRef.current;
    const previous = audioRef.current;
    if (previous) {
      previous.pause();
      previous.replaceChildren();
    }
    requestRef.current = { messageId, text: markdown };
    setState({ status: 'synthesizing', messageId });
    try {
      const preview = await api.synthesizeSpeechPreview(text);
      if (generation !== generationRef.current) return;
      const audio = document.createElement('audio');
      const support = audio.canPlayType(preview.mediaType);
      if (!playableMediaType(preview.mediaType, support)) {
        throw playbackError('unsupported_format');
      }
      const source = document.createElement('source');
      source.src = convertFileSrc(preview.path);
      source.type = preview.mediaType;
      audio.appendChild(source);
      audio.preload = 'auto';
      audioRef.current = audio;
      await waitForCanPlay(audio);
      if (generation !== generationRef.current) return;
      audio.onended = () => {
        if (generation === generationRef.current) setState({ status: 'idle' });
      };
      await audio.play().catch((error: unknown) => {
        if (error instanceof DOMException && error.name === 'NotAllowedError') throw playbackError('autoplay_blocked');
        throw error;
      });
      setState({ status: 'playing', messageId });
    } catch (error) {
      if (generation !== generationRef.current) return;
      const code = errorCode(error);
      setState({ status: 'error', messageId, code, error: mediaErrorMessage(code) });
    }
  }, []);

  const pause = useCallback(() => {
    if (state.status !== 'playing' || !audioRef.current) return;
    audioRef.current.pause();
    setState({ status: 'paused', messageId: state.messageId });
  }, [state]);

  const resume = useCallback(async () => {
    if (state.status !== 'paused' || !audioRef.current) return;
    try {
      await audioRef.current.play();
      setState({ status: 'playing', messageId: state.messageId });
    } catch (error) {
      const code = errorCode(error);
      setState({ status: 'error', messageId: state.messageId, code, error: mediaErrorMessage(code) });
    }
  }, [state]);

  const retry = useCallback(async () => {
    const request = requestRef.current;
    if (request) await speakMessage(request.messageId, request.text);
  }, [speakMessage]);

  useEffect(() => () => {
    generationRef.current += 1;
    audioRef.current?.pause();
  }, []);

  const value = useMemo<SpeechPlaybackContextValue>(
    () => ({ state, speakMessage, pause, resume, stop, retry }),
    [pause, resume, retry, speakMessage, state, stop],
  );
  return <SpeechPlaybackContext.Provider value={value}>{children}</SpeechPlaybackContext.Provider>;
}

export function useSpeechPlayback(): SpeechPlaybackContextValue {
  const value = useContext(SpeechPlaybackContext);
  if (!value) throw new Error('useSpeechPlayback must be used within SpeechPlaybackProvider');
  return value;
}

function waitForCanPlay(audio: HTMLAudioElement): Promise<void> {
  return new Promise((resolve, reject) => {
    const timeout = window.setTimeout(() => reject(playbackError(classifyMediaError(audio.error?.code))), 15_000);
    const cleanup = () => {
      window.clearTimeout(timeout);
      audio.removeEventListener('canplay', onCanPlay);
      audio.removeEventListener('error', onError);
    };
    const onCanPlay = () => { cleanup(); resolve(); };
    const onError = () => { cleanup(); reject(playbackError(classifyMediaError(audio.error?.code))); };
    audio.addEventListener('canplay', onCanPlay, { once: true });
    audio.addEventListener('error', onError, { once: true });
    audio.load();
  });
}

function playbackError(code: SpeechPlaybackErrorCode): Error & { code: SpeechPlaybackErrorCode } {
  return Object.assign(new Error(mediaErrorMessage(code)), { code });
}
function errorCode(error: unknown): SpeechPlaybackErrorCode {
  if (error instanceof DOMException && error.name === 'NotAllowedError') return 'autoplay_blocked';
  if (error && typeof error === 'object' && 'code' in error && typeof error.code === 'string') {
    return error.code as SpeechPlaybackErrorCode;
  }
  return 'provider';
}
