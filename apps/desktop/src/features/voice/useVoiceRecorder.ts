import { useCallback, useEffect, useRef, useState } from 'react';

const VOICE_PCM_WORKLET_URL = new URL('./voicePcmProcessor.js', import.meta.url).href;
const VOICE_PCM_PROCESSOR_NAME = 'nexa-voice-pcm-processor';
const WORKLET_CHUNK_DURATION_MS = 20;
const WORKLET_MAX_CREDITS = 4;
const WORKLET_MAX_PENDING_CHUNKS = 8;
const WORKLET_FLUSH_TIMEOUT_MS = 250;

export type VoiceCaptureState =
  | 'idle'
  | 'capturing'
  | 'paused'
  | 'interrupted'
  | 'disconnected';

export interface VoiceRecordingOptions {
  /** PCM sample rate supplied to onPcmChunk. */
  targetSampleRate: number;
  /** Receives ordered, mono, little-endian PCM16 chunks while recording. */
  onPcmChunk: (chunk: Uint8Array) => boolean | void;
  /** Receives bounded-capture failures that require a safe spool finalize. */
  onCaptureIssue?: (state: Extract<VoiceCaptureState, 'interrupted' | 'disconnected'>) => void;
}

export interface UseVoiceRecorderReturn {
  isRecording: boolean;
  isPaused: boolean;
  captureState: VoiceCaptureState;
  startRecording: (options: VoiceRecordingOptions) => Promise<void>;
  pauseRecording: () => Promise<void>;
  resumeRecording: () => Promise<void>;
  stopRecording: () => Promise<void>;
  cancelRecording: () => void;
  recordingDuration: number;
  /** Analyser tapped off the live capture graph, for waveform visualization. */
  analyser: AnalyserNode | null;
}

type FlushWaiter = {
  timeout: ReturnType<typeof setTimeout>;
  resolve: () => void;
};

type VoiceWorkletMessage =
  | { type: 'pcm'; buffer: ArrayBuffer }
  | { type: 'overflow' }
  | { type: 'flushed'; requestId: number };

/**
 * Captures microphone audio through an AudioWorklet into fixed PCM16 chunks.
 * Credit/ACK flow control bounds MessagePort ownership, while resampling and
 * encoding stay off the React renderer thread. No complete recording is kept.
 */
