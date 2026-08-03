//! Video analysis: FFmpeg audio extraction + Whisper speech-to-text.
//!
//! Follows the same pattern as `ocr.rs`: config struct, model management,
//! DB persistence, progress callbacks.

use crate::app_settings::SpeechToTextConfig;
use crate::db::Database;
use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static ACTIVE_MEDIA_JOBS: AtomicUsize = AtomicUsize::new(0);
static MEDIA_ANALYSIS_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();

struct MediaJobLease {
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl MediaJobLease {
    fn acquire() -> Self {
        ACTIVE_MEDIA_JOBS.fetch_add(1, Ordering::SeqCst);
        let guard = MEDIA_ANALYSIS_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self { _guard: guard }
    }
}

impl Drop for MediaJobLease {
    fn drop(&mut self) {
        ACTIVE_MEDIA_JOBS.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn media_jobs_active() -> bool {
    ACTIVE_MEDIA_JOBS.load(Ordering::SeqCst) > 0
}

// ═══════════════════════════════════════════
// Configuration
// ═══════════════════════════════════════════

/// Whisper model sizes with file size and accuracy trade-offs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhisperModel {
    Tiny, // ~39 MB, fastest, lowest accuracy
    #[default]
    Base, // ~142 MB, good balance
    Small, // ~466 MB, better accuracy
    Medium, // ~1.5 GB, high accuracy
    Large, // ~3.1 GB, most accurate
    LargeTurbo, // ~1.6 GB, best speed/accuracy tradeoff
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaTranscriptionMode {
    LocalWhisper,
    #[default]
    InheritSpeechToText,
    Disabled,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaFailurePolicy {
    #[default]
    BestEffort,
    RequireTranscript,
}

impl WhisperModel {
    /// HuggingFace repo ID for downloading model files.
    pub fn repo_id(&self) -> &'static str {
        match self {
            Self::Tiny => "openai/whisper-tiny",
            Self::Base => "openai/whisper-base",
            Self::Small => "openai/whisper-small",
            Self::Medium => "openai/whisper-medium",
            Self::Large => "openai/whisper-large-v3",
            Self::LargeTurbo => "openai/whisper-large-v3-turbo",
        }
    }

    /// Local directory name for storing model files.
    pub fn dir_name(&self) -> &'static str {
        match self {
            Self::Tiny => "whisper-tiny",
            Self::Base => "whisper-base",
            Self::Small => "whisper-small",
            Self::Medium => "whisper-medium",
            Self::Large => "whisper-large-v3",
            Self::LargeTurbo => "whisper-large-v3-turbo",
        }
    }

    /// Files needed to run this model (config, tokenizer, and weights).
    pub fn required_files(&self) -> Vec<&'static str> {
        let mut files = vec!["config.json", "tokenizer.json"];
        match self {
            Self::Large => {
                files.push("model-00001-of-00002.safetensors");
                files.push("model-00002-of-00002.safetensors");
            }
            _ => {
                files.push("model.safetensors");
            }
        }
        files
    }

    /// Weight file(s) only (safetensors).
    pub fn weight_files(&self) -> Vec<&'static str> {
        match self {
            Self::Large => vec![
                "model-00001-of-00002.safetensors",
                "model-00002-of-00002.safetensors",
            ],
            _ => vec!["model.safetensors"],
        }
    }

    /// HuggingFace download URLs for all required files.
    pub fn download_urls(&self) -> Vec<(&'static str, String)> {
        let repo_id = self.repo_id();
        self.required_files()
            .iter()
            .map(|f| {
                (
                    *f,
                    format!("https://huggingface.co/{repo_id}/resolve/main/{f}"),
                )
            })
            .collect()
    }

    /// Mirror URLs for regions where `huggingface.co` is unreachable.
    /// Host swapped: `huggingface.co` → `<mirror_base>`.
    /// Returns an empty vec when `mirror_base` is empty (fallback disabled).
    pub fn fallback_download_urls(&self, mirror_base: &str) -> Vec<(&'static str, String)> {
        let base = mirror_base.trim().trim_end_matches('/');
        if base.is_empty() {
            return Vec::new();
        }
        let repo_id = self.repo_id();
        self.required_files()
            .iter()
            .map(|f| (*f, format!("{base}/{repo_id}/resolve/main/{f}")))
            .collect()
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Tiny => "Tiny (~151 MB)",
            Self::Base => "Base (~290 MB)",
            Self::Small => "Small (~967 MB)",
            Self::Medium => "Medium (~3.1 GB)",
            Self::Large => "Large v3 (~6.2 GB)",
            Self::LargeTurbo => "Large v3 Turbo (~3.1 GB)",
        }
    }
}

/// User-configurable video analysis settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoConfig {
    pub enabled: bool,
    #[serde(default)]
    pub transcription_mode: MediaTranscriptionMode,
    #[serde(default)]
    pub failure_policy: MediaFailurePolicy,
    #[serde(alias = "whisper_model")]
    pub whisper_model: WhisperModel,
    pub language: Option<String>, // None = auto-detect
    #[serde(alias = "translate_to_english")]
    pub translate_to_english: bool,
    #[serde(alias = "ffmpeg_path")]
    pub ffmpeg_path: Option<String>, // None = use system PATH
    #[serde(alias = "frame_extraction_enabled")]
    pub frame_extraction_enabled: bool,
    #[serde(alias = "frame_interval_secs")]
    pub frame_interval_secs: u32, // Extract 1 frame every N seconds
    #[serde(alias = "model_path")]
    pub model_path: String,
    #[serde(default = "default_scene_threshold", alias = "scene_threshold")]
    pub scene_threshold: f64,
    #[serde(default, alias = "use_gpu")]
    pub use_gpu: bool,
    #[serde(default = "default_true", alias = "prefer_embedded_subtitles")]
    pub prefer_embedded_subtitles: bool,
    #[serde(default = "default_beam_size", alias = "beam_size")]
    pub beam_size: u32,
}

fn default_scene_threshold() -> f64 {
    0.4
}
fn default_true() -> bool {
    true
}
fn default_beam_size() -> u32 {
    1
}

impl Default for VideoConfig {
    fn default() -> Self {
        let model_path = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(crate::APP_DIR)
            .join("models")
            .join("whisper")
            .to_string_lossy()
            .to_string();
        Self {
            enabled: false, // Disabled by default (unlike OCR)
            transcription_mode: MediaTranscriptionMode::default(),
            failure_policy: MediaFailurePolicy::default(),
            whisper_model: WhisperModel::default(),
            language: None,
            translate_to_english: false,
            ffmpeg_path: None,
            frame_extraction_enabled: false,
            frame_interval_secs: 10,
            model_path,
            scene_threshold: default_scene_threshold(),
            use_gpu: false,
            prefer_embedded_subtitles: default_true(),
            beam_size: default_beam_size(),
        }
    }
}

// ═══════════════════════════════════════════
// Progress & Results
// ═══════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoDownloadProgress {
    pub filename: String,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoProcessingProgress {
    pub phase: String, // "extracting_audio", "transcribing", "extracting_frames", "ocr"
    pub progress_pct: f32,
    pub detail: Option<String>,
}

/// A timestamped segment of transcribed speech.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSegment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualEvent {
    pub timestamp_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub confidence: f32,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedFrame {
    pub path: PathBuf,
    pub timestamp_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaAnalysisWarning {
    pub code: String,
    pub message: String,
}

/// Result of video analysis.
#[derive(Debug, Clone)]
pub struct VideoAnalysisResult {
    pub transcript_segments: Vec<TranscriptSegment>,
    pub full_transcript: String,
    pub duration_secs: Option<f64>,
    pub visual_events: Vec<VisualEvent>,
    pub warnings: Vec<MediaAnalysisWarning>,
    pub thumbnail_path: Option<PathBuf>,
    pub metadata: Option<VideoMetadata>,
}

/// Rich metadata extracted from a video file via ffprobe.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMetadata {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub codec: Option<String>,
    pub bitrate: Option<u64>,
    pub framerate: Option<f64>,
    #[serde(alias = "creation_time")]
    pub creation_time: Option<String>,
    #[serde(alias = "duration_secs")]
    pub duration_secs: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct MediaProbe {
    pub has_audio: bool,
    pub subtitle_streams: Vec<SubtitleStream>,
    pub metadata: VideoMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityRuntimeStatus {
    pub configured: bool,
    pub ready: bool,
    pub degraded: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaRuntimeStatus {
    pub enabled: bool,
    pub transcription_mode: MediaTranscriptionMode,
    pub failure_policy: MediaFailurePolicy,
    pub probe: CapabilityRuntimeStatus,
    pub transcription: CapabilityRuntimeStatus,
    pub visual_analysis: CapabilityRuntimeStatus,
}

/// Metadata for an embedded subtitle stream.
#[derive(Debug, Clone)]
pub struct SubtitleStream {
    pub index: usize,
    pub language: Option<String>,
    pub codec_name: String,
    pub title: Option<String>,
}

// ═══════════════════════════════════════════
// FFmpeg Operations
// ═══════════════════════════════════════════

/// Derive ffprobe path from the ffmpeg binary path.
fn derive_ffprobe_path(ffmpeg_path: &str) -> String {
    let path = std::path::Path::new(ffmpeg_path);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("ffmpeg");
    let ffprobe_name = if stem.eq_ignore_ascii_case("ffmpeg") {
        if ext.is_empty() {
            "ffprobe".to_string()
        } else {
            format!("ffprobe.{ext}")
        }
    } else {
        let replaced = stem.replace("ffmpeg", "ffprobe");
        if ext.is_empty() {
            replaced
        } else {
            format!("{replaced}.{ext}")
        }
    };
    if let Some(parent) = path.parent() {
        if parent != Path::new("") && parent != Path::new(".") {
            return parent.join(&ffprobe_name).to_string_lossy().to_string();
        }
    }
    ffprobe_name
}

/// Run an FFmpeg/ffprobe command with a timeout. Kills the child process on timeout.
fn run_ffmpeg_command(
    program: &str,
    args: &[&str],
    timeout_secs: u64,
) -> Result<std::process::Output, CoreError> {
    use std::io::Read;
    use std::process::Stdio;

    let mut child = std::process::Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CoreError::Video(format!("Failed to run {program}: {e}")))?;

    // Take ownership of pipes and drain them in background threads
    // to prevent OS pipe buffer (64 KB on Windows) from filling up
    // and deadlocking the child process.
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let stdout_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = stdout_pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });

    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = stderr_pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });

    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdout_handle.join();
                    let _ = stderr_handle.join();
                    return Err(CoreError::Video(format!(
                        "{program} timed out after {timeout_secs} seconds"
                    )));
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_handle.join();
                let _ = stderr_handle.join();
                return Err(CoreError::Video(format!(
                    "Failed to check process status: {e}"
                )));
            }
        }
    };

    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();

    if !status.success() {
        let stderr_str = String::from_utf8_lossy(&stderr);
        return Err(CoreError::Video(format!("{program} failed: {stderr_str}")));
    }

    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// Check if FFmpeg is available on the system.
pub fn check_ffmpeg(config: &VideoConfig) -> Result<bool, CoreError> {
    let ffmpeg = config.ffmpeg_path.as_deref().unwrap_or("ffmpeg");
    match std::process::Command::new(ffmpeg)
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) => Ok(status.success()),
        Err(_) => Ok(false),
    }
}

/// Extract audio from video file as WAV (16kHz mono PCM, required by Whisper).
pub fn extract_audio(
    video_path: &Path,
    output_wav: &Path,
    config: &VideoConfig,
) -> Result<(), CoreError> {
    let ffmpeg = config.ffmpeg_path.as_deref().unwrap_or("ffmpeg");
    let canonical_path =
        std::fs::canonicalize(video_path).unwrap_or_else(|_| video_path.to_path_buf());
    let video = canonical_path.to_string_lossy();
    let output = output_wav.to_string_lossy();
    run_ffmpeg_command(
        ffmpeg,
        &[
            "-i",
            &video,
            "-vn", // No video
            "-acodec",
            "pcm_s16le", // 16-bit PCM
            "-ar",
            "16000", // 16kHz (Whisper requirement)
            "-ac",
            "1",  // Mono
            "-y", // Overwrite
            &output,
        ],
        3600,
    )?;
    Ok(())
}

