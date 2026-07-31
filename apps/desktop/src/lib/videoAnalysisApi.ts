import { invoke } from '@tauri-apps/api/core';

import type { VideoAnalysisResult } from '../types/video';

/**
 * Run native video analysis and retain its timestamped transcript and frame OCR
 * timeline. Keep this focused entry point until the legacy monolithic API module
 * is split into domain clients.
 */
export const analyzeVideoDetailed = (path: string) =>
  invoke<VideoAnalysisResult>('analyze_video_cmd', { path });
