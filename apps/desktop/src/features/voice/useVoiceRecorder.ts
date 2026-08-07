import { useState, useRef, useCallback, useEffect } from 'react';
import { StreamingPcm16Encoder } from './realtimePcm';

export interface VoiceRecordingOptions {
  /** PCM sample rate supplied to onPcmChunk. */
  targetSampleRate: number;
  /** Receives ordered, mono, little-endian PCM16 chunks while recording. */
  onPcmChunk: (chunk: Uint8Array) => void;
}

export interface UseVoiceRecorderReturn {
  isRecording: boolean;
  startRecording: (options: VoiceRecordingOptions) => Promise<void>;
  stopRecording: () => Promise<void>;
  cancelRecording: () => void;
  recordingDuration: number;
  /** Analyser tapped off the live capture graph, for waveform visualization. */
  analyser: AnalyserNode | null;
}

/**
 * Captures microphone audio into fixed, resampled PCM16 chunks. The hook never
 * retains a complete recording; callers must stream every chunk to a bounded
 * native or provider adapter.
 */
export function useVoiceRecorder(deviceId?: string | null): UseVoiceRecorderReturn {
  const [isRecording, setIsRecording] = useState(false);
  const [recordingDuration, setRecordingDuration] = useState(0);
  const [analyser, setAnalyser] = useState<AnalyserNode | null>(null);

  const streamRef = useRef<MediaStream | null>(null);
  const audioCtxRef = useRef<AudioContext | null>(null);
  const processorRef = useRef<ScriptProcessorNode | null>(null);
  const sourceRef = useRef<MediaStreamAudioSourceNode | null>(null);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const cancelledRef = useRef(false);
  const recordingOptionsRef = useRef<VoiceRecordingOptions | null>(null);
  const pcmEncoderRef = useRef<StreamingPcm16Encoder | null>(null);

  const cleanup = useCallback(() => {
    if (timerRef.current) {
      clearInterval(timerRef.current);
      timerRef.current = null;
    }
    processorRef.current?.disconnect();
    sourceRef.current?.disconnect();
    audioCtxRef.current?.close().catch(() => {});
    streamRef.current?.getTracks().forEach((track) => track.stop());
    processorRef.current = null;
    sourceRef.current = null;
    audioCtxRef.current = null;
    streamRef.current = null;
    pcmEncoderRef.current = null;
    recordingOptionsRef.current = null;
    setAnalyser(null);
    setRecordingDuration(0);
  }, []);

  useEffect(() => cleanup, [cleanup]);

  const startRecording = useCallback(async (options: VoiceRecordingOptions) => {
    cancelledRef.current = false;
    recordingOptionsRef.current = options;
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
    pcmEncoderRef.current = new StreamingPcm16Encoder(
      audioCtx.sampleRate,
      options.targetSampleRate,
    );

    const source = audioCtx.createMediaStreamSource(stream);
    sourceRef.current = source;

    // AudioWorklet migration is intentionally isolated in the next roadmap
    // change. This interim callback is fixed-size and never retains full PCM.
    const processor = audioCtx.createScriptProcessor(4096, 1, 1);
    processorRef.current = processor;
    processor.onaudioprocess = (event) => {
      if (cancelledRef.current) return;
      const samples = new Float32Array(event.inputBuffer.getChannelData(0));
      const pcmChunk = pcmEncoderRef.current?.encode(samples);
      if (pcmChunk && pcmChunk.length > 0) {
        recordingOptionsRef.current?.onPcmChunk(pcmChunk);
      }
    };

    source.connect(processor);
    // ScriptProcessorNode requires a destination connection to emit callbacks.
    processor.connect(audioCtx.destination);

    const analyserNode = audioCtx.createAnalyser();
    analyserNode.fftSize = 1024;
    source.connect(analyserNode);
    setAnalyser(analyserNode);

    setIsRecording(true);
    setRecordingDuration(0);
    const startedAt = Date.now();
    timerRef.current = setInterval(() => {
      setRecordingDuration(Math.floor((Date.now() - startedAt) / 1000));
    }, 250);
  }, [deviceId]);

  const stopRecording = useCallback(async (): Promise<void> => {
    setIsRecording(false);
    cleanup();
  }, [cleanup]);

  const cancelRecording = useCallback(() => {
    cancelledRef.current = true;
    setIsRecording(false);
    cleanup();
  }, [cleanup]);

  return {
    isRecording,
    startRecording,
    stopRecording,
    cancelRecording,
    recordingDuration,
    analyser,
  };
}
