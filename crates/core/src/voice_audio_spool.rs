//! Bounded, recoverable native audio spooling for microphone transcription.
//!
//! The renderer streams ordered PCM16 chunks through a small interface while
//! this module owns filesystem paths, WAV finalization, integrity metadata,
//! expiry, and privacy deletion. Callers never need to retain a full recording
//! in memory or expose a native path across IPC.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::CoreError;

const WAV_HEADER_BYTES: u64 = 44;
const PCM_CHANNELS: u16 = 1;
const PCM_BITS_PER_SAMPLE: u16 = 16;
const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 96_000;
const VOICE_SPOOL_MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct VoiceSpoolLimits {
    pub max_sessions: usize,
    pub max_chunk_bytes: usize,
    pub max_audio_bytes_per_session: u64,
    pub max_total_audio_bytes: u64,
    pub retention: Duration,
}

impl Default for VoiceSpoolLimits {
    fn default() -> Self {
        Self {
            max_sessions: 8,
            max_chunk_bytes: 256 * 1024,
            // More than four hours of mono 16 kHz PCM16 and comfortably above
            // the required 60-minute soak duration.
            max_audio_bytes_per_session: 512 * 1024 * 1024,
            max_total_audio_bytes: 1024 * 1024 * 1024,
            retention: Duration::from_secs(24 * 60 * 60),
        }
    }
}

