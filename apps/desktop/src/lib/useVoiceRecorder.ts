import { useState, useRef, useCallback, useEffect } from 'react';

export interface UseVoiceRecorderReturn {
  isRecording: boolean;
  isProcessing: boolean;
  startRecording: () => Promise<void>;
  stopRecording: () => Promise<Uint8Array | null>;
  cancelRecording: () => void;
  recordingDuration: number;
}

/**
 * Captures microphone audio as raw PCM, resamples to 16 kHz mono,
 * and returns a WAV‑encoded Uint8Array ready for Whisper transcription.
 * No FFmpeg required — all processing happens in the browser.
 *
 * @param deviceId - Optional audio input device ID. When provided the
 *   exact device is requested; falls back to the default mic on error.
 */
export function useVoiceRecorder(deviceId?: string | null): UseVoiceRecorderReturn {
  const [isRecording, setIsRecording] = useState(false);
  const [isProcessing, setIsProcessing] = useState(false);
  const [recordingDuration, setRecordingDuration] = useState(0);

  const streamRef = useRef<MediaStream | null>(null);
  const audioCtxRef = useRef<AudioContext | null>(null);
  const processorRef = useRef<ScriptProcessorNode | null>(null);
  const sourceRef = useRef<MediaStreamAudioSourceNode | null>(null);
  const buffersRef = useRef<Float32Array[]>([]);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const cancelledRef = useRef(false);

  const cleanup = useCallback(() => {
    if (timerRef.current) {
      clearInterval(timerRef.current);
      timerRef.current = null;
    }
    processorRef.current?.disconnect();
    sourceRef.current?.disconnect();
    audioCtxRef.current?.close().catch(() => {});
    streamRef.current?.getTracks().forEach((t) => t.stop());
    processorRef.current = null;
    sourceRef.current = null;
    audioCtxRef.current = null;
    streamRef.current = null;
    buffersRef.current = [];
    setRecordingDuration(0);
  }, []);

  // Cleanup on unmount
  useEffect(() => cleanup, [cleanup]);

  const startRecording = useCallback(async () => {
    cancelledRef.current = false;
    let stream: MediaStream;
    if (deviceId) {
      try {
        stream = await navigator.mediaDevices.getUserMedia({
          audio: { deviceId: { exact: deviceId } },
        });
      } catch {
        // Selected device unavailable — fall back to default
        console.warn(`[useVoiceRecorder] deviceId ${deviceId} unavailable, falling back to default`);
        stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      }
    } else {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    }
    streamRef.current = stream;

    const audioCtx = new AudioContext();
    audioCtxRef.current = audioCtx;

    const source = audioCtx.createMediaStreamSource(stream);
    sourceRef.current = source;

    // ScriptProcessorNode captures raw PCM (mono, channel 0)
    const processor = audioCtx.createScriptProcessor(4096, 1, 1);
    processorRef.current = processor;
    buffersRef.current = [];

    processor.onaudioprocess = (e) => {
      if (!cancelledRef.current) {
        buffersRef.current.push(new Float32Array(e.inputBuffer.getChannelData(0)));
      }
    };

    source.connect(processor);
    // Must connect to destination for onaudioprocess to fire
    processor.connect(audioCtx.destination);

    setIsRecording(true);
    setRecordingDuration(0);
    const start = Date.now();
    timerRef.current = setInterval(() => {
      setRecordingDuration(Math.floor((Date.now() - start) / 1000));
    }, 250);
  }, [deviceId]);

  const stopRecording = useCallback(async (): Promise<Uint8Array | null> => {
    if (!audioCtxRef.current || cancelledRef.current) {
      cleanup();
      setIsRecording(false);
      return null;
    }

    const sourceSampleRate = audioCtxRef.current.sampleRate;
    const buffers = buffersRef.current.slice();

    // Stop capturing
    setIsRecording(false);
    cleanup();

    if (buffers.length === 0) return null;

    setIsProcessing(true);
    try {
      // Merge buffers into one Float32Array
      const totalLength = buffers.reduce((sum, b) => sum + b.length, 0);
      const merged = new Float32Array(totalLength);
      let offset = 0;
      for (const buf of buffers) {
        merged.set(buf, offset);
        offset += buf.length;
      }

      // Resample to 16 kHz
      const resampled = await resampleTo16k(merged, sourceSampleRate);
      // Encode as 16‑bit PCM WAV
      return encodeWav(resampled, 16000);
    } finally {
      setIsProcessing(false);
    }
  }, [cleanup]);

  const cancelRecording = useCallback(() => {
    cancelledRef.current = true;
    setIsRecording(false);
    cleanup();
  }, [cleanup]);

  return { isRecording, isProcessing, startRecording, stopRecording, cancelRecording, recordingDuration };
}

// ── helpers ──────────────────────────────────────────────────────────

async function resampleTo16k(audioData: Float32Array, sourceSampleRate: number): Promise<Float32Array> {
  if (sourceSampleRate === 16000) return audioData;

  const duration = audioData.length / sourceSampleRate;
  const targetLength = Math.round(duration * 16000);
  const offlineCtx = new OfflineAudioContext(1, targetLength, 16000);
  const buffer = offlineCtx.createBuffer(1, audioData.length, sourceSampleRate);
  buffer.getChannelData(0).set(audioData);
  const src = offlineCtx.createBufferSource();
  src.buffer = buffer;
  src.connect(offlineCtx.destination);
  src.start();
  const rendered = await offlineCtx.startRendering();
  return rendered.getChannelData(0);
}

function encodeWav(samples: Float32Array, sampleRate: number): Uint8Array {
  const numChannels = 1;
  const bitsPerSample = 16;
  const byteRate = sampleRate * numChannels * (bitsPerSample / 8);
  const blockAlign = numChannels * (bitsPerSample / 8);
  const dataSize = samples.length * (bitsPerSample / 8);
  const buffer = new ArrayBuffer(44 + dataSize);
  const view = new DataView(buffer);

  // RIFF header
  writeString(view, 0, 'RIFF');
  view.setUint32(4, 36 + dataSize, true);
  writeString(view, 8, 'WAVE');

  // fmt sub-chunk
  writeString(view, 12, 'fmt ');
  view.setUint32(16, 16, true);           // sub-chunk size
  view.setUint16(20, 1, true);            // PCM format
  view.setUint16(22, numChannels, true);
  view.setUint32(24, sampleRate, true);
  view.setUint32(28, byteRate, true);
  view.setUint16(32, blockAlign, true);
  view.setUint16(34, bitsPerSample, true);

  // data sub-chunk
  writeString(view, 36, 'data');
  view.setUint32(40, dataSize, true);

  // Convert float32 → int16
  let offset = 44;
  for (let i = 0; i < samples.length; i++) {
    const s = Math.max(-1, Math.min(1, samples[i]));
    view.setInt16(offset, s < 0 ? s * 0x8000 : s * 0x7fff, true);
    offset += 2;
  }

  return new Uint8Array(buffer);
}

function writeString(view: DataView, offset: number, str: string) {
  for (let i = 0; i < str.length; i++) {
    view.setUint8(offset + i, str.charCodeAt(i));
  }
}