/// Extract key frames from video at fixed intervals.
pub fn extract_frames(
    video_path: &Path,
    output_dir: &Path,
    interval_secs: u32,
    config: &VideoConfig,
) -> Result<Vec<ExtractedFrame>, CoreError> {
    let interval_secs = interval_secs.max(1);
    std::fs::create_dir_all(output_dir)
        .map_err(|e| CoreError::Video(format!("Failed to create frame output dir: {e}")))?;

    let ffmpeg = config.ffmpeg_path.as_deref().unwrap_or("ffmpeg");
    let fps_filter = format!("fps=1/{interval_secs},showinfo");
    let output_pattern = output_dir.join("frame_%04d.jpg");
    let canonical_path =
        std::fs::canonicalize(video_path).unwrap_or_else(|_| video_path.to_path_buf());
    let video = canonical_path.to_string_lossy();
    let pattern = output_pattern.to_string_lossy();

    let output = run_ffmpeg_command(
        ffmpeg,
        &[
            "-i",
            &video,
            "-vf",
            &fps_filter,
            "-q:v",
            "2", // High quality JPEG
            "-y",
            &pattern,
        ],
        1800,
    )?;

    // Collect extracted frame paths
    let mut frames: Vec<PathBuf> = std::fs::read_dir(output_dir)
        .map_err(|e| CoreError::Video(format!("Failed to read frame dir: {e}")))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "jpg"))
        .collect();
    frames.sort();
    let timestamps = parse_showinfo_timestamps(&String::from_utf8_lossy(&output.stderr));
    Ok(frames
        .into_iter()
        .enumerate()
        .map(|(index, path)| ExtractedFrame {
            path,
            timestamp_ms: timestamps
                .get(index)
                .copied()
                .unwrap_or(index as i64 * interval_secs as i64 * 1000),
        })
        .collect())
}

/// Extract keyframes using FFmpeg scene-change detection.
/// Falls back to empty vec if scene detection produces no frames.
pub fn extract_keyframes(
    video_path: &Path,
    output_dir: &Path,
    scene_threshold: f64,
    config: &VideoConfig,
) -> Result<Vec<ExtractedFrame>, CoreError> {
    std::fs::create_dir_all(output_dir)
        .map_err(|e| CoreError::Video(format!("Failed to create keyframe output dir: {e}")))?;

    let ffmpeg = config.ffmpeg_path.as_deref().unwrap_or("ffmpeg");
    let vf = format!("select='gt(scene,{scene_threshold})',showinfo");
    let output_pattern = output_dir.join("scene_%04d.jpg");
    let canonical_path =
        std::fs::canonicalize(video_path).unwrap_or_else(|_| video_path.to_path_buf());
    let video = canonical_path.to_string_lossy();
    let pattern = output_pattern.to_string_lossy();

    // Scene detection may legitimately produce zero frames (static video), so
    // we treat a non-zero exit as an error but zero output files as OK.
    let result = run_ffmpeg_command(
        ffmpeg,
        &[
            "-i", &video, "-vf", &vf, "-vsync", "vfr", "-q:v", "3", "-y", &pattern,
        ],
        1800,
    );

    let output = match result {
        Ok(output) => output,
        Err(e) => {
            tracing::warn!("Scene detection failed, will fall back to fixed-interval: {e}");
            return Ok(Vec::new());
        }
    };

    let mut frames: Vec<PathBuf> = std::fs::read_dir(output_dir)
        .map_err(|e| CoreError::Video(format!("Failed to read keyframe dir: {e}")))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "jpg"))
        .collect();
    frames.sort();
    let timestamps = parse_showinfo_timestamps(&String::from_utf8_lossy(&output.stderr));
    if frames.len() != timestamps.len() {
        tracing::warn!(
            "Scene timestamp count ({}) did not match frame count ({}); falling back to fixed interval",
            timestamps.len(),
            frames.len(),
        );
        return Ok(Vec::new());
    }
    Ok(frames
        .into_iter()
        .zip(timestamps)
        .map(|(path, timestamp_ms)| ExtractedFrame { path, timestamp_ms })
        .collect())
}

fn parse_showinfo_timestamps(stderr: &str) -> Vec<i64> {
    let marker = "pts_time:";
    stderr
        .lines()
        .filter_map(|line| {
            let value = line.split_once(marker)?.1.split_whitespace().next()?;
            value
                .parse::<f64>()
                .ok()
                .map(|seconds| (seconds * 1000.0).round() as i64)
        })
        .collect()
}

/// Get video duration in seconds using FFprobe.
pub fn get_video_duration(video_path: &Path, config: &VideoConfig) -> Result<f64, CoreError> {
    let ffmpeg = config.ffmpeg_path.as_deref().unwrap_or("ffmpeg");
    let ffprobe = derive_ffprobe_path(ffmpeg);
    let video = video_path.to_string_lossy();

    let output = run_ffmpeg_command(
        &ffprobe,
        &[
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
            &video,
        ],
        30,
    )?;

    let duration_str = String::from_utf8_lossy(&output.stdout);
    duration_str
        .trim()
        .parse::<f64>()
        .map_err(|e| CoreError::Video(format!("Failed to parse duration: {e}")))
}

// ═══════════════════════════════════════════
// Subtitle Extraction
// ═══════════════════════════════════════════