impl VoiceSpoolLimits {
    fn validate(&self) -> Result<(), CoreError> {
        if self.max_sessions == 0
            || self.max_chunk_bytes == 0
            || self.max_audio_bytes_per_session == 0
            || self.max_total_audio_bytes < self.max_audio_bytes_per_session
        {
            return Err(CoreError::InvalidInput(
                "Voice spool limits must be positive and internally consistent".into(),
            ));
        }
        if self.max_audio_bytes_per_session > u32::MAX as u64 {
            return Err(CoreError::InvalidInput(
                "A PCM WAV spool cannot exceed the RIFF 32-bit data limit".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VoiceSpoolDescriptor {
    pub session_id: String,
    pub audio_bytes: u64,
    pub duration_ms: u64,
    pub sample_rate: u32,
    pub checksum_sha256: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VoiceSpoolStarted {
    pub session_id: String,
    pub sample_rate: u32,
    pub max_chunk_bytes: usize,
    pub max_audio_bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VoiceSpoolProgress {
    pub session_id: String,
    pub accepted_bytes: usize,
    pub audio_bytes: u64,
    pub duration_ms: u64,
    pub next_sequence: u64,
}

#[derive(Debug, Clone)]
pub struct PreparedVoiceSpool {
    pub descriptor: VoiceSpoolDescriptor,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VoiceSpoolManifest {
    version: u32,
    descriptor: VoiceSpoolDescriptor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VoiceSpoolDeletionMarker {
    session_id: String,
    requested_at_ms: u64,
}

struct RecordingSession {
    created_at_ms: u64,
    sample_rate: u32,
    next_sequence: u64,
    audio_bytes: u64,
    hasher: Sha256,
    file: File,
    part_path: PathBuf,
}

enum VoiceSession {
    Recording(RecordingSession),
    Ready(VoiceSpoolDescriptor),
}

pub struct VoiceAudioSpool {
    root: PathBuf,
    limits: VoiceSpoolLimits,
    sessions: Mutex<HashMap<String, VoiceSession>>,
}

impl VoiceAudioSpool {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, CoreError> {
        Self::with_limits(root, VoiceSpoolLimits::default())
    }

    pub fn with_limits(
        root: impl Into<PathBuf>,
        limits: VoiceSpoolLimits,
    ) -> Result<Self, CoreError> {
        limits.validate()?;
        let root = root.into();
        fs::create_dir_all(&root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        }
        let root = fs::canonicalize(root)?;
        let spool = Self {
            root,
            limits,
            sessions: Mutex::new(HashMap::new()),
        };
        spool.recover_ready_sessions()?;
        Ok(spool)
    }

    pub fn start(&self, sample_rate: u32) -> Result<VoiceSpoolStarted, CoreError> {
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(CoreError::InvalidInput(format!(
                "Voice spool sample rate must be between {MIN_SAMPLE_RATE} and {MAX_SAMPLE_RATE} Hz"
            )));
        }

        self.prune_expired()?;
        let mut sessions = self.lock_sessions()?;
        if sessions.len() >= self.limits.max_sessions {
            return Err(CoreError::Conflict(format!(
                "Voice spool session limit ({}) reached",
                self.limits.max_sessions
            )));
        }

        let session_id = Uuid::new_v4().to_string();
        let (part_path, _, _) = self.paths_for_valid_id(&session_id)?;
        let mut open_options = OpenOptions::new();
        open_options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            open_options.mode(0o600);
        }
        let mut file = open_options.open(&part_path)?;
        file.write_all(&[0_u8; WAV_HEADER_BYTES as usize])?;

        sessions.insert(
            session_id.clone(),
            VoiceSession::Recording(RecordingSession {
                created_at_ms: now_ms(),
                sample_rate,
                next_sequence: 0,
                audio_bytes: 0,
                hasher: Sha256::new(),
                file,
                part_path,
            }),
        );
        Ok(VoiceSpoolStarted {
            session_id,
            sample_rate,
            max_chunk_bytes: self.limits.max_chunk_bytes,
            max_audio_bytes: self.limits.max_audio_bytes_per_session,
        })
    }

    pub fn append(
        &self,
        session_id: &str,
        sequence: u64,
        pcm16: &[u8],
    ) -> Result<VoiceSpoolProgress, CoreError> {
        if pcm16.is_empty() {
            return Err(CoreError::InvalidInput(
                "Voice spool chunk cannot be empty".into(),
            ));
        }
        if pcm16.len() % 2 != 0 {
            return Err(CoreError::InvalidInput(
                "Voice spool PCM16 chunk must contain complete samples".into(),
            ));
        }
        if pcm16.len() > self.limits.max_chunk_bytes {
            return Err(CoreError::InvalidInput(format!(
                "Voice spool chunk exceeds {} bytes",
                self.limits.max_chunk_bytes
            )));
        }

        let mut sessions = self.lock_sessions()?;
        let total_audio_bytes = sessions
            .values()
            .map(|session| match session {
                VoiceSession::Recording(recording) => recording.audio_bytes,
                VoiceSession::Ready(descriptor) => descriptor.audio_bytes,
            })
            .sum::<u64>();
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| CoreError::NotFound(format!("Voice spool session {session_id}")))?;
        let VoiceSession::Recording(recording) = session else {
            return Err(CoreError::Conflict(
                "Voice spool session has already been finalized".into(),
            ));
        };
        if sequence != recording.next_sequence {
            return Err(CoreError::Conflict(format!(
                "Voice spool expected sequence {}, received {sequence}",
                recording.next_sequence
            )));
        }
        let next_session_bytes = recording
            .audio_bytes
            .checked_add(pcm16.len() as u64)
            .ok_or_else(|| CoreError::InvalidInput("Voice spool byte count overflow".into()))?;
        if next_session_bytes > self.limits.max_audio_bytes_per_session {
            return Err(CoreError::InvalidInput(format!(
                "Voice spool session exceeds {} bytes",
                self.limits.max_audio_bytes_per_session
            )));
        }
        if total_audio_bytes.saturating_add(pcm16.len() as u64) > self.limits.max_total_audio_bytes
        {
            return Err(CoreError::Conflict(
                "Voice spool total storage limit reached".into(),
            ));
        }

        recording.file.write_all(pcm16)?;
        recording.hasher.update(pcm16);
        recording.audio_bytes = next_session_bytes;
        recording.next_sequence += 1;
        Ok(VoiceSpoolProgress {
            session_id: session_id.to_string(),
            accepted_bytes: pcm16.len(),
            audio_bytes: recording.audio_bytes,
            duration_ms: pcm_duration_ms(recording.audio_bytes, recording.sample_rate),
            next_sequence: recording.next_sequence,
        })
    }

    pub fn finish(&self, session_id: &str) -> Result<VoiceSpoolDescriptor, CoreError> {
        let mut sessions = self.lock_sessions()?;
        if let Some(VoiceSession::Ready(descriptor)) = sessions.get(session_id) {
            return Ok(descriptor.clone());
        }
        let session = sessions
            .remove(session_id)
            .ok_or_else(|| CoreError::NotFound(format!("Voice spool session {session_id}")))?;
        let VoiceSession::Recording(mut recording) = session else {
            unreachable!("ready sessions return before removal")
        };
        if recording.audio_bytes == 0 {
            drop(recording.file);
            let _ = fs::remove_file(&recording.part_path);
            return Err(CoreError::InvalidInput(
                "Voice spool cannot finalize an empty recording".into(),
            ));
        }

        let finalize_result = (|| -> Result<VoiceSpoolDescriptor, CoreError> {
            let header = pcm16_wav_header(recording.sample_rate, recording.audio_bytes)?;
            recording.file.seek(SeekFrom::Start(0))?;
            recording.file.write_all(&header)?;
            recording.file.flush()?;
            recording.file.sync_all()?;
            drop(recording.file);

            let (_, wav_path, manifest_path) = self.paths_for_valid_id(session_id)?;
            fs::rename(&recording.part_path, &wav_path)?;
            let finalized_at_ms = now_ms();
            let descriptor = VoiceSpoolDescriptor {
                session_id: session_id.to_string(),
                audio_bytes: recording.audio_bytes,
                duration_ms: pcm_duration_ms(recording.audio_bytes, recording.sample_rate),
                sample_rate: recording.sample_rate,
                checksum_sha256: format!("{:x}", recording.hasher.finalize()),
                created_at_ms: recording.created_at_ms,
                expires_at_ms: finalized_at_ms
                    .saturating_add(self.limits.retention.as_millis() as u64),
            };
            if let Err(error) = write_manifest(
                &manifest_path,
                &VoiceSpoolManifest {
                    version: VOICE_SPOOL_MANIFEST_VERSION,
                    descriptor: descriptor.clone(),
                },
            ) {
                let _ = fs::remove_file(&wav_path);
                return Err(error);
            }
            Ok(descriptor)
        })();

        match finalize_result {
            Ok(descriptor) => {
                sessions.insert(
                    session_id.to_string(),
                    VoiceSession::Ready(descriptor.clone()),
                );
                Ok(descriptor)
            }
            Err(error) => {
                let (part_path, wav_path, manifest_path) = self.paths_for_valid_id(session_id)?;
                let _ = fs::remove_file(part_path);
                let _ = fs::remove_file(wav_path);
                let _ = fs::remove_file(manifest_path);
                Err(error)
            }
        }
    }

    pub fn prepare_transcription(&self, session_id: &str) -> Result<PreparedVoiceSpool, CoreError> {
        self.prune_expired()?;
        let descriptor = {
            let sessions = self.lock_sessions()?;
            match sessions.get(session_id) {
                Some(VoiceSession::Ready(descriptor)) => descriptor.clone(),
                Some(VoiceSession::Recording(_)) => {
                    return Err(CoreError::Conflict(
                        "Voice spool session must be finalized before transcription".into(),
                    ))
                }
                None => {
                    return Err(CoreError::NotFound(format!(
                        "Voice spool session {session_id}"
                    )))
                }
            }
        };
        let (_, path, _) = self.paths_for_valid_id(session_id)?;
        verify_voice_spool_file(&path, &descriptor)?;
        Ok(PreparedVoiceSpool { descriptor, path })
    }

    pub fn list_ready(&self) -> Result<Vec<VoiceSpoolDescriptor>, CoreError> {
        self.prune_expired()?;
        let sessions = self.lock_sessions()?;
        let mut ready = sessions
            .values()
            .filter_map(|session| match session {
                VoiceSession::Ready(descriptor) => Some(descriptor.clone()),
                VoiceSession::Recording(_) => None,
            })
            .collect::<Vec<_>>();
        ready.sort_by_key(|descriptor| descriptor.created_at_ms);
        Ok(ready)
    }

    pub fn remove(&self, session_id: &str) -> Result<(), CoreError> {
        let mut sessions = self.lock_sessions()?;
        let removed = sessions.remove(session_id);
        let (part_path, wav_path, manifest_path) = self.paths_for_valid_id(session_id)?;
        let deletion_path = self.deletion_marker_path(session_id)?;
        let has_managed_files = part_path.exists()
            || wav_path.exists()
            || manifest_path.exists()
            || deletion_path.exists();
        if removed.is_none() && !has_managed_files {
            return Ok(());
        }
        if let Err(error) = write_deletion_marker(&deletion_path, session_id) {
            if let Some(session) = removed {
                sessions.insert(session_id.to_string(), session);
            }
            return Err(error);
        }
        drop(removed);
        drop(sessions);
        self.complete_pending_deletion(session_id)
    }

    pub fn prune_expired(&self) -> Result<usize, CoreError> {
        let current_ms = now_ms();
        let expired = {
            let sessions = self.lock_sessions()?;
            sessions
                .iter()
                .filter_map(|(session_id, session)| match session {
                    VoiceSession::Ready(descriptor) if descriptor.expires_at_ms <= current_ms => {
                        Some(session_id.clone())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        for session_id in &expired {
            self.remove(session_id)?;
        }
        Ok(expired.len())
    }

    fn recover_ready_sessions(&self) -> Result<(), CoreError> {
        let current_ms = now_ms();
        let mut recovered = HashMap::new();
        let mut retained_wavs = HashSet::new();
        let mut deleting_ids = HashSet::new();

        for entry in fs::read_dir(&self.root)? {
            let path = entry?.path();
            let file_name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            let Some(session_id) = file_name.strip_suffix(".deleting.json") else {
                continue;
            };
            if Uuid::parse_str(session_id).is_err() {
                remove_if_exists(&path)?;
                continue;
            }
            deleting_ids.insert(session_id.to_string());
            let _ = self.complete_pending_deletion(session_id);
        }

        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            let file_name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            if file_name.ends_with(".part") || file_name.ends_with(".tmp") {
                remove_if_exists(&path)?;
            }
        }

        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let manifest_path = entry.path();
            if manifest_path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.ends_with(".deleting.json"))
            {
                continue;
            }
            if manifest_path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let manifest = fs::read(&manifest_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<VoiceSpoolManifest>(&bytes).ok());
            let Some(manifest) = manifest else {
                remove_if_exists(&manifest_path)?;
                continue;
            };
            let descriptor = manifest.descriptor;
            if manifest.version != VOICE_SPOOL_MANIFEST_VERSION {
                remove_if_exists(&manifest_path)?;
                continue;
            }
            if deleting_ids.contains(&descriptor.session_id) {
                continue;
            }
            let valid_id = Uuid::parse_str(&descriptor.session_id).is_ok()
                && manifest_path.file_stem().and_then(|value| value.to_str())
                    == Some(descriptor.session_id.as_str());
            if !valid_id {
                remove_if_exists(&manifest_path)?;
                continue;
            }
            let (_, wav_path, _) = self.paths_for_valid_id(&descriptor.session_id)?;
            let valid_file = fs::metadata(&wav_path)
                .map(|metadata| metadata.len() == descriptor.audio_bytes + WAV_HEADER_BYTES)
                .unwrap_or(false);
            if !valid_file || descriptor.expires_at_ms <= current_ms {
                remove_if_exists(&manifest_path)?;
                remove_if_exists(&wav_path)?;
                continue;
            }
            retained_wavs.insert(wav_path);
            recovered.insert(
                descriptor.session_id.clone(),
                VoiceSession::Ready(descriptor),
            );
        }

        for entry in fs::read_dir(&self.root)? {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) == Some("wav")
                && !retained_wavs.contains(&path)
            {
                remove_if_exists(&path)?;
            }
        }

        if recovered.len() > self.limits.max_sessions {
            let mut ids = recovered
                .iter()
                .filter_map(|(id, session)| match session {
                    VoiceSession::Ready(descriptor) => Some((id.clone(), descriptor.created_at_ms)),
                    VoiceSession::Recording(_) => None,
                })
                .collect::<Vec<_>>();
            ids.sort_by_key(|(_, created_at_ms)| *created_at_ms);
            let remove_count = recovered.len() - self.limits.max_sessions;
            for (id, _) in ids.into_iter().take(remove_count) {
                recovered.remove(&id);
                let deletion_path = self.deletion_marker_path(&id)?;
                write_deletion_marker(&deletion_path, &id)?;
                self.complete_pending_deletion(&id)?;
            }
        }

        *self.lock_sessions()? = recovered;
        Ok(())
    }

    fn paths_for_valid_id(
        &self,
        session_id: &str,
    ) -> Result<(PathBuf, PathBuf, PathBuf), CoreError> {
        Uuid::parse_str(session_id)
            .map_err(|_| CoreError::InvalidInput("Voice spool session id must be a UUID".into()))?;
        Ok((
            self.root.join(format!("{session_id}.wav.part")),
            self.root.join(format!("{session_id}.wav")),
            self.root.join(format!("{session_id}.json")),
        ))
    }

    fn deletion_marker_path(&self, session_id: &str) -> Result<PathBuf, CoreError> {
        Uuid::parse_str(session_id)
            .map_err(|_| CoreError::InvalidInput("Voice spool session id must be a UUID".into()))?;
        Ok(self.root.join(format!("{session_id}.deleting.json")))
    }

    fn complete_pending_deletion(&self, session_id: &str) -> Result<(), CoreError> {
        let (part_path, wav_path, manifest_path) = self.paths_for_valid_id(session_id)?;
        let deletion_path = self.deletion_marker_path(session_id)?;
        remove_if_exists(&part_path)?;
        remove_if_exists(&wav_path)?;
        remove_if_exists(&manifest_path)?;
        remove_if_exists(&deletion_path)?;
        Ok(())
    }

    fn lock_sessions(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<String, VoiceSession>>, CoreError> {
        self.sessions
            .lock()
            .map_err(|_| CoreError::Internal("Voice spool state lock was poisoned".into()))
    }
}

impl Drop for VoiceAudioSpool {
    fn drop(&mut self) {
        let root = self.root.clone();
        let Ok(sessions) = self.sessions.get_mut() else {
            return;
        };
        let recording_ids = sessions
            .iter()
            .filter_map(|(session_id, session)| {
                matches!(session, VoiceSession::Recording(_)).then(|| session_id.clone())
            })
            .collect::<Vec<_>>();
        for session_id in recording_ids {
            let session = sessions.remove(&session_id);
            drop(session);
            let _ = fs::remove_file(root.join(format!("{session_id}.wav.part")));
        }
    }
}

pub fn pcm_duration_ms(audio_bytes: u64, sample_rate: u32) -> u64 {
    if sample_rate == 0 {
        return 0;
    }
    audio_bytes.saturating_mul(1_000)
        / (u64::from(sample_rate) * u64::from(PCM_CHANNELS) * u64::from(PCM_BITS_PER_SAMPLE / 8))
}

fn verify_voice_spool_file(
    path: &Path,
    descriptor: &VoiceSpoolDescriptor,
) -> Result<(), CoreError> {
    if descriptor.duration_ms != pcm_duration_ms(descriptor.audio_bytes, descriptor.sample_rate) {
        return Err(CoreError::Conflict(
            "Voice spool duration metadata failed integrity validation".into(),
        ));
    }
    let expected_header = pcm16_wav_header(descriptor.sample_rate, descriptor.audio_bytes)?;
    let mut file = File::open(path)?;
    let mut actual_header = [0_u8; WAV_HEADER_BYTES as usize];
    file.read_exact(&mut actual_header)?;
    if actual_header != expected_header {
        return Err(CoreError::Conflict(
            "Voice spool WAV header failed integrity validation".into(),
        ));
    }
    let mut hasher = Sha256::new();
    let mut remaining = descriptor.audio_bytes;
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        let read_len = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded read length fits usize");
        file.read_exact(&mut buffer[..read_len])?;
        hasher.update(&buffer[..read_len]);
        remaining -= read_len as u64;
    }
    if format!("{:x}", hasher.finalize()) != descriptor.checksum_sha256 {
        return Err(CoreError::Conflict(
            "Voice spool checksum failed integrity validation".into(),
        ));
    }
    Ok(())
}

fn pcm16_wav_header(sample_rate: u32, audio_bytes: u64) -> Result<[u8; 44], CoreError> {
    let data_size = u32::try_from(audio_bytes)
        .map_err(|_| CoreError::InvalidInput("Voice spool is too large for PCM WAV".into()))?;
    let block_align = PCM_CHANNELS * (PCM_BITS_PER_SAMPLE / 8);
    let byte_rate = sample_rate
        .checked_mul(u32::from(block_align))
        .ok_or_else(|| CoreError::InvalidInput("Voice spool WAV byte rate overflow".into()))?;
    let mut header = [0_u8; 44];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&(36_u32 + data_size).to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16_u32.to_le_bytes());
    header[20..22].copy_from_slice(&1_u16.to_le_bytes());
    header[22..24].copy_from_slice(&PCM_CHANNELS.to_le_bytes());
    header[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    header[32..34].copy_from_slice(&block_align.to_le_bytes());
    header[34..36].copy_from_slice(&PCM_BITS_PER_SAMPLE.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_size.to_le_bytes());
    Ok(header)
}

fn write_manifest(path: &Path, manifest: &VoiceSpoolManifest) -> Result<(), CoreError> {
    let temp_path = path.with_extension("json.tmp");
    remove_if_exists(&temp_path)?;
    let bytes = serde_json::to_vec(manifest)?;
    let mut open_options = OpenOptions::new();
    open_options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open_options.mode(0o600);
    }
    let mut file = open_options.open(&temp_path)?;
    file.write_all(&bytes)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    fs::rename(temp_path, path)?;
    Ok(())
}

fn write_deletion_marker(path: &Path, session_id: &str) -> Result<(), CoreError> {
    if path.exists() {
        return Ok(());
    }
    let marker = VoiceSpoolDeletionMarker {
        session_id: session_id.to_string(),
        requested_at_ms: now_ms(),
    };
    let bytes = serde_json::to_vec(&marker)?;
    let mut open_options = OpenOptions::new();
    open_options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open_options.mode(0o600);
    }
    let mut file = match open_options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(CoreError::Io(error)),
    };
    file.write_all(&bytes)?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<(), CoreError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CoreError::Io(error)),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_limits() -> VoiceSpoolLimits {
        VoiceSpoolLimits {
            max_sessions: 2,
            max_chunk_bytes: 8,
            max_audio_bytes_per_session: 64,
            max_total_audio_bytes: 96,
            retention: Duration::from_secs(60),
        }
    }

    #[test]
    fn ordered_chunks_finalize_a_valid_wav_and_integrity_descriptor() {
        let root = tempfile::tempdir().unwrap();
        let spool = VoiceAudioSpool::with_limits(root.path(), test_limits()).unwrap();
        let started = spool.start(16_000).unwrap();
        let first = [0_u8, 0, 1, 0];
        let second = [2_u8, 0, 3, 0];

        let progress = spool.append(&started.session_id, 0, &first).unwrap();
        assert_eq!(progress.next_sequence, 1);
        assert_eq!(progress.audio_bytes, 4);
        spool.append(&started.session_id, 1, &second).unwrap();
        let descriptor = spool.finish(&started.session_id).unwrap();

        let prepared = spool.prepare_transcription(&started.session_id).unwrap();
        let wav = fs::read(&prepared.path).unwrap();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 8);
        assert_eq!(&wav[44..], &[first, second].concat());
        assert_eq!(descriptor.audio_bytes, 8);
        assert_eq!(
            descriptor.checksum_sha256,
            format!("{:x}", Sha256::digest(&wav[44..]))
        );
    }

    #[test]
    fn transcription_rejects_audio_that_no_longer_matches_its_checksum() {
        let root = tempfile::tempdir().unwrap();
        let spool = VoiceAudioSpool::with_limits(root.path(), test_limits()).unwrap();
        let started = spool.start(16_000).unwrap();
        spool.append(&started.session_id, 0, &[0; 8]).unwrap();
        spool.finish(&started.session_id).unwrap();
        let path = spool.paths_for_valid_id(&started.session_id).unwrap().1;
        let mut file = OpenOptions::new().write(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(WAV_HEADER_BYTES)).unwrap();
        file.write_all(&[1, 0]).unwrap();

        assert!(spool.prepare_transcription(&started.session_id).is_err());
    }

    #[test]
    fn append_enforces_sequence_chunk_and_session_bounds_without_growth() {
        let root = tempfile::tempdir().unwrap();
        let spool = VoiceAudioSpool::with_limits(root.path(), test_limits()).unwrap();
        let started = spool.start(16_000).unwrap();

        assert!(spool.append(&started.session_id, 1, &[0, 0]).is_err());
        assert!(spool.append(&started.session_id, 0, &[0; 10]).is_err());
        assert!(spool.append(&started.session_id, 0, &[0]).is_err());
        let progress = spool.append(&started.session_id, 0, &[0; 8]).unwrap();
        assert_eq!(progress.audio_bytes, 8);
        assert_eq!(progress.next_sequence, 1);
    }

    #[test]
    fn finalized_sessions_recover_and_cancel_deletes_private_audio() {
        let root = tempfile::tempdir().unwrap();
        let session_id = {
            let spool = VoiceAudioSpool::with_limits(root.path(), test_limits()).unwrap();
            let started = spool.start(16_000).unwrap();
            spool.append(&started.session_id, 0, &[0; 8]).unwrap();
            spool.finish(&started.session_id).unwrap();
            started.session_id
        };

        let recovered = VoiceAudioSpool::with_limits(root.path(), test_limits()).unwrap();
        assert_eq!(recovered.list_ready().unwrap()[0].session_id, session_id);
        let path = recovered.prepare_transcription(&session_id).unwrap().path;
        recovered.remove(&session_id).unwrap();
        assert!(!path.exists());
        assert!(recovered.list_ready().unwrap().is_empty());
    }

    #[test]
    fn startup_removes_partial_and_orphan_audio() {
        let root = tempfile::tempdir().unwrap();
        let partial = root.path().join(format!("{}.wav.part", Uuid::new_v4()));
        let orphan = root.path().join(format!("{}.wav", Uuid::new_v4()));
        fs::write(&partial, b"partial").unwrap();
        fs::write(&orphan, b"orphan").unwrap();

        VoiceAudioSpool::with_limits(root.path(), test_limits()).unwrap();

        assert!(!partial.exists());
        assert!(!orphan.exists());
    }

    #[test]
    fn normal_shutdown_deletes_an_unfinished_private_recording() {
        let root = tempfile::tempdir().unwrap();
        let part_path = {
            let spool = VoiceAudioSpool::with_limits(root.path(), test_limits()).unwrap();
            let started = spool.start(16_000).unwrap();
            spool.append(&started.session_id, 0, &[0; 8]).unwrap();
            spool.paths_for_valid_id(&started.session_id).unwrap().0
        };

        assert!(!part_path.exists());
    }

    #[test]
    fn startup_retries_a_durable_deletion_marker_before_recovery() {
        let root = tempfile::tempdir().unwrap();
        let session_id = {
            let spool = VoiceAudioSpool::with_limits(root.path(), test_limits()).unwrap();
            let started = spool.start(16_000).unwrap();
            spool.append(&started.session_id, 0, &[0; 8]).unwrap();
            spool.finish(&started.session_id).unwrap();
            let deletion_path = spool.deletion_marker_path(&started.session_id).unwrap();
            write_deletion_marker(&deletion_path, &started.session_id).unwrap();
            started.session_id
        };

        let recovered = VoiceAudioSpool::with_limits(root.path(), test_limits()).unwrap();

        assert!(recovered.list_ready().unwrap().is_empty());
        assert!(!root.path().join(format!("{session_id}.wav")).exists());
        assert!(!root
            .path()
            .join(format!("{session_id}.deleting.json"))
            .exists());
    }

    #[test]
    fn duration_math_covers_required_long_recording_matrix_without_allocating_audio() {
        for minutes in [1_u64, 5, 15, 30, 60] {
            let bytes = minutes * 60 * 16_000 * 2;
            assert_eq!(pcm_duration_ms(bytes, 16_000), minutes * 60 * 1_000);
        }
    }

    #[test]
    fn session_limit_is_hard_and_cancel_releases_capacity() {
        let root = tempfile::tempdir().unwrap();
        let spool = VoiceAudioSpool::with_limits(root.path(), test_limits()).unwrap();
        let first = spool.start(16_000).unwrap();
        let second = spool.start(16_000).unwrap();
        assert!(spool.start(16_000).is_err());

        spool.remove(&first.session_id).unwrap();
        assert!(spool.start(16_000).is_ok());
        spool.remove(&second.session_id).unwrap();
    }
}
