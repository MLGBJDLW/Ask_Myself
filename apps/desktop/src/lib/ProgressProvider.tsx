import { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { progressStore } from './progressStore';
import type { CompileProgress } from './progressStore';
import type { DownloadProgress, ScanProgress, BatchProgress, FtsProgress } from '../types/ingest';
import type { OcrDownloadProgress } from '../types/ocr';
import type { VideoDownloadProgress, FfmpegDownloadProgress } from '../types/video';
import type { ProcessingPhase } from '../components/media/VideoProcessingProgress';

/**
 * Global Tauri event listener for all progress events.
 * Mount once at app root — never tears down, so progress survives page navigation.
 */
export function ProgressProvider() {
  useEffect(() => {
    let cancelled = false;
    const unlisteners: (() => void)[] = [];

    function reg<T>(event: string, handler: (payload: T) => void) {
      listen<T>(event, (e) => {
        if (!cancelled) handler(e.payload);
      }).then((fn) => {
        if (cancelled) { fn(); } else { unlisteners.push(fn); }
      });
    }

    // Model downloads
    reg<DownloadProgress>('model:download-progress', (p) => {
      progressStore.update('modelDownload', p);
    });

    reg<OcrDownloadProgress>('ocr:download-progress', (p) => {
      progressStore.update('ocrDownload', p);
    });

    reg<VideoDownloadProgress>('video:download-progress', (p) => {
      progressStore.update('videoDownload', p);
    });

    reg<FfmpegDownloadProgress>('ffmpeg:download-progress', (p) => {
      progressStore.update('ffmpegDownload', p);
    });

    // Scan / batch
    reg<ScanProgress>('source:scan-progress', (p) => {
      progressStore.update('scanProgress', p);
    });

    reg<BatchProgress>('batch:scan-progress', (p) => {
      progressStore.update('batchProgress', p);
    });

    reg<ScanProgress>('batch:rebuild-progress', (p) => {
      progressStore.update('embedRebuildProgress', p);
    });

    reg<FtsProgress>('batch:fts-progress', (p) => {
      if (p.phase === 'complete') {
        progressStore.update('ftsProgress', null);
      } else {
        progressStore.update('ftsProgress', p);
      }
    });

    // Compile
    reg<CompileProgress>('compile:progress', (p) => {
      progressStore.update('compileProgress', p);
    });

    // Video processing
    reg<{ phase: ProcessingPhase; progress: number; fileName: string }>(
      'video:processing-progress',
      (p) => {
        if (p.phase === 'complete') {
          progressStore.update('videoProcessing', null);
        } else {
          progressStore.update('videoProcessing', p);
        }
      },
    );

    return () => {
      cancelled = true;
      for (const fn of unlisteners) fn();
    };
  }, []);

  return null;
}
