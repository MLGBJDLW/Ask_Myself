export type WhisperModel = 'tiny' | 'base' | 'small' | 'medium' | 'large' | 'large_turbo';

export interface VideoConfig {
  enabled: boolean;
  whisperModel: WhisperModel;
  language: string | null; // null = auto-detect
  translateToEnglish: boolean;
  ffmpegPath: string | null;
  frameExtractionEnabled: boolean;
  frameIntervalSecs: number;
  modelPath: string;
  sceneThreshold: number;      // 0.1-0.9
  useGpu: boolean;
  preferEmbeddedSubtitles: boolean;
  beamSize: number;            // 1-10
}

export interface VideoDownloadProgress {
  filename: string;
  bytesDownloaded: number;
  totalBytes: number | null;
}

export interface FfmpegDownloadProgress {
  progressPct: number;
  status: string;
}

export interface TranscriptSegment {
  startMs: number;
  endMs: number;
  text: string;
}

export interface TranscriptChunk {
  text: string;
  startMs: number | null;
  endMs: number | null;
  chunkType: string; // 'transcript' | 'frame_ocr' | 'subtitle'
}

export interface VideoMetadata {
  durationSecs: number | null;
  width: number | null;
  height: number | null;
  codec: string | null;
  framerate: number | null;
  thumbnailPath: string | null;
  creationTime: string | null;
}

/**
 * Structured result returned by the native video-analysis command.
 *
 * Speaker identity, word alignment, and overlapping-speech metadata are not
 * available yet; those belong to the meeting-analysis pipeline described in
 * the platform capability audit.
 */
export interface VideoAnalysisResult {
  transcript: string;
  segmentCount: number;
  transcriptSegments: TranscriptSegment[];
  durationSecs: number | null;
  frameTextsCount: number;
  frameTexts: string[];
  thumbnailPath: string | null;
  metadata: VideoMetadata | null;
}