/// Detect embedded subtitle streams in a video file using ffprobe.
pub fn detect_subtitle_streams(
    ffmpeg_path: &str,
    video_path: &Path,
) -> Result<Vec<SubtitleStream>, CoreError> {
    let ffprobe = derive_ffprobe_path(ffmpeg_path);
    let video = video_path.to_string_lossy();

    let output = run_ffmpeg_command(
        &ffprobe,
        &[
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_streams",
            "-select_streams",
            "s",
            &video,
        ],
        30,
    )?;

    let json_str = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&json_str)
        .map_err(|e| CoreError::Video(format!("Failed to parse ffprobe JSON: {e}")))?;

    let streams = parsed["streams"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    let index = s["index"].as_u64()? as usize;
                    let codec_name = s["codec_name"].as_str()?.to_string();
                    let language = s["tags"]["language"].as_str().map(|l| l.to_string());
                    let title = s["tags"]["title"].as_str().map(|t| t.to_string());
                    Some(SubtitleStream {
                        index,
                        language,
                        codec_name,
                        title,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(streams)
}

/// Extract subtitles from a specific stream as SRT, returning the raw SRT content.
pub fn extract_subtitles(
    ffmpeg_path: &str,
    video_path: &Path,
    stream_index: usize,
    output_path: &Path,
) -> Result<String, CoreError> {
    let canonical_path =
        std::fs::canonicalize(video_path).unwrap_or_else(|_| video_path.to_path_buf());
    let video = canonical_path.to_string_lossy();
    let out = output_path.to_string_lossy();
    let map_arg = format!("0:{stream_index}");

    run_ffmpeg_command(
        ffmpeg_path,
        &["-i", &video, "-map", &map_arg, "-f", "srt", "-y", &out],
        60,
    )?;

    std::fs::read_to_string(output_path)
        .map_err(|e| CoreError::Video(format!("Failed to read extracted subtitles: {e}")))
}

/// Strip HTML tags from a string (handles <i>, <b>, <font>, etc.).
fn strip_html_tags(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    result
}

/// Parse SRT subtitle content into transcript segments.
fn parse_srt(content: &str) -> Vec<TranscriptSegment> {
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);
    let mut segments = Vec::new();
    let normalized = content.replace("\r\n", "\n");

    for block in normalized.split("\n\n") {
        let lines: Vec<&str> = block.trim().lines().collect();
        if lines.len() < 2 {
            continue;
        }

        // Timestamp line is either first (no sequence number) or second
        let ts_idx = if lines[0].contains("-->") { 0 } else { 1 };
        if ts_idx >= lines.len() || !lines[ts_idx].contains("-->") {
            continue;
        }

        let (start_ms, end_ms) = match parse_srt_timestamp_line(lines[ts_idx]) {
            Some(pair) => pair,
            None => continue,
        };

        let text: String = lines[ts_idx + 1..]
            .iter()
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join(" ");

        let text = strip_html_tags(text.trim()).trim().to_string();
        if !text.is_empty() {
            segments.push(TranscriptSegment {
                start_ms,
                end_ms,
                text,
            });
        }
    }

    segments
}

/// Parse an SRT timestamp line like "00:01:23,456 --> 00:02:34,567".
fn parse_srt_timestamp_line(line: &str) -> Option<(i64, i64)> {
    let parts: Vec<&str> = line.split("-->").collect();
    if parts.len() != 2 {
        return None;
    }
    let start = parse_srt_time(parts[0].trim())?;
    let end = parse_srt_time(parts[1].trim())?;
    Some((start, end))
}

/// Parse "HH:MM:SS,mmm" into milliseconds.
fn parse_srt_time(s: &str) -> Option<i64> {
    let s = s.replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let hours: i64 = parts[0].parse().ok()?;
    let minutes: i64 = parts[1].parse().ok()?;
    let sec_parts: Vec<&str> = parts[2].split('.').collect();
    let seconds: i64 = sec_parts[0].parse().ok()?;
    let millis: i64 = if sec_parts.len() > 1 {
        let ms_str = sec_parts[1];
        let padded = format!("{:0<3}", &ms_str[..ms_str.len().min(3)]);
        padded.parse().ok()?
    } else {
        0
    };
    Some(hours * 3_600_000 + minutes * 60_000 + seconds * 1000 + millis)
}

// ═══════════════════════════════════════════
// Whisper Transcription (candle-transformers)
// ═══════════════════════════════════════════

/// Compute mel filterbank coefficients (matches librosa/whisper convention).
#[cfg(feature = "video")]
fn compute_mel_filters(sample_rate: f64, n_fft: usize, n_mels: usize) -> Vec<f32> {
    let f_min = 0.0f64;
    let f_max = sample_rate / 2.0;
    let n_freqs = n_fft / 2 + 1;

    let hz_to_mel = |hz: f64| -> f64 { 2595.0 * (1.0 + hz / 700.0).log10() };
    let mel_to_hz = |mel: f64| -> f64 { 700.0 * (10.0f64.powf(mel / 2595.0) - 1.0) };

    let min_mel = hz_to_mel(f_min);
    let max_mel = hz_to_mel(f_max);

    // n_mels + 2 equally spaced points in mel-frequency domain
    let n_points = n_mels + 2;
    let mel_points: Vec<f64> = (0..n_points)
        .map(|i| min_mel + (max_mel - min_mel) * i as f64 / (n_points - 1) as f64)
        .collect();
    let hz_points: Vec<f64> = mel_points.iter().map(|&m| mel_to_hz(m)).collect();

    // Linear frequency bins
    let fft_freqs: Vec<f64> = (0..n_freqs)
        .map(|i| f_min + (f_max - f_min) * i as f64 / (n_freqs - 1) as f64)
        .collect();

    let mut filters = vec![0.0f32; n_mels * n_freqs];
    for i in 0..n_mels {
        let f_left = hz_points[i];
        let f_center = hz_points[i + 1];
        let f_right = hz_points[i + 2];
        // Slaney-style mel normalization
        let enorm = 2.0 / (f_right - f_left);
        for j in 0..n_freqs {
            let freq = fft_freqs[j];
            let lower = (freq - f_left) / (f_center - f_left);
            let upper = (f_right - freq) / (f_right - f_center);
            let val = f64::max(0.0, f64::min(lower, upper));
            filters[i * n_freqs + j] = (val * enorm) as f32;
        }
    }
    filters
}

/// Look up a special token id by its string representation.
#[cfg(feature = "video")]
fn whisper_token_id(tokenizer: &tokenizers::Tokenizer, token: &str) -> Result<u32, CoreError> {
    tokenizer
        .token_to_id(token)
        .ok_or_else(|| CoreError::Video(format!("Whisper token not found: {token}")))
}

/// Apply whisper timestamp constraint rules during autoregressive decoding.
///
/// Ensures timestamp tokens appear in non-decreasing pairs and that the first
/// generated token is always a timestamp.
#[cfg(feature = "video")]
fn apply_timestamp_rules(
    logits: &candle_core::Tensor,
    tokens: &[u32],
    no_timestamps_token: u32,
    eot_token: u32,
    vocab_size: u32,
    sample_begin: usize,
) -> Result<candle_core::Tensor, CoreError> {
    let device = logits.device().clone();
    let timestamp_begin = no_timestamps_token + 1;

    let sampled_tokens = if tokens.len() > sample_begin {
        &tokens[sample_begin..]
    } else {
        &[]
    };

    let mut masks: Vec<candle_core::Tensor> = Vec::new();
    let mut mask_buf = vec![0.0f32; vocab_size as usize];

    if !sampled_tokens.is_empty() {
        let last_was_ts = sampled_tokens
            .last()
            .map(|&t| t >= timestamp_begin)
            .unwrap_or(false);
        let pen_was_ts = sampled_tokens.len() >= 2
            && sampled_tokens[sampled_tokens.len() - 2] >= timestamp_begin;

        if last_was_ts {
            if pen_was_ts {
                // Two timestamps in a row → force non-timestamp
                for i in 0..vocab_size {
                    mask_buf[i as usize] = if i >= timestamp_begin {
                        f32::NEG_INFINITY
                    } else {
                        0.0
                    };
                }
            } else {
                // Single timestamp → force another timestamp or EOT
                for i in 0..vocab_size {
                    mask_buf[i as usize] = if i < eot_token {
                        f32::NEG_INFINITY
                    } else {
                        0.0
                    };
                }
            }
            masks.push(
                candle_core::Tensor::new(mask_buf.as_slice(), &device)
                    .map_err(|e| CoreError::Video(format!("Mask tensor: {e}")))?,
            );
        }

        // Non-decreasing timestamp constraint
        let ts_tokens: Vec<u32> = sampled_tokens
            .iter()
            .filter(|&&t| t >= timestamp_begin)
            .cloned()
            .collect();
        if !ts_tokens.is_empty() {
            let ts_last = if last_was_ts && !pen_was_ts {
                *ts_tokens.last().unwrap()
            } else {
                ts_tokens.last().unwrap() + 1
            };
            for i in 0..vocab_size {
                mask_buf[i as usize] = if i >= timestamp_begin && i < ts_last {
                    f32::NEG_INFINITY
                } else {
                    0.0
                };
            }
            masks.push(
                candle_core::Tensor::new(mask_buf.as_slice(), &device)
                    .map_err(|e| CoreError::Video(format!("Mask tensor: {e}")))?,
            );
        }
    }

    // Force initial timestamp at the start of decoding
    if tokens.len() == sample_begin {
        for i in 0..vocab_size {
            mask_buf[i as usize] = if i < timestamp_begin {
                f32::NEG_INFINITY
            } else {
                0.0
            };
        }
        masks.push(
            candle_core::Tensor::new(mask_buf.as_slice(), &device)
                .map_err(|e| CoreError::Video(format!("Mask tensor: {e}")))?,
        );
    }

    let mut result = logits.clone();
    for mask in masks {
        result = result
            .broadcast_add(&mask)
            .map_err(|e| CoreError::Video(format!("Apply mask: {e}")))?;
    }

    // Prefer timestamps when their combined probability exceeds any text token
    let log_probs = candle_nn::ops::log_softmax(&result, 0)
        .map_err(|e| CoreError::Video(format!("log_softmax: {e}")))?;
    let ts_log_probs = log_probs
        .narrow(
            0,
            timestamp_begin as usize,
            vocab_size as usize - timestamp_begin as usize,
        )
        .map_err(|e| CoreError::Video(format!("narrow ts: {e}")))?;
    let text_log_probs = log_probs
        .narrow(0, 0, timestamp_begin as usize)
        .map_err(|e| CoreError::Video(format!("narrow text: {e}")))?;

    let ts_logprob = {
        let max_val = ts_log_probs
            .max(0)
            .map_err(|e| CoreError::Video(format!("max ts: {e}")))?;
        let shifted = ts_log_probs
            .broadcast_sub(&max_val)
            .map_err(|e| CoreError::Video(format!("sub: {e}")))?;
        let sum_exp = shifted
            .exp()
            .map_err(|e| CoreError::Video(format!("exp: {e}")))?
            .sum(0)
            .map_err(|e| CoreError::Video(format!("sum: {e}")))?;
        let log_sum = sum_exp
            .log()
            .map_err(|e| CoreError::Video(format!("log: {e}")))?;
        max_val
            .broadcast_add(&log_sum)
            .map_err(|e| CoreError::Video(format!("add: {e}")))?
            .to_scalar::<f32>()
            .map_err(|e| CoreError::Video(format!("scalar: {e}")))?
    };
    let max_text_logprob: f32 = text_log_probs
        .max(0)
        .map_err(|e| CoreError::Video(format!("max text: {e}")))?
        .to_scalar::<f32>()
        .map_err(|e| CoreError::Video(format!("scalar: {e}")))?;

    if ts_logprob > max_text_logprob {
        for i in 0..vocab_size {
            mask_buf[i as usize] = if i < timestamp_begin {
                f32::NEG_INFINITY
            } else {
                0.0
            };
        }
        let mask = candle_core::Tensor::new(mask_buf.as_slice(), &device)
            .map_err(|e| CoreError::Video(format!("Mask tensor: {e}")))?;
        result = result
            .broadcast_add(&mask)
            .map_err(|e| CoreError::Video(format!("Apply mask: {e}")))?;
    }

    Ok(result)
}

#[cfg(feature = "video")]
struct CachedWhisperRuntime {
    model_key: PathBuf,
    device: candle_core::Device,
    tokenizer: tokenizers::Tokenizer,
    model: candle_transformers::models::whisper::model::Whisper,
    num_mel_bins: usize,
    vocab_size: usize,
    max_target_positions: usize,
    suppress_token_ids: Vec<u32>,
}

#[cfg(feature = "video")]
static WHISPER_RUNTIME: std::sync::OnceLock<std::sync::Mutex<Option<CachedWhisperRuntime>>> =
    std::sync::OnceLock::new();

#[cfg(feature = "video")]
fn load_whisper_runtime(config: &VideoConfig) -> Result<CachedWhisperRuntime, CoreError> {
    use candle_transformers::models::whisper::{self as m, Config as WhisperConfig};

    let device = candle_core::Device::Cpu;
    let model_dir = Path::new(&config.model_path).join(config.whisper_model.dir_name());
    let model_key = std::fs::canonicalize(&model_dir).unwrap_or_else(|_| model_dir.clone());
    let config_path = model_dir.join("config.json");
    let whisper_config: WhisperConfig = serde_json::from_str(
        &std::fs::read_to_string(&config_path)
            .map_err(|e| CoreError::Video(format!("Failed to read whisper config: {e}")))?,
    )
    .map_err(|e| CoreError::Video(format!("Failed to parse whisper config: {e}")))?;
    let tokenizer = tokenizers::Tokenizer::from_file(model_dir.join("tokenizer.json"))
        .map_err(|e| CoreError::Video(format!("Failed to load tokenizer: {e}")))?;
    let weight_paths: Vec<PathBuf> = config
        .whisper_model
        .weight_files()
        .iter()
        .map(|file| model_dir.join(file))
        .collect();
    // SAFETY: model files are read-only after download and deletion is blocked
    // while the cached runtime is in use.
    let vb =
        unsafe { candle_nn::VarBuilder::from_mmaped_safetensors(&weight_paths, m::DTYPE, &device) }
            .map_err(|e| CoreError::Video(format!("Failed to load model weights: {e}")))?;
    let num_mel_bins = whisper_config.num_mel_bins;
    let vocab_size = whisper_config.vocab_size;
    let max_target_positions = whisper_config.max_target_positions;
    let suppress_token_ids = whisper_config.suppress_tokens.clone();
    let model = m::model::Whisper::load(&vb, whisper_config)
        .map_err(|e| CoreError::Video(format!("Failed to build whisper model: {e}")))?;

    Ok(CachedWhisperRuntime {
        model_key,
        device,
        tokenizer,
        model,
        num_mel_bins,
        vocab_size,
        max_target_positions,
        suppress_token_ids,
    })
}

#[cfg(feature = "video")]
fn clear_whisper_runtime_cache() {
    if let Some(cache) = WHISPER_RUNTIME.get() {
        if let Ok(mut runtime) = cache.lock() {
            runtime.take();
        }
    }
}

/// Transcribe a WAV file using a process-wide cached candle-transformers
/// Whisper runtime. Access is serialized because its decoder owns a mutable
/// KV cache; the first decoder step flushes that cache for each chunk.
#[cfg(feature = "video")]
pub fn transcribe_audio(
    wav_path: &Path,
    config: &VideoConfig,
) -> Result<Vec<TranscriptSegment>, CoreError> {
    use candle_core::{IndexOp, Tensor};
    use candle_transformers::models::whisper::{self as m, audio};

    if config.use_gpu {
        tracing::warn!("GPU transcription was requested but this build uses the CPU backend");
    }
    if config.beam_size > 1 {
        tracing::warn!("Beam search was requested but this build uses greedy decoding");
    }

    let model_dir = Path::new(&config.model_path).join(config.whisper_model.dir_name());
    let model_key = std::fs::canonicalize(&model_dir).unwrap_or(model_dir);
    let cache = WHISPER_RUNTIME.get_or_init(|| std::sync::Mutex::new(None));
    let mut runtime_guard = cache
        .lock()
        .map_err(|_| CoreError::Video("Whisper runtime cache is poisoned".into()))?;
    if runtime_guard
        .as_ref()
        .is_none_or(|runtime| runtime.model_key != model_key)
    {
        *runtime_guard = Some(load_whisper_runtime(config)?);
    }
    let runtime = runtime_guard
        .as_mut()
        .ok_or_else(|| CoreError::Video("Whisper runtime failed to initialize".into()))?;
    let CachedWhisperRuntime {
        device,
        tokenizer,
        model,
        num_mel_bins,
        vocab_size,
        max_target_positions,
        suppress_token_ids,
        ..
    } = runtime;
    let num_mel_bins = *num_mel_bins;
    let vocab_size = *vocab_size;
    let max_target_positions = *max_target_positions;

    // ── special tokens ────────────────────────────────────────────────
    let sot_token = whisper_token_id(tokenizer, m::SOT_TOKEN)?;
    let eot_token = whisper_token_id(tokenizer, m::EOT_TOKEN)?;
    let transcribe_token = whisper_token_id(tokenizer, m::TRANSCRIBE_TOKEN)?;
    let translate_token = whisper_token_id(tokenizer, m::TRANSLATE_TOKEN)?;
    let no_timestamps_token = whisper_token_id(tokenizer, m::NO_TIMESTAMPS_TOKEN)?;
    let no_speech_token = m::NO_SPEECH_TOKENS
        .iter()
        .find_map(|t| whisper_token_id(tokenizer, t).ok())
        .ok_or_else(|| CoreError::Video("No-speech token not found in tokenizer".into()))?;

    let mut language_token = config
        .language
        .as_deref()
        .map(str::trim)
        .filter(|language| !language.is_empty() && !language.eq_ignore_ascii_case("auto"))
        .map(|language| {
            whisper_token_id(tokenizer, &format!("<|{language}|>"))
                .map_err(|_| CoreError::Video(format!("Unsupported language: {language}")))
        })
        .transpose()?;

    let task_token = if config.translate_to_english {
        translate_token
    } else {
        transcribe_token
    };

    // Suppress tokens mask (also suppress <|notimestamps|> since we want timestamps)
    let suppress_mask: Vec<f32> = (0..vocab_size as u32)
        .map(|i| {
            if suppress_token_ids.contains(&i) || i == no_timestamps_token {
                f32::NEG_INFINITY
            } else {
                0f32
            }
        })
        .collect();
    let suppress_mask = Tensor::new(suppress_mask.as_slice(), device)
        .map_err(|e| CoreError::Video(format!("Suppress mask tensor: {e}")))?;

    // ── audio → mel spectrogram ───────────────────────────────────────
    let audio_data = read_wav_pcm(wav_path)?;
    let mel_filters = compute_mel_filters(m::SAMPLE_RATE as f64, m::N_FFT, num_mel_bins);
    let mel = audio::pcm_to_mel(&model.config, &audio_data, &mel_filters);
    let mel_len = mel.len();
    let mel = Tensor::from_vec(mel, (1, num_mel_bins, mel_len / num_mel_bins), device)
        .map_err(|e| CoreError::Video(format!("Mel tensor: {e}")))?;

    // ── decode in 30-second chunks ────────────────────────────────────
    let (_, _, content_frames) = mel
        .dims3()
        .map_err(|e| CoreError::Video(format!("Mel dims: {e}")))?;

    let sample_len = max_target_positions / 2;
    let timestamp_begin = no_timestamps_token + 1;
    let mut seek = 0;
    let mut segments = Vec::new();

    while seek < content_frames {
        let time_offset = (seek * m::HOP_LENGTH) as f64 / m::SAMPLE_RATE as f64;
        let segment_size = usize::min(content_frames - seek, m::N_FRAMES);
        let segment_duration = (segment_size * m::HOP_LENGTH) as f64 / m::SAMPLE_RATE as f64;
        let mel_segment = mel
            .narrow(2, seek, segment_size)
            .map_err(|e| CoreError::Video(format!("Mel narrow: {e}")))?;

        // Encode
        let audio_features = model
            .encoder
            .forward(&mel_segment, true)
            .map_err(|e| CoreError::Video(format!("Encoder forward: {e}")))?;

        // Whisper multilingual models require a language token in the decode
        // prompt. When the user selects Auto, identify it from the first
        // encoded chunk using the same language-token scoring procedure as
        // Candle's upstream Whisper example, then reuse it for later chunks.
        if language_token.is_none() {
            const LANGUAGE_CODES: &str = "en zh de es ru ko fr ja pt tr pl ca nl ar sv it id hi fi vi he uk el ms cs ro da hu ta no th ur hr bg lt la mi ml cy sk te fa lv bn sr az sl kn et mk br eu is hy ne mn bs kk sq sw gl mr pa si km sn yo so af oc ka be tg sd gu am yi lo uz fo ht ps tk nn mt sa lb my bo tl mg as tt haw ln ha ba jw su";
            let tokens = Tensor::new(&[[sot_token]], device)
                .map_err(|e| CoreError::Video(format!("Language token input: {e}")))?;
            let ys = model
                .decoder
                .forward(&tokens, &audio_features, true)
                .map_err(|e| CoreError::Video(format!("Language detection decoder: {e}")))?;
            let logits = model
                .decoder
                .final_linear(&ys.i(..1).map_err(|e| CoreError::Video(format!("i: {e}")))?)
                .map_err(|e| CoreError::Video(format!("Language detection logits: {e}")))?
                .i(0)
                .and_then(|tensor| tensor.i(0))
                .map_err(|e| CoreError::Video(format!("Language detection slice: {e}")))?
                .to_vec1::<f32>()
                .map_err(|e| CoreError::Video(format!("Language detection values: {e}")))?;
            language_token = LANGUAGE_CODES
                .split_ascii_whitespace()
                .filter_map(|code| {
                    whisper_token_id(tokenizer, &format!("<|{code}|>"))
                        .ok()
                        .and_then(|token| {
                            logits
                                .get(token as usize)
                                .copied()
                                .map(|score| (token, score))
                        })
                })
                .max_by(|(_, left), (_, right)| left.total_cmp(right))
                .map(|(token, _)| token);
            if language_token.is_none() {
                return Err(CoreError::Video(
                    "Whisper tokenizer contains no supported language tokens".into(),
                ));
            }
        }

        // SOT, detected/configured language, then task token.
        let sample_begin = 3;

        // Build initial token sequence: SOT [lang] task
        let mut tokens = vec![sot_token];
        if let Some(lt) = language_token {
            tokens.push(lt);
        }
        tokens.push(task_token);

        // Autoregressive decoding with greedy search
        for i in 0..sample_len {
            let tokens_t = Tensor::new(tokens.as_slice(), device)
                .map_err(|e| CoreError::Video(format!("Token tensor: {e}")))?
                .unsqueeze(0)
                .map_err(|e| CoreError::Video(format!("Unsqueeze: {e}")))?;

            let ys = model
                .decoder
                .forward(&tokens_t, &audio_features, i == 0)
                .map_err(|e| CoreError::Video(format!("Decoder forward: {e}")))?;

            // Check no-speech probability on first iteration
            if i == 0 {
                let first_logits = model
                    .decoder
                    .final_linear(&ys.i(..1).map_err(|e| CoreError::Video(format!("i: {e}")))?)
                    .map_err(|e| CoreError::Video(format!("final_linear: {e}")))?
                    .i(0)
                    .map_err(|e| CoreError::Video(format!("i: {e}")))?
                    .i(0)
                    .map_err(|e| CoreError::Video(format!("i: {e}")))?;
                let probs = candle_nn::ops::softmax(&first_logits, 0)
                    .map_err(|e| CoreError::Video(format!("softmax: {e}")))?;
                let no_speech_prob = probs
                    .i(no_speech_token as usize)
                    .map_err(|e| CoreError::Video(format!("i: {e}")))?
                    .to_scalar::<f32>()
                    .map_err(|e| CoreError::Video(format!("scalar: {e}")))?;
                if no_speech_prob > m::NO_SPEECH_THRESHOLD as f32 {
                    break; // skip silent segment
                }
            }

            let (_, seq_len, _) = ys
                .dims3()
                .map_err(|e| CoreError::Video(format!("Decoder dims: {e}")))?;
            let logits = model
                .decoder
                .final_linear(
                    &ys.i((..1, seq_len - 1..))
                        .map_err(|e| CoreError::Video(format!("Slice: {e}")))?,
                )
                .map_err(|e| CoreError::Video(format!("Final linear: {e}")))?
                .i(0)
                .map_err(|e| CoreError::Video(format!("i: {e}")))?
                .i(0)
                .map_err(|e| CoreError::Video(format!("i: {e}")))?;

            // Apply timestamp rules + suppress tokens
            let logits = apply_timestamp_rules(
                &logits,
                &tokens,
                no_timestamps_token,
                eot_token,
                vocab_size as u32,
                sample_begin,
            )?;
            let logits = logits
                .broadcast_add(&suppress_mask)
                .map_err(|e| CoreError::Video(format!("Suppress: {e}")))?;

            // Greedy argmax
            let logits_v: Vec<f32> = logits
                .to_vec1()
                .map_err(|e| CoreError::Video(format!("to_vec1: {e}")))?;
            let next_token = logits_v
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.total_cmp(b))
                .map(|(idx, _)| idx as u32)
                .unwrap();

            tokens.push(next_token);
            if next_token == eot_token || tokens.len() > max_target_positions {
                break;
            }
        }

        // ── extract timestamped segments from token stream ────────────
        let mut text_tokens: Vec<u32> = Vec::new();
        let mut prev_ts_s = 0.0f64;

        for &token in &tokens {
            if token == sot_token || token == eot_token {
                continue;
            }
            // Skip language / task preamble tokens
            if Some(token) == language_token || token == task_token {
                continue;
            }
            if token >= timestamp_begin {
                let ts_s = (token - timestamp_begin) as f64 * 0.02;
                if !text_tokens.is_empty() {
                    let text = tokenizer
                        .decode(&text_tokens, true)
                        .map_err(|e| CoreError::Video(format!("Decode tokens: {e}")))?;
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        segments.push(TranscriptSegment {
                            start_ms: ((time_offset + prev_ts_s) * 1000.0) as i64,
                            end_ms: ((time_offset + ts_s) * 1000.0) as i64,
                            text,
                        });
                    }
                    text_tokens.clear();
                }
                prev_ts_s = ts_s;
            } else {
                text_tokens.push(token);
            }
        }
        // Trailing text without a closing timestamp
        if !text_tokens.is_empty() {
            let text = tokenizer
                .decode(&text_tokens, true)
                .map_err(|e| CoreError::Video(format!("Decode tokens: {e}")))?;
            let text = text.trim().to_string();
            if !text.is_empty() {
                segments.push(TranscriptSegment {
                    start_ms: ((time_offset + prev_ts_s) * 1000.0) as i64,
                    end_ms: ((time_offset + segment_duration) * 1000.0) as i64,
                    text,
                });
            }
        }

        seek += segment_size;
    }

    Ok(segments)
}