export function useVoiceRecorder(deviceId?: string | null): UseVoiceRecorderReturn {
  const [isRecording, setIsRecording] = useState(false);
  const [isPaused, setIsPaused] = useState(false);
  const [captureState, setCaptureState] = useState<VoiceCaptureState>('idle');
  const [recordingDuration, setRecordingDuration] = useState(0);
  const [analyser, setAnalyser] = useState<AnalyserNode | null>(null);

  const streamRef = useRef<MediaStream | null>(null);
  const audioCtxRef = useRef<AudioContext | null>(null);
  const workletRef = useRef<AudioWorkletNode | null>(null);
  const muteRef = useRef<GainNode | null>(null);
  const analyserRef = useRef<AnalyserNode | null>(null);
  const sourceRef = useRef<MediaStreamAudioSourceNode | null>(null);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const cancelledRef = useRef(false);
  const recordingRef = useRef(false);
  const pausedRef = useRef(false);
  const recordingOptionsRef = useRef<VoiceRecordingOptions | null>(null);
  const accumulatedDurationMsRef = useRef(0);
  const activeSegmentStartedAtRef = useRef(0);
  const nextFlushRequestIdRef = useRef(1);
  const flushWaitersRef = useRef(new Map<number, FlushWaiter>());

  const updateDuration = useCallback(() => {
    const activeDuration = activeSegmentStartedAtRef.current > 0
      ? Date.now() - activeSegmentStartedAtRef.current
      : 0;
    setRecordingDuration(Math.floor(
      (accumulatedDurationMsRef.current + activeDuration) / 1000,
    ));
  }, []);

  const reportCaptureIssue = useCallback((
    state: Extract<VoiceCaptureState, 'interrupted' | 'disconnected'>,
  ) => {
    if (cancelledRef.current || !recordingRef.current) return;
    setCaptureState(state);
    recordingOptionsRef.current?.onCaptureIssue?.(state);
  }, []);

  const teardown = useCallback((resetDuration: boolean) => {
    if (timerRef.current) {
      clearInterval(timerRef.current);
      timerRef.current = null;
    }
    for (const waiter of flushWaitersRef.current.values()) {
      clearTimeout(waiter.timeout);
      waiter.resolve();
    }
    flushWaitersRef.current.clear();

    const worklet = workletRef.current;
    if (worklet) {
      worklet.port.onmessage = null;
      worklet.port.postMessage({ type: 'stop' });
      worklet.disconnect();
    }
    muteRef.current?.disconnect();
    analyserRef.current?.disconnect();
    sourceRef.current?.disconnect();
    void audioCtxRef.current?.close().catch(() => {});
    streamRef.current?.getTracks().forEach((track) => {
      track.onended = null;
      track.stop();
    });

    workletRef.current = null;
    muteRef.current = null;
    analyserRef.current = null;
    sourceRef.current = null;
    audioCtxRef.current = null;
    streamRef.current = null;
    recordingOptionsRef.current = null;
    recordingRef.current = false;
    pausedRef.current = false;
    activeSegmentStartedAtRef.current = 0;
    accumulatedDurationMsRef.current = 0;
    setAnalyser(null);
    if (resetDuration) setRecordingDuration(0);
  }, []);

  useEffect(() => () => teardown(false), [teardown]);

  const flushWorklet = useCallback((options: { pauseAfter?: boolean; stopAfter?: boolean } = {}) => {
    const worklet = workletRef.current;
    if (!worklet) return Promise.resolve();
    const requestId = nextFlushRequestIdRef.current;
    nextFlushRequestIdRef.current += 1;
    return new Promise<void>((resolve) => {
      const timeout = setTimeout(() => {
        flushWaitersRef.current.delete(requestId);
        resolve();
      }, WORKLET_FLUSH_TIMEOUT_MS);
      flushWaitersRef.current.set(requestId, { timeout, resolve });
      worklet.port.postMessage({ type: 'flush', requestId, ...options });
    });
  }, []);

  const startRecording = useCallback(async (options: VoiceRecordingOptions) => {
    if (recordingRef.current) throw new Error('Voice recording is already active');
    cancelledRef.current = false;
    recordingOptionsRef.current = options;
    setRecordingDuration(0);
    setCaptureState('idle');

    try {
      let stream: MediaStream;
      if (deviceId) {
        try {
          stream = await navigator.mediaDevices.getUserMedia({
            audio: { deviceId: { exact: deviceId } },
          });
        } catch {
          console.warn(`[useVoiceRecorder] deviceId ${deviceId} unavailable, falling back to default`);
          stream = await navigator.mediaDevices.getUserMedia({ audio: true });
        }
      } else {
        stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      }
      streamRef.current = stream;

      const audioCtx = new AudioContext();
      audioCtxRef.current = audioCtx;
      if (!audioCtx.audioWorklet) {
        throw new Error('AudioWorklet is unavailable in this desktop runtime');
      }
      await audioCtx.audioWorklet.addModule(VOICE_PCM_WORKLET_URL);

      const source = audioCtx.createMediaStreamSource(stream);
      sourceRef.current = source;
      const analyserNode = audioCtx.createAnalyser();
      analyserNode.fftSize = 1024;
      analyserRef.current = analyserNode;

      const chunkFrames = Math.max(
        1,
        Math.round(options.targetSampleRate * WORKLET_CHUNK_DURATION_MS / 1000),
      );
      const worklet = new AudioWorkletNode(audioCtx, VOICE_PCM_PROCESSOR_NAME, {
        numberOfInputs: 1,
        numberOfOutputs: 1,
        outputChannelCount: [1],
        channelCount: 1,
        channelCountMode: 'explicit',
        processorOptions: {
          targetSampleRate: options.targetSampleRate,
          chunkFrames,
          maxCredits: WORKLET_MAX_CREDITS,
          maxPendingChunks: WORKLET_MAX_PENDING_CHUNKS,
        },
      });
      workletRef.current = worklet;
      worklet.port.onmessage = (event: MessageEvent<VoiceWorkletMessage>) => {
        const message = event.data;
        if (message.type === 'pcm') {
          try {
            const accepted = recordingOptionsRef.current?.onPcmChunk(
              new Uint8Array(message.buffer),
            );
            if (accepted === false) reportCaptureIssue('interrupted');
          } catch {
            reportCaptureIssue('interrupted');
          } finally {
            worklet.port.postMessage({ type: 'ack' });
          }
        } else if (message.type === 'overflow') {
          reportCaptureIssue('interrupted');
        } else if (message.type === 'flushed') {
          const waiter = flushWaitersRef.current.get(message.requestId);
          if (waiter) {
            clearTimeout(waiter.timeout);
            flushWaitersRef.current.delete(message.requestId);
            waiter.resolve();
          }
        }
      };

      const mute = audioCtx.createGain();
      mute.gain.value = 0;
      muteRef.current = mute;
      source.connect(analyserNode);
      analyserNode.connect(worklet);
      worklet.connect(mute);
      mute.connect(audioCtx.destination);

      for (const track of stream.getAudioTracks()) {
        track.onended = () => reportCaptureIssue('disconnected');
      }
      if (audioCtx.state === 'suspended') await audioCtx.resume();

      recordingRef.current = true;
      pausedRef.current = false;
      accumulatedDurationMsRef.current = 0;
      activeSegmentStartedAtRef.current = Date.now();
      setIsRecording(true);
      setIsPaused(false);
      setCaptureState('capturing');
      setAnalyser(analyserNode);
      timerRef.current = setInterval(updateDuration, 250);
    } catch (error) {
      teardown(true);
      setIsRecording(false);
      setIsPaused(false);
      setCaptureState('idle');
      throw error;
    }
  }, [deviceId, reportCaptureIssue, teardown, updateDuration]);

  const pauseRecording = useCallback(async () => {
    if (!recordingRef.current || pausedRef.current) return;
    pausedRef.current = true;
    accumulatedDurationMsRef.current += Date.now() - activeSegmentStartedAtRef.current;
    activeSegmentStartedAtRef.current = 0;
    updateDuration();
    await flushWorklet({ pauseAfter: true });
    await audioCtxRef.current?.suspend();
    setIsPaused(true);
    setCaptureState('paused');
  }, [flushWorklet, updateDuration]);

  const resumeRecording = useCallback(async () => {
    if (!recordingRef.current || !pausedRef.current) return;
    await audioCtxRef.current?.resume();
    workletRef.current?.port.postMessage({ type: 'resume' });
    pausedRef.current = false;
    activeSegmentStartedAtRef.current = Date.now();
    setIsPaused(false);
    setCaptureState('capturing');
  }, []);

  const stopRecording = useCallback(async (): Promise<void> => {
    if (!recordingRef.current) return;
    if (!pausedRef.current) {
      accumulatedDurationMsRef.current += Date.now() - activeSegmentStartedAtRef.current;
    }
    activeSegmentStartedAtRef.current = 0;
    updateDuration();
    recordingRef.current = false;
    setIsRecording(false);
    setIsPaused(false);
    setCaptureState('idle');
    await flushWorklet({ stopAfter: true });
    teardown(false);
  }, [flushWorklet, teardown, updateDuration]);

  const cancelRecording = useCallback(() => {
    cancelledRef.current = true;
    recordingRef.current = false;
    setIsRecording(false);
    setIsPaused(false);
    setCaptureState('idle');
    teardown(true);
  }, [teardown]);

  return {
    isRecording,
    isPaused,
    captureState,
    startRecording,
    pauseRecording,
    resumeRecording,
    stopRecording,
    cancelRecording,
    recordingDuration,
    analyser,
  };
}
