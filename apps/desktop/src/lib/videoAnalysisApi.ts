import { invoke } from '@tauri-apps/api/core';

import type { TranscriptSegment, VideoAnalysisResult } from '../types/video';

interface NativeTranscriptSegment {
  start_ms?: number;
  end_ms?: number;
  startMs?: number;
  endMs?: number;
  text: string;
}

interface NativeVideoAnalysisResult extends Omit<VideoAnalysisResult, 'transcriptSegments'> {
  transcriptSegments: NativeTranscriptSegment[];
}

/**
 * Run native video analysis and retain its timestamped transcript and frame OCR
 * timeline. Keep this focused entry point until the legacy monolithic API module
 * is split into domain clients.
 */
export async function analyzeVideoDetailed(path: string): Promise<VideoAnalysisResult> {
  const result = await invoke<NativeVideoAnalysisResult>('analyze_video_cmd', { path });
  return {
    ...result,
    transcriptSegments: result.transcriptSegments.map(normalizeTranscriptSegment),
  };
}

function normalizeTranscriptSegment(segment: NativeTranscriptSegment): TranscriptSegment {
  const startMs = segment.startMs ?? segment.start_ms;
  const endMs = segment.endMs ?? segment.end_ms;
  if (!Number.isFinite(startMs) || !Number.isFinite(endMs)) {
    throw new Error('Video analysis returned a transcript segment without valid timestamps');
  }
  return {
    startMs: startMs as number,
    endMs: endMs as number,
    text: segment.text,
  };
}