#[cfg(feature = "video")]
fn wav_duration_ms(wav_path: &Path) -> Result<i64, CoreError> {
    let reader = hound::WavReader::open(wav_path)
        .map_err(|error| CoreError::Video(format!("Failed to open WAV: {error}")))?;
    let spec = reader.spec();
    let frames = reader.duration() as f64 / spec.channels.max(1) as f64;
    Ok((frames / spec.sample_rate.max(1) as f64 * 1000.0).round() as i64)
}

#[cfg(feature = "video")]
fn transcribe_cloud_wav_chunks(
    wav_path: &Path,
    speech: &SpeechToTextConfig,
) -> Result<Vec<TranscriptSegment>, CoreError> {
    const CHUNK_SECONDS: usize = 8 * 60;
    let mut reader = hound::WavReader::open(wav_path)
        .map_err(|error| CoreError::Video(format!("Failed to open WAV: {error}")))?;
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 {
        return Err(CoreError::Video(
            "Cloud media transcription requires normalized 16-bit PCM WAV audio".into(),
        ));
    }
    let samples_per_chunk =
        spec.sample_rate as usize * spec.channels.max(1) as usize * CHUNK_SECONDS;
    let temp_dir = tempfile::tempdir()
        .map_err(|error| CoreError::Video(format!("Failed to create ASR temp dir: {error}")))?;
    let mut samples = reader.samples::<i16>();
    let mut processed_samples = 0usize;
    let mut chunk_index = 0usize;
    let mut segments = Vec::new();

    loop {
        let chunk_path = temp_dir.path().join(format!("chunk-{chunk_index:04}.wav"));
        let mut writer = hound::WavWriter::create(&chunk_path, spec)
            .map_err(|error| CoreError::Video(format!("Failed to create ASR chunk: {error}")))?;
        let mut written = 0usize;
        for _ in 0..samples_per_chunk {
            let Some(sample) = samples.next() else { break };
            writer
                .write_sample(
                    sample
                        .map_err(|error| CoreError::Video(format!("Read WAV sample: {error}")))?,
                )
                .map_err(|error| CoreError::Video(format!("Write ASR chunk: {error}")))?;
            written += 1;
        }
        writer
            .finalize()
            .map_err(|error| CoreError::Video(format!("Finalize ASR chunk: {error}")))?;
        if written == 0 {
            let _ = std::fs::remove_file(&chunk_path);
            break;
        }

        let start_ms = ((processed_samples as f64
            / spec.channels.max(1) as f64
            / spec.sample_rate.max(1) as f64)
            * 1000.0)
            .round() as i64;
        processed_samples += written;
        let end_ms = ((processed_samples as f64
            / spec.channels.max(1) as f64
            / spec.sample_rate.max(1) as f64)
            * 1000.0)
            .round() as i64;
        let text = crate::speech_to_text::transcribe_cloud_wav_blocking(
            std::fs::read(&chunk_path)?,
            speech,
        )?;
        if !text.trim().is_empty() {
            segments.push(TranscriptSegment {
                start_ms,
                end_ms,
                text,
            });
        }
        chunk_index += 1;
    }
    Ok(segments)
}

#[cfg(feature = "video")]
fn transcribe_media_wav(
    wav_path: &Path,
    video: &VideoConfig,
    speech: &SpeechToTextConfig,
) -> Result<Vec<TranscriptSegment>, CoreError> {
    match video.transcription_mode {
        MediaTranscriptionMode::Disabled => Ok(Vec::new()),
        MediaTranscriptionMode::LocalWhisper => transcribe_audio(wav_path, video),
        MediaTranscriptionMode::InheritSpeechToText => match speech.api_style.as_str() {
            "local_whisper" => {
                let mut inherited = video.clone();
                inherited.language = speech.language.clone().or_else(|| video.language.clone());
                transcribe_audio(wav_path, &inherited)
            }
            "openai_transcription" | "dashscope_asr" => {
                transcribe_cloud_wav_chunks(wav_path, speech)
            }
            "sherpa_onnx" => {
                let text = crate::speech_to_text::transcribe_sherpa_wav(wav_path, speech)?;
                Ok(vec![TranscriptSegment {
                    start_ms: 0,
                    end_ms: wav_duration_ms(wav_path)?,
                    text,
                }])
            }
            style => Err(CoreError::InvalidInput(format!(
                "Speech-to-text backend {style} cannot transcribe media files"
            ))),
        },
    }
}

fn media_warning(code: &str, error: &CoreError) -> MediaAnalysisWarning {
    MediaAnalysisWarning {
        code: code.to_string(),
        message: error.to_string(),
    }
}

