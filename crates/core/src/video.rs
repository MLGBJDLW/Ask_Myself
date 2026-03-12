//! Video analysis: FFmpeg audio extraction + Whisper speech-to-text.
//!
//! Follows the same pattern as `ocr.rs`: config struct, model management,
//! DB persistence, progress callbacks.

use crate::db::Database;
use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ═══════════════════════════════════════════
// Configuration
// ═══════════════════════════════════════════

/// Whisper model sizes with file size and accuracy trade-offs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhisperModel {
    Tiny,       // ~39 MB, fastest, lowest accuracy
    Base,       // ~142 MB, good balance
    Small,      // ~466 MB, better accuracy
    Medium,     // ~1.5 GB, high accuracy
    Large,      // ~3.1 GB, most accurate
    LargeTurbo, // ~1.6 GB, best speed/accuracy tradeoff
}

impl Default for WhisperModel {
    fn default() -> Self {
        Self::Base
    }
}

impl WhisperModel {
    pub fn filename(&self) -> &'static str {
        match self {
            Self::Tiny => "ggml-tiny.bin",
            Self::Base => "ggml-base.bin",
            Self::Small => "ggml-small.bin",
            Self::Medium => "ggml-medium.bin",
            Self::Large => "ggml-large-v3.bin",
            Self::LargeTurbo => "ggml-large-v3-turbo.bin",
        }
    }

    pub fn download_url(&self) -> &'static str {
        match self {
            Self::Tiny => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
            Self::Base => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
            Self::Small => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"
            }
            Self::Medium => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin"
            }
            Self::Large => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin"
            }
            Self::LargeTurbo => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin"
            }
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Tiny => "Tiny (~39 MB)",
            Self::Base => "Base (~142 MB)",
            Self::Small => "Small (~466 MB)",
            Self::Medium => "Medium (~1.5 GB)",
            Self::Large => "Large v3 (~3.1 GB)",
            Self::LargeTurbo => "Large v3 Turbo (~1.6 GB)",
        }
    }

    /// Expected file size in bytes for basic download integrity verification.
    /// Returns `None` for unknown models (download proceeds but logs a warning).
    pub fn expected_file_size(&self) -> Option<u64> {
        match self {
            Self::Tiny => Some(39_055_616),
            Self::Base => Some(147_951_465),
            Self::Small => Some(487_601_967),
            Self::Medium => Some(1_533_774_081),
            Self::Large => Some(3_095_033_483),
            Self::LargeTurbo => Some(1_624_938_331),
        }
    }
}

/// User-configurable video analysis settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoConfig {
    pub enabled: bool,
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
    #[serde(default = "default_true", alias = "use_gpu")]
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
    5
}

