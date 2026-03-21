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