fn degraded_analysis_result(
    duration_secs: Option<f64>,
    code: &str,
    error: &CoreError,
) -> VideoAnalysisResult {
    VideoAnalysisResult {
        transcript_segments: Vec::new(),
        full_transcript: String::new(),
        duration_secs,
        visual_events: Vec::new(),
        warnings: vec![media_warning(code, error)],
        thumbnail_path: None,
        metadata: None,
    }
}

/// Read WAV file into f32 samples normalized to [-1.0, 1.0], converting stereo to mono.
fn read_wav_pcm(wav_path: &Path) -> Result<Vec<f32>, CoreError> {
    let reader = hound::WavReader::open(wav_path)
        .map_err(|e| CoreError::Video(format!("Failed to read WAV file: {e}")))?;

    let spec = reader.spec();
    if spec.channels == 0 {
        return Err(CoreError::Video("Invalid WAV: 0 channels".into()));
    }
    if spec.bits_per_sample == 0 || spec.bits_per_sample > 32 {
        return Err(CoreError::Video(format!(
            "Invalid WAV: {} bits per sample",
            spec.bits_per_sample
        )));
    }
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .map(|s| s.map_err(|e| CoreError::Video(format!("WAV sample error: {e}"))))
            .collect::<Result<Vec<f32>, CoreError>>()?,
        hound::SampleFormat::Int => {
            let max_val = (1_i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .into_samples::<i32>()
                .map(|s| {
                    s.map(|v| v as f32 / max_val)
                        .map_err(|e| CoreError::Video(format!("WAV sample error: {e}")))
                })
                .collect::<Result<Vec<f32>, CoreError>>()?
        }
    };

    // Convert stereo to mono if needed
    if spec.channels == 2 {
        Ok(samples
            .chunks(2)
            .map(|c| (c[0] + c.get(1).copied().unwrap_or(0.0)) / 2.0)
            .collect())
    } else {
        Ok(samples)
    }
}

// ═══════════════════════════════════════════
// Model Management
// ═══════════════════════════════════════════

/// Check if all required Whisper model files exist on disk.
pub fn check_whisper_model_exists(config: &VideoConfig) -> bool {
    let model_dir = Path::new(&config.model_path).join(config.whisper_model.dir_name());
    config
        .whisper_model
        .required_files()
        .iter()
        .all(|f| model_dir.join(f).exists())
}

pub fn media_runtime_status(
    video: &VideoConfig,
    ocr: &crate::ocr::OcrConfig,
    speech: &SpeechToTextConfig,
) -> MediaRuntimeStatus {
    if !video.enabled {
        let inactive = CapabilityRuntimeStatus {
            configured: false,
            ready: false,
            degraded: false,
            reason: None,
        };
        return MediaRuntimeStatus {
            enabled: false,
            transcription_mode: video.transcription_mode,
            failure_policy: video.failure_policy,
            probe: inactive.clone(),
            transcription: inactive.clone(),
            visual_analysis: inactive,
        };
    }

    let ffmpeg_ready = check_ffmpeg(video).unwrap_or(false);
    let probe = CapabilityRuntimeStatus {
        configured: true,
        ready: ffmpeg_ready,
        degraded: !ffmpeg_ready,
        reason: (!ffmpeg_ready).then(|| "ffmpeg_unavailable".to_string()),
    };

    let transcription = match video.transcription_mode {
        MediaTranscriptionMode::Disabled => CapabilityRuntimeStatus {
            configured: true,
            ready: true,
            degraded: false,
            reason: None,
        },
        MediaTranscriptionMode::LocalWhisper => {
            let ready = check_whisper_model_exists(video);
            CapabilityRuntimeStatus {
                configured: true,
                ready,
                degraded: !ready,
                reason: (!ready).then(|| "whisper_model_unavailable".to_string()),
            }
        }
        MediaTranscriptionMode::InheritSpeechToText => {
            let configured = speech.is_configured();
            let ready = match speech.api_style.as_str() {
                "local_whisper" => configured && check_whisper_model_exists(video),
                "openai_transcription" | "dashscope_asr" | "sherpa_onnx" => configured,
                _ => false,
            };
            CapabilityRuntimeStatus {
                configured,
                ready,
                degraded: !ready,
                reason: (!configured)
                    .then(|| "speech_to_text_not_configured".to_string())
                    .or_else(|| (!ready).then(|| "speech_to_text_backend_unavailable".to_string())),
            }
        }
    };

    let visual_ready =
        !video.frame_extraction_enabled || (ocr.enabled && crate::ocr::check_ocr_models_exist(ocr));
    let visual_analysis = CapabilityRuntimeStatus {
        configured: !video.frame_extraction_enabled || ocr.enabled,
        ready: visual_ready,
        degraded: !visual_ready,
        reason: (!visual_ready).then(|| "ocr_models_unavailable".to_string()),
    };

    MediaRuntimeStatus {
        enabled: true,
        transcription_mode: video.transcription_mode,
        failure_policy: video.failure_policy,
        probe,
        transcription,
        visual_analysis,
    }
}

/// Download the selected Whisper model (safetensors format) with progress reporting.
///
/// `hf_mirror_base` — fallback mirror base URL (e.g. `https://hf-mirror.com`).
/// Empty string disables the fallback. The `HF_ENDPOINT` env var still wins
/// over settings for the primary URL.
pub fn download_whisper_model(
    config: &VideoConfig,
    hf_mirror_base: &str,
    on_progress: impl Fn(VideoDownloadProgress),
) -> Result<(), CoreError> {
    let model_dir = Path::new(&config.model_path).join(config.whisper_model.dir_name());
    std::fs::create_dir_all(&model_dir)
        .map_err(|e| CoreError::Video(format!("Failed to create model directory: {e}")))?;

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .connect_timeout(std::time::Duration::from_secs(15))
        .user_agent(concat!("nexa/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| CoreError::Video(format!("HTTP client error: {e}")))?;

    // HF_ENDPOINT env override: swap `https://huggingface.co` on primary URLs.
    // Env var > user-configured mirror base for the primary; mirror base remains
    // the secondary fallback (empty string disables it).
    let hf_endpoint = std::env::var("HF_ENDPOINT").ok().and_then(|v| {
        let trimmed = v.trim().trim_end_matches('/').to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    });

    let primary_urls = config.whisper_model.download_urls();
    let mirror_urls = config.whisper_model.fallback_download_urls(hf_mirror_base);

    for (idx, (filename, base_url)) in primary_urls.iter().enumerate() {
        let dest = model_dir.join(filename);
        if dest.exists() {
            tracing::info!("Model file already exists, skipping: {filename}");
            continue;
        }
        let tmp_dest = dest.with_extension("partial");

        let primary_url = match &hf_endpoint {
            Some(endpoint) => base_url.replace("https://huggingface.co", endpoint),
            None => base_url.clone(),
        };

        let mirror_url = mirror_urls.get(idx).map(|(_, u)| u.as_str());

        let filename_owned = filename.to_string();
        download_with_fallback(
            &client,
            &primary_url,
            mirror_url,
            &tmp_dest,
            |bytes_downloaded, total_bytes| {
                on_progress(VideoDownloadProgress {
                    filename: filename_owned.clone(),
                    bytes_downloaded,
                    total_bytes,
                });
            },
        )
        .map_err(|e| CoreError::Video(format!("Failed to download {filename}: {e}")))?;

        // Atomic rename: partial → final
        std::fs::rename(&tmp_dest, &dest).map_err(|e| {
            let _ = std::fs::remove_file(&tmp_dest);
            CoreError::Video(format!("Failed to finalize {filename}: {e}"))
        })?;

        tracing::info!("Downloaded {filename}");
    }

    Ok(())
}

/// Stream a single URL to `tmp_dest`. On HTTP error or network error, returns
/// a descriptive `CoreError::Video` and leaves any partial file in place (the
/// caller is responsible for cleanup before retrying).
fn stream_to_file(
    client: &reqwest::blocking::Client,
    url: &str,
    tmp_dest: &Path,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<u64, CoreError> {
    let resp = client
        .get(url)
        .send()
        .map_err(|e| CoreError::Video(format!("request {url}: {e}")))?;

    if !resp.status().is_success() {
        return Err(CoreError::Video(format!(
            "HTTP {} downloading {url}",
            resp.status()
        )));
    }

    let total_bytes = resp.content_length();
    let mut bytes_downloaded: u64 = 0;

    let mut file = std::fs::File::create(tmp_dest)
        .map_err(|e| CoreError::Video(format!("create {}: {e}", tmp_dest.display())))?;
    let mut reader = std::io::BufReader::new(resp);
    let mut buffer = [0u8; 65536];
    loop {
        use std::io::Read;
        let n = reader
            .read(&mut buffer)
            .map_err(|e| CoreError::Video(format!("read error: {e}")))?;
        if n == 0 {
            break;
        }
        use std::io::Write;
        file.write_all(&buffer[..n])
            .map_err(|e| CoreError::Video(format!("write error: {e}")))?;
        bytes_downloaded += n as u64;
        progress(bytes_downloaded, total_bytes);
    }
    Ok(bytes_downloaded)
}

/// Download `primary_url` to `tmp_dest`; on failure, fall back to `mirror_url`
/// (if provided). Partial files from a failed primary attempt are removed
/// before retrying the mirror.
fn download_with_fallback(
    client: &reqwest::blocking::Client,
    primary_url: &str,
    mirror_url: Option<&str>,
    tmp_dest: &Path,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<u64, CoreError> {
    match stream_to_file(client, primary_url, tmp_dest, &mut progress) {
        Ok(n) => Ok(n),
        Err(primary_err) => {
            let _ = std::fs::remove_file(tmp_dest);
            match mirror_url {
                Some(mirror) => {
                    tracing::warn!(
                        "Primary download failed ({primary_err}); retrying via mirror {mirror}"
                    );
                    stream_to_file(client, mirror, tmp_dest, &mut progress).map_err(|mirror_err| {
                        CoreError::Video(format!("primary: {primary_err}; mirror: {mirror_err}"))
                    })
                }
                None => Err(primary_err),
            }
        }
    }
}

/// Delete all model files for the configured whisper model.
pub fn delete_whisper_model(config: &VideoConfig) -> Result<(), CoreError> {
    clear_whisper_runtime_cache();
    let model_dir = Path::new(&config.model_path).join(config.whisper_model.dir_name());
    if model_dir.exists() {
        std::fs::remove_dir_all(&model_dir)
            .map_err(|e| CoreError::Video(format!("Failed to delete model directory: {e}")))?;
    }
    Ok(())
}

// ═══════════════════════════════════════════
// FFmpeg Download
// ═══════════════════════════════════════════

/// Progress info for FFmpeg download.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FfmpegDownloadProgress {
    pub progress_pct: f32,
    pub status: String,
}

/// Download FFmpeg + ffprobe static binaries for the current platform.
/// Returns the path to the downloaded ffmpeg binary.
///
/// `ghproxy_base` — GitHub reverse-proxy base URL used as fallback mirror.
/// Empty string disables the fallback.
pub fn download_ffmpeg(
    data_dir: &Path,
    ghproxy_base: &str,
    on_progress: impl Fn(FfmpegDownloadProgress),
) -> Result<PathBuf, CoreError> {
    let bin_dir = data_dir.join("bin");
    std::fs::create_dir_all(&bin_dir)
        .map_err(|e| CoreError::Video(format!("Failed to create bin directory: {e}")))?;

    let (ffmpeg_name, ffprobe_name) = if cfg!(windows) {
        ("ffmpeg.exe", "ffprobe.exe")
    } else {
        ("ffmpeg", "ffprobe")
    };

    // Already downloaded?
    let ffmpeg_dest = bin_dir.join(ffmpeg_name);
    let ffprobe_dest = bin_dir.join(ffprobe_name);
    if ffmpeg_dest.exists() && ffprobe_dest.exists() {
        tracing::info!("FFmpeg already exists at {}", ffmpeg_dest.display());
        return Ok(ffmpeg_dest);
    }

    let (url, archive_ext) = ffmpeg_download_url()?;
    let mirror_url = ffmpeg_mirror_url(ghproxy_base)?;

    on_progress(FfmpegDownloadProgress {
        progress_pct: 0.0,
        status: "Downloading FFmpeg...".into(),
    });

    // Stream download
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| CoreError::Video(format!("HTTP client error: {e}")))?;

    let tmp_archive = bin_dir.join(format!("ffmpeg-download.{archive_ext}"));

    download_with_fallback(
        &client,
        &url,
        mirror_url.as_deref(),
        &tmp_archive,
        |bytes_downloaded, total_bytes| {
            let pct = total_bytes
                .map(|t| (bytes_downloaded as f32 / t as f32) * 80.0) // 0-80% for download
                .unwrap_or(0.0);
            on_progress(FfmpegDownloadProgress {
                progress_pct: pct,
                status: format!(
                    "Downloading... {:.1} MB",
                    bytes_downloaded as f64 / 1_048_576.0
                ),
            });
        },
    )
    .map_err(|e| CoreError::Video(format!("Failed to download FFmpeg: {e}")))?;

    on_progress(FfmpegDownloadProgress {
        progress_pct: 80.0,
        status: "Extracting FFmpeg...".into(),
    });

    // Extract binaries
    if archive_ext == "zip" {
        extract_ffmpeg_from_zip(&tmp_archive, &bin_dir, ffmpeg_name, ffprobe_name)?;
    } else {
        extract_ffmpeg_from_tar_xz(&tmp_archive, &bin_dir, ffmpeg_name, ffprobe_name)?;
    }

    // Cleanup archive
    let _ = std::fs::remove_file(&tmp_archive);

    // Set executable permission on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        let _ = std::fs::set_permissions(&ffmpeg_dest, perms.clone());
        let _ = std::fs::set_permissions(&ffprobe_dest, perms);
    }

    on_progress(FfmpegDownloadProgress {
        progress_pct: 100.0,
        status: "FFmpeg ready".into(),
    });

    tracing::info!("FFmpeg downloaded to {}", ffmpeg_dest.display());
    Ok(ffmpeg_dest)
}

/// Returns (download_url, archive_extension) for the current platform.
fn ffmpeg_download_url() -> Result<(String, &'static str), CoreError> {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    let base = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest";

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        Ok((format!("{base}/ffmpeg-master-latest-win64-lgpl.zip"), "zip"))
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        Ok((
            format!("{base}/ffmpeg-master-latest-linux64-lgpl.tar.xz"),
            "tar.xz",
        ))
    }

    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        Ok((
            format!("{base}/ffmpeg-master-latest-linuxarm64-lgpl.tar.xz"),
            "tar.xz",
        ))
    }

    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        // evermeet.cx provides macOS static builds
        Ok((
            "https://evermeet.cx/ffmpeg/getrelease/zip".to_string(),
            "zip",
        ))
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        Ok((
            "https://evermeet.cx/ffmpeg/getrelease/zip".to_string(),
            "zip",
        ))
    }

    #[cfg(not(any(
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
    )))]
    {
        Err(CoreError::Video(
            "FFmpeg auto-download is not supported on this platform".into(),
        ))
    }
}

