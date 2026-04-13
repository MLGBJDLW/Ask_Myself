/**
 * Global progress store — persists progress state across page navigation.
 * Events are dispatched here by ProgressProvider and read by useProgress().
 */

import { useSyncExternalStore } from 'react';
import type { DownloadProgress, ScanProgress, BatchProgress, FtsProgress } from '../types/ingest';
import type { OcrDownloadProgress } from '../types/ocr';
import type { VideoDownloadProgress, FfmpegDownloadProgress } from '../types/video';
import type { ProcessingPhase } from '../components/media/VideoProcessingProgress';

/* ── Exported types ─────────────────────────────────────────────── */

export interface CompileProgress {
  current: number;
  total: number;
  documentId: string;
  documentTitle: string | null;
  phase: string;
}

export interface VideoProcessingState {
  phase: ProcessingPhase;
  progress: number;
  fileName: string;
}

export interface ProgressState {
  // Model downloads
  modelDownload: DownloadProgress | null;
  ocrDownload: OcrDownloadProgress | null;
  videoDownload: VideoDownloadProgress | null;
  ffmpegDownload: FfmpegDownloadProgress | null;

  // Batch operations
  scanProgress: ScanProgress | null;
  batchProgress: BatchProgress | null;
  ftsProgress: FtsProgress | null;
  embedRebuildProgress: ScanProgress | null;

  // Compile
  compileProgress: CompileProgress | null;

  // Video processing
  videoProcessing: VideoProcessingState | null;
}

/* ── Store implementation ───────────────────────────────────────── */

type Listener = () => void;

function createDefaultState(): ProgressState {
  return {
    modelDownload: null,
    ocrDownload: null,
    videoDownload: null,
    ffmpegDownload: null,
    scanProgress: null,
    batchProgress: null,
    ftsProgress: null,
    embedRebuildProgress: null,
    compileProgress: null,
    videoProcessing: null,
  };
}

let state: ProgressState = createDefaultState();
const listeners = new Set<Listener>();

function notify(): void {
  for (const cb of listeners) {
    cb();
  }
}

export const progressStore = {
  getState(): ProgressState {
    return state;
  },

  update<K extends keyof ProgressState>(key: K, value: ProgressState[K]): void {
    if (state[key] === value) return;
    state = { ...state, [key]: value };
    notify();
  },

  subscribe(callback: Listener): () => void {
    listeners.add(callback);
    return () => {
      listeners.delete(callback);
    };
  },
};

/* ── React hook ─────────────────────────────────────────────────── */

export function useProgress(): ProgressState {
  return useSyncExternalStore(
    progressStore.subscribe,
    progressStore.getState,
    progressStore.getState,
  );
}