impl Default for VideoConfig {
    fn default() -> Self {
        let model_path = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ask-myself")
            .join("models")
            .join("whisper")
            .to_string_lossy()
            .to_string();
        Self {
            enabled: false, // Disabled by default (unlike OCR)
            whisper_model: WhisperModel::default(),
            language: None,
            translate_to_english: false,
            ffmpeg_path: None,
            frame_extraction_enabled: false,
            frame_interval_secs: 10,
            model_path,
            scene_threshold: default_scene_threshold(),
            use_gpu: default_true(),
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
pub struct TranscriptSegment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

/// Result of video analysis.
#[derive(Debug, Clone)]
pub struct VideoAnalysisResult {
    pub transcript_segments: Vec<TranscriptSegment>,
    pub full_transcript: String,
    pub duration_secs: Option<f64>,
    pub frame_texts: Vec<String>, // OCR text from extracted frames
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
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("ffmpeg");
    let ffprobe_name = if stem.eq_ignore_ascii_case("ffmpeg") {
        if ext.is_empty() { "ffprobe".to_string() } else { format!("ffprobe.{ext}") }
    } else {
        let replaced = stem.replace("ffmpeg", "ffprobe");
        if ext.is_empty() { replaced } else { format!("{replaced}.{ext}") }
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
                return Err(CoreError::Video(format!("Failed to check process status: {e}")));
            }
        }
    };

    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();

    if !status.success() {
        let stderr_str = String::from_utf8_lossy(&stderr);
        return Err(CoreError::Video(format!("{program} failed: {stderr_str}")));
    }

    Ok(std::process::Output { status, stdout, stderr })
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
    let canonical_path = std::fs::canonicalize(video_path)
        .unwrap_or_else(|_| video_path.to_path_buf());
    let video = canonical_path.to_string_lossy();
    let output = output_wav.to_string_lossy();
    run_ffmpeg_command(
        ffmpeg,
        &[
            "-i", &video,
            "-vn",              // No video
            "-acodec", "pcm_s16le", // 16-bit PCM
            "-ar", "16000",     // 16kHz (Whisper requirement)
            "-ac", "1",         // Mono
            "-y",               // Overwrite
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
) -> Result<Vec<PathBuf>, CoreError> {
    let interval_secs = interval_secs.max(1);
    std::fs::create_dir_all(output_dir)
        .map_err(|e| CoreError::Video(format!("Failed to create frame output dir: {e}")))?;

    let ffmpeg = config.ffmpeg_path.as_deref().unwrap_or("ffmpeg");
    let fps_filter = format!("fps=1/{interval_secs}");
    let output_pattern = output_dir.join("frame_%04d.jpg");
    let canonical_path = std::fs::canonicalize(video_path)
        .unwrap_or_else(|_| video_path.to_path_buf());
    let video = canonical_path.to_string_lossy();
    let pattern = output_pattern.to_string_lossy();

    run_ffmpeg_command(
        ffmpeg,
        &[
            "-i", &video,
            "-vf", &fps_filter,
            "-q:v", "2", // High quality JPEG
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
        .filter(|p| p.extension().map_or(false, |ext| ext == "jpg"))
        .collect();
    frames.sort();
    Ok(frames)
}

/// Extract keyframes using FFmpeg scene-change detection.
/// Falls back to empty vec if scene detection produces no frames.
pub fn extract_keyframes(
    video_path: &Path,
    output_dir: &Path,
    scene_threshold: f64,
    config: &VideoConfig,
) -> Result<Vec<PathBuf>, CoreError> {
    std::fs::create_dir_all(output_dir)
        .map_err(|e| CoreError::Video(format!("Failed to create keyframe output dir: {e}")))?;

    let ffmpeg = config.ffmpeg_path.as_deref().unwrap_or("ffmpeg");
    let vf = format!("select='gt(scene,{scene_threshold})'");
    let output_pattern = output_dir.join("scene_%04d.jpg");
    let canonical_path = std::fs::canonicalize(video_path)
        .unwrap_or_else(|_| video_path.to_path_buf());
    let video = canonical_path.to_string_lossy();
    let pattern = output_pattern.to_string_lossy();

    // Scene detection may legitimately produce zero frames (static video), so
    // we treat a non-zero exit as an error but zero output files as OK.
    let result = run_ffmpeg_command(
        ffmpeg,
        &[
            "-i", &video,
            "-vf", &vf,
            "-vsync", "vfr",
            "-q:v", "3",
            "-y",
            &pattern,
        ],
        1800,
    );

    if let Err(e) = result {
        log::warn!("Scene detection failed, will fall back to fixed-interval: {e}");
        return Ok(Vec::new());
    }

    let mut frames: Vec<PathBuf> = std::fs::read_dir(output_dir)
        .map_err(|e| CoreError::Video(format!("Failed to read keyframe dir: {e}")))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|p| p.extension().map_or(false, |ext| ext == "jpg"))
        .collect();
    frames.sort();
    Ok(frames)
}

/// Get video duration in seconds using FFprobe.
pub fn get_video_duration(video_path: &Path, config: &VideoConfig) -> Result<f64, CoreError> {
    let ffmpeg = config.ffmpeg_path.as_deref().unwrap_or("ffmpeg");
    let ffprobe = derive_ffprobe_path(ffmpeg);
    let video = video_path.to_string_lossy();

    let output = run_ffmpeg_command(
        &ffprobe,
        &[
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "csv=p=0",
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

/// Check if a video file contains at least one audio stream.
fn has_audio_stream(ffmpeg_path: &str, video_path: &Path) -> bool {
    let ffprobe = derive_ffprobe_path(ffmpeg_path);
    let canonical_path = std::fs::canonicalize(video_path)
        .unwrap_or_else(|_| video_path.to_path_buf());
    let video_str = canonical_path.to_string_lossy();
    match run_ffmpeg_command(
        &ffprobe,
        &[
            "-v", "quiet",
            "-select_streams", "a",
            "-show_entries", "stream=codec_type",
            "-of", "csv=p=0",
            &video_str,
        ],
        30,
    ) {
        Ok(output) => !output.stdout.is_empty(),
        Err(_) => false,
    }
}

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
            "-v", "quiet",
            "-print_format", "json",
            "-show_streams",
            "-select_streams", "s",
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
    let canonical_path = std::fs::canonicalize(video_path)
        .unwrap_or_else(|_| video_path.to_path_buf());
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
// Whisper Transcription
// ═══════════════════════════════════════════

/// Transcribe a WAV file using whisper-rs.
#[cfg(feature = "video")]
pub fn transcribe_audio(
    wav_path: &Path,
    config: &VideoConfig,
) -> Result<Vec<TranscriptSegment>, CoreError> {
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    let model_path = Path::new(&config.model_path).join(config.whisper_model.filename());
    if !model_path.exists() {
        return Err(CoreError::Video(format!(
            "Whisper model not found: {}. Please download it first.",
            model_path.display()
        )));
    }

    // Initialize Whisper context with GPU acceleration
    let mut ctx_params = WhisperContextParameters::default();
    ctx_params.use_gpu(config.use_gpu);
    let ctx = WhisperContext::new_with_params(
        &model_path.to_string_lossy(),
        ctx_params,
    )
    .map_err(|e| CoreError::Video(format!("Failed to load Whisper model: {e}")))?;

    // Read WAV audio data
    let audio_data = read_wav_pcm(wav_path)?;

    // Configure transcription parameters
    let mut params = FullParams::new(SamplingStrategy::BeamSearch { beam_size: config.beam_size.max(1).min(16) as i32, patience: 1.0 });
    if let Some(ref lang) = config.language {
        params.set_language(Some(lang));
    }
    params.set_translate(config.translate_to_english);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_token_timestamps(true);

    // Run transcription
    let mut state = ctx
        .create_state()
        .map_err(|e| CoreError::Video(format!("Failed to create Whisper state: {e}")))?;
    state
        .full(params, &audio_data)
        .map_err(|e| CoreError::Video(format!("Whisper transcription failed: {e}")))?;

    // Extract segments
    let num_segments = state
        .full_n_segments()
        .map_err(|e| CoreError::Video(format!("Failed to get segment count: {e}")))?;
    let mut segments = Vec::new();

    for i in 0..num_segments {
        let start_ms = state
            .full_get_segment_t0(i)
            .map_err(|e| CoreError::Video(format!("Failed to get segment start: {e}")))?
            as i64
            * 10;
        let end_ms = state
            .full_get_segment_t1(i)
            .map_err(|e| CoreError::Video(format!("Failed to get segment end: {e}")))?
            as i64
            * 10;
        let text = state
            .full_get_segment_text(i)
            .map_err(|e| CoreError::Video(format!("Failed to get segment text: {e}")))?;

        let text = text.trim().to_string();
        if !text.is_empty() {
            segments.push(TranscriptSegment {
                start_ms,
                end_ms,
                text,
            });
        }
    }

    Ok(segments)
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

/// Check if the selected Whisper model exists on disk.
pub fn check_whisper_model_exists(config: &VideoConfig) -> bool {
    let model_path = Path::new(&config.model_path).join(config.whisper_model.filename());
    model_path.exists()
}

/// Download the selected Whisper model with progress reporting.
pub fn download_whisper_model(
    config: &VideoConfig,
    on_progress: impl Fn(VideoDownloadProgress),
) -> Result<(), CoreError> {
    let model_dir = Path::new(&config.model_path);
    std::fs::create_dir_all(model_dir)
        .map_err(|e| CoreError::Video(format!("Failed to create model directory: {e}")))?;

    let url = config.whisper_model.download_url();
    let filename = config.whisper_model.filename();
    let dest = model_dir.join(filename);
    let tmp_dest = dest.with_extension("bin.partial");

    // Download with progress
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(url)
        .send()
        .map_err(|e| CoreError::Video(format!("Failed to download model: {e}")))?;

    let total_bytes = resp.content_length();
    let mut bytes_downloaded: u64 = 0;

    let mut file = std::fs::File::create(&tmp_dest)
        .map_err(|e| CoreError::Video(format!("Failed to create model file: {e}")))?;

    let mut hasher = blake3::Hasher::new();
    let mut reader = std::io::BufReader::new(resp);
    let mut buffer = [0u8; 8192];
    loop {
        use std::io::Read;
        let n = reader
            .read(&mut buffer)
            .map_err(|e| CoreError::Video(format!("Download read error: {e}")))?;
        if n == 0 {
            break;
        }
        use std::io::Write;
        file.write_all(&buffer[..n])
            .map_err(|e| CoreError::Video(format!("Failed to write model file: {e}")))?;
        hasher.update(&buffer[..n]);
        bytes_downloaded += n as u64;
        on_progress(VideoDownloadProgress {
            filename: filename.to_string(),
            bytes_downloaded,
            total_bytes,
        });
    }
    drop(file);

    // Verify file size as a basic integrity check
    if let Some(expected_size) = config.whisper_model.expected_file_size() {
        if bytes_downloaded != expected_size {
            let _ = std::fs::remove_file(&tmp_dest);
            return Err(CoreError::Video(format!(
                "Download size mismatch for {filename}: expected {expected_size} bytes, got {bytes_downloaded}"
            )));
        }
    }

    // Atomic rename: partial -> final
    std::fs::rename(&tmp_dest, &dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_dest);
        CoreError::Video(format!("Failed to finalize model file: {e}"))
    })?;

    // Compute and log blake3 hash for audit / future pinning
    let hash = hasher.finalize();
    tracing::info!("Downloaded {filename}: blake3={hash}, size={bytes_downloaded}");

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
    let canonical_path = std::fs::canonicalize(video_path)
        .unwrap_or_else(|_| video_path.to_path_buf());
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

/// Extract rich metadata from a video file via ffprobe.
pub fn extract_video_metadata(
    ffmpeg_path: &str,
    video_path: &Path,
) -> Result<VideoMetadata, CoreError> {
    let ffprobe = derive_ffprobe_path(ffmpeg_path);
    let video = video_path.to_string_lossy();

    let output = run_ffmpeg_command(
        &ffprobe,
        &[
            "-v", "quiet",
            "-print_format", "json",
            "-show_format",
            "-show_streams",
            "-select_streams", "v:0",
            &video,
        ],
        30,
    )?;

    let json_str = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&json_str)
        .map_err(|e| CoreError::Video(format!("Failed to parse ffprobe metadata JSON: {e}")))?;

    let stream = parsed["streams"].as_array().and_then(|a| a.first());

    let width = stream.and_then(|s| s["width"].as_u64()).map(|v| v as u32);
    let height = stream.and_then(|s| s["height"].as_u64()).map(|v| v as u32);
    let codec = stream.and_then(|s| s["codec_name"].as_str()).map(|s| s.to_string());
    let framerate = stream.and_then(|s| {
        let r_frame_rate = s["r_frame_rate"].as_str()?;
        let parts: Vec<&str> = r_frame_rate.split('/').collect();
        if parts.len() == 2 {
            let num: f64 = parts[0].parse().ok()?;
            let den: f64 = parts[1].parse().ok()?;
            if den > 0.0 { Some(num / den) } else { None }
        } else {
            r_frame_rate.parse().ok()
        }
    });

    let format = &parsed["format"];
    let bitrate = format["bit_rate"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok());
    let duration_secs = format["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok());
    let creation_time = format["tags"]["creation_time"]
        .as_str()
        .map(|s| s.to_string());

    Ok(VideoMetadata {
        width,
        height,
        codec,
        bitrate,
        framerate,
        creation_time,
        duration_secs,
    })
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
    on_progress: impl Fn(VideoProcessingProgress),
) -> Result<VideoAnalysisResult, CoreError> {
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
            let bytes_read = f.read(&mut hdr)
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
            extract_audio(audio_path, &wav, config)?;
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
        extract_audio(audio_path, &wav, config)?;
        temp_dir = Some(td);
        wav
    };

    // 3. Transcribe
    on_progress(VideoProcessingProgress {
        phase: "transcribing".into(),
        progress_pct: 30.0,
        detail: Some("Running Whisper transcription...".into()),
    });
    let segments = transcribe_audio(&wav_path, config)?;
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
        frame_texts: Vec::new(), // No frames for audio
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
    on_progress: impl Fn(VideoProcessingProgress),
) -> Result<VideoAnalysisResult, CoreError> {
    // Early check: verify Whisper model exists if we'll need transcription
    let ffmpeg = config.ffmpeg_path.as_deref().unwrap_or("ffmpeg");
    let has_embedded_subs = config.prefer_embedded_subtitles
        && detect_subtitle_streams(ffmpeg, video_path)
            .map(|s| !s.is_empty())
            .unwrap_or(false);
    let has_audio = has_audio_stream(ffmpeg, video_path);

    if !has_embedded_subs && has_audio && !check_whisper_model_exists(config) {
        return Err(CoreError::Video(
            "Whisper model not found. Please download it in Settings > Models before analyzing video files.".into(),
        ));
    }

    // 1. Get duration
    let duration_secs = get_video_duration(video_path, config).ok();

    // 2. Create temp directory for working files (RAII cleanup on drop)
    let temp_dir = std::env::temp_dir().join(format!("ask-myself-video-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| CoreError::Video(format!("Failed to create temp dir: {e}")))?;
    let _guard = TempDirGuard(temp_dir.clone());

    // 3. Try embedded subtitle extraction first (faster & more accurate than Whisper)
    let subtitle_segments = if !config.prefer_embedded_subtitles {
        None
    } else {
        match detect_subtitle_streams(ffmpeg, video_path) {
        Ok(streams) if !streams.is_empty() => {
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

    // 4. Fall back to Whisper transcription if no embedded subtitles
    let segments = if let Some(subs) = subtitle_segments {
        subs
    } else if has_audio_stream(ffmpeg, video_path) {
        let wav_path = temp_dir.join("audio.wav");
        on_progress(VideoProcessingProgress {
            phase: "extracting_audio".into(),
            progress_pct: 10.0,
            detail: Some("Extracting audio track...".into()),
        });
        extract_audio(video_path, &wav_path, config)?;

        on_progress(VideoProcessingProgress {
            phase: "transcribing".into(),
            progress_pct: 30.0,
            detail: Some("Running Whisper transcription...".into()),
        });
        transcribe_audio(&wav_path, config)?
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
    let mut frame_texts = Vec::new();
    if config.frame_extraction_enabled {
        on_progress(VideoProcessingProgress {
            phase: "extracting_frames".into(),
            progress_pct: 70.0,
            detail: Some("Extracting key frames...".into()),
        });
        let frames_dir = temp_dir.join("frames");

        // Try scene-change detection first, fall back to fixed-interval
        let frame_paths = {
            let keyframes = extract_keyframes(video_path, &frames_dir, config.scene_threshold, config)?;
            if keyframes.is_empty() {
                let fallback_dir = temp_dir.join("frames_fixed");
                extract_frames(video_path, &fallback_dir, config.frame_interval_secs, config)?
            } else {
                keyframes
            }
        };

        // OCR each frame using existing OCR pipeline
        #[cfg(feature = "ocr")]
        {
            use crate::ocr::{extract_text_from_image, OcrConfig};
            let ocr_config = OcrConfig::default();
            for (i, frame_path) in frame_paths.iter().enumerate() {
                on_progress(VideoProcessingProgress {
                    phase: "ocr".into(),
                    progress_pct: 70.0 + (i as f32 / frame_paths.len() as f32) * 25.0,
                    detail: Some(format!("OCR frame {}/{}", i + 1, frame_paths.len())),
                });
                if let Ok(frame_bytes) = std::fs::read(frame_path) {
                    if let Ok(result) =
                        extract_text_from_image(&frame_bytes, "image/jpeg", &ocr_config, None)
                    {
                        if !result.full_text.trim().is_empty() {
                            frame_texts.push(result.full_text);
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
            .join("ask-myself")
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

    // 7. Extract video metadata via ffprobe
    let video_meta = extract_video_metadata(ffmpeg, video_path).ok();

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
        frame_texts,
        thumbnail_path,
        metadata: video_meta,
    })
}

// ═══════════════════════════════════════════
// Database Persistence
// ═══════════════════════════════════════════

impl Database {
    pub fn save_video_config(&self, config: &VideoConfig) -> Result<(), CoreError> {
        let json = serde_json::to_string(config)?;
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
                let config: VideoConfig = serde_json::from_str(&json)?;
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
        assert_eq!(WhisperModel::Tiny.filename(), "ggml-tiny.bin");
        assert_eq!(WhisperModel::Base.filename(), "ggml-base.bin");
    }

    #[test]
    fn test_default_config() {
        let config = VideoConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.whisper_model, WhisperModel::Base);
        assert_eq!(config.frame_interval_secs, 10);
        assert!(!config.frame_extraction_enabled);
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
            ..VideoConfig::default()
        };
        db.save_video_config(&config).unwrap();

        let loaded = db.load_video_config().unwrap();
        assert!(loaded.enabled);
        assert_eq!(loaded.whisper_model, WhisperModel::Small);
        assert_eq!(loaded.language, Some("zh".into()));
    }

    #[test]
    fn test_load_video_config_default() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::new(dir.path().join("test.db").to_str().unwrap()).unwrap();

        let config = db.load_video_config().unwrap();
        assert!(!config.enabled);
        assert_eq!(config.whisper_model, WhisperModel::Base);
    }
}