/// Returns the mirror URL for regions where `github.com` is unreachable.
///
/// For Windows/Linux (GitHub-hosted builds) this wraps the primary URL with
/// the configured GitHub reverse proxy (`ghproxy_base`). For macOS builds
/// (served from `evermeet.cx`) no mirror is configured and this returns
/// `Ok(None)`. An empty `ghproxy_base` also returns `Ok(None)`.
#[allow(unused_variables)]
fn ffmpeg_mirror_url(ghproxy_base: &str) -> Result<Option<String>, CoreError> {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        let base = ghproxy_base.trim().trim_end_matches('/');
        if base.is_empty() {
            return Ok(None);
        }
        let (primary, _) = ffmpeg_download_url()?;
        Ok(Some(format!("{base}/{primary}")))
    }

    #[cfg(target_os = "macos")]
    {
        // evermeet.cx — no generic mirror available.
        Ok(None)
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        Ok(None)
    }
}

/// Extract ffmpeg and ffprobe from a .zip archive (Windows / macOS).
fn extract_ffmpeg_from_zip(
    archive_path: &Path,
    dest_dir: &Path,
    ffmpeg_name: &str,
    ffprobe_name: &str,
) -> Result<(), CoreError> {
    let file = std::fs::File::open(archive_path)
        .map_err(|e| CoreError::Video(format!("Failed to open zip archive: {e}")))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| CoreError::Video(format!("Failed to read zip archive: {e}")))?;

    let mut found_ffmpeg = false;
    let mut found_ffprobe = false;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| CoreError::Video(format!("Failed to read zip entry: {e}")))?;
        let entry_name = entry.name().replace('\\', "/");
        let file_name = entry_name.rsplit('/').next().unwrap_or("");

        if file_name.eq_ignore_ascii_case(ffmpeg_name) {
            extract_zip_entry_to(&mut entry, &dest_dir.join(ffmpeg_name))?;
            found_ffmpeg = true;
        } else if file_name.eq_ignore_ascii_case(ffprobe_name) {
            extract_zip_entry_to(&mut entry, &dest_dir.join(ffprobe_name))?;
            found_ffprobe = true;
        }

        if found_ffmpeg && found_ffprobe {
            break;
        }
    }

    if !found_ffmpeg {
        return Err(CoreError::Video(
            "ffmpeg binary not found in zip archive".into(),
        ));
    }
    // ffprobe may not be in macOS evermeet builds — not fatal
    if !found_ffprobe {
        tracing::warn!("ffprobe not found in zip archive; only ffmpeg was extracted");
    }

    Ok(())
}

/// Extract a single zip entry to a destination file.
fn extract_zip_entry_to(entry: &mut zip::read::ZipFile<'_>, dest: &Path) -> Result<(), CoreError> {
    let mut out = std::fs::File::create(dest)
        .map_err(|e| CoreError::Video(format!("create {}: {e}", dest.display())))?;
    std::io::copy(entry, &mut out)
        .map_err(|e| CoreError::Video(format!("extract to {}: {e}", dest.display())))?;
    Ok(())
}

/// Extract ffmpeg and ffprobe from a .tar.xz archive (Linux).
fn extract_ffmpeg_from_tar_xz(
    archive_path: &Path,
    dest_dir: &Path,
    ffmpeg_name: &str,
    ffprobe_name: &str,
) -> Result<(), CoreError> {
    let file = std::fs::File::open(archive_path)
        .map_err(|e| CoreError::Video(format!("Failed to open tar.xz archive: {e}")))?;
    let decompressor = xz2::read::XzDecoder::new(file);
    let mut archive = tar::Archive::new(decompressor);

    let mut found_ffmpeg = false;
    let mut found_ffprobe = false;

    for entry_result in archive
        .entries()
        .map_err(|e| CoreError::Video(format!("Failed to read tar entries: {e}")))?
    {
        let mut entry =
            entry_result.map_err(|e| CoreError::Video(format!("Failed to read tar entry: {e}")))?;
        let path = entry
            .path()
            .map_err(|e| CoreError::Video(format!("Invalid tar entry path: {e}")))?;
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if file_name == ffmpeg_name {
            let dest = dest_dir.join(ffmpeg_name);
            let mut out = std::fs::File::create(&dest)
                .map_err(|e| CoreError::Video(format!("create {}: {e}", dest.display())))?;
            std::io::copy(&mut entry, &mut out)
                .map_err(|e| CoreError::Video(format!("extract {}: {e}", dest.display())))?;
            found_ffmpeg = true;
        } else if file_name == ffprobe_name {
            let dest = dest_dir.join(ffprobe_name);
            let mut out = std::fs::File::create(&dest)
                .map_err(|e| CoreError::Video(format!("create {}: {e}", dest.display())))?;
            std::io::copy(&mut entry, &mut out)
                .map_err(|e| CoreError::Video(format!("extract {}: {e}", dest.display())))?;
            found_ffprobe = true;
        }

        if found_ffmpeg && found_ffprobe {
            break;
        }
    }

    if !found_ffmpeg {
        return Err(CoreError::Video(
            "ffmpeg binary not found in tar.xz archive".into(),
        ));
    }
    if !found_ffprobe {
        tracing::warn!("ffprobe not found in tar.xz archive; only ffmpeg was extracted");
    }

    Ok(())
}

// ═══════════════════════════════════════════
// Thumbnail Generation
// ═══════════════════════════════════════════

/// Generate a thumbnail from a video file at a specific timestamp.
pub fn generate_thumbnail(
    ffmpeg_path: &str,
    video_path: &Path,
    output_path: &Path,
    timestamp_secs: f64,
    width: u32,
) -> Result<(), CoreError> {
    let ts = format!("{timestamp_secs}");
    let scale = format!("scale={width}:-1");
    let canonical_path =
        std::fs::canonicalize(video_path).unwrap_or_else(|_| video_path.to_path_buf());
    let video = canonical_path.to_string_lossy();
    let out = output_path.to_string_lossy();
    run_ffmpeg_command(
        ffmpeg_path,
        &[
            "-ss", &ts, "-i", &video, "-vframes", "1", "-vf", &scale, "-q:v", "2", "-y", &out,
        ],
        30,
    )?;
    Ok(())
}

// ═══════════════════════════════════════════
// Video Metadata Extraction
// ═══════════════════════════════════════════

fn parse_media_probe(parsed: &serde_json::Value) -> MediaProbe {
    let streams = parsed["streams"].as_array().cloned().unwrap_or_default();
    let video_stream = streams
        .iter()
        .find(|stream| stream["codec_type"].as_str() == Some("video"));
    let has_audio = streams
        .iter()
        .any(|stream| stream["codec_type"].as_str() == Some("audio"));
    let subtitle_streams = streams
        .iter()
        .filter(|stream| stream["codec_type"].as_str() == Some("subtitle"))
        .filter_map(|stream| {
            Some(SubtitleStream {
                index: stream["index"].as_u64()? as usize,
                language: stream["tags"]["language"].as_str().map(ToOwned::to_owned),
                codec_name: stream["codec_name"].as_str()?.to_string(),
                title: stream["tags"]["title"].as_str().map(ToOwned::to_owned),
            })
        })
        .collect();

    let width = video_stream
        .and_then(|stream| stream["width"].as_u64())
        .map(|value| value as u32);
    let height = video_stream
        .and_then(|stream| stream["height"].as_u64())
        .map(|value| value as u32);
    let codec = video_stream
        .and_then(|stream| stream["codec_name"].as_str())
        .map(ToOwned::to_owned);
    let framerate = video_stream.and_then(|stream| {
        let rate = stream["r_frame_rate"].as_str()?;
        let (numerator, denominator) = rate.split_once('/')?;
        let numerator = numerator.parse::<f64>().ok()?;
        let denominator = denominator.parse::<f64>().ok()?;
        (denominator > 0.0).then_some(numerator / denominator)
    });
    let format = &parsed["format"];
    let metadata = VideoMetadata {
        width,
        height,
        codec,
        bitrate: format["bit_rate"]
            .as_str()
            .and_then(|value| value.parse().ok()),
        framerate,
        creation_time: format["tags"]["creation_time"]
            .as_str()
            .map(ToOwned::to_owned),
        duration_secs: format["duration"]
            .as_str()
            .and_then(|value| value.parse().ok()),
    };
    MediaProbe {
        has_audio,
        subtitle_streams,
        metadata,
    }
}

/// Probe streams, duration, and video metadata with one ffprobe process.
pub fn probe_media(video_path: &Path, config: &VideoConfig) -> Result<MediaProbe, CoreError> {
    let ffmpeg_path = config.ffmpeg_path.as_deref().unwrap_or("ffmpeg");
    let ffprobe = derive_ffprobe_path(ffmpeg_path);
    let video = video_path.to_string_lossy();

    let output = run_ffmpeg_command(
        &ffprobe,
        &[
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            &video,
        ],
        30,
    )?;

    let json_str = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&json_str)
        .map_err(|e| CoreError::Video(format!("Failed to parse ffprobe metadata JSON: {e}")))?;

    Ok(parse_media_probe(&parsed))
}

/// Extract rich metadata from a video file via ffprobe.
pub fn extract_video_metadata(
    ffmpeg_path: &str,
    video_path: &Path,
) -> Result<VideoMetadata, CoreError> {
    let config = VideoConfig {
        ffmpeg_path: Some(ffmpeg_path.to_string()),
        ..VideoConfig::default()
    };
    Ok(probe_media(video_path, &config)?.metadata)
}

// ═══════════════════════════════════════════
// High-Level Video Analysis
// ═══════════════════════════════════════════

