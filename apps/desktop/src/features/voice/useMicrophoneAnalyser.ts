import { useCallback, useEffect, useRef, useState } from 'react';

export type MicrophoneAnalyserError = 'permission_denied' | 'unavailable';

export interface UseMicrophoneAnalyserReturn {
  /** Live analyser node while active, otherwise null. */
  analyser: AnalyserNode | null;
  error: MicrophoneAnalyserError | null;
}

/** Small window keeps the bars responsive without smearing the waveform. */
const FFT_SIZE = 1024;

function isPermissionDenied(error: unknown): boolean {
  return (
    typeof error === 'object'
    && error !== null
    && 'name' in error
    && (error as { name?: string }).name === 'NotAllowedError'
  );
}

/**
 * Open the selected microphone and expose an analyser for live visualization.
 *
 * The stream is only held while `active` is true so the OS recording indicator
 * mirrors what the user asked for, and switching devices restarts cleanly.
 */
export function useMicrophoneAnalyser(
  deviceId: string | null | undefined,
  active: boolean,
): UseMicrophoneAnalyserReturn {
  const [analyser, setAnalyser] = useState<AnalyserNode | null>(null);
  const [error, setError] = useState<MicrophoneAnalyserError | null>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const contextRef = useRef<AudioContext | null>(null);

  const teardown = useCallback(() => {
    streamRef.current?.getTracks().forEach((track) => track.stop());
    streamRef.current = null;
    contextRef.current?.close().catch(() => {});
    contextRef.current = null;
    setAnalyser(null);
  }, []);

  useEffect(() => {
    if (!active) {
      teardown();
      setError(null);
      return;
    }

    let cancelled = false;
    setError(null);

    void (async () => {
      try {
        const stream = await navigator.mediaDevices.getUserMedia({
          audio: deviceId ? { deviceId: { exact: deviceId } } : true,
        });
        if (cancelled) {
          stream.getTracks().forEach((track) => track.stop());
          return;
        }
        const context = new AudioContext();
        const node = context.createAnalyser();
        node.fftSize = FFT_SIZE;
        context.createMediaStreamSource(stream).connect(node);
        streamRef.current = stream;
        contextRef.current = context;
        setAnalyser(node);
      } catch (cause) {
        if (cancelled) return;
        setError(isPermissionDenied(cause) ? 'permission_denied' : 'unavailable');
      }
    })();

    return () => {
      cancelled = true;
      teardown();
    };
  }, [active, deviceId, teardown]);

  return { analyser, error };
}
