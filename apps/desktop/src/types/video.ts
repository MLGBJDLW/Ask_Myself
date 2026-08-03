export type WhisperModel = 'tiny' | 'base' | 'small' | 'medium' | 'large' | 'large_turbo';
export type MediaTranscriptionMode = 'local_whisper' | 'inherit_speech_to_text' | 'disabled';
export type MediaFailurePolicy = 'best_effort' | 'require_transcript';

export interface VideoConfig {
  enabled: boolean;
  transcriptionMode: MediaTranscriptionMode;
  failurePolicy: MediaFailurePolicy;
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

export interface CapabilityRuntimeStatus {
  configured: boolean;
  ready: boolean;
  degraded: boolean;
  reason: string | null;
}

export interface MediaRuntimeStatus {
  enabled: boolean;
  transcriptionMode: MediaTranscriptionMode;
  failurePolicy: MediaFailurePolicy;
  probe: CapabilityRuntimeStatus;
  transcription: CapabilityRuntimeStatus;
  visualAnalysis: CapabilityRuntimeStatus;
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

/** Metadata projected from the persisted document record. */
export interface VideoMetadata {
  durationSecs: number | null;
  width: number | null;
  height: number | null;
  codec: string | null;
  framerate: number | null;
  thumbnailPath: string | null;
  creationTime: string | null;
}

/** Metadata returned directly by ffprobe during a native analysis run. */
export interface VideoAnalysisMetadata {
  durationSecs: number | null;
  width: number | null;
  height: number | null;
  codec: string | null;
  bitrate: number | null;
  framerate: number | null;
  creationTime: string | null;
}

export interface VisualEvent {
  timestampMs: number;
  endMs: number;
  text: string;
  confidence: number;
  source: string;
}

export interface MediaAnalysisWarning {
  code: string;
  message: string;
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
  visualEvents: VisualEvent[];
  warnings: MediaAnalysisWarning[];
  thumbnailPath: string | null;
  metadata: VideoAnalysisMetadata | null;
}