/// Analyze an audio file: convert to WAV if needed, then transcribe with Whisper.
/// Skips frame extraction entirely. Returns transcript segments.
#[cfg(feature = "video")]
pub fn analyze_audio(
    audio_path: &Path,
    config: &VideoConfig,
    speech_config: &SpeechToTextConfig,
    on_progress: impl Fn(VideoProcessingProgress),
) -> Result<VideoAnalysisResult, CoreError> {
    if !config.enabled {
        return Err(CoreError::InvalidInput("Media analysis is disabled".into()));
    }
    let _job = MediaJobLease::acquire();
    if config.transcription_mode == MediaTranscriptionMode::Disabled {
        on_progress(VideoProcessingProgress {
            phase: "complete".into(),
            progress_pct: 100.0,
            detail: Some("Audio transcription is disabled".into()),
        });
        return Ok(VideoAnalysisResult {
            transcript_segments: Vec::new(),
            full_transcript: String::new(),
            duration_secs: None,
            visual_events: Vec::new(),
            warnings: Vec::new(),
            thumbnail_path: None,
            metadata: None,
        });
    }
    // 1. Get duration
    let duration_secs = get_video_duration(audio_path, config).ok();

    // 2. Determine WAV path — if already WAV, use directly; otherwise convert via FFmpeg.
    let temp_dir;
    let wav_path = if audio_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("wav"))
        .unwrap_or(false)
    {
        // Verify it's a valid WAV by checking RIFF header
        let is_valid_wav = {
            use std::io::Read;
            let mut hdr = [0u8; 12];
            let mut f = std::fs::File::open(audio_path)
                .map_err(|e| CoreError::Video(format!("Failed to read WAV file: {e}")))?;
            let bytes_read = f
                .read(&mut hdr)
                .map_err(|e| CoreError::Video(format!("Failed to read WAV header: {e}")))?;
            bytes_read >= 12 && &hdr[0..4] == b"RIFF" && &hdr[8..12] == b"WAVE"
        };
        if is_valid_wav {
            temp_dir = None;
            audio_path.to_path_buf()
        } else {
            // Not a valid WAV despite extension — convert
            let td = tempfile::tempdir()
                .map_err(|e| CoreError::Video(format!("Failed to create temp dir: {e}")))?;
            let wav = td.path().join("audio.wav");
            on_progress(VideoProcessingProgress {
                phase: "converting_audio".into(),
                progress_pct: 0.0,
                detail: Some("Converting to WAV format...".into()),
            });
            if let Err(error) = extract_audio(audio_path, &wav, config) {
                if config.failure_policy == MediaFailurePolicy::BestEffort {
                    on_progress(VideoProcessingProgress {
                        phase: "complete".into(),
                        progress_pct: 100.0,
                        detail: Some("Audio analysis completed with warnings".into()),
                    });
                    return Ok(degraded_analysis_result(
                        duration_secs,
                        "audio_conversion_unavailable",
                        &error,
                    ));
                }
                return Err(error);
            }
            temp_dir = Some(td);
            wav
        }
    } else {
        let td = tempfile::tempdir()
            .map_err(|e| CoreError::Video(format!("Failed to create temp dir: {e}")))?;
        let wav = td.path().join("audio.wav");
        on_progress(VideoProcessingProgress {
            phase: "converting_audio".into(),
            progress_pct: 0.0,
            detail: Some("Converting to WAV format...".into()),
        });
        if let Err(error) = extract_audio(audio_path, &wav, config) {
            if config.failure_policy == MediaFailurePolicy::BestEffort {
                on_progress(VideoProcessingProgress {
                    phase: "complete".into(),
                    progress_pct: 100.0,
                    detail: Some("Audio analysis completed with warnings".into()),
                });
                return Ok(degraded_analysis_result(
                    duration_secs,
                    "audio_conversion_unavailable",
                    &error,
                ));
            }
            return Err(error);
        }
        temp_dir = Some(td);
        wav
    };

    // 3. Transcribe
    on_progress(VideoProcessingProgress {
        phase: "transcribing".into(),
        progress_pct: 30.0,
        detail: Some("Running Whisper transcription...".into()),
    });
    let mut warnings = Vec::new();
    let segments = match transcribe_media_wav(&wav_path, config, speech_config) {
        Ok(segments) => segments,
        Err(error) if config.failure_policy == MediaFailurePolicy::BestEffort => {
            tracing::warn!("Audio transcription degraded: {error}");
            warnings.push(media_warning("transcription_unavailable", &error));
            Vec::new()
        }
        Err(error) => return Err(error),
    };
    let full_transcript = segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    // 4. Cleanup temp files (if any)
    drop(temp_dir);

    on_progress(VideoProcessingProgress {
        phase: "complete".into(),
        progress_pct: 100.0,
        detail: None,
    });

    Ok(VideoAnalysisResult {
        transcript_segments: segments,
        full_transcript,
        duration_secs,
        visual_events: Vec::new(),
        warnings,
        thumbnail_path: None,
        metadata: None,
    })
}

/// RAII guard that removes a temp directory on drop (even on panic/early return).
struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Analyze a video file: extract audio, transcribe, optionally extract frames.
/// Returns transcript segments and optional frame OCR text.
#[cfg(feature = "video")]
pub fn analyze_video(
    video_path: &Path,
    config: &VideoConfig,
    #[allow(unused_variables)] ocr_config: &crate::ocr::OcrConfig,
    speech_config: &SpeechToTextConfig,
    on_progress: impl Fn(VideoProcessingProgress),
) -> Result<VideoAnalysisResult, CoreError> {
    if !config.enabled {
        return Err(CoreError::InvalidInput("Media analysis is disabled".into()));
    }
    let _job = MediaJobLease::acquire();
    let ffmpeg = config.ffmpeg_path.as_deref().unwrap_or("ffmpeg");
    let probe = match probe_media(video_path, config) {
        Ok(probe) => probe,
        Err(error) if config.failure_policy == MediaFailurePolicy::BestEffort => {
            tracing::warn!("Media probing degraded: {error}");
            on_progress(VideoProcessingProgress {
                phase: "complete".into(),
                progress_pct: 100.0,
                detail: Some("Media analysis completed with warnings".into()),
            });
            return Ok(degraded_analysis_result(
                None,
                "media_probe_unavailable",
                &error,
            ));
        }
        Err(error) => return Err(error),
    };
    let duration_secs = probe.metadata.duration_secs;
    let mut warnings = Vec::new();

    // 2. Create temp directory for working files (RAII cleanup on drop)
    let temp_dir = std::env::temp_dir().join(format!("nexa-video-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| CoreError::Video(format!("Failed to create temp dir: {e}")))?;
    let _guard = TempDirGuard(temp_dir.clone());

    // 3. Try embedded subtitle extraction first (faster & more accurate than Whisper)
    let subtitle_segments = if !config.prefer_embedded_subtitles {
        None
    } else {
        match probe.subtitle_streams.as_slice() {
            streams if !streams.is_empty() => {
                on_progress(VideoProcessingProgress {
                    phase: "extracting_subtitles".into(),
                    progress_pct: 10.0,
                    detail: Some(format!(
                        "Found {} embedded subtitle stream(s), extracting...",
                        streams.len()
                    )),
                });
                let srt_path = temp_dir.join("subtitles.srt");
                let stream = &streams[0];
                match extract_subtitles(ffmpeg, video_path, stream.index, &srt_path) {
                    Ok(srt_content) => {
                        let parsed = parse_srt(&srt_content);
                        if parsed.is_empty() {
                            None
                        } else {
                            on_progress(VideoProcessingProgress {
                                phase: "subtitles_extracted".into(),
                                progress_pct: 30.0,
                                detail: Some(format!(
                                    "Extracted {} subtitle segments",
                                    parsed.len()
                                )),
                            });
                            Some(parsed)
                        }
                    }
                    Err(_) => None,
                }
            }
            _ => None,
        }
    };

    // 4. Fall back to the configured media transcription backend.
    let segments = if let Some(subs) = subtitle_segments {
        subs
    } else if probe.has_audio && config.transcription_mode != MediaTranscriptionMode::Disabled {
        let wav_path = temp_dir.join("audio.wav");
        on_progress(VideoProcessingProgress {
            phase: "extracting_audio".into(),
            progress_pct: 10.0,
            detail: Some("Extracting audio track...".into()),
        });
        match extract_audio(video_path, &wav_path, config) {
            Ok(()) => {
                on_progress(VideoProcessingProgress {
                    phase: "transcribing".into(),
                    progress_pct: 30.0,
                    detail: Some("Running media transcription...".into()),
                });
                match transcribe_media_wav(&wav_path, config, speech_config) {
                    Ok(segments) => segments,
                    Err(error) if config.failure_policy == MediaFailurePolicy::BestEffort => {
                        tracing::warn!("Video transcription degraded: {error}");
                        warnings.push(media_warning("transcription_unavailable", &error));
                        Vec::new()
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(error) if config.failure_policy == MediaFailurePolicy::BestEffort => {
                tracing::warn!("Video audio extraction degraded: {error}");
                warnings.push(media_warning("audio_extraction_unavailable", &error));
                Vec::new()
            }
            Err(error) => return Err(error),
        }
    } else if config.failure_policy == MediaFailurePolicy::RequireTranscript
        && config.transcription_mode != MediaTranscriptionMode::Disabled
    {
        return Err(CoreError::Video(
            "A transcript is required, but the media has no usable audio or subtitles".into(),
        ));
    } else {
        tracing::info!("No audio track found, skipping transcription");
        Vec::new()
    };

    let full_transcript = segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    // 5. Optionally extract frames for OCR
    #[cfg(feature = "ocr")]
    let mut visual_events = Vec::new();
    #[cfg(not(feature = "ocr"))]
    let visual_events = Vec::new();
    #[cfg(feature = "ocr")]
    if config.frame_extraction_enabled {
        if !ocr_config.enabled {
            warnings.push(MediaAnalysisWarning {
                code: "visual_analysis_disabled".into(),
                message: "Frame extraction is enabled but OCR is disabled".into(),
            });
        } else {
            on_progress(VideoProcessingProgress {
                phase: "extracting_frames".into(),
                progress_pct: 70.0,
                detail: Some("Extracting key frames...".into()),
            });
            let frames_dir = temp_dir.join("frames");
            let extracted =
                extract_keyframes(video_path, &frames_dir, config.scene_threshold, config)
                    .and_then(|keyframes| {
                        if keyframes.is_empty() {
                            extract_frames(
                                video_path,
                                &temp_dir.join("frames_fixed"),
                                config.frame_interval_secs,
                                config,
                            )
                        } else {
                            Ok(keyframes)
                        }
                    });
            let frame_paths = match extracted {
                Ok(frames) => frames,
                Err(error) => {
                    tracing::warn!("Frame extraction degraded: {error}");
                    warnings.push(media_warning("frame_extraction_unavailable", &error));
                    Vec::new()
                }
            };

            use crate::ocr::{extract_text_from_image, OcrSource};
            let mut ocr_warning_recorded = false;
            for (index, frame) in frame_paths.iter().enumerate() {
                on_progress(VideoProcessingProgress {
                    phase: "ocr".into(),
                    progress_pct: 70.0 + (index as f32 / frame_paths.len().max(1) as f32) * 25.0,
                    detail: Some(format!("OCR frame {}/{}", index + 1, frame_paths.len())),
                });
                let frame_bytes = match std::fs::read(&frame.path) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        if !ocr_warning_recorded {
                            warnings.push(MediaAnalysisWarning {
                                code: "frame_ocr_unavailable".into(),
                                message: error.to_string(),
                            });
                            ocr_warning_recorded = true;
                        }
                        continue;
                    }
                };
                match extract_text_from_image(&frame_bytes, "image/jpeg", ocr_config, None) {
                    Ok(result) if !result.full_text.trim().is_empty() => {
                        let source = match result.source {
                            OcrSource::PaddleOcr => "paddle_ocr",
                            OcrSource::LlmVision => "llm_vision",
                            OcrSource::None => "none",
                        };
                        let end_ms = frame_paths
                            .get(index + 1)
                            .map(|next| next.timestamp_ms)
                            .or_else(|| {
                                duration_secs.map(|duration| (duration * 1000.0).round() as i64)
                            })
                            .unwrap_or(
                                frame.timestamp_ms + config.frame_interval_secs as i64 * 1000,
                            )
                            .max(frame.timestamp_ms + 1);
                        visual_events.push(VisualEvent {
                            timestamp_ms: frame.timestamp_ms,
                            end_ms,
                            text: result.full_text,
                            confidence: result.avg_confidence,
                            source: source.into(),
                        });
                    }
                    Ok(_) => {}
                    Err(error) => {
                        if !ocr_warning_recorded {
                            warnings.push(media_warning("frame_ocr_unavailable", &error));
                            ocr_warning_recorded = true;
                        }
                    }
                }
            }
        }
    }

    // 6. Generate thumbnail
    let thumbnail_path = {
        let thumb_dir = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(crate::APP_DIR)
            .join("thumbnails");
        let _ = std::fs::create_dir_all(&thumb_dir);
        let video_hash = blake3::hash(video_path.to_string_lossy().as_bytes())
            .to_hex()
            .to_string();
        let thumb_path = thumb_dir.join(format!("{video_hash}.jpg"));
        let thumb_ts = match duration_secs {
            Some(dur) => (dur * 0.1).min(5.0),
            None => 2.0,
        };
        match generate_thumbnail(ffmpeg, video_path, &thumb_path, thumb_ts, 320) {
            Ok(()) => Some(thumb_path),
            Err(e) => {
                tracing::warn!("Failed to generate thumbnail: {e}");
                None
            }
        }
    };

    // Cleanup handled by TempDirGuard on drop
    on_progress(VideoProcessingProgress {
        phase: "complete".into(),
        progress_pct: 100.0,
        detail: None,
    });

    Ok(VideoAnalysisResult {
        transcript_segments: segments,
        full_transcript,
        duration_secs,
        visual_events,
        warnings,
        thumbnail_path,
        metadata: Some(probe.metadata),
    })
}

// ═══════════════════════════════════════════
// Database Persistence
// ═══════════════════════════════════════════

impl Database {
    pub fn save_video_config(&self, config: &VideoConfig) -> Result<(), CoreError> {
        let mut normalized = config.clone();
        // The bundled Candle runtime currently executes this path on CPU with
        // greedy decoding. Persist only settings that the runtime honors.
        normalized.use_gpu = false;
        normalized.beam_size = 1;
        let json = serde_json::to_string(&normalized)?;
        self.conn().execute(
            "INSERT OR REPLACE INTO video_config (key, value) VALUES ('config', ?1)",
            [&json],
        )?;
        Ok(())
    }

    pub fn load_video_config(&self) -> Result<VideoConfig, CoreError> {
        let result = self.conn().query_row(
            "SELECT value FROM video_config WHERE key = 'config'",
            [],
            |row| row.get::<_, String>(0),
        );
        match result {
            Ok(json) => {
                let mut config: VideoConfig = serde_json::from_str(&json)?;
                config.use_gpu = false;
                config.beam_size = 1;
                Ok(config)
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(VideoConfig::default()),
            Err(e) => Err(CoreError::Database(e)),
        }
    }
}

// ═══════════════════════════════════════════
// Supported MIME Types
// ═══════════════════════════════════════════

/// All supported video MIME types.
pub const VIDEO_MIME_TYPES: &[&str] = &[
    "video/mp4",
    "video/x-matroska",
    "video/webm",
    "video/x-msvideo",
    "video/quicktime",
    "video/x-flv",
    "video/mpeg",
    "video/x-ms-wmv",
    "video/x-m4v",
    "video/3gpp",
    "video/mp2t",
];

/// All supported audio MIME types.
pub const AUDIO_MIME_TYPES: &[&str] = &[
    "audio/mpeg",
    "audio/wav",
    "audio/flac",
    "audio/ogg",
    "audio/aac",
    "audio/mp4",
    "audio/x-ms-wma",
    "audio/opus",
];

/// Check if a MIME type is a supported audio format.
pub fn is_supported_audio(mime: &str) -> bool {
    AUDIO_MIME_TYPES.contains(&mime)
}

/// Check if a MIME type is a supported video format.
pub fn is_supported_video(mime: &str) -> bool {
    VIDEO_MIME_TYPES.contains(&mime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_whisper_model_filename() {
        // Whisper now uses the safetensors directory layout exclusively.
        assert_eq!(WhisperModel::Tiny.dir_name(), "whisper-tiny");
        assert_eq!(WhisperModel::Base.dir_name(), "whisper-base");
        assert_eq!(WhisperModel::Large.dir_name(), "whisper-large-v3");
        // Repo IDs
        assert_eq!(WhisperModel::Tiny.repo_id(), "openai/whisper-tiny");
        assert_eq!(WhisperModel::Large.repo_id(), "openai/whisper-large-v3");
        // Required files
        assert_eq!(WhisperModel::Tiny.required_files().len(), 3); // config, tokenizer, model
        assert_eq!(WhisperModel::Large.required_files().len(), 4); // config, tokenizer, 2 shards
    }

    #[test]
    fn test_default_config() {
        let config = VideoConfig::default();
        assert!(!config.enabled);
        assert_eq!(
            config.transcription_mode,
            MediaTranscriptionMode::InheritSpeechToText
        );
        assert_eq!(config.failure_policy, MediaFailurePolicy::BestEffort);
        assert_eq!(config.whisper_model, WhisperModel::Base);
        assert_eq!(config.frame_interval_secs, 10);
        assert!(!config.frame_extraction_enabled);
        assert!(!config.use_gpu);
        assert_eq!(config.beam_size, 1);
    }

    #[test]
    fn legacy_video_config_gets_media_policy_defaults() {
        let mut value = serde_json::to_value(VideoConfig::default()).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("transcriptionMode");
        object.remove("failurePolicy");

        let config: VideoConfig = serde_json::from_value(value).unwrap();
        assert_eq!(
            config.transcription_mode,
            MediaTranscriptionMode::InheritSpeechToText
        );
        assert_eq!(config.failure_policy, MediaFailurePolicy::BestEffort);
    }

    #[test]
    fn showinfo_parser_preserves_real_frame_timestamps() {
        let stderr = "[Parsed_showinfo_1] n:0 pts:123 pts_time:1.234 pos:0\n\
                      [Parsed_showinfo_1] n:1 pts:987 pts_time:9.8765 pos:1";
        assert_eq!(parse_showinfo_timestamps(stderr), vec![1234, 9877]);
    }

    #[test]
    fn media_probe_collects_stream_capabilities_in_one_result() {
        let probe = parse_media_probe(&serde_json::json!({
            "streams": [
                { "index": 0, "codec_type": "video", "codec_name": "h264", "width": 1920, "height": 1080, "r_frame_rate": "30000/1001" },
                { "index": 1, "codec_type": "audio", "codec_name": "aac" },
                { "index": 2, "codec_type": "subtitle", "codec_name": "subrip", "tags": { "language": "zh", "title": "Chinese" } }
            ],
            "format": { "duration": "42.5", "bit_rate": "900000" }
        }));
        assert!(probe.has_audio);
        assert_eq!(probe.subtitle_streams.len(), 1);
        assert_eq!(probe.subtitle_streams[0].language.as_deref(), Some("zh"));
        assert_eq!(probe.metadata.width, Some(1920));
        assert_eq!(probe.metadata.duration_secs, Some(42.5));
    }

    #[test]
    fn inherited_cloud_speech_readiness_does_not_require_whisper() {
        let video = VideoConfig {
            enabled: true,
            transcription_mode: MediaTranscriptionMode::InheritSpeechToText,
            ffmpeg_path: Some("/nonexistent/ffmpeg".into()),
            ..VideoConfig::default()
        };
        let speech = SpeechToTextConfig {
            provider: "openai".into(),
            api_style: "openai_transcription".into(),
            api_key: "test-key".into(),
            base_url: Some("https://example.invalid/v1".into()),
            model: "whisper-1".into(),
            ..SpeechToTextConfig::default()
        };
        let status = media_runtime_status(&video, &crate::ocr::OcrConfig::default(), &speech);
        assert!(status.transcription.configured);
        assert!(status.transcription.ready);
        assert!(!status.transcription.degraded);
        assert!(status.probe.degraded);
    }

    #[test]
    fn disabled_media_runtime_has_no_degraded_capabilities() {
        let status = media_runtime_status(
            &VideoConfig::default(),
            &crate::ocr::OcrConfig::default(),
            &SpeechToTextConfig::default(),
        );
        assert!(!status.enabled);
        assert!(!status.probe.degraded);
        assert!(!status.transcription.degraded);
        assert!(!status.visual_analysis.degraded);
    }

    #[test]
    fn disabled_audio_transcription_does_not_require_ffmpeg_or_asr() {
        let temp = tempfile::NamedTempFile::with_suffix(".mp3").unwrap();
        let video = VideoConfig {
            enabled: true,
            transcription_mode: MediaTranscriptionMode::Disabled,
            ffmpeg_path: Some("/definitely/missing/ffmpeg".into()),
            ..VideoConfig::default()
        };
        let result = analyze_audio(temp.path(), &video, &SpeechToTextConfig::default(), |_| {})
            .expect("disabled transcription should be a no-op");
        assert!(result.transcript_segments.is_empty());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn media_job_lease_tracks_model_deletion_guard() {
        // Other media tests may execute in parallel, so only assert the
        // invariant this lease owns: while it exists, deletion is guarded.
        let _lease = MediaJobLease::acquire();
        assert!(media_jobs_active());
    }

    #[test]
    fn test_is_supported_video() {
        assert!(is_supported_video("video/mp4"));
        assert!(is_supported_video("video/webm"));
        assert!(is_supported_video("video/quicktime"));
        assert!(!is_supported_video("audio/mp3"));
        assert!(!is_supported_video("image/png"));
    }

    #[test]
    fn test_check_ffmpeg_not_found() {
        let config = VideoConfig {
            ffmpeg_path: Some("/nonexistent/path/ffmpeg".into()),
            ..VideoConfig::default()
        };
        assert!(!check_ffmpeg(&config).unwrap());
    }

    #[test]
    fn test_save_and_load_video_config() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path().join("test.db").to_str().unwrap()).unwrap();

        let config = VideoConfig {
            enabled: true,
            whisper_model: WhisperModel::Small,
            language: Some("zh".into()),
            use_gpu: true,
            beam_size: 7,
            ..VideoConfig::default()
        };
        db.save_video_config(&config).unwrap();

        let loaded = db.load_video_config().unwrap();
        assert!(loaded.enabled);
        assert_eq!(loaded.whisper_model, WhisperModel::Small);
        assert_eq!(loaded.language, Some("zh".into()));
        assert!(!loaded.use_gpu);
        assert_eq!(loaded.beam_size, 1);
    }

    #[test]
    fn test_load_video_config_default() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path().join("test.db").to_str().unwrap()).unwrap();

        let config = db.load_video_config().unwrap();
        assert!(!config.enabled);
        assert_eq!(config.whisper_model, WhisperModel::Base);
    }

    #[test]
    fn whisper_fallback_urls_swap_host() {
        let urls = WhisperModel::Tiny.fallback_download_urls("https://hf-mirror.com");
        assert!(!urls.is_empty());
        assert!(urls.iter().all(|(_, u)| u.contains("hf-mirror.com")));
        assert!(urls.iter().all(|(_, u)| !u.contains("huggingface.co")));
        // Primary still points at huggingface.co
        let primary = WhisperModel::Tiny.download_urls();
        assert!(primary.iter().all(|(_, u)| u.contains("huggingface.co")));
    }

    #[test]
    fn whisper_fallback_urls_empty_base_disables_mirror() {
        let urls = WhisperModel::Tiny.fallback_download_urls("");
        assert!(urls.is_empty());
    }

    #[test]
    fn whisper_fallback_urls_cover_all_required_files() {
        let primary = WhisperModel::Base.download_urls();
        let mirror = WhisperModel::Base.fallback_download_urls("https://hf-mirror.com");
        assert_eq!(primary.len(), mirror.len());
        for ((fp, _), (fm, _)) in primary.iter().zip(mirror.iter()) {
            assert_eq!(fp, fm);
        }
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn ffmpeg_mirror_url_uses_ghproxy_on_github_platforms() {
        let mirror = ffmpeg_mirror_url("https://mirror.ghproxy.com")
            .expect("mirror url ok")
            .expect("some");
        assert!(mirror.starts_with("https://mirror.ghproxy.com/"));
        assert!(mirror.contains("github.com"));
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn ffmpeg_mirror_url_empty_base_disables_mirror() {
        assert!(ffmpeg_mirror_url("").unwrap().is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn ffmpeg_mirror_url_none_on_macos() {
        assert!(ffmpeg_mirror_url("https://mirror.ghproxy.com")
            .unwrap()
            .is_none());
    }
}
